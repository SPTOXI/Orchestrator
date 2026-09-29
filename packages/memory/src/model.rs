//! Public types of the store (camelCase JSON for the UI).

use chrono::{DateTime, Utc};
use orchestrator_core::{CallOrigin, EventKind, SessionInfo, SessionStatus};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A project the Orchestrator has opened.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Project {
    pub id: String,
    pub path: String,
    pub name: String,
    pub created_at: DateTime<Utc>,
    pub last_opened_at: DateTime<Utc>,
    /// What the detector found the last time the project was opened.
    pub stack: Option<Value>,
}

/// A recent project coming from the UI's old list (`localStorage`).
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecentImport {
    pub path: String,
    pub name: String,
    pub opened_at: DateTime<Utc>,
}

/// Filters of `history`. Pages go backwards in time.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct HistoryQuery {
    /// Only events of this project.
    pub project_id: Option<String>,
    /// Only these kinds (empty = all).
    pub kinds: Vec<EventKind>,
    /// Case-insensitive text in the summary.
    pub text: Option<String>,
    /// Hide successful read-only tool calls.
    pub hide_reads: bool,
    /// Cursor: events strictly older than this one (`HistoryPage::next`).
    pub before: Option<String>,
    /// Page size (default 200, at most 1000).
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryPage {
    /// Oldest first.
    pub events: Vec<orchestrator_core::AuditEvent>,
    /// Cursor for the previous (older) page; `None` at the beginning.
    pub next: Option<String>,
}

/// Kind of a project memory entry (L2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MemoryKind {
    Architecture,
    Stack,
    Convention,
    Rule,
    Note,
}

impl MemoryKind {
    pub fn id(self) -> &'static str {
        match self {
            MemoryKind::Architecture => "architecture",
            MemoryKind::Stack => "stack",
            MemoryKind::Convention => "convention",
            MemoryKind::Rule => "rule",
            MemoryKind::Note => "note",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        [
            MemoryKind::Architecture,
            MemoryKind::Stack,
            MemoryKind::Convention,
            MemoryKind::Rule,
            MemoryKind::Note,
        ]
        .into_iter()
        .find(|k| k.id() == text)
    }
}

/// Who wrote a memory entry or decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Source {
    User,
    Agent,
    Detector,
}

impl Source {
    pub fn id(self) -> &'static str {
        match self {
            Source::User => "user",
            Source::Agent => "agent",
            Source::Detector => "detector",
        }
    }

    pub fn parse(text: &str) -> Self {
        match text {
            "agent" => Source::Agent,
            "detector" => Source::Detector,
            _ => Source::User,
        }
    }

    /// The source of a write made by `origin`.
    pub fn of(origin: &CallOrigin) -> Self {
        match origin {
            CallOrigin::User => Source::User,
            CallOrigin::Agent { .. } | CallOrigin::Council { .. } => Source::Agent,
            CallOrigin::System => Source::Detector,
        }
    }
}

/// A project memory entry (L2).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryEntry {
    pub id: String,
    pub project_id: String,
    pub kind: MemoryKind,
    pub title: String,
    pub content: String,
    pub tags: Vec<String>,
    pub pinned: bool,
    pub source: Source,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Creates (`id: None`) or updates an entry.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryInput {
    #[serde(default)]
    pub id: Option<String>,
    pub project_id: String,
    pub kind: MemoryKind,
    pub title: String,
    pub content: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub pinned: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DecisionStatus {
    Proposed,
    Accepted,
    Superseded,
    Rejected,
}

impl DecisionStatus {
    pub fn id(self) -> &'static str {
        match self {
            DecisionStatus::Proposed => "proposed",
            DecisionStatus::Accepted => "accepted",
            DecisionStatus::Superseded => "superseded",
            DecisionStatus::Rejected => "rejected",
        }
    }

    pub fn parse(text: &str) -> Self {
        match text {
            "accepted" => DecisionStatus::Accepted,
            "superseded" => DecisionStatus::Superseded,
            "rejected" => DecisionStatus::Rejected,
            _ => DecisionStatus::Proposed,
        }
    }
}

/// A project decision. Never deleted: it changes status.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Decision {
    pub id: String,
    pub project_id: String,
    pub title: String,
    pub context: String,
    pub decision: String,
    pub consequences: String,
    pub status: DecisionStatus,
    pub source: Source,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DecisionInput {
    #[serde(default)]
    pub id: Option<String>,
    pub project_id: String,
    pub title: String,
    #[serde(default)]
    pub context: String,
    pub decision: String,
    #[serde(default)]
    pub consequences: String,
    pub status: DecisionStatus,
}

/// A session as stored: the info the UI lists, the provider's native
/// session (with the state it needs to resume) and what it was opened for.
#[derive(Debug, Clone, PartialEq)]
pub struct StoredSession {
    pub info: SessionInfo,
    /// JSON `NativeSession`.
    pub native: Value,
    /// JSON: instructions and the model asked for.
    pub spec: Value,
}

/// L1 working memory: what is happening in the project now, derived from
/// the history.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkingMemory {
    pub sessions: Vec<WorkingSession>,
    pub files: Vec<WorkingFile>,
    pub commands: Vec<WorkingCommand>,
    pub errors: Vec<WorkingError>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkingSession {
    pub id: String,
    pub title: String,
    pub provider: String,
    pub model: Option<String>,
    pub status: SessionStatus,
    pub turns: u32,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkingFile {
    pub path: String,
    /// `created`, `modified`, `deleted`, `moved`…
    pub change: String,
    pub at: DateTime<Utc>,
    /// `user`, `agent`, `council` or `system`.
    pub by: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkingCommand {
    pub command: String,
    pub exit_code: Option<i64>,
    pub background: bool,
    pub at: DateTime<Utc>,
    pub by: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkingError {
    pub kind: EventKind,
    pub summary: String,
    pub detail: Option<String>,
    pub at: DateTime<Utc>,
}

/// Overview of a project's memory for the MEMORY panel.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryOverview {
    pub project: Project,
    pub working: WorkingMemory,
    pub memory_entries: usize,
    pub decisions: usize,
    pub sessions: usize,
    pub events: usize,
}

/// A search result (L3).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchHit {
    /// `memory`, `decision`, `message` or `event`.
    pub kind: String,
    /// Entry, decision, session or event id.
    pub ref_id: String,
    pub title: String,
    /// Matching text with the terms between `[` and `]`.
    pub snippet: String,
    pub at: DateTime<Utc>,
}
