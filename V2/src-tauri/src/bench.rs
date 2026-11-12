//! "Test performance" and "Try … now": measure what an engine really does
//! with the selected chat model on this computer.
//!
//! A test runs each engine in turn, alone, with the same model, the same
//! prompt and the same context size, and records what happened: whether it
//! loaded, which device the engine's own log says holds the model, how long
//! loading took, the time to the first token, tokens per second, total time,
//! the process's peak memory and the memory taken on the device.
//!
//! Energy is only measured on battery, and only as the whole computer's
//! drain as its battery reports it. That is not the draw of one processor,
//! and where the battery reports nothing, efficiency is left *unmeasured*
//! rather than guessed.
//!
//! Every result is stored with what it was measured on (model file, context,
//! engine revision, processor, device, driver). A result whose identity no
//! longer matches is shown as out of date and is not used for anything.
//! Current results put "Automatic" in measured order; with none, Automatic
//! keeps its rule of thumb. Either way the user's own choice wins.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager};

use crate::engine;
use crate::inference::{self, AutoGoal, Backend, DeviceKind, Engine, Inventory, PowerSource, Resolved, Target};
use crate::runtime::{self, ServerKind};
use crate::AppState;

// -------------------------------- records -----------------------------------

/// What a measurement was taken with. If any of it changes, the measurement
/// says nothing about the present.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Identity {
    pub model_id: String,
    pub model_file: String,
    pub model_bytes: u64,
    /// The context size the server was started with.
    pub context: u32,
    /// The llama.cpp revision of the engine builds.
    pub revision: String,
    pub cpu: String,
    pub device: Option<String>,
    pub driver: Option<String>,
}

/// The chat model under test.
#[derive(Clone, Debug, PartialEq)]
pub struct Model {
    pub id: String,
    pub file: String,
    pub bytes: u64,
    pub ctx: u32,
}

/// What `model` on `resolved` would be measured as, right now.
pub fn identity(inventory: &Inventory, model: &Model, resolved: &Resolved) -> Identity {
    let device = resolved.device.as_ref();
    // The driver comes from the operating system's list: the same name, or
    // failing that the only device of that maker and kind.
    let driver = device.and_then(|d| {
        let same_name = inventory.hardware.iter().find(|h| h.name == d.name);
        let alike: Vec<_> = inventory.hardware.iter().filter(|h| h.vendor == d.vendor && h.kind == d.kind).collect();
        same_name.or(if alike.len() == 1 { Some(alike[0]) } else { None }).and_then(|h| h.driver.clone())
    });
    Identity {
        model_id: model.id.clone(),
        model_file: model.file.clone(),
        model_bytes: model.bytes,
        context: runtime::effective_ctx(model.ctx, resolved),
        revision: inventory.revision.clone(),
        cpu: inventory.cpu.clone(),
        device: device.map(|d| d.name.clone()),
        driver,
    }
}

/// One engine, measured once.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Measurement {
    pub target: Target,
    /// "NVIDIA GeForce RTX 4050 Laptop GPU (Vulkan)"
    pub label: String,
    pub identity: Identity,
    /// When, in seconds since 1970.
    pub at: u64,
    /// The power source during the test.
    pub power_source: PowerSource,
    pub ok: bool,
    pub error: Option<String>,
    /// The engine's own log confirmed where the model was loaded.
    pub verified: bool,
    /// What that log showed.
    pub actual: Option<String>,
    pub load_ms: Option<u64>,
    pub first_token_ms: Option<u64>,
    pub total_ms: Option<u64>,
    pub tokens_per_second: Option<f32>,
    pub answer_tokens: Option<u32>,
    /// The most memory the server process held, in MiB.
    pub peak_memory_mib: Option<u64>,
    /// What the engine says it put on the device (weights and context), in MiB.
    pub device_memory_mib: Option<u64>,
    /// The whole computer's battery drain during the test, in watts.
    pub drain_watts: Option<f32>,
    /// `drain_watts` per token per second: joules of battery per token.
    pub joules_per_token: Option<f32>,
}

impl Measurement {
    fn new(target: &Target, label: &str, identity: Identity, power_source: PowerSource) -> Self {
        Measurement {
            target: target.clone(),
            label: label.to_string(),
            identity,
            at: SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0),
            power_source,
            ok: false,
            error: None,
            verified: false,
            actual: None,
            load_ms: None,
            first_token_ms: None,
            total_ms: None,
            tokens_per_second: None,
            answer_tokens: None,
            peak_memory_mib: None,
            device_memory_mib: None,
            drain_watts: None,
            joules_per_token: None,
        }
    }

    fn on_battery(&self) -> bool {
        self.power_source == PowerSource::Battery
    }

    /// Usable as evidence: it ran, where it was meant to, and gave a speed.
    fn sound(&self) -> bool {
        self.ok && self.verified && self.tokens_per_second.is_some_and(|s| s > 0.0)
    }
}

/// The stored measurements (`benchmarks.json`).
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct Book {
    #[serde(default)]
    pub results: Vec<Measurement>,
}

impl Book {
    pub fn load(path: &Path) -> Book {
        std::fs::read_to_string(path).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
    }

    pub fn save(&self, path: &Path) {
        if let Ok(json) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(path, json);
        }
    }

    /// Keep `m` in place of any earlier measurement of the same engine with
    /// the same model on the same kind of power.
    pub fn record(&mut self, m: Measurement) {
        self.results
            .retain(|old| !(old.target == m.target && old.identity.model_id == m.identity.model_id && old.on_battery() == m.on_battery()));
        self.results.push(m);
    }
}

/// Why a stored measurement no longer applies, or `None` if it does.
pub fn stale_reason(m: &Measurement, inventory: &Inventory, model: &Model) -> Option<String> {
    if m.identity.model_id != model.id || m.identity.model_file != model.file || m.identity.model_bytes != model.bytes {
        return Some("measured with another model".to_string());
    }
    let Ok(resolved) = inference::resolve(inventory, &m.target) else {
        return Some("that engine or device is not available now".to_string());
    };
    let now = identity(inventory, model, &resolved);
    if now == m.identity {
        None
    } else if now.revision != m.identity.revision {
        Some("measured with an older version of the engine".to_string())
    } else if now.driver != m.identity.driver {
        Some("the device driver has changed since".to_string())
    } else if now.context != m.identity.context {
        Some("measured with another context size".to_string())
    } else {
        Some("the hardware has changed since".to_string())
    }
}

/// The measured order for "automatic", best first. Empty when nothing
/// suitable has been measured, which leaves the rule of thumb in charge.
///
/// * Performance: tokens per second, from tests run on mains power.
/// * Battery saving: battery energy per token, from tests run on battery.
///   Speed alone says nothing about battery life, so without an energy
///   figure there is no measured order.
///
/// The same engine measured twice differs by about a tenth, so results
/// within `TIE` of the best are not told apart by the numbers: among them
/// the order of `preference` (the rule of thumb) decides. That keeps
/// "automatic" from flipping between two engines that are equally good.
pub fn ranking(current: &[&Measurement], goal: AutoGoal, preference: &[Target]) -> Vec<Target> {
    let mut rows: Vec<(&Measurement, f32)> = match goal {
        AutoGoal::Performance => current
            .iter()
            .filter(|m| m.sound() && !m.on_battery())
            .map(|m| (*m, -m.tokens_per_second.unwrap_or(0.0)))
            .collect(),
        AutoGoal::BatterySaving => current
            .iter()
            .filter(|m| m.sound() && m.on_battery())
            .filter_map(|m| m.joules_per_token.filter(|j| *j > 0.0).map(|j| (*m, j)))
            .collect(),
    };
    rows.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
    // Scores are "lower is better" (negated speed, or joules per token).
    if let Some(best) = rows.first().map(|(_, score)| *score) {
        let tied = rows.iter().take_while(|(_, score)| as_good_as(best, *score)).count();
        let place = |m: &Measurement| preference.iter().position(|t| t == &m.target).unwrap_or(usize::MAX);
        rows[..tied].sort_by_key(|(m, _)| place(m)); // stable: unlisted ones keep their measured order
    }
    rows.into_iter().map(|(m, _)| m.target.clone()).collect()
}

/// How far apart two measurements must be before the difference counts.
const TIE: f32 = 0.15;

/// True if `score` is within `TIE` of `best` (both "lower is better").
fn as_good_as(best: f32, score: f32) -> bool {
    if best < 0.0 {
        score <= best * (1.0 - TIE) // negated speeds: at least 85% as fast
    } else {
        score <= best * (1.0 + TIE) // energy: at most 15% more
    }
}

/// A recommendation, with the measurement behind it.
#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Pick {
    pub target: Target,
    pub label: String,
    pub reason: String,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Advice {
    pub plugged_in: Option<Pick>,
    pub on_battery: Option<Pick>,
    /// What is and is not known about battery use.
    pub battery_note: String,
}

fn rate(tokens_per_second: f32) -> String {
    if tokens_per_second >= 10.0 {
        format!("{tokens_per_second:.0} tokens/s")
    } else {
        format!("{tokens_per_second:.1} tokens/s")
    }
}

/// What the current measurements suggest. Loft recommends; the user decides.
pub fn advice(current: &[&Measurement], inventory: &Inventory) -> Advice {
    let find = |target: &Target, battery: bool| current.iter().find(|m| &m.target == target && m.on_battery() == battery).copied();

    let fastest = ranking(current, AutoGoal::Performance, &inference::auto_chain(inventory, AutoGoal::Performance, &[]));
    let plugged_in = fastest.first().and_then(|t| find(t, false)).map(|best| {
        let own = best.tokens_per_second.unwrap_or(0.0);
        let speed = rate(own);
        let reason = match fastest.get(1).and_then(|t| find(t, false)) {
            // The runner-up measured about the same: the test does not separate them.
            Some(next) if as_good_as(-own.max(next.tokens_per_second.unwrap_or(0.0)), -own.min(next.tokens_per_second.unwrap_or(0.0))) => format!(
                "As fast as anything in the test: {speed}. {} measured about the same ({}); the difference is within what one run varies from the next.",
                next.label,
                rate(next.tokens_per_second.unwrap_or(0.0))
            ),
            Some(next) => format!("Fastest in the test: {speed} (next: {}, {}).", next.label, rate(next.tokens_per_second.unwrap_or(0.0))),
            None => format!("The only engine measured: {speed}."),
        };
        Pick {
            target: best.target.clone(),
            label: best.label.clone(),
            reason,
        }
    });

    let frugal = ranking(current, AutoGoal::BatterySaving, &inference::auto_chain(inventory, AutoGoal::BatterySaving, &[]));
    let on_battery = frugal.first().and_then(|t| find(t, true)).map(|best| Pick {
        target: best.target.clone(),
        label: best.label.clone(),
        reason: format!(
            "Least battery per token in the test: {:.1} J per token at {} (the whole computer drew {:.0} W).",
            best.joules_per_token.unwrap_or(0.0),
            rate(best.tokens_per_second.unwrap_or(0.0)),
            best.drain_watts.unwrap_or(0.0)
        ),
    });

    let tested_on_battery = current.iter().any(|m| m.on_battery() && m.sound());
    let battery_note = if on_battery.is_some() {
        "Battery figures are the whole computer's drain as its battery reports it (screen and everything else included), not the draw of one processor. They compare engines; they do not predict battery life.".to_string()
    } else if tested_on_battery {
        "Power efficiency is unmeasured: this computer's battery did not report its drain during the test. Automatic on battery keeps to its rule of thumb (it stays off the discrete graphics card).".to_string()
    } else {
        "Power efficiency is unmeasured: energy can only be measured while running on battery. Unplug and test again to compare engines by battery use; until then Automatic on battery keeps to its rule of thumb (it stays off the discrete graphics card).".to_string()
    };
    Advice {
        plugged_in,
        on_battery,
        battery_note,
    }
}

/// A stored measurement as shown in Settings.
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Row {
    #[serde(flatten)]
    pub measurement: Measurement,
    /// Why it no longer applies; `None` = current.
    pub stale: Option<String>,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Report {
    /// The chat model the rows are for.
    pub model: Option<String>,
    pub running: bool,
    pub rows: Vec<Row>,
    /// Engines that can be used but have no current measurement (a newly
    /// installed one, say). "Automatic" ranks them after the measured ones.
    pub unmeasured: Vec<String>,
    pub advice: Advice,
}

/// The labels of the usable `targets` that `rows` hold no current
/// measurement for. Empty while nothing at all has been measured.
pub fn unmeasured(rows: &[Row], targets: &[(Target, String)]) -> Vec<String> {
    let current: Vec<&Target> = rows.iter().filter(|r| r.stale.is_none()).map(|r| &r.measurement.target).collect();
    if current.is_empty() {
        return Vec::new();
    }
    targets.iter().filter(|(target, _)| !current.contains(&target)).map(|(_, label)| label.clone()).collect()
}

/// The rows for `model`: current ones first, fastest first.
pub fn rows_for(book: &Book, inventory: &Inventory, model: &Model) -> Vec<Row> {
    let mut rows: Vec<Row> = book
        .results
        .iter()
        .filter(|m| m.identity.model_id == model.id)
        .map(|m| Row {
            measurement: m.clone(),
            stale: stale_reason(m, inventory, model),
        })
        .collect();
    rows.sort_by(|a, b| {
        let key = |r: &Row| (r.stale.is_some(), r.measurement.on_battery(), !r.measurement.ok, -(r.measurement.tokens_per_second.unwrap_or(0.0) * 100.0) as i64);
        key(a).cmp(&key(b))
    });
    rows
}

// ------------------------------ the prompt ----------------------------------

/// Tokens generated in the measured run.
const BENCH_TOKENS: u32 = 96;

/// The same request for every engine: shaped like one of Loft's own (a
/// system prompt, numbered excerpts, a question), a few hundred tokens long.
fn bench_messages() -> serde_json::Value {
    let excerpts = [
        "[1] handbook.md — Opening hours. The shop is open Monday to Friday from 08:00 to 18:00 and on Saturday from 09:00 to 14:00. It is closed on Sundays and on public holidays. Deliveries are accepted at the back door until one hour before closing. The till is counted by two members of staff at the end of every day and the totals are written in the day book.",
        "[2] suppliers.md — The coffee beans come from BeanWorks on a twelve-month contract that renews every March. Orders are placed on Tuesdays for delivery on Thursdays. The contract is owned by the operations manager, who also approves any change of price above five per cent. Milk and bread are bought locally and paid for weekly.",
        "[3] training.md — New staff shadow an experienced colleague for their first three shifts. They learn the opening checklist, the coffee machine cleaning routine, how to take card and cash payments, and what to do if the fire alarm sounds. A short quiz at the end of the first week covers food safety and allergens.",
        "[4] maintenance.md — The coffee machine is descaled on the first Monday of each month and serviced by the supplier twice a year. The fridge temperatures are logged every morning. Anything above five degrees is reported to the shift lead, and the stock is moved to the spare fridge until an engineer has visited.",
    ];
    let user = format!(
        "Excerpts from the user's files:\n\n{}\n\nQuestion: Summarise how the shop runs day to day, in one paragraph of about eighty words.",
        excerpts.join("\n\n")
    );
    serde_json::json!([
        { "role": "system", "content": "You answer questions using only the excerpts provided. Be accurate and plain." },
        { "role": "user", "content": user },
    ])
}

/// A different, tiny request first, so the measured one does not include
/// one-off start-up work (compiling shaders, building a graph).
fn warmup_messages() -> serde_json::Value {
    serde_json::json!([{ "role": "user", "content": "Reply with the single word: ready" }])
}

/// The short question "Try … now" asks, to show the engine really answers.
fn trial_messages() -> serde_json::Value {
    serde_json::json!([
        { "role": "system", "content": "You are a concise assistant." },
        { "role": "user", "content": "In two sentences, explain what it means to run an AI model locally on a computer." },
    ])
}

/// One streamed request, timed.
#[derive(Clone, Debug)]
pub struct Run {
    pub text: String,
    pub first_token_ms: u64,
    pub total_ms: u64,
    pub tokens_per_second: Option<f32>,
    pub answer_tokens: u32,
}

/// Send `messages` to the chat server on `port` and time the answer.
pub async fn run_prompt(port: u16, messages: &serde_json::Value, max_tokens: u32, cancel: &(dyn Fn() -> bool + Send + Sync)) -> Result<Run, String> {
    let started = Instant::now();
    let mut first: Option<Duration> = None;
    let output = crate::rag::stream_chat(port, messages, max_tokens, 0.0, cancel, |_| {
        first.get_or_insert_with(|| started.elapsed());
    })
    .await?;
    let total = started.elapsed();
    let usage = output.usage.ok_or_else(|| "Cancelled.".to_string())?;
    let first = first.ok_or_else(|| "the engine produced no text".to_string())?;
    // The server's own figure; failing that, tokens over the time after the first.
    let generating = total.saturating_sub(first).as_secs_f32();
    let tokens_per_second = usage
        .tokens_per_second
        .or_else(|| (usage.answer_tokens > 1 && generating > 0.0).then(|| (usage.answer_tokens - 1) as f32 / generating));
    Ok(Run {
        text: output.text.trim().to_string(),
        first_token_ms: first.as_millis() as u64,
        total_ms: total.as_millis() as u64,
        tokens_per_second,
        answer_tokens: usage.answer_tokens,
    })
}

// ------------------------------- app state ----------------------------------

pub struct BenchState {
    book: Mutex<Book>,
    path: PathBuf,
    /// A test is in progress (the chat server is stopped while it runs).
    running: AtomicBool,
    cancel: AtomicBool,
}

impl BenchState {
    pub fn new(path: PathBuf) -> Self {
        BenchState {
            book: Mutex::new(Book::load(&path)),
            path,
            running: AtomicBool::new(false),
            cancel: AtomicBool::new(false),
        }
    }

    pub fn running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }

    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::SeqCst);
    }
}

/// The selected chat model, if it is downloaded.
fn current_model(state: &AppState) -> Option<(Model, PathBuf)> {
    let id = state.selection.lock().unwrap().chat.clone()?;
    let entry = state.entry(&id).filter(|e| state.is_installed(e))?;
    let path = state.model_path(entry);
    let bytes = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
    Some((
        Model {
            id,
            file: entry.file.clone(),
            bytes,
            ctx: entry.ctx,
        },
        path,
    ))
}

/// The measured order of targets for `engine` right now (see `ranking`).
/// Experimental engines are left out for the embedding model, which they
/// do not run reliably.
pub fn measured(app: &AppHandle, inventory: &Inventory, engine: Engine) -> Vec<Target> {
    let state = app.state::<AppState>();
    let Some((model, _)) = current_model(&state) else {
        return Vec::new();
    };
    let goal = {
        let inf = state.engines.state.lock().unwrap();
        inference::goal_for(&inf.settings, inf.power)
    };
    let book = state.bench.book.lock().unwrap();
    let current: Vec<&Measurement> = book.results.iter().filter(|m| stale_reason(m, inventory, &model).is_none()).collect();
    let mut targets = ranking(&current, goal, &inference::auto_chain(inventory, goal, &[]));
    if engine == Engine::Embed {
        targets.retain(|t| !t.backend.experimental());
    }
    targets
}

/// The stored results for the selected chat model, and what they suggest.
pub fn report(app: &AppHandle) -> Report {
    let state = app.state::<AppState>();
    let inventory = state.runtime.lock().unwrap().inventory().clone();
    let model = current_model(&state).map(|(model, _)| model);
    let settings = state.engines.state.lock().unwrap().settings.clone();
    let usable: Vec<(Target, String)> = engine::target_options(&inventory, &settings, None)
        .into_iter()
        .filter(|o| o.available)
        .map(|o| (o.target, o.label))
        .collect();
    let book = state.bench.book.lock().unwrap();
    let rows = model.as_ref().map(|m| rows_for(&book, &inventory, m)).unwrap_or_default();
    let current: Vec<&Measurement> = rows.iter().filter(|r| r.stale.is_none()).map(|r| &r.measurement).collect();
    Report {
        model: model.map(|m| m.id),
        running: state.bench.running(),
        advice: advice(&current, &inventory),
        unmeasured: unmeasured(&rows, &usable),
        rows,
    }
}

/// Progress of a running test, for the UI (`bench:progress`).
#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct Progress {
    running: bool,
    index: usize,
    total: usize,
    label: String,
    /// loading | measuring | done
    stage: &'static str,
}

/// Averages the battery's reported drain while something runs.
struct DrainSampler {
    stop: Arc<AtomicBool>,
    task: tauri::async_runtime::JoinHandle<Vec<f32>>,
}

impl DrainSampler {
    fn start() -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let task = tauri::async_runtime::spawn(async move {
            let mut samples = Vec::new();
            while !flag.load(Ordering::SeqCst) {
                if let Some(watts) = crate::power::battery_drain_watts() {
                    samples.push(watts);
                }
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
            samples
        });
        DrainSampler { stop, task }
    }

    async fn finish(self) -> Option<f32> {
        self.stop.store(true, Ordering::SeqCst);
        let samples = self.task.await.unwrap_or_default();
        (!samples.is_empty()).then(|| samples.iter().sum::<f32>() / samples.len() as f32)
    }
}

/// How long a model may take to load in a test.
const LOAD_TIMEOUT: Duration = Duration::from_secs(180);

/// Start the model on `resolved`, check where it went, and time the prompt.
/// Fills `m` in as far as it gets; the server is stopped when this returns.
async fn measure_on(
    app: &AppHandle,
    model: &Model,
    model_path: &Path,
    resolved: &Resolved,
    m: &mut Measurement,
    on_stage: &(dyn Fn(&'static str) + Send + Sync),
) -> Result<(), String> {
    let state = app.state::<AppState>();
    let cancelled = || state.bench.cancel.load(Ordering::SeqCst);

    let started = Instant::now();
    // Not kept by the runtime: it is this test's own server, and dropping it
    // (on any way out of this function) stops it.
    let mut server = state.runtime.lock().unwrap().start(model_path, &model.id, model.ctx, resolved, &[])?;
    loop {
        if runtime::is_healthy(server.port).await {
            break;
        }
        if server.exited() {
            return Err(inference::load_failure(&server.report(), true));
        }
        if cancelled() {
            return Err("Cancelled.".into());
        }
        if started.elapsed() > LOAD_TIMEOUT {
            return Err("the model did not load within three minutes".into());
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    m.load_ms = Some(started.elapsed().as_millis() as u64);

    // Development builds: pretend this target cannot run the model.
    if state.engines.state.lock().unwrap().dev_fail.as_ref() == Some(&resolved.target) {
        return Err("simulated failure (development test)".to_string());
    }
    // Running is not proof of where: read the engine's own load log.
    let report = server.report();
    let verification = inference::verify(resolved, &report);
    m.actual = Some(verification.detail.clone());
    m.verified = verification.ok;
    if !verification.ok {
        return Err(verification.detail);
    }
    m.device_memory_mib = resolved.device.as_ref().and_then(|d| match (resolved.backend, d.kind) {
        // OpenVINO names its buffers after itself, and on the processor
        // "the device" is system memory, which the peak figure already covers.
        (Backend::OpenVino, DeviceKind::Cpu) => None,
        (Backend::OpenVino, _) => report.engine_memory_mib("OPENVINO"),
        _ => report.device_memory_mib(&d.runtime_id),
    });

    on_stage("measuring");
    run_prompt(server.port, &warmup_messages(), 4, &cancelled).await?;
    let sampler = DrainSampler::start();
    let run = measured_run(server.port, &cancelled).await;
    let watts = sampler.finish().await;
    let run = run?;

    m.first_token_ms = Some(run.first_token_ms);
    m.total_ms = Some(run.total_ms);
    m.tokens_per_second = run.tokens_per_second;
    m.answer_tokens = Some(run.answer_tokens);
    m.peak_memory_mib = server.peak_memory_mib();
    if let (Some(watts), Some(speed)) = (watts, run.tokens_per_second.filter(|s| *s > 0.0)) {
        m.drain_watts = Some(watts);
        m.joules_per_token = Some(watts / speed);
    }
    Ok(())
}

/// A run this short is repeated, because on a fast engine it is over in a
/// second or two and one sample is noisy.
const REPEAT_UNDER: Duration = Duration::from_secs(5);

/// Time the benchmark prompt. On a fast engine it is run three times and the
/// middle result (by speed) is kept; on a slow one, once is long enough.
async fn measured_run(port: u16, cancel: &(dyn Fn() -> bool + Send + Sync)) -> Result<Run, String> {
    let first = run_prompt(port, &bench_messages(), BENCH_TOKENS, cancel).await?;
    if Duration::from_millis(first.total_ms) >= REPEAT_UNDER {
        return Ok(first);
    }
    // The repeats find the prompt already processed, so only their speed
    // is used: the times are those of the first run.
    let (first_token_ms, total_ms) = (first.first_token_ms, first.total_ms);
    let mut runs = vec![first];
    for _ in 0..2 {
        runs.push(run_prompt(port, &bench_messages(), BENCH_TOKENS, cancel).await?);
    }
    let mut run = middle_by_speed(runs);
    run.first_token_ms = first_token_ms;
    run.total_ms = total_ms;
    Ok(run)
}

/// The run with the middle tokens-per-second of an odd number of runs.
fn middle_by_speed(mut runs: Vec<Run>) -> Run {
    runs.sort_by(|a, b| a.tokens_per_second.partial_cmp(&b.tokens_per_second).unwrap_or(std::cmp::Ordering::Equal));
    let middle = runs.len() / 2;
    runs.swap_remove(middle)
}

/// Clears the "running" flag however the test ends.
struct RunningGuard<'a>(&'a AtomicBool);

impl Drop for RunningGuard<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

/// "Test performance": measure `wanted` (or every available engine) with the
/// selected chat model, one at a time. The chat server is stopped for the
/// duration, so each engine is measured alone, and started again afterwards
/// on whatever the settings ask for. Nothing else can restart a server
/// meanwhile: the test holds the coordinator's transition lock.
pub async fn run(app: &AppHandle, wanted: Option<Vec<Target>>) -> Result<Report, String> {
    let state = app.state::<AppState>();
    if state.bench.running.swap(true, Ordering::SeqCst) {
        return Err("A performance test is already running.".into());
    }
    let _running = RunningGuard(&state.bench.running);
    state.bench.cancel.store(false, Ordering::SeqCst);

    let (model, model_path) = current_model(&state).ok_or("Choose and download a chat model first.")?;
    let busy = || state.engines.activity.busy(Engine::Chat);
    if busy() {
        return Err("An answer is being written. Test again when it has finished.".into());
    }
    let lock = state.engines.transition_lock();
    let transition = lock.lock().await;
    if busy() {
        return Err("An answer is being written. Test again when it has finished.".into());
    }

    let inventory = state.runtime.lock().unwrap().inventory().clone();
    let (settings, power) = {
        let inf = state.engines.state.lock().unwrap();
        (inf.settings.clone(), inf.power)
    };
    let targets: Vec<Target> = match wanted {
        Some(list) if !list.is_empty() => list,
        _ => engine::target_options(&inventory, &settings, None).into_iter().filter(|o| o.available).map(|o| o.target).collect(),
    };
    if targets.is_empty() {
        return Err("No engine is installed to test.".into());
    }

    state.runtime.lock().unwrap().stop(ServerKind::Chat);
    engine::emit_status(app);

    let total = targets.len();
    for (index, target) in targets.iter().enumerate() {
        if state.bench.cancel.load(Ordering::SeqCst) {
            break;
        }
        let label = inference::target_label(&inventory, target);
        let progress = |stage: &'static str| {
            let _ = app.emit(
                "bench:progress",
                Progress {
                    running: true,
                    index,
                    total,
                    label: label.clone(),
                    stage,
                },
            );
        };
        progress("loading");
        let measurement = match inference::resolve(&inventory, target) {
            Ok(resolved) => {
                let mut m = Measurement::new(target, &label, identity(&inventory, &model, &resolved), power);
                match measure_on(app, &model, &model_path, &resolved, &mut m, &progress).await {
                    Ok(()) => m.ok = true,
                    Err(reason) => {
                        let hint = inference::failure_hint(target, resolved.device.as_ref(), &model.file, &reason);
                        m.error = Some(match hint {
                            Some(hint) => format!("{reason}. {hint}"),
                            None => reason,
                        });
                    }
                }
                m
            }
            Err(reason) => {
                let unknown = Identity {
                    model_id: model.id.clone(),
                    model_file: model.file.clone(),
                    model_bytes: model.bytes,
                    context: model.ctx,
                    revision: inventory.revision.clone(),
                    cpu: inventory.cpu.clone(),
                    device: None,
                    driver: None,
                };
                let mut m = Measurement::new(target, &label, unknown, power);
                m.error = Some(reason);
                m
            }
        };
        if measurement.error.as_deref() == Some("Cancelled.") {
            break; // a cancelled run is not a result
        }
        let mut book = state.bench.book.lock().unwrap();
        book.record(measurement);
        book.save(&state.bench.path);
    }

    // Bring the chat model back, now with the measurements to go on.
    drop(transition);
    state.bench.running.store(false, Ordering::SeqCst);
    engine::reconcile(app, "performance test finished").await;
    let _ = app.emit(
        "bench:progress",
        Progress {
            running: false,
            index: total,
            total,
            label: String::new(),
            stage: "done",
        },
    );
    Ok(report(app))
}

// --------------------------------- trial ------------------------------------

/// What happened when an engine was tried ("Try another engine now",
/// "Try NPU now").
#[derive(Serialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct Trial {
    /// What was asked for.
    pub requested: String,
    /// What the chat model is running on now.
    pub active: Option<String>,
    /// The requested engine is running the model and answered the test prompt.
    pub ok: bool,
    /// The switch is waiting for an answer in progress.
    pub pending: bool,
    /// The engine's own log confirmed the device.
    pub verified: bool,
    pub detail: Option<String>,
    /// Something else is running instead (fallback is allowed).
    pub fallback_used: bool,
    /// What went wrong, if anything did.
    pub problem: Option<String>,
    /// What to do about it.
    pub hint: Option<String>,
    /// Worth knowing about this engine and model.
    pub note: Option<String>,
    pub answer: Option<String>,
    pub first_token_ms: Option<u64>,
    pub total_ms: Option<u64>,
    pub tokens_per_second: Option<f32>,
    pub answer_tokens: Option<u32>,
}

/// Run the chat model on `target` (a temporary override; the saved choices
/// are untouched), confirm from the engine's log that it is really there,
/// and ask it a short question. The override stays until the user ends it.
pub async fn trial(app: &AppHandle, target: Target) -> Trial {
    let state = app.state::<AppState>();
    let inventory = state.runtime.lock().unwrap().inventory().clone();
    let model_file = current_model(&state).map(|(m, _)| m.file).unwrap_or_default();
    let resolved = inference::resolve(&inventory, &target);
    let mut trial = Trial {
        requested: inference::target_label(&inventory, &target),
        note: resolved.as_ref().ok().and_then(|r| inference::compatibility_note(r, &model_file)),
        ..Trial::default()
    };
    if state.bench.running() {
        trial.problem = Some("A performance test is running. Try again when it has finished.".into());
        return trial;
    }
    let device = resolved.as_ref().ok().and_then(|r| r.device.clone());

    engine::set_override(app, Some(target.clone())).await;
    let chat = engine::status(app).chat;
    trial.active = chat.active.clone();
    trial.verified = chat.verified;
    trial.detail = chat.detail.clone();
    trial.fallback_used = chat.fallback_used;
    if chat.pending {
        trial.pending = true;
        trial.problem = Some("The switch is waiting for the answer in progress; it will happen as soon as that has finished.".into());
        return trial;
    }
    let on_target = chat.state == "ready" && chat.active_target.as_ref() == Some(&target) && chat.verified;
    if !on_target {
        let reason = chat
            .fallback_reason
            .or(chat.error)
            .unwrap_or_else(|| "the engine did not start".to_string());
        trial.hint = inference::failure_hint(&target, device.as_ref(), &model_file, &reason);
        trial.problem = Some(reason);
        return trial;
    }

    // It loaded where it should. Does it answer?
    // (Marked as work in progress, so nothing switches the engine under it.)
    let busy = engine::begin(app, &[Engine::Chat]);
    let port = state.runtime.lock().unwrap().port_of(ServerKind::Chat);
    let Some(port) = port else {
        trial.problem = Some("the engine stopped before it could be tested".into());
        return trial;
    };
    let run = run_prompt(port, &trial_messages(), 64, &|| false).await;
    drop(busy);
    match run {
        Ok(run) => {
            trial.ok = true;
            trial.answer = Some(run.text);
            trial.first_token_ms = Some(run.first_token_ms);
            trial.total_ms = Some(run.total_ms);
            trial.tokens_per_second = run.tokens_per_second;
            trial.answer_tokens = Some(run.answer_tokens);
        }
        Err(reason) => {
            // It loaded but cannot answer: treat it as any other failed
            // target (fall back if that is allowed, otherwise stop).
            trial.hint = inference::failure_hint(&target, device.as_ref(), &model_file, &reason);
            trial.problem = Some(format!("the model loaded but could not answer ({reason})"));
            engine::mark_failed(app, Engine::Chat, &target).await;
            let chat = engine::status(app).chat;
            trial.active = chat.active;
            trial.fallback_used = chat.fallback_used;
        }
    }
    trial
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::inference::{parse_devices, Backend, BackendInfo, BackendStatus, DeviceKind, Hardware, Vendor};

    const NVIDIA: &str = "nvidia-geforce-rtx-4050-laptop-gpu";
    const IRIS: &str = "intel-r-iris-r-xe-graphics";

    fn inventory() -> Inventory {
        let vulkan = "  Vulkan0: Intel(R) Iris(R) Xe Graphics (16193 MiB, 15425 MiB free)\n  Vulkan1: NVIDIA GeForce RTX 4050 Laptop GPU (5920 MiB, 5152 MiB free)\n";
        let info = |backend: Backend, list: &str| BackendInfo {
            backend,
            status: BackendStatus::Available,
            experimental: backend.experimental(),
            devices: parse_devices(backend, list),
            detail: None,
        };
        let hardware = |name: &str, vendor, driver: &str| Hardware {
            name: name.to_string(),
            kind: DeviceKind::Gpu,
            vendor,
            driver: Some(driver.to_string()),
        };
        Inventory {
            cpu: "12th Gen Intel(R) Core(TM) i7-12700H".into(),
            hardware: vec![
                hardware("NVIDIA GeForce RTX 4050 Laptop GPU", Vendor::Nvidia, "32.0.16.2101"),
                hardware("Intel(R) Iris(R) Xe Graphics", Vendor::Intel, "32.0.101.6078"),
            ],
            backends: vec![info(Backend::Vulkan, vulkan), info(Backend::Cpu, "")],
            revision: "b12110".into(),
        }
    }

    fn model() -> Model {
        Model {
            id: "qwen2.5-3b-instruct-q4".into(),
            file: "qwen2.5-3b-instruct-q4_k_m.gguf".into(),
            bytes: 2_104_932_768,
            ctx: 32768,
        }
    }

    fn vulkan(key: &str) -> Target {
        Target::new(Backend::Vulkan, Some(key))
    }

    /// A successful measurement of `target` at `speed` tokens/s; on battery
    /// when `watts` is given.
    fn measured_at(inv: &Inventory, target: Target, speed: f32, watts: Option<f32>) -> Measurement {
        let resolved = inference::resolve(inv, &target).unwrap();
        let mut m = Measurement::new(&target, &resolved.label(), identity(inv, &model(), &resolved), if watts.is_some() { PowerSource::Battery } else { PowerSource::Ac });
        m.ok = true;
        m.verified = true;
        m.tokens_per_second = Some(speed);
        m.drain_watts = watts;
        m.joules_per_token = watts.map(|w| w / speed);
        m
    }

    #[test]
    fn a_measurement_records_what_it_was_taken_with() {
        let inv = inventory();
        let on_nvidia = identity(&inv, &model(), &inference::resolve(&inv, &vulkan(NVIDIA)).unwrap());
        assert_eq!(on_nvidia.device.as_deref(), Some("NVIDIA GeForce RTX 4050 Laptop GPU"));
        assert_eq!(on_nvidia.driver.as_deref(), Some("32.0.16.2101"));
        assert_eq!(on_nvidia.revision, "b12110");
        assert_eq!(on_nvidia.context, 32768);
        assert_eq!(on_nvidia.model_bytes, 2_104_932_768);
        let on_cpu = identity(&inv, &model(), &inference::resolve(&inv, &Target::cpu()).unwrap());
        assert_eq!((on_cpu.device, on_cpu.driver), (None, None));
        assert_eq!(on_cpu.cpu, "12th Gen Intel(R) Core(TM) i7-12700H");
    }

    #[test]
    fn out_of_date_measurements_are_not_evidence() {
        let inv = inventory();
        let m = measured_at(&inv, vulkan(NVIDIA), 55.0, None);
        assert_eq!(stale_reason(&m, &inv, &model()), None);

        // Another model, or the same model re-downloaded as another file.
        let other = Model { id: "phi-3.5-mini-q4".into(), ..model() };
        assert_eq!(stale_reason(&m, &inv, &other).as_deref(), Some("measured with another model"));
        let resized = Model { bytes: 1, ..model() };
        assert_eq!(stale_reason(&m, &inv, &resized).as_deref(), Some("measured with another model"));

        // A new engine revision.
        let newer = Inventory { revision: "b13000".into(), ..inv.clone() };
        assert_eq!(stale_reason(&m, &newer, &model()).as_deref(), Some("measured with an older version of the engine"));

        // A driver update.
        let mut updated = inv.clone();
        updated.hardware[0].driver = Some("33.0.0.1".into());
        assert_eq!(stale_reason(&m, &updated, &model()).as_deref(), Some("the device driver has changed since"));
        // ...which does not touch a measurement on another device.
        assert_eq!(stale_reason(&measured_at(&inv, vulkan(IRIS), 9.0, None), &updated, &model()), None);

        // The card is gone.
        let mut removed = inv.clone();
        removed.backends[0].devices.retain(|d| d.key != NVIDIA);
        assert_eq!(stale_reason(&m, &removed, &model()).as_deref(), Some("that engine or device is not available now"));

        // An engine with no current measurement is named, so it can be tested.
        let usable = [(vulkan(NVIDIA), "NVIDIA (Vulkan)".to_string()), (vulkan(IRIS), "Intel (Vulkan)".to_string()), (Target::cpu(), "CPU only".to_string())];
        let mut some = Book::default();
        some.record(measured_at(&inv, vulkan(IRIS), 9.0, None));
        some.record(m.clone());
        assert_eq!(unmeasured(&rows_for(&some, &inv, &model()), &usable), ["CPU only"]);
        // A stale measurement does not count as one.
        assert_eq!(unmeasured(&rows_for(&some, &updated, &model()), &usable), ["NVIDIA (Vulkan)", "CPU only"]);
        // Before any test there is nothing to compare with, so nothing is listed.
        assert!(unmeasured(&rows_for(&Book::default(), &inv, &model()), &usable).is_empty());

        // Stale rows are listed last and say why; current ones lead, fastest first.
        let mut book = Book::default();
        book.record(measured_at(&inv, vulkan(IRIS), 9.0, None));
        book.record(measured_at(&inv, Target::cpu(), 8.0, None));
        book.record(m.clone());
        let rows = rows_for(&book, &updated, &model());
        let order: Vec<(&str, bool)> = rows.iter().map(|r| (r.measurement.label.as_str(), r.stale.is_some())).collect();
        assert_eq!(
            order,
            [
                ("Intel Iris Xe Graphics (Vulkan)", false),
                ("CPU only", false),
                ("NVIDIA GeForce RTX 4050 Laptop GPU (Vulkan)", true),
            ]
        );
    }

    #[test]
    fn automatic_follows_measurements_only_where_there_are_any() {
        let inv = inventory();
        let (nvidia, iris, cpu) = (vulkan(NVIDIA), vulkan(IRIS), Target::cpu());
        let mains = [measured_at(&inv, iris.clone(), 9.5, None), measured_at(&inv, nvidia.clone(), 55.0, None), measured_at(&inv, cpu.clone(), 8.0, None)];
        let refs: Vec<&Measurement> = mains.iter().collect();

        // Performance: by speed. (Intel at 9.5 and the CPU at 8.0 are more than 15% apart.)
        assert_eq!(ranking(&refs, AutoGoal::Performance, &[]), [nvidia.clone(), iris.clone(), cpu.clone()]);
        // Battery saving: speed is not evidence about energy, so no measured order...
        assert!(ranking(&refs, AutoGoal::BatterySaving, &[]).is_empty());
        // ...and Automatic keeps its rule of thumb there.
        assert_eq!(inference::auto_chain(&inv, AutoGoal::BatterySaving, &ranking(&refs, AutoGoal::BatterySaving, &[])), [iris.clone(), cpu.clone(), nvidia.clone()]);
        // With measurements, Automatic for performance follows them.
        assert_eq!(inference::auto_chain(&inv, AutoGoal::Performance, &ranking(&refs, AutoGoal::Performance, &[]))[0], nvidia);

        let said = advice(&refs, &inv);
        let pick = said.plugged_in.expect("a recommendation for mains power");
        assert_eq!(pick.target, nvidia);
        assert_eq!(pick.reason, "Fastest in the test: 55 tokens/s (next: Intel Iris Xe Graphics (Vulkan), 9.5 tokens/s).");
        assert_eq!(said.on_battery, None);
        assert!(said.battery_note.starts_with("Power efficiency is unmeasured: energy can only be measured while running on battery."));

        // Tested on battery with energy figures: least energy per token wins,
        // even though it is not the fastest. NVIDIA: 60 W / 50 = 1.2 J; Intel: 18 W / 9 = 2.0 J; CPU: 30 W / 8 = 3.75 J.
        let battery = [
            measured_at(&inv, cpu.clone(), 8.0, Some(30.0)),
            measured_at(&inv, iris.clone(), 9.0, Some(18.0)),
            measured_at(&inv, nvidia.clone(), 50.0, Some(60.0)),
        ];
        let all: Vec<&Measurement> = mains.iter().chain(battery.iter()).collect();
        assert_eq!(ranking(&all, AutoGoal::BatterySaving, &[]), [nvidia.clone(), iris.clone(), cpu.clone()]);
        // Battery runs do not count towards the mains ranking, nor the reverse.
        assert_eq!(ranking(&all, AutoGoal::Performance, &[]), [nvidia.clone(), iris.clone(), cpu.clone()]);
        let said = advice(&all, &inv);
        let pick = said.on_battery.expect("a recommendation for battery");
        assert_eq!(pick.target, nvidia);
        assert_eq!(pick.reason, "Least battery per token in the test: 1.2 J per token at 50 tokens/s (the whole computer drew 60 W).");
        assert!(said.battery_note.contains("whole computer's drain") && said.battery_note.contains("do not predict battery life"));

        // On battery, but the battery reported no drain: still unmeasured.
        let silent = [measured_at(&inv, cpu.clone(), 8.0, Some(30.0))].map(|mut m| {
            m.drain_watts = None;
            m.joules_per_token = None;
            m
        });
        let refs: Vec<&Measurement> = silent.iter().collect();
        assert!(ranking(&refs, AutoGoal::BatterySaving, &[]).is_empty());
        assert!(advice(&refs, &inv).battery_note.contains("did not report its drain"));
    }

    #[test]
    fn engines_that_measure_about_the_same_are_not_ranked_by_noise() {
        // Two engines on the same card, as measured on a real laptop in two
        // runs of the test: 72.6 / 65.3 tokens/s, then 64.4 / 73.4.
        let mut inv = inventory();
        let cuda_list = "  CUDA0: NVIDIA GeForce RTX 4050 Laptop GPU (6140 MiB, 6140 MiB free)\n";
        inv.backends.insert(
            0,
            BackendInfo {
                backend: Backend::Cuda,
                status: BackendStatus::Available,
                experimental: false,
                devices: parse_devices(Backend::Cuda, cuda_list),
                detail: None,
            },
        );
        let cuda = Target::new(Backend::Cuda, Some(NVIDIA));
        let (vulkan_nv, iris) = (vulkan(NVIDIA), vulkan(IRIS));
        let rule = inference::auto_chain(&inv, AutoGoal::Performance, &[]);
        assert_eq!(rule[..2], [cuda.clone(), vulkan_nv.clone()]);

        for (cuda_speed, vulkan_speed) in [(72.6, 65.3), (64.4, 73.4)] {
            let rows = [measured_at(&inv, vulkan_nv.clone(), vulkan_speed, None), measured_at(&inv, cuda.clone(), cuda_speed, None), measured_at(&inv, iris.clone(), 11.1, None)];
            let refs: Vec<&Measurement> = rows.iter().collect();
            // Whichever happened to come out ahead, the order is the same: the
            // rule of thumb decides between near-equals, the numbers decide the rest.
            assert_eq!(ranking(&refs, AutoGoal::Performance, &rule), [cuda.clone(), vulkan_nv.clone(), iris.clone()]);
            let pick = advice(&refs, &inv).plugged_in.unwrap();
            assert_eq!(pick.target, cuda);
            assert!(pick.reason.starts_with("As fast as anything in the test:") && pick.reason.contains("measured about the same"), "{}", pick.reason);
        }
        // A clear difference still wins over the rule of thumb.
        let rows = [measured_at(&inv, vulkan_nv.clone(), 73.0, None), measured_at(&inv, cuda.clone(), 40.0, None)];
        let refs: Vec<&Measurement> = rows.iter().collect();
        assert_eq!(ranking(&refs, AutoGoal::Performance, &rule), [vulkan_nv.clone(), cuda.clone()]);
        assert!(advice(&refs, &inv).plugged_in.unwrap().reason.starts_with("Fastest in the test: 73 tokens/s (next: NVIDIA GeForce RTX 4050 Laptop GPU (CUDA)"));

        // Of three runs the middle one is kept.
        let run = |speed: f32| Run { text: String::new(), first_token_ms: 100, total_ms: 1500, tokens_per_second: Some(speed), answer_tokens: 96 };
        assert_eq!(middle_by_speed(vec![run(73.4), run(64.4), run(70.1)]).tokens_per_second, Some(70.1));
        assert_eq!(middle_by_speed(vec![run(5.0)]).tokens_per_second, Some(5.0));
    }

    #[test]
    fn failures_and_unverified_runs_never_rank() {
        let inv = inventory();
        let mut failed = measured_at(&inv, vulkan(NVIDIA), 80.0, None);
        failed.ok = false;
        failed.error = Some("the model could not be loaded".into());
        let mut unverified = measured_at(&inv, vulkan(IRIS), 70.0, None);
        unverified.verified = false;
        let fine = measured_at(&inv, Target::cpu(), 8.0, None);
        let all = [&failed, &unverified, &fine];
        assert_eq!(ranking(&all, AutoGoal::Performance, &[]), [Target::cpu()]);
        assert_eq!(advice(&all, &inv).plugged_in.unwrap().reason, "The only engine measured: 8.0 tokens/s.");
        assert_eq!(advice(&[], &inv).plugged_in, None);
    }

    #[test]
    fn the_book_keeps_one_result_per_engine_model_and_power() {
        let inv = inventory();
        let path = std::env::temp_dir().join(format!("loft-bench-test-{}.json", std::process::id()));
        let mut book = Book::default();
        book.record(measured_at(&inv, vulkan(NVIDIA), 50.0, None));
        book.record(measured_at(&inv, vulkan(NVIDIA), 55.0, None)); // a re-test replaces
        book.record(measured_at(&inv, vulkan(NVIDIA), 48.0, Some(60.0))); // battery is kept apart
        book.record(measured_at(&inv, Target::cpu(), 8.0, None));
        assert_eq!(book.results.len(), 3);
        assert_eq!(book.results[0].tokens_per_second, Some(55.0));

        book.save(&path);
        assert_eq!(Book::load(&path), book);
        // camelCase on disk, like the rest of Loft's settings.
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("\"tokensPerSecond\"") && text.contains("\"modelBytes\"") && text.contains("\"powerSource\": \"battery\""));
        let _ = std::fs::remove_file(&path);
        // A missing or damaged file is an empty book, not an error.
        assert_eq!(Book::load(&path), Book::default());
        std::fs::write(&path, "{ not json").unwrap();
        assert_eq!(Book::load(&path), Book::default());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn every_engine_gets_the_same_request() {
        let messages = bench_messages();
        assert_eq!(messages, bench_messages());
        let user = messages[1]["content"].as_str().unwrap();
        // Long enough to resemble a real question with excerpts.
        assert!(user.split_whitespace().count() > 250, "{} words", user.split_whitespace().count());
        assert_ne!(warmup_messages(), messages);
    }
}
