//! Handoffs between AIs and the facts they are built from (ADR-0013).

use crate::model::SessionFacts;
use crate::search;
use crate::store::{event_from_row, parse_ts, ts, MemoryStore, Sql};
use crate::working::{command_of, error_of, file_of};
use chrono::{DateTime, Utc};
use orchestrator_core::{EventKind, Handoff, HandoffEnd, HandoffStatus, SessionEvent};
use rusqlite::{params, Connection, OptionalExtension};

const FILES: usize = 20;
const COMMANDS: usize = 15;
const ERRORS: usize = 10;
const INPUTS: usize = 5;

fn status_text(status: HandoffStatus) -> &'static str {
    match status {
        HandoffStatus::Created => "created",
        HandoffStatus::Accepted => "accepted",
    }
}

fn save(conn: &Connection, handoff: &Handoff) -> Sql<()> {
    let data = serde_json::to_string(handoff).expect("a handoff serializes");
    conn.execute(
        "INSERT INTO handoffs (id, project_id, project_path, from_session, to_session, status, created_at, accepted_at, data)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
         ON CONFLICT(id) DO UPDATE SET
             project_id = excluded.project_id, to_session = excluded.to_session,
             status = excluded.status, accepted_at = excluded.accepted_at, data = excluded.data",
        params![
            handoff.id.as_str(),
            handoff.project_id,
            handoff.project_path,
            handoff.from.session_id.as_str(),
            handoff.to.as_ref().map(|t| t.session_id.as_str().to_owned()),
            status_text(handoff.status),
            ts(&handoff.created_at),
            handoff.accepted_at.as_ref().map(ts),
            data,
        ],
    )?;
    search::index_handoff(conn, handoff)
}

fn load(conn: &Connection, id: &str) -> Sql<Option<Handoff>> {
    conn.query_row("SELECT data FROM handoffs WHERE id = ?1", [id], |r| {
        r.get::<_, String>(0)
    })
    .optional()
    .map(|data| data.and_then(|d| serde_json::from_str(&d).ok()))
}

impl MemoryStore {
    /// Records a handoff (new or changed) and indexes it for L3.
    pub fn handoff_save(&self, handoff: &Handoff) -> Result<(), String> {
        let conn = self.db.conn.lock();
        save(&conn, handoff).map_err(|e| e.to_string())
    }

    pub fn handoff(&self, id: &str) -> Option<Handoff> {
        let conn = self.db.conn.lock();
        load(&conn, id).ok().flatten()
    }

    /// Handoffs of a project (all projects with `None`), newest first.
    pub fn handoffs_list(&self, project_id: Option<&str>, limit: usize) -> Vec<Handoff> {
        let conn = self.db.conn.lock();
        let rows = conn
            .prepare(
                "SELECT data FROM handoffs WHERE (?1 IS NULL OR project_id = ?1)
                 ORDER BY created_at DESC LIMIT ?2",
            )
            .and_then(|mut stmt| {
                stmt.query_map(params![project_id, limit.clamp(1, 500) as i64], |r| {
                    r.get::<_, String>(0)
                })?
                .collect::<Sql<Vec<String>>>()
            });
        rows.unwrap_or_default()
            .iter()
            .filter_map(|data| serde_json::from_str(data).ok())
            .collect()
    }

    /// Marks a handoff as taken over by `to`. A handoff is accepted once.
    pub fn handoff_accept(
        &self,
        id: &str,
        to: HandoffEnd,
        at: DateTime<Utc>,
    ) -> Result<Handoff, String> {
        let conn = self.db.conn.lock();
        let mut handoff = load(&conn, id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("handoff {id} não encontrado"))?;
        if handoff.status == HandoffStatus::Accepted {
            let by = handoff
                .to
                .as_ref()
                .map(|t| t.title.as_str())
                .unwrap_or("outra sessão");
            return Err(format!("este handoff já foi assumido por {by}"));
        }
        handoff.status = HandoffStatus::Accepted;
        handoff.to = Some(to);
        handoff.accepted_at = Some(at);
        save(&conn, &handoff).map_err(|e| e.to_string())?;
        Ok(handoff)
    }

    /// Project of a session, when known.
    pub fn session_project_id(&self, session_id: &str) -> Option<String> {
        let conn = self.db.conn.lock();
        conn.query_row(
            "SELECT project_id FROM sessions WHERE id = ?1",
            [session_id],
            |r| r.get::<_, Option<String>>(0),
        )
        .optional()
        .ok()
        .flatten()
        .flatten()
    }

    /// What the history says a session did: its messages, and the files,
    /// commands and failures of the session and of the user while it was
    /// open.
    pub fn session_facts(&self, session_id: &str) -> Result<SessionFacts, String> {
        let conn = self.db.conn.lock();
        facts(&conn, session_id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("sessão {session_id} não encontrada"))
    }
}

fn facts(conn: &Connection, session_id: &str) -> Sql<Option<SessionFacts>> {
    let Some((title, project_id, created_at)) = conn
        .query_row(
            "SELECT title, project_id, created_at FROM sessions WHERE id = ?1",
            [session_id],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, Option<String>>(1)?,
                    r.get::<_, String>(2)?,
                ))
            },
        )
        .optional()?
    else {
        return Ok(None);
    };

    let mut inputs = Vec::new();
    let mut stmt = conn.prepare(
        "SELECT event FROM session_entries
         WHERE session_id = ?1 AND json_extract(event, '$.type') = 'turnStarted'
         ORDER BY seq LIMIT ?2",
    )?;
    for event in stmt.query_map(params![session_id, INPUTS as i64], |r| {
        r.get::<_, String>(0)
    })? {
        if let Ok(SessionEvent::TurnStarted { input, .. }) = serde_json::from_str(&event?) {
            inputs.push(input);
        }
    }

    // The session's own events, plus what the user did in the project while
    // it was open (edits, commands).
    let mut stmt = conn.prepare(
        "SELECT * FROM audit_events
         WHERE (session_id = ?1 OR (?2 IS NOT NULL AND project_id = ?2 AND session_id IS NULL
                                   AND at >= ?3 AND kind IN ('FILE_CHANGED', 'COMMAND_EXECUTED')))
           AND kind IN ('FILE_CHANGED', 'COMMAND_EXECUTED', 'TOOL_CALLED', 'TURN_COMPLETED',
                        'PROCESS_EXITED')
         ORDER BY at DESC LIMIT 600",
    )?;
    let events = stmt
        .query_map(params![session_id, project_id, created_at], event_from_row)?
        .collect::<Sql<Vec<_>>>()?;

    let mut files = Vec::new();
    let mut commands = Vec::new();
    let mut errors = Vec::new();
    for event in events {
        match event.kind {
            EventKind::FileChanged => {
                if let Some(file) = file_of(&event) {
                    if files.len() < FILES
                        && !files
                            .iter()
                            .any(|f: &crate::WorkingFile| f.path == file.path)
                    {
                        files.push(file);
                    }
                }
            }
            EventKind::CommandExecuted if commands.len() < COMMANDS => {
                commands.push(command_of(&event))
            }
            _ => {}
        }
        if errors.len() < ERRORS {
            if let Some(error) = error_of(event) {
                errors.push(error);
            }
        }
    }

    Ok(Some(SessionFacts {
        session_id: session_id.to_owned(),
        project_id,
        title,
        created_at: parse_ts(&created_at),
        inputs,
        files,
        commands,
        errors,
    }))
}
