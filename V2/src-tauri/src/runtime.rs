//! M1 — llama.cpp runtime manager.
//!
//! Locates the bundled `llama-server` binary + accelerator, spawns and
//! supervises the chat and embedding servers (each a separate process, since
//! embedding models are pooling-based and don't share a process with chat),
//! health-checks them over localhost, and tears them down on exit.

use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

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

/// A running llama-server process. Killed on drop.
pub struct Server {
    child: Child,
    pub port: u16,
    pub model_id: String,
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

pub struct Runtime {
    /// Directories searched for `<platform>-<accel>/llama-server`.
    roots: Vec<PathBuf>,
    pub binary: Option<PathBuf>,
    pub accelerator: Accelerator,
    pub chat: Option<Server>,
    pub embed: Option<Server>,
    pub chat_state: ServerState,
    pub embed_state: ServerState,
    pub message: Option<String>,
    /// GPU device to offload to (`--device`), detected on first GPU spawn.
    /// `Some(None)` = detected, let llama.cpp choose.
    device: Option<Option<String>>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Accelerator {
    Cpu,
    Metal,
    Cuda,
    Vulkan,
}

impl Accelerator {
    pub fn as_str(self) -> &'static str {
        match self {
            Accelerator::Cpu => "cpu",
            Accelerator::Metal => "metal",
            Accelerator::Cuda => "cuda",
            Accelerator::Vulkan => "vulkan",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Accelerator::Cpu => "CPU",
            Accelerator::Metal => "Metal",
            Accelerator::Cuda => "CUDA",
            Accelerator::Vulkan => "Vulkan",
        }
    }
}

/// One line of `llama-server --list-devices`.
#[derive(Debug, Clone, PartialEq)]
struct GpuDevice {
    id: String,
    name: String,
    total_mib: u64,
}

/// Parse lines such as
/// `  Vulkan1: NVIDIA GeForce RTX 4050 Laptop GPU (5920 MiB, 5152 MiB free)`.
fn parse_devices(output: &str) -> Vec<GpuDevice> {
    output
        .lines()
        .filter_map(|line| {
            let (id, rest) = line.trim().split_once(": ")?;
            let valid_id = !id.is_empty()
                && id.chars().all(|c| c.is_ascii_alphanumeric())
                && id.ends_with(|c: char| c.is_ascii_digit());
            if !valid_id || !rest.ends_with("free)") {
                return None;
            }
            let (name, mem) = rest.rsplit_once(" (")?;
            let total_mib = mem.split_whitespace().next()?.parse().ok()?;
            Some(GpuDevice {
                id: id.to_string(),
                name: name.trim().to_string(),
                total_mib,
            })
        })
        .collect()
}

fn is_discrete(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    ["nvidia", "geforce", "rtx", "gtx", "quadro", "radeon rx", "radeon pro", "intel(r) arc", "intel arc"]
        .iter()
        .any(|k| n.contains(k))
}

/// The device to pass to `--device`: the discrete GPU with the most memory,
/// when there is a choice to make. `None` leaves it to llama.cpp (a single
/// GPU, or nothing recognisably discrete).
fn pick_device(devices: &[GpuDevice]) -> Option<String> {
    if devices.len() < 2 {
        return None;
    }
    devices
        .iter()
        .filter(|d| is_discrete(&d.name))
        .max_by_key(|d| d.total_mib)
        .map(|d| d.id.clone())
}

/// Ask the GPU build which devices it can see and choose one.
fn detect_device(binary: &Path) -> Option<String> {
    let mut cmd = Command::new(binary);
    cmd.arg("--list-devices").stdin(Stdio::null());
    no_console_window(&mut cmd);
    let out = cmd.output().ok()?;
    let text = format!(
        "{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    pick_device(&parse_devices(&text))
}

impl Runtime {
    #[cfg(test)]
    pub fn new() -> Self {
        Self::with_extra_root(None)
    }

    /// `extra_root` is the bundled app's `binaries/` resource directory.
    pub fn with_extra_root(extra_root: Option<PathBuf>) -> Self {
        let mut roots: Vec<PathBuf> = extra_root.into_iter().collect();
        roots.extend(binary_roots());
        let (binary, accelerator) = resolve_binary(&roots, true);
        let message = if binary.is_none() {
            Some("llama-server binary not found in binaries/.".to_string())
        } else {
            None
        };
        Runtime {
            roots,
            binary,
            accelerator,
            chat: None,
            embed: None,
            chat_state: ServerState::Stopped,
            embed_state: ServerState::Stopped,
            message,
            device: None,
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
        match kind {
            ServerKind::Chat => self.chat.as_ref().map(|s| s.port),
            ServerKind::Embed => self.embed.as_ref().map(|s| s.port),
        }
    }

    pub fn model_id_of(&self, kind: ServerKind) -> Option<String> {
        match kind {
            ServerKind::Chat => self.chat.as_ref().map(|s| s.model_id.clone()),
            ServerKind::Embed => self.embed.as_ref().map(|s| s.model_id.clone()),
        }
    }

    /// True when the server process for `kind` has exited (crashed or was
    /// killed) while we still hold it.
    pub fn exited(&mut self, kind: ServerKind) -> bool {
        let server = match kind {
            ServerKind::Chat => self.chat.as_mut(),
            ServerKind::Embed => self.embed.as_mut(),
        };
        match server {
            Some(s) => !matches!(s.child.try_wait(), Ok(None)),
            None => false,
        }
    }

    /// Switch from a GPU build to the CPU build (after a GPU start failed).
    /// Returns false when already on CPU or no CPU build is present.
    pub fn fall_back_to_cpu(&mut self) -> bool {
        if self.accelerator == Accelerator::Cpu {
            return false;
        }
        match resolve_binary(&self.roots, false) {
            (Some(binary), accel) => {
                self.message = Some(format!(
                    "{} acceleration failed to start, so Loft is running on the CPU.",
                    self.accelerator.label()
                ));
                self.binary = Some(binary);
                self.accelerator = accel;
                true
            }
            _ => false,
        }
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

    /// The command line every server shares: address, model, context size
    /// and, on a GPU build, which GPU to use.
    fn base_command(&mut self, binary: &Path, port: u16, model_path: &Path, ctx: u32) -> Command {
        let ctx = ctx.clamp(512, 32768); // keep memory sane on CPU
        let mut cmd = Command::new(binary);
        cmd.arg("--host")
            .arg("127.0.0.1")
            .arg("--port")
            .arg(port.to_string())
            .arg("--model")
            .arg(model_path)
            .arg("--ctx-size")
            .arg(ctx.to_string())
            .arg("--no-webui");

        // GPU builds decide by themselves how many layers fit in free VRAM
        // (llama.cpp's default), so a small GPU still helps instead of failing.
        // We only say *which* GPU: on a laptop with an integrated and a
        // discrete GPU, llama.cpp would otherwise split the model across both.
        if matches!(self.accelerator, Accelerator::Vulkan | Accelerator::Cuda) {
            let device = self
                .device
                .get_or_insert_with(|| detect_device(binary))
                .clone();
            if let Some(device) = device {
                cmd.arg("--device").arg(device);
            }
        }
        cmd
    }

    /// Start a server for a picture (vision) model: the model plus its image
    /// projector. Unlike the chat and embedding servers it is not kept or
    /// supervised here: the caller owns it, and dropping it stops it. Check
    /// `wait_healthy` on its port before use.
    pub fn spawn_vision(&mut self, model_path: &Path, mmproj_path: &Path, model_id: &str) -> Result<Server, String> {
        let binary = self
            .binary
            .clone()
            .ok_or_else(|| "llama-server binary not found".to_string())?;
        for file in [model_path, mmproj_path] {
            if !file.exists() {
                return Err(format!("model file missing: {}", file.display()));
            }
        }
        let port = free_port()?;
        // Room for a large picture (a few thousand image tokens) and the reply.
        let mut cmd = self.base_command(&binary, port, model_path, 8192);
        cmd.arg("--mmproj").arg(mmproj_path);
        cmd.stdout(Stdio::null()).stderr(Stdio::null());
        no_console_window(&mut cmd);
        let child = cmd
            .spawn()
            .map_err(|e| format!("failed to spawn llama-server: {e}"))?;
        kill_with_parent(&child);
        Ok(Server {
            child,
            port,
            model_id: model_id.to_string(),
        })
    }

    /// Spawn a llama-server for `kind` against `model_path`. Returns the port.
    /// Sets state to Loading; the caller awaits `wait_healthy` to confirm Ready.
    pub fn spawn(
        &mut self,
        kind: ServerKind,
        model_path: &Path,
        model_id: &str,
        ctx: u32,
    ) -> Result<u16, String> {
        let binary = self
            .binary
            .clone()
            .ok_or_else(|| "llama-server binary not found".to_string())?;
        if !model_path.exists() {
            return Err(format!("model file missing: {}", model_path.display()));
        }

        // Replace any existing server of this kind.
        self.stop(kind);

        let port = free_port()?;
        let mut cmd = self.base_command(&binary, port, model_path, ctx);
        if kind == ServerKind::Embed {
            cmd.arg("--embeddings");
        }

        // Quiet: llama-server is chatty; we only want the API.
        cmd.stdout(Stdio::null()).stderr(Stdio::null());
        no_console_window(&mut cmd);

        let child = cmd
            .spawn()
            .map_err(|e| format!("failed to spawn llama-server: {e}"))?;
        // If Loft dies without cleaning up (crash, Task Manager), the OS
        // kills the server too instead of leaving a multi-GB orphan.
        kill_with_parent(&child);

        let server = Server {
            child,
            port,
            model_id: model_id.to_string(),
        };
        match kind {
            ServerKind::Chat => self.chat = Some(server),
            ServerKind::Embed => self.embed = Some(server),
        }
        self.set_state(kind, ServerState::Loading);
        Ok(port)
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
fn platform_dir() -> &'static str {
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

fn server_exe_name() -> &'static str {
    if cfg!(target_os = "windows") {
        "llama-server.exe"
    } else {
        "llama-server"
    }
}

/// Find the best usable `llama-server` binary and the accelerator it implies.
/// Looks under each root for `<platform>-<accel>/llama-server`, preferring a
/// GPU build that is both shipped and supported by this machine, then CPU.
fn resolve_binary(roots: &[PathBuf], allow_gpu: bool) -> (Option<PathBuf>, Accelerator) {
    let candidates: &[Accelerator] = if cfg!(target_os = "macos") {
        &[Accelerator::Metal, Accelerator::Cpu]
    } else {
        &[Accelerator::Cuda, Accelerator::Vulkan, Accelerator::Cpu]
    };

    for accel in candidates {
        if *accel != Accelerator::Cpu && !(allow_gpu && accel_available(*accel)) {
            continue;
        }
        let sub = format!("{}-{}", platform_dir(), accel.as_str());
        for root in roots {
            let candidate = root.join(&sub).join(server_exe_name());
            if candidate.exists() {
                return (Some(candidate), *accel);
            }
        }
    }
    (None, Accelerator::Cpu)
}

/// Cheap check that this machine has the driver a GPU build needs. It only
/// rules out the obvious "no such GPU" case; a GPU build that still fails to
/// start (too little VRAM, broken driver) is handled by `fall_back_to_cpu`.
fn accel_available(accel: Accelerator) -> bool {
    #[cfg(target_os = "windows")]
    fn has_driver(windows_dll: &str, _unix_lib: &str) -> bool {
        let root = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into());
        PathBuf::from(root).join("System32").join(windows_dll).exists()
    }
    #[cfg(not(target_os = "windows"))]
    fn has_driver(_windows_dll: &str, unix_lib: &str) -> bool {
        ["/usr/lib/x86_64-linux-gnu", "/usr/lib/aarch64-linux-gnu", "/usr/lib64", "/usr/lib"]
            .iter()
            .any(|dir| PathBuf::from(dir).join(unix_lib).exists())
    }
    match accel {
        Accelerator::Cpu => true,
        Accelerator::Metal => cfg!(target_os = "macos"),
        Accelerator::Vulkan => has_driver("vulkan-1.dll", "libvulkan.so.1"),
        Accelerator::Cuda => has_driver("nvcuda.dll", "libcuda.so.1"),
    }
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
// Integration test: spawn a real embedding server against the Nomic model (if
// present in APPDATA) and confirm /health + /v1/embeddings work. Run with:
//   cargo test -- --ignored --nocapture
// ---------------------------------------------------------------------------
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picks_the_discrete_gpu() {
        let laptop = "load_backend: loaded Vulkan backend\nAvailable devices:\n  \
            Vulkan0: Intel(R) Iris(R) Xe Graphics (16193 MiB, 15425 MiB free)\n  \
            Vulkan1: NVIDIA GeForce RTX 4050 Laptop GPU (5920 MiB, 5152 MiB free)\n";
        let devices = parse_devices(laptop);
        assert_eq!(devices.len(), 2);
        assert_eq!(devices[1].name, "NVIDIA GeForce RTX 4050 Laptop GPU");
        assert_eq!(devices[1].total_mib, 5920);
        assert_eq!(pick_device(&devices), Some("Vulkan1".into()));

        // Two discrete cards: the bigger one.
        let two = parse_devices(
            "  CUDA0: NVIDIA GeForce GTX 1650 (4096 MiB, 3900 MiB free)\n  CUDA1: NVIDIA RTX A4000 (16376 MiB, 16000 MiB free)\n",
        );
        assert_eq!(pick_device(&two), Some("CUDA1".into()));

        // One GPU, or only integrated ones: leave it to llama.cpp.
        let single = parse_devices("  Vulkan0: AMD Radeon(TM) Graphics (8000 MiB, 7000 MiB free)\n");
        assert_eq!(single.len(), 1);
        assert_eq!(pick_device(&single), None);
        let integrated = parse_devices(
            "  Vulkan0: Intel(R) UHD Graphics (8000 MiB, 7000 MiB free)\n  Vulkan1: AMD Radeon(TM) Graphics (8000 MiB, 7000 MiB free)\n",
        );
        assert_eq!(pick_device(&integrated), None);

        // Log noise is not a device.
        assert!(parse_devices("error: something (12 MiB, 3 MiB free)\nAvailable devices:\n").is_empty());
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
        assert!(rt.binary.is_some(), "llama-server binary should resolve");
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

    fn dirs_models() -> PathBuf {
        // Mirror the app's APPDATA/com.loft.desktop/models location on Windows.
        if cfg!(target_os = "windows") {
            if let Ok(appdata) = std::env::var("APPDATA") {
                return PathBuf::from(appdata).join("com.loft.desktop").join("models");
            }
        }
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("models")
    }
}
