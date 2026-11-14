//! llama.cpp runtime: finding the engine builds, asking them what hardware
//! they can use, and starting and stopping `llama-server` processes.
//!
//! Loft runs separate servers for chat and for embeddings (embedding models
//! are pooling-based and do not share a process with chat). Each server is
//! started on a specific *target*: a backend (which build) and a device.
//! Deciding the target is `inference.rs`; switching safely is `engine.rs`.
//! This module only does what it is told, and records what the server itself
//! logs about where the model went, so that can be reported truthfully.

use std::collections::VecDeque;
use std::io::{BufRead, BufReader};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::hardware;
use crate::inference::{
    self, AutoGoal, Backend, BackendInfo, BackendStatus, DeviceKind, Hardware, Inventory, LoadReport, Resolved, Vendor,
};

/// The llama.cpp release the engine builds come from (see `scripts/fetch-llama.mjs`).
pub const LLAMA_REVISION: &str = "b12110";

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ServerKind {
    Chat,
    Embed,
}

impl ServerKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ServerKind::Chat => "chat",
            ServerKind::Embed => "embed",
        }
    }
}

/// Per-server lifecycle state surfaced to the UI.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ServerState {
    Stopped,
    Loading,
    Ready,
    Error,
}

impl ServerState {
    pub fn as_str(self) -> &'static str {
        match self {
            ServerState::Stopped => "stopped",
            ServerState::Loading => "loading",
            ServerState::Ready => "ready",
            ServerState::Error => "error",
        }
    }
}

/// What a server has written to its log: the facts about where the model was
/// loaded, and the last lines for diagnostics.
#[derive(Default)]
struct ServerLog {
    report: LoadReport,
    tail: VecDeque<String>,
}

/// Lines of server log kept in memory for diagnostics.
const LOG_TAIL: usize = 300;

/// A running llama-server process. Killed on drop.
pub struct Server {
    child: Child,
    pub port: u16,
    pub model_id: String,
    /// The backend and device it was started on.
    pub resolved: Resolved,
    /// The context size it was started with, in tokens.
    pub ctx: u32,
    log: Arc<Mutex<ServerLog>>,
}

impl Server {
    /// What the server's own log says about where the model went.
    pub fn report(&self) -> LoadReport {
        self.log.lock().map(|log| log.report.clone()).unwrap_or_default()
    }

    /// The last `n` lines the server logged.
    pub fn log_tail(&self, n: usize) -> Vec<String> {
        let Ok(log) = self.log.lock() else {
            return Vec::new();
        };
        log.tail.iter().rev().take(n).rev().cloned().collect()
    }

    /// True when the process has exited (crashed or was killed).
    pub fn exited(&mut self) -> bool {
        !matches!(self.child.try_wait(), Ok(None))
    }

    /// The server's process id.
    pub fn pid(&self) -> u32 {
        self.child.id()
    }

    /// The most memory the process has held at once, in MiB, where the
    /// operating system can tell. (Memory on a graphics card is not in it.)
    pub fn peak_memory_mib(&self) -> Option<u64> {
        peak_memory_mib(&self.child)
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

pub struct Runtime {
    /// Directories searched for `<platform>-<backend>/llama-server`.
    roots: Vec<PathBuf>,
    pub chat: Option<Server>,
    pub embed: Option<Server>,
    pub chat_state: ServerState,
    pub embed_state: ServerState,
    pub message: Option<String>,
    /// The engines and devices found, once looked for.
    inventory: Option<Inventory>,
}

impl Runtime {
    #[cfg(test)]
    pub fn new() -> Self {
        Self::with_extra_roots(Vec::new())
    }

    /// `extra_roots` come first: the bundled app's `binaries/` resources, and
    /// the folder that optional engines are installed into.
    pub fn with_extra_roots(extra_roots: Vec<PathBuf>) -> Self {
        let mut roots = extra_roots;
        roots.extend(binary_roots());
        Runtime {
            roots,
            chat: None,
            embed: None,
            chat_state: ServerState::Stopped,
            embed_state: ServerState::Stopped,
            message: None,
            inventory: None,
        }
    }

    /// The engines installed and the devices each can use. Looked up on first
    /// use (it starts each build briefly to ask); `refresh_inventory` repeats it.
    pub fn inventory(&mut self) -> &Inventory {
        if self.inventory.is_none() {
            self.inventory = Some(discover(&self.roots));
        }
        self.inventory.as_ref().expect("just set")
    }

    pub fn refresh_inventory(&mut self) -> &Inventory {
        self.inventory = None;
        self.inventory()
    }

    fn server(&self, kind: ServerKind) -> Option<&Server> {
        match kind {
            ServerKind::Chat => self.chat.as_ref(),
            ServerKind::Embed => self.embed.as_ref(),
        }
    }

    pub fn state_of(&self, kind: ServerKind) -> ServerState {
        match kind {
            ServerKind::Chat => self.chat_state,
            ServerKind::Embed => self.embed_state,
        }
    }

    pub fn set_state(&mut self, kind: ServerKind, s: ServerState) {
        match kind {
            ServerKind::Chat => self.chat_state = s,
            ServerKind::Embed => self.embed_state = s,
        }
    }

    pub fn port_of(&self, kind: ServerKind) -> Option<u16> {
        self.server(kind).map(|s| s.port)
    }

    pub fn model_id_of(&self, kind: ServerKind) -> Option<String> {
        self.server(kind).map(|s| s.model_id.clone())
    }

    /// What the `kind` server was started on, if it is running.
    pub fn resolved_of(&self, kind: ServerKind) -> Option<Resolved> {
        self.server(kind).map(|s| s.resolved.clone())
    }

    /// What the `kind` server's log says about where its model went.
    pub fn report_of(&self, kind: ServerKind) -> Option<LoadReport> {
        self.server(kind).map(Server::report)
    }

    pub fn log_tail_of(&self, kind: ServerKind, n: usize) -> Vec<String> {
        self.server(kind).map(|s| s.log_tail(n)).unwrap_or_default()
    }

    pub fn pid_of(&self, kind: ServerKind) -> Option<u32> {
        self.server(kind).map(Server::pid)
    }

    /// What the running `kind` server is, for the usage panel.
    pub fn usage_of(&self, kind: ServerKind, model: Option<String>) -> Option<crate::usage::ModelServer> {
        self.server(kind).map(|s| crate::usage::ModelServer {
            pid: s.pid(),
            model,
            resolved: s.resolved.clone(),
            report: s.report(),
            ctx: s.ctx,
        })
    }

    /// True when the server process for `kind` has exited (crashed or was
    /// killed) while we still hold it.
    pub fn exited(&mut self, kind: ServerKind) -> bool {
        let server = match kind {
            ServerKind::Chat => self.chat.as_mut(),
            ServerKind::Embed => self.embed.as_mut(),
        };
        server.is_some_and(Server::exited)
    }

    /// Stop a server (kills the process via Drop).
    pub fn stop(&mut self, kind: ServerKind) {
        match kind {
            ServerKind::Chat => self.chat = None,
            ServerKind::Embed => self.embed = None,
        }
        self.set_state(kind, ServerState::Stopped);
    }

    pub fn stop_all(&mut self) {
        self.stop(ServerKind::Chat);
        self.stop(ServerKind::Embed);
    }

    /// The `llama-server` of the build for `backend`, if it is installed.
    /// "CPU only" falls back to any installed build, which is then told not
    /// to offload.
    fn binary_for(&self, backend: Backend) -> Option<PathBuf> {
        find_binary(&self.roots, backend).or_else(|| {
            (backend == Backend::Cpu)
                .then(|| supported_backends().iter().find_map(|b| find_binary(&self.roots, *b)))
                .flatten()
        })
    }

    /// Start a llama-server process (not kept by the runtime): `model_path`
    /// on `resolved`, with `extra` arguments. Check `wait_healthy` on its
    /// port, then `Server::report` for where the model really went.
    pub fn start(
        &mut self,
        model_path: &Path,
        model_id: &str,
        ctx: u32,
        resolved: &Resolved,
        extra: &[&str],
    ) -> Result<Server, String> {
        let binary = self
            .binary_for(resolved.backend)
            .ok_or_else(|| format!("the {} engine is not installed", resolved.backend.label()))?;
        if !model_path.exists() {
            return Err(format!("model file missing: {}", model_path.display()));
        }
        let spec = inference::launch_spec(resolved);
        let port = free_port()?;
        let ctx = effective_ctx(ctx, resolved);

        let mut cmd = Command::new(&binary);
        cmd.arg("--host")
            .arg("127.0.0.1") // loopback only: never reachable from the network
            .arg("--port")
            .arg(port.to_string())
            .arg("--model")
            .arg(model_path)
            .arg("--ctx-size")
            .arg(ctx.to_string())
            .arg("--no-webui")
            // Detailed enough for the server to say which device it loaded
            // the model onto. It is read from the pipe, never written to disk.
            .arg("--log-verbosity")
            .arg("4")
            .args(&spec.args)
            .args(extra);
        for (name, value) in &spec.env {
            cmd.env(name, value);
        }
        if resolved.backend != Backend::OpenVino {
            // An inherited setting must not redirect another engine.
            cmd.env_remove("GGML_OPENVINO_DEVICE");
        }
        cmd.current_dir(scratch_dir());
        cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped());
        no_console_window(&mut cmd);

        let mut child = cmd
            .spawn()
            .map_err(|e| format!("failed to start llama-server: {e}"))?;
        // If Loft dies without cleaning up (crash, Task Manager), the OS
        // kills the server too instead of leaving a multi-GB orphan.
        kill_with_parent(&child);

        let log = Arc::new(Mutex::new(ServerLog::default()));
        if let Some(stderr) = child.stderr.take() {
            let log = log.clone();
            std::thread::spawn(move || {
                for line in BufReader::new(stderr).lines() {
                    let Ok(line) = line else { break };
                    let Ok(mut log) = log.lock() else { break };
                    log.report.read(&line);
                    if log.tail.len() == LOG_TAIL {
                        log.tail.pop_front();
                    }
                    log.tail.push_back(line);
                }
            });
        }
        Ok(Server {
            child,
            port,
            model_id: model_id.to_string(),
            resolved: resolved.clone(),
            ctx,
            log,
        })
    }

    /// Start (or replace) the server for `kind` on `resolved`. Returns the
    /// port. Sets state to Loading; the caller confirms Ready.
    pub fn spawn_on(
        &mut self,
        kind: ServerKind,
        model_path: &Path,
        model_id: &str,
        ctx: u32,
        resolved: &Resolved,
    ) -> Result<u16, String> {
        self.stop(kind); // replace any existing server of this kind
        let extra: &[&str] = if kind == ServerKind::Embed { &["--embeddings"] } else { &[] };
        let server = self.start(model_path, model_id, ctx, resolved, extra)?;
        let port = server.port;
        match kind {
            ServerKind::Chat => self.chat = Some(server),
            ServerKind::Embed => self.embed = Some(server),
        }
        self.set_state(kind, ServerState::Loading);
        Ok(port)
    }

    /// The first thing "automatic, for performance" would use.
    pub fn default_target(&mut self) -> Result<Resolved, String> {
        let inventory = self.inventory().clone();
        inference::auto_chain(&inventory, AutoGoal::Performance, &[])
            .iter()
            .find_map(|target| inference::resolve(&inventory, target).ok())
            .ok_or_else(|| "llama-server binary not found in binaries/".to_string())
    }

    /// Start the server for `kind` on the default target (tests and tools;
    /// the app chooses targets through `engine.rs`).
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn spawn(&mut self, kind: ServerKind, model_path: &Path, model_id: &str, ctx: u32) -> Result<u16, String> {
        let resolved = self.default_target()?;
        self.spawn_on(kind, model_path, model_id, ctx, &resolved)
    }

    /// Start a server for a picture (vision) model: the model plus its image
    /// projector, on the same target as the chat model (or the default).
    /// It is not kept or supervised here: the caller owns it, and dropping it
    /// stops it. Check `wait_healthy` on its port before use.
    pub fn spawn_vision(&mut self, model_path: &Path, mmproj_path: &Path, model_id: &str) -> Result<Server, String> {
        if !mmproj_path.exists() {
            return Err(format!("model file missing: {}", mmproj_path.display()));
        }
        // OpenVINO does not run multimodal models; use the default there.
        let resolved = match self.resolved_of(ServerKind::Chat) {
            Some(resolved) if resolved.backend != Backend::OpenVino => resolved,
            _ => self.default_target()?,
        };
        let mmproj = mmproj_path.to_string_lossy().into_owned();
        // Room for a large picture (a few thousand image tokens) and the reply.
        self.start(model_path, model_id, 8192, &resolved, &["--mmproj", &mmproj])
    }
}

/// The context size a server is really started with: the model's, kept to
/// a sane amount of memory, and to what the device can take.
pub fn effective_ctx(ctx: u32, resolved: &Resolved) -> u32 {
    ctx.clamp(512, 32768).min(inference::launch_spec(resolved).max_ctx.unwrap_or(u32::MAX))
}

#[cfg(target_os = "windows")]
fn peak_memory_mib(child: &Child) -> Option<u64> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::System::ProcessStatus::{K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};
    // SAFETY: `counters` is a valid, writable PROCESS_MEMORY_COUNTERS of the
    // stated size, and the handle is the live child's own.
    let mut counters: PROCESS_MEMORY_COUNTERS = unsafe { std::mem::zeroed() };
    counters.cb = std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32;
    let ok = unsafe { K32GetProcessMemoryInfo(child.as_raw_handle() as _, &mut counters, counters.cb) };
    (ok != 0).then(|| counters.PeakWorkingSetSize as u64 / (1024 * 1024))
}

#[cfg(target_os = "linux")]
fn peak_memory_mib(child: &Child) -> Option<u64> {
    // "VmHWM:   1234567 kB": the peak resident set size.
    let status = std::fs::read_to_string(format!("/proc/{}/status", child.id())).ok()?;
    let line = status.lines().find(|l| l.starts_with("VmHWM:"))?;
    line.split_whitespace().nth(1)?.parse::<u64>().ok().map(|kb| kb / 1024)
}

#[cfg(not(any(target_os = "windows", target_os = "linux")))]
fn peak_memory_mib(_child: &Child) -> Option<u64> {
    None
}

/// The folder model servers are started in. Engines write stray files to
/// their working directory (Intel's graphics compiler, used by OpenVINO,
/// leaves a `kernel.errors.txt` there), and that must not be Loft's install
/// folder or whatever folder Loft happened to be launched from.
fn scratch_dir() -> PathBuf {
    let dir = std::env::temp_dir().join("loft-engine-work");
    match std::fs::create_dir_all(&dir) {
        Ok(()) => dir,
        Err(_) => std::env::temp_dir(),
    }
}

/// One `/health` probe: true once the model has loaded.
pub async fn is_healthy(port: u16) -> bool {
    reqwest::Client::new()
        .get(format!("http://127.0.0.1:{port}/health"))
        .timeout(Duration::from_secs(3))
        .send()
        .await
        .map(|resp| resp.status().is_success())
        .unwrap_or(false)
}

/// Poll `/health` until the server reports ready (200) or we give up.
#[cfg_attr(not(test), allow(dead_code))] // the app uses its own loop that also watches for exit
pub async fn wait_healthy(port: u16, timeout: Duration) -> bool {
    let deadline = tokio::time::Instant::now() + timeout;
    while tokio::time::Instant::now() < deadline {
        if is_healthy(port).await {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    false
}

/// Bind to port 0 to get a free localhost port, then release it.
fn free_port() -> Result<u16, String> {
    let listener =
        TcpListener::bind("127.0.0.1:0").map_err(|e| format!("no free port: {e}"))?;
    let port = listener
        .local_addr()
        .map_err(|e| format!("no local addr: {e}"))?
        .port();
    Ok(port)
}

/// Detect the current platform folder name under `binaries/`.
pub fn platform_dir() -> &'static str {
    #[cfg(all(target_os = "windows", target_arch = "aarch64"))]
    {
        "win-arm64"
    }
    #[cfg(all(target_os = "windows", not(target_arch = "aarch64")))]
    {
        "win-x64"
    }
    #[cfg(target_os = "macos")]
    {
        #[cfg(target_arch = "aarch64")]
        {
            "mac-arm64"
        }
        #[cfg(not(target_arch = "aarch64"))]
        {
            "mac-x64"
        }
    }
    #[cfg(all(target_os = "linux", target_arch = "aarch64"))]
    {
        "linux-arm64"
    }
    #[cfg(all(target_os = "linux", not(target_arch = "aarch64")))]
    {
        "linux-x64"
    }
}

pub fn server_exe_name() -> &'static str {
    if cfg!(target_os = "windows") {
        "llama-server.exe"
    } else {
        "llama-server"
    }
}

// ------------------------------ discovery -----------------------------------

/// The backends that can exist on this operating system, in display order.
pub fn supported_backends() -> &'static [Backend] {
    if cfg!(target_os = "macos") {
        &[Backend::Metal, Backend::Cpu]
    } else {
        &[Backend::Cuda, Backend::Vulkan, Backend::OpenVino, Backend::Cpu]
    }
}

/// `<root>/<platform>-<backend>/llama-server`, in the first root that has it.
fn find_binary(roots: &[PathBuf], backend: Backend) -> Option<PathBuf> {
    let sub = format!("{}-{}", platform_dir(), backend.dir_suffix());
    roots
        .iter()
        .map(|root| root.join(&sub).join(server_exe_name()))
        .find(|candidate| candidate.exists())
}

/// Run `llama-server --list-devices` for one build, giving up after a while
/// (a broken driver can make it hang).
fn list_devices(binary: &Path, openvino_device: Option<&str>) -> Result<String, String> {
    let mut cmd = Command::new(binary);
    cmd.arg("--list-devices");
    match openvino_device {
        Some(device) => cmd.env("GGML_OPENVINO_DEVICE", device),
        None => cmd.env_remove("GGML_OPENVINO_DEVICE"), // an inherited one could abort the listing
    };
    cmd.current_dir(scratch_dir())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    no_console_window(&mut cmd);
    let child = cmd.spawn().map_err(|e| format!("could not start it: {e}"))?;
    let (tx, rx) = std::sync::mpsc::channel();
    let id = child.id();
    std::thread::spawn(move || {
        let _ = tx.send(child.wait_with_output());
    });
    match rx.recv_timeout(Duration::from_secs(20)) {
        Ok(Ok(out)) => Ok(format!(
            "{}\n{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        )),
        Ok(Err(e)) => Err(format!("it did not run: {e}")),
        Err(_) => {
            kill_process(id);
            Err("it did not answer in time".to_string())
        }
    }
}

/// The devices the OpenVINO build can use.
///
/// The documented way is `--list-devices`, and it is tried first. The
/// b12110 build for Windows prints nothing for it, so failing that the
/// engine is asked for a device that cannot exist: it refuses, and the
/// refusal names the devices it does have. That refusal is an abort, which
/// Windows records as a crash of `llama-server`, so the answer is kept
/// beside the engine and asked for again only when the engine or the
/// computer's devices and drivers have changed.
fn openvino_devices(binary: &Path, hardware: &[Hardware]) -> Result<Vec<inference::Device>, String> {
    if let Ok(output) = list_devices(binary, None) {
        let devices = inference::parse_devices(Backend::OpenVino, &output);
        if !devices.is_empty() {
            return Ok(devices);
        }
    }
    let stamp = std::fs::metadata(binary)
        .ok()
        .map(|m| (m.len(), m.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs()).unwrap_or(0)));
    let signature = format!(
        "{stamp:?}|{}",
        hardware.iter().map(|h| format!("{}={}", h.name, h.driver.as_deref().unwrap_or("?"))).collect::<Vec<_>>().join(";")
    );
    let cache = binary.with_file_name("loft-devices.json");
    let cached = std::fs::read_to_string(&cache)
        .ok()
        .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
        .filter(|saved| saved["signature"].as_str() == Some(signature.as_str()))
        .and_then(|saved| serde_json::from_value::<Vec<(String, String)>>(saved["devices"].clone()).ok())
        .filter(|pairs| !pairs.is_empty());
    let pairs = match cached {
        Some(pairs) => pairs,
        None => {
            let pairs = inference::parse_openvino_refusal(&list_devices(binary, Some("LOFT_PROBE"))?);
            if !pairs.is_empty() {
                // Best effort: an engine folder that cannot be written to is asked each time.
                let _ = std::fs::write(&cache, serde_json::json!({ "signature": signature, "devices": pairs }).to_string());
            }
            pairs
        }
    };
    Ok(inference::openvino_devices(&pairs))
}

/// What one backend can do on this machine.
fn probe(roots: &[PathBuf], backend: Backend, cpu: &str, hardware: &[Hardware]) -> BackendInfo {
    let label = backend.label();
    let info = |status: BackendStatus, devices, detail: Option<String>| BackendInfo {
        backend,
        status,
        experimental: backend.experimental(),
        devices,
        detail,
    };
    // What the operating system says is there. On systems where that list is
    // not available (it is empty), nothing is ruled out on its account.
    let known = !hardware.is_empty();
    let has = |vendor: Vendor, kind: DeviceKind| hardware.iter().any(|h| h.vendor == vendor && h.kind == kind);
    let hardware_problem: Option<(BackendStatus, String)> = match backend {
        Backend::Cuda if known && !has(Vendor::Nvidia, DeviceKind::Gpu) => Some((
            BackendStatus::HardwareUnavailable,
            "CUDA needs an NVIDIA graphics card, and this computer has none.".into(),
        )),
        Backend::Cuda if !hardware::has_library("nvcuda.dll", "libcuda.so.1") => Some((
            BackendStatus::DriverMissing,
            "The NVIDIA graphics driver is not installed (or is too old to include CUDA).".into(),
        )),
        Backend::Vulkan if !hardware::has_library("vulkan-1.dll", "libvulkan.so.1") => Some((
            BackendStatus::DriverMissing,
            "No Vulkan runtime was found; it comes with current graphics drivers.".into(),
        )),
        Backend::OpenVino if known && !cpu.contains("Intel") && !hardware.iter().any(|h| h.vendor == Vendor::Intel) => Some((
            BackendStatus::HardwareUnavailable,
            "OpenVINO runs on Intel processors, graphics and NPUs, and this computer has none.".into(),
        )),
        _ => None,
    };

    let Some(binary) = find_binary(roots, backend) else {
        return match hardware_problem {
            Some((status, detail)) => info(status, Vec::new(), Some(detail)),
            None => info(BackendStatus::NotInstalled, Vec::new(), Some(format!("The {label} engine is an optional download."))),
        };
    };
    if let Some((status, detail)) = hardware_problem {
        return info(status, Vec::new(), Some(detail));
    }
    if backend == Backend::Cpu || backend == Backend::Metal {
        return info(BackendStatus::Available, Vec::new(), None);
    }
    let listed = if backend == Backend::OpenVino {
        openvino_devices(&binary, hardware)
    } else {
        list_devices(&binary, None).map(|output| inference::parse_devices(backend, &output))
    };
    match listed {
        Ok(devices) => {
            if devices.is_empty() {
                let detail = format!("The {label} engine is installed but found no device it can use.");
                info(BackendStatus::HardwareUnavailable, devices, Some(detail))
            } else {
                let detail = match backend {
                    Backend::OpenVino if !devices.iter().any(|d| d.kind == DeviceKind::Npu) => {
                        Some("No NPU was found by OpenVINO (it needs an Intel Core Ultra processor and the Intel NPU driver).".to_string())
                    }
                    _ => None,
                };
                info(BackendStatus::Available, devices, detail)
            }
        }
        Err(reason) => info(BackendStatus::Failed, Vec::new(), Some(format!("The {label} engine could not list its devices: {reason}."))),
    }
}

/// Find every engine build and ask each what it can use. Starts each
/// installed GPU build once, briefly.
pub fn discover(roots: &[PathBuf]) -> Inventory {
    let cpu = hardware::cpu_name();
    let hardware = hardware::devices();
    let backends = supported_backends().iter().map(|b| probe(roots, *b, &cpu, &hardware)).collect();
    Inventory {
        cpu,
        hardware,
        backends,
        revision: LLAMA_REVISION.to_string(),
    }
}

#[cfg(target_os = "windows")]
fn kill_process(pid: u32) {
    let mut cmd = Command::new("taskkill");
    cmd.args(["/PID", &pid.to_string(), "/F"]).stdout(Stdio::null()).stderr(Stdio::null());
    no_console_window(&mut cmd);
    let _ = cmd.status();
}

#[cfg(not(target_os = "windows"))]
fn kill_process(pid: u32) {
    let _ = Command::new("kill").args(["-9", &pid.to_string()]).status();
}

/// Directories that may contain the `binaries/` folder: next to the executable
/// (bundled app) and the crate's source dir (during `tauri dev`).
fn binary_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            roots.push(dir.join("binaries"));
            // Tauri resource layout: resources/binaries
            roots.push(dir.join("resources").join("binaries"));
        }
    }
    // Dev fallback: <crate>/binaries
    roots.push(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("binaries"));
    roots
}

#[cfg(target_os = "windows")]
fn no_console_window(cmd: &mut Command) {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    cmd.creation_flags(CREATE_NO_WINDOW);
}

#[cfg(not(target_os = "windows"))]
fn no_console_window(_cmd: &mut Command) {}

/// Windows: put the child in a job object that kills its processes when the
/// last handle closes, i.e. when Loft exits for any reason.
#[cfg(target_os = "windows")]
fn kill_with_parent(child: &Child) {
    use std::os::windows::io::AsRawHandle;
    use std::sync::OnceLock;
    use windows_sys::Win32::Foundation::HANDLE;
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
        SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    };

    struct Job(HANDLE);
    // A job handle is just a reference to a kernel object; safe to share.
    unsafe impl Send for Job {}
    unsafe impl Sync for Job {}
    static JOB: OnceLock<Option<Job>> = OnceLock::new();

    let job = JOB.get_or_init(|| unsafe {
        let handle = CreateJobObjectW(std::ptr::null(), std::ptr::null());
        if handle.is_null() {
            return None;
        }
        let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        let ok = SetInformationJobObject(
            handle,
            JobObjectExtendedLimitInformation,
            &info as *const _ as *const core::ffi::c_void,
            std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        );
        // The handle is deliberately never closed: it lives as long as Loft.
        (ok != 0).then_some(Job(handle))
    });
    if let Some(job) = job {
        unsafe {
            AssignProcessToJobObject(job.0, child.as_raw_handle() as HANDLE);
        }
    }
}

#[cfg(not(target_os = "windows"))]
fn kill_with_parent(_child: &Child) {}

// ---------------------------------------------------------------------------
// Tests. The ignored ones start real servers against the downloaded models:
//   cargo test --lib runtime:: -- --ignored --nocapture
// ---------------------------------------------------------------------------
#[cfg(test)]
mod tests {
    use super::*;
    use crate::inference::{resolve, verify, Target};

    fn dirs_models() -> PathBuf {
        // Mirror the app's APPDATA/com.loft.desktop/models location on Windows.
        if cfg!(target_os = "windows") {
            if let Ok(appdata) = std::env::var("APPDATA") {
                return PathBuf::from(appdata).join("com.loft.desktop").join("models");
            }
        }
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("models")
    }

    /// Prints what is installed and usable here. The CPU build is part of the
    /// normal setup, so it must be found.
    #[test]
    fn engines_on_this_computer_are_discovered() {
        let mut rt = Runtime::new();
        let inventory = rt.inventory().clone();
        eprintln!("cpu: {}", inventory.cpu);
        for h in &inventory.hardware {
            eprintln!("hardware: {:?} {} (driver {:?})", h.kind, h.name, h.driver);
        }
        for b in &inventory.backends {
            eprintln!("{:9} {:?} {}", b.backend.label(), b.status, b.detail.as_deref().unwrap_or(""));
            for d in &b.devices {
                eprintln!("            {} = {} [{}] discrete={} {:?} MiB", d.runtime_id, d.name, d.key, d.discrete, d.memory_mib);
            }
        }
        assert_eq!(inventory.revision, LLAMA_REVISION);
        // The standard engine is CPU on Windows and Linux, Metal on Apple
        // silicon. Skip only when nothing at all has been fetched.
        let installed: Vec<Backend> = supported_backends().iter().copied().filter(|b| find_binary(&rt.roots, *b).is_some()).collect();
        if installed.is_empty() {
            eprintln!("skip: no llama.cpp builds here (run `npm run fetch-llama`)");
            return;
        }
        for backend in &installed {
            let info = inventory.backend(*backend).unwrap_or_else(|| panic!("{} is listed", backend.label()));
            assert_eq!(info.status, BackendStatus::Available, "{}", backend.label());
        }
        // Whatever is available can be resolved, and something is the default.
        let default = rt.default_target().expect("a default target");
        eprintln!("default target: {}", default.label());
        // Asking again does not start the builds again.
        assert_eq!(rt.inventory(), &inventory);
    }

    /// Starts the embedding model on every available device in turn and
    /// checks, from the server's own log, that the model went where it was
    /// sent: each GPU, and nowhere but system memory for "CPU only".
    #[tokio::test]
    #[ignore = "requires a downloaded model + bundled binaries"]
    async fn each_target_runs_where_it_is_told() {
        let model = dirs_models().join("nomic-embed-text-v1.5.f16.gguf");
        if !model.exists() {
            eprintln!("skip: model not found at {}", model.display());
            return;
        }
        let mut rt = Runtime::new();
        let inventory = rt.inventory().clone();
        let mut targets = vec![Target::cpu()];
        for backend in &inventory.backends {
            if backend.status == BackendStatus::Available {
                targets.extend(backend.devices.iter().map(|d| Target::new(backend.backend, Some(&d.key))));
            }
        }
        let mut checked = 0;
        for target in targets {
            let resolved = resolve(&inventory, &target).expect("an available target resolves");
            let started = std::time::Instant::now();
            let port = rt
                .spawn_on(ServerKind::Embed, &model, "nomic-embed-text-v1.5", 2048, &resolved)
                .expect("server starts");
            let healthy = wait_healthy(port, Duration::from_secs(90)).await;
            let report = rt.report_of(ServerKind::Embed).unwrap();
            let verdict = verify(&resolved, &report);
            eprintln!(
                "{:55} healthy={healthy} verified={} in {:.1}s | {}",
                resolved.label(),
                verdict.ok,
                started.elapsed().as_secs_f32(),
                verdict.detail
            );
            if !healthy {
                eprintln!("   last log lines: {:#?}", rt.log_tail_of(ServerKind::Embed, 6));
            }
            // Experimental engines may refuse a model; the others must work.
            if !target.backend.experimental() {
                assert!(healthy, "{} did not start", resolved.label());
                assert!(verdict.ok, "{}: {}", resolved.label(), verdict.detail);
                // And it is not mistaken for any other target.
                let cpu = resolve(&inventory, &Target::cpu()).unwrap();
                if target != Target::cpu() {
                    assert!(!verify(&cpu, &report).ok, "a GPU load must not pass for CPU only");
                }
                // The server really answers.
                let vector = crate::embed::embed_one(port, "hello world").await.expect("an embedding");
                assert!(!vector.is_empty());
                checked += 1;
            }
            rt.stop(ServerKind::Embed);
        }
        assert!(checked >= 1);
    }

    #[tokio::test]
    #[ignore = "requires a downloaded model + bundled binary"]
    async fn embed_server_roundtrip() {
        let model = dirs_models().join("nomic-embed-text-v1.5.f16.gguf");
        if !model.exists() {
            eprintln!("skip: model not found at {}", model.display());
            return;
        }
        let mut rt = Runtime::new();
        let port = rt
            .spawn(ServerKind::Embed, &model, "nomic-embed-text-v1.5", 2048)
            .expect("spawn embed server");
        assert!(
            wait_healthy(port, Duration::from_secs(60)).await,
            "server became healthy"
        );

        let client = reqwest::Client::new();
        let body = serde_json::json!({ "input": "hello world" });
        let resp = client
            .post(format!("http://127.0.0.1:{port}/v1/embeddings"))
            .json(&body)
            .send()
            .await
            .expect("embeddings request");
        assert!(resp.status().is_success(), "embeddings ok: {}", resp.status());
        let v: serde_json::Value = resp.json().await.expect("json");
        let dims = v["data"][0]["embedding"].as_array().map(|a| a.len()).unwrap_or(0);
        eprintln!("embedding dims = {dims}");
        assert!(dims > 0, "got a non-empty embedding vector");
    }
}
