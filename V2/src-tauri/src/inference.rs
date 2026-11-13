//! Which engine runs a model, and where.
//!
//! A **backend** is the software path (a llama.cpp build: CUDA, Vulkan,
//! OpenVINO, CPU). A **device** is the processor that does the work (an NVIDIA
//! GPU, the Intel integrated GPU, an Intel NPU, the CPU). The two are kept
//! apart: Vulkan can drive several GPUs, and one GPU can be driven by CUDA or
//! by Vulkan.
//!
//! This module is the decision logic only. It has no processes, no clocks of
//! its own and no Tauri types, so every rule here is unit-tested:
//!
//! * what the user asked for (`PowerSettings`, a temporary override) and the
//!   power source give a **plan**: an ordered list of targets to try;
//! * the plan, what is running and what has already failed give a
//!   **decision**: leave it, wait, restart, or stop with an error;
//! * the server's own load log tells which device **actually** holds the
//!   model, which is what gets reported.
//!
//! `runtime.rs` starts processes, `engine.rs` carries the decisions out.

use std::collections::HashSet;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

// ------------------------------- vocabulary ---------------------------------

/// A llama.cpp build: the software path a model runs through.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[serde(rename_all = "lowercase")]
pub enum Backend {
    Cuda,
    Vulkan,
    OpenVino,
    Metal,
    Cpu,
}

impl Backend {
    /// Suffix of the build's folder: `binaries/<platform>-<suffix>/`.
    pub fn dir_suffix(self) -> &'static str {
        match self {
            Backend::Cuda => "cuda",
            Backend::Vulkan => "vulkan",
            Backend::OpenVino => "openvino",
            Backend::Metal => "metal",
            Backend::Cpu => "cpu",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Backend::Cuda => "CUDA",
            Backend::Vulkan => "Vulkan",
            Backend::OpenVino => "OpenVINO",
            Backend::Metal => "Metal",
            Backend::Cpu => "CPU",
        }
    }

    /// Experimental backends are offered and selectable, with a warning. They
    /// are never chosen automatically without a successful measurement.
    pub fn experimental(self) -> bool {
        self == Backend::OpenVino
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum DeviceKind {
    Gpu,
    Npu,
    Cpu,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Vendor {
    Nvidia,
    Intel,
    Amd,
    Apple,
    Other,
}

/// Which of Loft's two model servers.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[serde(rename_all = "lowercase")]
pub enum Engine {
    Chat,
    Embed,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum PowerSource {
    Ac,
    Battery,
    #[default]
    Unknown,
}

/// A device as one backend sees it.
#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Device {
    /// Stable across restarts and driver updates: made from the device's
    /// name, not from llama.cpp's index (`Vulkan0`/`Vulkan1` can swap).
    pub key: String,
    pub name: String,
    pub kind: DeviceKind,
    pub vendor: Vendor,
    /// A separate graphics card, as opposed to graphics built into the CPU.
    pub discrete: bool,
    pub memory_mib: Option<u64>,
    /// What the backend calls it right now: `Vulkan1`, `CUDA0`, `GPU.0`, `NPU`.
    pub runtime_id: String,
}

#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum BackendStatus {
    /// Installed and usable on this machine.
    Available,
    /// The engine build is not installed (it is an optional download).
    NotInstalled,
    /// Installed, but this machine has no hardware it can use.
    HardwareUnavailable,
    /// The hardware is there but its driver is not.
    DriverMissing,
    /// Installed, but it could not be asked which devices it sees.
    Failed,
}

/// One backend and what it can use on this machine. Being experimental is
/// separate from being available: an experimental backend can be both.
#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct BackendInfo {
    pub backend: Backend,
    pub status: BackendStatus,
    pub experimental: bool,
    pub devices: Vec<Device>,
    /// Why it is not available, or how to set it up.
    pub detail: Option<String>,
}

/// A processor the operating system reports, whether or not any installed
/// backend can use it.
#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Hardware {
    pub name: String,
    pub kind: DeviceKind,
    pub vendor: Vendor,
    pub driver: Option<String>,
}

#[derive(Serialize, Clone, Debug, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct Inventory {
    pub cpu: String,
    pub hardware: Vec<Hardware>,
    pub backends: Vec<BackendInfo>,
    /// The llama.cpp revision the engine builds come from.
    pub revision: String,
}

impl Inventory {
    pub fn backend(&self, backend: Backend) -> Option<&BackendInfo> {
        self.backends.iter().find(|b| b.backend == backend)
    }
}

/// What to run a model on: a backend, and for GPU/NPU backends a device.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, Hash)]
#[serde(rename_all = "camelCase")]
pub struct Target {
    pub backend: Backend,
    /// A `Device::key`. `None` = the backend's best device (or the CPU).
    #[serde(default)]
    pub device: Option<String>,
}

impl Target {
    pub fn new(backend: Backend, device: Option<&str>) -> Self {
        Target {
            backend,
            device: device.map(str::to_string),
        }
    }
    pub fn cpu() -> Self {
        Target::new(Backend::Cpu, None)
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum ProfileMode {
    /// Loft chooses (and may change its mind as hardware changes).
    #[default]
    Automatic,
    /// The user chose `target`.
    Manual,
}

/// The choice for one power state.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct Profile {
    pub mode: ProfileMode,
    /// The chosen target when `mode` is manual. Kept even while unavailable.
    pub target: Option<Target>,
    /// Advanced: a different target for the embedding model. `None` = the
    /// embeddings follow the chat target where they can.
    pub embed_target: Option<Target>,
}

/// The saved "Performance & Battery" settings.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct PowerSettings {
    pub plugged_in: Profile,
    pub on_battery: Profile,
    /// Use the battery choice while on battery. Off: the plugged-in choice
    /// is used all the time.
    pub auto_switch: bool,
    /// If the chosen engine cannot run the model, try another one. Off: stop
    /// and say so (for deliberate experiments, such as the NPU).
    pub allow_fallback: bool,
}

impl Default for PowerSettings {
    fn default() -> Self {
        PowerSettings {
            plugged_in: Profile::default(),
            on_battery: Profile::default(),
            auto_switch: true,
            allow_fallback: true,
        }
    }
}

/// What "automatic" optimises for.
#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum AutoGoal {
    Performance,
    BatterySaving,
}

/// Where the current choice comes from.
#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ChoiceSource {
    /// "Try another engine now": temporary, until the user ends it.
    Override,
    PluggedIn,
    OnBattery,
}

// ------------------------------- devices ------------------------------------

/// A stable key for a device name: lower-case letters and digits only, with
/// an ordinal when two devices share a name.
pub fn device_key(name: &str, ordinal: usize) -> String {
    let mut key = String::new();
    let mut gap = false;
    for c in name.chars().flat_map(char::to_lowercase) {
        if c.is_ascii_alphanumeric() {
            if gap && !key.is_empty() {
                key.push('-');
            }
            key.push(c);
            gap = false;
        } else {
            gap = true;
        }
    }
    if ordinal > 0 {
        key.push_str(&format!("#{}", ordinal + 1));
    }
    key
}

pub fn vendor_of(name: &str) -> Vendor {
    let n = name.to_ascii_lowercase();
    if ["nvidia", "geforce", "quadro", " rtx", " gtx"].iter().any(|k| n.contains(k)) || n.starts_with("rtx") {
        Vendor::Nvidia
    } else if n.contains("intel") {
        Vendor::Intel
    } else if n.contains("amd") || n.contains("radeon") {
        Vendor::Amd
    } else if n.contains("apple") {
        Vendor::Apple
    } else {
        Vendor::Other
    }
}

/// A separate graphics card rather than graphics built into the processor.
pub fn is_discrete(name: &str) -> bool {
    let n = name.to_ascii_lowercase().replace("(tm)", "").replace("(r)", "");
    match vendor_of(name) {
        Vendor::Nvidia => true,
        Vendor::Amd => n.contains("radeon rx") || n.contains("radeon pro") || n.contains("firepro"),
        // "Intel(R) Arc(TM) A770 Graphics" is a card; "Intel(R) Arc(TM) Graphics" is built in.
        Vendor::Intel => {
            let after = n.split("arc").nth(1).unwrap_or("");
            let model = after.trim_start_matches(|c: char| !c.is_ascii_alphanumeric());
            n.contains("arc") && (model.starts_with('a') || model.starts_with('b')) && model.chars().nth(1).is_some_and(|c| c.is_ascii_digit())
        }
        _ => false,
    }
}

/// A device's name as it is shown: without the trademark marks, and
/// without the "(iGPU)" / "(dGPU)" OpenVINO appends. (The name itself is
/// kept as the engine gives it: keys and stored measurements are made of it.)
pub fn display_name(name: &str) -> String {
    let mut shown = name.to_string();
    for mark in ["(R)", "(r)", "(TM)", "(tm)", "(iGPU)", "(dGPU)"] {
        shown = shown.replace(mark, "");
    }
    shown.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Parse `llama-server --list-devices` for one backend. Lines look like
/// `  Vulkan1: NVIDIA GeForce RTX 4050 Laptop GPU (5920 MiB, 5152 MiB free)`,
/// and for OpenVINO
/// `  OPENVINO2: GGML_OPENVINO_DEVICE=NPU - Intel(R) AI Boost (0 MiB, 0 MiB free)`.
pub fn parse_devices(backend: Backend, output: &str) -> Vec<Device> {
    let mut devices: Vec<Device> = Vec::new();
    for line in output.lines() {
        let Some((id, rest)) = line.trim().split_once(": ") else {
            continue;
        };
        let valid_id = !id.is_empty() && id.chars().all(|c| c.is_ascii_alphanumeric()) && id.ends_with(|c: char| c.is_ascii_digit());
        if !valid_id || !rest.ends_with("free)") {
            continue;
        }
        let Some((description, memory)) = rest.rsplit_once(" (") else {
            continue;
        };
        let memory_mib = memory.split_whitespace().next().and_then(|m| m.parse::<u64>().ok()).filter(|m| *m > 0);

        let (runtime_id, name, kind) = if backend == Backend::OpenVino {
            // The id to select is OpenVINO's own (CPU, GPU.0, NPU), not OPENVINOn.
            let Some(value) = description.strip_prefix("GGML_OPENVINO_DEVICE=") else {
                continue;
            };
            let (ov_id, name) = match value.split_once(" - ") {
                Some((id, name)) => (id, name),
                None => (value, value),
            };
            let ov_id = ov_id.trim().trim_end_matches("(selected)").trim();
            let kind = if ov_id.starts_with("NPU") {
                DeviceKind::Npu
            } else if ov_id.starts_with("GPU") {
                DeviceKind::Gpu
            } else {
                DeviceKind::Cpu
            };
            (ov_id.to_string(), name.trim().to_string(), kind)
        } else {
            (id.to_string(), description.trim().to_string(), DeviceKind::Gpu)
        };

        let ordinal = devices.iter().filter(|d| d.name == name).count();
        devices.push(Device {
            key: device_key(&name, ordinal),
            vendor: if kind == DeviceKind::Npu && vendor_of(&name) == Vendor::Other { Vendor::Intel } else { vendor_of(&name) },
            discrete: kind == DeviceKind::Gpu && is_discrete(&name),
            kind,
            memory_mib,
            runtime_id,
            name,
        });
    }
    devices
}

/// The marker in the message OpenVINO prints when asked for a device it
/// does not have; the devices it does have follow, one per line.
const OPENVINO_REFUSAL: &str = "available OpenVINO devices:";

/// The devices named in OpenVINO's refusal of an unknown device:
///
/// ```text
/// … GGML_OPENVINO_DEVICE=X is not available. Set it to one of the available OpenVINO devices:
///   CPU: 12th Gen Intel(R) Core(TM) i7-12700H
///   GPU.0: Intel(R) Iris(R) Xe Graphics (iGPU)
///   GPU.1: NVIDIA GeForce RTX 4050 Laptop GPU (dGPU)
/// ```
///
/// as (id, name) pairs. This is how Loft learns them on builds whose
/// `--list-devices` prints nothing (b12110 for Windows is one).
pub fn parse_openvino_refusal(output: &str) -> Vec<(String, String)> {
    let Some((_, list)) = output.split_once(OPENVINO_REFUSAL) else {
        return Vec::new();
    };
    list.lines()
        .filter_map(|line| line.trim().split_once(": "))
        .filter(|(id, name)| {
            let family = id.split('.').next().unwrap_or("");
            let number = id.split('.').nth(1);
            !name.trim().is_empty()
                && matches!(family, "CPU" | "GPU" | "NPU")
                && number.is_none_or(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
        })
        .map(|(id, name)| (id.to_string(), name.trim().to_string()))
        .collect()
}

/// OpenVINO devices from (id, name) pairs such as ("GPU.0", "Intel(R) Iris(R) Xe Graphics (iGPU)").
pub fn openvino_devices(pairs: &[(String, String)]) -> Vec<Device> {
    let mut devices: Vec<Device> = Vec::new();
    for (id, name) in pairs {
        let kind = if id.starts_with("NPU") {
            DeviceKind::Npu
        } else if id.starts_with("GPU") {
            DeviceKind::Gpu
        } else {
            DeviceKind::Cpu
        };
        let ordinal = devices.iter().filter(|d| &d.name == name).count();
        devices.push(Device {
            key: device_key(name, ordinal),
            vendor: if kind == DeviceKind::Npu && vendor_of(name) == Vendor::Other { Vendor::Intel } else { vendor_of(name) },
            discrete: kind == DeviceKind::Gpu && (name.ends_with("(dGPU)") || is_discrete(name)),
            kind,
            memory_mib: None,
            runtime_id: id.clone(),
            name: name.clone(),
        });
    }
    devices
}

/// Why a server did not come up, in words, from what it logged.
pub fn load_failure(report: &LoadReport, exited: bool) -> String {
    match (report.last_error(), exited) {
        (Some(error), _) => format!("the model could not be loaded ({error})"),
        (None, true) => "the engine stopped while loading the model, without giving a reason".to_string(),
        (None, false) => "the model did not finish loading in time".to_string(),
    }
}

// ------------------------------- resolving ----------------------------------

/// A target matched to what is installed right now.
#[derive(Clone, Debug, PartialEq)]
pub struct Resolved {
    pub target: Target,
    pub backend: Backend,
    /// `None` for the CPU backend.
    pub device: Option<Device>,
}

impl Resolved {
    /// "NVIDIA GeForce RTX 4050 Laptop GPU (Vulkan)", "CPU only".
    pub fn label(&self) -> String {
        match &self.device {
            Some(device) if device.kind != DeviceKind::Cpu || self.backend != Backend::Cpu => {
                // The device first: it is what people recognise. The
                // engine follows, to tell apart two ways of driving it.
                format!("{} ({})", display_name(&device.name), self.backend.label())
            }
            _ => "CPU only".to_string(),
        }
    }
    pub fn device_name(&self) -> String {
        self.device.as_ref().map(|d| display_name(&d.name)).unwrap_or_else(|| "CPU".to_string())
    }
}

/// The GPU a backend would use when none is named: the discrete one with the
/// most memory, else whatever it has.
fn best_gpu(info: &BackendInfo) -> Option<&Device> {
    let gpus = || info.devices.iter().filter(|d| d.kind == DeviceKind::Gpu);
    gpus()
        .filter(|d| d.discrete)
        .max_by_key(|d| d.memory_mib.unwrap_or(0))
        .or_else(|| gpus().max_by_key(|d| d.memory_mib.unwrap_or(0)))
}

/// Match `target` to an installed backend and a present device, or say in
/// plain words why that is not possible.
pub fn resolve(inventory: &Inventory, target: &Target) -> Result<Resolved, String> {
    let label = target.backend.label();
    let info = inventory
        .backend(target.backend)
        .ok_or_else(|| format!("{label} is not supported on this system"))?;
    match info.status {
        BackendStatus::Available => {}
        BackendStatus::NotInstalled => return Err(format!("the {label} engine is not installed")),
        BackendStatus::HardwareUnavailable => {
            return Err(info.detail.clone().unwrap_or_else(|| format!("this computer has no hardware {label} can use")))
        }
        BackendStatus::DriverMissing => {
            return Err(info.detail.clone().unwrap_or_else(|| format!("the driver {label} needs is not installed")))
        }
        BackendStatus::Failed => {
            return Err(info.detail.clone().unwrap_or_else(|| format!("the {label} engine could not be started")))
        }
    }
    let device = match (&target.device, target.backend) {
        (Some(key), _) => Some(
            info.devices
                .iter()
                .find(|d| &d.key == key)
                .cloned()
                .ok_or_else(|| format!("the chosen device is not available to {label} right now"))?,
        ),
        (None, Backend::Cpu) => None,
        (None, Backend::OpenVino) => info.devices.iter().find(|d| d.kind == DeviceKind::Cpu).cloned(),
        (None, _) => Some(best_gpu(info).cloned().ok_or_else(|| format!("{label} found no graphics card to use"))?),
    };
    Ok(Resolved {
        target: target.clone(),
        backend: target.backend,
        device,
    })
}

// ------------------------------ automatic -----------------------------------

/// The targets "automatic" would try, best first.
///
/// * **Performance**: CUDA, then Vulkan on the best GPU, then the CPU.
/// * **Battery saving**: stay off the discrete graphics card, which is the
///   part that costs the most to wake: built-in graphics, then the CPU, and
///   the discrete card only as a last resort. This is a rule of thumb, not a
///   measurement; `measured` (benchmark results, best first) replaces it when
///   there are any.
///
/// Experimental backends are never in the rule-of-thumb lists.
pub fn auto_chain(inventory: &Inventory, goal: AutoGoal, measured: &[Target]) -> Vec<Target> {
    let mut chain: Vec<Target> = Vec::new();
    let push = |chain: &mut Vec<Target>, target: Target| {
        if !chain.contains(&target) && resolve(inventory, &target).is_ok() {
            chain.push(target);
        }
    };
    for target in measured {
        push(&mut chain, target.clone());
    }

    let gpus = |backend: Backend, discrete: bool| -> Vec<Target> {
        let Some(info) = inventory.backend(backend).filter(|b| b.status == BackendStatus::Available) else {
            return Vec::new();
        };
        let mut devices: Vec<&Device> = info.devices.iter().filter(|d| d.kind == DeviceKind::Gpu && d.discrete == discrete).collect();
        devices.sort_by_key(|d| std::cmp::Reverse(d.memory_mib.unwrap_or(0)));
        devices.into_iter().map(|d| Target::new(backend, Some(&d.key))).collect()
    };
    let metal = Target::new(Backend::Metal, None);

    match goal {
        AutoGoal::Performance => {
            for backend in [Backend::Cuda, Backend::Vulkan] {
                for target in gpus(backend, true) {
                    push(&mut chain, target);
                }
            }
            push(&mut chain, metal);
            for target in gpus(Backend::Vulkan, false) {
                push(&mut chain, target);
            }
            push(&mut chain, Target::cpu());
        }
        AutoGoal::BatterySaving => {
            push(&mut chain, metal);
            for target in gpus(Backend::Vulkan, false) {
                push(&mut chain, target);
            }
            push(&mut chain, Target::cpu());
            for backend in [Backend::Vulkan, Backend::Cuda] {
                for target in gpus(backend, true) {
                    push(&mut chain, target);
                }
            }
        }
    }
    chain
}

// -------------------------------- planning ----------------------------------

/// What is wanted for one engine, and what to try in what order.
#[derive(Clone, Debug, PartialEq)]
pub struct Plan {
    pub source: ChoiceSource,
    /// `None` = automatic.
    pub requested: Option<Target>,
    pub goal: AutoGoal,
    /// Targets to try, in order. Empty only if nothing at all is installed.
    pub chain: Vec<Target>,
    /// No fallback: if the first target fails, stop and say so.
    pub strict: bool,
    /// Things the user should know about how the request was interpreted.
    pub notes: Vec<String>,
}

impl Plan {
    /// "Automatic (performance)", "NVIDIA GeForce RTX 4050 Laptop GPU (Vulkan)".
    pub fn requested_label(&self, inventory: &Inventory) -> String {
        match &self.requested {
            None => match self.goal {
                AutoGoal::Performance => "Automatic (performance)".to_string(),
                AutoGoal::BatterySaving => "Automatic (battery saving)".to_string(),
            },
            Some(target) => target_label(inventory, target),
        }
    }
}

/// A target in words, whether or not it can be used right now.
pub fn target_label(inventory: &Inventory, target: &Target) -> String {
    match resolve(inventory, target) {
        Ok(resolved) => resolved.label(),
        Err(_) => match (&target.device, target.backend) {
            (_, Backend::Cpu) => "CPU only".to_string(),
            (Some(key), backend) => format!("{key} ({})", backend.label()),
            (None, backend) => backend.label().to_string(),
        },
    }
}

/// What "automatic" is after under these settings and this power source.
pub fn goal_for(settings: &PowerSettings, power: PowerSource) -> AutoGoal {
    if settings.auto_switch && power == PowerSource::Battery {
        AutoGoal::BatterySaving
    } else {
        AutoGoal::Performance
    }
}

/// Decide what one engine should run on.
///
/// The profile is the temporary override if there is one; otherwise the
/// battery profile while on battery (when auto-switching is on) and the
/// plugged-in profile at all other times, including when the power source is
/// unknown.
pub fn plan(
    settings: &PowerSettings,
    power: PowerSource,
    override_target: Option<&Target>,
    inventory: &Inventory,
    engine: Engine,
    measured: &[Target],
) -> Plan {
    let on_battery = settings.auto_switch && power == PowerSource::Battery;
    let (source, profile, goal) = if on_battery {
        (ChoiceSource::OnBattery, &settings.on_battery, AutoGoal::BatterySaving)
    } else {
        (ChoiceSource::PluggedIn, &settings.plugged_in, AutoGoal::Performance)
    };
    // The override is a manual choice for the chat model; it never touches
    // the saved profiles, and the goal for anything automatic stays the same.
    let (source, chat_target) = match override_target {
        Some(target) => (ChoiceSource::Override, Some(target.clone())),
        None => (source, (profile.mode == ProfileMode::Manual).then(|| profile.target.clone()).flatten()),
    };

    let mut notes = Vec::new();
    let requested = match engine {
        Engine::Chat => chat_target,
        Engine::Embed => match (override_target, &profile.embed_target) {
            // A separate embeddings choice applies under the saved profiles.
            (None, Some(embed)) => Some(embed.clone()),
            _ => match chat_target {
                // OpenVINO runs few embedding models (none on the NPU), so
                // the embeddings do not follow the chat model there.
                Some(target) if target.backend == Backend::OpenVino => {
                    notes.push("Embeddings stay on another engine: OpenVINO does not support Loft's embedding models reliably.".to_string());
                    None
                }
                other => other,
            },
        },
    };

    let automatic = auto_chain(inventory, goal, measured);
    let (chain, strict) = match &requested {
        None => (automatic, false),
        Some(target) => {
            let mut chain = vec![target.clone()];
            if settings.allow_fallback {
                chain.extend(automatic.into_iter().filter(|t| t != target));
            }
            (chain, !settings.allow_fallback)
        }
    };
    Plan {
        source,
        requested,
        goal,
        chain,
        strict,
        notes,
    }
}

// ------------------------------- deciding -----------------------------------

/// What an engine's server is running on right now.
#[derive(Clone, Debug, PartialEq)]
pub struct Running {
    pub model_id: String,
    pub target: Target,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Decision {
    /// Already as wanted: do nothing (a repeated request never restarts).
    Keep,
    /// A change is wanted, but the engine is in the middle of work: wait.
    Wait,
    /// (Re)start on these targets, trying each in order.
    Start(Vec<Resolved>),
    /// Nothing can be tried. The engine stays stopped with this explanation.
    Stop(String),
}

/// Compare what is wanted with what is running.
///
/// `failed` holds targets that could not run this model since the settings
/// last changed (or since the user asked to retry). They are skipped, so a
/// fallback is not undone and retried on every event.
pub fn decide(
    plan: &Plan,
    inventory: &Inventory,
    model_id: &str,
    running: Option<&Running>,
    failed: &HashSet<Target>,
    busy: bool,
) -> Decision {
    let mut candidates: Vec<Resolved> = Vec::new();
    let mut first_problem: Option<String> = None;
    for target in &plan.chain {
        if failed.contains(target) {
            first_problem.get_or_insert_with(|| format!("{} could not run this model", target_label(inventory, target)));
            continue;
        }
        match resolve(inventory, target) {
            Ok(resolved) => candidates.push(resolved),
            Err(reason) => {
                first_problem.get_or_insert_with(|| format!("{}: {reason}", target_label(inventory, target)));
            }
        }
    }
    // Without fallback only the requested target counts.
    if plan.strict {
        candidates.retain(|c| Some(&c.target) == plan.requested.as_ref());
    }
    let Some(first) = candidates.first() else {
        return Decision::Stop(first_problem.unwrap_or_else(|| "No inference engine is installed.".to_string()));
    };
    match running {
        Some(running) if running.model_id == model_id && running.target == first.target => Decision::Keep,
        _ if busy => Decision::Wait,
        _ => Decision::Start(candidates),
    }
}

// ------------------------------ launching -----------------------------------

/// How to start `llama-server` for a resolved target.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct LaunchSpec {
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    /// Upper limit on the context size, where the device needs one.
    pub max_ctx: Option<u32>,
}

/// NPUs compile a fixed-size graph: a large context fails or crawls. This is
/// the smallest that still fits Loft's prompt (excerpts, history, answer).
pub const NPU_CONTEXT: u32 = 4096;

pub fn launch_spec(resolved: &Resolved) -> LaunchSpec {
    let mut spec = LaunchSpec::default();
    match (resolved.backend, &resolved.device) {
        // OpenVINO picks its device from the environment, not from --device.
        (Backend::OpenVino, device) => {
            let id = device.as_ref().map(|d| d.runtime_id.as_str()).unwrap_or("CPU");
            spec.env.push(("GGML_OPENVINO_DEVICE".to_string(), id.to_string()));
            if device.as_ref().is_some_and(|d| d.kind == DeviceKind::Npu) {
                // One request at a time, and a bounded context (see NPU_CONTEXT).
                spec.args.extend(["--parallel".to_string(), "1".to_string()]);
                spec.max_ctx = Some(NPU_CONTEXT);
            }
        }
        // "CPU only" must mean it, even if a GPU build is what gets started.
        (Backend::Cpu, _) => spec.args.extend(["--device".to_string(), "none".to_string()]),
        (_, Some(device)) => spec.args.extend(["--device".to_string(), device.runtime_id.clone()]),
        (_, None) => {}
    }
    // On a graphics card the weights live in the card's memory, but with the
    // model file memory-mapped (llama.cpp's default) the pages read while
    // loading stay in system memory too: about the size of the model, for
    // nothing. Measured on an RTX 4050 with Qwen 2.5 3B, resident memory went
    // from 2.3 GB to 0.6 GB (CUDA) and 2.0 GB to 0.35 GB (Vulkan), with the
    // same load time and answer. Only for a card with memory of its own: on
    // Intel Iris Xe graphics, whose memory is system memory, it saved
    // nothing. Not for Metal (one memory shared with the processor), OpenVINO
    // (not measured), or the processor (the weights have to be in RAM).
    let on_card = resolved.device.as_ref().is_some_and(|d| d.kind == DeviceKind::Gpu && d.discrete);
    if matches!(resolved.backend, Backend::Cuda | Backend::Vulkan) && on_card {
        spec.args.extend(["--load-mode".to_string(), "none".to_string()]);
    }
    spec
}

// ----------------------------- compatibility --------------------------------

/// What llama.cpp's own documentation says about a model on the Intel NPU
/// (the OpenVINO backend's "validated models" table at this revision, tested
/// by its authors with Q4_K_M files on a Core Ultra Series 2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NpuSupport {
    /// Listed as passing on the NPU.
    Validated,
    /// Listed as failing, or as giving wrong answers, on the NPU.
    KnownProblem,
    /// Not in the list either way.
    Unlisted,
}

/// Look a model file up in that table, by its name.
pub fn npu_support(model_file: &str) -> NpuSupport {
    let name = model_file.to_ascii_lowercase().replace('_', "-");
    const PROBLEM: &[&str] = &["qwen3.5", "gemma-4-12b", "gemma-4-e2b"];
    const VALIDATED: &[&str] = &[
        "llama-3.2-1b",
        "llama-3.2-3b",
        "llama-3.1-8b",
        "qwen2.5-1.5b",
        "qwen2.5-coder-7b",
        "qwen3-0.6b",
        "qwen3-1.7b",
        "qwen3-4b",
        "qwen3-8b",
        "gemma-3-4b",
        "gemma-4-e4b",
        "phi-3-mini",
        "phi-3.5-mini",
        "phi-4-mini",
        "mistral-7b",
        "ministral-3b",
        "ministral-8b",
        "deepseek-r1-distill-llama-8b",
    ];
    if PROBLEM.iter().any(|p| name.contains(p)) {
        NpuSupport::KnownProblem
    } else if VALIDATED.iter().any(|v| name.contains(v)) {
        NpuSupport::Validated
    } else {
        NpuSupport::Unlisted
    }
}

/// Something worth knowing before running `model_file` on `resolved`, if
/// anything. A warning, never a refusal: an unvalidated model may be tried.
pub fn compatibility_note(resolved: &Resolved, model_file: &str) -> Option<String> {
    if resolved.backend != Backend::OpenVino {
        return None;
    }
    let on_npu = resolved.device.as_ref().is_some_and(|d| d.kind == DeviceKind::Npu);
    if let Some(device) = resolved.device.as_ref().filter(|d| d.kind == DeviceKind::Gpu && d.vendor != Vendor::Intel) {
        return Some(format!(
            "OpenVINO lists {}, but its graphics support is made for Intel graphics: expect this not to start. CUDA or Vulkan are the engines for this card.",
            display_name(&device.name)
        ));
    }
    if !on_npu {
        return Some("OpenVINO is experimental in llama.cpp: some models fail or answer less accurately than on other engines.".to_string());
    }
    Some(match npu_support(model_file) {
        NpuSupport::Validated => format!(
            "llama.cpp lists this model as working on the Intel NPU. The NPU runs with a {NPU_CONTEXT}-token context, so long conversations are trimmed sooner."
        ),
        NpuSupport::KnownProblem => {
            "llama.cpp lists this model as failing or answering wrongly on the Intel NPU. You can still try it.".to_string()
        }
        NpuSupport::Unlisted => {
            "llama.cpp has not validated this model on the Intel NPU: it may not load, or may be slow. You can still try it.".to_string()
        }
    })
}

/// What to do about a target that could not run a model: one or two
/// concrete steps, for the cases that have them.
pub fn failure_hint(target: &Target, device: Option<&Device>, model_file: &str, reason: &str) -> Option<String> {
    let reason = reason.to_ascii_lowercase();
    let device_kind = device.map(|d| d.kind);
    match target.backend {
        Backend::OpenVino if reason.contains("is not available") => Some(
            "OpenVINO cannot see that device. Install or update the Intel driver for it (for an NPU: the Intel NPU driver, on a Core Ultra processor), then choose \"Look for engines again\".".to_string(),
        ),
        Backend::OpenVino if device_kind == Some(DeviceKind::Npu) => Some(match npu_support(model_file) {
            NpuSupport::Validated => {
                "This model is listed as working on the NPU, so check the Intel NPU driver is current, and that nothing else is using the NPU.".to_string()
            }
            _ => "The NPU runs only some models. Ones llama.cpp lists as working there include Llama 3.2 3B, Gemma 3 4B and Phi-3.5 mini (Q4_K_M): select one of those in Models and try again.".to_string(),
        }),
        Backend::OpenVino if device.is_some_and(|d| d.kind == DeviceKind::Gpu && d.vendor != Vendor::Intel) => Some(
            "OpenVINO's graphics support is made for Intel graphics. For this card use CUDA or Vulkan; OpenVINO can be tried on the Intel processor or Intel graphics instead.".to_string(),
        ),
        Backend::OpenVino => Some("OpenVINO support in llama.cpp is experimental and does not run every model. Try another model, or another engine.".to_string()),
        Backend::Cuda if reason.contains("out of memory") || reason.contains("failed to allocate") => {
            Some("The graphics card does not have enough free memory for this model. Close other programs using it, or choose a smaller model.".to_string())
        }
        Backend::Cuda => Some("Update the NVIDIA graphics driver (the CUDA engine needs a recent one), or use Vulkan on the same card.".to_string()),
        Backend::Vulkan if reason.contains("out of memory") || reason.contains("failed to allocate") => {
            Some("The graphics device does not have enough free memory for this model. Close other programs using it, or choose a smaller model.".to_string())
        }
        _ => None,
    }
}

// ----------------------------- verification ---------------------------------

/// What the server's own log says happened when it loaded the model.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct LoadReport {
    /// `using device Vulkan1 (NVIDIA …)` lines: (id, name).
    pub using: Vec<(String, String)>,
    /// `<id> model buffer size = N MiB` lines: where the weights went.
    pub buffers: Vec<(String, f64)>,
    /// `<id> KV buffer size` and `<id> compute buffer size` lines: the
    /// memory the context needs on top of the weights.
    pub working: Vec<(String, f64)>,
    /// `OpenVINO: using device NPU`.
    pub openvino_device: Option<String>,
    /// Lines that look like the reason for a failure, most recent last.
    pub errors: Vec<String>,
}

impl LoadReport {
    /// Read one line of the server's log.
    pub fn read(&mut self, line: &str) {
        if let Some(rest) = line.split("using device ").nth(1) {
            if line.contains("OpenVINO:") {
                self.openvino_device = rest.split_whitespace().next().map(str::to_string);
            } else if let Some((id, tail)) = rest.split_once(" (") {
                // "NAME) (0000:01:00.0) - 5150 MiB free"; names may contain brackets.
                let tail = tail.rsplit_once(" - ").map(|(left, _)| left).unwrap_or(tail);
                let name = tail.rsplit_once(") (").map(|(name, _)| name).unwrap_or(tail).trim_end_matches(')');
                self.using.push((id.trim().to_string(), name.to_string()));
            }
            return;
        }
        for (marker, weights) in [(" model buffer size =", true), (" KV buffer size =", false), (" compute buffer size =", false)] {
            if let Some((left, right)) = line.split_once(marker) {
                let id = left.split_whitespace().last().unwrap_or("");
                if let Some(mib) = right.split_whitespace().next().and_then(|n| n.parse::<f64>().ok()) {
                    let list = if weights { &mut self.buffers } else { &mut self.working };
                    list.push((id.to_string(), mib));
                }
                return;
            }
        }
        let lower = line.to_ascii_lowercase();
        let failure = [" e ", "error", "failed", "is not available", "abort", "exception", "out of memory", "unable to"];
        if failure.iter().any(|k| lower.contains(k)) && !lower.contains("no error") {
            // Drop the timestamp and log tag in front of the message.
            let message = line.splitn(4, ' ').last().unwrap_or(line).trim();
            if self.errors.len() >= 8 {
                self.errors.remove(0);
            }
            self.errors.push(message.chars().take(300).collect());
        }
    }

    /// Megabytes of the model held by something other than the CPU. (A
    /// `CUDA_Host` or `Vulkan_Host` buffer is system memory.)
    fn offloaded(&self) -> Vec<(&str, f64)> {
        self.buffers
            .iter()
            .filter(|(id, mib)| !is_host_buffer(id) && *mib > 0.0)
            .map(|(id, mib)| (id.as_str(), *mib))
            .collect()
    }

    /// Megabytes the engine says it put on the device `runtime_id`: the
    /// weights plus the context's working memory. `None` if nothing went there.
    pub fn device_memory_mib(&self, runtime_id: &str) -> Option<u64> {
        let total: f64 = self.buffers.iter().chain(&self.working).filter(|(id, _)| id == runtime_id).map(|(_, mib)| mib).sum();
        (total > 0.0).then(|| total.round() as u64)
    }

    /// The same, for an engine that names its buffers its own way
    /// (OpenVINO: `OPENVINO1`, whatever the device): everything under ids
    /// starting with `prefix`, host-side buffers excepted.
    pub fn engine_memory_mib(&self, prefix: &str) -> Option<u64> {
        let total: f64 = self
            .buffers
            .iter()
            .chain(&self.working)
            .filter(|(id, _)| id.starts_with(prefix) && !id.to_ascii_uppercase().ends_with("_HOST"))
            .map(|(_, mib)| mib)
            .sum();
        (total > 0.0).then(|| total.round() as u64)
    }

    /// The most recent thing that looks like a failure reason.
    pub fn last_error(&self) -> Option<&str> {
        self.errors.last().map(String::as_str)
    }
}

/// A buffer in system memory: `CPU`, `CPU_Mapped`, `CUDA_Host`, `Vulkan_Host`.
fn is_host_buffer(id: &str) -> bool {
    let id = id.to_ascii_uppercase();
    id.starts_with("CPU") || id.ends_with("_HOST")
}

/// Whether the model really is where it was asked to be.
#[derive(Clone, Debug, PartialEq)]
pub struct Verification {
    pub ok: bool,
    /// What the log shows, in words.
    pub detail: String,
}

/// Check the load log against the target. A server that is up and healthy is
/// not proof of anything: a GPU build can quietly keep the model in system
/// memory, so this looks at where the weights were actually put.
pub fn verify(resolved: &Resolved, report: &LoadReport) -> Verification {
    let offloaded = report.offloaded();
    let on_cpu: f64 = report.buffers.iter().filter(|(id, _)| is_host_buffer(id)).map(|(_, m)| m).sum();
    match (resolved.backend, &resolved.device) {
        (Backend::Cpu, _) => {
            if offloaded.is_empty() {
                Verification { ok: true, detail: "The model is held in system memory; nothing was offloaded.".into() }
            } else {
                Verification { ok: false, detail: format!("CPU only was requested, but {:.0} MiB of the model went to {}.", offloaded[0].1, offloaded[0].0) }
            }
        }
        (Backend::OpenVino, device) => {
            let wanted = device.as_ref().map(|d| d.runtime_id.as_str()).unwrap_or("CPU");
            match report.openvino_device.as_deref() {
                Some(actual) if actual == wanted => {
                    Verification { ok: true, detail: format!("OpenVINO reports the model is running on {actual}.") }
                }
                Some(actual) => Verification { ok: false, detail: format!("OpenVINO used {actual}, not {wanted}.") },
                None => Verification { ok: false, detail: "OpenVINO did not report which device it used.".into() },
            }
        }
        (_, Some(device)) => {
            let name = display_name(&device.name);
            match offloaded.iter().find(|(id, _)| *id == device.runtime_id) {
                Some((_, mib)) if on_cpu > 0.0 => Verification {
                    ok: true,
                    detail: format!("{mib:.0} MiB of the model is on {name} and {on_cpu:.0} MiB in system memory."),
                },
                Some((_, mib)) => Verification { ok: true, detail: format!("{mib:.0} MiB of the model is on {name}.") },
                None if offloaded.is_empty() => Verification {
                    ok: false,
                    detail: format!("The model was loaded into system memory, not onto {name}."),
                },
                None => Verification { ok: false, detail: format!("The model went to {}, not to {name}.", offloaded[0].0) },
            }
        }
        // Metal and anything else without a named device: offload is expected.
        (_, None) => Verification { ok: !offloaded.is_empty() || report.buffers.is_empty(), detail: "Loaded.".into() },
    }
}

// --------------------------- power debouncing -------------------------------

/// Turns a stream of power notifications into settled changes. A new state
/// counts only once it has held for `settle`; duplicates and quick
/// back-and-forth (a loose charger) produce nothing; "unknown" is ignored.
#[derive(Debug)]
pub struct PowerDebounce {
    stable: PowerSource,
    candidate: Option<(PowerSource, Instant)>,
    settle: Duration,
}

impl PowerDebounce {
    pub fn new(initial: PowerSource, settle: Duration) -> Self {
        PowerDebounce {
            stable: initial,
            candidate: None,
            settle,
        }
    }

    #[cfg(test)]
    pub fn stable(&self) -> PowerSource {
        self.stable
    }

    /// A notification arrived.
    pub fn observe(&mut self, source: PowerSource, now: Instant) {
        if source == PowerSource::Unknown && self.stable != PowerSource::Unknown {
            return; // a failed reading does not unsettle a known state
        }
        if source == self.stable {
            self.candidate = None; // back to where it was: nothing happened
        } else if self.candidate.map(|(s, _)| s) != Some(source) {
            self.candidate = Some((source, now));
        }
    }

    /// When the next `poll` could report a change, if one is pending.
    pub fn due(&self) -> Option<Instant> {
        self.candidate.map(|(_, since)| since + self.settle)
    }

    /// The new settled state, once, when a change has held long enough.
    pub fn poll(&mut self, now: Instant) -> Option<PowerSource> {
        let (source, since) = self.candidate?;
        if now.duration_since(since) < self.settle {
            return None;
        }
        self.candidate = None;
        self.stable = source;
        Some(source)
    }
}

// --------------------------------- tests ------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    const VULKAN_LIST: &str = "Available devices:\n  Vulkan0: Intel(R) Iris(R) Xe Graphics (16193 MiB, 15425 MiB free)\n  Vulkan1: NVIDIA GeForce RTX 4050 Laptop GPU (5920 MiB, 5152 MiB free)\n";
    const CUDA_LIST: &str = "Available devices:\n  CUDA0: NVIDIA GeForce RTX 4050 Laptop GPU (6141 MiB, 5000 MiB free)\n";
    const OPENVINO_LIST: &str = "Available devices:\n  OPENVINO0: GGML_OPENVINO_DEVICE=CPU (selected) - Intel(R) Core(TM) Ultra 5 238V (0 MiB, 0 MiB free)\n  OPENVINO1: GGML_OPENVINO_DEVICE=GPU - Intel(R) Arc(TM) Graphics (iGPU) (14000 MiB, 14000 MiB free)\n  OPENVINO2: GGML_OPENVINO_DEVICE=NPU - Intel(R) AI Boost (0 MiB, 0 MiB free)\n";

    const NVIDIA: &str = "nvidia-geforce-rtx-4050-laptop-gpu";
    const IRIS: &str = "intel-r-iris-r-xe-graphics";

    fn available(backend: Backend, list: &str) -> BackendInfo {
        BackendInfo {
            backend,
            status: BackendStatus::Available,
            experimental: backend.experimental(),
            devices: parse_devices(backend, list),
            detail: None,
        }
    }
    fn missing(backend: Backend, status: BackendStatus) -> BackendInfo {
        BackendInfo {
            backend,
            status,
            experimental: backend.experimental(),
            devices: Vec::new(),
            detail: None,
        }
    }
    fn inventory(backends: Vec<BackendInfo>) -> Inventory {
        Inventory {
            cpu: "Test CPU".into(),
            hardware: Vec::new(),
            backends,
            revision: "test".into(),
        }
    }
    /// A laptop like the development machine: NVIDIA + Intel graphics, with
    /// Vulkan and CPU builds installed and CUDA and OpenVINO not.
    fn laptop() -> Inventory {
        inventory(vec![
            missing(Backend::Cuda, BackendStatus::NotInstalled),
            available(Backend::Vulkan, VULKAN_LIST),
            missing(Backend::OpenVino, BackendStatus::NotInstalled),
            available(Backend::Cpu, ""),
        ])
    }
    /// The same laptop with every optional engine installed, plus an NPU.
    fn everything() -> Inventory {
        inventory(vec![
            available(Backend::Cuda, CUDA_LIST),
            available(Backend::Vulkan, VULKAN_LIST),
            available(Backend::OpenVino, OPENVINO_LIST),
            available(Backend::Cpu, ""),
        ])
    }
    fn vulkan(key: &str) -> Target {
        Target::new(Backend::Vulkan, Some(key))
    }
    fn npu() -> Target {
        Target::new(Backend::OpenVino, Some("intel-r-ai-boost"))
    }
    fn manual(target: Target) -> Profile {
        Profile {
            mode: ProfileMode::Manual,
            target: Some(target),
            embed_target: None,
        }
    }
    fn chat_plan(settings: &PowerSettings, power: PowerSource, over: Option<&Target>, inv: &Inventory) -> Plan {
        plan(settings, power, over, inv, Engine::Chat, &[])
    }
    fn no_failures() -> HashSet<Target> {
        HashSet::new()
    }
    fn running(target: Target) -> Running {
        Running {
            model_id: "model".into(),
            target,
        }
    }

    // ---- hardware and backends

    #[test]
    fn devices_are_parsed_per_backend() {
        let gpus = parse_devices(Backend::Vulkan, VULKAN_LIST);
        assert_eq!(gpus.len(), 2);
        assert_eq!((gpus[0].runtime_id.as_str(), gpus[0].key.as_str()), ("Vulkan0", IRIS));
        assert_eq!((gpus[0].vendor, gpus[0].discrete, gpus[0].memory_mib), (Vendor::Intel, false, Some(16193)));
        assert_eq!((gpus[1].runtime_id.as_str(), gpus[1].key.as_str()), ("Vulkan1", NVIDIA));
        assert_eq!((gpus[1].vendor, gpus[1].discrete), (Vendor::Nvidia, true));

        // Vulkan does not mean NVIDIA, and CUDA sees only the NVIDIA card.
        let cuda = parse_devices(Backend::Cuda, CUDA_LIST);
        assert_eq!(cuda.len(), 1);
        assert_eq!((cuda[0].runtime_id.as_str(), cuda[0].key.as_str()), ("CUDA0", NVIDIA));

        // OpenVINO names its devices itself; the NPU is a device of its own.
        let ov = parse_devices(Backend::OpenVino, OPENVINO_LIST);
        assert_eq!(ov.iter().map(|d| d.runtime_id.as_str()).collect::<Vec<_>>(), ["CPU", "GPU", "NPU"]);
        assert_eq!(ov.iter().map(|d| d.kind).collect::<Vec<_>>(), [DeviceKind::Cpu, DeviceKind::Gpu, DeviceKind::Npu]);
        assert_eq!((ov[2].name.as_str(), ov[2].vendor, ov[2].memory_mib), ("Intel(R) AI Boost", Vendor::Intel, None));

        // Nothing usable in the output, or a CPU build: no devices.
        assert!(parse_devices(Backend::Cpu, "Available devices:\n  (none)\n").is_empty());
        assert!(parse_devices(Backend::Vulkan, "error: something\nnot a device line").is_empty());
    }

    #[test]
    fn device_keys_survive_reordering_and_tell_twins_apart() {
        // The same card keeps its key when llama.cpp numbers it differently.
        let swapped = "  Vulkan0: NVIDIA GeForce RTX 4050 Laptop GPU (5920 MiB, 5152 MiB free)\n  Vulkan1: Intel(R) Iris(R) Xe Graphics (16193 MiB, 15425 MiB free)\n";
        let devices = parse_devices(Backend::Vulkan, swapped);
        assert_eq!(devices[0].key, NVIDIA);
        assert_eq!(devices[0].runtime_id, "Vulkan0");
        let inv = inventory(vec![BackendInfo { devices, ..available(Backend::Vulkan, "") }]);
        assert_eq!(resolve(&inv, &vulkan(NVIDIA)).unwrap().device.unwrap().runtime_id, "Vulkan0");

        let twins = "  CUDA0: NVIDIA RTX A4000 (16000 MiB, 15000 MiB free)\n  CUDA1: NVIDIA RTX A4000 (16000 MiB, 15000 MiB free)\n";
        let keys: Vec<String> = parse_devices(Backend::Cuda, twins).into_iter().map(|d| d.key).collect();
        assert_eq!(keys, ["nvidia-rtx-a4000", "nvidia-rtx-a4000#2"]);

        assert!(is_discrete("AMD Radeon RX 7600M XT"));
        assert!(!is_discrete("AMD Radeon(TM) Graphics"));
        assert!(is_discrete("Intel(R) Arc(TM) A770 Graphics"));
        assert!(!is_discrete("Intel(R) Arc(TM) Graphics"));
        assert_eq!(vendor_of("AMD Radeon(TM) Graphics"), Vendor::Amd);
    }

    #[test]
    fn targets_resolve_or_explain_why_not() {
        let inv = laptop();
        let nvidia = resolve(&inv, &vulkan(NVIDIA)).unwrap();
        assert_eq!(nvidia.label(), "NVIDIA GeForce RTX 4050 Laptop GPU (Vulkan)");
        assert_eq!(resolve(&inv, &vulkan(IRIS)).unwrap().label(), "Intel Iris Xe Graphics (Vulkan)");
        assert_eq!(resolve(&inv, &Target::cpu()).unwrap().label(), "CPU only");
        // No device named: the discrete card.
        assert_eq!(resolve(&inv, &Target::new(Backend::Vulkan, None)).unwrap().device.unwrap().key, NVIDIA);

        // NVIDIA hardware does not make CUDA available.
        assert_eq!(resolve(&inv, &Target::new(Backend::Cuda, Some(NVIDIA))).unwrap_err(), "the CUDA engine is not installed");
        assert_eq!(resolve(&inv, &npu()).unwrap_err(), "the OpenVINO engine is not installed");
        // A device that has gone (card removed, driver update).
        assert!(resolve(&inv, &vulkan("amd-radeon-rx-7600")).unwrap_err().contains("not available to Vulkan right now"));
        // A backend this system does not have at all.
        assert!(resolve(&inv, &Target::new(Backend::Metal, None)).unwrap_err().contains("not supported"));

        let no_driver = inventory(vec![BackendInfo {
            detail: Some("The NVIDIA driver is not installed.".into()),
            ..missing(Backend::Cuda, BackendStatus::DriverMissing)
        }]);
        assert_eq!(resolve(&no_driver, &Target::new(Backend::Cuda, None)).unwrap_err(), "The NVIDIA driver is not installed.");

        // An experimental backend is still selectable when it is available.
        let all = everything();
        assert!(all.backend(Backend::OpenVino).unwrap().experimental);
        assert_eq!(resolve(&all, &npu()).unwrap().label(), "Intel AI Boost (OpenVINO)");
    }

    #[test]
    fn automatic_orders_depend_on_the_goal() {
        let labels = |inv: &Inventory, goal| -> Vec<String> { auto_chain(inv, goal, &[]).iter().map(|t| target_label(inv, t)).collect() };

        let inv = laptop();
        assert_eq!(
            labels(&inv, AutoGoal::Performance),
            ["NVIDIA GeForce RTX 4050 Laptop GPU (Vulkan)", "Intel Iris Xe Graphics (Vulkan)", "CPU only"]
        );
        // On battery the discrete card is the last resort.
        assert_eq!(
            labels(&inv, AutoGoal::BatterySaving),
            ["Intel Iris Xe Graphics (Vulkan)", "CPU only", "NVIDIA GeForce RTX 4050 Laptop GPU (Vulkan)"]
        );

        // CUDA goes first for performance once it is installed; the NPU is
        // never picked by rule of thumb.
        let all = everything();
        assert_eq!(labels(&all, AutoGoal::Performance)[0], "NVIDIA GeForce RTX 4050 Laptop GPU (CUDA)");
        assert!(labels(&all, AutoGoal::Performance).iter().chain(&labels(&all, AutoGoal::BatterySaving)).all(|l| !l.contains("OpenVINO")));

        // A machine with nothing but the CPU build.
        let cpu_only = inventory(vec![available(Backend::Cpu, "")]);
        assert_eq!(labels(&cpu_only, AutoGoal::Performance), ["CPU only"]);
        assert_eq!(labels(&cpu_only, AutoGoal::BatterySaving), ["CPU only"]);

        // Measurements, when there are any, come first (and may include the NPU).
        let measured = [npu(), Target::cpu()];
        let chain = auto_chain(&all, AutoGoal::BatterySaving, &measured);
        assert_eq!(chain[..2], measured);
        assert_eq!(chain.iter().filter(|t| **t == Target::cpu()).count(), 1, "no duplicates");
    }

    // ---- power states

    #[test]
    fn the_power_source_selects_the_profile() {
        let inv = laptop();
        let settings = PowerSettings {
            plugged_in: manual(vulkan(NVIDIA)),
            on_battery: manual(Target::cpu()),
            ..PowerSettings::default()
        };
        // Starting on mains, on battery, and with an unknown source.
        let ac = chat_plan(&settings, PowerSource::Ac, None, &inv);
        assert_eq!((ac.source, ac.requested.clone()), (ChoiceSource::PluggedIn, Some(vulkan(NVIDIA))));
        let battery = chat_plan(&settings, PowerSource::Battery, None, &inv);
        assert_eq!((battery.source, battery.requested.clone()), (ChoiceSource::OnBattery, Some(Target::cpu())));
        let unknown = chat_plan(&settings, PowerSource::Unknown, None, &inv);
        assert_eq!(unknown.source, ChoiceSource::PluggedIn);

        // Auto-switching off: the plugged-in choice, whatever the power.
        let fixed = PowerSettings { auto_switch: false, ..settings.clone() };
        assert_eq!(chat_plan(&fixed, PowerSource::Battery, None, &inv).requested, Some(vulkan(NVIDIA)));

        // The defaults: automatic for performance, automatic for battery.
        let defaults = PowerSettings::default();
        let ac = chat_plan(&defaults, PowerSource::Ac, None, &inv);
        assert_eq!((ac.requested.clone(), ac.goal, ac.strict), (None, AutoGoal::Performance, false));
        assert_eq!(ac.requested_label(&inv), "Automatic (performance)");
        assert_eq!(ac.chain[0], vulkan(NVIDIA));
        let battery = chat_plan(&defaults, PowerSource::Battery, None, &inv);
        assert_eq!(battery.requested_label(&inv), "Automatic (battery saving)");
        assert_eq!(battery.chain[0], vulkan(IRIS));
    }

    #[test]
    fn power_notifications_are_debounced() {
        let settle = Duration::from_secs(2);
        let t0 = Instant::now();
        let at = |ms: u64| t0 + Duration::from_millis(ms);
        let mut power = PowerDebounce::new(PowerSource::Ac, settle);

        // Duplicate notifications of the current state do nothing.
        power.observe(PowerSource::Ac, at(0));
        assert_eq!(power.poll(at(5000)), None);

        // Unplugged: reported once, after it has held.
        power.observe(PowerSource::Battery, at(100));
        power.observe(PowerSource::Battery, at(150)); // duplicate: the clock is not restarted
        assert_eq!(power.poll(at(1000)), None);
        assert_eq!(power.due(), Some(at(2100)));
        assert_eq!(power.poll(at(2100)), Some(PowerSource::Battery));
        assert_eq!(power.poll(at(2200)), None);
        assert_eq!(power.stable(), PowerSource::Battery);

        // A loose charger: back and forth inside the window changes nothing.
        power.observe(PowerSource::Ac, at(3000));
        power.observe(PowerSource::Battery, at(3500));
        assert_eq!(power.poll(at(9000)), None);
        assert_eq!(power.stable(), PowerSource::Battery);

        // Rapid changes settle on the last one.
        power.observe(PowerSource::Ac, at(10_000));
        power.observe(PowerSource::Battery, at(10_200));
        power.observe(PowerSource::Ac, at(10_400));
        assert_eq!(power.poll(at(12_300)), None, "the clock runs from the last change");
        assert_eq!(power.poll(at(12_400)), Some(PowerSource::Ac));

        // A failed reading does not unsettle a known state…
        power.observe(PowerSource::Unknown, at(13_000));
        assert_eq!(power.poll(at(20_000)), None);
        assert_eq!(power.stable(), PowerSource::Ac);
        // …but a first real reading replaces "unknown".
        let mut fresh = PowerDebounce::new(PowerSource::Unknown, settle);
        fresh.observe(PowerSource::Battery, at(0));
        assert_eq!(fresh.poll(at(2000)), Some(PowerSource::Battery));
    }

    // ---- choosing and switching

    #[test]
    fn a_repeated_request_never_restarts() {
        let inv = laptop();
        let settings = PowerSettings::default();
        let plan = chat_plan(&settings, PowerSource::Ac, None, &inv);
        // Nothing running (first start, or after a crash): start.
        let Decision::Start(candidates) = decide(&plan, &inv, "model", None, &no_failures(), false) else {
            panic!("expected a start");
        };
        assert_eq!(candidates[0].label(), "NVIDIA GeForce RTX 4050 Laptop GPU (Vulkan)");
        assert_eq!(candidates.len(), 3, "the fallbacks come along");
        // Running as wanted: keep, however often it is asked, busy or not.
        let now = running(vulkan(NVIDIA));
        assert_eq!(decide(&plan, &inv, "model", Some(&now), &no_failures(), false), Decision::Keep);
        assert_eq!(decide(&plan, &inv, "model", Some(&now), &no_failures(), true), Decision::Keep);
        // Another model was selected: restart.
        assert!(matches!(decide(&plan, &inv, "other-model", Some(&now), &no_failures(), false), Decision::Start(_)));
    }

    #[test]
    fn a_switch_waits_for_work_in_progress() {
        let inv = laptop();
        let settings = PowerSettings::default();
        let now = running(vulkan(NVIDIA));
        // The charger is pulled while an answer is streaming.
        let battery = chat_plan(&settings, PowerSource::Battery, None, &inv);
        assert_eq!(decide(&battery, &inv, "model", Some(&now), &no_failures(), true), Decision::Wait);
        // The answer ends: the switch goes ahead.
        let Decision::Start(candidates) = decide(&battery, &inv, "model", Some(&now), &no_failures(), false) else {
            panic!("expected a start");
        };
        assert_eq!(candidates[0].label(), "Intel Iris Xe Graphics (Vulkan)");
        // Plugged back in before the answer ended: nothing to do after all.
        let ac = chat_plan(&settings, PowerSource::Ac, None, &inv);
        assert_eq!(decide(&ac, &inv, "model", Some(&now), &no_failures(), false), Decision::Keep);
    }

    #[test]
    fn fallback_is_remembered_and_not_retried_in_a_loop() {
        let inv = everything();
        let settings = PowerSettings { plugged_in: manual(npu()), ..PowerSettings::default() };
        let plan = chat_plan(&settings, PowerSource::Ac, None, &inv);
        assert_eq!(plan.requested, Some(npu()));
        assert!(!plan.strict);

        // First the NPU, with the automatic order behind it.
        let Decision::Start(candidates) = decide(&plan, &inv, "model", None, &no_failures(), false) else {
            panic!("expected a start");
        };
        assert_eq!(candidates[0].target, npu());
        assert_eq!(candidates[1].label(), "NVIDIA GeForce RTX 4050 Laptop GPU (CUDA)");

        // The NPU could not load the model, CUDA did. The request is still
        // the NPU, the engine keeps running on CUDA, and nothing loops.
        let failed: HashSet<Target> = [npu()].into();
        let on_cuda = running(Target::new(Backend::Cuda, Some(NVIDIA)));
        assert_eq!(decide(&plan, &inv, "model", Some(&on_cuda), &failed, false), Decision::Keep);
        assert_eq!(plan.requested_label(&inv), "Intel AI Boost (OpenVINO)");
    }

    #[test]
    fn without_fallback_a_failure_stops_the_engine() {
        let inv = everything();
        let settings = PowerSettings {
            plugged_in: manual(npu()),
            allow_fallback: false,
            ..PowerSettings::default()
        };
        let plan = chat_plan(&settings, PowerSource::Ac, None, &inv);
        assert!(plan.strict);
        assert_eq!(plan.chain, [npu()], "nothing else may be tried");

        let Decision::Start(candidates) = decide(&plan, &inv, "model", None, &no_failures(), false) else {
            panic!("expected a start");
        };
        assert_eq!(candidates.len(), 1);

        // It failed: no silent CPU run. Stopped, with the reason, and the
        // user's choice is still what is requested.
        let failed: HashSet<Target> = [npu()].into();
        assert_eq!(
            decide(&plan, &inv, "model", None, &failed, false),
            Decision::Stop("Intel AI Boost (OpenVINO) could not run this model".into())
        );
        assert_eq!(plan.requested, Some(npu()));

        // The engine is not installed at all: also a stop, with the reason.
        let laptop = laptop();
        let plan = chat_plan(&settings, PowerSource::Ac, None, &laptop);
        let Decision::Stop(reason) = decide(&plan, &laptop, "model", None, &no_failures(), false) else {
            panic!("expected a stop");
        };
        assert!(reason.contains("the OpenVINO engine is not installed"), "{reason}");
    }

    #[test]
    fn a_missing_device_keeps_the_preference() {
        // The saved choice names a card that is no longer there.
        let inv = laptop();
        let gone = vulkan("amd-radeon-rx-7600");
        let settings = PowerSettings { plugged_in: manual(gone.clone()), ..PowerSettings::default() };
        let plan = chat_plan(&settings, PowerSource::Ac, None, &inv);
        assert_eq!(plan.requested, Some(gone.clone()), "the preference is kept");
        assert_eq!(plan.requested_label(&inv), "amd-radeon-rx-7600 (Vulkan)");
        // With fallback the engine still runs, on the next best thing.
        let Decision::Start(candidates) = decide(&plan, &inv, "model", None, &no_failures(), false) else {
            panic!("expected a start");
        };
        assert_eq!(candidates[0].label(), "NVIDIA GeForce RTX 4050 Laptop GPU (Vulkan)");
        // Without fallback it stops and says why.
        let strict = PowerSettings { allow_fallback: false, ..settings };
        let plan = chat_plan(&strict, PowerSource::Ac, None, &inv);
        assert!(matches!(decide(&plan, &inv, "model", None, &no_failures(), false), Decision::Stop(_)));
    }

    #[test]
    fn an_override_outranks_the_power_profiles_and_survives_power_changes() {
        let inv = everything();
        let settings = PowerSettings {
            plugged_in: manual(vulkan(NVIDIA)),
            on_battery: manual(Target::cpu()),
            ..PowerSettings::default()
        };
        let trying = npu();
        for power in [PowerSource::Ac, PowerSource::Battery, PowerSource::Unknown] {
            let plan = chat_plan(&settings, power, Some(&trying), &inv);
            assert_eq!((plan.source, plan.requested.clone()), (ChoiceSource::Override, Some(trying.clone())), "{power:?}");
            assert_eq!(decide(&plan, &inv, "model", Some(&running(trying.clone())), &no_failures(), false), Decision::Keep);
        }
        // The saved profiles are untouched, and apply again once it ends.
        assert_eq!(chat_plan(&settings, PowerSource::Battery, None, &inv).requested, Some(Target::cpu()));
    }

    #[test]
    fn embeddings_follow_the_chat_engine_except_onto_openvino() {
        let inv = everything();
        let embed = |settings: &PowerSettings, over: Option<&Target>| plan(settings, PowerSource::Ac, over, &inv, Engine::Embed, &[]);

        // A manual GPU choice covers both engines.
        let settings = PowerSettings { plugged_in: manual(vulkan(IRIS)), ..PowerSettings::default() };
        assert_eq!(embed(&settings, None).requested, Some(vulkan(IRIS)));

        // The NPU for chat: embeddings go their own (automatic) way, and say so.
        let settings = PowerSettings { plugged_in: manual(npu()), allow_fallback: false, ..PowerSettings::default() };
        let plan_embed = embed(&settings, None);
        assert_eq!(plan_embed.requested, None);
        assert!(!plan_embed.strict, "the embeddings are not part of the experiment");
        assert!(plan_embed.notes[0].contains("OpenVINO"));
        assert!(plan_embed.chain.iter().all(|t| t.backend != Backend::OpenVino));
        // Same for a temporary NPU override.
        assert_eq!(embed(&PowerSettings::default(), Some(&npu())).requested, None);

        // Advanced: a separate embeddings target.
        let settings = PowerSettings {
            plugged_in: Profile { embed_target: Some(Target::cpu()), ..manual(Target::new(Backend::Cuda, Some(NVIDIA))) },
            ..PowerSettings::default()
        };
        assert_eq!(embed(&settings, None).requested, Some(Target::cpu()));
        assert_eq!(chat_plan(&settings, PowerSource::Ac, None, &inv).requested, Some(Target::new(Backend::Cuda, Some(NVIDIA))));
    }

    #[test]
    fn saved_settings_survive_round_trips_and_bad_input() {
        let settings = PowerSettings {
            plugged_in: manual(vulkan(NVIDIA)),
            on_battery: Profile { embed_target: Some(Target::cpu()), ..manual(npu()) },
            auto_switch: false,
            allow_fallback: false,
        };
        let json = serde_json::to_string(&settings).unwrap();
        assert!(json.contains(r#""backend":"openvino""#) && json.contains(r#""pluggedIn""#) && json.contains(r#""allowFallback":false"#));
        assert_eq!(serde_json::from_str::<PowerSettings>(&json).unwrap(), settings);

        // Written by an older version, or by hand: missing parts take defaults.
        assert_eq!(serde_json::from_str::<PowerSettings>("{}").unwrap(), PowerSettings::default());
        let partial: PowerSettings = serde_json::from_str(r#"{"onBattery":{"mode":"manual","target":{"backend":"cpu"}}}"#).unwrap();
        assert_eq!(partial.on_battery, manual(Target::cpu()));
        assert!(partial.auto_switch && partial.allow_fallback);
        // Manual with no target behaves as automatic.
        let odd: PowerSettings = serde_json::from_str(r#"{"pluggedIn":{"mode":"manual"}}"#).unwrap();
        assert_eq!(chat_plan(&odd, PowerSource::Ac, None, &laptop()).requested, None);
        // An unknown backend name is an error for the caller to default on.
        assert!(serde_json::from_str::<PowerSettings>(r#"{"pluggedIn":{"mode":"manual","target":{"backend":"quantum"}}}"#).is_err());
    }

    // ---- launching and verifying

    #[test]
    fn launch_arguments_say_exactly_where_to_run() {
        let inv = everything();
        let spec = |target: &Target| launch_spec(&resolve(&inv, target).unwrap());

        // On a graphics card, without keeping the model file mapped in RAM as well.
        assert_eq!(spec(&vulkan(NVIDIA)).args, ["--device", "Vulkan1", "--load-mode", "none"]);
        // (Not on built-in graphics, whose memory is system memory: no saving there.)
        assert_eq!(spec(&vulkan(IRIS)).args, ["--device", "Vulkan0"]);
        assert_eq!(spec(&Target::new(Backend::Cuda, Some(NVIDIA))).args, ["--device", "CUDA0", "--load-mode", "none"]);
        // CPU only must not offload, even from a GPU build.
        assert_eq!(spec(&Target::cpu()).args, ["--device", "none"]);
        assert!(spec(&Target::cpu()).env.is_empty());

        // OpenVINO: the device goes in the environment, and the NPU gets one
        // slot and a bounded context.
        let on_npu = spec(&npu());
        assert_eq!(on_npu.env, [("GGML_OPENVINO_DEVICE".to_string(), "NPU".to_string())]);
        assert_eq!(on_npu.args, ["--parallel", "1"]);
        assert_eq!(on_npu.max_ctx, Some(NPU_CONTEXT));
        let on_ov_gpu = spec(&Target::new(Backend::OpenVino, Some("intel-r-arc-tm-graphics-igpu")));
        assert_eq!(on_ov_gpu.env[0].1, "GPU");
        assert!(on_ov_gpu.args.is_empty() && on_ov_gpu.max_ctx.is_none());
    }

    fn report(log: &str) -> LoadReport {
        let mut report = LoadReport::default();
        log.lines().for_each(|line| report.read(line));
        report
    }

    // Lines copied from llama-server b12110 at log level 4.
    const GPU_LOG: &str = "0.00.311.001 I llama_prepare_model_devices: using device Vulkan1 (NVIDIA GeForce RTX 4050 Laptop GPU) (0000:01:00.0) - 5150 MiB free\n\
0.00.350.718 I load_tensors: offloaded 13/13 layers to GPU\n\
0.00.350.722 I load_tensors:   CPU_Mapped model buffer size =    44.72 MiB\n\
0.00.350.723 I load_tensors:      Vulkan1 model buffer size =   216.15 MiB\n";
    const NO_OFFLOAD_LOG: &str = "0.00.281.541 I load_tensors: offloading 11 repeating layers to GPU\n\
0.00.281.541 I load_tensors: offloaded 13/13 layers to GPU\n\
0.00.281.544 I load_tensors:   CPU_Mapped model buffer size =   260.86 MiB\n";

    #[test]
    fn the_load_log_shows_where_the_model_really_is() {
        let inv = everything();
        let nvidia = resolve(&inv, &vulkan(NVIDIA)).unwrap();
        let iris = resolve(&inv, &vulkan(IRIS)).unwrap();
        let cpu = resolve(&inv, &Target::cpu()).unwrap();

        let gpu = report(GPU_LOG);
        assert_eq!(gpu.using, [("Vulkan1".to_string(), "NVIDIA GeForce RTX 4050 Laptop GPU".to_string())]);
        assert_eq!(gpu.buffers, [("CPU_Mapped".to_string(), 44.72), ("Vulkan1".to_string(), 216.15)]);
        let ok = verify(&nvidia, &gpu);
        assert!(ok.ok, "{}", ok.detail);
        assert_eq!(ok.detail, "216 MiB of the model is on NVIDIA GeForce RTX 4050 Laptop GPU and 45 MiB in system memory.");
        // The same log is not proof for a different card, nor for "CPU only".
        assert!(!verify(&iris, &gpu).ok);
        assert!(!verify(&cpu, &gpu).ok);

        // "offloaded 13/13 layers to GPU" is printed even when nothing was:
        // only the buffer lines count.
        let none = report(NO_OFFLOAD_LOG);
        assert!(verify(&cpu, &none).ok);
        let wrong = verify(&nvidia, &none);
        assert!(!wrong.ok);
        assert_eq!(wrong.detail, "The model was loaded into system memory, not onto NVIDIA GeForce RTX 4050 Laptop GPU.");

        // Starting up proves nothing: an empty log verifies no GPU.
        assert!(!verify(&nvidia, &LoadReport::default()).ok);

        // The memory the engine says it took on the device: weights, plus
        // the context's cache and working buffers.
        let full = report(
            "0.01.135.378 I load_tensors:      Vulkan1 model buffer size =  1834.82 MiB\n\
0.01.135.379 I load_tensors:   CPU_Mapped model buffer size =   166.50 MiB\n\
0.02.676.632 I llama_kv_cache:    Vulkan1 KV buffer size =   288.00 MiB\n\
0.02.704.699 I sched_reserve:    Vulkan1 compute buffer size =    88.51 MiB\n\
0.02.704.712 I sched_reserve: Vulkan_Host compute buffer size =    24.02 MiB\n",
        );
        assert_eq!(full.device_memory_mib("Vulkan1"), Some(2211));
        assert_eq!(full.device_memory_mib("Vulkan0"), None);
        assert!(verify(&nvidia, &full).ok, "working buffers do not confuse the check");
    }

    #[test]
    fn npu_compatibility_is_a_warning_with_a_way_forward() {
        // The table in llama.cpp's OpenVINO documentation, by file name.
        assert_eq!(npu_support("Llama-3.2-3B-Instruct-Q4_K_M.gguf"), NpuSupport::Validated);
        assert_eq!(npu_support("gemma-3-4b-it-Q4_K_M.gguf"), NpuSupport::Validated);
        assert_eq!(npu_support("Phi-3.5-mini-instruct-Q4_K_M.gguf"), NpuSupport::Validated);
        assert_eq!(npu_support("qwen2.5-1.5b-instruct-q4_k_m.gguf"), NpuSupport::Validated);
        // Loft's default chat model is a size the table does not cover.
        assert_eq!(npu_support("qwen2.5-3b-instruct-q4_k_m.gguf"), NpuSupport::Unlisted);
        assert_eq!(npu_support("Qwen_Qwen3.5-4B-Q4_K_M.gguf"), NpuSupport::KnownProblem);
        assert_eq!(npu_support("google_gemma-4-E2B-it-Q4_K_M.gguf"), NpuSupport::KnownProblem);
        assert_eq!(npu_support("Qwen3-4B-Q4_K_M.gguf"), NpuSupport::Validated);

        let inv = everything();
        let on_npu = resolve(&inv, &npu()).unwrap();
        let unlisted = compatibility_note(&on_npu, "qwen2.5-3b-instruct-q4_k_m.gguf").unwrap();
        assert!(unlisted.contains("has not validated") && unlisted.contains("You can still try it"));
        assert!(compatibility_note(&on_npu, "Llama-3.2-3B-Instruct-Q4_K_M.gguf").unwrap().contains("lists this model as working"));
        // Nothing to say about the ordinary engines.
        assert_eq!(compatibility_note(&resolve(&inv, &vulkan(NVIDIA)).unwrap(), "any.gguf"), None);
        assert_eq!(compatibility_note(&resolve(&inv, &Target::cpu()).unwrap(), "any.gguf"), None);

        // After a failure: what to do next.
        let npu_device = on_npu.device.clone();
        let nvidia_device = resolve(&inv, &vulkan(NVIDIA)).unwrap().device;
        let hint = failure_hint(&npu(), npu_device.as_ref(), "qwen2.5-3b-instruct-q4_k_m.gguf", "the model could not be loaded").unwrap();
        assert!(hint.contains("Llama 3.2 3B"));
        let driver = failure_hint(&npu(), npu_device.as_ref(), "x.gguf", "GGML_OPENVINO_DEVICE=NPU is not available. Set it to one of").unwrap();
        assert!(driver.contains("Intel NPU driver"));
        assert!(failure_hint(&vulkan(NVIDIA), nvidia_device.as_ref(), "x.gguf", "ErrorOutOfDeviceMemory: failed to allocate").unwrap().contains("enough free memory"));
        assert_eq!(failure_hint(&Target::cpu(), None, "x.gguf", "anything"), None);
        assert_eq!(failure_hint(&vulkan(NVIDIA), nvidia_device.as_ref(), "x.gguf", "simulated failure"), None);

        // The goal follows the power source only while switching is on.
        let settings = PowerSettings::default();
        assert_eq!(goal_for(&settings, PowerSource::Battery), AutoGoal::BatterySaving);
        assert_eq!(goal_for(&settings, PowerSource::Ac), AutoGoal::Performance);
        assert_eq!(goal_for(&settings, PowerSource::Unknown), AutoGoal::Performance);
        let fixed = PowerSettings { auto_switch: false, ..PowerSettings::default() };
        assert_eq!(goal_for(&fixed, PowerSource::Battery), AutoGoal::Performance);
    }

    // Copied from llama-server b12110 (OpenVINO 2026.4.1, Windows) on a
    // laptop with Intel and NVIDIA graphics and no NPU.
    const OPENVINO_REFUSED: &str = "D:\\a\\llama.cpp\\llama.cpp\\ggml\\src\\ggml-openvino\\ggml-openvino-extra.cpp:86: GGML OpenVINO Backend: GGML_OPENVINO_DEVICE=LOFT_PROBE is not available. Set it to one of the available OpenVINO devices:\n  CPU: 12th Gen Intel(R) Core(TM) i7-12700H\n  GPU.0: Intel(R) Iris(R) Xe Graphics (iGPU)\n  GPU.1: NVIDIA GeForce RTX 4050 Laptop GPU (dGPU)\n";
    const OPENVINO_STARTED: &str = "0.00.190.995 I OpenVINO: using device GPU.0\n\
0.00.201.575 I cmn  common_param:   - OPENVINO0: GGML_OPENVINO_DEVICE=CPU - 12th Gen Intel(R) Core(TM) i7-12700H (32387 MiB, 11543 MiB free)\n\
0.00.201.695 I cmn  common_param:   - OPENVINO1: GGML_OPENVINO_DEVICE=GPU.0 (selected) - Intel(R) Iris(R) Xe Graphics (iGPU) (12955 MiB, 11543 MiB free)\n\
0.00.201.740 I cmn  common_param:   - OPENVINO2: GGML_OPENVINO_DEVICE=GPU.1 - NVIDIA GeForce RTX 4050 Laptop GPU (dGPU) (6140 MiB, 6140 MiB free)\n\
0.00.499.818 I llama_prepare_model_devices: using device OPENVINO1 (GGML_OPENVINO_DEVICE=GPU.0 (selected) - Intel(R) Iris(R) Xe Graphics (iGPU)) (unknown id) - 11494 MiB free\n\
0.01.661.985 I load_tensors:   CPU_Mapped model buffer size =   166.92 MiB\n\
0.01.661.988 I load_tensors:    OPENVINO1 model buffer size =  1994.82 MiB\n\
0.11.327.931 I llama_context: OPENVINO1_HOST  output buffer size =     2.32 MiB\n\
0.11.398.796 I llama_kv_cache:  OPENVINO1 KV buffer size =   144.00 MiB\n\
0.11.452.309 I sched_reserve:  OPENVINO1 compute buffer size =    76.50 MiB\n\
0.11.452.319 I sched_reserve: OPENVINO1_HOST compute buffer size =    12.02 MiB\n";

    #[test]
    fn openvino_devices_are_learned_from_what_the_engine_really_prints() {
        // This build prints nothing at all for --list-devices...
        assert!(parse_devices(Backend::OpenVino, "").is_empty());
        // ...but refuses an unknown device by naming the ones it has.
        let pairs = parse_openvino_refusal(OPENVINO_REFUSED);
        assert_eq!(
            pairs,
            [
                ("CPU".to_string(), "12th Gen Intel(R) Core(TM) i7-12700H".to_string()),
                ("GPU.0".to_string(), "Intel(R) Iris(R) Xe Graphics (iGPU)".to_string()),
                ("GPU.1".to_string(), "NVIDIA GeForce RTX 4050 Laptop GPU (dGPU)".to_string()),
            ]
        );
        let devices = openvino_devices(&pairs);
        let seen: Vec<(&str, DeviceKind, Vendor, bool)> = devices.iter().map(|d| (d.runtime_id.as_str(), d.kind, d.vendor, d.discrete)).collect();
        assert_eq!(
            seen,
            [
                ("CPU", DeviceKind::Cpu, Vendor::Intel, false),
                ("GPU.0", DeviceKind::Gpu, Vendor::Intel, false),
                ("GPU.1", DeviceKind::Gpu, Vendor::Nvidia, true),
            ]
        );
        assert_eq!(devices[1].key, "intel-r-iris-r-xe-graphics-igpu");
        // The same devices, with the same keys, as the list a starting server
        // logs (which is the documented --list-devices format, after a prefix).
        let listing: String = OPENVINO_STARTED.lines().filter_map(|l| l.split_once("  - ")).map(|(_, device)| format!("  {device}\n")).collect();
        let logged = parse_devices(Backend::OpenVino, &listing);
        assert_eq!(logged.iter().map(|d| (&d.key, &d.runtime_id)).collect::<Vec<_>>(), devices.iter().map(|d| (&d.key, &d.runtime_id)).collect::<Vec<_>>());
        // Anything else is not a device list.
        assert!(parse_openvino_refusal("error: something else went wrong\n  CPU: not a list").is_empty());
        assert!(parse_openvino_refusal("…available OpenVINO devices:\n  note: none\n  GPU.x: bad id\n").is_empty());

        // A real start on the Intel graphics: the engine says where it is,
        // and how much it put there (under its own name for the device).
        let started = report(OPENVINO_STARTED);
        assert_eq!(started.openvino_device.as_deref(), Some("GPU.0"));
        let inv = Inventory {
            backends: vec![BackendInfo {
                backend: Backend::OpenVino,
                status: BackendStatus::Available,
                experimental: true,
                devices: devices.clone(),
                detail: None,
            }],
            ..everything()
        };
        let on_intel = resolve(&inv, &Target::new(Backend::OpenVino, Some("intel-r-iris-r-xe-graphics-igpu"))).unwrap();
        let on_nvidia = resolve(&inv, &Target::new(Backend::OpenVino, Some("nvidia-geforce-rtx-4050-laptop-gpu-dgpu"))).unwrap();
        assert!(verify(&on_intel, &started).ok);
        assert_eq!(verify(&on_nvidia, &started).detail, "OpenVINO used GPU.0, not GPU.1.");
        assert_eq!(launch_spec(&on_intel).env, [("GGML_OPENVINO_DEVICE".to_string(), "GPU.0".to_string())]);
        assert_eq!(started.engine_memory_mib("OPENVINO"), Some(2215));
        assert_eq!(started.device_memory_mib("GPU.0"), None);

        // OpenVINO lists the NVIDIA card but cannot run on it: say so before, and after.
        assert!(compatibility_note(&on_nvidia, "x.gguf").unwrap().contains("made for Intel graphics"));
        assert!(compatibility_note(&on_intel, "x.gguf").unwrap().contains("experimental"));
        let died = load_failure(&LoadReport::default(), true);
        assert_eq!(died, "the engine stopped while loading the model, without giving a reason");
        assert!(failure_hint(&on_nvidia.target, on_nvidia.device.as_ref(), "x.gguf", &died).unwrap().contains("use CUDA or Vulkan"));
        assert_eq!(load_failure(&LoadReport::default(), false), "the model did not finish loading in time");
        assert_eq!(load_failure(&report("0.00.100.000 E llama_model_load: failed to load model\n"), true), "the model could not be loaded (failed to load model)");
    }

    #[test]
    fn openvino_must_name_the_device_it_used() {
        let inv = everything();
        let on_npu = resolve(&inv, &npu()).unwrap();
        let said_npu = report("0.00.120.000 I OpenVINO: using device NPU\n");
        assert_eq!(said_npu.openvino_device.as_deref(), Some("NPU"));
        assert!(verify(&on_npu, &said_npu).ok);
        // Asked for the NPU, got the CPU: not verified.
        let said_cpu = report("0.00.120.000 I OpenVINO: using device CPU\n");
        assert_eq!(verify(&on_npu, &said_cpu).detail, "OpenVINO used CPU, not NPU.");
        // Said nothing: not verified either.
        assert!(!verify(&on_npu, &LoadReport::default()).ok);

        // The reason for a failure is kept for the user.
        let failed = report("0.00.100.000 E GGML OpenVINO Backend: GGML_OPENVINO_DEVICE=NPU is not available. Set it to one of the available OpenVINO devices:\n");
        assert!(failed.last_error().unwrap().contains("GGML_OPENVINO_DEVICE=NPU is not available"));
    }
}
