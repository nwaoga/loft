//! The transition coordinator: the one place where Loft's model servers are
//! started, switched and restarted.
//!
//! Model selection, the "Performance & Battery" settings, a temporary
//! override, a change of power source and crash recovery all end in the same
//! call, `reconcile`. It compares what is wanted (`inference::plan`) with what
//! is running (`inference::decide`) and, only if they differ, restarts the
//! affected server. Because every path goes through it:
//!
//! * transitions are serialised (one lock), so two triggers can never
//!   restart the same server at the same time;
//! * a repeated request is a no-op;
//! * a server in the middle of work is left alone: the switch is marked
//!   pending and carried out when the work ends (see `Activity`);
//! * after a start, the server's own load log is checked, so the device
//!   reported is the one that really holds the model, and a failed start
//!   never leaves the runtime saying "ready".

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};

use crate::inference::{
    self, Backend, BackendStatus, ChoiceSource, Decision, Engine, Inventory, Plan, PowerDebounce, PowerSettings,
    PowerSource, Resolved, Running, Target, Verification,
};
use crate::runtime::{self, ServerKind, ServerState};
use crate::AppState;

/// How long a new power state must hold before Loft acts on it.
const POWER_SETTLE: Duration = Duration::from_secs(3);
/// How long a model may take to load.
const LOAD_TIMEOUT: Duration = Duration::from_secs(180);

fn kind_of(engine: Engine) -> ServerKind {
    match engine {
        Engine::Chat => ServerKind::Chat,
        Engine::Embed => ServerKind::Embed,
    }
}

// ------------------------------- activity -----------------------------------

/// Work in progress per engine. A server with work in progress is never
/// stopped; `Activity::begin` returns a guard, and when the last guard for an
/// engine is dropped any switch that was waiting goes ahead.
#[derive(Default)]
pub struct Activity {
    chat: AtomicUsize,
    embed: AtomicUsize,
}

impl Activity {
    fn counter(&self, engine: Engine) -> &AtomicUsize {
        match engine {
            Engine::Chat => &self.chat,
            Engine::Embed => &self.embed,
        }
    }

    pub fn busy(&self, engine: Engine) -> bool {
        self.counter(engine).load(Ordering::SeqCst) > 0
    }
}

/// Marks `engines` as busy until dropped.
pub struct Busy {
    app: AppHandle,
    engines: Vec<Engine>,
}

/// Declare that work is starting on `engines` (an answer, an indexing run).
pub fn begin(app: &AppHandle, engines: &[Engine]) -> Busy {
    let state = app.state::<AppState>();
    for engine in engines {
        state.engines.activity.counter(*engine).fetch_add(1, Ordering::SeqCst);
    }
    Busy {
        app: app.clone(),
        engines: engines.to_vec(),
    }
}

impl Drop for Busy {
    fn drop(&mut self) {
        let state = self.app.state::<AppState>();
        let mut idle = false;
        for engine in &self.engines {
            if state.engines.activity.counter(*engine).fetch_sub(1, Ordering::SeqCst) == 1 {
                idle = true;
            }
        }
        // The work ended, whether it finished, failed or was cancelled: a
        // switch that was waiting for it can go ahead now.
        let waiting = state.engines.state.lock().map(|s| !s.pending.is_empty()).unwrap_or(false);
        if idle && waiting {
            let app = self.app.clone();
            tauri::async_runtime::spawn(async move { reconcile(&app, "work finished").await });
        }
    }
}

// --------------------------------- state ------------------------------------

/// What happened the last time an engine was (re)started.
#[derive(Clone, Default)]
struct Record {
    plan: Option<Plan>,
    /// What it is running on, when it is running.
    active: Option<Resolved>,
    /// What the load log showed.
    verification: Option<Verification>,
    /// Why something other than the first choice is running.
    fallback_reason: Option<String>,
    /// Why nothing is running.
    error: Option<String>,
    /// What to do about a failure, where there is something to do.
    hint: Option<String>,
}

pub struct InferenceState {
    pub settings: PowerSettings,
    /// The settled power source.
    pub power: PowerSource,
    /// "Try another engine now": a temporary choice for the chat model. It
    /// is never saved and never changed by a power notification.
    pub override_target: Option<Target>,
    /// Targets that could not run an engine's model since the settings last
    /// changed. They are skipped, so a fallback is not retried in a loop.
    failed: HashMap<Engine, HashSet<Target>>,
    /// Engines whose server keeps crashing; left stopped until retried.
    gave_up: HashSet<Engine>,
    /// Engines with a switch waiting for work to finish.
    pending: HashSet<Engine>,
    switching: bool,
    records: HashMap<Engine, Record>,
    /// The embedding model the index was last brought up to date for.
    indexed_with: Option<String>,
    /// Development builds: a target that is made to fail, to test fallback.
    pub dev_fail: Option<Target>,
}

impl InferenceState {
    fn new(settings: PowerSettings, power: PowerSource) -> Self {
        InferenceState {
            settings,
            power,
            override_target: None,
            failed: HashMap::new(),
            gave_up: HashSet::new(),
            pending: HashSet::new(),
            switching: false,
            records: HashMap::new(),
            indexed_with: None,
            dev_fail: None,
        }
    }

    /// Forget past failures: the settings changed, or the user asked to retry.
    fn forgive(&mut self) {
        self.failed.clear();
        self.gave_up.clear();
    }
}

pub struct Coordinator {
    pub state: Mutex<InferenceState>,
    /// Held for the whole of a transition.
    transition: Arc<tokio::sync::Mutex<()>>,
    pub activity: Activity,
    /// True from launch until the engines have been looked for and the
    /// models started (or found to be unstartable) for the first time.
    starting: AtomicBool,
    power_tx: UnboundedSender<PowerSource>,
    power_rx: Mutex<Option<UnboundedReceiver<PowerSource>>>,
}

impl Coordinator {
    pub fn new(settings: PowerSettings, power: PowerSource) -> Self {
        let (power_tx, power_rx) = unbounded_channel();
        Coordinator {
            state: Mutex::new(InferenceState::new(settings, power)),
            transition: Arc::new(tokio::sync::Mutex::new(())),
            activity: Activity::default(),
            starting: AtomicBool::new(true),
            power_tx,
            power_rx: Mutex::new(Some(power_rx)),
        }
    }

    /// True while a transition is being carried out.
    pub fn in_transition(&self) -> bool {
        self.transition.try_lock().is_err()
    }

    /// Loft is still getting the models going: it has only just launched,
    /// or a server is being started or switched right now. A server that is
    /// not running at such a moment is on its way, not missing.
    pub fn settling(&self) -> bool {
        self.starting.load(Ordering::SeqCst) || self.in_transition()
    }

    /// The lock every transition holds. The performance test holds it too,
    /// for as long as it has the chat server stopped.
    pub fn transition_lock(&self) -> Arc<tokio::sync::Mutex<()>> {
        self.transition.clone()
    }

    /// Feed a power reading in (from the operating system, or a test).
    pub fn power_reading(&self, source: PowerSource) {
        let _ = self.power_tx.send(source);
    }
}

// ------------------------------ status DTOs ---------------------------------

/// One engine, as shown in Settings.
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct EngineStatus {
    pub engine: Engine,
    /// The model it runs (or would run).
    pub model: Option<String>,
    /// stopped | loading | ready | error
    pub state: String,
    /// What was asked for: "Automatic (performance)", "Intel AI Boost (OpenVINO)".
    pub requested: String,
    pub requested_target: Option<Target>,
    /// What is running: "NVIDIA GeForce RTX 4050 Laptop GPU (Vulkan)".
    pub active: Option<String>,
    pub active_target: Option<Target>,
    pub active_backend: Option<Backend>,
    pub active_device: Option<String>,
    /// The device was confirmed from the server's own load log.
    pub verified: bool,
    /// What that log showed.
    pub detail: Option<String>,
    pub fallback_used: bool,
    pub fallback_reason: Option<String>,
    /// A switch is waiting for work in progress to end.
    pub pending: bool,
    pub notes: Vec<String>,
    pub error: Option<String>,
    /// What to do about a failure or a fallback, where there is something to do.
    pub hint: Option<String>,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct InferenceStatus {
    pub power_source: PowerSource,
    /// Which choice is in force: the override, plugged in, or on battery.
    pub profile: ChoiceSource,
    pub switching: bool,
    pub pending_switch: bool,
    pub override_target: Option<Target>,
    pub chat: EngineStatus,
    pub embed: EngineStatus,
    /// One line for the whole thing: "NVIDIA… (Vulkan)", "Switch pending — finishing current answer".
    pub message: String,
}

/// Something that can be chosen in a dropdown.
#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TargetOption {
    pub target: Target,
    /// "NVIDIA GeForce RTX 4050 Laptop GPU (Vulkan)"
    pub label: String,
    pub available: bool,
    pub experimental: bool,
    /// Why it cannot be used right now.
    pub reason: Option<String>,
}

/// An engine build that can be added to, or removed from, this computer.
#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct EngineDownload {
    pub backend: Backend,
    pub size_bytes: u64,
    pub note: Option<String>,
    /// It is in Loft's own engines folder (so it can be removed again).
    pub installed: bool,
    /// Why installing it here would be pointless, if it would.
    pub blocked: Option<String>,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct InferenceOverview {
    pub settings: PowerSettings,
    pub inventory: Inventory,
    pub options: Vec<TargetOption>,
    pub downloads: Vec<EngineDownload>,
    pub status: InferenceStatus,
}

/// The optional engines for this platform, from the pinned list.
pub fn engine_downloads(inventory: &Inventory, engines_dir: &std::path::Path, platform: &str) -> Vec<EngineDownload> {
    let manifest = crate::installer::manifest();
    inventory
        .backends
        .iter()
        .filter_map(|info| {
            let build = manifest.build(platform, info.backend).filter(|b| b.optional)?;
            let folder = engines_dir.join(format!("{platform}-{}", info.backend.dir_suffix()));
            Some(EngineDownload {
                backend: info.backend,
                size_bytes: build.size(),
                note: build.note.clone(),
                installed: folder.join(runtime::server_exe_name()).is_file(),
                blocked: (info.status == BackendStatus::HardwareUnavailable).then(|| info.detail.clone()).flatten(),
            })
        })
        .collect()
}

/// Everything selectable, built from what was actually detected, plus any
/// saved choice that is no longer available (shown, not silently dropped).
pub fn target_options(inventory: &Inventory, settings: &PowerSettings, extra: Option<&Target>) -> Vec<TargetOption> {
    let mut options: Vec<TargetOption> = Vec::new();
    for info in &inventory.backends {
        let name = info.backend.label();
        let experimental = info.experimental;
        if info.status != BackendStatus::Available {
            options.push(TargetOption {
                target: Target::new(info.backend, None),
                label: match info.status {
                    BackendStatus::NotInstalled => format!("{name} — not installed"),
                    _ => format!("{name} — unavailable"),
                },
                available: false,
                experimental,
                reason: info.detail.clone(),
            });
        } else if info.backend == Backend::Cpu {
            options.push(TargetOption {
                target: Target::cpu(),
                label: "CPU only".to_string(),
                available: true,
                experimental,
                reason: None,
            });
        } else if info.devices.is_empty() {
            options.push(TargetOption {
                target: Target::new(info.backend, None),
                label: name.to_string(),
                available: true,
                experimental,
                reason: None,
            });
        } else {
            for device in &info.devices {
                options.push(TargetOption {
                    target: Target::new(info.backend, Some(&device.key)),
                    label: format!("{} ({name})", inference::display_name(&device.name)),
                    available: true,
                    experimental,
                    reason: None,
                });
            }
        }
    }
    // Saved choices that are not on offer any more (a card was removed).
    let saved = [&settings.plugged_in, &settings.on_battery]
        .into_iter()
        .flat_map(|profile| [profile.target.as_ref(), profile.embed_target.as_ref()])
        .flatten()
        .chain(extra);
    for target in saved {
        if !options.iter().any(|o| &o.target == target) {
            options.push(TargetOption {
                target: target.clone(),
                label: format!("{} — unavailable", inference::target_label(inventory, target)),
                available: false,
                experimental: target.backend.experimental(),
                reason: inference::resolve(inventory, target).err(),
            });
        }
    }
    options
}

fn engine_status(app: &AppHandle, engine: Engine, inventory: &Inventory) -> EngineStatus {
    let state = app.state::<AppState>();
    let model = {
        let sel = state.selection.lock().unwrap();
        match engine {
            Engine::Chat => sel.chat.clone(),
            Engine::Embed => sel.embed.clone(),
        }
    };
    let server_state = {
        let mut rt = state.runtime.lock().unwrap();
        // A server that has died is not "ready", even in the moment before
        // crash recovery picks it up.
        match rt.state_of(kind_of(engine)) {
            ServerState::Ready if rt.exited(kind_of(engine)) => ServerState::Error,
            other => other,
        }
    };
    let measured = crate::bench::measured(app, inventory, engine);
    let model_file = model.as_deref().and_then(|id| state.entry(id)).map(|entry| entry.file.clone()).unwrap_or_default();
    let inf = state.engines.state.lock().unwrap();
    let record = inf.records.get(&engine).cloned().unwrap_or_default();
    // What would be asked for right now, even before anything has started.
    let plan = record
        .plan
        .clone()
        .unwrap_or_else(|| inference::plan(&inf.settings, inf.power, inf.override_target.as_ref(), inventory, engine, &measured));
    let running = matches!(server_state, ServerState::Ready | ServerState::Loading);
    let active = record.active.as_ref().filter(|_| running);
    let mut notes = plan.notes.clone();
    // Worth knowing about the engine that was asked for and this model.
    if engine == Engine::Chat {
        let asked = plan.requested.as_ref().and_then(|target| inference::resolve(inventory, target).ok());
        if let Some(note) = asked.and_then(|resolved| inference::compatibility_note(&resolved, &model_file)) {
            notes.push(note);
        }
    }
    EngineStatus {
        engine,
        model,
        state: server_state.as_str().to_string(),
        requested: plan.requested_label(inventory),
        requested_target: plan.requested.clone(),
        active: active.map(Resolved::label),
        active_target: active.map(|r| r.target.clone()),
        active_backend: active.map(|r| r.backend),
        active_device: active.map(Resolved::device_name),
        verified: active.is_some() && record.verification.as_ref().is_some_and(|v| v.ok),
        detail: record.verification.as_ref().map(|v| v.detail.clone()),
        fallback_used: active.is_some() && record.fallback_reason.is_some(),
        fallback_reason: record.fallback_reason.clone().filter(|_| active.is_some()),
        pending: inf.pending.contains(&engine),
        notes,
        error: record.error.clone().filter(|_| !running),
        hint: record.hint.clone(),
    }
}

pub fn status(app: &AppHandle) -> InferenceStatus {
    let state = app.state::<AppState>();
    let inventory = state.runtime.lock().unwrap().inventory().clone();
    let chat = engine_status(app, Engine::Chat, &inventory);
    let embed = engine_status(app, Engine::Embed, &inventory);
    let testing = state.bench.running();
    let inf = state.engines.state.lock().unwrap();
    let profile = inference::plan(&inf.settings, inf.power, inf.override_target.as_ref(), &inventory, Engine::Chat, &[]).source;
    let pending = !inf.pending.is_empty();
    let message = if testing {
        "Testing performance…".to_string()
    } else if inf.switching {
        match profile {
            ChoiceSource::OnBattery => "Switching to battery profile…".to_string(),
            ChoiceSource::PluggedIn => "Switching to plugged-in profile…".to_string(),
            ChoiceSource::Override => "Switching engine…".to_string(),
        }
    } else if pending {
        let what = if inf.pending.contains(&Engine::Chat) { "current answer" } else { "indexing" };
        format!("Switch pending — finishing {what}")
    } else if let Some(error) = &chat.error {
        error.clone()
    } else if let (true, Some(active)) = (chat.fallback_used, &chat.active) {
        format!("{} failed — fallback to {active}", chat.requested)
    } else {
        chat.active.clone().unwrap_or_else(|| "No chat model running".to_string())
    };
    InferenceStatus {
        power_source: inf.power,
        profile,
        switching: inf.switching,
        pending_switch: pending,
        override_target: inf.override_target.clone(),
        chat,
        embed,
        message,
    }
}

pub fn overview(app: &AppHandle) -> InferenceOverview {
    let state = app.state::<AppState>();
    let inventory = state.runtime.lock().unwrap().inventory().clone();
    let (settings, extra) = {
        let inf = state.engines.state.lock().unwrap();
        (inf.settings.clone(), inf.override_target.clone())
    };
    InferenceOverview {
        options: target_options(&inventory, &settings, extra.as_ref()),
        downloads: engine_downloads(&inventory, &state.engines_dir, runtime::platform_dir()),
        settings,
        inventory,
        status: status(app),
    }
}

/// Tell the UI: the engine status, and the older runtime status built from it.
pub fn emit_status(app: &AppHandle) {
    let _ = app.emit("inference:status", status(app));
    crate::emit_status(app);
}

// ------------------------------ transitions ---------------------------------

/// Wait until the `kind` server on `port` has loaded its model. Gives up at
/// once if the process exits, instead of waiting out the whole timeout.
async fn wait_ready(app: &AppHandle, kind: ServerKind, port: u16) -> bool {
    let deadline = tokio::time::Instant::now() + LOAD_TIMEOUT;
    while tokio::time::Instant::now() < deadline {
        if runtime::is_healthy(port).await {
            return true;
        }
        let exited = app.state::<AppState>().runtime.lock().unwrap().exited(kind);
        if exited {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(400)).await;
    }
    false
}

/// Start `kind` on one target and make sure the model really is there.
/// On any failure the server is stopped again and the reason returned.
async fn start_on(
    app: &AppHandle,
    kind: ServerKind,
    model_path: &std::path::Path,
    model_id: &str,
    ctx: u32,
    candidate: &Resolved,
) -> Result<Verification, String> {
    let state = app.state::<AppState>();
    let port = state.runtime.lock().unwrap().spawn_on(kind, model_path, model_id, ctx, candidate)?;
    emit_status(app);
    let healthy = wait_ready(app, kind, port).await;
    let mut rt = state.runtime.lock().unwrap();
    let report = rt.report_of(kind).unwrap_or_default();
    if !healthy {
        let reason = inference::load_failure(&report, rt.exited(kind));
        rt.stop(kind);
        return Err(reason);
    }
    // Development builds: pretend this target cannot run the model.
    let simulated = state.engines.state.lock().unwrap().dev_fail.as_ref() == Some(&candidate.target);
    if simulated {
        rt.stop(kind);
        return Err("simulated failure (development test)".to_string());
    }
    // Up and healthy is not proof: check where the weights went.
    let verification = inference::verify(candidate, &report);
    if !verification.ok {
        rt.stop(kind);
        return Err(verification.detail);
    }
    rt.set_state(kind, ServerState::Ready);
    Ok(verification)
}

async fn reconcile_engine(app: &AppHandle, engine: Engine, inventory: &Inventory) {
    let state = app.state::<AppState>();
    let kind = kind_of(engine);
    let model_id = {
        let sel = state.selection.lock().unwrap();
        match engine {
            Engine::Chat => sel.chat.clone(),
            Engine::Embed => sel.embed.clone(),
        }
    };
    let Some(model_id) = model_id else {
        return; // no model chosen: nothing to run
    };
    let Some((model_path, ctx)) = state
        .entry(&model_id)
        .filter(|entry| state.is_installed(entry))
        .map(|entry| (state.model_path(entry), entry.ctx))
    else {
        return; // not downloaded (yet)
    };

    // Engines that have been measured with this model come first in
    // "automatic"; without measurements the rule of thumb stands.
    let measured = crate::bench::measured(app, inventory, engine);
    let model_file = state.entry(&model_id).map(|entry| entry.file.clone()).unwrap_or_default();
    let (plan, failed, gave_up) = {
        let inf = state.engines.state.lock().unwrap();
        (
            inference::plan(&inf.settings, inf.power, inf.override_target.as_ref(), inventory, engine, &measured),
            inf.failed.get(&engine).cloned().unwrap_or_default(),
            inf.gave_up.contains(&engine),
        )
    };
    if gave_up {
        return; // it keeps crashing; left alone until the user retries
    }
    let running = {
        let mut rt = state.runtime.lock().unwrap();
        if rt.state_of(kind) == ServerState::Ready && !rt.exited(kind) {
            rt.resolved_of(kind).zip(rt.model_id_of(kind)).map(|(resolved, model_id)| Running {
                model_id,
                target: resolved.target,
            })
        } else {
            None
        }
    };
    let busy = state.engines.activity.busy(engine);

    match inference::decide(&plan, inventory, &model_id, running.as_ref(), &failed, busy) {
        Decision::Keep => {
            let mut inf = state.engines.state.lock().unwrap();
            inf.pending.remove(&engine);
            inf.records.entry(engine).or_default().plan = Some(plan);
        }
        Decision::Wait => {
            let mut inf = state.engines.state.lock().unwrap();
            inf.pending.insert(engine);
            inf.records.entry(engine).or_default().plan = Some(plan);
        }
        Decision::Stop(reason) => {
            {
                let mut rt = state.runtime.lock().unwrap();
                rt.stop(kind);
                rt.set_state(kind, ServerState::Error);
            }
            let mut inf = state.engines.state.lock().unwrap();
            inf.pending.remove(&engine);
            inf.records.insert(
                engine,
                Record {
                    plan: Some(plan),
                    error: Some(reason),
                    ..Record::default()
                },
            );
        }
        Decision::Start(candidates) => {
            {
                let mut inf = state.engines.state.lock().unwrap();
                inf.pending.remove(&engine);
                inf.switching = true;
                inf.records.entry(engine).or_default().plan = Some(plan.clone());
            }
            emit_status(app);

            let mut problems: Vec<String> = Vec::new();
            let mut hint: Option<String> = None;
            let mut started: Option<(Resolved, Verification)> = None;
            for candidate in &candidates {
                match start_on(app, kind, &model_path, &model_id, ctx, candidate).await {
                    Ok(verification) => {
                        started = Some((candidate.clone(), verification));
                        break;
                    }
                    Err(reason) => {
                        eprintln!("Loft: {} could not run the {} model: {reason}", candidate.label(), kind.as_str());
                        if problems.is_empty() {
                            hint = inference::failure_hint(&candidate.target, candidate.device.as_ref(), &model_file, &reason);
                        }
                        problems.push(format!("{} — {reason}", candidate.label()));
                        let mut inf = state.engines.state.lock().unwrap();
                        inf.failed.entry(engine).or_default().insert(candidate.target.clone());
                    }
                }
            }

            let record = match started {
                Some((resolved, verification)) => {
                    // Anything but the first choice is a fallback, and says why.
                    let first_choice = plan.chain.first() == Some(&resolved.target);
                    let earlier_failure = failed.iter().next().map(|t| format!("{} could not run this model", inference::target_label(inventory, t)));
                    Record {
                        plan: Some(plan),
                        active: Some(resolved),
                        verification: Some(verification),
                        fallback_reason: if first_choice { None } else { problems.first().cloned().or(earlier_failure).or_else(|| Some("the first choice is not available".to_string())) },
                        error: None,
                        hint: if first_choice { None } else { hint },
                    }
                }
                None => {
                    state.runtime.lock().unwrap().set_state(kind, ServerState::Error);
                    Record {
                        plan: Some(plan),
                        error: Some(problems.join("; ")),
                        hint,
                        ..Record::default()
                    }
                }
            };
            let ready = record.active.is_some();
            state.engines.state.lock().unwrap().records.insert(engine, record);

            // A new or changed embedding model: (re)build what needs indexing.
            // A change of device alone leaves the index as it is.
            if engine == Engine::Embed && ready {
                let changed = {
                    let mut inf = state.engines.state.lock().unwrap();
                    let changed = inf.indexed_with.as_deref() != Some(model_id.as_str());
                    inf.indexed_with = Some(model_id.clone());
                    changed
                };
                if changed {
                    crate::jobs::on_embed_ready(app.clone()).await;
                }
            }
        }
    }
}

/// Bring the model servers in line with what is wanted. Safe to call at any
/// time, from anywhere, for any reason: calls are serialised, and one that
/// finds nothing to change changes nothing.
pub async fn reconcile(app: &AppHandle, reason: &str) {
    let state = app.state::<AppState>();
    let lock = state.engines.transition.clone();
    let _transition = lock.lock().await;
    let inventory = state.runtime.lock().unwrap().inventory().clone();
    let _ = reason;
    // Embeddings first: indexing can begin while the chat model loads.
    for engine in [Engine::Embed, Engine::Chat] {
        reconcile_engine(app, engine, &inventory).await;
    }
    state.engines.state.lock().unwrap().switching = false;
    state.engines.starting.store(false, Ordering::SeqCst);
    drop(_transition); // so the status that goes out says "settled"
    emit_status(app);
}

/// The result of the last transition for `engine`, as an error to hand back
/// to whoever asked for it.
pub fn outcome(app: &AppHandle, engine: Engine) -> Result<(), String> {
    let state = app.state::<AppState>();
    let ready = state.runtime.lock().unwrap().state_of(kind_of(engine)) == ServerState::Ready;
    let inf = state.engines.state.lock().unwrap();
    match inf.records.get(&engine).and_then(|r| r.error.clone()) {
        Some(error) if !ready => Err(error),
        _ => Ok(()),
    }
}

// ------------------------------- triggers -----------------------------------

/// The settings changed: save them (the caller persists), forget failures
/// and apply.
pub async fn apply_settings(app: &AppHandle, settings: PowerSettings) {
    {
        let state = app.state::<AppState>();
        let mut inf = state.engines.state.lock().unwrap();
        inf.settings = settings;
        inf.forgive();
    }
    reconcile(app, "settings changed").await;
}

/// Start or end "Try another engine now". The saved profiles are untouched.
pub async fn set_override(app: &AppHandle, target: Option<Target>) {
    {
        let state = app.state::<AppState>();
        let mut inf = state.engines.state.lock().unwrap();
        inf.override_target = target;
        inf.forgive();
    }
    reconcile(app, "override").await;
}

/// `target` started but cannot do the work (it loaded, then failed to
/// answer). Treated like a target that failed to start: skipped until the
/// settings change or the user retries, so the fallback rules apply.
pub async fn mark_failed(app: &AppHandle, engine: Engine, target: &Target) {
    app.state::<AppState>().engines.state.lock().unwrap().failed.entry(engine).or_default().insert(target.clone());
    reconcile(app, "engine could not answer").await;
}

/// A model was selected (or is being retried): past failures no longer apply.
pub async fn model_changed(app: &AppHandle) {
    app.state::<AppState>().engines.state.lock().unwrap().forgive();
    reconcile(app, "model selected").await;
}

/// Look for engines and devices again, forget failures and try again.
pub async fn retry(app: &AppHandle) {
    let state = app.state::<AppState>();
    let app_for_scan = app.clone();
    let _ = tauri::async_runtime::spawn_blocking(move || {
        let state = app_for_scan.state::<AppState>();
        state.runtime.lock().unwrap().refresh_inventory();
    })
    .await;
    state.engines.state.lock().unwrap().forgive();
    reconcile(app, "retry").await;
}

/// Watch the power source and switch profiles when it changes. Readings are
/// debounced, so a loose charger does not restart anything, and a reading
/// that changes nothing does nothing.
pub fn watch_power(app: AppHandle) {
    let Some(mut readings) = app.state::<AppState>().engines.power_rx.lock().unwrap().take() else {
        return; // already watching
    };
    let for_os = app.clone();
    crate::power::watch(move |source| for_os.state::<AppState>().engines.power_reading(source));

    tauri::async_runtime::spawn(async move {
        let initial = app.state::<AppState>().engines.state.lock().unwrap().power;
        let mut power = PowerDebounce::new(initial, POWER_SETTLE);
        loop {
            // Sleep until a reading arrives or a pending change has settled.
            let wait = power.due().map(|due| due.saturating_duration_since(Instant::now()));
            let reading = match wait {
                Some(wait) => tokio::time::timeout(wait, readings.recv()).await.ok(),
                None => Some(readings.recv().await),
            };
            match reading {
                Some(Some(source)) => power.observe(source, Instant::now()),
                Some(None) => break, // the app is shutting down
                None => {}           // the settle time has passed
            }
            if let Some(settled) = power.poll(Instant::now()) {
                app.state::<AppState>().engines.state.lock().unwrap().power = settled;
                emit_status(&app);
                reconcile(&app, "power source changed").await;
            }
        }
    });
}

/// Crash recovery: a server that has exited is restarted through the same
/// coordinator as everything else, so it cannot collide with a switch.
pub fn supervise(app: AppHandle) {
    const MAX_CRASHES: u32 = 3;
    const CRASH_WINDOW: Duration = Duration::from_secs(300);
    tauri::async_runtime::spawn(async move {
        let mut crashes: HashMap<Engine, (u32, Instant)> = HashMap::new();
        loop {
            tokio::time::sleep(Duration::from_secs(3)).await;
            let state = app.state::<AppState>();
            if state.engines.in_transition() {
                continue; // a transition is handling the servers right now
            }
            let mut crashed = Vec::new();
            for engine in [Engine::Embed, Engine::Chat] {
                let kind = kind_of(engine);
                let mut rt = state.runtime.lock().unwrap();
                if rt.state_of(kind) == ServerState::Ready && rt.exited(kind) {
                    rt.stop(kind);
                    rt.set_state(kind, ServerState::Error);
                    crashed.push(engine);
                }
            }
            if crashed.is_empty() {
                continue;
            }
            for engine in crashed {
                let (count, last) = crashes.entry(engine).or_insert((0, Instant::now()));
                if last.elapsed() > CRASH_WINDOW {
                    *count = 0;
                }
                *count += 1;
                *last = Instant::now();
                let kind = kind_of(engine);
                if *count > MAX_CRASHES {
                    eprintln!("Loft: the {} server keeps crashing; not restarting it", kind.as_str());
                    let mut inf = state.engines.state.lock().unwrap();
                    inf.gave_up.insert(engine);
                    inf.records.entry(engine).or_default().error = Some(format!(
                        "The {} model keeps crashing. Try another engine or a smaller model in Settings, then Retry.",
                        kind.as_str()
                    ));
                } else {
                    eprintln!("Loft: the {} server stopped unexpectedly; restarting it ({count}/{MAX_CRASHES})", kind.as_str());
                }
            }
            reconcile(&app, "crash recovery").await;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::inference::{parse_devices, BackendInfo, Profile, ProfileMode};

    fn inventory() -> Inventory {
        let vulkan = "  Vulkan0: Intel(R) Iris(R) Xe Graphics (16193 MiB, 15425 MiB free)\n  Vulkan1: NVIDIA GeForce RTX 4050 Laptop GPU (5920 MiB, 5152 MiB free)\n";
        let info = |backend: Backend, status, list: &str, detail: Option<&str>| BackendInfo {
            backend,
            status,
            experimental: backend.experimental(),
            devices: parse_devices(backend, list),
            detail: detail.map(str::to_string),
        };
        Inventory {
            cpu: "Test CPU".into(),
            hardware: Vec::new(),
            backends: vec![
                info(Backend::Cuda, BackendStatus::NotInstalled, "", Some("The CUDA engine is an optional download.")),
                info(Backend::Vulkan, BackendStatus::Available, vulkan, None),
                info(Backend::OpenVino, BackendStatus::NotInstalled, "", Some("The OpenVINO engine is an optional download.")),
                info(Backend::Cpu, BackendStatus::Available, "", None),
            ],
            revision: "test".into(),
        }
    }

    #[test]
    fn options_show_the_devices_that_were_detected() {
        let inv = inventory();
        let options = target_options(&inv, &PowerSettings::default(), None);
        let labels: Vec<&str> = options.iter().map(|o| o.label.as_str()).collect();
        assert_eq!(
            labels,
            [
                "CUDA — not installed",
                "Intel Iris Xe Graphics (Vulkan)",
                "NVIDIA GeForce RTX 4050 Laptop GPU (Vulkan)",
                "OpenVINO — not installed",
                "CPU only",
            ]
        );
        // Real device names, each selectable by its stable key.
        assert_eq!(options[2].target, Target::new(Backend::Vulkan, Some("nvidia-geforce-rtx-4050-laptop-gpu")));
        assert!(options[1].available && options[2].available && options[4].available);
        // Missing engines are listed with the reason, and marked experimental where they are.
        assert!(!options[0].available);
        assert_eq!(options[0].reason.as_deref(), Some("The CUDA engine is an optional download."));
        assert!(options[3].experimental && !options[3].available);
    }

    #[test]
    fn a_saved_choice_that_has_gone_is_still_listed() {
        let inv = inventory();
        let gone = Target::new(Backend::Vulkan, Some("amd-radeon-rx-7600"));
        let settings = PowerSettings {
            on_battery: Profile {
                mode: ProfileMode::Manual,
                target: Some(gone.clone()),
                embed_target: None,
            },
            ..PowerSettings::default()
        };
        let options = target_options(&inv, &settings, None);
        let kept = options.iter().find(|o| o.target == gone).expect("the saved choice is listed");
        assert!(!kept.available);
        assert_eq!(kept.label, "amd-radeon-rx-7600 (Vulkan) — unavailable");
        assert!(kept.reason.as_deref().unwrap().contains("not available to Vulkan right now"));
        // A choice that is available is not listed twice.
        let fine = PowerSettings {
            plugged_in: Profile {
                mode: ProfileMode::Manual,
                target: Some(Target::cpu()),
                embed_target: None,
            },
            ..PowerSettings::default()
        };
        assert_eq!(target_options(&inv, &fine, None).len(), 5);
        // The temporary override is listed too if it is not otherwise on offer.
        assert_eq!(target_options(&inv, &PowerSettings::default(), Some(&gone)).len(), 6);
    }
}
