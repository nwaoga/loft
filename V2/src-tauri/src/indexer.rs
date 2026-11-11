//! M4 — indexer core.
//!
//! Walks a folder, and for every supported file that is new or changed:
//! parse -> chunk -> embed -> store. Unchanged files are skipped (mtime+size
//! fast path, then a blake3 content hash), and files that disappeared are
//! removed from the index. Free of Tauri types so it can be tested directly;
//! `jobs.rs` wraps it with queuing, watching and UI events.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::Serialize;

use crate::chunk::{chunk_sections, CHUNK_MAX, CHUNK_OVERLAP};
use crate::parse::{self, Kind};
use crate::store::{ChunkInput, FileRecord, Store};
use crate::vision::Describer;

/// Chunks sent to the embedding server per request.
const EMBED_BATCH: usize = 16;

/// Directories never worth indexing (dependencies and build output).
pub const SKIP_DIRS: &[&str] = &[
    "node_modules",
    "target",
    "obj",
    "__pycache__",
    "venv",
    ".venv",
    ".git",
];

/// Generated files that are huge and useless for questions.
const SKIP_FILES: &[&str] = &[
    "package-lock.json",
    "yarn.lock",
    "pnpm-lock.yaml",
    "cargo.lock",
    "composer.lock",
    "poetry.lock",
];

/// Where and how to embed document chunks.
#[derive(Debug, Clone)]
pub struct EmbedTarget {
    pub port: u16,
    pub doc_prefix: String,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct Progress {
    pub done: u32,
    pub total: u32,
    pub current: String,
}

#[derive(Debug, Default, Clone, PartialEq)]
pub struct Summary {
    pub indexed: u32,
    pub unchanged: u32,
    pub removed: u32,
    pub failed: u32,
    pub chunks: u32,
    pub cancelled: bool,
}

enum Outcome {
    Unchanged,
    Indexed(u32),
}

enum FileError {
    /// This file could not be indexed; carry on with the rest.
    Skip(String),
    /// The whole job cannot continue (store or embedding server is down).
    Fatal(String),
}

enum Prepared {
    Unchanged,
    Ready {
        hash: String,
        chunks: Vec<ChunkInput>,
        /// For a picture that is going to be described: its bytes and the
        /// text OCR read off it (possibly none).
        picture: Option<(Vec<u8>, String)>,
    },
}

fn path_str(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

/// True when `path` (relative to a watched root) is inside a skipped or
/// hidden directory, or is itself hidden / a lock or temp file.
pub fn is_ignored_relative(rel: &Path) -> bool {
    rel.components().any(|c| {
        let name = c.as_os_str().to_string_lossy();
        name.starts_with('.') || name.starts_with("~$") || SKIP_DIRS.contains(&name.as_ref())
    })
}

/// Every indexable file under `root`, honouring .gitignore, skipping hidden
/// files, dependency/build folders, lock files and oversized files.
pub fn walk(root: &Path) -> Vec<(PathBuf, Kind)> {
    let walker = ignore::WalkBuilder::new(root)
        .hidden(true)
        .git_ignore(true)
        .git_global(false)
        .git_exclude(false)
        .require_git(false)
        .follow_links(false)
        .filter_entry(|entry| {
            let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
            let name = entry.file_name().to_string_lossy();
            !(is_dir && SKIP_DIRS.contains(&name.as_ref()))
        })
        .build();

    let mut files = Vec::new();
    for entry in walker.flatten() {
        if !entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
            continue;
        }
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_lowercase();
        if name.starts_with("~$") || name.ends_with(".min.js") || SKIP_FILES.contains(&name.as_str()) {
            continue;
        }
        let Some(kind) = parse::kind_for(path) else {
            continue;
        };
        let size = entry.metadata().map(|m| m.len()).unwrap_or(u64::MAX);
        if size == 0 || size > kind.max_bytes() {
            continue;
        }
        files.push((path.to_path_buf(), kind));
    }
    files.sort_by(|a, b| a.0.cmp(&b.0));
    files
}

/// Read, hash, and (if changed) parse + chunk a file. Blocking. With
/// `keep_picture`, a picture's bytes are handed back for describing.
fn prepare(path: &Path, kind: Kind, known_hash: Option<String>, keep_picture: bool) -> Result<Prepared, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("read failed: {e}"))?;
    let hash = blake3::hash(&bytes).to_hex().to_string();
    if known_hash.as_deref() == Some(hash.as_str()) {
        return Ok(Prepared::Unchanged);
    }
    let sections = parse::parse(kind, &bytes)?;
    let chunks = chunk_sections(&sections, CHUNK_MAX, CHUNK_OVERLAP);
    let picture = (keep_picture && kind == Kind::Image).then(|| {
        let text = sections.iter().map(|s| s.text.as_str()).collect::<Vec<_>>().join("\n");
        (bytes, text)
    });
    Ok(Prepared::Ready { hash, chunks, picture })
}

fn embed_input(embed: &EmbedTarget, file_name: &str, text: &str) -> String {
    // The file name gives the vector a little context ("budget.xlsx").
    format!("{}{}\n{}", embed.doc_prefix, file_name, text)
}

fn truncate_chars(text: &str, max: usize) -> &str {
    if text.len() <= max {
        return text;
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

/// Embed a file's chunks in batches. A chunk the server rejects (e.g. too many
/// tokens) is retried alone, then truncated, then dropped — the rest survive.
async fn embed_chunks(
    embed: &EmbedTarget,
    file_name: &str,
    chunks: Vec<ChunkInput>,
) -> Result<(Vec<ChunkInput>, Vec<Vec<f32>>), FileError> {
    if chunks.is_empty() {
        return Ok((chunks, Vec::new()));
    }
    let mut kept = Vec::with_capacity(chunks.len());
    let mut vectors = Vec::with_capacity(chunks.len());
    for batch in chunks.chunks(EMBED_BATCH) {
        let inputs: Vec<String> = batch
            .iter()
            .map(|c| embed_input(embed, file_name, &c.text))
            .collect();
        match crate::embed::embed(embed.port, &inputs).await {
            Ok(batch_vectors) => {
                kept.extend(batch.iter().cloned());
                vectors.extend(batch_vectors);
            }
            Err(_) => {
                for chunk in batch {
                    let mut result =
                        crate::embed::embed_one(embed.port, &embed_input(embed, file_name, &chunk.text)).await;
                    if result.is_err() {
                        let shorter = truncate_chars(&chunk.text, 600);
                        result =
                            crate::embed::embed_one(embed.port, &embed_input(embed, file_name, shorter)).await;
                    }
                    if let Ok(vector) = result {
                        kept.push(chunk.clone());
                        vectors.push(vector);
                    }
                }
            }
        }
    }
    if kept.is_empty() {
        if !crate::runtime::wait_healthy(embed.port, Duration::from_secs(3)).await {
            return Err(FileError::Fatal(
                "The embedding model stopped responding.".into(),
            ));
        }
        return Err(FileError::Skip("no chunk could be embedded".into()));
    }
    Ok((kept, vectors))
}

async fn index_one(
    store: &Arc<Mutex<Store>>,
    folder_id: i64,
    path: &Path,
    path_key: &str,
    file_name: &str,
    kind: Kind,
    embed: &EmbedTarget,
    vision: Option<&Describer>,
) -> Result<Outcome, FileError> {
    let meta = std::fs::metadata(path).map_err(|e| FileError::Skip(e.to_string()))?;
    let mtime = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let size = meta.len() as i64;

    let record = store
        .lock()
        .unwrap()
        .file_record(folder_id, path_key)
        .map_err(FileError::Fatal)?;
    if let Some(r) = &record {
        if r.mtime == mtime && r.size == size {
            return Ok(Outcome::Unchanged); // fast path: not touched
        }
    }

    let known_hash = record.map(|r| r.hash);
    let owned = path.to_path_buf();
    let keep_picture = vision.is_some();
    let prepared = tokio::task::spawn_blocking(move || prepare(&owned, kind, known_hash, keep_picture))
        .await
        .map_err(|e| FileError::Skip(format!("parser task failed: {e}")))?
        .map_err(FileError::Skip)?;

    match prepared {
        Prepared::Unchanged => {
            // Touched but identical content: remember the new mtime.
            store
                .lock()
                .unwrap()
                .touch_file(folder_id, path_key, mtime, size)
                .map_err(FileError::Fatal)?;
            Ok(Outcome::Unchanged)
        }
        Prepared::Ready { hash, mut chunks, picture } => {
            // A picture is indexed under what the picture model says it shows,
            // plus any text read off it. Without a description (no model, or
            // it failed) it keeps the OCR text alone.
            if let (Some((bytes, ocr_text)), Some(describer)) = (picture, vision) {
                if let Some(description) = describer.describe(&bytes, file_name).await {
                    let text = crate::vision::picture_text(Some(&description), &ocr_text);
                    chunks = chunk_sections(&[parse::Section { text, page: None }], CHUNK_MAX, CHUNK_OVERLAP);
                }
            }
            let (chunks, vectors) = embed_chunks(embed, file_name, chunks).await?;
            let record = FileRecord { hash, mtime, size };
            store
                .lock()
                .unwrap()
                .replace_file(folder_id, path_key, &record, kind.as_str(), &chunks, &vectors)
                .map_err(FileError::Fatal)?;
            Ok(Outcome::Indexed(chunks.len() as u32))
        }
    }
}

/// Bring the index for `root` up to date. Returns what changed; `Err` means
/// the job could not continue at all.
#[cfg_attr(not(test), allow(dead_code))] // the app always goes through `index_folder_with`
pub async fn index_folder(
    store: Arc<Mutex<Store>>,
    folder_id: i64,
    root: PathBuf,
    embed: EmbedTarget,
    cancel: Arc<AtomicBool>,
    on_progress: impl FnMut(&Progress) + Send,
) -> Result<Summary, String> {
    index_folder_with(store, folder_id, root, embed, None, cancel, on_progress).await
}

/// `index_folder`, with an optional picture model: when `vision` is given,
/// pictures are described by it as they are indexed.
pub async fn index_folder_with(
    store: Arc<Mutex<Store>>,
    folder_id: i64,
    root: PathBuf,
    embed: EmbedTarget,
    vision: Option<&Describer>,
    cancel: Arc<AtomicBool>,
    mut on_progress: impl FnMut(&Progress) + Send,
) -> Result<Summary, String> {
    let walk_root = root.clone();
    let files = tokio::task::spawn_blocking(move || walk(&walk_root))
        .await
        .map_err(|e| format!("folder scan failed: {e}"))?;

    let mut summary = Summary::default();

    // Drop files that are gone (or no longer indexable).
    let on_disk: HashSet<String> = files.iter().map(|(p, _)| path_str(p)).collect();
    let known = store.lock().unwrap().file_paths(folder_id)?;
    for path in known {
        if !on_disk.contains(&path) {
            store.lock().unwrap().delete_file(folder_id, &path)?;
            summary.removed += 1;
        }
    }

    let mut progress = Progress {
        done: 0,
        total: files.len() as u32,
        current: String::new(),
    };
    on_progress(&progress);

    for (path, kind) in files {
        if cancel.load(Ordering::Relaxed) {
            summary.cancelled = true;
            break;
        }
        let path_key = path_str(&path);
        let file_name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        progress.current = file_name.clone();
        on_progress(&progress);

        match index_one(&store, folder_id, &path, &path_key, &file_name, kind, &embed, vision).await {
            Ok(Outcome::Unchanged) => summary.unchanged += 1,
            Ok(Outcome::Indexed(n)) => {
                summary.indexed += 1;
                summary.chunks += n;
            }
            Err(FileError::Skip(reason)) => {
                summary.failed += 1;
                eprintln!("Loft: skipped {path_key}: {reason}");
            }
            Err(FileError::Fatal(reason)) => return Err(reason),
        }
        progress.done += 1;
        on_progress(&progress);
    }
    Ok(summary)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::fixtures;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("loft-idx-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn walk_filters_what_is_indexable() {
        let dir = temp_dir("walk");
        std::fs::write(dir.join("notes.md"), "hello").unwrap();
        std::fs::write(dir.join("movie.mp4"), [1, 2, 3]).unwrap();
        std::fs::write(dir.join("empty.txt"), "").unwrap();
        std::fs::write(dir.join(".secret.txt"), "hidden").unwrap();
        std::fs::write(dir.join("package-lock.json"), "{}").unwrap();
        std::fs::write(dir.join("~$draft.docx"), "lock").unwrap();
        std::fs::create_dir_all(dir.join("node_modules/pkg")).unwrap();
        std::fs::write(dir.join("node_modules/pkg/index.js"), "x").unwrap();
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(dir.join("src/main.rs"), "fn main() {}").unwrap();
        std::fs::write(dir.join(".gitignore"), "ignored.txt\n").unwrap();
        std::fs::write(dir.join("ignored.txt"), "nope").unwrap();

        let found: Vec<String> = walk(&dir)
            .into_iter()
            .map(|(p, _)| p.strip_prefix(&dir).unwrap().to_string_lossy().replace('\\', "/"))
            .collect();
        assert_eq!(found, vec!["notes.md", "src/main.rs"]);

        assert!(is_ignored_relative(Path::new("node_modules/x/y.js")));
        assert!(is_ignored_relative(Path::new(".git/HEAD")));
        assert!(!is_ignored_relative(Path::new("src/main.rs")));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Full pipeline against the real embedding server: index a folder holding
    /// every supported format, query it, then change/delete files and confirm
    /// the incremental pass does the minimum work. Run with:
    ///   cargo test --lib indexer::tests::end_to_end -- --ignored --nocapture
    #[tokio::test]
    #[ignore = "requires the Nomic model + bundled llama-server"]
    async fn end_to_end() {
        use crate::runtime::{wait_healthy, Runtime, ServerKind};

        let model = PathBuf::from(std::env::var("APPDATA").unwrap_or_default())
            .join("com.loft.desktop")
            .join("models")
            .join("nomic-embed-text-v1.5.f16.gguf");
        if !model.exists() {
            eprintln!("skip: model not found at {}", model.display());
            return;
        }
        let mut rt = Runtime::new();
        let port = rt
            .spawn(ServerKind::Embed, &model, "nomic-embed-text-v1.5", 8192)
            .expect("spawn embed server");
        assert!(wait_healthy(port, Duration::from_secs(60)).await);
        let embed = EmbedTarget {
            port,
            doc_prefix: "search_document: ".into(),
        };

        // A folder with one file of every supported kind (+ things to skip).
        let dir = temp_dir("e2e");
        std::fs::write(
            dir.join("notes.md"),
            "# Garden log\n\nThe tomato seedlings were moved to the greenhouse on the 4th of May.\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("report.docx"),
            fixtures::docx(&["Annual report", "Customer churn fell to three percent after the loyalty scheme launched."]),
        )
        .unwrap();
        std::fs::write(
            dir.join("budget.xlsx"),
            fixtures::xlsx("Budget", &[&["Item", "Cost"], &["Forklift lease", "4800"], &["Warehouse heating", "950"]]),
        )
        .unwrap();
        std::fs::write(
            dir.join("deck.pptx"),
            fixtures::pptx(&["Kickoff", "The mobile app launches in Lisbon first, then Madrid."]),
        )
        .unwrap();
        std::fs::write(
            dir.join("manual.pdf"),
            fixtures::pdf("Descale the espresso machine every ninety days using citric acid."),
        )
        .unwrap();
        // A long file that must split into several chunks.
        let long: String = (0..400)
            .map(|i| format!("Line {i}: the warehouse inventory count for aisle {i} was verified.\n"))
            .collect();
        std::fs::write(dir.join("inventory.txt"), &long).unwrap();
        std::fs::create_dir_all(dir.join("node_modules")).unwrap();
        std::fs::write(dir.join("node_modules/junk.js"), "skip me").unwrap();
        std::fs::write(dir.join("image.png"), [0u8, 1, 2]).unwrap();

        let db = std::env::temp_dir().join(format!("loft-idx-e2e-{}.db", std::process::id()));
        let _ = std::fs::remove_file(&db);
        let store = Arc::new(Mutex::new(Store::open(&db).unwrap()));
        let dim = crate::embed::embed_one(port, "probe").await.unwrap().len();
        store.lock().unwrap().ensure_embedding_space("nomic-embed-text-v1.5", dim).unwrap();
        let folder = store.lock().unwrap().add_folder(&path_str(&dir)).unwrap();

        let run = |store: Arc<Mutex<Store>>, embed: EmbedTarget, dir: PathBuf| async move {
            let mut last = Progress::default();
            let summary = index_folder(store, folder, dir, embed, Arc::new(AtomicBool::new(false)), |p| {
                last = p.clone();
            })
            .await
            .expect("index");
            (summary, last)
        };

        // --- first pass: everything is new ---
        let (summary, last) = run(store.clone(), embed.clone(), dir.clone()).await;
        eprintln!("first pass: {summary:?}");
        assert_eq!(summary.indexed, 6);
        assert_eq!(summary.failed, 0);
        assert_eq!((last.done, last.total), (6, 6));
        assert!(summary.chunks > 10, "the long file should produce many chunks");

        // --- every format is retrievable ---
        for (question, expect_file, expect_text) in [
            ("where did the tomato plants go", "notes.md", "greenhouse"),
            ("what happened to customer churn", "report.docx", "three percent"),
            ("how much is the forklift lease", "budget.xlsx", "4800"),
            ("which city gets the mobile app first", "deck.pptx", "Lisbon"),
            ("how often should I descale the coffee machine", "manual.pdf", "ninety days"),
        ] {
            let q = crate::embed::embed_one(port, &format!("search_query: {question}")).await.unwrap();
            let hits = store.lock().unwrap().search(&q, 3).unwrap();
            eprintln!("Q: {question}\n   -> {:.3} {} | {}", hits[0].score, hits[0].path, hits[0].text.replace('\n', " "));
            assert!(hits[0].path.ends_with(expect_file), "expected {expect_file}, got {}", hits[0].path);
            assert!(hits[0].text.contains(expect_text));
        }
        // Page / sheet / slide numbers survive for citations.
        let q = crate::embed::embed_one(port, "search_query: which city gets the mobile app first").await.unwrap();
        assert_eq!(store.lock().unwrap().search(&q, 1).unwrap()[0].page, Some(2), "slide 2");

        // --- second pass: nothing changed, nothing re-embedded ---
        let (summary, _) = run(store.clone(), embed.clone(), dir.clone()).await;
        eprintln!("second pass: {summary:?}");
        assert_eq!((summary.indexed, summary.unchanged, summary.removed), (0, 6, 0));

        // --- edit one file, delete another ---
        std::fs::write(dir.join("notes.md"), "# Garden log\n\nThe beehive was relocated next to the orchard in June.\n").unwrap();
        std::fs::remove_file(dir.join("deck.pptx")).unwrap();
        let (summary, _) = run(store.clone(), embed.clone(), dir.clone()).await;
        eprintln!("third pass: {summary:?}");
        assert_eq!((summary.indexed, summary.unchanged, summary.removed), (1, 4, 1));

        let q = crate::embed::embed_one(port, "search_query: where is the beehive").await.unwrap();
        let hits = store.lock().unwrap().search(&q, 3).unwrap();
        assert!(hits[0].text.contains("orchard"));
        assert!(hits.iter().all(|h| !h.path.ends_with("deck.pptx")), "deleted file is gone");
        assert!(hits.iter().all(|h| !h.text.contains("tomato")), "old content is gone");

        // --- cancellation stops promptly ---
        store.lock().unwrap().delete_folder_index(folder).unwrap();
        let cancel = Arc::new(AtomicBool::new(true));
        let summary = index_folder(store.clone(), folder, dir.clone(), embed.clone(), cancel, |_| {})
            .await
            .unwrap();
        assert!(summary.cancelled);
        assert_eq!(summary.indexed, 0);

        drop(store);
        let _ = std::fs::remove_file(&db);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
