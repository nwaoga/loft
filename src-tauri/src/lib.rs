// Loft backend (Tauri 2).
//
// M0: window shell + command surface.
// M1: llama.cpp runtime — spawn/supervise the chat + embedding servers.
// M2: model downloads.
// M3: vector store (sqlite-vec) + embedding client.
// M4: indexing — parsers, chunker, incremental indexer, job queue, watchers.
// M5: RAG answers — retrieve, grounded prompt, streamed answer, sources.

mod bench;
mod chunk;
mod embed;
mod engine;
mod hardware;
mod history;
mod indexer;
mod inference;
mod installer;
mod jobs;
mod models;
mod ocr;
mod opener;
mod parse;
mod power;
mod rag;
mod runtime;
mod store;
mod usage;
mod vision;

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
#[cfg(debug_assertions)] // only the development aids wait
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State};

use inference::{Backend, Engine, PowerSettings, PowerSource, Target};
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
    /// Embedding models: the similarity below which nothing counts as
    /// relevant. Each model has its own scale; absent = `rag::MIN_SCORE`.
    #[serde(default)]
    min_score: Option<f32>,
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

/// How Loft appears on screen.
#[derive(Serialize, Deserialize, Default, Clone, Copy, PartialEq, Debug)]
#[serde(rename_all = "lowercase")]
enum Style {
    /// Docked at a screen edge, like a hardware notch (the default).
    #[default]
    Notch,
    /// A window of its own that stays wherever it is dragged.
    Floating,
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
    /// "Performance & Battery": which engine runs the models when plugged
    /// in and on battery. A section that cannot be read takes the defaults
    /// instead of costing the rest of the file.
    #[serde(default, deserialize_with = "lenient")]
    inference: PowerSettings,
    /// Notch (docked) or floating.
    #[serde(default)]
    style: Style,
    /// Floating: where the window was left, as the top-centre of the window
    /// in physical pixels. `None` = not moved yet.
    #[serde(default)]
    float_anchor: Option<(i32, i32)>,
    /// Live in the tray: nothing on screen until Loft is opened from its
    /// tray (menu bar) icon or with the hotkey.
    #[serde(default)]
    tray_only: bool,
    /// The panel opens twice as large (as far as the display allows).
    #[serde(default)]
    large: bool,
}

/// Deserialize a field, falling back to its default if it is malformed.
fn lenient<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Default + serde::de::DeserializeOwned,
{
    let value = serde_json::Value::deserialize(deserializer)?;
    Ok(serde_json::from_value(value).unwrap_or_default())
}

struct AppState {
    runtime: Mutex<Runtime>,
    models_dir: PathBuf,
    /// The Loft library: a folder Loft owns, where files dropped on the window
    /// are copied and indexed. Registered as an ordinary watched folder.
    library_dir: PathBuf,
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
    /// Which engine runs the models, and the switching between them.
    engines: engine::Coordinator,
    /// Where optional engines (CUDA, OpenVINO) are installed.
    engines_dir: PathBuf,
    /// The optional engine being installed, if one is.
    installing: Mutex<Option<Backend>>,
    install_cancel: Arc<AtomicBool>,
    /// "Test performance": the stored measurements and the test in progress.
    bench: bench::BenchState,
    /// Processor readings between calls of `get_usage`.
    usage: Mutex<usage::Meter>,
    /// The tray (menu bar) icon exists. Without one Loft cannot live there.
    tray_ok: AtomicBool,
    /// Saved conversations (`chats.db`): separate from the index, and from
    /// what is sent to the model.
    history: Mutex<history::History>,
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

/// The state to show for a model server. A server that is not running
/// while Loft is still getting the models going (just launched, switching
/// engines, testing performance) is on its way: "loading", not "stopped",
/// which the UI reads as "nothing has been selected".
fn shown_state(state: ServerState, selected: bool, settling: bool) -> &'static str {
    match state {
        ServerState::Stopped if selected && settling => "loading",
        other => other.as_str(),
    }
}

fn status_payload(state: &AppState) -> RuntimeStatus {
    let (chat_selected, embed_selected) = {
        let sel = state.selection.lock().unwrap();
        (sel.chat.is_some(), sel.embed.is_some())
    };
    let testing = state.bench.running();
    let settling = state.engines.settling() || testing;
    let rt = state.runtime.lock().unwrap();
    let chat = shown_state(rt.chat_state, chat_selected, settling);
    let embed = shown_state(rt.embed_state, embed_selected, settling);
    let message = rt.message.clone().or_else(|| {
        if testing {
            Some("Testing engine performance. The chat model will be back when it has finished.".into())
        } else if chat == "stopped" && embed == "stopped" {
            Some("Select a chat and an embedding model to start.".into())
        } else {
            None
        }
    });
    // The badge shows what the chat model is really running on.
    let accelerator = rt
        .resolved_of(ServerKind::Chat)
        .map(|resolved| resolved.backend.dir_suffix())
        .unwrap_or("cpu");
    RuntimeStatus {
        chat: chat.to_string(),
        embed: embed.to_string(),
        accelerator: accelerator.to_string(),
        message,
    }
}

pub(crate) fn emit_status(app: &AppHandle) {
    let state = app.state::<AppState>();
    let _ = app.emit("runtime:status", status_payload(&state));
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
    // The coordinator starts it on whatever the current profile asks for.
    engine::model_changed(&app).await;
    engine::outcome(&app, if kind == ServerKind::Embed { Engine::Embed } else { Engine::Chat })
}

// ------------------------- Performance & Battery ---------------------------

/// Engines, devices, the saved choices and what is running right now.
#[tauri::command]
async fn get_inference(app: AppHandle) -> engine::InferenceOverview {
    // The first look at the engines starts each build briefly: off the UI thread.
    let scan = app.clone();
    let _ = tauri::async_runtime::spawn_blocking(move || {
        scan.state::<AppState>().runtime.lock().unwrap().inventory();
    })
    .await;
    engine::overview(&app)
}

/// Save the plugged-in and battery choices and apply them. A switch waits
/// for any answer or indexing run in progress.
#[tauri::command]
async fn set_inference_settings(app: AppHandle, settings: PowerSettings) -> engine::InferenceOverview {
    {
        let state = app.state::<AppState>();
        let mut sel = state.selection.lock().unwrap();
        sel.inference = settings.clone();
        save_selection(&state.config_path, &sel);
    }
    engine::apply_settings(&app, settings).await;
    engine::overview(&app)
}

/// "Try another engine now": run the chat model on `target` until this is
/// called again with `None`. The saved choices are not changed.
#[tauri::command]
async fn set_engine_override(app: AppHandle, target: Option<Target>) -> engine::InferenceOverview {
    engine::set_override(&app, target).await;
    engine::overview(&app)
}

/// Look for engines and devices again, forget earlier failures, and try again.
#[tauri::command]
async fn retry_engines(app: AppHandle) -> engine::InferenceOverview {
    engine::retry(&app).await;
    engine::overview(&app)
}

/// What each part of Loft is using right now: memory, graphics memory and
/// processor, for the panel in Settings (which asks every couple of seconds).
#[tauri::command]
async fn get_usage(app: AppHandle) -> usage::Usage {
    let (chat, embed) = {
        let state = app.state::<AppState>();
        let rt = state.runtime.lock().unwrap();
        let name = |kind: ServerKind| rt.model_id_of(kind).map(|id| state.entry(&id).map(|e| e.name.clone()).unwrap_or(id));
        (
            rt.usage_of(ServerKind::Chat, name(ServerKind::Chat)),
            rt.usage_of(ServerKind::Embed, name(ServerKind::Embed)),
        )
    };
    // Listing processes is quick but is not UI work.
    let measuring = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let state = measuring.state::<AppState>();
        let mut meter = state.usage.lock().unwrap();
        usage::measure(&mut meter, chat, embed)
    })
    .await
    .unwrap_or_else(|_| usage::measure(&mut usage::Meter::default(), None, None))
}

/// The last lines a model server logged, for the diagnostics panel.
#[tauri::command]
fn get_engine_log(state: State<AppState>, engine: Engine) -> Vec<String> {
    let kind = if engine == Engine::Embed { ServerKind::Embed } else { ServerKind::Chat };
    state.runtime.lock().unwrap().log_tail_of(kind, 60)
}

/// Progress of an engine install, for the UI (`engine:install`).
#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct EngineInstallProgress {
    backend: Backend,
    stage: Option<installer::Stage>,
    received: u64,
    total: u64,
    done: bool,
    error: Option<String>,
}

/// Download and install an optional engine (CUDA, OpenVINO). The frontend
/// names the backend and nothing else: what is downloaded, from where, and
/// the checksum it must have all come from the list built into Loft.
#[tauri::command]
async fn install_engine(app: AppHandle, backend: Backend) -> Result<engine::InferenceOverview, String> {
    let label = backend.label();
    let (build, dest, cancel) = {
        let state = app.state::<AppState>();
        let platform = runtime::platform_dir();
        let manifest = installer::manifest();
        let build = manifest
            .build(platform, backend)
            .cloned()
            .ok_or_else(|| format!("There is no {label} engine for this system."))?;
        if !build.archives.iter().all(|archive| manifest.trusted(archive)) {
            return Err("Loft's list of engines is not valid, so nothing was downloaded.".into());
        }
        let inventory = state.runtime.lock().unwrap().inventory().clone();
        match inventory.backend(backend) {
            None => return Err(format!("{label} is not supported on this system.")),
            Some(info) if info.status == inference::BackendStatus::Available => return Err(format!("The {label} engine is already installed.")),
            Some(info) if info.status == inference::BackendStatus::HardwareUnavailable => {
                return Err(info.detail.clone().unwrap_or_else(|| format!("This computer has no hardware {label} can use.")))
            }
            Some(_) => {}
        }
        let mut installing = state.installing.lock().unwrap();
        if installing.is_some() {
            return Err("Another engine is being installed. Wait for it to finish.".into());
        }
        *installing = Some(backend);
        state.install_cancel.store(false, Ordering::SeqCst);
        (
            build,
            state.engines_dir.join(format!("{platform}-{}", backend.dir_suffix())),
            state.install_cancel.clone(),
        )
    };

    let progress = |stage: Option<installer::Stage>, received: u64, total: u64, done: bool, error: Option<String>| {
        let _ = app.emit(
            "engine:install",
            EngineInstallProgress {
                backend,
                stage,
                received,
                total,
                done,
                error,
            },
        );
    };
    let mut last = (None, 0u64);
    let total = build.size();
    let result = installer::install(
        &build,
        &dest,
        runtime::server_exe_name(),
        &|| cancel.load(Ordering::SeqCst),
        |stage, received, total| {
            // About a hundred updates per install, plus every change of stage.
            if last.0 != Some(stage) || received - last.1 >= total / 100 || received == total {
                last = (Some(stage), received);
                progress(Some(stage), received, total, false, None);
            }
        },
    )
    .await;
    *app.state::<AppState>().installing.lock().unwrap() = None;
    match &result {
        Ok(files) => {
            eprintln!("Loft: installed the {label} engine ({files} files) in {}", dest.display());
            progress(None, total, total, true, None);
        }
        Err(e) => progress(None, 0, total, true, Some(e.clone())),
    }
    result?;
    // Find it, ask it what it can use, and let "automatic" consider it.
    engine::retry(&app).await;
    Ok(engine::overview(&app))
}

/// Stop the engine download in progress, if any.
#[tauri::command]
fn cancel_engine_install(state: State<AppState>) {
    state.install_cancel.store(true, Ordering::SeqCst);
}

/// Remove an optional engine that was installed from Settings. Engines that
/// are part of Loft itself cannot be removed, nor can one that is in use.
#[tauri::command]
async fn remove_engine(app: AppHandle, backend: Backend) -> Result<engine::InferenceOverview, String> {
    let label = backend.label();
    {
        let state = app.state::<AppState>();
        if state.bench.running() {
            return Err("A performance test is running. Try again when it has finished.".into());
        }
        let dest = state.engines_dir.join(format!("{}-{}", runtime::platform_dir(), backend.dir_suffix()));
        if !dest.join(runtime::server_exe_name()).is_file() {
            return Err(format!("The {label} engine is part of Loft and cannot be removed."));
        }
        // No switch may start a server from it while it is being removed.
        let lock = state.engines.transition_lock();
        let _transition = lock.lock().await;
        let in_use = {
            let rt = state.runtime.lock().unwrap();
            [ServerKind::Chat, ServerKind::Embed].iter().any(|kind| rt.resolved_of(*kind).is_some_and(|r| r.backend == backend))
        };
        if in_use {
            return Err(format!("The {label} engine is in use. Choose another engine first, then remove it."));
        }
        installer::remove(&dest)?;
    }
    engine::retry(&app).await;
    Ok(engine::overview(&app))
}

/// What "Try another engine now" and "Try NPU now" report back.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct TrialDone {
    trial: bench::Trial,
    overview: engine::InferenceOverview,
}

/// "Try another engine now" / "Try NPU now": run the chat model on `target`
/// (temporarily; the saved choices are not changed), confirm where it is
/// really running, and ask it a short test question.
#[tauri::command]
async fn try_engine(app: AppHandle, target: Target) -> TrialDone {
    let trial = bench::trial(&app, target).await;
    TrialDone {
        trial,
        overview: engine::overview(&app),
    }
}

/// "Test performance": measure the available engines (or just `targets`)
/// with the selected chat model. Takes a while; progress is `bench:progress`.
#[tauri::command]
async fn run_benchmark(app: AppHandle, targets: Option<Vec<Target>>) -> Result<bench::Report, String> {
    bench::run(&app, targets).await
}

/// The stored measurements for the selected chat model.
#[tauri::command]
async fn get_benchmarks(app: AppHandle) -> bench::Report {
    let scan = app.clone();
    let _ = tauri::async_runtime::spawn_blocking(move || {
        scan.state::<AppState>().runtime.lock().unwrap().inventory();
    })
    .await;
    bench::report(&app)
}

/// Stop the performance test after the engine it is on.
#[tauri::command]
fn cancel_benchmark(state: State<AppState>) {
    state.bench.cancel();
}

/// Dev aid (debug builds only): make one target fail to start, to exercise
/// fallback and the no-fallback stop. `None` ends it.
#[tauri::command]
fn dev_fail_target(state: State<AppState>, target: Option<Target>) -> Result<(), String> {
    if !cfg!(debug_assertions) {
        return Err("only available in development builds".into());
    }
    state.engines.state.lock().unwrap().dev_fail = target;
    Ok(())
}

/// Dev aid (debug builds only): kill a model server as a crash would, to
/// exercise crash recovery. Returns the process id that was killed.
#[tauri::command]
fn dev_kill_engine(state: State<AppState>, engine: Engine) -> Result<u32, String> {
    if !cfg!(debug_assertions) {
        return Err("only available in development builds".into());
    }
    let kind = if engine == Engine::Embed { ServerKind::Embed } else { ServerKind::Chat };
    let pid = state.runtime.lock().unwrap().pid_of(kind).ok_or("that engine is not running")?;
    let killer = if cfg!(windows) { "taskkill" } else { "kill" };
    let args: Vec<String> = if cfg!(windows) { vec!["/PID".into(), pid.to_string(), "/F".into()] } else { vec!["-9".into(), pid.to_string()] };
    std::process::Command::new(killer).args(args).output().map_err(|e| e.to_string())?;
    Ok(pid)
}

/// Dev aid (debug builds only): the process ids of the two model servers,
/// and of every `llama-server` running on this computer, so a test can show
/// that a switch replaced exactly one process and left no orphan behind.
#[tauri::command]
fn dev_engines(state: State<AppState>) -> Result<serde_json::Value, String> {
    if !cfg!(debug_assertions) {
        return Err("only available in development builds".into());
    }
    let (chat, embed) = {
        let rt = state.runtime.lock().unwrap();
        (rt.pid_of(ServerKind::Chat), rt.pid_of(ServerKind::Embed))
    };
    let mut all: Vec<u32> = Vec::new();
    if cfg!(windows) {
        let out = std::process::Command::new("tasklist")
            .args(["/FI", "IMAGENAME eq llama-server.exe", "/FO", "CSV", "/NH"])
            .output()
            .map_err(|e| e.to_string())?;
        for line in String::from_utf8_lossy(&out.stdout).lines() {
            if let Some(pid) = line.split(',').nth(1).and_then(|p| p.trim_matches('"').parse().ok()) {
                all.push(pid);
            }
        }
    } else if let Ok(out) = std::process::Command::new("pgrep").args(["-x", "llama-server"]).output() {
        // macOS and Linux. (Not run by the author: see HANDOVER.md.)
        all.extend(String::from_utf8_lossy(&out.stdout).lines().filter_map(|pid| pid.trim().parse::<u32>().ok()));
    }
    // How many questions and stops there have been, so a test can tell that
    // someone else used the window while it was waiting for an answer.
    let asks = state.ask_gen.load(Ordering::SeqCst);
    Ok(serde_json::json!({ "chat": chat, "embed": embed, "all": all, "asks": asks }))
}

/// Dev aid: pretend the power source changed (debug builds only), so the
/// switching can be exercised without pulling the charger.
#[tauri::command]
fn dev_set_power(state: State<AppState>, source: PowerSource) -> Result<(), String> {
    if !cfg!(debug_assertions) {
        return Err("only available in development builds".into());
    }
    state.engines.power_reading(source);
    Ok(())
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
#[serde(rename_all = "camelCase")]
struct AskDone {
    sources: Vec<rag::Source>,
    /// Context usage; absent when the model was not called or was stopped.
    stats: Option<rag::Stats>,
    /// How the answer ended: complete, or stopped by the user.
    status: history::Status,
    /// The conversation the turn was saved in; absent when it was not saved.
    conversation: Option<history::Conversation>,
    /// The saved answer's message id.
    message: Option<String>,
    /// The turn could not be saved, and why. (The answer is still shown.)
    save_error: Option<String>,
}

/// A piece of a streamed answer. `request` is the id the UI gave the
/// question, so text can only ever be added to the answer it belongs to.
#[derive(Serialize, Clone)]
struct AskToken<'a> {
    request: &'a str,
    text: &'a str,
}

#[derive(Serialize, Clone)]
struct AskSources<'a> {
    request: &'a str,
    sources: &'a [rag::Source],
}

/// Sent as soon as a question has been saved: which conversation it is in
/// (a new one for a first question) and the id of the answer being written.
#[derive(Serialize, Clone)]
struct AskSaved<'a> {
    request: &'a str,
    conversation: &'a history::Conversation,
    message: &'a str,
}

/// Save a question that is about to be answered: in `conversation`, or in a
/// new one named after it. Returns the conversation and the id of the
/// (pending) answer.
fn begin_saved_turn(state: &AppState, conversation: Option<&str>, question: &str, model: Option<&str>) -> Result<(history::Conversation, String), String> {
    let now = history::now_ms();
    let mut chats = state.history.lock().map_err(|_| "the chat history is unavailable".to_string())?;
    let conversation = match conversation {
        Some(id) => chats.conversation(id)?.ok_or_else(|| "that conversation no longer exists".to_string())?,
        None => chats.create(question, now)?,
    };
    let (_, answer_id) = chats.begin_turn(&conversation.id, question, model, now)?;
    Ok((conversation, answer_id))
}

/// A citation as it is saved: the source as shown, with the hash of the
/// file's contents at this moment.
fn source_record(state: &AppState, source: &rag::Source) -> history::SourceRecord {
    let file_hash = state.store.lock().ok().and_then(|store| store.file_hash(&source.path).ok().flatten());
    history::SourceRecord {
        file: source.file.clone(),
        path: source.path.clone(),
        page: source.page,
        location: source.location.clone(),
        score: source.score,
        snippet: source.snippet.clone(),
        file_hash,
    }
}

/// Answer a question from the indexed folders. `history` is the conversation
/// so far as context (the UI sends it; only the last few turns are used),
/// which makes the question a follow-up; `options` are the chat options that
/// shape the answer. `request` is an id of the UI's choosing, carried by
/// every event of this answer.
///
/// With `save` (or an existing `conversation`), the question is saved in the
/// chat history at once and the answer when it ends, marked as complete,
/// stopped or failed.
///
/// Emits `ask:saved` once the question is stored, `ask:sources` once
/// retrieval is done and `ask:token` for each piece of the streamed answer;
/// resolves when the answer is complete (or was stopped).
#[tauri::command]
async fn ask(
    app: AppHandle,
    question: String,
    history: Option<Vec<rag::Turn>>,
    options: Option<rag::Options>,
    request: Option<String>,
    conversation: Option<String>,
    save: Option<bool>,
) -> Result<AskDone, String> {
    let history = history.unwrap_or_default();
    let options = options.unwrap_or_default();
    let request = request.unwrap_or_default();
    // An answer in progress keeps both servers where they are: a change of
    // engine (the charger was pulled) waits until it has finished.
    let _busy = engine::begin(&app, &[Engine::Chat, Engine::Embed]);
    let (store, endpoints, gen, my_gen, model) = {
        let state = app.state::<AppState>();
        if state.bench.running() {
            return Err("Loft is testing engine performance. Ask again when the test has finished.".into());
        }
        let model = state.selection.lock().unwrap().chat.clone();
        // The folders Loft answers about: named when a request is declined.
        let folders: Vec<String> = state
            .store
            .lock()
            .ok()
            .and_then(|store| store.list_folders().ok())
            .unwrap_or_default()
            .into_iter()
            .map(|f| f.path)
            .collect();
        // A model that is on its way (Loft has just launched, or is
        // switching engines) is not a model that is missing.
        let settling = state.engines.settling();
        let rt = state.runtime.lock().unwrap();
        let not_ready = |kind: ServerKind, name: &str| {
            if settling || rt.state_of(kind) == ServerState::Loading {
                format!("The {name} model is still starting. Ask again in a moment.")
            } else if rt.state_of(kind) == ServerState::Error {
                format!("The {name} model could not be started. See Performance & battery in Settings.")
            } else {
                format!("The {name} model isn't running. Download and select one in Settings.")
            }
        };
        let embed_port = ready_port(&rt, ServerKind::Embed).ok_or_else(|| not_ready(ServerKind::Embed, "embedding"))?;
        let chat_port = ready_port(&rt, ServerKind::Chat).ok_or_else(|| not_ready(ServerKind::Chat, "chat"))?;
        // How to phrase a question for the embedding model in use, and
        // where its "not relevant" line is.
        let embedder = rt.embed.as_ref().and_then(|s| state.entry(&s.model_id));
        let query_prefix = embedder.map(|e| e.query_prefix.clone()).unwrap_or_default();
        let min_score = embedder.and_then(|e| e.min_score).unwrap_or(rag::MIN_SCORE);
        let gen = state.ask_gen.clone();
        let my_gen = gen.fetch_add(1, Ordering::SeqCst) + 1;
        (
            state.store.clone(),
            rag::Endpoints {
                embed_port,
                chat_port,
                query_prefix,
                min_score,
                folders,
                options,
            },
            gen,
            my_gen,
            model,
        )
    };

    // The question is saved before anything is generated; the answer gets
    // a place of its own, marked as still being written.
    let mut save_error: Option<String> = None;
    let mut saved: Option<(history::Conversation, String)> = None;
    if save.unwrap_or(false) || conversation.is_some() {
        match begin_saved_turn(&app.state::<AppState>(), conversation.as_deref(), &question, model.as_deref()) {
            Ok((conversation, answer_id)) => {
                let _ = app.emit(
                    "ask:saved",
                    AskSaved {
                        request: &request,
                        conversation: &conversation,
                        message: &answer_id,
                    },
                );
                saved = Some((conversation, answer_id));
            }
            Err(e) => save_error = Some(format!("This turn was not saved in the chat history: {e}.")),
        }
    }

    // What has been produced so far, kept here too: if generation fails,
    // this is what gets saved (as failed), not lost.
    let written = Arc::new(Mutex::new(String::new()));
    let found: Arc<Mutex<Vec<rag::Source>>> = Arc::new(Mutex::new(Vec::new()));
    let (app_sources, app_tokens) = (app.clone(), app.clone());
    let (request_sources, request_tokens) = (request.clone(), request.clone());
    let (found_now, written_now) = (found.clone(), written.clone());
    let still_mine = gen.clone();
    let result = rag::answer(
        &store,
        &endpoints,
        &question,
        &history,
        move || gen.load(Ordering::SeqCst) != my_gen,
        move |sources| {
            *found_now.lock().unwrap() = sources.to_vec();
            let _ = app_sources.emit(
                "ask:sources",
                AskSources {
                    request: &request_sources,
                    sources,
                },
            );
        },
        move |token| {
            written_now.lock().unwrap().push_str(token);
            let _ = app_tokens.emit(
                "ask:token",
                AskToken {
                    request: &request_tokens,
                    text: token,
                },
            );
        },
    )
    .await;

    // How it ended. A newer question or the Stop button ends an answer early:
    // that is "stopped", whatever had been written, never "complete".
    let stopped = still_mine.load(Ordering::SeqCst) != my_gen;
    let (status, error) = match &result {
        Err(e) => (history::Status::Failed, Some(e.clone())),
        Ok(_) if stopped => (history::Status::Stopped, None),
        Ok(_) => (history::Status::Complete, None),
    };
    let mut done = AskDone {
        sources: match &result {
            Ok(outcome) => outcome.sources.clone(),
            Err(_) => found.lock().unwrap().clone(),
        },
        stats: result.as_ref().ok().and_then(|outcome| outcome.stats.clone()),
        status,
        conversation: None,
        message: None,
        save_error,
    };
    if let Some((conversation, answer_id)) = saved {
        let state = app.state::<AppState>();
        let content = match &result {
            Ok(outcome) => outcome.answer.clone(),
            Err(_) => written.lock().unwrap().clone(),
        };
        let sources: Vec<history::SourceRecord> = done.sources.iter().map(|s| source_record(&state, s)).collect();
        let stats = done.stats.as_ref().and_then(|s| serde_json::to_value(s).ok());
        let mut chats = state.history.lock().unwrap();
        if let Err(e) = chats.finish_turn(&answer_id, status, &content, stats.as_ref(), error.as_deref(), &sources, history::now_ms()) {
            done.save_error = Some(format!("This answer was not saved in the chat history: {e}."));
        }
        done.conversation = chats.conversation(&conversation.id).ok().flatten().or(Some(conversation));
        done.message = Some(answer_id);
    }
    result?;
    Ok(done)
}

// ------------------------------ Chat history --------------------------------

/// The saved conversations (newest first), or those matching `query`.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ConversationList {
    conversations: Vec<history::Conversation>,
    /// How many there are in all, whatever the search.
    total: u32,
}

/// A saved citation, with what has become of its file since.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SourceView {
    #[serde(flatten)]
    source: history::SourceRecord,
    /// same | changed | missing
    state: history::FileState,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct MessageView {
    id: String,
    role: String,
    content: String,
    created_at: i64,
    status: history::Status,
    model: Option<String>,
    stats: Option<serde_json::Value>,
    error: Option<String>,
    sources: Vec<SourceView>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ConversationView {
    conversation: history::Conversation,
    messages: Vec<MessageView>,
}

fn chats_unavailable<T>(_: T) -> String {
    "The chat history is unavailable.".to_string()
}

/// The saved conversations, most recently updated first; with `query`, the
/// ones whose title or messages contain it.
#[tauri::command]
async fn list_conversations(app: AppHandle, query: Option<String>) -> Result<ConversationList, String> {
    let state = app.state::<AppState>();
    let chats = state.history.lock().map_err(chats_unavailable)?;
    Ok(ConversationList {
        conversations: chats.list(query.as_deref())?,
        total: chats.count()?,
    })
}

/// One conversation in full, as it was saved: every message in order, each
/// answer with the sources it was written from. Nothing is regenerated.
#[tauri::command]
async fn get_conversation(app: AppHandle, id: String) -> Result<ConversationView, String> {
    let state = app.state::<AppState>();
    let (conversation, messages) = {
        let chats = state.history.lock().map_err(chats_unavailable)?;
        let conversation = chats.conversation(&id)?.ok_or("That conversation no longer exists.")?;
        chats.opened(&id, history::now_ms())?;
        (conversation, chats.messages(&id)?)
    };
    // What has become of each cited file: checked by its own path only.
    let store = state.store.lock().map_err(|_| "index is unavailable".to_string())?;
    let messages = messages
        .into_iter()
        .map(|m| MessageView {
            sources: m
                .sources
                .into_iter()
                .map(|source| {
                    let current = store.file_hash(&source.path).ok().flatten();
                    SourceView {
                        state: history::file_state(source.file_hash.as_deref(), current.as_deref()),
                        source,
                    }
                })
                .collect(),
            id: m.id,
            role: m.role,
            content: m.content,
            created_at: m.created_at,
            status: m.status,
            model: m.model,
            stats: m.stats,
            error: m.error,
        })
        .collect();
    Ok(ConversationView { conversation, messages })
}

#[tauri::command]
async fn rename_conversation(app: AppHandle, id: String, title: String) -> Result<history::Conversation, String> {
    let state = app.state::<AppState>();
    let chats = state.history.lock().map_err(chats_unavailable)?;
    chats.rename(&id, &title)
}

/// Delete a conversation, its messages and their source records. Files and
/// their index are a different database and are not touched.
#[tauri::command]
async fn delete_conversation(app: AppHandle, id: String) -> Result<(), String> {
    let state = app.state::<AppState>();
    let chats = state.history.lock().map_err(chats_unavailable)?;
    chats.delete(&id).map(|_| ())
}

/// A turn of the conversation an older Loft kept in the webview's storage.
#[derive(Deserialize)]
struct ImportedTurn {
    question: String,
    answer: String,
    #[serde(default)]
    sources: Vec<history::SourceRecord>,
    #[serde(default)]
    stats: Option<serde_json::Value>,
    #[serde(default)]
    error: String,
}

/// Bring the one conversation an older Loft saved into the chat history.
#[tauri::command]
async fn import_conversation(app: AppHandle, turns: Vec<ImportedTurn>) -> Result<history::Conversation, String> {
    let first = turns.first().ok_or("There is nothing to import.")?;
    let state = app.state::<AppState>();
    let mut chats = state.history.lock().map_err(chats_unavailable)?;
    let now = history::now_ms();
    let conversation = chats.create(&first.question, now)?;
    for (i, turn) in turns.iter().enumerate() {
        let at = now + i as i64;
        let (_, answer_id) = chats.begin_turn(&conversation.id, &turn.question, None, at)?;
        let (status, error) = if turn.error.is_empty() {
            (history::Status::Complete, None)
        } else {
            (history::Status::Failed, Some(turn.error.as_str()))
        };
        chats.finish_turn(&answer_id, status, &turn.answer, turn.stats.as_ref(), error, &turn.sources, at)?;
    }
    chats.conversation(&conversation.id)?.ok_or_else(|| "The conversation could not be imported.".to_string())
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
/// The panel (logical px) at its normal size; the window adds a margin.
const PANEL_SIZE: (f64, f64) = (460.0, 520.0);
const PANEL_MARGIN: (f64, f64) = (80.0, 60.0); // corners and shadow: window 540x580
// The same, hanging from the right edge: the pill stands upright (36x190) and
// the margins are above, below and to the left.
const COLLAPSED_SIZE_RIGHT: (f64, f64) = (76.0, 274.0);
const PANEL_MARGIN_RIGHT: (f64, f64) = (60.0, 80.0); // window 520x600
/// Room left between a large panel's window and the edges of the display.
const LARGE_GAP: f64 = 24.0;

/// The panel's size (logical px): normal, or twice that when `large`, kept
/// small enough for its window to fit in `room` (the display's usable area,
/// logical px), and never smaller than normal.
fn panel_size(large: bool, room: Option<(f64, f64)>) -> (f64, f64) {
    if !large {
        return PANEL_SIZE;
    }
    let (mut width, mut height) = (PANEL_SIZE.0 * 2.0, PANEL_SIZE.1 * 2.0);
    if let Some((room_w, room_h)) = room {
        // The larger of the two margins, so the size is the same at either edge.
        let margin = PANEL_MARGIN.0.max(PANEL_MARGIN_RIGHT.0) + LARGE_GAP;
        width = width.min(room_w - margin);
        height = height.min(room_h - PANEL_MARGIN.1.max(PANEL_MARGIN_RIGHT.1) - LARGE_GAP);
    }
    (width.max(PANEL_SIZE.0).floor(), height.max(PANEL_SIZE.1).floor())
}

/// Window size (logical px) for a placement and state, around a `panel`.
fn notch_size(placement: Placement, expanded: bool, panel: (f64, f64)) -> (f64, f64) {
    match (placement, expanded) {
        (Placement::Top, false) => COLLAPSED_SIZE,
        (Placement::Top, true) => (panel.0 + PANEL_MARGIN.0, panel.1 + PANEL_MARGIN.1),
        (Placement::Right, false) => COLLAPSED_SIZE_RIGHT,
        (Placement::Right, true) => (panel.0 + PANEL_MARGIN_RIGHT.0, panel.1 + PANEL_MARGIN_RIGHT.1),
    }
}

/// The usable part of a display (without the taskbar or dock), logical px.
fn room_on(monitor: &tauri::Monitor) -> (f64, f64) {
    let area = monitor.work_area();
    let scale = monitor.scale_factor();
    (area.size.width as f64 / scale, area.size.height as f64 / scale)
}

/// Top-left corner (physical px) of a `size` window docked at `placement` on
/// a monitor at `origin` measuring `monitor`: centred along its edge.
fn notch_origin(placement: Placement, origin: (i32, i32), monitor: (i32, i32), size: (i32, i32)) -> (i32, i32) {
    match placement {
        Placement::Top => (origin.0 + (monitor.0 - size.0) / 2, origin.1),
        Placement::Right => (origin.0 + monitor.0 - size.0, origin.1 + (monitor.1 - size.1) / 2),
    }
}

/// Top-left corner (physical px) of a floating window of `size` whose
/// top-centre is `anchor`, moved as little as needed to be wholly on the
/// monitor at `origin` measuring `monitor`.
fn floating_origin(anchor: (i32, i32), origin: (i32, i32), monitor: (i32, i32), size: (i32, i32)) -> (i32, i32) {
    let x = (anchor.0 - size.0 / 2).clamp(origin.0, (origin.0 + monitor.0 - size.0).max(origin.0));
    let y = anchor.1.clamp(origin.1, (origin.1 + monitor.1 - size.1).max(origin.1));
    (x, y)
}

/// Where a floating window goes before it has ever been moved: centred,
/// a sixth of the way down the monitor.
fn default_anchor(origin: (i32, i32), monitor: (i32, i32)) -> (i32, i32) {
    (origin.0 + monitor.0 / 2, origin.1 + monitor.1 / 6)
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
/// top-centre, or centred on the right edge. Returns the panel's size.
fn size_notch(window: &tauri::WebviewWindow, expanded: bool, placement: Placement, monitor: Option<&str>, large: bool) -> (f64, f64) {
    let Some(monitor) = target_monitor(window, monitor) else {
        let panel = panel_size(large, None);
        let (width, height) = notch_size(placement, expanded, panel);
        let _ = window.set_size(tauri::LogicalSize::new(width, height));
        return panel;
    };
    let panel = panel_size(large, Some(room_on(&monitor)));
    let (width, height) = notch_size(placement, expanded, panel);
    let scale = monitor.scale_factor();
    let size = ((width * scale).round() as i32, (height * scale).round() as i32);
    let origin = notch_origin(
        placement,
        (monitor.position().x, monitor.position().y),
        (monitor.size().width as i32, monitor.size().height as i32),
        size,
    );
    move_and_size(window, origin, size);
    panel
}

/// Put the window at `origin` with `size` (physical px).
fn move_and_size(window: &tauri::WebviewWindow, origin: (i32, i32), size: (i32, i32)) {
    let position = tauri::PhysicalPosition::new(origin.0, origin.1);
    let new_size = tauri::PhysicalSize::new(size.0 as u32, size.1 as u32);
    // GTK will not shrink a window below its first size while that window is
    // non-resizable, and the first configure event locks the flag back off.
    // The size that sticks is the one given as both the minimum and the
    // maximum. The window stays notionally resizable; those two bounds are
    // equal, so it cannot be dragged to another size.
    #[cfg(target_os = "linux")]
    {
        let fixed = tauri::Size::Physical(new_size);
        let _ = window.set_resizable(true);
        let _ = window.set_min_size(Some(tauri::Size::Physical(tauri::PhysicalSize::new(1, 1))));
        let _ = window.set_max_size(Some(fixed));
        let _ = window.set_min_size(Some(fixed));
    }
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

/// Place the floating window: its top-centre at `anchor` (where it was
/// left), on whichever display that point is on, kept wholly on screen. A
/// point that is on no display any more (a monitor was unplugged) falls back
/// to the default spot on the chosen display.
fn place_floating(
    window: &tauri::WebviewWindow,
    expanded: bool,
    anchor: Option<(i32, i32)>,
    fallback_monitor: Option<&str>,
    large: bool,
) -> (f64, f64) {
    let holds = |m: &tauri::Monitor, p: (i32, i32)| {
        let (x, y) = (m.position().x, m.position().y);
        p.0 >= x && p.0 < x + m.size().width as i32 && p.1 >= y && p.1 < y + m.size().height as i32
    };
    let monitor = anchor
        .and_then(|a| window.available_monitors().unwrap_or_default().into_iter().find(|m| holds(m, a)))
        .or_else(|| target_monitor(window, fallback_monitor));
    let Some(monitor) = monitor else {
        let panel = panel_size(large, None);
        let (width, height) = notch_size(Placement::Top, expanded, panel);
        let _ = window.set_size(tauri::LogicalSize::new(width, height));
        return panel;
    };
    let panel = panel_size(large, Some(room_on(&monitor)));
    let (width, height) = notch_size(Placement::Top, expanded, panel);
    let scale = monitor.scale_factor();
    let size = ((width * scale).round() as i32, (height * scale).round() as i32);
    let origin = (monitor.position().x, monitor.position().y);
    let extent = (monitor.size().width as i32, monitor.size().height as i32);
    let anchor = anchor.filter(|a| holds(&monitor, *a)).unwrap_or_else(|| default_anchor(origin, extent));
    move_and_size(window, floating_origin(anchor, origin, extent, size), size);
    panel
}

/// The panel's size, for the UI (logical px).
#[derive(Serialize, Clone, Copy, Debug, PartialEq)]
struct PanelSize {
    width: f64,
    height: f64,
}

/// Put the window where its style says: docked at the saved edge and
/// display, or floating where it was left. The panel's size (which depends
/// on the display) is returned and sent to the UI as `loft://panel-size`.
fn dock(window: &tauri::WebviewWindow, state: &AppState, expanded: bool) -> PanelSize {
    let (style, placement, monitor, anchor, large) = {
        let sel = state.selection.lock().unwrap();
        (sel.style, sel.placement, sel.monitor.clone(), sel.float_anchor, sel.large)
    };
    let (width, height) = match style {
        Style::Notch => size_notch(window, expanded, placement, monitor.as_deref(), large),
        Style::Floating => place_floating(window, expanded, anchor, monitor.as_deref(), large),
    };
    let panel = PanelSize { width, height };
    let _ = window.emit("loft://panel-size", panel);
    panel
}

/// Loft is living in the tray: it has a tray icon and the user asked for it.
fn in_tray(state: &AppState) -> bool {
    state.tray_ok.load(Ordering::SeqCst) && state.selection.lock().unwrap().tray_only
}

/// Called by the UI just before it expands and just after it collapses.
#[tauri::command]
fn set_expanded(window: tauri::WebviewWindow, state: State<AppState>, expanded: bool) -> PanelSize {
    let panel = dock(&window, &state, expanded);
    if expanded {
        let _ = window.show();
    } else if in_tray(&state) {
        // Nothing stays on screen: the tray icon (or the hotkey) brings it back.
        let _ = window.hide();
    }
    panel
}

/// The larger panel on or off, remembered. Returns the panel's new size.
#[tauri::command]
fn set_large(window: tauri::WebviewWindow, state: State<AppState>, large: bool, expanded: bool) -> PanelSize {
    {
        let mut sel = state.selection.lock().unwrap();
        sel.large = large;
        save_selection(&state.config_path, &sel);
    }
    dock(&window, &state, expanded)
}

/// How Loft appears, for Settings.
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
struct WindowSettings {
    style: Style,
    tray_only: bool,
    /// There is a tray (menu bar) icon to live in.
    tray_available: bool,
    /// The panel opens twice as large.
    large: bool,
}

#[tauri::command]
fn get_window_settings(state: State<AppState>) -> WindowSettings {
    let sel = state.selection.lock().unwrap();
    WindowSettings {
        style: sel.style,
        tray_only: sel.tray_only && state.tray_ok.load(Ordering::SeqCst),
        tray_available: state.tray_ok.load(Ordering::SeqCst),
        large: sel.large,
    }
}

/// Switch between the docked notch and the floating window, and remember it.
#[tauri::command]
fn set_window_style(window: tauri::WebviewWindow, state: State<AppState>, style: Style, expanded: bool) {
    {
        let mut sel = state.selection.lock().unwrap();
        sel.style = style;
        save_selection(&state.config_path, &sel);
    }
    dock(&window, &state, expanded);
}

/// Floating: remember where the window is now (it was just dragged there).
#[tauri::command]
fn remember_position(window: tauri::WebviewWindow, state: State<AppState>) {
    let mut sel = state.selection.lock().unwrap();
    if sel.style != Style::Floating {
        return;
    }
    if let (Ok(position), Ok(size)) = (window.outer_position(), window.outer_size()) {
        let anchor = (position.x + size.width as i32 / 2, position.y);
        if sel.float_anchor != Some(anchor) {
            sel.float_anchor = Some(anchor);
            save_selection(&state.config_path, &sel);
        }
    }
}

/// Live in the tray (no pill on screen) or not. Refused where there is no
/// tray icon, since Loft would then be unreachable.
#[tauri::command]
fn set_tray_only(window: tauri::WebviewWindow, state: State<AppState>, enabled: bool, expanded: bool) -> Result<bool, String> {
    if enabled && !state.tray_ok.load(Ordering::SeqCst) {
        return Err("This desktop gave Loft no tray icon to live in, so it stays on screen.".into());
    }
    {
        let mut sel = state.selection.lock().unwrap();
        sel.tray_only = enabled;
        save_selection(&state.config_path, &sel);
    }
    if !enabled {
        let _ = window.show();
    } else if !expanded {
        let _ = window.hide();
    }
    Ok(enabled)
}

/// Dev aid (debug builds only): what the tray icon and the hotkey do, for a
/// test that cannot click the tray.
#[tauri::command]
fn dev_summon(app: AppHandle) -> Result<(), String> {
    if !cfg!(debug_assertions) {
        return Err("only available in development builds".into());
    }
    summon(&app);
    Ok(())
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
            let tray_only = selection.tray_only;
            let inference_settings = selection.inference.clone();
            let catalog = load_catalog();
            let store = store::Store::open(&data.join("loft.db"))
                .map_err(|e| -> Box<dyn std::error::Error> { e.into() })?;
            // Files that had no readable text under older parsers get another go.
            if let Ok(n) = store.upgrade_parsers(parse::PARSER_VERSION) {
                if n > 0 {
                    eprintln!("Loft: {n} files with no text will be read again (better parsers)");
                }
            }
            // The Loft library: a folder Loft owns, where files dropped on the
            // window are kept and indexed. It is registered as an ordinary
            // watched folder the first time something is dropped (see
            // jobs::add_documents), so a fresh install still shows "no folders
            // yet" until the user actually adds or drops something.
            let library_dir = data.join("Library");
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
            let engines_dir = app.path().app_data_dir()?.join("engines");
            // Dev aid (debug builds): LOFT_ENGINES_DIR installs optional
            // engines somewhere else, so a test does not add to the real ones.
            #[cfg(debug_assertions)]
            let engines_dir = std::env::var_os("LOFT_ENGINES_DIR").map(PathBuf::from).unwrap_or(engines_dir);
            let bench_path = data.join("benchmarks.json");
            let chats = history::History::open(&data.join("chats.db")).map_err(|e| -> Box<dyn std::error::Error> { e.into() })?;
            // An answer that was being written when Loft last closed is not an answer.
            match chats.mark_interrupted() {
                Ok(0) | Err(_) => {}
                Ok(n) => eprintln!("Loft: {n} answer(s) were cut off when Loft last closed; they are marked as interrupted"),
            }

            app.manage(AppState {
                // Optional engines (CUDA, OpenVINO) are installed into the
                // app-data folder, which is writable; it is searched first.
                runtime: Mutex::new(Runtime::with_extra_roots(
                    std::iter::once(engines_dir.clone()).chain(bundled_binaries).collect(),
                )),
                engines_dir,
                tray_ok: AtomicBool::new(false),
                history: Mutex::new(chats),
                installing: Mutex::new(None),
                install_cancel: Arc::new(AtomicBool::new(false)),
                bench: bench::BenchState::new(bench_path),
                usage: Mutex::new(usage::Meter::default()),
                models_dir,
                library_dir,
                config_path,
                catalog,
                selection: Mutex::new(selection),
                downloading: Mutex::new(HashSet::new()),
                store: Arc::new(Mutex::new(store)),
                index,
                ask_gen: Arc::new(AtomicU64::new(0)),
                engines: engine::Coordinator::new(inference_settings, power::current()),
            });

            // Tray icon (the menu bar on macOS, the notification area on
            // Windows, an app indicator on Linux): Loft has no title bar, so
            // this is where Quit lives, and where Loft itself can live. Not
            // fatal if the desktop has no tray (some Linux setups).
            let tray_ok = match build_tray(app) {
                Ok(()) => true,
                Err(e) => {
                    eprintln!("Loft: could not create the tray icon: {e}");
                    false
                }
            };
            app.state::<AppState>().tray_ok.store(tray_ok, Ordering::SeqCst);
            // Living in the menu bar on macOS also means no Dock icon. (Takes
            // effect at launch; turning the setting on or off applies it next time.)
            #[cfg(target_os = "macos")]
            if tray_only && tray_ok {
                app.set_activation_policy(tauri::ActivationPolicy::Accessory);
            }

            // Place the window, then show it: unless Loft lives in the tray,
            // where it stays out of sight until it is asked for.
            if let Some(win) = app.get_webview_window("notch") {
                dock(&win, &app.state::<AppState>(), false);
                if !(tray_only && tray_ok) {
                    let _ = win.show();
                }
                // The window's first configure event (on show) forces the
                // resizable flag from tauri.conf and, on GTK, that throws
                // away a size set just before. Place it again once that has
                // happened. By then the dev script has not started (it waits
                // several seconds).
                #[cfg(target_os = "linux")]
                {
                    let handle = app.handle().clone();
                    std::thread::spawn(move || {
                        std::thread::sleep(std::time::Duration::from_millis(400));
                        if let Some(win) = handle.get_webview_window("notch") {
                            let state = handle.state::<AppState>();
                            dock(&win, &state, false);
                        }
                    });
                }
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
                // Find the installed engines and what each can use (this starts
                // each build briefly), then start the selected models on the
                // target the current power profile asks for.
                let scan = handle.clone();
                let _ = tauri::async_runtime::spawn_blocking(move || {
                    scan.state::<AppState>().runtime.lock().unwrap().inventory();
                })
                .await;
                engine::reconcile(&handle, "startup").await;

                // From here on: restart a server if its process dies, and
                // switch profile when the power source changes.
                engine::supervise(handle.clone());
                engine::watch_power(handle.clone());

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
            get_inference,
            set_inference_settings,
            set_engine_override,
            retry_engines,
            get_engine_log,
            get_usage,
            install_engine,
            cancel_engine_install,
            remove_engine,
            try_engine,
            run_benchmark,
            get_benchmarks,
            cancel_benchmark,
            dev_set_power,
            dev_engines,
            dev_fail_target,
            dev_kill_engine,
            list_models,
            select_model,
            set_picture_model,
            download_model,
            jobs::list_folders,
            jobs::add_folder,
            jobs::add_documents,
            jobs::reindex_folder,
            jobs::delete_index,
            jobs::remove_folder,
            ask,
            cancel_ask,
            list_conversations,
            get_conversation,
            rename_conversation,
            delete_conversation,
            import_conversation,
            open_source,
            set_expanded,
            get_window_settings,
            set_window_style,
            set_tray_only,
            remember_position,
            set_large,
            dev_summon,
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
    fn each_embedding_model_has_its_own_relevance_floor() {
        let catalog = load_catalog();
        let floor = |id: &str| catalog.iter().find(|e| e.id == id).unwrap_or_else(|| panic!("{id} is in the catalog")).min_score;
        // Measured on the sample folder (see rag::MIN_SCORE).
        assert_eq!(floor("nomic-embed-text-v1.5"), Some(0.45));
        assert_eq!(floor("embeddinggemma-300m"), Some(0.2));
        // Not measured yet: takes the default.
        assert_eq!(floor("bge-m3"), None);
        // Only embedding models have one, and every one given is a sane similarity.
        for entry in &catalog {
            if let Some(score) = entry.min_score {
                assert_eq!(entry.r#type, "embed", "{} is not an embedding model", entry.id);
                assert!((0.0..1.0).contains(&score), "{}: {score}", entry.id);
            }
        }
    }

    #[test]
    fn a_model_on_its_way_is_shown_as_loading_not_missing() {
        // Just launched (or switching, or testing): the selected model's
        // server is not up yet, and that is "loading".
        assert_eq!(shown_state(ServerState::Stopped, true, true), "loading");
        // Nothing selected: there is nothing on its way.
        assert_eq!(shown_state(ServerState::Stopped, false, true), "stopped");
        // Settled and still not running: it really is stopped.
        assert_eq!(shown_state(ServerState::Stopped, true, false), "stopped");
        // Every other state is reported as it is, settling or not.
        for settling in [true, false] {
            assert_eq!(shown_state(ServerState::Ready, true, settling), "ready");
            assert_eq!(shown_state(ServerState::Loading, true, settling), "loading");
            assert_eq!(shown_state(ServerState::Error, true, settling), "error");
        }
    }

    #[test]
    fn a_floating_window_stays_where_it_was_put_and_on_screen() {
        // A 1920x1080 monitor whose top-left corner is at (100, 50).
        let (origin, monitor) = ((100, 50), (1920, 1080));
        // The pill and the panel share a top-centre, so the panel opens
        // straight down from the pill.
        let anchor = (800, 300);
        assert_eq!(floating_origin(anchor, origin, monitor, (320, 76)), (640, 300));
        assert_eq!(floating_origin(anchor, origin, monitor, (540, 580)), (530, 300));
        // Left near a corner, the window is moved in just far enough to fit.
        assert_eq!(floating_origin((110, 60), origin, monitor, (320, 76)), (100, 60));
        assert_eq!(floating_origin((2010, 1100), origin, monitor, (540, 580)), (1480, 550));
        // A window larger than the monitor starts at its corner.
        assert_eq!(floating_origin((500, 500), origin, (400, 300), (540, 580)), (100, 50));
        // Never moved: centred, a sixth of the way down.
        assert_eq!(default_anchor(origin, monitor), (1060, 230));

        // Settings saved before this existed read as the notch, on screen.
        let old: Selection = serde_json::from_str(r#"{"chat":"a","embed":"b","placement":"right"}"#).unwrap();
        assert_eq!((old.style, old.float_anchor, old.tray_only), (Style::Notch, None, false));
        let new: Selection = serde_json::from_str(r#"{"chat":null,"embed":null,"style":"floating","float_anchor":[800,300],"tray_only":true}"#).unwrap();
        assert_eq!((new.style, new.float_anchor, new.tray_only), (Style::Floating, Some((800, 300)), true));
    }

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
            let (cw, ch) = notch_size(placement, false, PANEL_SIZE);
            let (ew, eh) = notch_size(placement, true, PANEL_SIZE);
            assert!(ew > cw && eh > ch, "{placement:?}: the panel is larger than the pill");
        }
        // The normal panel's windows are the sizes the UI is laid out for.
        assert_eq!(notch_size(Placement::Top, true, PANEL_SIZE), (540.0, 580.0));
        assert_eq!(notch_size(Placement::Right, true, PANEL_SIZE), (520.0, 600.0));
    }

    #[test]
    fn the_large_panel_is_twice_the_size_where_it_fits() {
        assert_eq!(panel_size(false, Some((2560.0, 1400.0))), PANEL_SIZE);
        // Room to spare: twice as wide and twice as tall.
        assert_eq!(panel_size(true, Some((2560.0, 1400.0))), (920.0, 1040.0));
        assert_eq!(panel_size(true, None), (920.0, 1040.0));
        // A 1920x1080 display at 125% with a taskbar (1536x816 usable):
        // as tall as fits, still twice as wide.
        let (w, h) = panel_size(true, Some((1536.0, 816.0)));
        assert_eq!(w, 920.0);
        assert_eq!(h, 816.0 - 80.0 - LARGE_GAP);
        // Its window fits on that display at either edge.
        for placement in [Placement::Top, Placement::Right] {
            let (ww, wh) = notch_size(placement, true, (w, h));
            assert!(ww <= 1536.0 && wh <= 816.0, "{placement:?}: {ww}x{wh}");
        }
        // Room for more in one direction only: larger in that one.
        assert_eq!(panel_size(true, Some((800.0, 560.0))), (800.0 - 80.0 - LARGE_GAP, 520.0));
        // A display too small for any more: never smaller than normal.
        assert_eq!(panel_size(true, Some((500.0, 500.0))), PANEL_SIZE);
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
