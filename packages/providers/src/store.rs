//! Where the `SessionManager` keeps sessions between runs (ADR-0012). The
//! app stores them in its database; tests use [`MemorySessionStore`].

use crate::failover::Reserve;
use crate::project_context::ContextOptions;
use crate::provider::NativeSession;
use orchestrator_core::{SessionId, SessionInfo, SessionLogEntry};
use parking_lot::Mutex;
use std::collections::BTreeMap;

/// A session as stored.
#[derive(Debug, Clone, PartialEq)]
pub struct PersistedSession {
    pub info: SessionInfo,
    /// With the provider's state from `AIProvider::snapshot`.
    pub native: NativeSession,
    /// What the session was opened with.
    pub instructions: Option<String>,
    pub requested_model: Option<String>,
    /// Project context options (ADR-0013).
    pub context: ContextOptions,
    /// Who takes over when the session's AI fails a turn (ADR-0024).
    pub reserves: Vec<Reserve>,
}

/// Persistence of provider sessions. Implementations report their own
/// errors: a failed write never fails a turn.
pub trait SessionStore: Send + Sync + 'static {
    /// Stored sessions, oldest first, with their transcripts.
    fn load(&self) -> Vec<(PersistedSession, Vec<SessionLogEntry>)>;

    /// Inserts or updates a session.
    fn save(&self, session: &PersistedSession);

    /// Adds transcript entries (an entry with a known `seq` is replaced).
    fn append(&self, id: &SessionId, entries: &[SessionLogEntry]);
}

/// A stored session and its transcript by `seq`.
type Stored = (PersistedSession, BTreeMap<u64, SessionLogEntry>);

/// Keeps sessions in memory (tests, or a store that failed to open).
#[derive(Default)]
pub struct MemorySessionStore {
    sessions: Mutex<BTreeMap<String, Stored>>,
}

impl MemorySessionStore {
    pub fn new() -> Self {
        Self::default()
    }
}

impl SessionStore for MemorySessionStore {
    fn load(&self) -> Vec<(PersistedSession, Vec<SessionLogEntry>)> {
        let mut out: Vec<_> = self
            .sessions
            .lock()
            .values()
            .map(|(session, entries)| (session.clone(), entries.values().cloned().collect()))
            .collect();
        out.sort_by(|a, b| {
            a.0.info
                .created_at
                .cmp(&b.0.info.created_at)
                .then_with(|| a.0.info.id.cmp(&b.0.info.id))
        });
        out
    }

    fn save(&self, session: &PersistedSession) {
        let mut sessions = self.sessions.lock();
        let slot = sessions
            .entry(session.info.id.to_string())
            .or_insert_with(|| (session.clone(), BTreeMap::new()));
        slot.0 = session.clone();
    }

    fn append(&self, id: &SessionId, entries: &[SessionLogEntry]) {
        if let Some((_, stored)) = self.sessions.lock().get_mut(id.as_str()) {
            for entry in entries {
                stored.insert(entry.seq, entry.clone());
            }
        }
    }
}
