//! Glue between the database (`orchestrator-memory`, ADR-0012) and the
//! router, which does not depend on it. Provider sessions use the engine's
//! `StoreSessions` (ADR-0013).

use orchestrator_memory::MemoryStore;
use orchestrator_router::{Deliberation, DeliberationStore};
use std::sync::Arc;

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
    use orchestrator_core::{DeliberationId, TokenUsage};
    use orchestrator_router::{Activity, CouncilMode, Preference, Recommendation};

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
