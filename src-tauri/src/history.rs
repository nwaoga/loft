//! Chat history: conversations, their messages, and the sources each answer
//! was written from, kept in `chats.db` (SQLite) beside the index.
//!
//! History is storage. It is not what the model sees: the UI shows a saved
//! conversation in full, while each new question is sent with only the last
//! few turns, trimmed to a budget (`rag::recent_history`). Nothing here
//! builds a prompt.
//!
//! It has its own database file, so deleting or rebuilding an index (or
//! changing the embedding model, which drops every vector) cannot touch a
//! conversation, and deleting a conversation cannot touch a file or its
//! index.
//!
//! An answer is written in two steps. When a question is asked, the question
//! is saved at once and the answer gets a row with the status `pending`.
//! When the answer ends, that row is filled in as `complete`, `stopped`
//! (the user stopped it) or `failed`. A row still `pending` when Loft next
//! starts was cut off by Loft closing, and becomes `interrupted`. So text
//! that is not a finished answer is never stored as one.
//!
//! Free of Tauri types; tested directly against real database files.

use std::path::Path;

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

fn e<T: std::fmt::Display>(err: T) -> String {
    err.to_string()
}

/// Milliseconds since 1970, UTC.
pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// How an assistant message ended.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    /// Being written right now.
    Pending,
    /// Finished normally.
    Complete,
    /// The user stopped it; what had been written is kept, marked as such.
    Stopped,
    /// Generation failed; `error` says why.
    Failed,
    /// Loft closed while it was being written.
    Interrupted,
}

impl Status {
    fn as_str(self) -> &'static str {
        match self {
            Status::Pending => "pending",
            Status::Complete => "complete",
            Status::Stopped => "stopped",
            Status::Failed => "failed",
            Status::Interrupted => "interrupted",
        }
    }

    fn parse(s: &str) -> Status {
        match s {
            "pending" => Status::Pending,
            "stopped" => Status::Stopped,
            "failed" => Status::Failed,
            "interrupted" => Status::Interrupted,
            _ => Status::Complete,
        }
    }
}

/// A conversation, as listed.
#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Conversation {
    pub id: String,
    pub title: String,
    /// The user named it; an automatic title must not replace this.
    pub title_manual: bool,
    /// Milliseconds since 1970, UTC.
    pub created_at: i64,
    pub updated_at: i64,
    pub opened_at: Option<i64>,
    pub pinned: bool,
    pub archived: bool,
    /// Questions asked in it.
    pub questions: u32,
}

/// Where an answer came from, as it was when the answer was written.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SourceRecord {
    /// Display name of the file.
    pub file: String,
    pub path: String,
    pub page: Option<u32>,
    /// "page 3" / "slide 2" / "sheet 1" where known.
    pub location: Option<String>,
    pub score: f32,
    pub snippet: String,
    /// Hash of the file's contents when it was cited, when known.
    #[serde(default)]
    pub file_hash: Option<String>,
}

/// One saved message.
#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Message {
    pub id: String,
    pub conversation_id: String,
    /// "user" or "assistant".
    pub role: String,
    pub content: String,
    pub created_at: i64,
    pub status: Status,
    /// The chat model that wrote it (assistant messages).
    pub model: Option<String>,
    /// Context usage, as the answer pipeline reported it (JSON).
    pub stats: Option<serde_json::Value>,
    pub error: Option<String>,
    /// In the order they were shown.
    pub sources: Vec<SourceRecord>,
}

/// A short title from the first question: tidied, without its question
/// mark, and cut at a word. Deterministic: no model is asked, so the first
/// answer is not delayed.
pub fn title_from(question: &str) -> String {
    const MAX: usize = 48;
    let tidy = question.split_whitespace().collect::<Vec<_>>().join(" ");
    let tidy = tidy.trim_end_matches(|c: char| matches!(c, '?' | '.' | '!' | ':' | ' ')).to_string();
    if tidy.is_empty() {
        return "New conversation".to_string();
    }
    let mut title = if tidy.chars().count() <= MAX {
        tidy
    } else {
        let cut: String = tidy.chars().take(MAX).collect();
        // Back to the last whole word (unless the cut fell between two words
        // anyway, or that would leave too little).
        let between_words = tidy.chars().nth(MAX) == Some(' ');
        let whole = match cut.rfind(' ') {
            Some(at) if !between_words && at >= MAX / 2 => cut[..at].to_string(),
            _ => cut,
        };
        format!("{}…", whole.trim_end_matches(|c: char| matches!(c, ',' | ';' | ':' | '-' | ' ')))
    };
    if let Some(first) = title.chars().next() {
        let upper: String = first.to_uppercase().collect();
        title.replace_range(..first.len_utf8(), &upper);
    }
    title
}

/// Escape `%`, `_` and `\` so user text is matched literally by LIKE.
fn like_pattern(text: &str) -> String {
    let mut pattern = String::with_capacity(text.len() + 2);
    pattern.push('%');
    for c in text.chars() {
        if matches!(c, '%' | '_' | '\\') {
            pattern.push('\\');
        }
        pattern.push(c);
    }
    pattern.push('%');
    pattern
}

pub struct History {
    conn: Connection,
}

impl History {
    pub fn open(path: &Path) -> Result<Self, String> {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let conn = Connection::open(path).map_err(e)?;
        conn.pragma_update(None, "journal_mode", "WAL").map_err(e)?;
        conn.pragma_update(None, "foreign_keys", "ON").map_err(e)?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS conversations(
                 id           TEXT PRIMARY KEY,
                 title        TEXT NOT NULL,
                 title_manual INTEGER NOT NULL DEFAULT 0,
                 created_at   INTEGER NOT NULL,
                 updated_at   INTEGER NOT NULL,
                 opened_at    INTEGER,
                 pinned       INTEGER NOT NULL DEFAULT 0,
                 archived     INTEGER NOT NULL DEFAULT 0
             );
             CREATE INDEX IF NOT EXISTS idx_conversations_updated ON conversations(updated_at);
             CREATE TABLE IF NOT EXISTS messages(
                 id              TEXT PRIMARY KEY,
                 conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
                 ordinal         INTEGER NOT NULL,
                 role            TEXT NOT NULL,
                 content         TEXT NOT NULL,
                 created_at      INTEGER NOT NULL,
                 status          TEXT NOT NULL,
                 model           TEXT,
                 stats           TEXT,
                 error           TEXT,
                 UNIQUE(conversation_id, ordinal)
             );
             CREATE TABLE IF NOT EXISTS message_sources(
                 message_id TEXT NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
                 ordinal    INTEGER NOT NULL,
                 file       TEXT NOT NULL,
                 path       TEXT NOT NULL,
                 page       INTEGER,
                 location   TEXT,
                 score      REAL NOT NULL,
                 snippet    TEXT NOT NULL,
                 file_hash  TEXT,
                 PRIMARY KEY(message_id, ordinal)
             );",
        )
        .map_err(e)?;
        Ok(History { conn })
    }

    /// A new GUID, in the usual 8-4-4-4-12 form.
    fn new_id(&self) -> Result<String, String> {
        let hex: String = self.conn.query_row("SELECT lower(hex(randomblob(16)))", [], |r| r.get(0)).map_err(e)?;
        Ok(format!("{}-{}-{}-{}-{}", &hex[0..8], &hex[8..12], &hex[12..16], &hex[16..20], &hex[20..32]))
    }

    /// Answers left `pending` by a Loft that closed while writing them are
    /// not answers: mark them. Returns how many there were.
    pub fn mark_interrupted(&self) -> Result<usize, String> {
        self.conn
            .execute("UPDATE messages SET status = 'interrupted' WHERE status = 'pending'", [])
            .map_err(e)
    }

    /// Start a conversation, titled after its first question.
    pub fn create(&self, first_question: &str, now: i64) -> Result<Conversation, String> {
        let id = self.new_id()?;
        let title = title_from(first_question);
        self.conn
            .execute(
                "INSERT INTO conversations(id, title, created_at, updated_at, opened_at) VALUES (?1, ?2, ?3, ?3, ?3)",
                params![id, title, now],
            )
            .map_err(e)?;
        self.conversation(&id)?.ok_or_else(|| "the conversation could not be saved".to_string())
    }

    pub fn conversation(&self, id: &str) -> Result<Option<Conversation>, String> {
        self.conn
            .query_row(
                "SELECT c.id, c.title, c.title_manual, c.created_at, c.updated_at, c.opened_at, c.pinned, c.archived,
                        (SELECT count(*) FROM messages m WHERE m.conversation_id = c.id AND m.role = 'user')
                 FROM conversations c WHERE c.id = ?1",
                [id],
                Self::row_to_conversation,
            )
            .optional()
            .map_err(e)
    }

    fn row_to_conversation(r: &rusqlite::Row) -> rusqlite::Result<Conversation> {
        Ok(Conversation {
            id: r.get(0)?,
            title: r.get(1)?,
            title_manual: r.get::<_, i64>(2)? != 0,
            created_at: r.get(3)?,
            updated_at: r.get(4)?,
            opened_at: r.get(5)?,
            pinned: r.get::<_, i64>(6)? != 0,
            archived: r.get::<_, i64>(7)? != 0,
            questions: r.get(8)?,
        })
    }

    /// Conversations, most recently updated first. With `query`, only those
    /// whose title or any message contains it (case-insensitively).
    pub fn list(&self, query: Option<&str>) -> Result<Vec<Conversation>, String> {
        let query = query.map(str::trim).filter(|q| !q.is_empty());
        let select = "SELECT c.id, c.title, c.title_manual, c.created_at, c.updated_at, c.opened_at, c.pinned, c.archived,
                             (SELECT count(*) FROM messages m WHERE m.conversation_id = c.id AND m.role = 'user')
                      FROM conversations c";
        let order = " ORDER BY c.updated_at DESC, c.created_at DESC";
        let rows = match query {
            None => {
                let mut stmt = self.conn.prepare(&format!("{select} WHERE c.archived = 0{order}")).map_err(e)?;
                let rows = stmt.query_map([], Self::row_to_conversation).map_err(e)?;
                rows.collect::<rusqlite::Result<Vec<_>>>().map_err(e)?
            }
            Some(text) => {
                let pattern = like_pattern(text);
                let mut stmt = self
                    .conn
                    .prepare(&format!(
                        "{select} WHERE c.archived = 0 AND (c.title LIKE ?1 ESCAPE '\\'
                            OR EXISTS (SELECT 1 FROM messages m WHERE m.conversation_id = c.id AND m.content LIKE ?1 ESCAPE '\\')){order}"
                    ))
                    .map_err(e)?;
                let rows = stmt.query_map([pattern], Self::row_to_conversation).map_err(e)?;
                rows.collect::<rusqlite::Result<Vec<_>>>().map_err(e)?
            }
        };
        Ok(rows)
    }

    /// Note that a conversation was opened.
    pub fn opened(&self, id: &str, now: i64) -> Result<(), String> {
        self.conn.execute("UPDATE conversations SET opened_at = ?2 WHERE id = ?1", params![id, now]).map(|_| ()).map_err(e)
    }

    /// Give a conversation a title of the user's choosing. It is never
    /// replaced by an automatic one afterwards.
    pub fn rename(&self, id: &str, title: &str) -> Result<Conversation, String> {
        let title = title.split_whitespace().collect::<Vec<_>>().join(" ");
        if title.is_empty() {
            return Err("A conversation needs a name.".into());
        }
        let title: String = title.chars().take(120).collect();
        let changed = self
            .conn
            .execute("UPDATE conversations SET title = ?2, title_manual = 1 WHERE id = ?1", params![id, title])
            .map_err(e)?;
        if changed == 0 {
            return Err("That conversation no longer exists.".into());
        }
        self.conversation(id)?.ok_or_else(|| "That conversation no longer exists.".to_string())
    }

    /// Delete a conversation with its messages and their source records.
    /// Nothing outside this database is touched.
    pub fn delete(&self, id: &str) -> Result<bool, String> {
        self.conn.execute("DELETE FROM conversations WHERE id = ?1", [id]).map(|n| n > 0).map_err(e)
    }

    fn next_ordinal(tx: &rusqlite::Transaction, conversation: &str) -> Result<i64, String> {
        tx.query_row("SELECT coalesce(max(ordinal), -1) + 1 FROM messages WHERE conversation_id = ?1", [conversation], |r| r.get(0))
            .map_err(e)
    }

    /// A question was asked: save it, and open a `pending` row for the
    /// answer. Returns (question message id, answer message id).
    pub fn begin_turn(&mut self, conversation: &str, question: &str, model: Option<&str>, now: i64) -> Result<(String, String), String> {
        let (user_id, answer_id) = (self.new_id()?, self.new_id()?);
        let tx = self.conn.transaction().map_err(e)?;
        let known: Option<i64> = tx.query_row("SELECT 1 FROM conversations WHERE id = ?1", [conversation], |r| r.get(0)).optional().map_err(e)?;
        if known.is_none() {
            return Err("That conversation no longer exists.".into());
        }
        let ordinal = Self::next_ordinal(&tx, conversation)?;
        tx.execute(
            "INSERT INTO messages(id, conversation_id, ordinal, role, content, created_at, status) VALUES (?1, ?2, ?3, 'user', ?4, ?5, 'complete')",
            params![user_id, conversation, ordinal, question, now],
        )
        .map_err(e)?;
        tx.execute(
            "INSERT INTO messages(id, conversation_id, ordinal, role, content, created_at, status, model) VALUES (?1, ?2, ?3, 'assistant', '', ?4, 'pending', ?5)",
            params![answer_id, conversation, ordinal + 1, now, model],
        )
        .map_err(e)?;
        tx.execute("UPDATE conversations SET updated_at = ?2 WHERE id = ?1", params![conversation, now]).map_err(e)?;
        tx.commit().map_err(e)?;
        Ok((user_id, answer_id))
    }

    /// The answer ended (in whichever way): fill its row in.
    #[allow(clippy::too_many_arguments)]
    pub fn finish_turn(
        &mut self,
        answer_id: &str,
        status: Status,
        content: &str,
        stats: Option<&serde_json::Value>,
        error: Option<&str>,
        sources: &[SourceRecord],
        now: i64,
    ) -> Result<(), String> {
        if status == Status::Pending {
            return Err("an answer cannot be finished as pending".into());
        }
        let tx = self.conn.transaction().map_err(e)?;
        let stats = stats.map(|s| s.to_string());
        let changed = tx
            .execute(
                "UPDATE messages SET content = ?2, status = ?3, stats = ?4, error = ?5 WHERE id = ?1 AND role = 'assistant'",
                params![answer_id, content, status.as_str(), stats, error],
            )
            .map_err(e)?;
        if changed == 0 {
            return Err("the answer's place in the history is gone (its conversation was deleted)".into());
        }
        tx.execute("DELETE FROM message_sources WHERE message_id = ?1", [answer_id]).map_err(e)?;
        for (ordinal, s) in sources.iter().enumerate() {
            tx.execute(
                "INSERT INTO message_sources(message_id, ordinal, file, path, page, location, score, snippet, file_hash)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![answer_id, ordinal as i64, s.file, s.path, s.page, s.location, s.score as f64, s.snippet, s.file_hash],
            )
            .map_err(e)?;
        }
        tx.execute(
            "UPDATE conversations SET updated_at = ?2 WHERE id = (SELECT conversation_id FROM messages WHERE id = ?1)",
            params![answer_id, now],
        )
        .map_err(e)?;
        tx.commit().map_err(e)
    }

    /// Every message of a conversation, in the order they were written,
    /// each answer with its sources in the order they were shown.
    pub fn messages(&self, conversation: &str) -> Result<Vec<Message>, String> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT id, conversation_id, role, content, created_at, status, model, stats, error
                 FROM messages WHERE conversation_id = ?1 ORDER BY ordinal",
            )
            .map_err(e)?;
        let mut messages: Vec<Message> = stmt
            .query_map([conversation], |r| {
                Ok(Message {
                    id: r.get(0)?,
                    conversation_id: r.get(1)?,
                    role: r.get(2)?,
                    content: r.get(3)?,
                    created_at: r.get(4)?,
                    status: Status::parse(&r.get::<_, String>(5)?),
                    model: r.get(6)?,
                    stats: r.get::<_, Option<String>>(7)?.and_then(|s| serde_json::from_str(&s).ok()),
                    error: r.get(8)?,
                    sources: Vec::new(),
                })
            })
            .map_err(e)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(e)?;
        let mut sources = self
            .conn
            .prepare("SELECT file, path, page, location, score, snippet, file_hash FROM message_sources WHERE message_id = ?1 ORDER BY ordinal")
            .map_err(e)?;
        for message in messages.iter_mut().filter(|m| m.role == "assistant") {
            message.sources = sources
                .query_map([&message.id], |r| {
                    Ok(SourceRecord {
                        file: r.get(0)?,
                        path: r.get(1)?,
                        page: r.get(2)?,
                        location: r.get(3)?,
                        score: r.get::<_, f64>(4)? as f32,
                        snippet: r.get(5)?,
                        file_hash: r.get(6)?,
                    })
                })
                .map_err(e)?
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(e)?;
        }
        Ok(messages)
    }

    /// How many conversations there are.
    pub fn count(&self) -> Result<u32, String> {
        self.conn.query_row("SELECT count(*) FROM conversations WHERE archived = 0", [], |r| r.get(0)).map_err(e)
    }

    #[cfg(test)]
    fn source_rows(&self) -> u32 {
        self.conn.query_row("SELECT count(*) FROM message_sources", [], |r| r.get(0)).unwrap()
    }
}

/// What has become of a cited file since the answer was written.
#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum FileState {
    /// Still indexed, with the same contents: it can be opened.
    Same,
    /// Still at that path, but its contents have changed since.
    Changed,
    /// No longer in the index (deleted, moved, or its folder was removed).
    Missing,
}

/// Compare a citation with the index as it is now. `current_hash` is the
/// hash of the file indexed at the citation's path, if there is one. A
/// citation never points at anything but its own path: a file that has gone
/// is reported as missing, not matched to another.
pub fn file_state(cited_hash: Option<&str>, current_hash: Option<&str>) -> FileState {
    match (cited_hash, current_hash) {
        (_, None) => FileState::Missing,
        (Some(cited), Some(current)) if cited != current => FileState::Changed,
        _ => FileState::Same,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_db(name: &str) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!("loft-history-test-{name}-{}.db", std::process::id()));
        for suffix in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
        }
        path
    }

    fn source(file: &str, hash: &str) -> SourceRecord {
        SourceRecord {
            file: file.to_string(),
            path: format!("C:\\docs\\{file}"),
            page: Some(3),
            location: Some("page 3".into()),
            score: 0.82,
            snippet: format!("an excerpt of {file}"),
            file_hash: Some(hash.to_string()),
        }
    }

    /// Ask and answer one turn; returns the answer's message id.
    fn turn(h: &mut History, conversation: &str, question: &str, answer: &str, sources: &[SourceRecord], now: i64) -> String {
        let (_, answer_id) = h.begin_turn(conversation, question, Some("qwen2.5-3b-instruct-q4"), now).unwrap();
        let stats = serde_json::json!({ "promptTokens": 900, "answerTokens": 40, "turnsSent": 1 });
        h.finish_turn(&answer_id, Status::Complete, answer, Some(&stats), None, sources, now + 5).unwrap();
        answer_id
    }

    #[test]
    fn conversations_survive_a_restart_with_their_messages_in_order() {
        let path = temp_db("restart");
        let (first, second);
        {
            let mut h = History::open(&path).unwrap();
            first = h.create("When did Reuben Baker start working?", 1_000).unwrap();
            assert_eq!(first.title, "When did Reuben Baker start working");
            assert_eq!(first.id.len(), 36, "a GUID: {}", first.id);
            turn(&mut h, &first.id, "When did Reuben Baker start working?", "On 3 March 2021.", &[source("contract.pdf", "h1"), source("letter.docx", "h2")], 1_000);
            turn(&mut h, &first.id, "And who is his manager?", "Dana Whitfield.", &[source("org-chart.xlsx", "h3")], 2_000);
            second = h.create("  what is\tthe cloud POS architecture?  ", 3_000).unwrap();
            turn(&mut h, &second.id, "what is the cloud POS architecture?", "Three services.", &[], 3_000);
        } // Loft closes

        let h = History::open(&path).unwrap();
        assert_eq!(h.mark_interrupted().unwrap(), 0);
        // Most recently updated first.
        let listed = h.list(None).unwrap();
        assert_eq!(listed.iter().map(|c| c.title.as_str()).collect::<Vec<_>>(), ["What is the cloud POS architecture", "When did Reuben Baker start working"]);
        assert_eq!(listed[1].questions, 2);
        assert_eq!((listed[1].created_at, listed[1].updated_at), (1_000, 2_005));
        assert_eq!(h.count().unwrap(), 2);

        // Each message is with its own conversation, in the order written.
        let messages = h.messages(&first.id).unwrap();
        let seen: Vec<(&str, &str)> = messages.iter().map(|m| (m.role.as_str(), m.content.as_str())).collect();
        assert_eq!(
            seen,
            [
                ("user", "When did Reuben Baker start working?"),
                ("assistant", "On 3 March 2021."),
                ("user", "And who is his manager?"),
                ("assistant", "Dana Whitfield."),
            ]
        );
        assert!(messages.iter().all(|m| m.conversation_id == first.id && m.status == Status::Complete));
        // Sources come back with the answer they belong to, in the order shown.
        assert_eq!(messages[1].sources, [source("contract.pdf", "h1"), source("letter.docx", "h2")]);
        assert_eq!(messages[3].sources, [source("org-chart.xlsx", "h3")]);
        assert!(messages[0].sources.is_empty() && messages[2].sources.is_empty());
        // The model and the usage figures are kept with the answer.
        assert_eq!(messages[1].model.as_deref(), Some("qwen2.5-3b-instruct-q4"));
        assert_eq!(messages[1].stats.as_ref().unwrap()["answerTokens"], 40);
        assert_eq!(h.messages(&second.id).unwrap().len(), 2);
        drop(h);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn an_answer_that_did_not_finish_is_never_stored_as_finished() {
        let path = temp_db("interrupted");
        let (conversation, stopped, failed, cut_off);
        {
            let mut h = History::open(&path).unwrap();
            conversation = h.create("Summarise the handbook", 10).unwrap().id;
            // Stopped by the user: what was written is kept, as stopped.
            stopped = h.begin_turn(&conversation, "Summarise the handbook", None, 10).unwrap().1;
            h.finish_turn(&stopped, Status::Stopped, "The handbook covers", None, None, &[source("handbook.md", "h")], 20).unwrap();
            // Failed: the reason is kept, and it is not an answer.
            failed = h.begin_turn(&conversation, "And the holidays?", None, 30).unwrap().1;
            h.finish_turn(&failed, Status::Failed, "", None, Some("the chat model stopped before it finished the answer"), &[], 40).unwrap();
            // Still being written when Loft closes.
            cut_off = h.begin_turn(&conversation, "And the opening hours?", None, 50).unwrap().1;
            let pending = h.messages(&conversation).unwrap();
            assert_eq!(pending.last().unwrap().status, Status::Pending);
            // "Pending" is not something an answer can end as.
            assert!(h.finish_turn(&cut_off, Status::Pending, "x", None, None, &[], 60).is_err());
        }
        let h = History::open(&path).unwrap();
        assert_eq!(h.mark_interrupted().unwrap(), 1);
        assert_eq!(h.mark_interrupted().unwrap(), 0, "only once");
        let by_id = |id: &str| h.messages(&conversation).unwrap().into_iter().find(|m| m.id == id).unwrap();
        assert_eq!((by_id(&stopped).status, by_id(&stopped).content.as_str()), (Status::Stopped, "The handbook covers"));
        assert_eq!(by_id(&stopped).sources.len(), 1);
        assert_eq!(by_id(&failed).status, Status::Failed);
        assert_eq!(by_id(&failed).error.as_deref(), Some("the chat model stopped before it finished the answer"));
        assert_eq!(by_id(&cut_off).status, Status::Interrupted);
        // The questions themselves were saved at once, all three.
        assert_eq!(h.conversation(&conversation).unwrap().unwrap().questions, 3);
        drop(h);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn rename_delete_and_search() {
        let path = temp_db("manage");
        let mut h = History::open(&path).unwrap();
        let a = h.create("How does API authentication work?", 100).unwrap();
        turn(&mut h, &a.id, "How does API authentication work?", "With signed tokens that expire after an hour.", &[source("auth.md", "h1")], 100);
        let b = h.create("Project documentation overview", 200).unwrap();
        turn(&mut h, &b.id, "Project documentation overview", "There are four documents.", &[source("readme.md", "h2"), source("plan.md", "h3")], 200);
        let c = h.create("100% of the budget_2026 file", 300).unwrap();
        turn(&mut h, &c.id, "100% of the budget_2026 file", "It is fully allocated.", &[], 300);

        // Rename: kept as the user wrote it, tidied, and marked as theirs.
        let renamed = h.rename(&a.id, "  Auth   flow  ").unwrap();
        assert_eq!((renamed.title.as_str(), renamed.title_manual), ("Auth flow", true));
        assert!(!h.conversation(&b.id).unwrap().unwrap().title_manual);
        assert!(h.rename(&a.id, "   ").is_err());
        assert!(h.rename("no-such-id", "x").is_err());
        // Asking more in it changes neither the name nor whose it is.
        turn(&mut h, &a.id, "And refresh tokens?", "They last thirty days.", &[], 400);
        let after = h.conversation(&a.id).unwrap().unwrap();
        assert_eq!((after.title.as_str(), after.title_manual, after.updated_at), ("Auth flow", true, 405));

        // Search: titles and message text, case-insensitively; most recent first.
        let titles = |q: &str| h.list(Some(q)).unwrap().into_iter().map(|c| c.title).collect::<Vec<_>>();
        assert_eq!(titles("auth"), ["Auth flow"]);
        assert_eq!(titles("DOCUMENT"), ["Project documentation overview"]);
        assert_eq!(titles("thirty days"), ["Auth flow"], "found by what an answer says");
        assert_eq!(titles("signed tokens"), ["Auth flow"]);
        assert!(titles("nothing like this").is_empty());
        // Signs that mean something to the database are matched as typed.
        assert_eq!(titles("100%"), ["100% of the budget_2026 file"]);
        assert_eq!(titles("budget_2026"), ["100% of the budget_2026 file"]);
        assert!(titles("budgetX2026").is_empty());
        assert_eq!(titles("  ").len(), 3, "an empty search lists everything");

        // Delete: the conversation, its messages and their sources go; the others stay.
        assert_eq!(h.source_rows(), 3);
        assert!(h.delete(&b.id).unwrap());
        assert!(!h.delete(&b.id).unwrap(), "already gone");
        assert_eq!(h.source_rows(), 1);
        assert!(h.messages(&b.id).unwrap().is_empty());
        assert_eq!(h.list(None).unwrap().iter().map(|c| c.title.as_str()).collect::<Vec<_>>(), ["Auth flow", "100% of the budget_2026 file"]);
        assert_eq!(h.messages(&a.id).unwrap().len(), 4);

        // A write for something that is gone is an error, not a silent success.
        assert!(h.begin_turn(&b.id, "still there?", None, 500).is_err());
        let (_, orphan) = h.begin_turn(&a.id, "one more", None, 600).unwrap();
        h.delete(&a.id).unwrap();
        let lost = h.finish_turn(&orphan, Status::Complete, "an answer", None, None, &[], 700).unwrap_err();
        assert!(lost.contains("conversation was deleted"), "{lost}");
        drop(h);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn titles_come_from_the_first_question() {
        assert_eq!(title_from("When did Reuben Baker start working?"), "When did Reuben Baker start working");
        assert_eq!(title_from("  what   are these\nfiles about ?? "), "What are these files about");
        assert_eq!(title_from("Summarise the kickoff notes."), "Summarise the kickoff notes");
        // Long questions are cut at a word, with an ellipsis.
        let long = title_from("Describe everything in the project plan, the FAQ and the kickoff notes in as much detail as you can");
        assert_eq!(long, "Describe everything in the project plan, the FAQ…");
        assert!(long.chars().count() <= 49);
        // One very long word is cut where it has to be.
        assert_eq!(title_from(&"x".repeat(80)).chars().count(), 49);
        assert_eq!(title_from("élan vital?"), "Élan vital");
        assert_eq!(title_from("???"), "New conversation");
        assert_eq!(title_from(""), "New conversation");
    }

    #[test]
    fn a_citation_is_checked_against_the_file_as_it_is_now() {
        // Same file, same contents: it can be opened.
        assert_eq!(file_state(Some("abc"), Some("abc")), FileState::Same);
        // Still there, but edited since the answer was written.
        assert_eq!(file_state(Some("abc"), Some("def")), FileState::Changed);
        // Deleted, moved, or its folder removed: missing, never "some other file".
        assert_eq!(file_state(Some("abc"), None), FileState::Missing);
        assert_eq!(file_state(None, None), FileState::Missing);
        // Cited before hashes were recorded: present is the most that can be said.
        assert_eq!(file_state(None, Some("abc")), FileState::Same);
    }
}
