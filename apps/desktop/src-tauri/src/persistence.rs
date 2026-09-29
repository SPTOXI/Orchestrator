//! Glue between the database (`orchestrator-memory`, ADR-0012) and the
//! traits of the provider layer and the router, which do not depend on it.

use orchestrator_core::{SessionId, SessionLogEntry};
use orchestrator_memory::{MemoryStore, StoredSession};
use orchestrator_providers::{NativeSession, PersistedSession, SessionStore};
use orchestrator_router::{Deliberation, DeliberationStore};
use serde_json::{json, Value};
use std::sync::Arc;

/// Provider sessions and transcripts in the database.
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
                Some((
                    PersistedSession {
                        instructions: text("instructions"),
                        requested_model: text("requestedModel"),
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

/// Council deliberations (history and cache) in the database.
pub struct StoreDeliberations(pub Arc<MemoryStore>);

impl DeliberationStore for StoreDeliberations {
    fn save(
        &self,
        deliberation: &Deliberation,
        cache: Option<(&str, chrono::DateTime<chrono::Utc>)>,
    ) {
        let saved = serde_json::to_value(deliberation)
            .map_err(|e| e.to_string())
            .and_then(|value| {
                self.0.deliberation_save(
                    deliberation.id.as_str(),
                    &deliberation.created_at,
                    cache,
                    &value,
                )
            });
        if let Err(err) = saved {
            eprintln!(
                "[orchestrator] cannot save deliberation {}: {err}",
                deliberation.id
            );
        }
    }

    fn cached(&self, key: &str, now: chrono::DateTime<chrono::Utc>) -> Option<Deliberation> {
        self.0
            .deliberation_cached(key, &now)
            .and_then(|value| serde_json::from_value(value).ok())
    }

    fn recent(&self, limit: usize) -> Vec<Deliberation> {
        self.0
            .deliberations_recent(limit)
            .into_iter()
            .filter_map(|value| serde_json::from_value(value).ok())
            .collect()
    }

    fn clear_cache(&self) {
        if let Err(err) = self.0.deliberations_clear_cache() {
            eprintln!("[orchestrator] cannot clear the Council cache: {err}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Duration, Utc};
    use orchestrator_core::{
        DeliberationId, SessionEvent, SessionInfo, SessionStatus, TokenUsage, TurnId,
    };
    use orchestrator_router::{Activity, CouncilMode, Preference, Recommendation};

    #[test]
    fn sessions_round_trip_through_the_database() {
        let store = Arc::new(MemoryStore::in_memory());
        let sessions = StoreSessions(store);
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
        assert_eq!(loaded, vec![(session, vec![entry])]);
    }

    #[test]
    fn deliberations_round_trip_through_the_database() {
        let store = Arc::new(MemoryStore::in_memory());
        let deliberations = StoreDeliberations(store);
        let deliberation = Deliberation {
            id: DeliberationId::new(),
            created_at: Utc::now(),
            task: "Planeje as filas".into(),
            mode: CouncilMode::Suggest,
            recommendation: Recommendation {
                activity: Activity::Planning,
                detected: true,
                preference: Preference::Quality,
                needs_tools: false,
                min_context: None,
                candidates: Vec::new(),
                excluded: Vec::new(),
            },
            shortlist: Vec::new(),
            votes: Vec::new(),
            decision: None,
            usage: TokenUsage::default(),
            cached: false,
            cached_from: None,
            saved_usage: None,
            notices: vec!["aviso".into()],
            auto_apply: false,
            duration_ms: 12,
        };
        let until = deliberation.created_at + Duration::minutes(60);
        deliberations.save(&deliberation, Some(("0000abcd", until)));
        assert_eq!(
            deliberations.cached("0000abcd", Utc::now()).as_ref(),
            Some(&deliberation)
        );
        assert_eq!(deliberations.recent(5), vec![deliberation]);
        deliberations.clear_cache();
        assert!(deliberations.cached("0000abcd", Utc::now()).is_none());
    }
}
