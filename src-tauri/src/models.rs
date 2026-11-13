//! M2 — model downloader.
//!
//! Streams a GGUF model from its catalog URL to the models directory with
//! progress callbacks and resume support, validates the GGUF magic header, and
//! atomically moves the finished file into place. Kept free of Tauri types so it
//! can be unit-tested directly; the command layer wraps it with event emission.

use std::io::Read;
use std::path::Path;

/// Download `url` to `dest`, reporting (downloaded, total) bytes via `on_progress`.
/// Resumes from a partial `<dest>.part` when the server supports range requests.
pub async fn download_model_file(
    url: &str,
    dest: &Path,
    expected_sha256: Option<&str>,
    mut on_progress: impl FnMut(u64, u64),
) -> Result<(), String> {
    let part = dest.with_extension("part");

    if let Some(parent) = dest.parent() {
        let _ = std::fs::create_dir_all(parent);
    }

    // Resume from an existing partial file if present.
    let existing = std::fs::metadata(&part).map(|m| m.len()).unwrap_or(0);

    let client = reqwest::Client::builder()
        .user_agent("loft/0.1")
        .build()
        .map_err(|e| e.to_string())?;

    let mut req = client.get(url);
    if existing > 0 {
        req = req.header(reqwest::header::RANGE, format!("bytes={existing}-"));
    }
    let mut resp = req.send().await.map_err(|e| e.to_string())?;
    let status = resp.status();
    if !status.is_success() {
        return Err(format!("download failed: HTTP {status}"));
    }

    // 206 = server honoured our range and we can append; otherwise start fresh.
    let resumed = status.as_u16() == 206 && existing > 0;
    let body_len = resp.content_length().unwrap_or(0);
    let total = if resumed { existing + body_len } else { body_len };

    use std::io::Write;
    let mut file = if resumed {
        std::fs::OpenOptions::new()
            .append(true)
            .open(&part)
            .map_err(|e| e.to_string())?
    } else {
        std::fs::File::create(&part).map_err(|e| e.to_string())?
    };

    let mut downloaded = if resumed { existing } else { 0 };
    on_progress(downloaded, total);

    while let Some(chunk) = resp.chunk().await.map_err(|e| e.to_string())? {
        file.write_all(&chunk).map_err(|e| e.to_string())?;
        downloaded += chunk.len() as u64;
        on_progress(downloaded, total);
    }
    file.flush().map_err(|e| e.to_string())?;
    drop(file);

    // Integrity: a complete GGUF file starts with the ASCII magic "GGUF".
    verify_gguf(&part)?;
    // Authenticity: the bytes must match the hash pinned in the catalog.
    if let Some(expected) = expected_sha256.filter(|s| !s.is_empty()) {
        let path = part.clone();
        let actual = tokio::task::spawn_blocking(move || sha256_file(&path))
            .await
            .map_err(|e| e.to_string())??;
        if !actual.eq_ignore_ascii_case(expected) {
            // Don't resume from (or keep) a file that is not the real model.
            let _ = std::fs::remove_file(&part);
            return Err(
                "The downloaded file does not match the expected checksum, so it was discarded. Try the download again.".into(),
            );
        }
    }

    // Atomic-ish move into place.
    std::fs::rename(&part, dest).map_err(|e| e.to_string())?;
    Ok(())
}

/// Lower-case hex SHA-256 of a file.
pub fn sha256_file(path: &Path) -> Result<String, String> {
    use sha2::{Digest, Sha256};
    let mut file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = file.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}

fn verify_gguf(path: &Path) -> Result<(), String> {
    let mut f = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut magic = [0u8; 4];
    f.read_exact(&mut magic)
        .map_err(|e| format!("could not read downloaded file: {e}"))?;
    if &magic != b"GGUF" {
        return Err("downloaded file is not a valid GGUF model (bad header)".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn models_dir() -> PathBuf {
        if cfg!(target_os = "windows") {
            if let Ok(appdata) = std::env::var("APPDATA") {
                return PathBuf::from(appdata).join("com.loft.desktop").join("models");
            }
        }
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("models")
    }

    #[test]
    fn sha256_of_a_known_file() {
        let path = std::env::temp_dir().join(format!("loft-sha-{}.bin", std::process::id()));
        std::fs::write(&path, b"abc").unwrap();
        assert_eq!(
            sha256_file(&path).unwrap(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        let _ = std::fs::remove_file(&path);
    }

    /// A download whose bytes don't match the pinned hash is rejected and
    /// removed; a matching one is installed. Uses a local one-shot HTTP server.
    #[tokio::test]
    async fn checksum_gates_installation() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let body: &'static [u8] = b"GGUF-not-really-a-model";
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/model.gguf", listener.local_addr().unwrap());
        tokio::spawn(async move {
            loop {
                let Ok((mut sock, _)) = listener.accept().await else { break };
                let mut req = [0u8; 2048];
                let _ = sock.read(&mut req).await;
                let head = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = sock.write_all(head.as_bytes()).await;
                let _ = sock.write_all(body).await;
            }
        });

        let dir = std::env::temp_dir().join(format!("loft-dl-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let dest = dir.join("model.gguf");

        let wrong = "0".repeat(64);
        let err = download_model_file(&url, &dest, Some(&wrong), |_, _| {})
            .await
            .unwrap_err();
        assert!(err.contains("checksum"), "{err}");
        assert!(!dest.exists(), "a mismatching file is not installed");
        assert!(!dest.with_extension("part").exists(), "and not kept for resume");

        // sha256("GGUF-not-really-a-model"), computed by the code under test
        // on a copy of the same bytes.
        let copy = dir.join("copy.bin");
        std::fs::write(&copy, body).unwrap();
        let right = sha256_file(&copy).unwrap().to_uppercase(); // case-insensitive match
        download_model_file(&url, &dest, Some(&right), |_, _| {})
            .await
            .unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), body);

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Checks every catalog hash against the model files already on disk.
    ///   cargo test --lib models::tests::installed_models_match_catalog -- --ignored --nocapture
    #[test]
    #[ignore = "hashes the downloaded models (several GB)"]
    fn installed_models_match_catalog() {
        let catalog: serde_json::Value =
            serde_json::from_str(include_str!("../catalog.json")).unwrap();
        let mut checked = 0;
        for m in catalog["models"].as_array().unwrap() {
            let file = m["file"].as_str().unwrap();
            let sha = m["sha256"].as_str().unwrap_or_default();
            assert_eq!(sha.len(), 64, "{file} has a pinned sha256");
            let path = models_dir().join(file);
            if !path.exists() {
                eprintln!("not installed, skipped: {file}");
                continue;
            }
            assert_eq!(
                std::fs::metadata(&path).unwrap().len(),
                m["sizeBytes"].as_u64().unwrap(),
                "size of {file}"
            );
            assert_eq!(sha256_file(&path).unwrap(), sha, "sha256 of {file}");
            eprintln!("ok: {file}");
            checked += 1;
        }
        assert!(checked > 0, "no installed models to check");
    }

    /// Downloads the ~2 GB Qwen2.5-3B chat model through the real code path and
    /// verifies it. Run with:
    ///   cargo test --lib models::tests::download_qwen -- --ignored --nocapture
    #[tokio::test]
    #[ignore = "downloads ~2 GB"]
    async fn download_qwen() {
        let url = "https://huggingface.co/Qwen/Qwen2.5-3B-Instruct-GGUF/resolve/main/qwen2.5-3b-instruct-q4_k_m.gguf";
        let dest = models_dir().join("qwen2.5-3b-instruct-q4_k_m.gguf");
        if dest.exists() {
            eprintln!("already present: {}", dest.display());
            return;
        }
        let mut last = -1.0f32;
        download_model_file(url, &dest, None, |d, t| {
            let pct = if t > 0 { d as f32 / t as f32 } else { 0.0 };
            if pct - last >= 0.05 {
                last = pct;
                eprintln!("  {:.0}%  ({} / {} bytes)", pct * 100.0, d, t);
            }
        })
        .await
        .expect("download qwen");
        let size = std::fs::metadata(&dest).unwrap().len();
        eprintln!("downloaded {} bytes to {}", size, dest.display());
        assert!(size > 1_000_000_000, "expected a multi-GB file");
    }
}
