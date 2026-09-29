//! Provider sessions: what the provider layer, the UI and the history share
//! about a conversation with an AI provider (ADR-0009).
//!
//! A session belongs to the Orchestrator, not to the provider: the provider
//! only holds a native session (e.g. an API conversation) referenced by
//! [`SessionInfo::native_ref`].

use crate::context::ContextSummary;
use crate::ids::{HandoffId, ProviderId, SessionId, TurnId};
use crate::tool::{ToolCall, ToolResult};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::ops::AddAssign;
use std::path::PathBuf;

/// Tokens (and cost) consumed by a turn or a session (section 23).
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TokenUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    /// Input tokens served from the provider's prompt cache.
    #[serde(default)]
    pub cached_input_tokens: u64,
    #[serde(default)]
    pub reasoning_tokens: u64,
    /// Cost in USD, when the provider reports it.
    #[serde(default)]
    pub cost_usd: Option<f64>,
    /// True when any part was estimated instead of reported by the vendor.
    #[serde(default)]
    pub estimated: bool,
}

impl TokenUsage {
    pub fn total_tokens(&self) -> u64 {
        self.input_tokens + self.output_tokens
    }

    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

impl AddAssign for TokenUsage {
    fn add_assign(&mut self, other: Self) {
        self.input_tokens += other.input_tokens;
        self.output_tokens += other.output_tokens;
        self.cached_input_tokens += other.cached_input_tokens;
        self.reasoning_tokens += other.reasoning_tokens;
        self.cost_usd = match (self.cost_usd, other.cost_usd) {
            (None, None) => None,
            (a, b) => Some(a.unwrap_or(0.0) + b.unwrap_or(0.0)),
        };
        self.estimated |= other.estimated;
    }
}

/// Lifecycle of a session. One turn runs at a time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SessionStatus {
    /// Ready for the next turn.
    Idle,
    /// A turn is in progress.
    Running,
    /// Closed; `resume` reopens it.
    Closed,
}

/// How a turn ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TurnStatus {
    Completed,
    Cancelled,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum NoticeLevel {
    Info,
    Warning,
    Error,
}

/// Snapshot of a session, as listed by the UI and recorded in history.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionInfo {
    pub id: SessionId,
    pub provider: ProviderId,
    pub title: String,
    pub model: Option<String>,
    /// Project (workspace) the session works on.
    pub project_path: PathBuf,
    /// Session that spawned this one (subagent), if any.
    pub parent_id: Option<SessionId>,
    pub status: SessionStatus,
    /// Provider-native session reference (e.g. a thread id), for resume.
    pub native_ref: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    /// Finished turns (any status).
    pub turns: u32,
    /// Sum of every turn's usage.
    pub usage: TokenUsage,
    /// Error of the last failed turn, cleared by the next successful one.
    pub last_error: Option<String>,
}

/// Something that happened in a session. Streamed live
/// (`StreamEvent::Session`) and kept in the session transcript.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum SessionEvent {
    TurnStarted {
        turn_id: TurnId,
        input: String,
    },
    /// Assistant output.
    TextDelta {
        turn_id: TurnId,
        text: String,
    },
    /// Reasoning / thinking output, when the provider exposes it.
    ReasoningDelta {
        turn_id: TurnId,
        text: String,
    },
    /// The provider asked the Orchestrator to execute a tool.
    ToolCallRequested {
        turn_id: TurnId,
        call: ToolCall,
    },
    /// The Orchestrator executed the tool (always recorded, even when the
    /// turn was cancelled meanwhile).
    ToolCallCompleted {
        turn_id: TurnId,
        result: ToolResult,
    },
    /// Usage reported during a turn (incremental).
    Usage {
        turn_id: TurnId,
        usage: TokenUsage,
    },
    Notice {
        turn_id: Option<TurnId>,
        level: NoticeLevel,
        message: String,
    },
    TurnCompleted {
        turn_id: TurnId,
        status: TurnStatus,
        error: Option<String>,
        /// Usage of this turn.
        usage: TokenUsage,
        duration_ms: u64,
        tool_calls: u32,
    },
    StatusChanged {
        status: SessionStatus,
    },
    SubagentSpawned {
        child_id: SessionId,
        provider: ProviderId,
        title: String,
    },
    /// Project context was attached to this turn (ADR-0013); the text went
    /// to the provider, the transcript keeps the summary.
    ContextAttached {
        turn_id: TurnId,
        summary: ContextSummary,
    },
    /// The work passed from one session to another through a handoff
    /// (ADR-0013). Recorded in both sessions.
    HandedOff {
        handoff_id: HandoffId,
        from_session: SessionId,
        to_session: SessionId,
        /// Provider of the session that took over.
        provider: ProviderId,
    },
}

impl SessionEvent {
    /// Turn the event belongs to, if any.
    pub fn turn_id(&self) -> Option<&TurnId> {
        match self {
            Self::TurnStarted { turn_id, .. }
            | Self::TextDelta { turn_id, .. }
            | Self::ReasoningDelta { turn_id, .. }
            | Self::ToolCallRequested { turn_id, .. }
            | Self::ToolCallCompleted { turn_id, .. }
            | Self::Usage { turn_id, .. }
            | Self::TurnCompleted { turn_id, .. }
            | Self::ContextAttached { turn_id, .. } => Some(turn_id),
            Self::Notice { turn_id, .. } => turn_id.as_ref(),
            Self::StatusChanged { .. } | Self::SubagentSpawned { .. } | Self::HandedOff { .. } => {
                None
            }
        }
    }
}

/// One numbered entry of a session transcript. `seq` is the same number
/// carried by the live `StreamEvent::Session`, so a reader can merge a
/// snapshot with live events without gaps or duplicates.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionLogEntry {
    pub seq: u64,
    pub at: DateTime<Utc>,
    pub event: SessionEvent,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn usage_adds_up_and_keeps_cost_optional() {
        let mut total = TokenUsage::default();
        total += TokenUsage {
            input_tokens: 10,
            output_tokens: 5,
            ..Default::default()
        };
        assert_eq!(total.cost_usd, None);
        total += TokenUsage {
            input_tokens: 1,
            output_tokens: 2,
            cost_usd: Some(0.5),
            estimated: true,
            ..Default::default()
        };
        assert_eq!(total.total_tokens(), 18);
        assert_eq!(total.cost_usd, Some(0.5));
        assert!(total.estimated);
    }

    #[test]
    fn session_event_is_tagged_and_camel_case() {
        let event = SessionEvent::TextDelta {
            turn_id: TurnId::from("t1"),
            text: "oi".into(),
        };
        assert_eq!(
            serde_json::to_value(&event).unwrap(),
            json!({"type": "textDelta", "turnId": "t1", "text": "oi"})
        );
        assert_eq!(event.turn_id(), Some(&TurnId::from("t1")));
        let status = SessionEvent::StatusChanged {
            status: SessionStatus::Running,
        };
        assert_eq!(
            serde_json::to_value(&status).unwrap(),
            json!({"type": "statusChanged", "status": "running"})
        );
        assert_eq!(status.turn_id(), None);
    }
}
