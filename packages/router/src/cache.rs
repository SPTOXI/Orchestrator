//! Deliberations reused while nothing relevant changed, so the Council does
//! not spend tokens twice on the same question (ADR-0011).

use crate::council::Deliberation;
use parking_lot::Mutex;
use std::collections::HashMap;
use std::time::{Duration, Instant};

pub const CAPACITY: usize = 100;

pub struct DeliberationCache {
    entries: Mutex<HashMap<u64, (Instant, Deliberation)>>,
    capacity: usize,
}

impl DeliberationCache {
    pub fn new(capacity: usize) -> Self {
        Self {
            entries: Mutex::new(HashMap::new()),
            capacity: capacity.max(1),
        }
    }

    /// The deliberation stored under `key` if younger than `ttl`.
    pub fn get(&self, key: u64, ttl: Duration) -> Option<Deliberation> {
        let mut entries = self.entries.lock();
        match entries.get(&key) {
            Some((at, deliberation)) if at.elapsed() < ttl => Some(deliberation.clone()),
            Some(_) => {
                entries.remove(&key);
                None
            }
            None => None,
        }
    }

    pub fn put(&self, key: u64, deliberation: Deliberation) {
        let mut entries = self.entries.lock();
        entries.insert(key, (Instant::now(), deliberation));
        while entries.len() > self.capacity {
            let oldest = entries
                .iter()
                .min_by_key(|(_, (at, _))| *at)
                .map(|(key, _)| *key);
            match oldest {
                Some(key) => entries.remove(&key),
                None => break,
            };
        }
    }

    pub fn clear(&self) {
        self.entries.lock().clear();
    }
}

impl Default for DeliberationCache {
    fn default() -> Self {
        Self::new(CAPACITY)
    }
}
