//! SQLite-backed symbol storage with session memory.

use anyhow::{Context, Result};
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use std::path::Path;

use crate::symbol::{Language, Symbol, SymbolKind};

/// A stored chat message within a session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredMessage {
    pub role: String,
    pub content: String,
    pub timestamp_secs: i64,
}

/// A named conversation session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub id: String,
    pub created_at: i64,
    pub updated_at: i64,
    pub message_count: u32,
}

/// Persistent symbol store backed by SQLite.
pub struct SymbolStore {
    conn: Connection,
}

impl SymbolStore {
    /// Open or create a symbol store at the given path.
    pub fn open(db_path: &Path) -> Result<Self> {
        let conn = Connection::open(db_path).context("failed to open symbol database")?;
        let store = Self { conn };
        store.init_schema()?;
        Ok(store)
    }

    /// Create an in-memory store (for testing).
    pub fn in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory().context("failed to create in-memory database")?;
        let store = Self { conn };
        store.init_schema()?;
        Ok(store)
    }

    fn init_schema(&self) -> Result<()> {
        self.conn
            .execute_batch(
                "
            CREATE TABLE IF NOT EXISTS files (
                path TEXT PRIMARY KEY,
                mtime_ns INTEGER NOT NULL,
                content_hash TEXT NOT NULL,
                language TEXT NOT NULL,
                indexed_at INTEGER NOT NULL
            );

            CREATE TABLE IF NOT EXISTS symbols (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                file_path TEXT NOT NULL REFERENCES files(path) ON DELETE CASCADE,
                name TEXT NOT NULL,
                kind TEXT NOT NULL,
                language TEXT NOT NULL,
                start_line INTEGER NOT NULL,
                end_line INTEGER NOT NULL,
                start_col INTEGER NOT NULL,
                end_col INTEGER NOT NULL,
                parent_name TEXT,
                signature TEXT,
                indexed_at INTEGER NOT NULL
            );

            CREATE INDEX IF NOT EXISTS idx_symbols_file ON symbols(file_path);
            CREATE INDEX IF NOT EXISTS idx_symbols_name ON symbols(name);
            CREATE INDEX IF NOT EXISTS idx_symbols_kind ON symbols(kind);

            CREATE VIRTUAL TABLE IF NOT EXISTS code_chunks USING fts5(
                file_path UNINDEXED,
                start_line UNINDEXED,
                end_line UNINDEXED,
                content,
                tokenize='porter ascii'
            );

            CREATE TABLE IF NOT EXISTS sessions (
                id TEXT PRIMARY KEY,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL
            );

            CREATE TABLE IF NOT EXISTS session_messages (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
                role TEXT NOT NULL,
                content TEXT NOT NULL,
                timestamp_secs INTEGER NOT NULL
            );

            CREATE INDEX IF NOT EXISTS idx_session_messages_session
                ON session_messages(session_id, id);

            PRAGMA journal_mode=WAL;
            PRAGMA foreign_keys=ON;
        ",
            )
            .context("failed to initialize schema")?;
        Ok(())
    }

    /// Check if a file needs re-indexing based on content hash.
    pub fn file_needs_update(&self, path: &str, content_hash: &str) -> Result<bool> {
        let mut stmt = self
            .conn
            .prepare("SELECT content_hash FROM files WHERE path = ?1")?;
        let result: Option<String> = stmt.query_row(params![path], |row| row.get(0)).ok();
        Ok(result.as_deref() != Some(content_hash))
    }

    /// Replace all symbols for a file (transactional).
    pub fn upsert_file(
        &self,
        file_path: &str,
        mtime_ns: i64,
        content_hash: &str,
        language: Language,
        symbols: &[Symbol],
    ) -> Result<()> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;

        let tx = self.conn.unchecked_transaction()?;

        // Upsert file record
        tx.execute(
            "INSERT OR REPLACE INTO files (path, mtime_ns, content_hash, language, indexed_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![file_path, mtime_ns, content_hash, language.as_str(), now],
        )?;

        // Delete old symbols for this file
        tx.execute(
            "DELETE FROM symbols WHERE file_path = ?1",
            params![file_path],
        )?;

        // Insert new symbols
        let mut stmt = tx.prepare(
            "INSERT INTO symbols (file_path, name, kind, language, start_line, end_line,
                                  start_col, end_col, parent_name, signature, indexed_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        )?;

        for sym in symbols {
            stmt.execute(params![
                sym.file_path,
                sym.name,
                sym.kind.as_str(),
                sym.language.as_str(),
                sym.start_line,
                sym.end_line,
                sym.start_col,
                sym.end_col,
                sym.parent_name,
                sym.signature,
                now,
            ])?;
        }

        drop(stmt);
        tx.commit()?;
        Ok(())
    }

    /// Query symbols by name (prefix match).
    pub fn query_by_name(&self, name_prefix: &str) -> Result<Vec<Symbol>> {
        let mut stmt = self.conn.prepare(
            "SELECT file_path, name, kind, language, start_line, end_line,
                    start_col, end_col, parent_name, signature
             FROM symbols WHERE name LIKE ?1
             ORDER BY name, file_path",
        )?;
        let pattern = format!("{name_prefix}%");
        let rows = stmt.query_map(params![pattern], row_to_symbol)?;
        rows.collect::<Result<Vec<_>, _>>()
            .context("failed to read symbols")
    }

    /// Query all symbols in a specific file.
    pub fn query_by_file(&self, file_path: &str) -> Result<Vec<Symbol>> {
        let mut stmt = self.conn.prepare(
            "SELECT file_path, name, kind, language, start_line, end_line,
                    start_col, end_col, parent_name, signature
             FROM symbols WHERE file_path = ?1
             ORDER BY start_line",
        )?;
        let rows = stmt.query_map(params![file_path], row_to_symbol)?;
        rows.collect::<Result<Vec<_>, _>>()
            .context("failed to read symbols")
    }

    /// Query symbols by kind across all files.
    pub fn query_by_kind(&self, kind: SymbolKind) -> Result<Vec<Symbol>> {
        let mut stmt = self.conn.prepare(
            "SELECT file_path, name, kind, language, start_line, end_line,
                    start_col, end_col, parent_name, signature
             FROM symbols WHERE kind = ?1
             ORDER BY name, file_path",
        )?;
        let rows = stmt.query_map(params![kind.as_str()], row_to_symbol)?;
        rows.collect::<Result<Vec<_>, _>>()
            .context("failed to read symbols")
    }

    /// Get all indexed file paths.
    pub fn indexed_files(&self) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare("SELECT path FROM files ORDER BY path")?;
        let rows = stmt.query_map([], |row| row.get(0))?;
        rows.collect::<Result<Vec<_>, _>>()
            .context("failed to read files")
    }

    /// Remove a file and its symbols.
    pub fn remove_file(&self, file_path: &str) -> Result<()> {
        self.conn
            .execute("DELETE FROM files WHERE path = ?1", params![file_path])?;
        self.conn.execute(
            "DELETE FROM symbols WHERE file_path = ?1",
            params![file_path],
        )?;
        Ok(())
    }

    /// Total number of symbols in the store.
    pub fn symbol_count(&self) -> Result<u64> {
        let count: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM symbols", [], |row| row.get(0))?;
        Ok(count as u64)
    }

    /// Total number of indexed files.
    pub fn file_count(&self) -> Result<u64> {
        let count: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM files", [], |row| row.get(0))?;
        Ok(count as u64)
    }

    /// Store 50-line chunks of file content in FTS5 for semantic search.
    pub fn upsert_chunks(&self, file_path: &str, content: &str) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "DELETE FROM code_chunks WHERE file_path = ?1",
            params![file_path],
        )?;

        let lines: Vec<&str> = content.lines().collect();
        if lines.is_empty() {
            tx.commit()?;
            return Ok(());
        }

        let chunk_size = 50usize;
        let overlap = 10usize;
        let mut stmt = tx.prepare(
            "INSERT INTO code_chunks (file_path, start_line, end_line, content)
             VALUES (?1, ?2, ?3, ?4)",
        )?;

        let mut i = 0usize;
        loop {
            let end = (i + chunk_size).min(lines.len());
            let chunk = lines[i..end].join("\n");
            stmt.execute(params![file_path, i + 1, end, chunk])?;
            if end == lines.len() {
                break;
            }
            i += chunk_size - overlap;
        }

        drop(stmt);
        tx.commit()?;
        Ok(())
    }

    /// Full-text search over code chunks using BM25 ranking.
    ///
    /// Returns up to `limit` results ordered by relevance.
    pub fn search_chunks(&self, query: &str, limit: usize) -> Result<Vec<ChunkResult>> {
        let mut stmt = self.conn.prepare(
            "SELECT file_path, start_line, end_line, content
             FROM code_chunks
             WHERE content MATCH ?1
             ORDER BY rank
             LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![query, limit as i64], |row| {
            Ok(ChunkResult {
                file_path: row.get(0)?,
                start_line: row.get(1)?,
                end_line: row.get(2)?,
                content: row.get(3)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>()
            .context("failed to search chunks")
    }

    // ── Session memory ────────────────────────────────────────────────

    /// Create or touch a session. Returns true if it was newly created.
    pub fn ensure_session(&self, session_id: &str) -> Result<bool> {
        let now = unix_now();
        let existed = self
            .conn
            .query_row(
                "SELECT COUNT(*) FROM sessions WHERE id = ?1",
                params![session_id],
                |row| row.get::<_, i64>(0),
            )
            .map(|c| c > 0)
            .unwrap_or(false);

        if existed {
            self.conn.execute(
                "UPDATE sessions SET updated_at = ?1 WHERE id = ?2",
                params![now, session_id],
            )?;
        } else {
            self.conn.execute(
                "INSERT INTO sessions (id, created_at, updated_at) VALUES (?1, ?2, ?3)",
                params![session_id, now, now],
            )?;
        }
        Ok(!existed)
    }

    /// Append a message to a session.
    pub fn add_message(&self, session_id: &str, role: &str, content: &str) -> Result<()> {
        self.ensure_session(session_id)?;
        let now = unix_now();
        self.conn.execute(
            "INSERT INTO session_messages (session_id, role, content, timestamp_secs)
             VALUES (?1, ?2, ?3, ?4)",
            params![session_id, role, content, now],
        )?;
        self.conn.execute(
            "UPDATE sessions SET updated_at = ?1 WHERE id = ?2",
            params![now, session_id],
        )?;
        Ok(())
    }

    /// Get the most recent N messages from a session, oldest first.
    pub fn get_recent_messages(
        &self,
        session_id: &str,
        limit: usize,
    ) -> Result<Vec<StoredMessage>> {
        let mut stmt = self.conn.prepare(
            "SELECT role, content, timestamp_secs
             FROM session_messages
             WHERE session_id = ?1
             ORDER BY id DESC
             LIMIT ?2",
        )?;
        let mut rows: Vec<StoredMessage> = stmt
            .query_map(params![session_id, limit as i64], |row| {
                Ok(StoredMessage {
                    role: row.get(0)?,
                    content: row.get(1)?,
                    timestamp_secs: row.get(2)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        rows.reverse();
        Ok(rows)
    }

    /// List all sessions, newest first.
    pub fn list_sessions(&self) -> Result<Vec<Session>> {
        let mut stmt = self.conn.prepare(
            "SELECT s.id, s.created_at, s.updated_at,
                    (SELECT COUNT(*) FROM session_messages WHERE session_id = s.id) as msg_count
             FROM sessions s
             ORDER BY s.updated_at DESC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(Session {
                id: row.get(0)?,
                created_at: row.get(1)?,
                updated_at: row.get(2)?,
                message_count: row.get::<_, i64>(3)? as u32,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>()
            .context("failed to list sessions")
    }

    /// Delete a session and all its messages.
    pub fn delete_session(&self, session_id: &str) -> Result<bool> {
        let deleted = self
            .conn
            .execute("DELETE FROM sessions WHERE id = ?1", params![session_id])?;
        Ok(deleted > 0)
    }
}

/// A matched code chunk from FTS5 search.
#[derive(Debug, Clone)]
pub struct ChunkResult {
    pub file_path: String,
    pub start_line: usize,
    pub end_line: usize,
    pub content: String,
}

fn row_to_symbol(row: &rusqlite::Row) -> rusqlite::Result<Symbol> {
    let kind_str: String = row.get(2)?;
    let lang_str: String = row.get(3)?;
    Ok(Symbol {
        file_path: row.get(0)?,
        name: row.get(1)?,
        kind: SymbolKind::from_str(&kind_str).unwrap_or(SymbolKind::Function),
        language: match lang_str.as_str() {
            "rust" => Language::Rust,
            "python" => Language::Python,
            "typescript" => Language::TypeScript,
            _ => Language::Rust,
        },
        start_line: row.get(4)?,
        end_line: row.get(5)?,
        start_col: row.get(6)?,
        end_col: row.get(7)?,
        parent_name: row.get(8)?,
        signature: row.get(9)?,
    })
}

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_symbols() -> Vec<Symbol> {
        vec![
            Symbol {
                file_path: "src/main.rs".to_string(),
                name: "main".to_string(),
                kind: SymbolKind::Function,
                language: Language::Rust,
                start_line: 0,
                end_line: 5,
                start_col: 0,
                end_col: 1,
                parent_name: None,
                signature: Some("fn main()".to_string()),
            },
            Symbol {
                file_path: "src/main.rs".to_string(),
                name: "Config".to_string(),
                kind: SymbolKind::Struct,
                language: Language::Rust,
                start_line: 7,
                end_line: 10,
                start_col: 0,
                end_col: 1,
                parent_name: None,
                signature: Some("pub struct Config".to_string()),
            },
        ]
    }

    #[test]
    fn test_roundtrip() {
        let store = SymbolStore::in_memory().unwrap();
        let symbols = sample_symbols();

        store
            .upsert_file("src/main.rs", 1000, "abc123", Language::Rust, &symbols)
            .unwrap();

        assert_eq!(store.file_count().unwrap(), 1);
        assert_eq!(store.symbol_count().unwrap(), 2);

        let result = store.query_by_file("src/main.rs").unwrap();
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].name, "main");
        assert_eq!(result[1].name, "Config");
    }

    #[test]
    fn test_query_by_name() {
        let store = SymbolStore::in_memory().unwrap();
        let symbols = sample_symbols();

        store
            .upsert_file("src/main.rs", 1000, "abc123", Language::Rust, &symbols)
            .unwrap();

        let result = store.query_by_name("Con").unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].name, "Config");
    }

    #[test]
    fn test_upsert_replaces() {
        let store = SymbolStore::in_memory().unwrap();
        let symbols = sample_symbols();

        store
            .upsert_file("src/main.rs", 1000, "abc123", Language::Rust, &symbols)
            .unwrap();
        assert_eq!(store.symbol_count().unwrap(), 2);

        // Re-index with only one symbol
        store
            .upsert_file("src/main.rs", 2000, "def456", Language::Rust, &symbols[..1])
            .unwrap();
        assert_eq!(store.symbol_count().unwrap(), 1);
    }

    #[test]
    fn test_file_needs_update() {
        let store = SymbolStore::in_memory().unwrap();

        // New file always needs update
        assert!(store.file_needs_update("src/main.rs", "abc123").unwrap());

        store
            .upsert_file("src/main.rs", 1000, "abc123", Language::Rust, &[])
            .unwrap();

        // Same hash — no update needed
        assert!(!store.file_needs_update("src/main.rs", "abc123").unwrap());

        // Different hash — needs update
        assert!(store.file_needs_update("src/main.rs", "def456").unwrap());
    }
}
