//! Opening the database and migrating its schema (`PRAGMA user_version`).

use parking_lot::Mutex;
use rusqlite::{Connection, OpenFlags};
use std::path::{Path, PathBuf};

/// Schema version this build writes.
pub const SCHEMA_VERSION: i64 = 5;

/// Every migration, in order; `MIGRATIONS[n]` takes the schema from `n` to
/// `n + 1`. Tables of later phases come with their own migrations
/// (ADR-0012).
const MIGRATIONS: [&str; 5] = [
    r#"
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
"#,
    r#"
-- Phase 7 (ADR-0013): handoffs between AIs.
CREATE TABLE handoffs (
    id            TEXT PRIMARY KEY,
    project_id    TEXT REFERENCES projects(id) ON DELETE SET NULL,
    project_path  TEXT NOT NULL,
    from_session  TEXT NOT NULL,
    to_session    TEXT,
    status        TEXT NOT NULL,
    created_at    TEXT NOT NULL,
    accepted_at   TEXT,
    data          TEXT NOT NULL        -- JSON Handoff
);
CREATE INDEX handoffs_project ON handoffs(project_id, created_at);
CREATE INDEX handoffs_from ON handoffs(from_session);
"#,
    r#"
-- Phase 8a (ADR-0014): tasks of a project.
CREATE TABLE tasks (
    id           TEXT PRIMARY KEY,
    project_id   TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    parent_task  TEXT REFERENCES tasks(id) ON DELETE SET NULL,
    status       TEXT NOT NULL,
    priority     INTEGER NOT NULL,     -- higher comes first
    created_at   TEXT NOT NULL,
    updated_at   TEXT NOT NULL,
    data         TEXT NOT NULL         -- JSON Task
);
CREATE INDEX tasks_project ON tasks(project_id, status, priority DESC, created_at);
CREATE INDEX tasks_parent ON tasks(parent_task);

-- The dependency rows are the truth: a task is loaded with what they say.
CREATE TABLE task_dependencies (
    task_id     TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
    depends_on  TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
    PRIMARY KEY (task_id, depends_on)
);
CREATE INDEX task_dependencies_depends ON task_dependencies(depends_on);
"#,
    // 3 → 4 (ADR-0015): agents and the files they hold.
    r#"
CREATE TABLE agents (
    id            TEXT PRIMARY KEY,
    project_id    TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    task          TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
    session       TEXT,
    parent_agent  TEXT REFERENCES agents(id) ON DELETE SET NULL,
    status        TEXT NOT NULL,        -- QUEUED, RUNNING, DONE, FAILED, STOPPED
    created_at    TEXT NOT NULL,
    updated_at    TEXT NOT NULL,
    data          TEXT NOT NULL         -- JSON Agent
);
CREATE INDEX agents_project ON agents(project_id, status, created_at);
CREATE INDEX agents_task ON agents(task, created_at);
CREATE INDEX agents_session ON agents(session);

-- One owner per file: the key is what makes the lock exclusive. Taking a
-- lock is an INSERT that either writes or names who already has it.
CREATE TABLE file_locks (
    project_id  TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    path        TEXT NOT NULL,
    agent_id    TEXT NOT NULL REFERENCES agents(id) ON DELETE CASCADE,
    task        TEXT NOT NULL,
    at          TEXT NOT NULL,
    data        TEXT NOT NULL,          -- JSON FileLock
    PRIMARY KEY (project_id, path)
);
CREATE INDEX file_locks_agent ON file_locks(agent_id);
"#,
    // 4 → 5 (ADR-0023): projects open side by side, and the projects that
    // work together.
    r#"
-- Place in the sidebar of a project open in the app; NULL when closed.
ALTER TABLE projects ADD COLUMN open_rank INTEGER;

-- Projects that work together (an API and the app that uses it, a library
-- and who depends on it): their AIs consult each other. One row per pair,
-- the smaller id first.
CREATE TABLE project_links (
    a           TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    b           TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    note        TEXT NOT NULL DEFAULT '',  -- how they relate, in the user's words
    created_at  TEXT NOT NULL,
    PRIMARY KEY (a, b),
    CHECK (a < b)
);
CREATE INDEX project_links_b ON project_links(b);
"#,
];

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

/// The schema of the database file at `path`, without changing it: `None`
/// when there is no file (or it is empty), `Some(0)` for a file that was
/// never migrated. Lets the app copy the data before a migration
/// (ADR-0022).
pub fn schema_of(path: &Path) -> Result<Option<i64>, String> {
    match std::fs::metadata(path) {
        Ok(meta) if meta.len() == 0 => return Ok(None),
        Ok(_) => {}
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(format!("cannot read {}: {err}", path.display())),
    }
    let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|e| format!("cannot open {}: {e}", path.display()))?;
    conn.pragma_query_value(None, "user_version", |row| row.get(0))
        .map(Some)
        .map_err(|e| format!("cannot read {}: {e}", path.display()))
}

/// Writes a consistent copy of the database file `src` to `dest` (which
/// must not exist), including what is still in its WAL. What `src` holds
/// is not changed, and its schema may be newer than this build's.
pub fn snapshot_file(src: &Path, dest: &Path) -> Result<(), String> {
    // Read-write without create: a WAL database may need its `-shm`.
    let conn = Connection::open_with_flags(src, OpenFlags::SQLITE_OPEN_READ_WRITE)
        .map_err(|e| format!("cannot open {}: {e}", src.display()))?;
    conn.busy_timeout(std::time::Duration::from_secs(5))
        .map_err(|e| e.to_string())?;
    vacuum_into(&conn, dest)
}

/// `VACUUM INTO`: a compact, consistent copy made by SQLite itself, safe
/// while the database is in use.
pub(crate) fn vacuum_into(conn: &Connection, dest: &Path) -> Result<(), String> {
    if let Some(dir) = dest.parent() {
        std::fs::create_dir_all(dir)
            .map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    }
    let target = dest
        .to_str()
        .ok_or_else(|| format!("{} is not valid UTF-8", dest.display()))?;
    conn.execute("VACUUM INTO ?1", [target])
        .map(|_| ())
        .map_err(|e| format!("cannot copy the database to {}: {e}", dest.display()))
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
    fn upgrades_a_phase_6_database() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("orchestrator.db");
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(&format!(
                "BEGIN;\n{}\nPRAGMA user_version = 1;\nCOMMIT;",
                MIGRATIONS[0]
            ))
            .unwrap();
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
        let projects: i64 = conn
            .query_row("SELECT count(*) FROM projects", [], |r| r.get(0))
            .unwrap();
        let handoffs: i64 = conn
            .query_row("SELECT count(*) FROM handoffs", [], |r| r.get(0))
            .unwrap();
        let tasks: i64 = conn
            .query_row("SELECT count(*) FROM tasks", [], |r| r.get(0))
            .unwrap();
        assert_eq!((projects, handoffs, tasks), (1, 0, 0));
    }

    #[test]
    fn upgrades_a_phase_7_database() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("orchestrator.db");
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(&format!(
                "BEGIN;\n{}\n{}\nPRAGMA user_version = 2;\nCOMMIT;",
                MIGRATIONS[0], MIGRATIONS[1]
            ))
            .unwrap();
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
        // The tasks of the new schema live beside what was already there.
        let projects: i64 = conn
            .query_row("SELECT count(*) FROM projects", [], |r| r.get(0))
            .unwrap();
        let tasks: i64 = conn
            .query_row("SELECT count(*) FROM tasks", [], |r| r.get(0))
            .unwrap();
        assert_eq!((projects, tasks), (1, 0));
    }

    #[test]
    fn upgrades_a_phase_8a_database() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("orchestrator.db");
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(&format!(
                "BEGIN;\n{}\n{}\n{}\nPRAGMA user_version = 3;\nCOMMIT;",
                MIGRATIONS[0], MIGRATIONS[1], MIGRATIONS[2]
            ))
            .unwrap();
            conn.execute(
                "INSERT INTO projects (id, path, name, created_at, last_opened_at) VALUES ('p', '/x', 'x', 'a', 'a')",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO tasks (id, project_id, status, priority, created_at, updated_at, data)
                 VALUES ('t', 'p', 'TODO', 1, 'a', 'a', '{}')",
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
        // The tasks of Phase 8a are still there, now with agents beside them.
        let tasks: i64 = conn
            .query_row("SELECT count(*) FROM tasks", [], |r| r.get(0))
            .unwrap();
        let agents: i64 = conn
            .query_row("SELECT count(*) FROM agents", [], |r| r.get(0))
            .unwrap();
        let locks: i64 = conn
            .query_row("SELECT count(*) FROM file_locks", [], |r| r.get(0))
            .unwrap();
        assert_eq!((tasks, agents, locks), (1, 0, 0));
    }

    #[test]
    fn upgrades_a_phase_12_database() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("orchestrator.db");
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(&format!(
                "BEGIN;\n{}\nPRAGMA user_version = 4;\nCOMMIT;",
                MIGRATIONS[..4].join("\n")
            ))
            .unwrap();
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
        // The project is still there, closed, with no links yet.
        let open: Option<i64> = conn
            .query_row("SELECT open_rank FROM projects WHERE id = 'p'", [], |r| {
                r.get(0)
            })
            .unwrap();
        let links: i64 = conn
            .query_row("SELECT count(*) FROM project_links", [], |r| r.get(0))
            .unwrap();
        assert_eq!((open, links), (None, 0));
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
    fn snapshots_keep_everything_and_leave_the_file_alone() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("orchestrator.db");
        assert_eq!(schema_of(&path).unwrap(), None);
        let db = Database::open(&path).unwrap();
        db.conn
            .lock()
            .execute(
                "INSERT INTO projects (id, path, name, created_at, last_opened_at) VALUES ('p', '/x', 'x', 'a', 'a')",
                [],
            )
            .unwrap();
        // Still open, the row only in the WAL: the copy has it anyway.
        let copy = dir.path().join("backup").join("orchestrator.db");
        snapshot_file(&path, &copy).unwrap();
        assert_eq!(schema_of(&copy).unwrap(), Some(SCHEMA_VERSION));
        let conn = Connection::open(&copy).unwrap();
        let count: i64 = conn
            .query_row("SELECT count(*) FROM projects", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1);
        // A copy never overwrites.
        assert!(snapshot_file(&path, &copy).is_err());
        drop(db);
        assert_eq!(schema_of(&path).unwrap(), Some(SCHEMA_VERSION));
    }

    #[test]
    fn a_newer_schema_can_still_be_copied() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("orchestrator.db");
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch("CREATE TABLE future (x); INSERT INTO future VALUES (1);")
                .unwrap();
            conn.pragma_update(None, "user_version", 99).unwrap();
        }
        assert_eq!(schema_of(&path).unwrap(), Some(99));
        let copy = dir.path().join("copy.db");
        snapshot_file(&path, &copy).unwrap();
        assert_eq!(schema_of(&copy).unwrap(), Some(99));
        assert!(Database::open(&path).is_err(), "never migrated down");
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
