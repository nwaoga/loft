//! M3 — vector store (SQLite + sqlite-vec).
//!
//! One `loft.db` file holds folders, files, text chunks, and their embedding
//! vectors (a `vec0` virtual table using cosine distance). The vector table is
//! created lazily for the active embedding model's dimension; switching to a
//! model with a different id/dimension resets the index (vectors from different
//! models are not comparable).

use std::path::Path;
use std::sync::Once;

use rusqlite::{params, Connection, OptionalExtension};

static REGISTER_VEC: Once = Once::new();

/// Register sqlite-vec as an auto-extension so every connection gets `vec0`.
fn register_vec() {
    REGISTER_VEC.call_once(|| unsafe {
        rusqlite::ffi::sqlite3_auto_extension(Some(std::mem::transmute(
            sqlite_vec::sqlite3_vec_init as *const (),
        )));
    });
}

fn e<T: std::fmt::Display>(err: T) -> String {
    err.to_string()
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn f32_bytes(v: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(v.len() * 4);
    for f in v {
        out.extend_from_slice(&f.to_le_bytes());
    }
    out
}

#[derive(Debug, Clone)]
pub struct FolderRow {
    pub id: i64,
    pub path: String,
    pub status: String,
    pub file_count: u32,
    pub chunk_count: u32,
}

/// What we remember about an indexed file, for incremental re-indexing.
#[derive(Debug, Clone, PartialEq)]
pub struct FileRecord {
    pub hash: String,
    pub mtime: i64,
    pub size: i64,
}

#[derive(Debug, Clone)]
pub struct ChunkInput {
    pub text: String,
    /// Page / sheet / slide number where known.
    pub page: Option<u32>,
}

#[derive(Debug, Clone)]
pub struct Hit {
    #[allow(dead_code)] // kept for de-duplication / future "open at chunk"
    pub chunk_id: i64,
    pub text: String,
    pub page: Option<u32>,
    pub path: String,
    /// Cosine similarity in [-1, 1]; higher is closer.
    pub score: f32,
}

pub struct Store {
    conn: Connection,
}

impl Store {
    pub fn open(path: &Path) -> Result<Self, String> {
        register_vec();
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let conn = Connection::open(path).map_err(e)?;
        conn.pragma_update(None, "journal_mode", "WAL").map_err(e)?;
        conn.pragma_update(None, "foreign_keys", "ON").map_err(e)?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS meta(
                 key   TEXT PRIMARY KEY,
                 value TEXT NOT NULL
             );
             CREATE TABLE IF NOT EXISTS folders(
                 id       INTEGER PRIMARY KEY,
                 path     TEXT NOT NULL UNIQUE,
                 added_at INTEGER NOT NULL,
                 status   TEXT NOT NULL DEFAULT 'idle'
             );
             CREATE TABLE IF NOT EXISTS files(
                 id         INTEGER PRIMARY KEY,
                 folder_id  INTEGER NOT NULL REFERENCES folders(id) ON DELETE CASCADE,
                 path       TEXT NOT NULL,
                 hash       TEXT NOT NULL,
                 mtime      INTEGER NOT NULL,
                 size       INTEGER NOT NULL,
                 kind       TEXT NOT NULL,
                 indexed_at INTEGER NOT NULL,
                 UNIQUE(folder_id, path)
             );
             CREATE INDEX IF NOT EXISTS idx_files_folder ON files(folder_id);
             CREATE TABLE IF NOT EXISTS chunks(
                 id      INTEGER PRIMARY KEY,
                 file_id INTEGER NOT NULL REFERENCES files(id) ON DELETE CASCADE,
                 ordinal INTEGER NOT NULL,
                 text    TEXT NOT NULL,
                 page    INTEGER
             );
             CREATE INDEX IF NOT EXISTS idx_chunks_file ON chunks(file_id);",
        )
        .map_err(e)?;
        Ok(Store { conn })
    }

    // ------------------------------ meta ---------------------------------

    fn meta_get(&self, key: &str) -> Result<Option<String>, String> {
        self.conn
            .query_row("SELECT value FROM meta WHERE key = ?1", [key], |r| r.get(0))
            .optional()
            .map_err(e)
    }

    fn meta_set(&self, key: &str, value: &str) -> Result<(), String> {
        self.conn
            .execute(
                "INSERT INTO meta(key, value) VALUES (?1, ?2)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                params![key, value],
            )
            .map(|_| ())
            .map_err(e)
    }

    /// Parsing improves between versions (scanned PDFs became readable, for
    /// one). When this index was built by parsers older than `version`,
    /// forget every file that yielded no text, so the next scan reads it
    /// again instead of skipping it as unchanged. Returns how many.
    pub fn upgrade_parsers(&self, version: u32) -> Result<usize, String> {
        let built_with: u32 = self
            .meta_get("parser_version")?
            .and_then(|v| v.parse().ok())
            .unwrap_or(1);
        if built_with >= version {
            return Ok(0);
        }
        let forgotten = self
            .conn
            .execute("DELETE FROM files WHERE id NOT IN (SELECT DISTINCT file_id FROM chunks)", [])
            .map_err(e)?;
        self.meta_set("parser_version", &version.to_string())?;
        Ok(forgotten)
    }

    /// Forget every indexed file of one kind ("image", "pdf"…), so the next
    /// scan reads them again. Returns how many.
    pub fn forget_kind(&mut self, kind: &str) -> Result<usize, String> {
        let files: Vec<(i64, String)> = {
            let mut stmt = self
                .conn
                .prepare("SELECT folder_id, path FROM files WHERE kind = ?1")
                .map_err(e)?;
            let rows = stmt
                .query_map([kind], |r| Ok((r.get(0)?, r.get(1)?)))
                .map_err(e)?
                .collect::<Result<_, _>>()
                .map_err(e)?;
            rows
        };
        for (folder_id, path) in &files {
            self.delete_file(*folder_id, path)?;
        }
        Ok(files.len())
    }

    fn has_vec_table(&self) -> Result<bool, String> {
        self.conn
            .query_row(
                "SELECT 1 FROM sqlite_master WHERE name = 'vec_chunks'",
                [],
                |_| Ok(()),
            )
            .optional()
            .map(|o| o.is_some())
            .map_err(e)
    }

    /// The embedding model id + dimension the current index was built with.
    pub fn embedding_space(&self) -> Result<Option<(String, usize)>, String> {
        let model = self.meta_get("embed_model")?;
        let dim = self.meta_get("embed_dim")?.and_then(|d| d.parse().ok());
        Ok(match (model, dim) {
            (Some(m), Some(d)) => Some((m, d)),
            _ => None,
        })
    }

    /// Make sure the vector table matches `model_id` / `dim`. If the index was
    /// built with a different model, all indexed data is dropped (folders are
    /// kept and marked idle) and `true` is returned so callers can re-index.
    pub fn ensure_embedding_space(&mut self, model_id: &str, dim: usize) -> Result<bool, String> {
        if dim == 0 {
            return Err("embedding dimension must be > 0".into());
        }
        let current = self.embedding_space()?;
        let matches = current
            .as_ref()
            .map(|(m, d)| m == model_id && *d == dim)
            .unwrap_or(false);
        if matches && self.has_vec_table()? {
            return Ok(false);
        }

        let had_data = current.is_some();
        let tx = self.conn.transaction().map_err(e)?;
        tx.execute_batch(
            // 'idle' folders had their index deleted on purpose; leave them be.
            "DROP TABLE IF EXISTS vec_chunks;
             DELETE FROM files;
             UPDATE folders SET status = 'pending' WHERE status != 'idle';",
        )
        .map_err(e)?;
        tx.execute_batch(&format!(
            "CREATE VIRTUAL TABLE vec_chunks USING vec0(
                 chunk_id  INTEGER PRIMARY KEY,
                 embedding FLOAT[{dim}] distance_metric=cosine
             );"
        ))
        .map_err(e)?;
        tx.commit().map_err(e)?;
        self.meta_set("embed_model", model_id)?;
        self.meta_set("embed_dim", &dim.to_string())?;
        Ok(had_data)
    }

    // ----------------------------- folders -------------------------------

    /// Add a folder (idempotent on path). Returns its id.
    pub fn add_folder(&self, path: &str) -> Result<i64, String> {
        self.conn
            .execute(
                "INSERT OR IGNORE INTO folders(path, added_at, status) VALUES (?1, ?2, 'pending')",
                params![path, now_secs()],
            )
            .map_err(e)?;
        self.conn
            .query_row("SELECT id FROM folders WHERE path = ?1", [path], |r| r.get(0))
            .map_err(e)
    }

    pub fn folder(&self, id: i64) -> Result<Option<FolderRow>, String> {
        Ok(self.list_folders()?.into_iter().find(|f| f.id == id))
    }

    pub fn list_folders(&self) -> Result<Vec<FolderRow>, String> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT f.id, f.path, f.status,
                        (SELECT COUNT(*) FROM files fi WHERE fi.folder_id = f.id),
                        (SELECT COUNT(*) FROM chunks c
                           JOIN files fi ON fi.id = c.file_id
                          WHERE fi.folder_id = f.id)
                   FROM folders f
                  ORDER BY f.added_at, f.id",
            )
            .map_err(e)?;
        let rows = stmt
            .query_map([], |r| {
                Ok(FolderRow {
                    id: r.get(0)?,
                    path: r.get(1)?,
                    status: r.get(2)?,
                    file_count: r.get::<_, i64>(3)? as u32,
                    chunk_count: r.get::<_, i64>(4)? as u32,
                })
            })
            .map_err(e)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(e)
    }

    pub fn set_folder_status(&self, id: i64, status: &str) -> Result<(), String> {
        self.conn
            .execute(
                "UPDATE folders SET status = ?2 WHERE id = ?1",
                params![id, status],
            )
            .map(|_| ())
            .map_err(e)
    }

    /// Drop a folder's indexed data but keep the folder registered.
    pub fn delete_folder_index(&mut self, id: i64) -> Result<(), String> {
        let has_vec = self.has_vec_table()?;
        let tx = self.conn.transaction().map_err(e)?;
        if has_vec {
            delete_vectors(
                &tx,
                "SELECT c.id FROM chunks c JOIN files fi ON fi.id = c.file_id WHERE fi.folder_id = ?1",
                id,
            )?;
        }
        tx.execute("DELETE FROM files WHERE folder_id = ?1", [id])
            .map_err(e)?;
        tx.execute("UPDATE folders SET status = 'idle' WHERE id = ?1", [id])
            .map_err(e)?;
        tx.commit().map_err(e)
    }

    /// Remove a folder and everything indexed from it.
    pub fn remove_folder(&mut self, id: i64) -> Result<(), String> {
        self.delete_folder_index(id)?;
        self.conn
            .execute("DELETE FROM folders WHERE id = ?1", [id])
            .map(|_| ())
            .map_err(e)
    }

    // ------------------------------ files --------------------------------

    pub fn file_record(&self, folder_id: i64, path: &str) -> Result<Option<FileRecord>, String> {
        self.conn
            .query_row(
                "SELECT hash, mtime, size FROM files WHERE folder_id = ?1 AND path = ?2",
                params![folder_id, path],
                |r| {
                    Ok(FileRecord {
                        hash: r.get(0)?,
                        mtime: r.get(1)?,
                        size: r.get(2)?,
                    })
                },
            )
            .optional()
            .map_err(e)
    }

    /// Record a new mtime/size for a file whose content hash did not change.
    pub fn touch_file(&self, folder_id: i64, path: &str, mtime: i64, size: i64) -> Result<(), String> {
        self.conn
            .execute(
                "UPDATE files SET mtime = ?3, size = ?4 WHERE folder_id = ?1 AND path = ?2",
                params![folder_id, path, mtime, size],
            )
            .map(|_| ())
            .map_err(e)
    }

    /// Paths of every indexed file in a folder (to detect deletions).
    pub fn file_paths(&self, folder_id: i64) -> Result<Vec<String>, String> {
        let mut stmt = self
            .conn
            .prepare("SELECT path FROM files WHERE folder_id = ?1")
            .map_err(e)?;
        let rows = stmt.query_map([folder_id], |r| r.get(0)).map_err(e)?;
        rows.collect::<Result<Vec<String>, _>>().map_err(e)
    }

    /// True when `path` is a file currently in the index.
    /// The content hash of the indexed file at `path`, if it is indexed.
    /// (Chat history records it with a citation, to tell later whether the
    /// file is still the one that was cited.)
    pub fn file_hash(&self, path: &str) -> Result<Option<String>, String> {
        self.conn
            .query_row("SELECT hash FROM files WHERE path = ?1 LIMIT 1", [path], |r| r.get(0))
            .optional()
            .map_err(e)
    }

    pub fn is_indexed(&self, path: &str) -> Result<bool, String> {
        self.conn
            .query_row("SELECT 1 FROM files WHERE path = ?1 LIMIT 1", [path], |_| Ok(()))
            .optional()
            .map(|o| o.is_some())
            .map_err(e)
    }

    /// Remove one file's chunks + vectors.
    pub fn delete_file(&mut self, folder_id: i64, path: &str) -> Result<(), String> {
        let has_vec = self.has_vec_table()?;
        let tx = self.conn.transaction().map_err(e)?;
        delete_file_tx(&tx, has_vec, folder_id, path)?;
        tx.commit().map_err(e)
    }

    /// Insert (or replace) a file with its chunks and matching embeddings.
    pub fn replace_file(
        &mut self,
        folder_id: i64,
        path: &str,
        record: &FileRecord,
        kind: &str,
        chunks: &[ChunkInput],
        embeddings: &[Vec<f32>],
    ) -> Result<(), String> {
        if chunks.len() != embeddings.len() {
            return Err(format!(
                "chunk/embedding count mismatch: {} vs {}",
                chunks.len(),
                embeddings.len()
            ));
        }
        let dim = match self.embedding_space()? {
            Some((_, d)) => d,
            None => return Err("vector index not initialised (no embedding model set)".into()),
        };
        if let Some(bad) = embeddings.iter().find(|v| v.len() != dim) {
            return Err(format!(
                "embedding has {} dims, index expects {dim}",
                bad.len()
            ));
        }

        let tx = self.conn.transaction().map_err(e)?;
        delete_file_tx(&tx, true, folder_id, path)?;
        tx.execute(
            "INSERT INTO files(folder_id, path, hash, mtime, size, kind, indexed_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                folder_id,
                path,
                record.hash,
                record.mtime,
                record.size,
                kind,
                now_secs()
            ],
        )
        .map_err(e)?;
        let file_id = tx.last_insert_rowid();
        {
            let mut ins_chunk = tx
                .prepare("INSERT INTO chunks(file_id, ordinal, text, page) VALUES (?1, ?2, ?3, ?4)")
                .map_err(e)?;
            let mut ins_vec = tx
                .prepare("INSERT INTO vec_chunks(chunk_id, embedding) VALUES (?1, ?2)")
                .map_err(e)?;
            for (i, (chunk, emb)) in chunks.iter().zip(embeddings).enumerate() {
                ins_chunk
                    .execute(params![file_id, i as i64, chunk.text, chunk.page])
                    .map_err(e)?;
                let chunk_id = tx.last_insert_rowid();
                ins_vec
                    .execute(params![chunk_id, f32_bytes(emb)])
                    .map_err(e)?;
            }
        }
        tx.commit().map_err(e)
    }

    // ------------------------------ search -------------------------------

    /// K-nearest chunks to `query` by cosine similarity, best first.
    pub fn search(&self, query: &[f32], k: usize) -> Result<Vec<Hit>, String> {
        if k == 0 || !self.has_vec_table()? {
            return Ok(Vec::new());
        }
        let mut stmt = self
            .conn
            .prepare(
                "WITH knn AS (
                     SELECT chunk_id, distance
                       FROM vec_chunks
                      WHERE embedding MATCH ?1 AND k = ?2
                 )
                 SELECT c.id, c.text, c.page, fi.path, knn.distance
                   FROM knn
                   JOIN chunks c ON c.id = knn.chunk_id
                   JOIN files fi ON fi.id = c.file_id
                  ORDER BY knn.distance",
            )
            .map_err(e)?;
        let rows = stmt
            .query_map(params![f32_bytes(query), k as i64], |r| {
                let distance: f64 = r.get(4)?;
                Ok(Hit {
                    chunk_id: r.get(0)?,
                    text: r.get(1)?,
                    page: r.get::<_, Option<i64>>(2)?.map(|p| p as u32),
                    path: r.get(3)?,
                    score: 1.0 - distance as f32,
                })
            })
            .map_err(e)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(e)
    }

    pub fn chunk_count(&self) -> Result<u32, String> {
        self.conn
            .query_row("SELECT COUNT(*) FROM chunks", [], |r| r.get::<_, i64>(0))
            .map(|n| n as u32)
            .map_err(e)
    }
}

/// Delete the vec0 rows for every chunk id returned by `select_ids_sql` (one
/// integer parameter). vec0 rows are removed one by one by primary key.
fn delete_vectors(tx: &rusqlite::Transaction, select_ids_sql: &str, param: i64) -> Result<(), String> {
    let ids: Vec<i64> = {
        let mut stmt = tx.prepare(select_ids_sql).map_err(e)?;
        let rows = stmt.query_map([param], |r| r.get(0)).map_err(e)?;
        rows.collect::<Result<Vec<i64>, _>>().map_err(e)?
    };
    let mut del = tx
        .prepare("DELETE FROM vec_chunks WHERE chunk_id = ?1")
        .map_err(e)?;
    for id in ids {
        del.execute([id]).map_err(e)?;
    }
    Ok(())
}

fn delete_file_tx(
    tx: &rusqlite::Transaction,
    has_vec: bool,
    folder_id: i64,
    path: &str,
) -> Result<(), String> {
    let file_id: Option<i64> = tx
        .query_row(
            "SELECT id FROM files WHERE folder_id = ?1 AND path = ?2",
            params![folder_id, path],
            |r| r.get(0),
        )
        .optional()
        .map_err(e)?;
    if let Some(file_id) = file_id {
        if has_vec {
            delete_vectors(tx, "SELECT id FROM chunks WHERE file_id = ?1", file_id)?;
        }
        tx.execute("DELETE FROM files WHERE id = ?1", [file_id])
            .map_err(e)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn better_parsers_get_another_go_at_empty_files() {
        let path = temp_db("parsers");
        let mut store = Store::open(&path).unwrap();
        // A fresh index is simply stamped with the current version.
        assert_eq!(store.upgrade_parsers(2).unwrap(), 0);
        store.meta_set("parser_version", "1").unwrap(); // as if built before OCR

        store.ensure_embedding_space("test-model", 2).unwrap();
        let folder = store.add_folder("/docs").unwrap();
        let record = FileRecord { hash: "h".into(), mtime: 1, size: 1 };
        let chunk = ChunkInput { text: "typed text".into(), page: Some(1) };
        store.replace_file(folder, "/docs/typed.pdf", &record, "pdf", &[chunk], &[vec![0.1, 0.2]]).unwrap();
        store.replace_file(folder, "/docs/scan.pdf", &record, "pdf", &[], &[]).unwrap();
        assert_eq!(store.file_paths(folder).unwrap().len(), 2);

        // Only the file that yielded nothing is forgotten, and only once.
        assert_eq!(store.upgrade_parsers(2).unwrap(), 1);
        assert_eq!(store.file_paths(folder).unwrap(), vec!["/docs/typed.pdf".to_string()]);
        assert!(store.file_record(folder, "/docs/scan.pdf").unwrap().is_none());
        assert_eq!(store.upgrade_parsers(2).unwrap(), 0);

        // Forgetting a kind drops those files (and their chunks) only.
        let picture = ChunkInput { text: "a chart".into(), page: None };
        store.replace_file(folder, "/docs/chart.png", &record, "image", &[picture], &[vec![0.3, 0.4]]).unwrap();
        assert_eq!(store.chunk_count().unwrap(), 2);
        assert_eq!(store.forget_kind("image").unwrap(), 1);
        assert_eq!(store.file_paths(folder).unwrap(), vec!["/docs/typed.pdf".to_string()]);
        assert_eq!(store.chunk_count().unwrap(), 1);
        assert_eq!(store.forget_kind("image").unwrap(), 0);

        drop(store);
        let _ = std::fs::remove_file(&path);
    }

    fn temp_db(name: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!(
            "loft-test-{name}-{}-{}.db",
            std::process::id(),
            now_secs()
        ));
        let _ = std::fs::remove_file(&p);
        p
    }

    fn rec(hash: &str) -> FileRecord {
        FileRecord {
            hash: hash.into(),
            mtime: 1,
            size: 1,
        }
    }

    fn chunk(text: &str) -> ChunkInput {
        ChunkInput {
            text: text.into(),
            page: None,
        }
    }

    #[test]
    fn store_roundtrip_with_synthetic_vectors() {
        let path = temp_db("roundtrip");
        let mut store = Store::open(&path).unwrap();

        // Fresh index: nothing to reset.
        assert!(!store.ensure_embedding_space("model-a", 4).unwrap());
        assert!(!store.ensure_embedding_space("model-a", 4).unwrap());

        let folder = store.add_folder("/docs").unwrap();
        assert_eq!(store.add_folder("/docs").unwrap(), folder, "idempotent");

        store
            .replace_file(
                folder,
                "/docs/a.md",
                &rec("h1"),
                "text",
                &[chunk("alpha"), chunk("beta"), chunk("gamma")],
                &[
                    vec![1.0, 0.0, 0.0, 0.0],
                    vec![0.0, 1.0, 0.0, 0.0],
                    vec![0.0, 0.0, 1.0, 0.0],
                ],
            )
            .unwrap();

        // Nearest neighbour is the chunk pointing the same way.
        let hits = store.search(&[0.9, 0.1, 0.0, 0.0], 2).unwrap();
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].text, "alpha");
        assert_eq!(hits[0].path, "/docs/a.md");
        assert!(hits[0].score > hits[1].score);
        assert!(hits[0].score > 0.9);

        // Incremental bookkeeping.
        assert_eq!(store.file_record(folder, "/docs/a.md").unwrap(), Some(rec("h1")));
        assert_eq!(store.file_record(folder, "/docs/missing.md").unwrap(), None);

        // Replacing a file swaps its chunks + vectors.
        store
            .replace_file(
                folder,
                "/docs/a.md",
                &rec("h2"),
                "text",
                &[chunk("delta")],
                &[vec![0.0, 0.0, 0.0, 1.0]],
            )
            .unwrap();
        assert_eq!(store.chunk_count().unwrap(), 1);
        let hits = store.search(&[1.0, 0.0, 0.0, 0.0], 5).unwrap();
        assert_eq!(hits.len(), 1, "old vectors are gone");
        assert_eq!(hits[0].text, "delta");

        let f = store.folder(folder).unwrap().unwrap();
        assert_eq!((f.file_count, f.chunk_count), (1, 1));

        // Wrong dimension is rejected.
        assert!(store
            .replace_file(folder, "/docs/b.md", &rec("h"), "text", &[chunk("x")], &[vec![1.0, 2.0]])
            .is_err());

        // Deleting the folder's index empties search but keeps the folder.
        store.delete_folder_index(folder).unwrap();
        assert_eq!(store.chunk_count().unwrap(), 0);
        assert!(store.search(&[1.0, 0.0, 0.0, 0.0], 5).unwrap().is_empty());
        assert_eq!(store.list_folders().unwrap().len(), 1);

        // Switching embedding model resets the index and reports it.
        store
            .replace_file(folder, "/docs/a.md", &rec("h3"), "text", &[chunk("e")], &[vec![1.0, 0.0, 0.0, 0.0]])
            .unwrap();
        assert!(store.ensure_embedding_space("model-b", 8).unwrap());
        assert_eq!(store.chunk_count().unwrap(), 0);
        assert_eq!(store.embedding_space().unwrap(), Some(("model-b".into(), 8)));

        store.remove_folder(folder).unwrap();
        assert!(store.list_folders().unwrap().is_empty());

        drop(store);
        let _ = std::fs::remove_file(&path);
    }

    fn app_db() -> PathBuf {
        PathBuf::from(std::env::var("APPDATA").unwrap_or_default())
            .join("com.loft.desktop")
            .join("loft.db")
    }

    /// Dev helper: print the folders in the real app database.
    ///   <test exe> --ignored --exact store::tests::dev_list_folders --nocapture
    #[test]
    #[ignore = "dev helper: reads the real app database"]
    fn dev_list_folders() {
        let store = Store::open(&app_db()).unwrap();
        for f in store.list_folders().unwrap() {
            println!(
                "folder {} | {} | status={} files={} chunks={}",
                f.id, f.path, f.status, f.file_count, f.chunk_count
            );
        }
        println!("embedding space: {:?}", store.embedding_space().unwrap());
    }

    /// Dev helper: register the folder in LOFT_SEED_FOLDER with the real app
    /// database (same as "Add folder" in the UI, minus the dialog).
    #[test]
    #[ignore = "dev helper: writes to the real app database"]
    fn dev_seed_folder() {
        let folder = std::env::var("LOFT_SEED_FOLDER").expect("set LOFT_SEED_FOLDER");
        let store = Store::open(&app_db()).unwrap();
        let id = store.add_folder(&folder).unwrap();
        println!("seeded folder {id}: {folder}");
    }

    /// Real semantic retrieval: embed documents with the Nomic server, store
    /// them, and check a natural-language query finds the right one. Run with:
    ///   cargo test --lib store::tests::semantic_retrieval -- --ignored --nocapture
    #[tokio::test]
    #[ignore = "requires the Nomic model + bundled llama-server"]
    async fn semantic_retrieval() {
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
            .spawn(ServerKind::Embed, &model, "nomic-embed-text-v1.5", 2048)
            .expect("spawn embed server");
        assert!(wait_healthy(port, std::time::Duration::from_secs(60)).await);

        let docs = [
            "The quarterly budget review is scheduled for the first Monday of March in the finance office.",
            "To reset the router, hold the recessed button for ten seconds until the light blinks amber.",
            "Our sourdough starter needs feeding twice a day with equal parts flour and water.",
            "The indexer hashes every file with blake3 so unchanged files are skipped on re-index.",
        ];
        let doc_inputs: Vec<String> = docs.iter().map(|d| format!("search_document: {d}")).collect();
        let doc_vecs = crate::embed::embed(port, &doc_inputs).await.expect("embed docs");
        let dim = doc_vecs[0].len();
        eprintln!("embedded {} docs, dim = {dim}", doc_vecs.len());

        let path = temp_db("semantic");
        let mut store = Store::open(&path).unwrap();
        store.ensure_embedding_space("nomic-embed-text-v1.5", dim).unwrap();
        let folder = store.add_folder("/kb").unwrap();
        let chunks: Vec<ChunkInput> = docs.iter().map(|d| chunk(d)).collect();
        store
            .replace_file(folder, "/kb/notes.md", &rec("h"), "text", &chunks, &doc_vecs)
            .unwrap();

        for (question, expect) in [
            ("How do I restart my wifi box?", "router"),
            ("when is the finance meeting", "budget"),
            ("how does loft avoid re-embedding files that did not change", "blake3"),
        ] {
            let q = crate::embed::embed_one(port, &format!("search_query: {question}"))
                .await
                .expect("embed query");
            let hits = store.search(&q, 2).unwrap();
            eprintln!("Q: {question}\n   top: {:.3}  {}", hits[0].score, hits[0].text);
            assert!(
                hits[0].text.contains(expect),
                "expected top hit to mention {expect:?}, got {:?}",
                hits[0].text
            );
        }

        drop(store);
        let _ = std::fs::remove_file(&path);
    }
}
