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

/// String fields at least this long and spanning lines are sent to models
/// as raw text instead of a JSON string (see [`render_for_model`]).
const RAW_FIELD_MIN_CHARS: usize = 200;

/// A tool's argument schema in fewer tokens, for every request that
/// carries it (ADR-0018), with the same meaning for a model: optional
/// fields lose their `null` alternative (`["string", "null"]` becomes
/// `"string"`, `anyOf [X, null]` becomes X), `"default": null`, integer
/// `format`s (`uint32`…) and the `title`/`$schema` annotations (generators
/// such as pydantic title every field) go, the rest stays.
pub fn slim_schema(mut schema: Value) -> Value {
    slim(&mut schema);
    schema
}

fn slim(value: &mut Value) {
    match value {
        Value::Object(map) => {
            if map.get("default").is_some_and(Value::is_null) {
                map.remove("default");
            }
            // Only as annotations (strings): a property named `title` is
            // an object and stays.
            for key in ["title", "$schema"] {
                if map.get(key).is_some_and(Value::is_string) {
                    map.remove(key);
                }
            }
            if map
                .get("format")
                .and_then(Value::as_str)
                .is_some_and(|f| f.starts_with("int") || f.starts_with("uint"))
            {
                map.remove("format");
            }
            if let Some(Value::Array(types)) = map.get_mut("type") {
                types.retain(|t| t != "null");
                if types.len() == 1 {
                    let only = types.remove(0);
                    map.insert("type".into(), only);
                }
            }
            for key in ["anyOf", "oneOf"] {
                let Some(Value::Array(options)) = map.get_mut(key) else {
                    continue;
                };
                options.retain(|o| o.get("type").and_then(Value::as_str) != Some("null"));
                if options.len() == 1 {
                    let Value::Object(only) = options.remove(0) else {
                        continue;
                    };
                    map.remove(key);
                    for (k, v) in only {
                        map.entry(k).or_insert(v);
                    }
                }
            }
            for child in map.values_mut() {
                slim(child);
            }
        }
        Value::Array(items) => items.iter_mut().for_each(slim),
        _ => {}
    }
}

/// Lists of at least this many rows of flat objects go as tables.
const TABLE_MIN_ROWS: usize = 3;

/// A tool's output as text for a model, in as few tokens as possible: a
/// string goes as is; an object's long multi-line strings (file contents,
/// command output) go as raw blocks, and its lists of flat objects
/// (directory entries, commits, search hits) as tables with one row per
/// item, after the compact JSON of the rest, so newlines and quotes are not
/// escaped and keys are not repeated; anything else is compact JSON.
pub fn render_for_model(output: &Value) -> String {
    match output {
        Value::String(s) => s.clone(),
        Value::Array(_) => match table(output) {
            Some(rows) => rows,
            None => compact(output),
        },
        Value::Object(map) => {
            let is_raw = |v: &Value| {
                v.as_str()
                    .is_some_and(|s| s.contains('\n') && s.chars().count() >= RAW_FIELD_MIN_CHARS)
            };
            let mut rest = serde_json::Map::new();
            let mut blocks = Vec::new();
            for (key, value) in map {
                match value.as_str() {
                    Some(text) if is_raw(value) => blocks.push(format!("[{key}]\n{text}")),
                    _ => match table(value) {
                        Some(rows) => blocks.push(format!("[{key}: {rows}")),
                        None => {
                            rest.insert(key.clone(), value.clone());
                        }
                    },
                }
            }
            if blocks.is_empty() {
                return compact(output);
            }
            let mut text = if rest.is_empty() {
                String::new()
            } else {
                compact(&Value::Object(rest))
            };
            for block in blocks {
                if !text.is_empty() {
                    text.push('\n');
                }
                text.push_str(&block);
            }
            text
        }
        other => compact(other),
    }
}

fn compact(value: &Value) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "null".into())
}

/// A list of flat objects as `col|col]` plus one `value|value` line per
/// item (`|`, `\` and newlines escaped; null and missing are empty), or
/// `None` when it is not one: too short, or with nested values.
fn table(value: &Value) -> Option<String> {
    let rows = value.as_array().filter(|a| a.len() >= TABLE_MIN_ROWS)?;
    let mut columns: Vec<&str> = Vec::new();
    for row in rows {
        let row = row.as_object().filter(|r| !r.is_empty())?;
        for (key, cell) in row {
            if cell.is_array() || cell.is_object() {
                return None;
            }
            if !columns.contains(&key.as_str()) {
                columns.push(key);
            }
        }
    }
    let cell = |value: Option<&Value>| match value {
        None | Some(Value::Null) => String::new(),
        Some(Value::String(s)) => s
            .replace('\\', "\\\\")
            .replace('|', "\\|")
            .replace('\n', "\\n"),
        Some(other) => other.to_string(),
    };
    let mut text = format!("{}]", columns.join("|"));
    for row in rows {
        text.push('\n');
        let line: Vec<String> = columns.iter().map(|c| cell(row.get(*c))).collect();
        text.push_str(&line.join("|"));
    }
    Some(text)
}

/// Cuts `text` to about `max` characters for a model, keeping the start
/// and the end (where errors and summaries of command output usually are).
pub fn truncate_for_model(text: String, max: usize) -> String {
    let total = text.chars().count();
    if total <= max {
        return text;
    }
    let head_len = max * 3 / 4;
    let tail_len = max - head_len;
    let head: String = text.chars().take(head_len).collect();
    let tail: String = text.chars().skip(total - tail_len).collect();
    format!(
        "{head}\n… [truncated: {} of {total} characters omitted; ask for less, e.g. maxBytes or a narrower path] …\n{tail}",
        total - max
    )
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
mod render_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn strings_and_long_text_fields_go_raw() {
        assert_eq!(render_for_model(&json!("a\n\"b\"")), "a\n\"b\"");
        assert_eq!(render_for_model(&json!({"a": 1})), "{\"a\":1}");
        let body = "line \"quoted\"\n".repeat(20);
        let text = render_for_model(&json!({"path": "x.rs", "content": body}));
        assert_eq!(text, format!("{{\"path\":\"x.rs\"}}\n[content]\n{body}"));
        assert!(!text.contains("\\n"));
    }

    #[test]
    fn schemas_lose_what_says_nothing() {
        let schema = json!({
            "type": "object",
            "properties": {
                "path": {"type": ["string", "null"], "default": null, "description": "d"},
                "limit": {"type": ["integer", "null"], "format": "uint32", "minimum": 0},
                "mode": {"anyOf": [{"type": "string", "enum": ["a", "b"]}, {"type": "null"}],
                         "description": "m"},
                "ratio": {"type": "number", "format": "double"},
                "title": {"type": "string", "title": "Title"}
            },
            "title": "Args",
            "$schema": "http://json-schema.org/draft-07/schema#"
        });
        assert_eq!(
            slim_schema(schema),
            json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "d"},
                    "limit": {"type": "integer", "minimum": 0},
                    "mode": {"type": "string", "enum": ["a", "b"], "description": "m"},
                    "ratio": {"type": "number", "format": "double"},
                    "title": {"type": "string"}
                }
            })
        );
    }

    #[test]
    fn lists_of_flat_objects_go_as_tables() {
        let entries = json!([
            {"name": "a.rs", "kind": "file", "size": 12},
            {"name": "src", "kind": "dir", "size": null},
            {"name": "x|y\nz", "kind": "file"},
        ]);
        let table = "kind|name|size]\nfile|a.rs|12\ndir|src|\nfile|x\\|y\\nz|";
        assert_eq!(render_for_model(&entries), table);
        let text = render_for_model(&json!({"path": ".", "entries": entries}));
        assert_eq!(text, format!("{{\"path\":\".\"}}\n[entries: {table}"));
        // Short or nested lists stay JSON.
        assert_eq!(render_for_model(&json!([{"a": 1}])), "[{\"a\":1}]");
        let nested = json!([{"a": [1]}, {"a": [2]}, {"a": [3]}]);
        assert_eq!(render_for_model(&nested), compact(&nested));
    }

    #[test]
    fn truncation_keeps_head_and_tail() {
        let text = format!("{}{}", "a".repeat(100), "z".repeat(100));
        let cut = truncate_for_model(text, 40);
        assert!(cut.starts_with(&"a".repeat(30)));
        assert!(cut.ends_with(&"z".repeat(10)));
        assert!(cut.contains("160 of 200"));
        assert_eq!(truncate_for_model("short".into(), 40), "short");
    }
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
