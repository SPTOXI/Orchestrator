//! Where the `RouterService` keeps deliberations between runs (ADR-0012):
//! the history of the Council and, while valid, its cache.

use crate::council::Deliberation;
use chrono::{DateTime, Utc};

pub trait DeliberationStore: Send + Sync + 'static {
    /// Stores a deliberation. With `cache`, it may answer the same question
    /// (`key`) until the given time.
    fn save(&self, deliberation: &Deliberation, cache: Option<(&str, DateTime<Utc>)>);

    /// The newest deliberation stored under `key` and still valid at `now`.
    fn cached(&self, key: &str, now: DateTime<Utc>) -> Option<Deliberation>;

    /// Newest first.
    fn recent(&self, limit: usize) -> Vec<Deliberation>;

    /// Forgets every cache entry; the deliberations stay in the history.
    fn clear_cache(&self);
}
