//! M4 — indexing jobs, file watching and the folder commands.
//!
//! Wraps the Tauri-free indexer core with:
//!  * a per-folder job queue (one index run at a time across all folders;
//!    requests made while a run is in flight coalesce into one follow-up run),
//!  * file-system watchers with debouncing, so edits re-index automatically,
//!  * the `list/add/reindex/delete_index/remove` folder commands,
//!  * `folders:changed` events so the UI can refresh.
//!
//! Folder statuses: `pending` (waiting to be indexed), `indexing`, `ready`,
//! `error`, and `idle` (the user deleted its index — left alone until they
//! ask for a re-index).

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use notify::{EventKind, RecursiveMode, Watcher};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};

use crate::indexer::{self, EmbedTarget, Progress};
use crate::runtime::ServerState;
use crate::AppState;

struct Job {
    /// A run is wanted; the bool is "force a full re-index".
    pending: Option<bool>,
    /// Set to stop the run currently in flight.
    cancel: Arc<AtomicBool>,
}

pub struct IndexState {
    jobs: Mutex<HashMap<i64, Job>>,
    /// Serialises index runs and index-deleting operations.
    lock: Arc<tokio::sync::Mutex<()>>,
    progress: Mutex<HashMap<i64, Progress>>,
    errors: Mutex<HashMap<i64, String>>,
    watchers: Mutex<HashMap<i64, notify::RecommendedWatcher>>,
    watch_tx: UnboundedSender<i64>,
}

impl IndexState {
    pub fn new() -> (Self, UnboundedReceiver<i64>) {
        let (watch_tx, watch_rx) = unbounded_channel();
        (
            IndexState {
                jobs: Mutex::new(HashMap::new()),
                lock: Arc::new(tokio::sync::Mutex::new(())),
                progress: Mutex::new(HashMap::new()),
                errors: Mutex::new(HashMap::new()),
                watchers: Mutex::new(HashMap::new()),
                watch_tx,
            },
            watch_rx,
        )
    }
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct FolderInfo {
    id: String,
    path: String,
    status: String,
    file_count: u32,
    chunk_count: u32,
    progress: Option<Progress>,
    error: Option<String>,
}

fn parse_id(id: &str) -> Result<i64, String> {
    id.parse::<i64>().map_err(|_| format!("bad folder id: {id}"))
}

fn emit_changed(app: &AppHandle) {
    let _ = app.emit("folders:changed", ());
}

fn set_status(app: &AppHandle, folder_id: i64, status: &str) {
    let state = app.state::<AppState>();
    let _ = state.store.lock().unwrap().set_folder_status(folder_id, status);
}

fn set_error(app: &AppHandle, folder_id: i64, error: Option<String>) {
    let state = app.state::<AppState>();
    let mut errors = state.index.errors.lock().unwrap();
    match error {
        Some(e) => {
            errors.insert(folder_id, e);
        }
        None => {
            errors.remove(&folder_id);
        }
    }
}

/// The running embedding server, if it is ready.
fn embed_target(app: &AppHandle) -> Option<EmbedTarget> {
    let state = app.state::<AppState>();
    let (port, model_id) = {
        let rt = state.runtime.lock().unwrap();
        if rt.embed_state != ServerState::Ready {
            return None;
        }
        let server = rt.embed.as_ref()?;
        (server.port, server.model_id.clone())
    };
    let doc_prefix = state
        .entry(&model_id)
        .map(|e| e.doc_prefix.clone())
        .unwrap_or_default();
    Some(EmbedTarget { port, doc_prefix })
}

/// The picture model chosen in Settings, ready to be started on first use.
/// `None` when picture descriptions are off or the model is not downloaded.
fn picture_describer(app: &AppHandle) -> Option<crate::vision::Describer> {
    let state = app.state::<AppState>();
    let id = state.selection.lock().unwrap().vision.clone()?;
    let entry = state.entry(&id)?;
    if entry.mmproj_file.is_empty() || !state.is_installed(entry) {
        return None;
    }
    let (model, mmproj) = (state.model_path(entry), state.models_dir.join(&entry.mmproj_file));
    let app = app.clone();
    Some(crate::vision::Describer::new(move || {
        let state = app.state::<AppState>();
        let mut runtime = state.runtime.lock().unwrap();
        runtime.spawn_vision(&model, &mmproj, &id)
    }))
}

/// Read every indexed picture again: called when picture descriptions are
/// turned on, off, or switched to another model. Returns how many pictures.
pub fn reread_pictures(app: &AppHandle) -> Result<usize, String> {
    let (forgotten, folders) = {
        let state = app.state::<AppState>();
        let mut store = state.store.lock().unwrap();
        (store.forget_kind("image")?, store.list_folders()?)
    };
    for folder in folders {
        if folder.status != "idle" {
            request_index(app, folder.id, false);
        }
    }
    emit_changed(app);
    Ok(forgotten)
}

// ------------------------------- jobs ------------------------------------

/// Ask for `folder_id` to be (re)indexed. `force` rebuilds it from scratch.
pub fn request_index(app: &AppHandle, folder_id: i64, force: bool) {
    let spawn_worker = {
        let state = app.state::<AppState>();
        let mut jobs = state.index.jobs.lock().unwrap();
        match jobs.get_mut(&folder_id) {
            Some(job) => {
                // Coalesce with the queued/running job.
                job.pending = Some(job.pending.unwrap_or(false) || force);
                if force {
                    job.cancel.store(true, Ordering::Relaxed);
                }
                false
            }
            None => {
                jobs.insert(
                    folder_id,
                    Job {
                        pending: Some(force),
                        cancel: Arc::new(AtomicBool::new(false)),
                    },
                );
                true
            }
        }
    };
    if spawn_worker {
        tauri::async_runtime::spawn(worker(app.clone(), folder_id));
    }
}

/// Stop any run for this folder and drop queued requests.
fn cancel_folder(app: &AppHandle, folder_id: i64) {
    let state = app.state::<AppState>();
    let mut jobs = state.index.jobs.lock().unwrap();
    if let Some(job) = jobs.get_mut(&folder_id) {
        job.pending = None;
        job.cancel.store(true, Ordering::Relaxed);
    }
}

async fn worker(app: AppHandle, folder_id: i64) {
    loop {
        // Take the next request, or retire this worker.
        let next = {
            let state = app.state::<AppState>();
            let mut jobs = state.index.jobs.lock().unwrap();
            let taken = jobs.get_mut(&folder_id).and_then(|job| {
                job.pending.take().map(|force| {
                    job.cancel = Arc::new(AtomicBool::new(false));
                    (force, job.cancel.clone())
                })
            });
            if taken.is_none() {
                jobs.remove(&folder_id);
            }
            taken
        };
        let Some((force, cancel)) = next else {
            break;
        };

        let lock = app.state::<AppState>().index.lock.clone();
        let guard = lock.lock().await;
        run_job(&app, folder_id, force, cancel).await;
        drop(guard);
    }
}

async fn run_job(app: &AppHandle, folder_id: i64, force: bool, cancel: Arc<AtomicBool>) {
    if cancel.load(Ordering::Relaxed) {
        return;
    }
    // An indexing run keeps the embedding server where it is: a change of
    // engine waits until the run has ended.
    let _busy = crate::engine::begin(app, &[crate::inference::Engine::Embed]);
    let (store, folder) = {
        let state = app.state::<AppState>();
        let folder = state.store.lock().unwrap().folder(folder_id).ok().flatten();
        (state.store.clone(), folder)
    };
    let Some(folder) = folder else {
        return; // removed in the meantime
    };

    let Some(target) = embed_target(app) else {
        set_status(app, folder_id, "pending");
        set_error(
            app,
            folder_id,
            Some("Choose an embedding model in Settings to index this folder.".into()),
        );
        emit_changed(app);
        return;
    };
    let root = PathBuf::from(&folder.path);
    if !root.is_dir() {
        set_status(app, folder_id, "error");
        set_error(app, folder_id, Some("Folder not found.".into()));
        emit_changed(app);
        return;
    }

    if force {
        if let Err(err) = store.lock().unwrap().delete_folder_index(folder_id) {
            set_status(app, folder_id, "error");
            set_error(app, folder_id, Some(err));
            emit_changed(app);
            return;
        }
    }
    set_status(app, folder_id, "indexing");
    set_error(app, folder_id, None);
    emit_changed(app);

    // Started only if a picture actually needs describing; stopped when this
    // run ends (it is dropped below).
    let describer = picture_describer(app);
    let progress_app = app.clone();
    let mut last_emit = Instant::now() - Duration::from_secs(1);
    let result = indexer::index_folder_with(store, folder_id, root, target, describer.as_ref(), cancel, move |p| {
        let state = progress_app.state::<AppState>();
        state.index.progress.lock().unwrap().insert(folder_id, p.clone());
        // Throttle UI refreshes; always send the final tick.
        if last_emit.elapsed() >= Duration::from_millis(300) || p.done == p.total {
            last_emit = Instant::now();
            emit_changed(&progress_app);
        }
    })
    .await;
    drop(describer);

    app.state::<AppState>()
        .index
        .progress
        .lock()
        .unwrap()
        .remove(&folder_id);

    match result {
        Ok(summary) if summary.cancelled => set_status(app, folder_id, "pending"),
        Ok(summary) => {
            set_status(app, folder_id, "ready");
            if let Some(note) = run_note(summary.failed, summary.partial) {
                set_error(app, folder_id, Some(note));
            }
        }
        Err(err) => {
            set_status(app, folder_id, "error");
            set_error(app, folder_id, Some(err));
        }
    }
    emit_changed(app);
}

/// What the folder shows after a run, when something was left out.
fn run_note(failed: u32, partial: u32) -> Option<String> {
    let files = |n: u32| if n == 1 { "file" } else { "files" };
    let mut notes = Vec::new();
    if failed > 0 {
        notes.push(format!("{failed} {} could not be read.", files(failed)));
    }
    if partial > 0 {
        let (verb, each) = if partial == 1 { ("was", "") } else { ("were", " each") };
        notes.push(format!(
            "{partial} very large {} {verb} only partly indexed (the first {} chunks{each}).",
            files(partial),
            indexer::MAX_CHUNKS_PER_FILE,
        ));
    }
    (!notes.is_empty()).then(|| notes.join(" "))
}

#[cfg(test)]
mod note_tests {
    use super::run_note;

    #[test]
    fn a_run_says_what_was_left_out() {
        assert_eq!(run_note(0, 0), None);
        assert_eq!(run_note(1, 0).as_deref(), Some("1 file could not be read."));
        assert_eq!(run_note(0, 1).as_deref(), Some("1 very large file was only partly indexed (the first 2000 chunks)."));
        assert_eq!(
            run_note(3, 2).as_deref(),
            Some("3 files could not be read. 2 very large files were only partly indexed (the first 2000 chunks each).")
        );
    }
}

// ------------------------------ watching ---------------------------------

/// Watch `path` recursively; changes are debounced into index requests.
pub fn start_watcher(app: &AppHandle, folder_id: i64, path: &str) {
    let state = app.state::<AppState>();
    let tx = state.index.watch_tx.clone();
    let root = PathBuf::from(path);
    let watch_root = root.clone();
    let watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        let Ok(event) = res else { return };
        if matches!(event.kind, EventKind::Access(_)) {
            return;
        }
        // Ignore churn inside hidden / dependency / build directories.
        let relevant = event.paths.iter().any(|p| match p.strip_prefix(&root) {
            Ok(rel) => !indexer::is_ignored_relative(rel),
            Err(_) => true,
        });
        if relevant {
            let _ = tx.send(folder_id);
        }
    });
    match watcher {
        Ok(mut watcher) => {
            if let Err(err) = watcher.watch(Path::new(&watch_root), RecursiveMode::Recursive) {
                eprintln!("Loft: cannot watch {path}: {err}");
                return;
            }
            state.index.watchers.lock().unwrap().insert(folder_id, watcher);
        }
        Err(err) => eprintln!("Loft: cannot create watcher for {path}: {err}"),
    }
}

fn stop_watcher(app: &AppHandle, folder_id: i64) {
    let state = app.state::<AppState>();
    state.index.watchers.lock().unwrap().remove(&folder_id);
}

/// Collect watcher events until things go quiet, then index the dirty folders.
pub fn spawn_debouncer(app: AppHandle, mut rx: UnboundedReceiver<i64>) {
    tauri::async_runtime::spawn(async move {
        while let Some(first) = rx.recv().await {
            let mut dirty = HashSet::from([first]);
            let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
            loop {
                let quiet = tokio::time::sleep(Duration::from_millis(1500));
                tokio::select! {
                    more = rx.recv() => match more {
                        Some(id) => { dirty.insert(id); }
                        None => return,
                    },
                    _ = quiet => break,
                }
                if tokio::time::Instant::now() >= deadline {
                    break; // constant churn: index anyway
                }
            }
            for folder_id in dirty {
                let status = {
                    let state = app.state::<AppState>();
                    let folder = state.store.lock().unwrap().folder(folder_id).ok().flatten();
                    folder.map(|f| f.status)
                };
                // 'idle' = the user deleted this index; don't rebuild it behind their back.
                if matches!(status.as_deref(), Some(s) if s != "idle") {
                    request_index(&app, folder_id, false);
                }
            }
        }
    });
}

/// Called whenever the embedding server becomes ready (app start, or a model
/// switch): make the vector table match the model, then bring every folder up
/// to date. A model change resets the index, so those passes re-embed everything.
pub async fn on_embed_ready(app: AppHandle) {
    let (port, model_id) = {
        let state = app.state::<AppState>();
        let rt = state.runtime.lock().unwrap();
        match rt.embed.as_ref() {
            Some(server) => (server.port, server.model_id.clone()),
            None => return,
        }
    };
    let dim = match crate::embed::embed_one(port, "dimension probe").await {
        Ok(v) => v.len(),
        Err(err) => {
            eprintln!("Loft: embedding probe failed: {err}");
            return;
        }
    };
    let folders = {
        let state = app.state::<AppState>();
        let mut store = state.store.lock().unwrap();
        if let Err(err) = store.ensure_embedding_space(&model_id, dim) {
            eprintln!("Loft: could not prepare the vector index: {err}");
            return;
        }
        let rows = store.list_folders().unwrap_or_default();
        rows
    };
    for folder in folders {
        let watching = app
            .state::<AppState>()
            .index
            .watchers
            .lock()
            .unwrap()
            .contains_key(&folder.id);
        if !watching {
            start_watcher(&app, folder.id, &folder.path);
        }
        if folder.status != "idle" {
            request_index(&app, folder.id, false);
        }
    }
    emit_changed(&app);
}

// ------------------------------ commands ---------------------------------

fn folder_info(state: &AppState, row: crate::store::FolderRow) -> FolderInfo {
    FolderInfo {
        id: row.id.to_string(),
        progress: state.index.progress.lock().unwrap().get(&row.id).cloned(),
        error: state.index.errors.lock().unwrap().get(&row.id).cloned(),
        path: row.path,
        status: row.status,
        file_count: row.file_count,
        chunk_count: row.chunk_count,
    }
}

#[tauri::command]
pub fn list_folders(state: State<AppState>) -> Result<Vec<FolderInfo>, String> {
    let rows = state.store.lock().unwrap().list_folders()?;
    Ok(rows.into_iter().map(|row| folder_info(&state, row)).collect())
}

#[tauri::command]
pub async fn add_folder(app: AppHandle) -> Result<Option<FolderInfo>, String> {
    use tauri_plugin_dialog::DialogExt;
    let (tx, rx) = tokio::sync::oneshot::channel();
    let mut dialog = app.dialog().file().set_title("Choose a folder for Loft to index");
    if let Some(window) = app.get_webview_window("notch") {
        dialog = dialog.set_parent(&window);
    }
    dialog.pick_folder(move |picked| {
        let _ = tx.send(picked);
    });
    let Some(picked) = rx.await.ok().flatten() else {
        return Ok(None); // cancelled
    };
    let path = picked
        .into_path()
        .map_err(|e| format!("unsupported folder location: {e}"))?;
    let path = path.to_string_lossy().into_owned();

    let (folder_id, info) = {
        let state = app.state::<AppState>();
        let store = state.store.lock().unwrap();
        let id = store.add_folder(&path)?;
        let row = store.folder(id)?.ok_or("folder vanished")?;
        drop(store);
        (id, folder_info(&state, row))
    };
    start_watcher(&app, folder_id, &path);
    request_index(&app, folder_id, false);
    emit_changed(&app);
    Ok(Some(info))
}

#[tauri::command]
pub fn reindex_folder(app: AppHandle, id: String) -> Result<(), String> {
    let folder_id = parse_id(&id)?;
    set_status(&app, folder_id, "pending");
    request_index(&app, folder_id, true);
    emit_changed(&app);
    Ok(())
}

#[tauri::command]
pub async fn delete_index(app: AppHandle, id: String) -> Result<(), String> {
    let folder_id = parse_id(&id)?;
    cancel_folder(&app, folder_id);
    // Wait for any in-flight run to notice the cancel and finish.
    let lock = app.state::<AppState>().index.lock.clone();
    let guard = lock.lock().await;
    let result = {
        let state = app.state::<AppState>();
        let result = state.store.lock().unwrap().delete_folder_index(folder_id);
        result
    };
    drop(guard);
    set_error(&app, folder_id, None);
    emit_changed(&app);
    result
}

#[tauri::command]
pub async fn remove_folder(app: AppHandle, id: String) -> Result<(), String> {
    let folder_id = parse_id(&id)?;
    cancel_folder(&app, folder_id);
    stop_watcher(&app, folder_id);
    let lock = app.state::<AppState>().index.lock.clone();
    let guard = lock.lock().await;
    let result = {
        let state = app.state::<AppState>();
        let result = state.store.lock().unwrap().remove_folder(folder_id);
        result
    };
    drop(guard);
    set_error(&app, folder_id, None);
    emit_changed(&app);
    result
}
