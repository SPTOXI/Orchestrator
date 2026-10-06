//! Tool calls: the contract between callers (UI, agents) and the Tool Runtime.
//!
//! AI providers never execute operations themselves. They produce a
//! [`ToolCall`]; the Orchestrator executes it through the Tool Runtime and
//! answers with a [`ToolResult`].

use crate::ids::{DeliberationId, ProviderId, SessionId, ToolCallId};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fmt;
use std::path::PathBuf;

/// Who asked for an operation. Recorded on every audit event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum CallOrigin {
    /// The human user, through the desktop UI.
    User,
    /// An AI agent. `agent_id` is the agent of the Agent Manager
    /// (ADR-0015) when one is driving the session, and the session id
    /// itself when the session is its own agent (ADR-0009).
    Agent {
        agent_id: String,
        /// Provider session that produced the call.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        session_id: Option<SessionId>,
        /// Provider behind the session.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider: Option<ProviderId>,
    },
    /// The Orchestrator itself (e.g. shutdown cleanup).
    System,
    /// The model Council acting on its own in Full mode (ADR-0011).
    Council {
        /// Deliberation whose decision was applied.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        deliberation_id: Option<DeliberationId>,
    },
}

impl CallOrigin {
    /// Origin of a call made by a provider session (Phase 3: the session is
    /// the agent).
    pub fn session(session_id: &SessionId, provider: &ProviderId) -> Self {
        Self::Agent {
            agent_id: session_id.to_string(),
            session_id: Some(session_id.clone()),
            provider: Some(provider.clone()),
        }
    }
}

/// A request to execute one tool of the runtime catalog, e.g. `filesystem.read`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolCall {
    pub id: ToolCallId,
    /// Fully qualified tool name: `<group>.<operation>`.
    pub tool: String,
    /// Tool arguments as a JSON object (`null` is treated as `{}`).
    #[serde(default)]
    pub args: Value,
    pub origin: CallOrigin,
    /// Where relative paths and commands land: the project of the session
    /// that made the call (ADR-0023). `None`: the runtime's own directory
    /// (the project open in the app).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<PathBuf>,
}

impl ToolCall {
    pub fn new(tool: impl Into<String>, args: Value, origin: CallOrigin) -> Self {
        Self {
            id: ToolCallId::new(),
            tool: tool.into(),
            args,
            origin,
            workspace: None,
        }
    }

    /// The same call, working in `workspace`.
    pub fn in_workspace(mut self, workspace: impl Into<PathBuf>) -> Self {
        self.workspace = Some(workspace.into());
        self
    }
}

/// Machine-readable error category of a failed tool call.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ToolErrorKind {
    /// The tool name is not in the catalog.
    UnknownTool,
    /// Arguments are missing, malformed or contain unknown fields.
    InvalidArgs,
    /// A file, directory, terminal or process does not exist.
    NotFound,
    /// The destination already exists.
    AlreadyExists,
    /// The operating system denied the operation.
    PermissionDenied,
    /// Other I/O failure.
    Io,
    /// A shell or process could not be started.
    Spawn,
    /// The target terminal/process is no longer running.
    NotRunning,
    /// An external command (e.g. `git`) ran and reported failure; the
    /// message carries its output.
    CommandFailed,
    /// Not executed: the agent turn that asked for it was cancelled
    /// (ADR-0009).
    Cancelled,
    /// Another agent holds the file this call would change (ADR-0015).
    /// Never returned to the user, who is not locked out of the project.
    Locked,
    /// The user, or one of the user's rules, refused the call (ADR-0016).
    /// The message always says who and why.
    Denied,
    /// Unexpected internal failure.
    Internal,
}

/// Error of a failed tool call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolError {
    pub kind: ToolErrorKind,
    pub message: String,
}

impl ToolError {
    pub fn new(kind: ToolErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    pub fn invalid_args(message: impl Into<String>) -> Self {
        Self::new(ToolErrorKind::InvalidArgs, message)
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new(ToolErrorKind::NotFound, message)
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(ToolErrorKind::Internal, message)
    }

    pub fn command_failed(message: impl Into<String>) -> Self {
        Self::new(ToolErrorKind::CommandFailed, message)
    }
}

impl fmt::Display for ToolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}: {}", self.kind, self.message)
    }
}

impl std::error::Error for ToolError {}

/// Outcome of a [`ToolCall`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolResult {
    pub call_id: ToolCallId,
    pub tool: String,
    pub ok: bool,
    /// Tool-specific output (`null` when `ok` is false).
    pub output: Value,
    pub error: Option<ToolError>,
    pub started_at: DateTime<Utc>,
    pub finished_at: DateTime<Utc>,
    pub duration_ms: u64,
}

impl ToolResult {
    /// Converts the result into a `Result`, for callers that prefer `?`.
    pub fn into_result(self) -> Result<Value, ToolError> {
        match self.error {
            Some(error) => Err(error),
            None if self.ok => Ok(self.output),
            None => Err(ToolError::internal("tool failed without error details")),
        }
    }
}

/// Catalog entry describing one tool.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolSpec {
    /// Fully qualified name, e.g. `terminal.create`.
    pub name: &'static str,
    /// Group, e.g. `terminal`.
    pub group: &'static str,
    pub description: &'static str,
    /// True when the tool only queries state (no files, processes or
    /// repository state change). Recorded on `TOOL_CALLED` (ADR-0008).
    pub read_only: bool,
}

/// A tool as offered to an AI model: catalog entry plus the JSON Schema of
/// its arguments (ADR-0010).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolDefinition {
    pub name: String,
    pub group: String,
    pub description: String,
    pub read_only: bool,
    /// JSON Schema (draft 7, no `$ref`) of the `args` object.
    pub parameters: Value,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn origin_serializes_with_type_tag() {
        assert_eq!(
            serde_json::to_value(CallOrigin::User).unwrap(),
            json!({"type": "user"})
        );
        assert_eq!(
            serde_json::to_value(CallOrigin::Agent {
                agent_id: "a1".into(),
                session_id: None,
                provider: None,
            })
            .unwrap(),
            json!({"type": "agent", "agentId": "a1"})
        );
        assert_eq!(
            serde_json::to_value(CallOrigin::session(
                &SessionId::from("s1"),
                &ProviderId::from("echo")
            ))
            .unwrap(),
            json!({"type": "agent", "agentId": "s1", "sessionId": "s1", "provider": "echo"})
        );
        assert_eq!(
            serde_json::to_value(CallOrigin::Council {
                deliberation_id: Some(DeliberationId::from("d1")),
            })
            .unwrap(),
            json!({"type": "council", "deliberationId": "d1"})
        );
        let origin: CallOrigin = serde_json::from_value(json!({"type": "council"})).unwrap();
        assert_eq!(
            origin,
            CallOrigin::Council {
                deliberation_id: None
            }
        );
    }

    #[test]
    fn agent_origin_without_session_still_parses() {
        let origin: CallOrigin =
            serde_json::from_value(json!({"type": "agent", "agentId": "a1"})).unwrap();
        assert_eq!(
            origin,
            CallOrigin::Agent {
                agent_id: "a1".into(),
                session_id: None,
                provider: None
            }
        );
    }

    #[test]
    fn tool_call_accepts_missing_args() {
        let call: ToolCall = serde_json::from_value(json!({
            "id": "c1",
            "tool": "process.list",
            "origin": {"type": "system"}
        }))
        .unwrap();
        assert_eq!(call.args, Value::Null);
        assert_eq!(call.origin, CallOrigin::System);
    }

    #[test]
    fn error_kind_uses_screaming_snake_case() {
        let err = ToolError::invalid_args("missing path");
        assert_eq!(
            serde_json::to_value(&err).unwrap(),
            json!({"kind": "INVALID_ARGS", "message": "missing path"})
        );
    }

    #[test]
    fn into_result_maps_errors() {
        let now = Utc::now();
        let failed = ToolResult {
            call_id: ToolCallId::new(),
            tool: "x.y".into(),
            ok: false,
            output: Value::Null,
            error: Some(ToolError::not_found("nope")),
            started_at: now,
            finished_at: now,
            duration_ms: 0,
        };
        assert_eq!(
            failed.into_result().unwrap_err().kind,
            ToolErrorKind::NotFound
        );
    }
}
