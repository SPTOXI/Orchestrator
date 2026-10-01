//! What the AIs spent, from the history (ADR-0018): every turn records its
//! usage (`TURN_COMPLETED`), every Council deliberation its own
//! (`COUNCIL_DELIBERATED`). Nothing is stored twice.

use crate::store::{ts, MemoryStore, Sql};
use chrono::{DateTime, Utc};
use rusqlite::params;
use serde::Serialize;

/// Spending of one provider and model.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SpendRow {
    /// Provider (connection) id; `conselho` for Council deliberations.
    pub provider: String,
    pub model: Option<String>,
    /// Turns (or deliberations).
    pub calls: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cached_input_tokens: u64,
    pub cache_write_tokens: u64,
    /// Known cost; calls without a price add nothing here.
    pub cost_usd: f64,
    pub cache_saved_usd: f64,
    /// Calls that used tokens but had no price: the real cost is higher.
    pub unpriced: u64,
}

/// Spending of a project (or of every project) since a moment.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SpendReport {
    pub since: DateTime<Utc>,
    pub cost_usd: f64,
    pub cache_saved_usd: f64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cached_input_tokens: u64,
    pub cache_write_tokens: u64,
    pub calls: u64,
    pub unpriced: u64,
    /// Conversations compacted (`CONTEXT_COMPACTED`).
    pub compactions: u64,
    /// Most expensive first.
    pub rows: Vec<SpendRow>,
}

const ROWS: &str = "
SELECT provider, model,
       COUNT(*),
       COALESCE(SUM(input), 0), COALESCE(SUM(output), 0),
       COALESCE(SUM(cached), 0), COALESCE(SUM(written), 0),
       COALESCE(SUM(cost), 0.0), COALESCE(SUM(saved), 0.0),
       SUM(CASE WHEN cost IS NULL AND (input > 0 OR output > 0) THEN 1 ELSE 0 END)
FROM (
    SELECT COALESCE(json_extract(data, '$.provider'), '?') AS provider,
           json_extract(data, '$.model') AS model,
           json_extract(data, '$.usage.inputTokens') AS input,
           json_extract(data, '$.usage.outputTokens') AS output,
           json_extract(data, '$.usage.cachedInputTokens') AS cached,
           json_extract(data, '$.usage.cacheWriteTokens') AS written,
           json_extract(data, '$.usage.costUsd') AS cost,
           json_extract(data, '$.usage.cacheSavedUsd') AS saved
    FROM audit_events
    WHERE kind = 'TURN_COMPLETED' AND at >= ?1 AND (?2 IS NULL OR project_id = ?2)
    UNION ALL
    SELECT 'conselho', NULL,
           json_extract(data, '$.usage.inputTokens'),
           json_extract(data, '$.usage.outputTokens'),
           json_extract(data, '$.usage.cachedInputTokens'),
           json_extract(data, '$.usage.cacheWriteTokens'),
           json_extract(data, '$.usage.costUsd'),
           json_extract(data, '$.usage.cacheSavedUsd')
    FROM audit_events
    WHERE kind = 'COUNCIL_DELIBERATED' AND at >= ?1 AND (?2 IS NULL OR project_id = ?2)
      AND COALESCE(json_extract(data, '$.cached'), 0) = 0
)
GROUP BY provider, model";

impl MemoryStore {
    /// What the AIs of `project_id` (every project when `None`) spent since
    /// `since`. Never fails: an unreadable history reports nothing.
    pub fn spend(&self, project_id: Option<&str>, since: DateTime<Utc>) -> SpendReport {
        let conn = self.db.conn.lock();
        let read = || -> Sql<SpendReport> {
            let mut stmt = conn.prepare(ROWS)?;
            let mut rows: Vec<SpendRow> = stmt
                .query_map(params![ts(&since), project_id], |r| {
                    Ok(SpendRow {
                        provider: r.get(0)?,
                        model: r.get(1)?,
                        calls: r.get::<_, i64>(2)? as u64,
                        input_tokens: r.get::<_, i64>(3)? as u64,
                        output_tokens: r.get::<_, i64>(4)? as u64,
                        cached_input_tokens: r.get::<_, i64>(5)? as u64,
                        cache_write_tokens: r.get::<_, i64>(6)? as u64,
                        cost_usd: r.get(7)?,
                        cache_saved_usd: r.get(8)?,
                        unpriced: r.get::<_, i64>(9)? as u64,
                    })
                })?
                .collect::<Sql<_>>()?;
            rows.sort_by(|a, b| {
                b.cost_usd
                    .total_cmp(&a.cost_usd)
                    .then(b.input_tokens.cmp(&a.input_tokens))
            });
            let compactions: i64 = conn.query_row(
                "SELECT COUNT(*) FROM audit_events
                 WHERE kind = 'CONTEXT_COMPACTED' AND at >= ?1
                   AND (?2 IS NULL OR project_id = ?2)",
                params![ts(&since), project_id],
                |r| r.get(0),
            )?;
            let mut report = SpendReport {
                since,
                compactions: compactions as u64,
                ..Default::default()
            };
            for row in &rows {
                report.cost_usd += row.cost_usd;
                report.cache_saved_usd += row.cache_saved_usd;
                report.input_tokens += row.input_tokens;
                report.output_tokens += row.output_tokens;
                report.cached_input_tokens += row.cached_input_tokens;
                report.cache_write_tokens += row.cache_write_tokens;
                report.calls += row.calls;
                report.unpriced += row.unpriced;
            }
            report.rows = rows;
            Ok(report)
        };
        read().unwrap_or_else(|err| {
            eprintln!("[orchestrator] cannot read spending: {err}");
            SpendReport {
                since,
                ..Default::default()
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use crate::MemoryStore;
    use chrono::{Duration, Utc};
    use orchestrator_core::{AuditEvent, CallOrigin, EventKind};
    use serde_json::json;

    fn turn(project: &str, provider: &str, model: &str, usage: serde_json::Value) -> AuditEvent {
        AuditEvent::new(
            EventKind::TurnCompleted,
            CallOrigin::User,
            "turn",
            json!({"projectId": project, "provider": provider, "model": model, "usage": usage}),
        )
    }

    #[test]
    fn spending_adds_turns_and_deliberations_by_model() {
        let store = MemoryStore::in_memory();
        for (path, name) in [("/p/a", "a"), ("/p/b", "b")] {
            store.record(&AuditEvent::new(
                EventKind::ProjectOpened,
                CallOrigin::User,
                "open",
                json!({"path": path, "name": name}),
            ));
        }
        let a = store.project_by_path("/p/a").unwrap().id;
        let b = store.project_by_path("/p/b").unwrap().id;
        let usage = |cost: Option<f64>| {
            json!({"inputTokens": 1000, "outputTokens": 100, "cachedInputTokens": 800,
                   "cacheWriteTokens": 0, "costUsd": cost, "cacheSavedUsd": cost.map(|_| 0.5)})
        };
        store.record(&turn(&a, "claude", "opus", usage(Some(1.0))));
        store.record(&turn(&a, "claude", "opus", usage(Some(2.0))));
        store.record(&turn(&a, "local", "llama", usage(None)));
        store.record(&turn(&b, "claude", "opus", usage(Some(9.0))));
        store.record(&AuditEvent::new(
            EventKind::CouncilDeliberated,
            CallOrigin::User,
            "council",
            json!({"projectId": a, "cached": false,
                   "usage": {"inputTokens": 50, "outputTokens": 5, "costUsd": 0.25}}),
        ));
        store.record(&AuditEvent::new(
            EventKind::CouncilDeliberated,
            CallOrigin::User,
            "council from cache",
            json!({"projectId": a, "cached": true,
                   "usage": {"inputTokens": 50, "outputTokens": 5, "costUsd": 0.25}}),
        ));
        store.record(&AuditEvent::new(
            EventKind::ContextCompacted,
            CallOrigin::User,
            "compacted",
            json!({"projectId": a}),
        ));

        let since = Utc::now() - Duration::hours(1);
        let report = store.spend(Some(&a), since);
        assert!((report.cost_usd - 3.25).abs() < 1e-9, "{report:?}");
        assert!((report.cache_saved_usd - 1.0).abs() < 1e-9);
        assert_eq!(report.calls, 4);
        assert_eq!(report.unpriced, 1, "the local model has no price");
        assert_eq!(report.cached_input_tokens, 2400);
        assert_eq!(report.compactions, 1);
        assert_eq!(report.rows[0].provider, "claude");
        assert_eq!(report.rows[0].calls, 2);
        assert!(report.rows.iter().any(|r| r.provider == "conselho"));

        let all = store.spend(None, since);
        assert!((all.cost_usd - 12.25).abs() < 1e-9);
        assert_eq!(
            store.spend(Some(&a), Utc::now() + Duration::hours(1)).calls,
            0
        );
    }
}
