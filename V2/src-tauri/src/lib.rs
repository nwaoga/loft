// Loft backend (Tauri 2).
//
// M0: window shell + command surface.
// M1: llama.cpp runtime — spawn/supervise the chat + embedding servers.
// M2: model downloads.
// M3: vector store (sqlite-vec) + embedding client.
// M4: indexing — parsers, chunker, incremental indexer, job queue, watchers.
// M5: RAG answers — retrieve, grounded prompt, streamed answer, sources.

mod chunk;
mod embed;
mod indexer;
mod jobs;
mod models;
mod ocr;
mod opener;
mod parse;
mod rag;
mod runtime;
mod store;
mod vision;

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State};

use runtime::{Runtime, ServerKind, ServerState};

// ----------------------------- Shared DTOs ---------------------------------
// camelCase to match src/lib/types.ts.

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct ModelInfo {
    id: String,
    name: String,
    r#type: String,
    params: String,
    quant: String,
    size_bytes: u64,
    ctx: u32,
    installed: bool,
    selected: bool,
    download_progress: Option<f32>,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct RuntimeStatus {
    chat: String,
    embed: String,
    accelerator: String,
    message: Option<String>,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct DownloadProgress {
    id: String,
    progress: f32,
    downloaded: u64,
    total: u64,
}

// ----------------------------- Catalog -------------------------------------

#[derive(Deserialize)]
struct Catalog {
    models: Vec<CatalogEntry>,
}

#[derive(Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
struct CatalogEntry {
    id: String,
    name: String,
    r#type: String, // "chat" | "embed" | "vision"
    params: String,
    quant: String,
    size_bytes: u64,
    ctx: u32,
    file: String,
    #[serde(default)]
    url: String,
    #[serde(default)]
    sha256: String,
    /// Text prepended to document chunks before embedding (model-specific).
    #[serde(default)]
    doc_prefix: String,
    /// Text prepended to questions before embedding (model-specific).
    #[serde(default)]
    query_prefix: String,
    /// A vision model's second file, the image projector, and where to get it.
    #[serde(default)]
    mmproj_file: String,
    #[serde(default)]
    mmproj_url: String,
    #[serde(default)]
    mmproj_sha256: String,
}

fn load_catalog() -> Vec<CatalogEntry> {
    let raw = include_str!("../catalog.json");
    let cat: Catalog = serde_json::from_str(raw).expect("catalog.json is valid");
    cat.models
}

// ----------------------------- App state -----------------------------------

/// Which screen edge the notch hangs from.
#[derive(Serialize, Deserialize, Default, Clone, Copy, PartialEq, Debug)]
#[serde(rename_all = "lowercase")]
enum Placement {
    /// Top edge, centred (the default).
    #[default]
    Top,
    /// Right edge, centred vertically.
    Right,
}

/// What is remembered between launches (selection.json).
#[derive(Serialize, Deserialize, Default, Clone)]
struct Selection {
    chat: Option<String>,
    embed: Option<String>,
    #[serde(default)]
    placement: Placement,
    /// The display the notch lives on (its system name); `None` = the main one.
    #[serde(default)]
    monitor: Option<String>,
    /// The picture (vision) model; `None` = pictures are not described.
    #[serde(default)]
    vision: Option<String>,
}

struct AppState {
    runtime: Mutex<Runtime>,
    models_dir: PathBuf,
    config_path: PathBuf,
    catalog: Vec<CatalogEntry>,
    selection: Mutex<Selection>,
    downloading: Mutex<HashSet<String>>,
    /// Vector store (loft.db), shared with indexing jobs.
    store: Arc<Mutex<store::Store>>,
    /// Indexing jobs, progress and file watchers.
    index: jobs::IndexState,
    /// Bumped by every new question and by "stop": an answer whose number is
    /// no longer current stops streaming.
    ask_gen: Arc<AtomicU64>,
}

impl AppState {
    fn entry(&self, id: &str) -> Option<&CatalogEntry> {
        self.catalog.iter().find(|e| e.id == id)
    }
    fn model_path(&self, e: &CatalogEntry) -> PathBuf {
        self.models_dir.join(&e.file)
    }
    fn is_installed(&self, e: &CatalogEntry) -> bool {
        self.model_path(e).exists() && (e.mmproj_file.is_empty() || self.models_dir.join(&e.mmproj_file).exists())
    }
}

fn load_selection(path: &PathBuf) -> Selection {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn save_selection(path: &PathBuf, sel: &Selection) {
    if let Ok(s) = serde_json::to_string_pretty(sel) {
        let _ = std::fs::write(path, s);
    }
}

// ----------------------------- Status helpers ------------------------------

fn status_payload(state: &AppState) -> RuntimeStatus {
    let rt = state.runtime.lock().unwrap();
    let message = rt.message.clone().or_else(|| match (rt.chat_state, rt.embed_state) {
        (ServerState::Stopped, ServerState::Stopped) => {
            Some("Select a chat and an embedding model to start.".into())
        }
        _ => None,
    });
    RuntimeStatus {
        chat: rt.chat_state.as_str().to_string(),
        embed: rt.embed_state.as_str().to_string(),
        accelerator: rt.accelerator.as_str().to_string(),
        message,
    }
}

fn emit_status(app: &AppHandle) {
    let state = app.state::<AppState>();
    let _ = app.emit("runtime:status", status_payload(&state));
}

/// Wait until the `kind` server on `port` has loaded its model. Gives up at
/// once if the process exits, instead of waiting out the whole timeout.
async fn wait_ready(app: &AppHandle, kind: ServerKind, port: u16, timeout: Duration) -> bool {
    let deadline = tokio::time::Instant::now() + timeout;
    while tokio::time::Instant::now() < deadline {
        if runtime::is_healthy(port).await {
            return true;
        }
        let exited = {
            let state = app.state::<AppState>();
            let mut rt = state.runtime.lock().unwrap();
            rt.exited(kind)
        };
        if exited {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    false
}

/// Watch the two servers; if one dies while it should be running, restart it
/// with the same model. Gives up after repeated crashes in a short time.
fn spawn_supervisor(app: AppHandle) {
    const MAX_CRASHES: u32 = 3;
    const CRASH_WINDOW: Duration = Duration::from_secs(300);
    tauri::async_runtime::spawn(async move {
        let mut crashes: [(u32, Option<std::time::Instant>); 2] = [(0, None), (0, None)];
        loop {
            tokio::time::sleep(Duration::from_secs(3)).await;
            for (i, kind) in [ServerKind::Embed, ServerKind::Chat].into_iter().enumerate() {
                let crashed_model = {
                    let state = app.state::<AppState>();
                    let mut rt = state.runtime.lock().unwrap();
                    if rt.state_of(kind) == ServerState::Ready && rt.exited(kind) {
                        let id = rt.model_id_of(kind);
                        rt.stop(kind);
                        rt.set_state(kind, ServerState::Error);
                        id
                    } else {
                        None
                    }
                };
                let Some(id) = crashed_model else { continue };

                let (count, last) = &mut crashes[i];
                if last.map(|t| t.elapsed() > CRASH_WINDOW).unwrap_or(true) {
                    *count = 0;
                }
                *count += 1;
                *last = Some(std::time::Instant::now());
                if *count > MAX_CRASHES {
                    eprintln!("Loft: the {} server keeps crashing; not restarting it", kind.as_str());
                    let state = app.state::<AppState>();
                    state.runtime.lock().unwrap().message = Some(format!(
                        "The {} model keeps crashing. Try selecting it again, or pick a smaller one, in Settings.",
                        kind.as_str()
                    ));
                    emit_status(&app);
                    continue;
                }
                eprintln!("Loft: the {} server stopped unexpectedly; restarting it ({count}/{MAX_CRASHES})", kind.as_str());
                emit_status(&app);
                let _ = start_server_for(app.clone(), kind, id).await;
            }
        }
    });
}

/// Spawn (or replace) the server for `kind` using model `id`, then health-check.
async fn start_server_for(app: AppHandle, kind: ServerKind, id: String) -> Result<(), String> {
    let (path, ctx) = {
        let state = app.state::<AppState>();
        let entry = state
            .entry(&id)
            .cloned()
            .ok_or_else(|| format!("unknown model: {id}"))?;
        (state.model_path(&entry), entry.ctx)
    };
    if !path.exists() {
        return Err(format!("model not downloaded: {id}"));
    }

    let mut healthy = false;
    for _attempt in 0..2 {
        // Spawn (quick, synchronous) — do not hold the lock across await.
        let port = {
            let state = app.state::<AppState>();
            let mut rt = state.runtime.lock().unwrap();
            rt.spawn(kind, &path, &id, ctx)?
        };
        emit_status(&app);

        // Wait for the model to load (or the process to die).
        healthy = wait_ready(&app, kind, port, Duration::from_secs(180)).await;
        if healthy {
            break;
        }
        // A GPU build that cannot start here (driver, VRAM): once more on CPU.
        let retry_on_cpu = {
            let state = app.state::<AppState>();
            let mut rt = state.runtime.lock().unwrap();
            rt.stop(kind);
            rt.fall_back_to_cpu()
        };
        if !retry_on_cpu {
            break;
        }
        eprintln!("Loft: GPU start failed for the {} server; retrying on CPU", kind.as_str());
    }
    {
        let state = app.state::<AppState>();
        let mut rt = state.runtime.lock().unwrap();
        rt.set_state(kind, if healthy { ServerState::Ready } else { ServerState::Error });
    }
    emit_status(&app);
    if !healthy {
        return Err(format!("{} server failed to become ready", kind.as_str()));
    }
    if kind == ServerKind::Embed {
        // New or changed embedding model: (re)build what needs indexing.
        jobs::on_embed_ready(app.clone()).await;
    }
    Ok(())
}

// ----------------------------- Commands ------------------------------------

#[tauri::command]
fn get_runtime_status(state: State<AppState>) -> RuntimeStatus {
    status_payload(&state)
}

#[tauri::command]
fn list_models(state: State<AppState>) -> Vec<ModelInfo> {
    let sel = state.selection.lock().unwrap().clone();
    state
        .catalog
        .iter()
        .map(|e| {
            let selected = match e.r#type.as_str() {
                "embed" => sel.embed.as_deref() == Some(e.id.as_str()),
                "vision" => sel.vision.as_deref() == Some(e.id.as_str()),
                _ => sel.chat.as_deref() == Some(e.id.as_str()),
            };
            ModelInfo {
                id: e.id.clone(),
                name: e.name.clone(),
                r#type: e.r#type.clone(),
                params: e.params.clone(),
                quant: e.quant.clone(),
                size_bytes: e.size_bytes,
                ctx: e.ctx,
                installed: state.is_installed(e),
                selected,
                download_progress: None,
            }
        })
        .collect()
}

#[tauri::command]
async fn select_model(app: AppHandle, id: String) -> Result<(), String> {
    let kind = {
        let state = app.state::<AppState>();
        let entry = state
            .entry(&id)
            .ok_or_else(|| format!("unknown model: {id}"))?;
        if !state.is_installed(entry) {
            return Err(format!("{} is not downloaded yet", entry.name));
        }
        if entry.r#type == "vision" {
            return set_picture_model(app.clone(), Some(id));
        }
        let kind = if entry.r#type == "embed" {
            ServerKind::Embed
        } else {
            ServerKind::Chat
        };
        // Persist selection.
        let mut sel = state.selection.lock().unwrap();
        match kind {
            ServerKind::Embed => sel.embed = Some(id.clone()),
            ServerKind::Chat => sel.chat = Some(id.clone()),
        }
        save_selection(&state.config_path, &sel);
        kind
    };
    start_server_for(app, kind, id).await
}

/// Turn picture descriptions on (with the model `id`) or off (`None`), and
/// read the indexed pictures again so the index matches the setting.
#[tauri::command]
fn set_picture_model(app: AppHandle, id: Option<String>) -> Result<(), String> {
    {
        let state = app.state::<AppState>();
        if let Some(id) = &id {
            let entry = state
                .entry(id)
                .filter(|e| e.r#type == "vision")
                .ok_or_else(|| format!("unknown picture model: {id}"))?;
            if !state.is_installed(entry) {
                return Err(format!("{} is not downloaded yet", entry.name));
            }
        }
        let mut sel = state.selection.lock().unwrap();
        if sel.vision == id {
            return Ok(());
        }
        sel.vision = id;
        save_selection(&state.config_path, &sel);
    }
    jobs::reread_pictures(&app).map(|_| ())
}

#[tauri::command]
async fn download_model(app: AppHandle, id: String) -> Result<(), String> {
    // A model is one file, or two for a vision model (model + image projector).
    let (files, catalog_size) = {
        let state = app.state::<AppState>();
        let e = state
            .entry(&id)
            .cloned()
            .ok_or_else(|| format!("unknown model: {id}"))?;
        if e.url.is_empty() || (!e.mmproj_file.is_empty() && e.mmproj_url.is_empty()) {
            return Err("no download URL configured for this model".into());
        }
        let mut files = vec![(e.url.clone(), state.model_path(&e), e.sha256.clone())];
        if !e.mmproj_file.is_empty() {
            files.push((e.mmproj_url.clone(), state.models_dir.join(&e.mmproj_file), e.mmproj_sha256.clone()));
        }
        (files, e.size_bytes)
    };
    if files.iter().all(|(_, dest, _)| dest.exists()) {
        return Ok(()); // already installed
    }

    // Guard against duplicate concurrent downloads of the same model.
    {
        let state = app.state::<AppState>();
        let mut dl = state.downloading.lock().unwrap();
        if !dl.insert(id.clone()) {
            return Err("already downloading".into());
        }
    }

    let result = async {
        let app_e = app.clone();
        let id_e = id.clone();
        let mut last = -1.0f32;
        let several = files.len() > 1;
        let mut done_before = 0u64; // bytes of the files already complete
        for (url, dest, sha) in &files {
            let size = |path: &std::path::Path| std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
            if dest.exists() {
                done_before += size(dest);
                continue;
            }
            let sha_opt = if sha.is_empty() { None } else { Some(sha.as_str()) };
            models::download_model_file(url, dest, sha_opt, |downloaded, total| {
                // Progress is over all of the model's files together.
                let (downloaded, total) = if several {
                    (done_before + downloaded, catalog_size.max(done_before + total))
                } else {
                    (downloaded, total)
                };
                let pct = if total > 0 {
                    downloaded as f32 / total as f32
                } else {
                    0.0
                };
                // Throttle UI events to ~1% steps.
                if pct - last >= 0.01 || pct >= 1.0 {
                    last = pct;
                    let _ = app_e.emit(
                        "download:progress",
                        DownloadProgress {
                            id: id_e.clone(),
                            progress: pct,
                            downloaded,
                            total,
                        },
                    );
                }
            })
            .await?;
            done_before += size(dest);
        }
        Ok(())
    }
    .await;

    // Clear the in-flight guard.
    {
        let state = app.state::<AppState>();
        state.downloading.lock().unwrap().remove(&id);
    }

    match &result {
        Ok(()) => {
            let _ = app.emit(
                "download:progress",
                DownloadProgress {
                    id: id.clone(),
                    progress: 1.0,
                    downloaded: 0,
                    total: 0,
                },
            );
        }
        Err(e) => {
            let _ = app.emit(
                "download:error",
                serde_json::json!({ "id": id, "error": e }),
            );
        }
    }
    result
}

/// Port of the `kind` server, if it is up and its model has loaded.
fn ready_port(rt: &Runtime, kind: ServerKind) -> Option<u16> {
    if rt.state_of(kind) == ServerState::Ready {
        rt.port_of(kind)
    } else {
        None
    }
}

/// What `ask` resolves with once the answer has ended.
#[derive(Serialize)]
struct AskDone {
    sources: Vec<rag::Source>,
    /// Context usage; absent when the model was not called or was stopped.
    stats: Option<rag::Stats>,
}

/// Answer a question from the indexed folders. `history` is the conversation
/// so far (the UI keeps it), which makes the question a follow-up; `options`
/// are the chat options that shape the answer. Emits `ask:sources` once
/// retrieval is done and `ask:token` for each piece of the streamed answer;
/// resolves with the sources when the answer is complete (or was stopped).
#[tauri::command]
async fn ask(
    app: AppHandle,
    question: String,
    history: Option<Vec<rag::Turn>>,
    options: Option<rag::Options>,
) -> Result<AskDone, String> {
    let history = history.unwrap_or_default();
    let options = options.unwrap_or_default();
    let (store, endpoints, gen, my_gen) = {
        let state = app.state::<AppState>();
        let rt = state.runtime.lock().unwrap();
        let embed_port = ready_port(&rt, ServerKind::Embed).ok_or_else(|| {
            "The embedding model isn't running. Download and select one in Settings.".to_string()
        })?;
        let chat_port = ready_port(&rt, ServerKind::Chat).ok_or_else(|| {
            "The chat model isn't running. Download and select one in Settings.".to_string()
        })?;
        let query_prefix = rt
            .embed
            .as_ref()
            .and_then(|s| state.entry(&s.model_id))
            .map(|e| e.query_prefix.clone())
            .unwrap_or_default();
        let gen = state.ask_gen.clone();
        let my_gen = gen.fetch_add(1, Ordering::SeqCst) + 1;
        (
            state.store.clone(),
            rag::Endpoints {
                embed_port,
                chat_port,
                query_prefix,
                options,
            },
            gen,
            my_gen,
        )
    };

    let app_sources = app.clone();
    let app_tokens = app.clone();
    let outcome = rag::answer(
        &store,
        &endpoints,
        &question,
        &history,
        move || gen.load(Ordering::SeqCst) != my_gen,
        move |sources| {
            let _ = app_sources.emit("ask:sources", sources);
        },
        move |token| {
            let _ = app_tokens.emit("ask:token", token);
        },
    )
    .await?;
    Ok(AskDone {
        sources: outcome.sources,
        stats: outcome.stats,
    })
}

/// Stop the answer that is currently streaming, if any.
#[tauri::command]
fn cancel_ask(state: State<AppState>) {
    state.ask_gen.fetch_add(1, Ordering::SeqCst);
}

/// Open a cited file (documents) or show it in the file manager (everything
/// else). Only files that are in the index can be opened this way.
#[tauri::command]
fn open_source(state: State<AppState>, path: String, page: Option<u32>) -> Result<(), String> {
    if !state.store.lock().unwrap().is_indexed(&path)? {
        return Err("That file is no longer in the index.".into());
    }
    opener::open_or_reveal(std::path::Path::new(&path), page)
}

// ----------------------------- Helpers -------------------------------------

/// Window size (logical px) when showing just the pill / the full panel. The
/// window is transparent but still swallows clicks, so it is kept only as big
/// as what is drawn, plus a margin wide enough for the notch's corners and
/// drop shadow (a shadow that reaches the window edge is cut off in a visible
/// rectangle). Keep in step with the sizes in src/App.svelte and src/app.css.
const COLLAPSED_SIZE: (f64, f64) = (320.0, 76.0); // pill 236x36 + corners, hover growth, shadow
const EXPANDED_SIZE: (f64, f64) = (540.0, 580.0); // panel 460x520 + corners and shadow
// The same, hanging from the right edge: the pill stands upright (36x190) and
// the margins are above, below and to the left.
const COLLAPSED_SIZE_RIGHT: (f64, f64) = (76.0, 274.0);
const EXPANDED_SIZE_RIGHT: (f64, f64) = (520.0, 600.0);

/// Window size (logical px) for a placement and state.
fn notch_size(placement: Placement, expanded: bool) -> (f64, f64) {
    match (placement, expanded) {
        (Placement::Top, false) => COLLAPSED_SIZE,
        (Placement::Top, true) => EXPANDED_SIZE,
        (Placement::Right, false) => COLLAPSED_SIZE_RIGHT,
        (Placement::Right, true) => EXPANDED_SIZE_RIGHT,
    }
}

/// Top-left corner (physical px) of a `size` window docked at `placement` on
/// a monitor at `origin` measuring `monitor`: centred along its edge.
fn notch_origin(placement: Placement, origin: (i32, i32), monitor: (i32, i32), size: (i32, i32)) -> (i32, i32) {
    match placement {
        Placement::Top => (origin.0 + (monitor.0 - size.0) / 2, origin.1),
        Placement::Right => (origin.0 + monitor.0 - size.0, origin.1 + (monitor.1 - size.1) / 2),
    }
}

/// One attached display, for the picker in Settings.
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
struct MonitorInfo {
    /// The system's name for it; what is saved.
    id: String,
    /// "Display 2".
    label: String,
    width: u32,
    height: u32,
    primary: bool,
    /// The one the notch is on now.
    selected: bool,
}

fn monitor_id(monitor: &tauri::Monitor) -> String {
    monitor
        .name()
        .cloned()
        .unwrap_or_else(|| format!("{},{}", monitor.position().x, monitor.position().y))
}

/// A readable name: Windows calls displays `\\.\DISPLAY2`, which becomes
/// "Display 2"; other systems' names ("Built-in Retina Display", "HDMI-1")
/// are shown as they are.
fn monitor_label(id: &str, index: usize) -> String {
    let name = id.trim_start_matches(|c| matches!(c, '\\' | '.'));
    let digits = name.trim_start_matches(|c: char| !c.is_ascii_digit());
    if name.to_ascii_uppercase().starts_with("DISPLAY") && !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit()) {
        format!("Display {digits}")
    } else if name.is_empty() || name.contains(',') {
        format!("Display {}", index + 1)
    } else {
        name.to_string()
    }
}

/// The display the notch should be on: the chosen one if it is attached,
/// otherwise the main display. Never "whichever the window is mostly on":
/// while it is being resized at a screen edge, that can be the neighbour.
fn target_monitor(window: &tauri::WebviewWindow, wanted: Option<&str>) -> Option<tauri::Monitor> {
    if let Some(wanted) = wanted {
        let found = window
            .available_monitors()
            .unwrap_or_default()
            .into_iter()
            .find(|m| monitor_id(m) == wanted);
        if found.is_some() {
            return found;
        }
    }
    window
        .primary_monitor()
        .ok()
        .flatten()
        .or_else(|| window.current_monitor().ok().flatten())
}

/// Resize the notch window for its state and dock it on its display:
/// top-centre, or centred on the right edge.
fn size_notch(window: &tauri::WebviewWindow, expanded: bool, placement: Placement, monitor: Option<&str>) {
    let (width, height) = notch_size(placement, expanded);
    let Some(monitor) = target_monitor(window, monitor) else {
        let _ = window.set_size(tauri::LogicalSize::new(width, height));
        return;
    };
    let scale = monitor.scale_factor();
    let size = ((width * scale).round() as i32, (height * scale).round() as i32);
    let (x, y) = notch_origin(
        placement,
        (monitor.position().x, monitor.position().y),
        (monitor.size().width as i32, monitor.size().height as i32),
        size,
    );
    let position = tauri::PhysicalPosition::new(x, y);
    let new_size = tauri::PhysicalSize::new(size.0 as u32, size.1 as u32);
    // Order matters at a screen edge: growing in place first would spill onto
    // the next display, and so would moving before shrinking.
    let growing = window
        .outer_size()
        .map(|cur| new_size.width > cur.width || new_size.height > cur.height)
        .unwrap_or(true);
    if growing {
        let _ = window.set_position(position);
        let _ = window.set_size(new_size);
    } else {
        let _ = window.set_size(new_size);
        let _ = window.set_position(position);
    }
}

/// Dock the window according to the saved placement and display.
fn dock(window: &tauri::WebviewWindow, state: &AppState, expanded: bool) {
    let (placement, monitor) = {
        let sel = state.selection.lock().unwrap();
        (sel.placement, sel.monitor.clone())
    };
    size_notch(window, expanded, placement, monitor.as_deref());
}

/// Called by the UI just before it expands and just after it collapses.
#[tauri::command]
fn set_expanded(window: tauri::WebviewWindow, state: State<AppState>, expanded: bool) {
    dock(&window, &state, expanded);
}

/// Where the notch is docked: "top" or "right".
#[tauri::command]
fn get_placement(state: State<AppState>) -> Placement {
    state.selection.lock().unwrap().placement
}

/// Dock the notch at another edge and remember it. `expanded` is the panel's
/// current state, so the window is moved at the right size.
#[tauri::command]
fn set_placement(
    window: tauri::WebviewWindow,
    state: State<AppState>,
    placement: Placement,
    expanded: bool,
) {
    {
        let mut sel = state.selection.lock().unwrap();
        sel.placement = placement;
        save_selection(&state.config_path, &sel);
    }
    dock(&window, &state, expanded);
}

/// The attached displays, with the one the notch is on marked.
#[tauri::command]
fn list_monitors(window: tauri::WebviewWindow, state: State<AppState>) -> Vec<MonitorInfo> {
    let wanted = state.selection.lock().unwrap().monitor.clone();
    let current = target_monitor(&window, wanted.as_deref()).map(|m| monitor_id(&m));
    let primary = window.primary_monitor().ok().flatten().map(|m| monitor_id(&m));
    window
        .available_monitors()
        .unwrap_or_default()
        .iter()
        .enumerate()
        .map(|(index, m)| {
            let id = monitor_id(m);
            MonitorInfo {
                label: monitor_label(&id, index),
                width: m.size().width,
                height: m.size().height,
                primary: primary.as_deref() == Some(id.as_str()),
                selected: current.as_deref() == Some(id.as_str()),
                id,
            }
        })
        .collect()
}

/// Move the notch to another display and remember it.
#[tauri::command]
fn set_monitor(window: tauri::WebviewWindow, state: State<AppState>, id: String, expanded: bool) {
    {
        let mut sel = state.selection.lock().unwrap();
        sel.monitor = Some(id);
        save_selection(&state.config_path, &sel);
    }
    dock(&window, &state, expanded);
}

/// Bring the notch forward and toggle the panel (hotkey, tray click).
fn summon(app: &AppHandle) {
    if let Some(win) = app.get_webview_window("notch") {
        let _ = win.show();
        let _ = win.set_focus();
        let _ = win.emit("loft://toggle", ());
    }
}

#[tauri::command]
fn quit_app(app: AppHandle) {
    app.exit(0); // RunEvent::Exit stops the llama-servers
}

#[tauri::command]
fn get_autostart(app: AppHandle) -> bool {
    use tauri_plugin_autostart::ManagerExt;
    app.autolaunch().is_enabled().unwrap_or(false)
}

/// Turn "launch at login" on or off; returns the resulting state.
#[tauri::command]
fn set_autostart(app: AppHandle, enabled: bool) -> Result<bool, String> {
    use tauri_plugin_autostart::ManagerExt;
    if cfg!(debug_assertions) {
        // Would register this development build's exe with the OS.
        return Err("Launch at login is only available in the installed app.".into());
    }
    let auto = app.autolaunch();
    let result = if enabled { auto.enable() } else { auto.disable() };
    result.map_err(|e| e.to_string())?;
    auto.is_enabled().map_err(|e| e.to_string())
}

/// Dev aid: lets a script run with LOFT_DEV_EVAL report back. Prints the
/// message and the window geometry to stderr, and returns the geometry as
/// `[width, height, x, y]` (physical px) so the script can check it. Does
/// nothing in release builds.
#[tauri::command]
fn dev_log(window: tauri::WebviewWindow, msg: String) -> Vec<i32> {
    #[cfg(debug_assertions)]
    {
        let size = window.outer_size().unwrap_or_default();
        let pos = window.outer_position().unwrap_or_default();
        eprintln!("[dev] {msg} | window {}x{} at {},{}", size.width, size.height, pos.x, pos.y);
        vec![size.width as i32, size.height as i32, pos.x, pos.y]
    }
    #[cfg(not(debug_assertions))]
    {
        let _ = (window, msg);
        Vec::new()
    }
}

fn build_tray(app: &tauri::App) -> tauri::Result<()> {
    use tauri::menu::{Menu, MenuItem};
    use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};

    let show = MenuItem::with_id(app, "show", "Show / hide Loft", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit Loft", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&show, &quit])?;
    let mut tray = TrayIconBuilder::with_id("loft")
        .tooltip("Loft")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "show" => summon(app),
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                summon(tray.app_handle());
            }
        });
    if let Some(icon) = app.default_window_icon() {
        tray = tray.icon(icon.clone());
    }
    tray.build(app)?;
    Ok(())
}

// ----------------------------- App entry -----------------------------------

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_store::Builder::new().build())
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None,
        ))
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(|app, _shortcut, event| {
                    use tauri_plugin_global_shortcut::ShortcutState;
                    if event.state() == ShortcutState::Pressed {
                        summon(app);
                    }
                })
                .build(),
        )
        .setup(|app| {
            // Resolve app data dirs and load persisted selection + catalog.
            let data = app.path().app_data_dir()?;
            let models_dir = data.join("models");
            std::fs::create_dir_all(&models_dir).ok();
            // Dev aid (debug builds): LOFT_DATA_DIR keeps the index and the
            // settings in another folder, so a test run neither sees nor
            // changes the user's own folders and choices. The models are
            // still read from the real folder (they are large, and read-only).
            #[cfg(debug_assertions)]
            let data = match std::env::var_os("LOFT_DATA_DIR") {
                Some(dir) => {
                    let dir = PathBuf::from(dir);
                    std::fs::create_dir_all(&dir).ok();
                    eprintln!("[dev] index and settings are in {}", dir.display());
                    dir
                }
                None => data,
            };
            let config_path = data.join("selection.json");
            let selection = load_selection(&config_path);
            let (placement, monitor) = (selection.placement, selection.monitor.clone());
            let catalog = load_catalog();
            let store = store::Store::open(&data.join("loft.db"))
                .map_err(|e| -> Box<dyn std::error::Error> { e.into() })?;
            // Files that had no readable text under older parsers get another go.
            if let Ok(n) = store.upgrade_parsers(parse::PARSER_VERSION) {
                if n > 0 {
                    eprintln!("Loft: {n} files with no text will be read again (better parsers)");
                }
            }
            // Dev aid (debug builds): LOFT_SEED_FOLDER registers a folder at
            // launch, since a test cannot click through the native folder picker.
            #[cfg(debug_assertions)]
            if let Ok(folder) = std::env::var("LOFT_SEED_FOLDER") {
                let known = store.list_folders().unwrap_or_default().iter().any(|f| f.path == folder);
                if !known {
                    match store.add_folder(&folder) {
                        Ok(_) => eprintln!("[dev] added folder {folder}"),
                        Err(e) => eprintln!("[dev] could not add folder {folder}: {e}"),
                    }
                }
            }
            let (index, watch_rx) = jobs::IndexState::new();

            // In an installed app the llama.cpp builds are bundled resources.
            let bundled_binaries = app.path().resource_dir().ok().map(|d| d.join("binaries"));

            app.manage(AppState {
                runtime: Mutex::new(Runtime::with_extra_root(bundled_binaries)),
                models_dir,
                config_path,
                catalog,
                selection: Mutex::new(selection),
                downloading: Mutex::new(HashSet::new()),
                store: Arc::new(Mutex::new(store)),
                index,
                ask_gen: Arc::new(AtomicU64::new(0)),
            });

            if let Some(win) = app.get_webview_window("notch") {
                size_notch(&win, false, placement, monitor.as_deref());
            }

            // Tray icon: the notch has no title bar, so this is where Quit lives
            // (also in the panel's settings menu). Not fatal if the desktop has
            // no tray (some Linux setups).
            if let Err(e) = build_tray(app) {
                eprintln!("Loft: could not create the tray icon: {e}");
            }

            // Summon hotkey (Ctrl/Cmd+Shift+Space).
            {
                use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Modifiers, Shortcut};
                let accel = if cfg!(target_os = "macos") {
                    Modifiers::SUPER | Modifiers::SHIFT
                } else {
                    Modifiers::CONTROL | Modifiers::SHIFT
                };
                let shortcut = Shortcut::new(Some(accel), Code::Space);
                if let Err(e) = app.global_shortcut().register(shortcut) {
                    eprintln!("Loft: could not register global shortcut: {e}");
                }
            }

            // File-watcher events -> debounced index requests.
            jobs::spawn_debouncer(app.handle().clone(), watch_rx);

            // Auto-start servers for any already-installed selected models.
            // (When the embedding server comes up, folders are brought up to date.)
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                let (chat_id, embed_id) = {
                    let state = handle.state::<AppState>();
                    let sel = state.selection.lock().unwrap();
                    (sel.chat.clone(), sel.embed.clone())
                };
                if let Some(id) = embed_id {
                    let _ = start_server_for(handle.clone(), ServerKind::Embed, id).await;
                }
                if let Some(id) = chat_id {
                    let _ = start_server_for(handle.clone(), ServerKind::Chat, id).await;
                }

                // From here on, restart a server if its process dies.
                spawn_supervisor(handle.clone());

                // Dev aid (debug builds): LOFT_DEV_EVAL=<path to a .js file> runs
                // that script inside the notch's webview once the servers are
                // up, so the real UI can be driven and checked without a human.
                // The script reports back through the `dev_log` command.
                #[cfg(debug_assertions)]
                if let Ok(script) = std::env::var("LOFT_DEV_EVAL") {
                    tokio::time::sleep(Duration::from_secs(8)).await; // let indexing settle
                    match (std::fs::read_to_string(&script), handle.get_webview_window("notch")) {
                        (Ok(js), Some(win)) => {
                            eprintln!("[dev] running {script}");
                            let _ = win.eval(&js);
                        }
                        (Err(e), _) => eprintln!("[dev] cannot read {script}: {e}"),
                        _ => eprintln!("[dev] no notch window"),
                    }
                }
            });

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_runtime_status,
            list_models,
            select_model,
            set_picture_model,
            download_model,
            jobs::list_folders,
            jobs::add_folder,
            jobs::reindex_folder,
            jobs::delete_index,
            jobs::remove_folder,
            ask,
            cancel_ask,
            open_source,
            set_expanded,
            get_placement,
            set_placement,
            list_monitors,
            set_monitor,
            quit_app,
            get_autostart,
            set_autostart,
            dev_log,
        ])
        .build(tauri::generate_context!())
        .expect("error while building Loft");

    app.run(|handle, event| {
        if let tauri::RunEvent::Exit = event {
            if let Some(state) = handle.try_state::<AppState>() {
                if let Ok(mut rt) = state.runtime.lock() {
                    rt.stop_all();
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notch_is_docked_centred_on_its_edge() {
        // A 1920x1080 monitor whose top-left corner is at (100, 50).
        let (origin, monitor) = ((100, 50), (1920, 1080));
        assert_eq!(notch_origin(Placement::Top, origin, monitor, (320, 76)), (900, 50));
        assert_eq!(notch_origin(Placement::Top, origin, monitor, (540, 580)), (790, 50));
        // Right edge: flush with the edge, centred vertically.
        assert_eq!(notch_origin(Placement::Right, origin, monitor, (76, 274)), (1944, 453));
        assert_eq!(notch_origin(Placement::Right, origin, monitor, (520, 600)), (1500, 290));

        // Either way the panel's window is the larger one.
        for placement in [Placement::Top, Placement::Right] {
            let (cw, ch) = notch_size(placement, false);
            let (ew, eh) = notch_size(placement, true);
            assert!(ew > cw && eh > ch, "{placement:?}: the panel is larger than the pill");
        }
    }

    #[test]
    fn displays_get_readable_names() {
        assert_eq!(monitor_label(r"\\.\DISPLAY1", 0), "Display 1");
        assert_eq!(monitor_label(r"\\.\DISPLAY12", 3), "Display 12");
        assert_eq!(monitor_label("Built-in Retina Display", 0), "Built-in Retina Display");
        assert_eq!(monitor_label("HDMI-1", 1), "HDMI-1");
        // No name from the system: numbered by position in the list.
        assert_eq!(monitor_label("", 1), "Display 2");
        assert_eq!(monitor_label("1920,0", 2), "Display 3");
    }

    #[test]
    fn placement_is_remembered_and_defaults_to_top() {
        // A selection.json written before placement existed still loads.
        let old: Selection = serde_json::from_str(r#"{"chat":"a","embed":"b"}"#).unwrap();
        assert_eq!(old.placement, Placement::Top);
        assert_eq!(old.monitor, None);
        assert_eq!(old.chat.as_deref(), Some("a"));

        let saved = serde_json::to_string(&Selection {
            placement: Placement::Right,
            ..old
        })
        .unwrap();
        assert!(saved.contains(r#""placement":"right""#));
        let back: Selection = serde_json::from_str(&saved).unwrap();
        assert_eq!(back.vision, None, "picture descriptions are off unless chosen");
        assert_eq!(back.placement, Placement::Right);
        assert_eq!(serde_json::from_str::<Placement>(r#""top""#).unwrap(), Placement::Top);
    }
}
