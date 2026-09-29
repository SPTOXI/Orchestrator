//! Opening the database and migrating its schema (`PRAGMA user_version`).

use parking_lot::Mutex;
use rusqlite::Connection;
use std::path::{Path, PathBuf};

/// Schema version this build writes.
pub const SCHEMA_VERSION: i64 = 1;

/// Every migration, in order; `MIGRATIONS[n]` takes the schema from `n` to
/// `n + 1`. Tables of later phases (tasks, agents, file locks…) come with
/// their own migrations (ADR-0012).
const MIGRATIONS: [&str; 1] = [r#"
CREATE TABLE projects (
    id              TEXT PRIMARY KEY,
    path            TEXT NOT NULL UNIQUE,
    name            TEXT NOT NULL,
    created_at      TEXT NOT NULL,
    last_opened_at  TEXT NOT NULL,
    stack           TEXT,              -- JSON: what the detector found
    hidden          INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE audit_events (
    id          TEXT PRIMARY KEY,
    at          TEXT NOT NULL,
    kind        TEXT NOT NULL,
    origin      TEXT NOT NULL,         -- JSON CallOrigin
    call_id     TEXT,
    summary     TEXT NOT NULL,
    data        TEXT NOT NULL,         -- JSON
    project_id  TEXT REFERENCES projects(id) ON DELETE SET NULL,
    session_id  TEXT
);
CREATE INDEX audit_events_at ON audit_events(at, id);
CREATE INDEX audit_events_kind ON audit_events(kind, at);
CREATE INDEX audit_events_project ON audit_events(project_id, at);
CREATE INDEX audit_events_session ON audit_events(session_id, at);

-- The master document's `tool_calls`, without duplicating the history.
CREATE VIEW tool_calls AS
    SELECT id, at, call_id, project_id, session_id, origin,
           json_extract(data, '$.tool')       AS tool,
           json_extract(data, '$.ok')         AS ok,
           json_extract(data, '$.durationMs') AS duration_ms,
           data
    FROM audit_events WHERE kind = 'TOOL_CALLED';

CREATE TABLE sessions (
    id            TEXT PRIMARY KEY,
    project_id    TEXT REFERENCES projects(id) ON DELETE SET NULL,
    project_path  TEXT NOT NULL,
    provider      TEXT NOT NULL,
    model         TEXT,
    title         TEXT NOT NULL,
    parent_id     TEXT,
    status        TEXT NOT NULL,
    created_at    TEXT NOT NULL,
    updated_at    TEXT NOT NULL,
    info          TEXT NOT NULL,       -- JSON SessionInfo
    native        TEXT NOT NULL,       -- JSON NativeSession (provider state)
    spec          TEXT NOT NULL        -- JSON: instructions and model asked for
);
CREATE INDEX sessions_project ON sessions(project_id, updated_at);

CREATE TABLE session_entries (
    session_id  TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    seq         INTEGER NOT NULL,
    at          TEXT NOT NULL,
    event       TEXT NOT NULL,         -- JSON SessionEvent
    PRIMARY KEY (session_id, seq)
);

CREATE TABLE memory_entries (
    id          TEXT PRIMARY KEY,
    project_id  TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    kind        TEXT NOT NULL,
    title       TEXT NOT NULL,
    content     TEXT NOT NULL,
    tags        TEXT NOT NULL,         -- JSON array
    pinned      INTEGER NOT NULL DEFAULT 0,
    source      TEXT NOT NULL,
    created_at  TEXT NOT NULL,
    updated_at  TEXT NOT NULL
);
CREATE INDEX memory_entries_project ON memory_entries(project_id, kind);

CREATE TABLE decisions (
    id            TEXT PRIMARY KEY,
    project_id    TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    title         TEXT NOT NULL,
    context       TEXT NOT NULL,
    decision      TEXT NOT NULL,
    consequences  TEXT NOT NULL,
    status        TEXT NOT NULL,
    source        TEXT NOT NULL,
    created_at    TEXT NOT NULL,
    updated_at    TEXT NOT NULL
);
CREATE INDEX decisions_project ON decisions(project_id, created_at);

CREATE TABLE deliberations (
    id           TEXT PRIMARY KEY,
    project_id   TEXT REFERENCES projects(id) ON DELETE SET NULL,
    created_at   TEXT NOT NULL,
    cache_key    TEXT,                 -- set when the decision may be reused
    valid_until  TEXT,
    data         TEXT NOT NULL         -- JSON Deliberation
);
CREATE INDEX deliberations_created ON deliberations(created_at);
CREATE INDEX deliberations_cache ON deliberations(cache_key, valid_until);

-- L3 search: memory, decisions, session messages and notable events.
CREATE VIRTUAL TABLE search_index USING fts5(
    kind UNINDEXED,
    ref_id UNINDEXED,
    project_id UNINDEXED,
    at UNINDEXED,
    title,
    body,
    tokenize = 'unicode61 remove_diacritics 2'
);
"#];

pub struct Database {
    pub(crate) conn: Mutex<Connection>,
    path: Option<PathBuf>,
}

impl Database {
    /// Opens (or creates) the database file and migrates it.
    pub fn open(path: &Path) -> Result<Self, String> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)
                .map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
        }
        let conn =
            Connection::open(path).map_err(|e| format!("cannot open {}: {e}", path.display()))?;
        // WAL keeps readers and the writer apart; NORMAL is durable enough
        // for a local history (a power loss may lose the last transaction).
        conn.pragma_update(None, "journal_mode", "WAL")
            .map_err(|e| e.to_string())?;
        conn.pragma_update(None, "synchronous", "NORMAL")
            .map_err(|e| e.to_string())?;
        Self::init(conn, Some(path.to_path_buf()))
    }

    /// A database that lives only in memory (tests, or when the file
    /// cannot be opened).
    pub fn in_memory() -> Self {
        let conn = Connection::open_in_memory().expect("in-memory SQLite");
        Self::init(conn, None).expect("in-memory schema")
    }

    fn init(conn: Connection, path: Option<PathBuf>) -> Result<Self, String> {
        conn.pragma_update(None, "foreign_keys", "ON")
            .map_err(|e| e.to_string())?;
        conn.busy_timeout(std::time::Duration::from_secs(5))
            .map_err(|e| e.to_string())?;
        migrate(&conn)?;
        Ok(Self {
            conn: Mutex::new(conn),
            path,
        })
    }

    /// The file, or `None` for an in-memory database.
    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }
}

fn migrate(conn: &Connection) -> Result<(), String> {
    let version: i64 = conn
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .map_err(|e| e.to_string())?;
    if version > SCHEMA_VERSION {
        return Err(format!(
            "the database was written by a newer Orchestrator (schema {version}, this build knows {SCHEMA_VERSION})"
        ));
    }
    for (index, sql) in MIGRATIONS.iter().enumerate().skip(version as usize) {
        let next = index as i64 + 1;
        let batch = format!("BEGIN;\n{sql}\nPRAGMA user_version = {next};\nCOMMIT;");
        conn.execute_batch(&batch).map_err(|e| {
            let _ = conn.execute_batch("ROLLBACK;");
            format!("migration to schema {next} failed: {e}")
        })?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrates_once_and_keeps_data() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub").join("orchestrator.db");
        {
            let db = Database::open(&path).unwrap();
            let conn = db.conn.lock();
            conn.execute(
                "INSERT INTO projects (id, path, name, created_at, last_opened_at) VALUES ('p', '/x', 'x', 'a', 'a')",
                [],
            )
            .unwrap();
        }
        let db = Database::open(&path).unwrap();
        let conn = db.conn.lock();
        let version: i64 = conn
            .pragma_query_value(None, "user_version", |r| r.get(0))
            .unwrap();
        assert_eq!(version, SCHEMA_VERSION);
        let count: i64 = conn
            .query_row("SELECT count(*) FROM projects", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1);
        let mode: String = conn
            .pragma_query_value(None, "journal_mode", |r| r.get(0))
            .unwrap();
        assert_eq!(mode, "wal");
    }

    #[test]
    fn refuses_a_newer_schema() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("orchestrator.db");
        {
            let conn = Connection::open(&path).unwrap();
            conn.pragma_update(None, "user_version", 99).unwrap();
        }
        let err = Database::open(&path).err().unwrap();
        assert!(err.contains("newer Orchestrator"), "{err}");
    }

    #[test]
    fn full_text_search_is_available() {
        let db = Database::in_memory();
        let conn = db.conn.lock();
        conn.execute(
            "INSERT INTO search_index (kind, ref_id, project_id, at, title, body) VALUES ('memory', '1', 'p', 'a', 'Arquitetura', 'Decisão sobre filas')",
            [],
        )
        .unwrap();
        let found: i64 = conn
            .query_row(
                "SELECT count(*) FROM search_index WHERE search_index MATCH 'decisao'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(found, 1, "accents are ignored");
    }
}
