//! Council deliberations (ADR-0011, ADR-0012): kept for the history and,
//! while valid, reused as the Council cache across restarts.

use crate::store::{ts, MemoryStore, Sql};
use chrono::{DateTime, Utc};
use rusqlite::{params, OptionalExtension};
use serde_json::Value;

impl MemoryStore {
    /// Stores a deliberation (JSON). With `cache`, it may answer the same
    /// question until `valid_until`.
    pub fn deliberation_save(
        &self,
        id: &str,
        created_at: &DateTime<Utc>,
        cache: Option<(&str, DateTime<Utc>)>,
        data: &Value,
    ) -> Result<(), String> {
        let conn = self.db.conn.lock();
        let project_id: Option<String> = self.current_project_id();
        conn.execute(
            "INSERT OR REPLACE INTO deliberations (id, project_id, created_at, cache_key, valid_until, data)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                id,
                project_id,
                ts(created_at),
                cache.map(|(key, _)| key.to_owned()),
                cache.map(|(_, until)| ts(&until)),
                serde_json::to_string(data).map_err(|e| e.to_string())?,
            ],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    }

    /// The newest deliberation stored under `key` and still valid at `now`.
    pub fn deliberation_cached(&self, key: &str, now: &DateTime<Utc>) -> Option<Value> {
        let conn = self.db.conn.lock();
        conn.query_row(
            "SELECT data FROM deliberations WHERE cache_key = ?1 AND valid_until > ?2
             ORDER BY created_at DESC LIMIT 1",
            params![key, ts(now)],
            |r| r.get::<_, String>(0),
        )
        .optional()
        .ok()
        .flatten()
        .and_then(|text| serde_json::from_str(&text).ok())
    }

    /// Forgets every cache entry (the deliberations stay in the history).
    pub fn deliberations_clear_cache(&self) -> Result<(), String> {
        let conn = self.db.conn.lock();
        conn.execute(
            "UPDATE deliberations SET cache_key = NULL, valid_until = NULL WHERE cache_key IS NOT NULL",
            [],
        )
        .map(|_| ())
        .map_err(|e| e.to_string())
    }

    /// Newest first.
    pub fn deliberations_recent(&self, limit: usize) -> Vec<Value> {
        let conn = self.db.conn.lock();
        let rows = conn
            .prepare("SELECT data FROM deliberations ORDER BY created_at DESC LIMIT ?1")
            .and_then(|mut stmt| {
                stmt.query_map([limit as i64], |r| r.get::<_, String>(0))?
                    .collect::<Sql<Vec<String>>>()
            })
            .unwrap_or_default();
        rows.iter()
            .filter_map(|text| serde_json::from_str(text).ok())
            .collect()
    }
}
