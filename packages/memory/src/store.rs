//! `MemoryStore`: the Orchestrator's local database (ADR-0012). History,
//! projects and the glue shared by the other modules.

use crate::db::Database;
use crate::model::{HistoryPage, HistoryQuery, Project, RecentImport};
use crate::search;
use chrono::{DateTime, SecondsFormat, Utc};
use orchestrator_core::{AuditEvent, CallOrigin, EventKind};
use parking_lot::Mutex;
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};
use std::path::Path;

const DEFAULT_PAGE: usize = 200;
const MAX_PAGE: usize = 1000;

pub struct MemoryStore {
    pub(crate) db: Database,
    /// Project open in the app: events without their own project get it.
    current: Mutex<Option<String>>,
}

pub(crate) type Sql<T> = rusqlite::Result<T>;

/// Sortable timestamp (fixed precision, UTC).
pub(crate) fn ts(at: &DateTime<Utc>) -> String {
    at.to_rfc3339_opts(SecondsFormat::Nanos, true)
}

pub(crate) fn parse_ts(text: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(text)
        .map(|t| t.with_timezone(&Utc))
        .unwrap_or_default()
}

pub(crate) fn kind_id(kind: EventKind) -> String {
    serde_json::to_value(kind)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_default()
}

pub(crate) fn new_id() -> String {
    uuid::Uuid::now_v7().to_string()
}

/// `user`, `agent`, `council` or `system`.
pub(crate) fn origin_type(origin: &CallOrigin) -> &'static str {
    match origin {
        CallOrigin::User => "user",
        CallOrigin::Agent { .. } => "agent",
        CallOrigin::Council { .. } => "council",
        CallOrigin::System => "system",
    }
}

pub(crate) fn event_from_row(row: &rusqlite::Row<'_>) -> Sql<AuditEvent> {
    let kind: String = row.get("kind")?;
    let origin: String = row.get("origin")?;
    let data: String = row.get("data")?;
    let at: String = row.get("at")?;
    let conversion = |e: serde_json::Error| {
        rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e))
    };
    Ok(AuditEvent {
        id: row.get::<_, String>("id")?.into(),
        at: parse_ts(&at),
        kind: serde_json::from_value(Value::String(kind)).map_err(conversion)?,
        origin: serde_json::from_str(&origin).map_err(conversion)?,
        call_id: row.get::<_, Option<String>>("call_id")?.map(Into::into),
        summary: row.get("summary")?,
        data: serde_json::from_str(&data).map_err(conversion)?,
    })
}

pub(crate) fn project_from_row(row: &rusqlite::Row<'_>) -> Sql<Project> {
    let stack: Option<String> = row.get("stack")?;
    Ok(Project {
        id: row.get("id")?,
        path: row.get("path")?,
        name: row.get("name")?,
        created_at: parse_ts(&row.get::<_, String>("created_at")?),
        last_opened_at: parse_ts(&row.get::<_, String>("last_opened_at")?),
        stack: stack.and_then(|s| serde_json::from_str(&s).ok()),
    })
}

fn text_of<'a>(data: &'a Value, key: &str) -> Option<&'a str> {
    data.get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
}

impl MemoryStore {
    /// Opens `path`. If it cannot be opened the store works in memory and the
    /// problem comes back as a warning: losing the database must never stop
    /// the app.
    pub fn open(path: &Path) -> (Self, Option<String>) {
        match Database::open(path) {
            Ok(db) => (Self::with(db), None),
            Err(err) => (
                Self::in_memory(),
                Some(format!(
                    "{err}; history and memory are kept in memory until the app closes"
                )),
            ),
        }
    }

    pub fn in_memory() -> Self {
        Self::with(Database::in_memory())
    }

    fn with(db: Database) -> Self {
        Self {
            db,
            current: Mutex::new(None),
        }
    }

    /// The database file (`None` = in memory).
    pub fn path(&self) -> Option<&Path> {
        self.db.path()
    }

    /// Writes a consistent copy of the database to `dest` (which must not
    /// exist) while the app keeps using it (ADR-0022). An in-memory store
    /// has nothing to copy.
    pub fn backup_to(&self, dest: &Path) -> Result<(), String> {
        if self.db.path().is_none() {
            return Err("the database is in memory".into());
        }
        crate::db::vacuum_into(&self.db.conn.lock(), dest)
    }

    // ------------------------------------------------------------ history

    /// Records an event. Returns the follow-up events it caused
    /// (`PROJECT_CREATED`, the detected stack as `MEMORY_SAVED`), which the
    /// caller publishes and records in turn. Never fails: a write error is
    /// reported on stderr.
    pub fn record(&self, event: &AuditEvent) -> Vec<AuditEvent> {
        let conn = self.db.conn.lock();
        match self.record_in(&conn, event, true) {
            Ok(follow) => follow,
            Err(err) => {
                eprintln!("[orchestrator] cannot record {:?}: {err}", event.kind);
                Vec::new()
            }
        }
    }

    fn record_in(&self, conn: &Connection, event: &AuditEvent, live: bool) -> Sql<Vec<AuditEvent>> {
        let mut follow = Vec::new();
        let data = &event.data;
        let session_id =
            text_of(data, "sessionId")
                .map(str::to_owned)
                .or_else(|| match &event.origin {
                    CallOrigin::Agent {
                        session_id: Some(id),
                        ..
                    } => Some(id.to_string()),
                    _ => None,
                });

        let project_id = if event.kind == EventKind::ProjectOpened {
            match (text_of(data, "path"), text_of(data, "name")) {
                (Some(path), Some(name)) => {
                    let (project, created) = upsert_project(conn, path, name, &event.at, data)?;
                    // The `project.open` call is recorded just before this
                    // event, still under the previous project (or none).
                    if let Some(call) = &event.call_id {
                        conn.execute(
                            "UPDATE audit_events SET project_id = ?1
                             WHERE call_id = ?2 AND kind = 'TOOL_CALLED'",
                            params![project.id, call.as_str()],
                        )?;
                    }
                    if live {
                        *self.current.lock() = Some(project.id.clone());
                        if created {
                            follow.push(AuditEvent::new(
                                EventKind::ProjectCreated,
                                event.origin.clone(),
                                format!("project registered · {} ({})", project.name, project.path),
                                json!({
                                    "projectId": project.id,
                                    "path": project.path,
                                    "name": project.name,
                                }),
                            ));
                        }
                        if let Some(entry) = crate::notes::upsert_stack(conn, &project)? {
                            follow.push(crate::notes::memory_event(
                                &entry,
                                true,
                                &CallOrigin::System,
                            ));
                        }
                    }
                    Some(project.id)
                }
                _ => self.current.lock().clone(),
            }
        } else {
            let by_id = match text_of(data, "projectId") {
                Some(id) => conn
                    .query_row("SELECT id FROM projects WHERE id = ?1", [id], |r| r.get(0))
                    .optional()?,
                None => None,
            };
            let by_path = match (by_id.is_none(), text_of(data, "projectPath")) {
                (true, Some(path)) => conn
                    .query_row("SELECT id FROM projects WHERE path = ?1", [path], |r| {
                        r.get(0)
                    })
                    .optional()?,
                _ => None,
            };
            let by_session = match (by_id.is_none() && by_path.is_none(), &session_id) {
                (true, Some(id)) => conn
                    .query_row("SELECT project_id FROM sessions WHERE id = ?1", [id], |r| {
                        r.get::<_, Option<String>>(0)
                    })
                    .optional()?
                    .flatten(),
                _ => None,
            };
            by_id
                .or(by_path)
                .or(by_session)
                .or_else(|| self.current.lock().clone())
        };

        conn.execute(
            "INSERT OR IGNORE INTO audit_events (id, at, kind, origin, call_id, summary, data, project_id, session_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                event.id.as_str(),
                ts(&event.at),
                kind_id(event.kind),
                serde_json::to_string(&event.origin).unwrap_or_default(),
                event.call_id.as_ref().map(|c| c.as_str().to_owned()),
                event.summary,
                serde_json::to_string(&event.data).unwrap_or_default(),
                project_id,
                session_id,
            ],
        )?;
        search::index_event(conn, event, project_id.as_deref())?;
        Ok(follow)
    }

    /// Imports a Phase 1–5 `audit.jsonl` (ADR-0005) once, then renames it
    /// to `audit.jsonl.imported`. Returns how many events were imported.
    pub fn import_jsonl(&self, path: &Path) -> Result<usize, String> {
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(0),
            Err(err) => return Err(format!("cannot read {}: {err}", path.display())),
        };
        let mut conn = self.db.conn.lock();
        let tx = conn.transaction().map_err(|e| e.to_string())?;
        let total = |tx: &Connection| -> Result<i64, String> {
            tx.query_row("SELECT count(*) FROM audit_events", [], |r| r.get(0))
                .map_err(|e| e.to_string())
        };
        let before = total(&tx)?;
        for line in text.lines().filter(|l| !l.trim().is_empty()) {
            // Lines that are not events (a torn last write) are skipped.
            if let Ok(event) = serde_json::from_str::<AuditEvent>(line) {
                self.record_in(&tx, &event, false)
                    .map_err(|e| e.to_string())?;
            }
        }
        let imported = (total(&tx)? - before) as usize;
        tx.commit().map_err(|e| e.to_string())?;
        let mut done = path.as_os_str().to_owned();
        done.push(".imported");
        std::fs::rename(path, &done)
            .map_err(|e| format!("imported, but cannot rename {}: {e}", path.display()))?;
        Ok(imported)
    }

    /// A page of history, newest page first (events oldest first inside the
    /// page).
    pub fn history(&self, query: &HistoryQuery) -> Result<HistoryPage, String> {
        let limit = query.limit.unwrap_or(DEFAULT_PAGE).clamp(1, MAX_PAGE);
        let mut sql = String::from("SELECT * FROM audit_events WHERE 1 = 1");
        let mut args: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
        if let Some(project) = &query.project_id {
            args.push(Box::new(project.clone()));
            sql.push_str(&format!(" AND project_id = ?{}", args.len()));
        }
        if !query.kinds.is_empty() {
            let mut marks = Vec::new();
            for kind in &query.kinds {
                args.push(Box::new(kind_id(*kind)));
                marks.push(format!("?{}", args.len()));
            }
            sql.push_str(&format!(" AND kind IN ({})", marks.join(", ")));
        }
        if let Some(text) = query
            .text
            .as_deref()
            .map(str::trim)
            .filter(|t| !t.is_empty())
        {
            args.push(Box::new(format!(
                "%{}%",
                text.replace('%', "\\%").replace('_', "\\_")
            )));
            sql.push_str(&format!(" AND summary LIKE ?{} ESCAPE '\\'", args.len()));
        }
        if query.hide_reads {
            sql.push_str(
                " AND NOT (kind = 'TOOL_CALLED' AND json_extract(data, '$.ok') = 1
                           AND json_extract(data, '$.readOnly') = 1)",
            );
        }
        if let Some((at, id)) = query.before.as_deref().and_then(|c| c.split_once('|')) {
            args.push(Box::new(at.to_owned()));
            let at_mark = args.len();
            args.push(Box::new(id.to_owned()));
            sql.push_str(&format!(
                " AND (at < ?{at_mark} OR (at = ?{at_mark} AND id < ?{}))",
                args.len()
            ));
        }
        args.push(Box::new((limit + 1) as i64));
        sql.push_str(&format!(" ORDER BY at DESC, id DESC LIMIT ?{}", args.len()));

        let conn = self.db.conn.lock();
        let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
        let params: Vec<&dyn rusqlite::ToSql> = args.iter().map(|a| a.as_ref()).collect();
        let mut events: Vec<AuditEvent> = stmt
            .query_map(params.as_slice(), event_from_row)
            .map_err(|e| e.to_string())?
            .collect::<Sql<_>>()
            .map_err(|e| e.to_string())?;
        let more = events.len() > limit;
        events.truncate(limit);
        let next = more
            .then(|| events.last().map(|e| format!("{}|{}", ts(&e.at), e.id)))
            .flatten();
        events.reverse();
        Ok(HistoryPage { events, next })
    }

    // ----------------------------------------------------------- projects

    /// Projects by last use (hidden ones left out).
    pub fn projects_recent(&self, limit: usize) -> Vec<Project> {
        let conn = self.db.conn.lock();
        let rows = conn
            .prepare(
                "SELECT * FROM projects WHERE hidden = 0 ORDER BY last_opened_at DESC LIMIT ?1",
            )
            .and_then(|mut stmt| {
                stmt.query_map([limit as i64], project_from_row)?
                    .collect::<Sql<Vec<_>>>()
            });
        rows.unwrap_or_else(|err| {
            eprintln!("[orchestrator] cannot list projects: {err}");
            Vec::new()
        })
    }

    pub fn project(&self, id: &str) -> Option<Project> {
        let conn = self.db.conn.lock();
        conn.query_row(
            "SELECT * FROM projects WHERE id = ?1",
            [id],
            project_from_row,
        )
        .optional()
        .ok()
        .flatten()
    }

    pub fn project_by_path(&self, path: &str) -> Option<Project> {
        let conn = self.db.conn.lock();
        conn.query_row(
            "SELECT * FROM projects WHERE path = ?1",
            [path],
            project_from_row,
        )
        .optional()
        .ok()
        .flatten()
    }

    pub(crate) fn current_project_id(&self) -> Option<String> {
        self.current.lock().clone()
    }

    /// The project open in the app, if any.
    pub fn current_project(&self) -> Option<Project> {
        let id = self.current.lock().clone()?;
        self.project(&id)
    }

    /// Removes a project from the recent list; its memory and history stay.
    pub fn project_forget(&self, id: &str) -> Result<(), String> {
        let conn = self.db.conn.lock();
        conn.execute("UPDATE projects SET hidden = 1 WHERE id = ?1", [id])
            .map(|_| ())
            .map_err(|e| e.to_string())
    }

    /// Imports the UI's old recent list (projects not known yet only).
    pub fn projects_import_recent(&self, list: &[RecentImport]) -> Result<usize, String> {
        let conn = self.db.conn.lock();
        let mut added = 0;
        for recent in list {
            if recent.path.trim().is_empty() {
                continue;
            }
            added += conn
                .execute(
                    "INSERT OR IGNORE INTO projects (id, path, name, created_at, last_opened_at)
                     VALUES (?1, ?2, ?3, ?4, ?4)",
                    params![new_id(), recent.path, recent.name, ts(&recent.opened_at)],
                )
                .map_err(|e| e.to_string())?;
        }
        Ok(added)
    }
}

/// The detected stack kept with the project (subset of `PROJECT_OPENED`).
fn stack_of(data: &Value) -> Value {
    let mut stack = serde_json::Map::new();
    for key in [
        "languages",
        "frameworks",
        "packageManagers",
        "runtimes",
        "databases",
        "tools",
        "docker",
        "monorepo",
    ] {
        if let Some(value) = data.get(key) {
            stack.insert(key.to_owned(), value.clone());
        }
    }
    Value::Object(stack)
}

/// Registers or refreshes a project. Returns it and whether it is new.
fn upsert_project(
    conn: &Connection,
    path: &str,
    name: &str,
    at: &DateTime<Utc>,
    data: &Value,
) -> Sql<(Project, bool)> {
    let stack = serde_json::to_string(&stack_of(data)).unwrap_or_default();
    let existing: Option<String> = conn
        .query_row("SELECT id FROM projects WHERE path = ?1", [path], |r| {
            r.get(0)
        })
        .optional()?;
    let created = existing.is_none();
    match existing {
        Some(id) => {
            conn.execute(
                "UPDATE projects SET name = ?2, last_opened_at = ?3, stack = ?4, hidden = 0 WHERE id = ?1",
                params![id, name, ts(at), stack],
            )?;
        }
        None => {
            conn.execute(
                "INSERT INTO projects (id, path, name, created_at, last_opened_at, stack)
                 VALUES (?1, ?2, ?3, ?4, ?4, ?5)",
                params![new_id(), path, name, ts(at), stack],
            )?;
        }
    }
    let project = conn.query_row(
        "SELECT * FROM projects WHERE path = ?1",
        [path],
        project_from_row,
    )?;
    Ok((project, created))
}
