//! Provider sessions and their transcripts (ADR-0012): saved when a
//! session opens, closes or resumes and at the end of every turn; loaded
//! when the app starts.

use crate::model::StoredSession;
use crate::search;
use crate::store::{ts, MemoryStore, Sql};
use orchestrator_core::{SessionEvent, SessionId, SessionInfo, SessionLogEntry, SessionStatus};
use rusqlite::{params, OptionalExtension};
use serde_json::Value;

fn status_id(status: SessionStatus) -> &'static str {
    match status {
        SessionStatus::Idle => "idle",
        SessionStatus::Running => "running",
        SessionStatus::Closed => "closed",
    }
}

impl MemoryStore {
    /// Inserts or updates a session. Its provider can change: a reserve
    /// takes over when the session's AI fails (ADR-0024).
    pub fn session_save(&self, session: &StoredSession) -> Result<(), String> {
        let info = &session.info;
        let conn = self.db.conn.lock();
        let project_path = info.project_path.to_string_lossy().to_string();
        let project_id: Option<String> = conn
            .query_row(
                "SELECT id FROM projects WHERE path = ?1",
                [&project_path],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| e.to_string())?;
        conn.execute(
            "INSERT INTO sessions (id, project_id, project_path, provider, model, title, parent_id, status,
                                   created_at, updated_at, info, native, spec)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
             ON CONFLICT(id) DO UPDATE SET
                project_id = COALESCE(excluded.project_id, sessions.project_id),
                provider = excluded.provider, model = excluded.model,
                title = excluded.title, status = excluded.status,
                updated_at = excluded.updated_at, info = excluded.info,
                native = excluded.native, spec = excluded.spec",
            params![
                info.id.as_str(),
                project_id,
                project_path,
                info.provider.as_str(),
                info.model,
                info.title,
                info.parent_id.as_ref().map(|p| p.as_str().to_owned()),
                status_id(info.status),
                ts(&info.created_at),
                ts(&info.updated_at),
                serde_json::to_string(info).map_err(|e| e.to_string())?,
                serde_json::to_string(&session.native).map_err(|e| e.to_string())?,
                serde_json::to_string(&session.spec).map_err(|e| e.to_string())?,
            ],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    }

    /// Adds transcript entries (an entry already stored with the same `seq`
    /// is replaced). User inputs and assistant text are indexed for search.
    pub fn session_append(
        &self,
        session_id: &SessionId,
        entries: &[SessionLogEntry],
    ) -> Result<(), String> {
        if entries.is_empty() {
            return Ok(());
        }
        let mut conn = self.db.conn.lock();
        let tx = conn.transaction().map_err(|e| e.to_string())?;
        let (title, project_id): (String, Option<String>) = tx
            .query_row(
                "SELECT title, project_id FROM sessions WHERE id = ?1",
                [session_id.as_str()],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("session {session_id} is not stored"))?;
        for entry in entries {
            tx.execute(
                "INSERT OR REPLACE INTO session_entries (session_id, seq, at, event) VALUES (?1, ?2, ?3, ?4)",
                params![
                    session_id.as_str(),
                    entry.seq as i64,
                    ts(&entry.at),
                    serde_json::to_string(&entry.event).map_err(|e| e.to_string())?,
                ],
            )
            .map_err(|e| e.to_string())?;
            let text = match &entry.event {
                SessionEvent::TurnStarted { input, .. } => Some(input.as_str()),
                SessionEvent::TextDelta { text, .. } => Some(text.as_str()),
                _ => None,
            };
            if let Some(text) = text.filter(|t| !t.trim().is_empty()) {
                search::index_message(
                    &tx,
                    session_id.as_str(),
                    entry.seq,
                    project_id.as_deref(),
                    &entry.at,
                    &title,
                    text,
                )
                .map_err(|e| e.to_string())?;
            }
        }
        tx.commit().map_err(|e| e.to_string())
    }

    /// Every stored session (oldest first) with its transcript.
    pub fn sessions_load(&self) -> Result<Vec<(StoredSession, Vec<SessionLogEntry>)>, String> {
        let conn = self.db.conn.lock();
        let mut stmt = conn
            .prepare("SELECT id, info, native, spec FROM sessions ORDER BY created_at, id")
            .map_err(|e| e.to_string())?;
        let rows: Vec<(String, String, String, String)> = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
            .map_err(|e| e.to_string())?
            .collect::<Sql<_>>()
            .map_err(|e| e.to_string())?;
        let mut entries_stmt = conn
            .prepare(
                "SELECT seq, at, event FROM session_entries WHERE session_id = ?1 ORDER BY seq",
            )
            .map_err(|e| e.to_string())?;
        let mut out = Vec::new();
        for (id, info, native, spec) in rows {
            let Ok(info) = serde_json::from_str::<SessionInfo>(&info) else {
                eprintln!("[orchestrator] session {id} ignored: unreadable info");
                continue;
            };
            let entries: Vec<SessionLogEntry> = entries_stmt
                .query_map([&id], |r| {
                    Ok((
                        r.get::<_, i64>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                    ))
                })
                .map_err(|e| e.to_string())?
                .filter_map(|row| {
                    let (seq, at, event) = row.ok()?;
                    Some(SessionLogEntry {
                        seq: seq as u64,
                        at: crate::store::parse_ts(&at),
                        event: serde_json::from_str(&event).ok()?,
                    })
                })
                .collect();
            out.push((
                StoredSession {
                    info,
                    native: serde_json::from_str(&native).unwrap_or(Value::Null),
                    spec: serde_json::from_str(&spec).unwrap_or(Value::Null),
                },
                entries,
            ));
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use orchestrator_core::TokenUsage;
    use serde_json::json;

    #[test]
    fn a_session_taken_over_by_a_reserve_is_saved_with_its_new_provider() {
        let store = MemoryStore::in_memory();
        let now = Utc::now();
        let mut info = SessionInfo {
            id: SessionId::new(),
            provider: "claude".into(),
            title: "Timeout".into(),
            model: Some("claude-1".into()),
            project_path: "/p/fila".into(),
            parent_id: None,
            status: SessionStatus::Idle,
            native_ref: None,
            created_at: now,
            updated_at: now,
            turns: 0,
            usage: TokenUsage::default(),
            last_error: None,
        };
        let save = |info: &SessionInfo| {
            store
                .session_save(&StoredSession {
                    info: info.clone(),
                    native: json!({}),
                    spec: json!({}),
                })
                .unwrap()
        };
        save(&info);
        info.provider = "gemini".into();
        info.model = Some("gemini-1".into());
        save(&info);
        let row: (String, String) = store
            .db
            .conn
            .lock()
            .query_row(
                "SELECT provider, model FROM sessions WHERE id = ?1",
                [info.id.as_str()],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(row, ("gemini".to_owned(), "gemini-1".to_owned()));
    }
}
