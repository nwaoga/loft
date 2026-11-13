//! Optional engines: download a llama.cpp build and install it.
//!
//! CUDA (about 600 MB) and OpenVINO (about 100 MB) are not bundled. They can
//! be added from Settings, and land in the app-data folder `engines/`, which
//! `runtime.rs` searches before the bundled builds.
//!
//! What can be installed is fixed at build time: `engines.json` pins one
//! llama.cpp release and, for every archive in it, the size and SHA-256 that
//! GitHub publishes. The frontend only ever names a backend. It never supplies
//! a URL or a path, and an archive is used only if
//!
//! * its URL is under the pinned release of the pinned repository,
//! * the download has exactly the pinned size and checksum, and
//! * `llama-server` is in it.
//!
//! Only files a server needs are unpacked (the same rule as
//! `scripts/fetch-llama.mjs`), by file name alone, so nothing in an archive
//! can be written outside the engine's own folder. The new engine replaces an
//! old one only once it is complete.
//!
//! Free of Tauri types, so it is tested directly (against a local server).

use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::inference::Backend;

/// One file of a build, as published.
#[derive(Deserialize, Clone, Debug, PartialEq)]
pub struct Archive {
    pub name: String,
    pub size: u64,
    pub sha256: String,
    pub url: String,
}

/// One engine build for one platform.
#[derive(Deserialize, Clone, Debug, PartialEq)]
pub struct Build {
    /// Not part of the standard set: installed on request.
    #[serde(default)]
    pub optional: bool,
    pub archives: Vec<Archive>,
    #[serde(default)]
    pub note: Option<String>,
}

impl Build {
    pub fn size(&self) -> u64 {
        self.archives.iter().map(|a| a.size).sum()
    }
}

/// `engines.json`: the builds of one pinned llama.cpp release.
#[derive(Deserialize, Debug)]
pub struct Manifest {
    pub repo: String,
    pub tag: String,
    /// platform ("win-x64") -> backend ("cuda") -> build
    pub builds: HashMap<String, HashMap<String, Build>>,
}

impl Manifest {
    pub fn build(&self, platform: &str, backend: Backend) -> Option<&Build> {
        self.builds.get(platform)?.get(backend.dir_suffix())
    }

    /// True only for an archive of the pinned release of the pinned
    /// repository, named plainly, with a full SHA-256.
    pub fn trusted(&self, archive: &Archive) -> bool {
        let prefix = format!("https://github.com/{}/releases/download/{}/", self.repo, self.tag);
        let plain_name = !archive.name.is_empty()
            && archive.name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
            && !archive.name.starts_with('.');
        plain_name
            && archive.url.strip_prefix(&prefix) == Some(archive.name.as_str())
            && archive.sha256.len() == 64
            && archive.sha256.bytes().all(|b| b.is_ascii_hexdigit())
            && archive.size > 0
    }
}

/// The manifest built into the app.
pub fn manifest() -> &'static Manifest {
    static MANIFEST: OnceLock<Manifest> = OnceLock::new();
    MANIFEST.get_or_init(|| serde_json::from_str(include_str!("../engines.json")).expect("engines.json is valid"))
}

/// Where an install has got to.
#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Stage {
    Downloading,
    Unpacking,
    Installing,
}

/// Keep the server, what it loads, and the licences; drop the other tools.
pub fn keep(name: &str, server_name: &str) -> bool {
    if name == server_name || name.to_ascii_uppercase().starts_with("LICENSE") {
        return true;
    }
    let lower = name.to_ascii_lowercase();
    let is_library = lower.ends_with(".dll") || lower.ends_with(".dylib") || is_shared_object(&lower);
    // Data files a backend's runtime reads at start-up (OpenVINO plugins).
    let is_data = [".xml", ".json", ".bin", ".blob", ".mvcmd", ".elf"].iter().any(|ext| lower.ends_with(ext));
    if !is_library && !is_data {
        return false; // every other file is another tool's executable
    }
    // "<tool>-impl" libraries belong to the other command-line tools.
    !lower.contains("-impl.") || lower.contains("llama-server-impl")
}

/// `libx.so`, `libx.so.1`, `libx.so.1.2.3`.
fn is_shared_object(name: &str) -> bool {
    let Some(at) = name.rfind(".so") else {
        return false;
    };
    let rest = &name[at + 3..];
    rest.is_empty() || (rest.starts_with('.') && rest[1..].split('.').all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit())))
}

/// More than any engine unpacks to; a guard against a malformed archive.
const MAX_UNPACKED: u64 = 6 * 1024 * 1024 * 1024;

/// Download `archive` to `dest`, checking the size and SHA-256 as pinned.
/// `on_bytes` gets the number of bytes received so far.
async fn download(
    archive: &Archive,
    dest: &Path,
    cancel: &(dyn Fn() -> bool + Send + Sync),
    mut on_bytes: impl FnMut(u64) + Send,
) -> Result<(), String> {
    let client = reqwest::Client::builder()
        .user_agent("loft/0.1")
        .connect_timeout(std::time::Duration::from_secs(20))
        .build()
        .map_err(|e| e.to_string())?;
    let mut resp = client
        .get(&archive.url)
        .send()
        .await
        .map_err(|e| format!("The download could not be started ({e}). Check the internet connection and try again."))?;
    if !resp.status().is_success() {
        return Err(format!("The download failed: HTTP {} for {}.", resp.status(), archive.name));
    }
    let mut file = std::fs::File::create(dest).map_err(|e| format!("cannot write {}: {e}", dest.display()))?;
    let mut hasher = Sha256::new();
    let mut received = 0u64;
    loop {
        if cancel() {
            return Err("Cancelled.".into());
        }
        let chunk = resp.chunk().await.map_err(|e| format!("The download was interrupted ({e}). Try again."))?;
        let Some(chunk) = chunk else { break };
        received += chunk.len() as u64;
        if received > archive.size {
            return Err(format!("{} is larger than expected, so it was discarded.", archive.name));
        }
        hasher.update(&chunk);
        file.write_all(&chunk).map_err(|e| format!("cannot write {}: {e}", dest.display()))?;
        on_bytes(received);
    }
    file.flush().map_err(|e| e.to_string())?;
    let actual: String = hasher.finalize().iter().map(|b| format!("{b:02x}")).collect();
    if received != archive.size || !actual.eq_ignore_ascii_case(&archive.sha256) {
        return Err(format!(
            "{} does not match its pinned checksum, so it was discarded. Try the download again.",
            archive.name
        ));
    }
    Ok(())
}

/// Unpack the files Loft uses from a `.zip` into `into` (flat). Returns how
/// many were written. Only the folder that holds `llama-server` is read (or,
/// in an archive without it, the folder with the most libraries), and files
/// are written under their own name only: a path inside the archive is never
/// used as a path on disk.
fn unpack_zip(archive: &Path, into: &Path, server_name: &str, budget: &mut u64) -> Result<usize, String> {
    let file = std::fs::File::open(archive).map_err(|e| e.to_string())?;
    let mut zip = zip::ZipArchive::new(file).map_err(|e| format!("not a readable archive: {e}"))?;

    // (index, folder inside the archive, file name)
    let mut entries: Vec<(usize, String, String)> = Vec::new();
    for index in 0..zip.len() {
        let entry = zip.by_index(index).map_err(|e| e.to_string())?;
        if entry.is_dir() {
            continue;
        }
        // `enclosed_name` rejects absolute paths and anything with "..".
        let Some(path) = entry.enclosed_name() else { continue };
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else { continue };
        let folder = path.parent().map(|p| p.to_string_lossy().replace('\\', "/")).unwrap_or_default();
        entries.push((index, folder, name.to_string()));
    }
    let payload = entries
        .iter()
        .find(|(_, _, name)| name == server_name)
        .map(|(_, folder, _)| folder.clone())
        .or_else(|| {
            let mut count: HashMap<&str, usize> = HashMap::new();
            for (_, folder, name) in &entries {
                if keep(name, server_name) {
                    *count.entry(folder.as_str()).or_default() += 1;
                }
            }
            count.into_iter().max_by_key(|(_, n)| *n).map(|(folder, _)| folder.to_string())
        });
    let Some(payload) = payload else {
        return Ok(0);
    };

    let mut written = 0;
    for (index, folder, name) in &entries {
        if folder != &payload || !keep(name, server_name) {
            continue;
        }
        let mut entry = zip.by_index(*index).map_err(|e| e.to_string())?;
        let mut out = std::fs::File::create(into.join(name)).map_err(|e| e.to_string())?;
        // Never trust the size an archive declares for itself.
        let copied = std::io::copy(&mut (&mut entry).take(*budget + 1), &mut out).map_err(|e| format!("cannot unpack {name}: {e}"))?;
        if copied > *budget {
            return Err("The archive unpacks to far more than an engine should; it was discarded.".into());
        }
        *budget -= copied;
        written += 1;
    }
    Ok(written)
}

/// Unpack a `.tar.gz` (macOS, Linux) with the system `tar`, then keep the
/// files Loft uses, the same way as for a `.zip`.
fn unpack_tar(archive: &Path, into: &Path, scratch: &Path, server_name: &str) -> Result<usize, String> {
    std::fs::create_dir_all(scratch).map_err(|e| e.to_string())?;
    let status = std::process::Command::new("tar")
        .arg("-xf")
        .arg(archive)
        .arg("-C")
        .arg(scratch)
        .status()
        .map_err(|e| format!("cannot run tar: {e}"))?;
    if !status.success() {
        return Err("The archive could not be unpacked.".into());
    }
    // The folder that holds the server, else the one with the most libraries.
    fn visit(dir: &Path, server_name: &str, best: &mut Option<(PathBuf, usize)>) -> Option<PathBuf> {
        let entries: Vec<_> = std::fs::read_dir(dir).ok()?.flatten().collect();
        if entries.iter().any(|e| e.file_name().to_str() == Some(server_name) && e.path().is_file()) {
            return Some(dir.to_path_buf());
        }
        let libraries = entries
            .iter()
            .filter(|e| !e.path().is_dir() && e.file_name().to_str().is_some_and(|n| keep(n, server_name)))
            .count();
        if libraries > best.as_ref().map(|(_, n)| *n).unwrap_or(0) {
            *best = Some((dir.to_path_buf(), libraries));
        }
        entries.iter().filter(|e| e.path().is_dir()).find_map(|e| visit(&e.path(), server_name, best))
    }
    let mut best = None;
    let Some(payload) = visit(scratch, server_name, &mut best).or(best.map(|(dir, _)| dir)) else {
        return Ok(0);
    };
    let mut written = 0;
    for entry in std::fs::read_dir(&payload).map_err(|e| e.to_string())?.flatten() {
        let Some(name) = entry.file_name().to_str().map(str::to_string) else { continue };
        // `is_file` follows symlinks: versioned library names become real files.
        if !keep(&name, server_name) || !entry.path().is_file() {
            continue;
        }
        std::fs::copy(entry.path(), into.join(&name)).map_err(|e| format!("cannot unpack {name}: {e}"))?;
        written += 1;
    }
    let _ = std::fs::remove_dir_all(scratch);
    Ok(written)
}

#[cfg(unix)]
fn make_executable(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755));
}

#[cfg(not(unix))]
fn make_executable(_path: &Path) {}

/// Download `build` and install it as the folder `dest`
/// (`<engines>/<platform>-<backend>`). Nothing is left at `dest` unless every
/// archive passed its checks and the result contains `server_name`; an
/// engine already there is replaced only at that point.
///
/// `on_progress(stage, received, total)` reports bytes over the whole build.
pub async fn install(
    build: &Build,
    dest: &Path,
    server_name: &str,
    cancel: &(dyn Fn() -> bool + Send + Sync),
    mut on_progress: impl FnMut(Stage, u64, u64) + Send,
) -> Result<usize, String> {
    let parent = dest.parent().ok_or("no folder to install into")?;
    let folder = dest.file_name().and_then(|n| n.to_str()).ok_or("no folder to install into")?;
    std::fs::create_dir_all(parent).map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
    let work = parent.join(format!(".{folder}-incoming"));
    let _ = std::fs::remove_dir_all(&work);
    let staged = work.join("files");
    std::fs::create_dir_all(&staged).map_err(|e| format!("cannot create {}: {e}", work.display()))?;

    let result = async {
        let total = build.size();
        let mut done = 0u64;
        let mut files = 0usize;
        let mut budget = MAX_UNPACKED;
        for (index, archive) in build.archives.iter().enumerate() {
            let file = work.join(format!("archive-{index}"));
            on_progress(Stage::Downloading, done, total);
            download(archive, &file, cancel, |received| on_progress(Stage::Downloading, done + received, total)).await?;
            done += archive.size;
            on_progress(Stage::Unpacking, done, total);

            let (file_for_task, staged_for_task, scratch) = (file.clone(), staged.clone(), work.join(format!("unpacked-{index}")));
            let (is_zip, server) = (archive.name.to_ascii_lowercase().ends_with(".zip"), server_name.to_string());
            let (written, left) = tokio::task::spawn_blocking(move || {
                let mut budget = budget;
                let written = if is_zip {
                    unpack_zip(&file_for_task, &staged_for_task, &server, &mut budget)
                } else {
                    unpack_tar(&file_for_task, &staged_for_task, &scratch, &server)
                };
                written.map(|n| (n, budget))
            })
            .await
            .map_err(|e| e.to_string())??;
            budget = left;
            if written == 0 {
                return Err(format!("{} contains nothing Loft uses.", archive.name));
            }
            files += written;
            let _ = std::fs::remove_file(&file);
            if cancel() {
                return Err("Cancelled.".into());
            }
        }
        if !staged.join(server_name).is_file() {
            return Err(format!("The build does not contain {server_name}."));
        }
        make_executable(&staged.join(server_name));

        // Swap in: the old engine goes only once the new one is complete.
        on_progress(Stage::Installing, total, total);
        let old = parent.join(format!(".{folder}-old"));
        let _ = std::fs::remove_dir_all(&old);
        let had_old = dest.exists();
        if had_old {
            std::fs::rename(dest, &old).map_err(|e| format!("The engine in use could not be replaced ({e})."))?;
        }
        if let Err(e) = std::fs::rename(&staged, dest) {
            if had_old {
                let _ = std::fs::rename(&old, dest); // put the old one back
            }
            return Err(format!("The engine could not be put in place ({e})."));
        }
        let _ = std::fs::remove_dir_all(&old);
        Ok(files)
    }
    .await;
    let _ = std::fs::remove_dir_all(&work);
    result
}

/// Remove an installed engine folder.
pub fn remove(dest: &Path) -> Result<(), String> {
    if !dest.exists() {
        return Ok(());
    }
    std::fs::remove_dir_all(dest).map_err(|e| format!("The engine could not be removed ({e})."))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    const SERVER: &str = "llama-server.exe";

    #[test]
    fn the_built_in_manifest_pins_trusted_archives_of_one_release() {
        let m = manifest();
        assert_eq!(m.tag, crate::runtime::LLAMA_REVISION);
        let mut seen = 0;
        for (platform, builds) in &m.builds {
            for (backend, build) in builds {
                assert!(!build.archives.is_empty(), "{platform}/{backend} has no archive");
                for archive in &build.archives {
                    assert!(m.trusted(archive), "{platform}/{backend}: {} is not a pinned release asset", archive.name);
                    seen += 1;
                }
            }
        }
        assert!(seen >= 10);
        // The optional engines for Windows are there, with their real sizes.
        let cuda = m.build("win-x64", Backend::Cuda).expect("CUDA for Windows");
        assert!(cuda.optional && cuda.archives.len() == 2 && cuda.size() > 500_000_000);
        let openvino = m.build("win-x64", Backend::OpenVino).expect("OpenVINO for Windows");
        assert!(openvino.optional && openvino.size() > 50_000_000);
        assert!(!m.build("win-x64", Backend::Cpu).unwrap().optional);
        assert!(m.build("mac-arm64", Backend::Cuda).is_none());
    }

    #[test]
    fn only_assets_of_the_pinned_release_are_trusted() {
        let m = Manifest {
            repo: "ggml-org/llama.cpp".into(),
            tag: "b1".into(),
            builds: HashMap::new(),
        };
        let good = Archive {
            name: "llama-b1-bin-win-cpu-x64.zip".into(),
            size: 10,
            sha256: "a".repeat(64),
            url: "https://github.com/ggml-org/llama.cpp/releases/download/b1/llama-b1-bin-win-cpu-x64.zip".into(),
        };
        assert!(m.trusted(&good));
        let with = |change: &dyn Fn(&mut Archive)| {
            let mut a = good.clone();
            change(&mut a);
            a
        };
        // Another host, another repository, another release, plain http.
        assert!(!m.trusted(&with(&|a| a.url = a.url.replace("github.com", "github.com.example.org"))));
        assert!(!m.trusted(&with(&|a| a.url = a.url.replace("ggml-org", "someone-else"))));
        assert!(!m.trusted(&with(&|a| a.url = a.url.replace("/b1/", "/b2/"))));
        assert!(!m.trusted(&with(&|a| a.url = a.url.replace("https://", "http://"))));
        // A name that is not the file in the URL, or that is a path.
        assert!(!m.trusted(&with(&|a| a.name = "other.zip".into())));
        assert!(!m.trusted(&with(&|a| {
            a.name = "../evil.zip".into();
            a.url = "https://github.com/ggml-org/llama.cpp/releases/download/b1/../evil.zip".into();
        })));
        // No real checksum.
        assert!(!m.trusted(&with(&|a| a.sha256 = "abc".into())));
        assert!(!m.trusted(&with(&|a| a.sha256 = "z".repeat(64))));
    }

    #[test]
    fn only_what_a_server_needs_is_kept() {
        for name in [SERVER, "ggml-vulkan.dll", "llama-server-impl.dll", "openvino_intel_npu_plugin.dll", "plugins.xml", "cache.json", "LICENSE", "LICENSE-curl"] {
            assert!(keep(name, SERVER), "{name} should be kept");
        }
        for name in ["llama-cli.exe", "llama-bench.exe", "llama-cli-impl.dll", "README.md", "run.bat", "notes.txt"] {
            assert!(!keep(name, SERVER), "{name} should be dropped");
        }
        assert!(keep("libllama.so", "llama-server") && keep("libggml.so.0.9.4", "llama-server") && keep("libmtmd.dylib", "llama-server"));
        assert!(!keep("libllama.so.backup", "llama-server") && !keep("sound", "llama-server"));
        assert!(keep("llama-server", "llama-server") && !keep("llama-cli", "llama-server"));
    }

    /// A zip with the given (path, contents) entries.
    fn zip_of(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut out = std::io::Cursor::new(Vec::new());
        {
            let mut zip = zip::ZipWriter::new(&mut out);
            let options = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
            for (path, contents) in entries {
                zip.start_file(*path, options).unwrap();
                zip.write_all(contents).unwrap();
            }
            zip.finish().unwrap();
        }
        out.into_inner()
    }

    fn sha256(bytes: &[u8]) -> String {
        Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
    }

    /// Serve `files` (name -> bytes) over HTTP on a local port, for the test.
    async fn serve(files: Vec<(String, Vec<u8>)>) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move {
            loop {
                let Ok((mut socket, _)) = listener.accept().await else { break };
                let files = files.clone();
                tokio::spawn(async move {
                    let mut request = vec![0u8; 4096];
                    let n = socket.read(&mut request).await.unwrap_or(0);
                    let request = String::from_utf8_lossy(&request[..n]).to_string();
                    let path = request.split_whitespace().nth(1).unwrap_or("/").trim_start_matches('/').to_string();
                    let response = match files.iter().find(|(name, _)| *name == path) {
                        Some((_, body)) => {
                            let mut r = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()).into_bytes();
                            r.extend_from_slice(body);
                            r
                        }
                        None => b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec(),
                    };
                    let _ = socket.write_all(&response).await;
                    let _ = socket.shutdown().await;
                });
            }
        });
        address
    }

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("loft-installer-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn names(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(dir).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        names.sort();
        names
    }

    #[tokio::test]
    async fn an_engine_is_downloaded_checked_and_installed() {
        // The build proper, and a second archive of extra libraries (as CUDA has).
        let main = zip_of(&[
            ("build/bin/llama-server.exe", b"server v2"),
            ("build/bin/ggml-cuda.dll", b"cuda"),
            ("build/bin/llama-cli.exe", b"another tool"),
            ("build/bin/llama-cli-impl.dll", b"another tool's library"),
            ("build/bin/LICENSE", b"licence"),
            ("build/README.md", b"readme"),
            ("build/other/ggml-cuda.dll", b"a decoy in another folder"),
            ("../escape.dll", b"must never be written"),
        ]);
        let extra = zip_of(&[("cudart64_13.dll", b"runtime"), ("cublas64_13.dll", b"blas")]);
        let address = serve(vec![("main.zip".into(), main.clone()), ("extra.zip".into(), extra.clone())]).await;
        let build = Build {
            optional: true,
            archives: vec![
                Archive { name: "main.zip".into(), size: main.len() as u64, sha256: sha256(&main), url: format!("{address}/main.zip") },
                Archive { name: "extra.zip".into(), size: extra.len() as u64, sha256: sha256(&extra), url: format!("{address}/extra.zip") },
            ],
            note: None,
        };

        let root = temp_dir("install");
        let dest = root.join("win-x64-cuda");
        // An older engine is already there.
        std::fs::create_dir_all(&dest).unwrap();
        std::fs::write(dest.join(SERVER), b"server v1").unwrap();
        std::fs::write(dest.join("old-only.dll"), b"old").unwrap();

        let mut stages = Vec::new();
        let mut last = (0, 0);
        let files = install(&build, &dest, SERVER, &|| false, |stage, received, total| {
            if stages.last() != Some(&stage) {
                stages.push(stage);
            }
            assert!(received >= last.0 && received <= total, "progress only moves forward");
            last = (received, total);
        })
        .await
        .expect("the install succeeds");

        assert_eq!(files, 5);
        assert_eq!(names(&dest), ["LICENSE", "cublas64_13.dll", "cudart64_13.dll", "ggml-cuda.dll", SERVER]);
        // The new server, the library from the server's own folder (not the decoy).
        assert_eq!(std::fs::read(dest.join(SERVER)).unwrap(), b"server v2");
        assert_eq!(std::fs::read(dest.join("ggml-cuda.dll")).unwrap(), b"cuda");
        // Nothing escaped the folder, and no work folders are left behind.
        assert_eq!(names(&root), ["win-x64-cuda"]);
        assert!(!root.parent().unwrap().join("escape.dll").exists());
        assert_eq!(stages, [Stage::Downloading, Stage::Unpacking, Stage::Downloading, Stage::Unpacking, Stage::Installing]);
        assert_eq!(last, (build.size(), build.size()));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn a_download_that_is_not_what_was_pinned_installs_nothing() {
        let real = zip_of(&[("llama-server.exe", b"server"), ("ggml.dll", b"lib")]);
        let tampered = zip_of(&[("llama-server.exe", b"something else"), ("ggml.dll", b"lib")]);
        let no_server = zip_of(&[("ggml.dll", b"lib")]);
        let address = serve(vec![("engine.zip".into(), tampered.clone()), ("no-server.zip".into(), no_server.clone())]).await;

        let root = temp_dir("reject");
        let dest = root.join("win-x64-openvino");
        std::fs::create_dir_all(&dest).unwrap();
        std::fs::write(dest.join(SERVER), b"the engine in use").unwrap();
        let untouched = |why: &str| {
            assert_eq!(std::fs::read(dest.join(SERVER)).unwrap(), b"the engine in use", "{why}");
            assert_eq!(names(&root), ["win-x64-openvino"], "{why}: work folders are cleaned up");
        };
        let build_of = |name: &str, size: u64, sha256: String| Build {
            optional: true,
            archives: vec![Archive { name: name.into(), size, sha256, url: format!("{address}/{name}") }],
            note: None,
        };

        // Same size class, different bytes: the checksum does not match.
        let err = install(&build_of("engine.zip", tampered.len() as u64, sha256(&real)), &dest, SERVER, &|| false, |_, _, _| {}).await.unwrap_err();
        assert!(err.contains("does not match its pinned checksum"), "{err}");
        untouched("wrong checksum");

        // Right bytes, but more of them than pinned.
        let err = install(&build_of("engine.zip", 10, sha256(&tampered)), &dest, SERVER, &|| false, |_, _, _| {}).await.unwrap_err();
        assert!(err.contains("larger than expected"), "{err}");
        untouched("wrong size");

        // A valid archive with no server in it.
        let err = install(&build_of("no-server.zip", no_server.len() as u64, sha256(&no_server)), &dest, SERVER, &|| false, |_, _, _| {}).await.unwrap_err();
        assert!(err.contains("does not contain llama-server.exe"), "{err}");
        untouched("no server");

        // Not there at all.
        let err = install(&build_of("missing.zip", 5, sha256(b"x")), &dest, SERVER, &|| false, |_, _, _| {}).await.unwrap_err();
        assert!(err.contains("HTTP 404"), "{err}");
        untouched("missing");

        // Cancelled part-way.
        let cancelled = AtomicBool::new(false);
        let err = install(&build_of("engine.zip", tampered.len() as u64, sha256(&tampered)), &dest, SERVER, &|| cancelled.load(Ordering::SeqCst), |_, _, _| {
            cancelled.store(true, Ordering::SeqCst)
        })
        .await
        .unwrap_err();
        assert_eq!(err, "Cancelled.");
        untouched("cancelled");

        // And removing an engine removes its folder.
        remove(&dest).unwrap();
        assert!(!dest.exists());
        remove(&dest).unwrap(); // already gone is fine
        let _ = std::fs::remove_dir_all(&root);
    }
}
