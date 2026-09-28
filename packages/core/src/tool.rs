//! Tool calls: the contract between callers (UI, agents) and the Tool Runtime.
//!
//! AI providers never execute operations themselves. They produce a
//! [`ToolCall`]; the Orchestrator executes it through the Tool Runtime and
//! answers with a [`ToolResult`].

use crate::ids::ToolCallId;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fmt;

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
    /// An AI agent (Phase 8+).
    Agent { agent_id: String },
    /// The Orchestrator itself (e.g. shutdown cleanup).
    System,
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
}

impl ToolCall {
    pub fn new(tool: impl Into<String>, args: Value, origin: CallOrigin) -> Self {
        Self {
            id: ToolCallId::new(),
            tool: tool.into(),
            args,
            origin,
        }
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
                agent_id: "a1".into()
            })
            .unwrap(),
            json!({"type": "agent", "agentId": "a1"})
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
