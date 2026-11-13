//! What Loft is using right now: memory, graphics memory and processor time,
//! for each part of it (the chat model, the embedding model, the picture
//! model while it reads pictures, and Loft's own window), for the panel in
//! Settings.
//!
//! Two sources, because neither says it all:
//! - The operating system, per process: the memory it has in RAM right now
//!   (resident memory, pages of a memory-mapped model file included) and
//!   the processor time it has used. Not "private" or committed memory: on
//!   Windows that also counts what a process has allocated on a graphics
//!   card, so a model on the card would be counted twice.
//! - The model server's own load log: how big the weights and the context
//!   are, and how much of them went to which device.

use std::collections::HashMap;
use std::time::Instant;

use serde::Serialize;

use crate::inference::{Backend, DeviceKind, LoadReport, Resolved};

const MIB: u64 = 1024 * 1024;

/// One process, as the process list shows it.
#[derive(Clone, Debug, PartialEq)]
pub struct Proc {
    pub pid: u32,
    pub parent: u32,
    pub name: String,
}

/// What a process is using, as the system reports it.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct Measure {
    /// Memory the process has in RAM right now.
    pub memory_bytes: Option<u64>,
    /// Processor time used since it started, all cores together.
    pub cpu_ms: Option<u64>,
}

/// Which part of Loft a process belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Group {
    Chat,
    Embed,
    /// Any other model server: the picture model while indexing, or an
    /// engine being tried or tested.
    OtherModel,
    /// Loft itself and its window (on Windows and Linux the web view runs in
    /// processes of its own).
    App,
}

/// Each process of Loft's own tree, by the part it belongs to. `me` is
/// Loft's own process; `chat` and `embed` the two model servers. A process
/// started by a model server belongs to that server.
pub fn group(me: u32, procs: &[Proc], chat: Option<u32>, embed: Option<u32>) -> HashMap<u32, Group> {
    let mut children: HashMap<u32, Vec<&Proc>> = HashMap::new();
    for p in procs {
        if p.pid != p.parent {
            children.entry(p.parent).or_default().push(p);
        }
    }
    let mut out = HashMap::from([(me, Group::App)]);
    // (process, the part it belongs to)
    let mut queue: Vec<(u32, Group)> = vec![(me, Group::App)];
    while let Some((pid, of)) = queue.pop() {
        for child in children.get(&pid).map(Vec::as_slice).unwrap_or_default() {
            if out.contains_key(&child.pid) {
                continue; // a loop in a stale list: never walk it twice
            }
            let part = if of != Group::App {
                of
            } else if Some(child.pid) == chat {
                Group::Chat
            } else if Some(child.pid) == embed {
                Group::Embed
            } else if child.name.to_ascii_lowercase().starts_with("llama-server") {
                Group::OtherModel
            } else {
                Group::App
            };
            out.insert(child.pid, part);
            queue.push((child.pid, part));
        }
    }
    out
}

/// Turns processor time into a share of the whole computer, between calls.
#[derive(Default)]
pub struct Meter {
    last: HashMap<u32, (Instant, u64)>,
}

impl Meter {
    /// The share of all processor cores `pid` used since the last reading,
    /// in percent (100 = every core flat out). `None` on the first reading.
    pub fn cpu_percent(&mut self, pid: u32, cpu_ms: u64, now: Instant, cores: usize) -> Option<f32> {
        let before = self.last.insert(pid, (now, cpu_ms));
        let (then, used_then) = before?;
        let wall_ms = now.duration_since(then).as_secs_f64() * 1000.0;
        if wall_ms < 50.0 || cpu_ms < used_then {
            return None; // too soon to say, or a different process with a reused id
        }
        let share = (cpu_ms - used_then) as f64 / (wall_ms * cores.max(1) as f64) * 100.0;
        Some(share.clamp(0.0, 100.0) as f32)
    }

    /// Forget processes that are gone.
    pub fn keep_only(&mut self, alive: &[u32]) {
        self.last.retain(|pid, _| alive.contains(pid));
    }
}

/// Memory a model has on a device other than the processor.
#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DeviceUse {
    /// "NVIDIA GeForce RTX 4050 Laptop GPU", "Intel AI Boost".
    pub name: String,
    pub bytes: u64,
    /// The device has no memory of its own (graphics built into the
    /// processor, an NPU, Apple silicon): this is system memory, and is
    /// already in the process's resident memory (measured on Intel Iris Xe).
    pub shared: bool,
}

/// How much of a model went to a device other than the processor, from the
/// server's load log.
pub fn device_use(resolved: &Resolved, report: &LoadReport) -> Option<DeviceUse> {
    let device = resolved.device.as_ref()?;
    if resolved.backend == Backend::Cpu || device.kind == DeviceKind::Cpu {
        return None;
    }
    let mib = match resolved.backend {
        // OpenVINO names its buffers after itself, whatever the device.
        Backend::OpenVino => report.engine_memory_mib("OPENVINO"),
        _ => report.device_memory_mib(&device.runtime_id),
    }?;
    Some(DeviceUse {
        name: crate::inference::display_name(&device.name),
        bytes: mib * MIB,
        shared: !device.discrete,
    })
}

/// One part of Loft, for the panel.
#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Part {
    /// chat | embed | otherModel | app
    pub kind: &'static str,
    pub label: String,
    /// The model's name, for the model servers.
    pub model: Option<String>,
    /// What it runs on: "NVIDIA GeForce RTX 4050 Laptop GPU (CUDA)", "CPU only".
    pub runs_on: Option<String>,
    /// System memory the part's processes have in RAM right now.
    pub memory_bytes: Option<u64>,
    pub cpu_percent: Option<f32>,
    pub device: Option<DeviceUse>,
    /// The model's weights, wherever they are.
    pub weights_bytes: Option<u64>,
    /// The context: room for the conversation and excerpts (the KV cache)
    /// plus the working memory to compute with.
    pub context_bytes: Option<u64>,
    pub context_tokens: Option<u32>,
    pub processes: usize,
}

/// What a running model server is, for the measurement.
#[derive(Clone, Debug)]
pub struct ModelServer {
    pub pid: u32,
    pub model: Option<String>,
    pub resolved: Resolved,
    pub report: LoadReport,
    pub ctx: u32,
}

/// The whole picture, for the panel.
#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Usage {
    pub parts: Vec<Part>,
    /// System memory used by all of Loft (shared graphics memory is in it).
    pub memory_bytes: u64,
    /// Memory on graphics cards with memory of their own.
    pub device_bytes: u64,
    pub cpu_percent: Option<f32>,
    pub system_total_bytes: Option<u64>,
    pub system_available_bytes: Option<u64>,
    pub cores: usize,
}

/// Measure every part of Loft now. `chat`/`embed`: the running model servers.
pub fn measure(meter: &mut Meter, chat: Option<ModelServer>, embed: Option<ModelServer>) -> Usage {
    let me = std::process::id();
    let procs = sys::processes();
    let groups = group(me, &procs, chat.as_ref().map(|s| s.pid), embed.as_ref().map(|s| s.pid));
    let cores = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1);
    let now = Instant::now();

    // (memory, cpu, processes, any cpu reading) per part
    let mut sums: HashMap<Group, (Option<u64>, Option<f32>, usize)> = HashMap::new();
    for (&pid, &part) in &groups {
        let m = sys::measure(pid);
        let entry = sums.entry(part).or_insert((None, None, 0));
        entry.2 += 1;
        if let Some(bytes) = m.memory_bytes {
            entry.0 = Some(entry.0.unwrap_or(0) + bytes);
        }
        if let Some(share) = m.cpu_ms.and_then(|ms| meter.cpu_percent(pid, ms, now, cores)) {
            entry.1 = Some(entry.1.unwrap_or(0.0) + share);
        }
    }
    meter.keep_only(&groups.keys().copied().collect::<Vec<_>>());

    let model_part = |kind: &'static str, label: &str, group: Group, server: &ModelServer| {
        let (memory, cpu, processes) = sums.get(&group).copied().unwrap_or((None, None, 0));
        let weights: f64 = server.report.buffers.iter().map(|(_, mib)| mib).sum();
        let context: f64 = server.report.working.iter().map(|(_, mib)| mib).sum();
        Part {
            kind,
            label: label.to_string(),
            model: server.model.clone(),
            runs_on: Some(server.resolved.label()),
            memory_bytes: memory,
            cpu_percent: cpu,
            device: device_use(&server.resolved, &server.report),
            weights_bytes: (weights > 0.0).then(|| (weights * MIB as f64) as u64),
            context_bytes: (context > 0.0).then(|| (context * MIB as f64) as u64),
            context_tokens: Some(server.ctx),
            processes,
        }
    };
    let plain_part = |kind: &'static str, label: &str, group: Group| {
        sums.get(&group).map(|&(memory, cpu, processes)| Part {
            kind,
            label: label.to_string(),
            model: None,
            runs_on: None,
            memory_bytes: memory,
            cpu_percent: cpu,
            device: None,
            weights_bytes: None,
            context_bytes: None,
            context_tokens: None,
            processes,
        })
    };

    let mut parts = Vec::new();
    if let Some(server) = &chat {
        parts.push(model_part("chat", "Chat model", Group::Chat, server));
    }
    if let Some(server) = &embed {
        parts.push(model_part("embed", "Embedding model", Group::Embed, server));
    }
    parts.extend(plain_part("otherModel", "Picture model (reading pictures)", Group::OtherModel));
    parts.extend(plain_part("app", "Loft and its window", Group::App));

    let memory_bytes = parts.iter().map(|p| p.memory_bytes.unwrap_or(0)).sum();
    let device_bytes = parts.iter().filter_map(|p| p.device.as_ref()).filter(|d| !d.shared).map(|d| d.bytes).sum();
    let readings: Vec<f32> = parts.iter().filter_map(|p| p.cpu_percent).collect();
    let (system_total_bytes, system_available_bytes) = sys::system_memory();
    Usage {
        parts,
        memory_bytes,
        device_bytes,
        cpu_percent: (!readings.is_empty()).then(|| readings.iter().sum()),
        system_total_bytes,
        system_available_bytes,
        cores,
    }
}

// ------------------------------- Windows ------------------------------------

#[cfg(target_os = "windows")]
mod sys {
    use super::{Measure, Proc};
    use windows_sys::Win32::Foundation::{CloseHandle, FILETIME, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
    };
    use windows_sys::Win32::System::ProcessStatus::{K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};
    use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
    use windows_sys::Win32::System::Threading::{GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};

    pub fn processes() -> Vec<Proc> {
        let mut out = Vec::new();
        // SAFETY: the snapshot handle is checked and closed; `entry` is a
        // valid PROCESSENTRY32W with its size set, as the API requires.
        unsafe {
            let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
            if snapshot == INVALID_HANDLE_VALUE || snapshot.is_null() {
                return out;
            }
            let mut entry: PROCESSENTRY32W = std::mem::zeroed();
            entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
            let mut more = Process32FirstW(snapshot, &mut entry) != 0;
            while more {
                let len = entry.szExeFile.iter().position(|&c| c == 0).unwrap_or(entry.szExeFile.len());
                out.push(Proc {
                    pid: entry.th32ProcessID,
                    parent: entry.th32ParentProcessID,
                    name: String::from_utf16_lossy(&entry.szExeFile[..len]),
                });
                more = Process32NextW(snapshot, &mut entry) != 0;
            }
            CloseHandle(snapshot);
        }
        out
    }

    pub fn measure(pid: u32) -> Measure {
        // SAFETY: the handle is checked before use and closed after; the
        // out-structures are zeroed and of the sizes the API is told.
        unsafe {
            let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if handle.is_null() {
                return Measure::default();
            }
            let mut counters: PROCESS_MEMORY_COUNTERS = std::mem::zeroed();
            counters.cb = std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32;
            let memory_bytes = (K32GetProcessMemoryInfo(handle, &mut counters, counters.cb) != 0).then_some(counters.WorkingSetSize as u64);
            let (mut created, mut exited, mut kernel, mut user): (FILETIME, FILETIME, FILETIME, FILETIME) =
                (std::mem::zeroed(), std::mem::zeroed(), std::mem::zeroed(), std::mem::zeroed());
            let ticks = |t: FILETIME| ((t.dwHighDateTime as u64) << 32) | t.dwLowDateTime as u64;
            let cpu_ms = (GetProcessTimes(handle, &mut created, &mut exited, &mut kernel, &mut user) != 0)
                .then(|| (ticks(kernel) + ticks(user)) / 10_000); // 100 ns units
            CloseHandle(handle);
            Measure { memory_bytes, cpu_ms }
        }
    }

    pub fn system_memory() -> (Option<u64>, Option<u64>) {
        // SAFETY: a zeroed MEMORYSTATUSEX with its length set, as required.
        unsafe {
            let mut status: MEMORYSTATUSEX = std::mem::zeroed();
            status.dwLength = std::mem::size_of::<MEMORYSTATUSEX>() as u32;
            if GlobalMemoryStatusEx(&mut status) == 0 {
                return (None, None);
            }
            (Some(status.ullTotalPhys), Some(status.ullAvailPhys))
        }
    }
}

// -------------------------------- Linux -------------------------------------

#[cfg(target_os = "linux")]
mod sys {
    use super::{Measure, Proc};

    /// Clock ticks per second in /proc (USER_HZ, 100 on every common build).
    const TICKS: u64 = 100;

    pub fn processes() -> Vec<Proc> {
        let Ok(dir) = std::fs::read_dir("/proc") else { return Vec::new() };
        dir.filter_map(|entry| {
            let pid: u32 = entry.ok()?.file_name().to_str()?.parse().ok()?;
            let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
            let (name, parent, _) = super::parse_proc_stat(&stat)?;
            Some(Proc { pid, parent, name })
        })
        .collect()
    }

    pub fn measure(pid: u32) -> Measure {
        let memory_bytes = std::fs::read_to_string(format!("/proc/{pid}/status")).ok().and_then(|status| {
            let line = status.lines().find(|l| l.starts_with("VmRSS:"))?;
            line.split_whitespace().nth(1)?.parse::<u64>().ok().map(|kb| kb * 1024)
        });
        let cpu_ms = std::fs::read_to_string(format!("/proc/{pid}/stat"))
            .ok()
            .and_then(|stat| super::parse_proc_stat(&stat))
            .map(|(_, _, ticks)| ticks * 1000 / TICKS);
        Measure { memory_bytes, cpu_ms }
    }

    pub fn system_memory() -> (Option<u64>, Option<u64>) {
        let Ok(info) = std::fs::read_to_string("/proc/meminfo") else { return (None, None) };
        let field = |name: &str| {
            let line = info.lines().find(|l| l.starts_with(name))?;
            line.split_whitespace().nth(1)?.parse::<u64>().ok().map(|kb| kb * 1024)
        };
        (field("MemTotal:"), field("MemAvailable:"))
    }
}

/// `/proc/<pid>/stat`: (name, parent, user + system clock ticks). The name
/// is in brackets and may itself contain spaces or brackets.
#[cfg_attr(not(any(test, target_os = "linux")), allow(dead_code))]
fn parse_proc_stat(stat: &str) -> Option<(String, u32, u64)> {
    let open = stat.find('(')?;
    let close = stat.rfind(')')?;
    let name = stat.get(open + 1..close)?.to_string();
    let fields: Vec<&str> = stat.get(close + 1..)?.split_whitespace().collect();
    // After the name: state, ppid, … utime (12th), stime (13th).
    let parent = fields.get(1)?.parse().ok()?;
    let user: u64 = fields.get(11)?.parse().ok()?;
    let system: u64 = fields.get(12)?.parse().ok()?;
    Some((name, parent, user + system))
}

// -------------------------------- macOS -------------------------------------

#[cfg(target_os = "macos")]
mod sys {
    use super::{Measure, Proc};
    use std::process::Command;

    fn ps(args: &[&str]) -> String {
        Command::new("ps").args(args).output().map(|o| String::from_utf8_lossy(&o.stdout).into_owned()).unwrap_or_default()
    }

    pub fn processes() -> Vec<Proc> {
        ps(&["-A", "-o", "pid=,ppid=,comm="])
            .lines()
            .filter_map(|line| {
                let mut words = line.split_whitespace();
                let pid = words.next()?.parse().ok()?;
                let parent = words.next()?.parse().ok()?;
                let path = words.collect::<Vec<_>>().join(" ");
                let name = path.rsplit('/').next().unwrap_or(&path).to_string();
                Some(Proc { pid, parent, name })
            })
            .collect()
    }

    pub fn measure(pid: u32) -> Measure {
        let out = ps(&["-o", "rss=,time=", "-p", &pid.to_string()]);
        let mut words = out.split_whitespace();
        let memory_bytes = words.next().and_then(|kb| kb.parse::<u64>().ok()).map(|kb| kb * 1024);
        let cpu_ms = words.next().and_then(super::parse_ps_time);
        Measure { memory_bytes, cpu_ms }
    }

    pub fn system_memory() -> (Option<u64>, Option<u64>) {
        let total = Command::new("sysctl")
            .args(["-n", "hw.memsize"])
            .output()
            .ok()
            .and_then(|o| String::from_utf8_lossy(&o.stdout).trim().parse().ok());
        (total, None)
    }
}

/// `ps` processor time: "1:02.50", "1:02:03.25", "2-01:02:03.00" → ms.
#[cfg_attr(not(any(test, target_os = "macos")), allow(dead_code))]
fn parse_ps_time(text: &str) -> Option<u64> {
    let (days, rest) = match text.split_once('-') {
        Some((d, rest)) => (d.parse::<u64>().ok()?, rest),
        None => (0, text),
    };
    let mut seconds = 0.0f64;
    for part in rest.split(':') {
        seconds = seconds * 60.0 + part.parse::<f64>().ok()?;
    }
    Some(days * 86_400_000 + (seconds * 1000.0).round() as u64)
}

#[cfg(not(any(target_os = "windows", target_os = "linux", target_os = "macos")))]
mod sys {
    use super::{Measure, Proc};
    pub fn processes() -> Vec<Proc> {
        Vec::new()
    }
    pub fn measure(_pid: u32) -> Measure {
        Measure::default()
    }
    pub fn system_memory() -> (Option<u64>, Option<u64>) {
        (None, None)
    }
}

// -------------------------------- tests -------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::inference::{Device, Target, Vendor};
    use std::time::Duration;

    fn proc(pid: u32, parent: u32, name: &str) -> Proc {
        Proc { pid, parent, name: name.into() }
    }

    #[test]
    fn processes_are_put_with_the_part_they_belong_to() {
        let procs = [
            proc(1, 0, "explorer.exe"),
            proc(10, 1, "loft.exe"),
            proc(11, 10, "msedgewebview2.exe"),
            proc(12, 11, "msedgewebview2.exe"),
            proc(20, 10, "llama-server.exe"),
            proc(21, 10, "llama-server.exe"),
            proc(22, 21, "helper.exe"),
            proc(23, 10, "llama-server.exe"),
            proc(30, 1, "llama-server.exe"), // someone else's: not Loft's
            proc(31, 30, "x.exe"),
        ];
        let groups = group(10, &procs, Some(20), Some(21));
        assert_eq!(groups.len(), 7);
        assert_eq!(groups[&10], Group::App);
        assert_eq!((groups[&11], groups[&12]), (Group::App, Group::App));
        assert_eq!(groups[&20], Group::Chat);
        assert_eq!((groups[&21], groups[&22]), (Group::Embed, Group::Embed));
        assert_eq!(groups[&23], Group::OtherModel);
        assert!(!groups.contains_key(&30) && !groups.contains_key(&31));

        // A process list with a loop in it (ids reused) does not hang.
        let looped = [proc(10, 11, "loft.exe"), proc(11, 10, "a.exe")];
        assert_eq!(group(10, &looped, None, None).len(), 2);
    }

    #[test]
    fn processor_share_is_measured_between_readings() {
        let mut meter = Meter::default();
        let start = Instant::now();
        assert_eq!(meter.cpu_percent(5, 1_000, start, 4), None);
        // 2 s later, 2 s of processor time on a 4-core machine: a quarter.
        assert_eq!(meter.cpu_percent(5, 3_000, start + Duration::from_secs(2), 4), Some(25.0));
        // A process id reused by a younger process: no reading, not a negative one.
        assert_eq!(meter.cpu_percent(5, 10, start + Duration::from_secs(4), 4), None);
        // Too soon after the last reading to say anything.
        assert_eq!(meter.cpu_percent(5, 20, start + Duration::from_millis(4010), 4), None);
        meter.keep_only(&[]);
        assert_eq!(meter.cpu_percent(5, 50, start + Duration::from_secs(6), 4), None);
    }

    fn report(log: &str) -> LoadReport {
        let mut r = LoadReport::default();
        log.lines().for_each(|l| r.read(l));
        r
    }

    fn resolved(backend: Backend, device: Option<(&str, DeviceKind, bool, &str)>) -> Resolved {
        Resolved {
            target: Target { backend, device: device.map(|d| d.0.to_lowercase()) },
            backend,
            device: device.map(|(name, kind, discrete, runtime_id)| Device {
                key: name.to_lowercase(),
                name: name.into(),
                kind,
                vendor: Vendor::Other,
                discrete,
                memory_mib: None,
                runtime_id: runtime_id.into(),
            }),
        }
    }

    #[test]
    fn device_memory_comes_from_the_load_log() {
        let cuda_log = report(
            "I load_tensors:        CUDA0 model buffer size =  1834.82 MiB\n\
             I load_tensors:   CPU_Mapped model buffer size =   166.50 MiB\n\
             I llama_kv_cache:      CUDA0 KV buffer size =  1152.00 MiB\n\
             I sched_reserve:      CUDA0 compute buffer size =   300.75 MiB\n\
             I sched_reserve:  CUDA_Host compute buffer size =    70.01 MiB\n",
        );
        let card = resolved(Backend::Cuda, Some(("NVIDIA GeForce RTX 4050 Laptop GPU", DeviceKind::Gpu, true, "CUDA0")));
        let used = device_use(&card, &cuda_log).unwrap();
        assert_eq!(used.name, "NVIDIA GeForce RTX 4050 Laptop GPU");
        assert_eq!(used.bytes, (1834.82f64 + 1152.0 + 300.75).round() as u64 * MIB);
        assert!(!used.shared);

        // On the processor nothing is on a device.
        let cpu_log = report("I load_tensors:   CPU_Mapped model buffer size =  1900.00 MiB\nI llama_kv_cache:        CPU KV buffer size =  1152.00 MiB\n");
        assert_eq!(device_use(&resolved(Backend::Cpu, None), &cpu_log), None);
        assert_eq!(device_use(&resolved(Backend::OpenVino, Some(("CPU", DeviceKind::Cpu, false, "CPU"))), &cpu_log), None);

        // OpenVINO on an NPU: its own buffer names, and memory shared with the system.
        let npu_log = report(
            "I load_tensors:    OPENVINO1 model buffer size =  1994.82 MiB\n\
             I llama_kv_cache:  OPENVINO1 KV buffer size =   144.00 MiB\n\
             I sched_reserve: OPENVINO1_HOST compute buffer size =    12.02 MiB\n",
        );
        let npu = device_use(&resolved(Backend::OpenVino, Some(("Intel(R) AI Boost", DeviceKind::Npu, false, "NPU"))), &npu_log).unwrap();
        assert_eq!((npu.name.as_str(), npu.bytes, npu.shared), ("Intel AI Boost", 2139 * MIB, true));
    }

    #[test]
    fn process_figures_are_read_on_each_system() {
        assert_eq!(parse_proc_stat("1234 (llama server (x)) S 99 1 1 0 -1 4194560 100 0 0 0 250 50 0 0 20 0"), Some(("llama server (x)".into(), 99, 300)));
        assert_eq!(parse_proc_stat("garbage"), None);
        assert_eq!(parse_ps_time("0:01.50"), Some(1_500));
        assert_eq!(parse_ps_time("1:02:03.25"), Some(3_723_250));
        assert_eq!(parse_ps_time("2-00:00:01.00"), Some(172_801_000));
        assert_eq!(parse_ps_time("x"), None);
    }

    /// A real model server, started the way Loft starts one, measured.
    ///   cargo test --lib usage::tests::measures_a_real_model_server -- --ignored --nocapture
    #[tokio::test]
    #[ignore = "requires the Nomic model and the bundled llama-server"]
    async fn measures_a_real_model_server() {
        use crate::runtime::{wait_healthy, Runtime, ServerKind};
        let model = std::path::PathBuf::from(std::env::var("APPDATA").unwrap_or_default())
            .join("com.loft.desktop")
            .join("models")
            .join("nomic-embed-text-v1.5.f16.gguf");
        if !model.exists() {
            eprintln!("skip: {} not found", model.display());
            return;
        }
        let mut rt = Runtime::new();
        let port = rt.spawn(ServerKind::Embed, &model, "nomic-embed-text-v1.5", 8192).expect("spawn");
        assert!(wait_healthy(port, Duration::from_secs(60)).await);
        let server = rt.usage_of(ServerKind::Embed, Some("Nomic".into())).unwrap();
        let mut meter = Meter::default();
        measure(&mut meter, None, Some(server.clone()));
        tokio::time::sleep(Duration::from_millis(500)).await;
        let usage = measure(&mut meter, None, Some(server.clone()));
        let part = usage.parts.iter().find(|p| p.kind == "embed").unwrap();
        eprintln!("{} on {}: {part:?}", server.model.as_deref().unwrap_or("?"), server.resolved.label());
        // The server, and on Windows the console host it is given.
        assert!((1..=2).contains(&part.processes), "{} processes", part.processes);
        assert!(part.memory_bytes.unwrap_or(0) > 20 * MIB);
        assert!(part.cpu_percent.is_some());
        assert_eq!(part.context_tokens, Some(8192));
        // The weights are all accounted for, wherever they went (the file is 262 MiB).
        let weights = part.weights_bytes.unwrap() / MIB;
        assert!((250..=270).contains(&weights), "{weights} MiB of weights");
        // On a graphics card, the card is named and holds the weights.
        if let Some(device) = &part.device {
            assert!(device.bytes >= 200 * MIB && !device.name.is_empty(), "{device:?}");
        }
        // The servers are Loft's children here too: none is put with the app.
        assert!(usage.parts.iter().all(|p| p.kind != "otherModel"));
    }

    #[test]
    fn loft_measures_itself() {
        // A real reading of this test process: it is "Loft" here.
        let mut meter = Meter::default();
        let first = measure(&mut meter, None, None);
        let app = first.parts.iter().find(|p| p.kind == "app").expect("Loft's own part");
        assert!(app.memory_bytes.unwrap_or(0) > MIB, "{app:?}");
        assert!(app.processes >= 1);
        assert_eq!(first.cpu_percent, None, "no processor share on the first reading");
        if cfg!(any(windows, target_os = "linux")) {
            let total = first.system_total_bytes.unwrap();
            assert!(total > 512 * MIB && first.system_available_bytes.unwrap() <= total);
        }
        let mut busy = 0u64;
        let until = Instant::now() + Duration::from_millis(300);
        while Instant::now() < until {
            busy = busy.wrapping_add(1);
        }
        let second = measure(&mut meter, None, None);
        assert!(busy > 0 && second.cpu_percent.is_some_and(|p| p > 0.0), "{:?}", second.cpu_percent);
    }
}
