//! L1 working memory, derived from the history of a project (ADR-0012):
//! recent sessions, files changed, commands run and failures.

use crate::model::{
    MemoryOverview, WorkingCommand, WorkingError, WorkingFile, WorkingMemory, WorkingSession,
};
use crate::store::{event_from_row, origin_type, MemoryStore, Sql};
use orchestrator_core::{AuditEvent, EventKind, SessionInfo};
use rusqlite::Connection;
use serde_json::Value;

const SESSIONS: usize = 8;
const FILES: usize = 15;
const COMMANDS: usize = 10;
const ERRORS: usize = 10;

fn events(conn: &Connection, sql: &str, project_id: &str, limit: usize) -> Sql<Vec<AuditEvent>> {
    let mut stmt = conn.prepare(sql)?;
    let rows = stmt.query_map(rusqlite::params![project_id, limit as i64], event_from_row)?;
    rows.collect()
}

fn count(conn: &Connection, table: &str, project_id: &str) -> usize {
    conn.query_row(
        &format!("SELECT count(*) FROM {table} WHERE project_id = ?1"),
        [project_id],
        |r| r.get::<_, i64>(0),
    )
    .map(|n| n as usize)
    .unwrap_or(0)
}

fn working(conn: &Connection, project_id: &str) -> Sql<WorkingMemory> {
    let mut stmt = conn.prepare(
        "SELECT info FROM sessions WHERE project_id = ?1 ORDER BY updated_at DESC LIMIT ?2",
    )?;
    let sessions = stmt
        .query_map(rusqlite::params![project_id, SESSIONS as i64], |r| {
            r.get::<_, String>(0)
        })?
        .filter_map(|row| serde_json::from_str::<SessionInfo>(&row.ok()?).ok())
        .map(|info| WorkingSession {
            id: info.id.to_string(),
            title: info.title,
            provider: info.provider.to_string(),
            model: info.model,
            status: info.status,
            turns: info.turns,
            updated_at: info.updated_at,
        })
        .collect();

    let mut files: Vec<WorkingFile> = Vec::new();
    for event in events(
        conn,
        "SELECT * FROM audit_events WHERE project_id = ?1 AND kind = 'FILE_CHANGED'
         ORDER BY at DESC LIMIT ?2",
        project_id,
        FILES * 4,
    )? {
        let data = &event.data;
        let path = data
            .get("path")
            .or_else(|| data.get("to"))
            .and_then(Value::as_str)
            .unwrap_or_default();
        if path.is_empty() || files.iter().any(|f| f.path == path) {
            continue;
        }
        files.push(WorkingFile {
            path: path.to_owned(),
            change: data
                .get("change")
                .and_then(Value::as_str)
                .unwrap_or("changed")
                .to_owned(),
            at: event.at,
            by: origin_type(&event.origin).to_owned(),
        });
        if files.len() == FILES {
            break;
        }
    }

    let commands = events(
        conn,
        "SELECT * FROM audit_events WHERE project_id = ?1 AND kind = 'COMMAND_EXECUTED'
         ORDER BY at DESC LIMIT ?2",
        project_id,
        COMMANDS,
    )?
    .into_iter()
    .map(|event| WorkingCommand {
        command: event
            .data
            .get("command")
            .and_then(Value::as_str)
            .unwrap_or(&event.summary)
            .to_owned(),
        exit_code: event.data.get("exitCode").and_then(Value::as_i64),
        background: event.data.get("background").and_then(Value::as_bool) == Some(true),
        at: event.at,
        by: origin_type(&event.origin).to_owned(),
    })
    .collect();

    let errors = events(
        conn,
        "SELECT * FROM audit_events WHERE project_id = ?1 AND (
             (kind = 'TOOL_CALLED' AND json_extract(data, '$.ok') = 0)
          OR (kind = 'TURN_COMPLETED' AND json_extract(data, '$.status') = 'failed')
          OR (kind = 'COMMAND_EXECUTED' AND json_extract(data, '$.exitCode') IS NOT NULL
              AND json_extract(data, '$.exitCode') != 0)
          OR (kind = 'PROCESS_EXITED' AND json_extract(data, '$.exitCode') IS NOT NULL
              AND json_extract(data, '$.exitCode') != 0
              AND COALESCE(json_extract(data, '$.stopped'), 0) = 0))
         ORDER BY at DESC LIMIT ?2",
        project_id,
        ERRORS,
    )?
    .into_iter()
    .map(|event| WorkingError {
        detail: match event.kind {
            EventKind::ToolCalled => event
                .data
                .pointer("/error/message")
                .and_then(Value::as_str)
                .map(str::to_owned),
            EventKind::TurnCompleted => event
                .data
                .get("error")
                .and_then(Value::as_str)
                .map(str::to_owned),
            EventKind::CommandExecuted => event
                .data
                .get("stderrTail")
                .and_then(Value::as_str)
                .filter(|s| !s.trim().is_empty())
                .map(str::to_owned),
            _ => None,
        },
        kind: event.kind,
        summary: event.summary,
        at: event.at,
    })
    .collect();

    Ok(WorkingMemory {
        sessions,
        files,
        commands,
        errors,
    })
}

impl MemoryStore {
    /// L1 of a project.
    pub fn working_memory(&self, project_id: &str) -> Result<WorkingMemory, String> {
        let conn = self.db.conn.lock();
        working(&conn, project_id).map_err(|e| e.to_string())
    }

    /// L1 plus how much the project has in L2 and L3.
    pub fn overview(&self, project_id: &str) -> Result<MemoryOverview, String> {
        let project = self
            .project(project_id)
            .ok_or_else(|| format!("projeto {project_id} não encontrado"))?;
        let conn = self.db.conn.lock();
        Ok(MemoryOverview {
            working: working(&conn, project_id).map_err(|e| e.to_string())?,
            memory_entries: count(&conn, "memory_entries", project_id),
            decisions: count(&conn, "decisions", project_id),
            sessions: count(&conn, "sessions", project_id),
            events: count(&conn, "audit_events", project_id),
            project,
        })
    }
}
