//! Events published by the runtime.
//!
//! - [`AuditEvent`]: durable history. Recorded for every operation, in every
//!   autonomy mode (observability is not a restriction).
//! - [`StreamEvent`]: high-frequency live output; not durable by itself (the
//!   runtime keeps bounded buffers that can be read back).

use crate::ids::{EventId, ProcessId, SessionId, TerminalId, ToolCallId};
use crate::session::SessionEvent;
use crate::tool::CallOrigin;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::Mutex;

/// Kind of a durable history event. Independent of any AI provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum EventKind {
    ProjectCreated,
    ProjectOpened,
    TaskCreated,
    TaskStarted,
    TaskCompleted,
    AgentStarted,
    AgentFinished,
    ProviderSwitched,
    ToolCalled,
    FileChanged,
    CommandExecuted,
    GitCommit,
    GitPush,
    HandoffCreated,
    HandoffAccepted,
    /// A managed process ended (ADR-0005).
    ProcessExited,
    /// The shell of a terminal ended (ADR-0005).
    TerminalExited,
    /// A provider session was opened (ADR-0009).
    SessionStarted,
    /// A closed provider session was reopened (ADR-0009).
    SessionResumed,
    /// A provider session was closed (ADR-0009).
    SessionClosed,
    /// A session turn ended: status, duration, tool calls and token usage
    /// (ADR-0009).
    TurnCompleted,
    /// An API connection was added or changed (never includes the key;
    /// ADR-0010).
    ConnectionSaved,
    /// An API connection was removed (ADR-0010).
    ConnectionRemoved,
    /// The Council settings (mode, members) were saved (ADR-0011).
    CouncilConfigured,
    /// The Council deliberated which model fits a task: candidates, votes,
    /// decision, usage and cost; also recorded for cache hits (ADR-0011).
    CouncilDeliberated,
    /// A model was applied to a new session, by the user or by the Council
    /// in Full mode (ADR-0011).
    RouteDecided,
    /// A project memory entry (L2) was created or changed (ADR-0012).
    MemorySaved,
    /// A project memory entry (L2) was deleted (ADR-0012).
    MemoryRemoved,
    /// A project decision was recorded or changed status (ADR-0012).
    DecisionSaved,
    /// Project context was attached to a session: sections, estimated
    /// tokens and what was left out, never the text (ADR-0013).
    ContextBuilt,
    /// A task changed in any way the three task events above do not cover:
    /// edited, re-prioritized, blocked, sent to review, cancelled or
    /// reopened (ADR-0014).
    TaskUpdated,
    /// The autonomy mode of a project, the default mode or the user's
    /// rules changed (ADR-0016).
    AutonomyChanged,
    /// An AI asked for something that needs the user's authorization
    /// (ADR-0016).
    ApprovalRequested,
    /// A request was approved, approved for the session, denied or
    /// cancelled (ADR-0016).
    ApprovalDecided,
    /// The AIs, or one agent, were paused (ADR-0016).
    ExecutionPaused,
    /// The AIs, or one agent, were resumed (ADR-0016).
    ExecutionResumed,
    /// A pull request was opened on GitHub (ADR-0017).
    GithubPrCreated,
    /// A pull request was merged on GitHub (ADR-0017).
    GithubPrMerged,
    /// An issue was opened on GitHub (ADR-0017).
    GithubIssueCreated,
    /// A session's conversation was compacted into a summary (ADR-0018).
    ContextCompacted,
    /// The app opened in a new version (ADR-0019).
    AppUpdated,
    /// Two projects were linked, or unlinked, as working together
    /// (ADR-0023).
    ProjectLinked,
    /// The AI of one project asked the AI of a related one, and got its
    /// answer (ADR-0023).
    ProjectAsked,
    /// A session's AI failed a turn and one of its reserves took over
    /// (ADR-0024).
    SessionFailover,
    /// The local engine (llama.cpp) was installed or updated (ADR-0025).
    LocalEngineInstalled,
    /// The local engine was removed (ADR-0025).
    LocalEngineRemoved,
    /// A local model was downloaded, imported or added (ADR-0025).
    LocalModelAdded,
    /// A local model was removed (ADR-0025).
    LocalModelRemoved,
    /// The local engine loaded a model to serve a session (ADR-0025).
    LocalModelLoaded,
}

/// A durable, provider-independent history entry.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuditEvent {
    pub id: EventId,
    pub at: DateTime<Utc>,
    pub kind: EventKind,
    pub origin: CallOrigin,
    /// Tool call that produced this event, if any.
    pub call_id: Option<ToolCallId>,
    /// One-line human readable description.
    pub summary: String,
    /// Structured details (kind specific).
    pub data: Value,
}

impl AuditEvent {
    pub fn new(
        kind: EventKind,
        origin: CallOrigin,
        summary: impl Into<String>,
        data: Value,
    ) -> Self {
        Self {
            id: EventId::new(),
            at: Utc::now(),
            kind,
            origin,
            call_id: None,
            summary: summary.into(),
            data,
        }
    }

    pub fn with_call(mut self, call_id: ToolCallId) -> Self {
        self.call_id = Some(call_id);
        self
    }
}

/// Output stream of a managed process.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum OutputStream {
    Stdout,
    Stderr,
}

/// Live output and lifecycle notifications of terminals, processes and
/// provider sessions.
///
/// `offset` is the byte offset of `data` in the whole output stream, the same
/// coordinate used by `terminal.read` / `process.read` (`since`/`next`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum StreamEvent {
    TerminalOutput {
        terminal_id: TerminalId,
        offset: u64,
        data: String,
    },
    TerminalExited {
        terminal_id: TerminalId,
        exit_code: Option<i32>,
    },
    ProcessOutput {
        process_id: ProcessId,
        stream: OutputStream,
        offset: u64,
        data: String,
    },
    ProcessExited {
        process_id: ProcessId,
        exit_code: Option<i32>,
        /// True when the process ended because `process.stop` was called.
        stopped: bool,
    },
    /// Something happened in a provider session (ADR-0009). `seq` numbers
    /// the session transcript (`session_get`).
    Session {
        session_id: SessionId,
        seq: u64,
        event: SessionEvent,
    },
}

/// Destination of runtime events. Implemented by the desktop app today and by
/// the SQLite history store in Phase 6.
pub trait EventSink: Send + Sync + 'static {
    fn audit(&self, event: AuditEvent);
    fn stream(&self, event: StreamEvent);
}

/// Discards every event.
#[derive(Debug, Default, Clone, Copy)]
pub struct NullSink;

impl EventSink for NullSink {
    fn audit(&self, _event: AuditEvent) {}
    fn stream(&self, _event: StreamEvent) {}
}

/// Keeps every event in memory. Useful for tests and diagnostics.
#[derive(Debug, Default)]
pub struct MemorySink {
    audit: Mutex<Vec<AuditEvent>>,
    stream: Mutex<Vec<StreamEvent>>,
}

impl MemorySink {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn audit_events(&self) -> Vec<AuditEvent> {
        self.audit.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    pub fn stream_events(&self) -> Vec<StreamEvent> {
        self.stream
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }
}

impl EventSink for MemorySink {
    fn audit(&self, event: AuditEvent) {
        self.audit
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(event);
    }

    fn stream(&self, event: StreamEvent) {
        self.stream
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(event);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn event_kind_matches_history_names() {
        assert_eq!(
            serde_json::to_value(EventKind::ToolCalled).unwrap(),
            json!("TOOL_CALLED")
        );
        assert_eq!(
            serde_json::to_value(EventKind::HandoffAccepted).unwrap(),
            json!("HANDOFF_ACCEPTED")
        );
        assert_eq!(
            serde_json::to_value(EventKind::ApprovalDecided).unwrap(),
            json!("APPROVAL_DECIDED")
        );
        assert_eq!(
            serde_json::to_value(EventKind::ExecutionPaused).unwrap(),
            json!("EXECUTION_PAUSED")
        );
    }

    #[test]
    fn stream_event_is_tagged_and_camel_case() {
        let event = StreamEvent::ProcessOutput {
            process_id: ProcessId::from("p1"),
            stream: OutputStream::Stderr,
            offset: 7,
            data: "boom".into(),
        };
        assert_eq!(
            serde_json::to_value(&event).unwrap(),
            json!({
                "type": "processOutput",
                "processId": "p1",
                "stream": "stderr",
                "offset": 7,
                "data": "boom"
            })
        );
    }

    #[test]
    fn session_stream_event_nests_the_session_event() {
        let event = StreamEvent::Session {
            session_id: SessionId::from("s1"),
            seq: 3,
            event: SessionEvent::StatusChanged {
                status: crate::session::SessionStatus::Idle,
            },
        };
        assert_eq!(
            serde_json::to_value(&event).unwrap(),
            json!({
                "type": "session",
                "sessionId": "s1",
                "seq": 3,
                "event": {"type": "statusChanged", "status": "idle"}
            })
        );
        assert_eq!(
            serde_json::to_value(EventKind::TurnCompleted).unwrap(),
            json!("TURN_COMPLETED")
        );
    }

    #[test]
    fn memory_sink_records_events() {
        let sink = MemorySink::new();
        sink.audit(AuditEvent::new(
            EventKind::ToolCalled,
            CallOrigin::User,
            "x",
            Value::Null,
        ));
        sink.stream(StreamEvent::TerminalExited {
            terminal_id: TerminalId::from("t"),
            exit_code: Some(0),
        });
        assert_eq!(sink.audit_events().len(), 1);
        assert_eq!(sink.stream_events().len(), 1);
    }
}
