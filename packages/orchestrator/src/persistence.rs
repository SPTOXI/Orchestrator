//! Provider sessions in the database (ADR-0012): the `SessionStore` of the
//! provider layer over `orchestrator-memory`, which it does not depend on.

use orchestrator_core::{SessionId, SessionLogEntry};
use orchestrator_memory::{MemoryStore, StoredSession};
use orchestrator_providers::{ContextOptions, NativeSession, PersistedSession, SessionStore};
use serde_json::{json, Value};
use std::sync::Arc;

/// Provider sessions and transcripts in the database. The `spec` column
/// keeps the instructions, the model asked for and the context options
/// (ADR-0013).
pub struct StoreSessions(pub Arc<MemoryStore>);

impl SessionStore for StoreSessions {
    fn load(&self) -> Vec<(PersistedSession, Vec<SessionLogEntry>)> {
        let rows = match self.0.sessions_load() {
            Ok(rows) => rows,
            Err(err) => {
                eprintln!("[orchestrator] cannot load sessions: {err}");
                return Vec::new();
            }
        };
        rows.into_iter()
            .filter_map(|(stored, entries)| {
                let native: NativeSession = serde_json::from_value(stored.native).ok()?;
                let text = |key: &str| {
                    stored
                        .spec
                        .get(key)
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                };
                // Sessions saved before Phase 7 have no context options.
                let context: ContextOptions = stored
                    .spec
                    .get("context")
                    .and_then(|v| serde_json::from_value(v.clone()).ok())
                    .unwrap_or_default();
                Some((
                    PersistedSession {
                        instructions: text("instructions"),
                        requested_model: text("requestedModel"),
                        context,
                        info: stored.info,
                        native,
                    },
                    entries,
                ))
            })
            .collect()
    }

    fn save(&self, session: &PersistedSession) {
        let stored = StoredSession {
            info: session.info.clone(),
            native: serde_json::to_value(&session.native).unwrap_or(Value::Null),
            spec: json!({
                "instructions": session.instructions,
                "requestedModel": session.requested_model,
                "context": session.context,
            }),
        };
        if let Err(err) = self.0.session_save(&stored) {
            eprintln!(
                "[orchestrator] cannot save session {}: {err}",
                session.info.id
            );
        }
    }

    fn append(&self, id: &SessionId, entries: &[SessionLogEntry]) {
        if let Err(err) = self.0.session_append(id, entries) {
            eprintln!("[orchestrator] cannot save the transcript of {id}: {err}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use orchestrator_core::{
        HandoffId, SessionEvent, SessionInfo, SessionStatus, TokenUsage, TurnId,
    };

    #[test]
    fn sessions_round_trip_through_the_database() {
        let store = Arc::new(MemoryStore::in_memory());
        let sessions = StoreSessions(store.clone());
        let now = Utc::now();
        let id = SessionId::new();
        let session = PersistedSession {
            info: SessionInfo {
                id: id.clone(),
                provider: "nuvem".into(),
                title: "Sessão".into(),
                model: Some("gpt-medio".into()),
                project_path: "/p/a".into(),
                parent_id: None,
                status: SessionStatus::Idle,
                native_ref: Some("nuvem-1".into()),
                created_at: now,
                updated_at: now,
                turns: 1,
                usage: TokenUsage::default(),
                last_error: None,
            },
            native: NativeSession {
                reference: "nuvem-1".into(),
                model: Some("gpt-medio".into()),
                data: json!({"conversation": {"messages": [{"role": "user", "parts": []}]}}),
            },
            instructions: Some("seja breve".into()),
            requested_model: None,
            context: ContextOptions {
                enabled: Some(true),
                budget: Some(900),
                handoff_id: Some(HandoffId::new()),
            },
        };
        sessions.save(&session);
        let entry = SessionLogEntry {
            seq: 3,
            at: now,
            event: SessionEvent::TurnStarted {
                turn_id: TurnId::new(),
                input: "olá".into(),
            },
        };
        sessions.append(&id, std::slice::from_ref(&entry));
        let loaded = sessions.load();
        assert_eq!(loaded, vec![(session.clone(), vec![entry])]);

        // A Phase 6 row (no context options) loads with the defaults.
        let old = StoredSession {
            info: SessionInfo {
                id: SessionId::new(),
                ..session.info.clone()
            },
            native: serde_json::to_value(&session.native).unwrap(),
            spec: json!({"instructions": null, "requestedModel": null}),
        };
        store.session_save(&old).unwrap();
        let loaded = sessions.load();
        let restored = loaded
            .iter()
            .find(|(s, _)| s.info.id == old.info.id)
            .unwrap();
        assert_eq!(restored.0.context, ContextOptions::default());
    }
}
