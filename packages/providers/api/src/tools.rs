//! Tools as seen by models: names, results, the prompt protocol for APIs
//! without function calling, and schema dialects (ADR-0010).

use crate::conversation::{ToolCallPart, ToolResultPart};
use orchestrator_core::{ToolDefinition, ToolResult};
use serde_json::{json, Map, Value};

/// Tool results longer than this are truncated before going back to the
/// model (the full result stays in the transcript and the history).
pub const MAX_RESULT_CHARS: usize = 40_000;

/// Orchestrator name → API name (`filesystem.read` → `filesystem__read`):
/// most function-calling APIs reject dots.
pub fn api_name(name: &str) -> String {
    name.replace('.', "__")
}

/// API name → Orchestrator name.
pub fn orchestrator_name(name: &str) -> String {
    name.replace("__", ".")
}

/// Text sent back to the model for a tool result.
pub fn result_content(result: &ToolResult) -> (String, bool) {
    match (&result.error, result.ok) {
        (None, true) => {
            let text = serde_json::to_string(&result.output).unwrap_or_else(|_| "null".into());
            (truncate_result(text), false)
        }
        (Some(error), _) => {
            let kind = serde_json::to_value(error.kind)
                .ok()
                .and_then(|v| v.as_str().map(str::to_owned))
                .unwrap_or_else(|| format!("{:?}", error.kind));
            (format!("{kind}: {}", error.message), true)
        }
        (None, false) => ("INTERNAL: tool failed without details".into(), true),
    }
}

fn truncate_result(text: String) -> String {
    if text.chars().count() <= MAX_RESULT_CHARS {
        return text;
    }
    let cut: String = text.chars().take(MAX_RESULT_CHARS).collect();
    format!("{cut}\n… [truncated: result longer than {MAX_RESULT_CHARS} characters; ask for less, e.g. maxBytes or a narrower path]")
}

/// Result for a call that was not executed (invalid or truncated arguments).
pub fn rejected(call: &ToolCallPart, reason: &str) -> ToolResultPart {
    ToolResultPart {
        id: call.id.clone(),
        name: call.name.clone(),
        content: format!("INVALID_ARGS: {reason}"),
        is_error: true,
        native_id: call.native_id,
    }
}

/// Tool used by "Testar conexão"; never reaches the Tool Runtime.
pub fn ping_tool() -> ToolDefinition {
    ToolDefinition {
        name: "orchestrator.ping".into(),
        group: "orchestrator".into(),
        description: "Connection test. Call it with the value you were asked to send.".into(),
        read_only: true,
        parameters: json!({
            "type": "object",
            "properties": {"value": {"type": "string", "description": "Value to echo."}},
            "required": ["value"],
            "additionalProperties": false
        }),
    }
}

// ---------------------------------------------------------------- prompt ---

pub const CALL_OPEN: &str = "<tool_call>";
pub const CALL_CLOSE: &str = "</tool_call>";

/// System prompt section that tells the model what it can do through the
/// tools it has, so it does not answer with a generic "I cannot access
/// that" when a tool does exactly that, and says what is missing when
/// one does not. Built from the tool names only: stable for a session.
pub fn capabilities_note(tools: &[ToolDefinition]) -> Option<String> {
    let has = |prefixes: &[&str]| {
        tools
            .iter()
            .any(|t| prefixes.iter().any(|p| t.name.starts_with(p)))
    };
    let mut can = Vec::new();
    if has(&["filesystem."]) {
        can.push("read, write, move and delete the project's files");
    }
    if has(&["shell.", "terminal.", "process.", "package.", "runtime."]) {
        can.push("run commands and programs on the user's machine (shell, terminals, processes, packages)");
    }
    if has(&["git."]) {
        can.push("use git in the project (status, diff, commit, branches, pull, push)");
    }
    if has(&["github."]) {
        can.push("work on GitHub with the account the user connected in the Orchestrator (pull requests, CI checks, reviews, issues, merge)");
    }
    if has(&["memory.", "decision."]) {
        can.push("read and save the project's memory and decisions");
    }
    if has(&["agent."]) {
        can.push("delegate parts of a task to sub-agents");
    }
    if can.is_empty() {
        return None;
    }
    let mut note = format!(
        "What you can do here: you act only through your tools, which the Orchestrator runs on the user's \
         machine. With them you can {}. Use them instead of saying you cannot: never claim you have no \
         access to something a tool reaches, and never ask the user to do by hand what a tool does.",
        can.join("; ")
    );
    if has(&["github."]) {
        note.push_str(
            " The github tools use the user's GitHub account through a token kept by the Orchestrator \
             (you never see it). If a call says GitHub is not connected, tell the user to connect it in the \
             GIT panel, GitHub section, \"Conectar ao GitHub\".",
        );
    }
    note.push_str(
        " You have no web browser and cannot sign in to websites or use the user's passwords. When a request \
         needs an access you do not have, say exactly what is missing and how the user can provide it, \
         instead of a generic refusal.",
    );
    Some(note)
}

/// System prompt section that teaches the tool protocol.
pub fn prompt_instructions(tools: &[ToolDefinition]) -> String {
    let mut text = String::from(
        "# Tools\n\
         You can ask the Orchestrator to run tools in the user's project. To call a tool, write a block exactly like this:\n\
         <tool_call>{\"tool\": \"filesystem.read\", \"args\": {\"path\": \"README.md\"}}</tool_call>\n\
         The content of a block must be a single JSON object with \"tool\" and \"args\". You may write several blocks in one reply. \
         After your reply, the Orchestrator runs them in order and answers with <tool_result> blocks; then continue the task. \
         When you are finished, reply without any <tool_call> block. Never invent tool results.\n\n\
         Available tools (name: description; JSON Schema of args):\n",
    );
    for tool in tools {
        text.push_str(&format!(
            "- {}: {} Args: {}\n",
            tool.name, tool.description, tool.parameters
        ));
    }
    text
}

/// Extracts `<tool_call>` blocks. Returns the calls and the text without
/// them.
pub fn parse_prompt_calls(text: &str, next_id: &mut u32) -> (Vec<ToolCallPart>, String) {
    let mut calls = Vec::new();
    let mut visible = String::new();
    let mut rest = text;
    while let Some(start) = rest.find(CALL_OPEN) {
        visible.push_str(&rest[..start]);
        let after = &rest[start + CALL_OPEN.len()..];
        let (inner, remainder) = match after.find(CALL_CLOSE) {
            Some(end) => (&after[..end], &after[end + CALL_CLOSE.len()..]),
            None => (after, ""),
        };
        *next_id += 1;
        let id = format!("call_{next_id}");
        let parsed = serde_json::from_str::<Value>(inner.trim());
        let call = match parsed {
            Ok(Value::Object(mut object)) => {
                let tool = object
                    .remove("tool")
                    .and_then(|v| v.as_str().map(str::to_owned))
                    .unwrap_or_default();
                let args = object.remove("args").unwrap_or_else(|| json!({}));
                ToolCallPart {
                    id,
                    invalid: tool
                        .is_empty()
                        .then(|| "missing \"tool\" in <tool_call>".to_owned()),
                    name: tool,
                    args,
                    native_id: false,
                }
            }
            Ok(_) => ToolCallPart {
                id,
                name: String::new(),
                args: Value::Null,
                native_id: false,
                invalid: Some("<tool_call> must contain a JSON object".into()),
            },
            Err(err) => ToolCallPart {
                id,
                name: String::new(),
                args: Value::Null,
                native_id: false,
                invalid: Some(format!("invalid JSON in <tool_call>: {err}")),
            },
        };
        calls.push(call);
        rest = remainder;
    }
    visible.push_str(rest);
    (calls, visible)
}

/// Results of prompt-protocol calls, as the next user message.
pub fn prompt_results(results: &[ToolResultPart]) -> String {
    results
        .iter()
        .map(|r| {
            format!(
                "<tool_result tool=\"{}\" ok=\"{}\">\n{}\n</tool_result>",
                if r.name.is_empty() { "?" } else { &r.name },
                !r.is_error,
                r.content
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Hides `<tool_call>…</tool_call>` from streamed text.
#[derive(Default)]
pub struct MarkupFilter {
    pending: String,
    inside: bool,
}

impl MarkupFilter {
    /// Visible part of the new text (may hold back a possible tag start).
    pub fn push(&mut self, chunk: &str) -> String {
        self.pending.push_str(chunk);
        let mut out = String::new();
        loop {
            if self.inside {
                match self.pending.find(CALL_CLOSE) {
                    Some(end) => {
                        self.pending.drain(..end + CALL_CLOSE.len());
                        self.inside = false;
                    }
                    None => {
                        let keep = partial_suffix(&self.pending, CALL_CLOSE);
                        let cut = self.pending.len() - keep;
                        self.pending.drain(..cut);
                        return out;
                    }
                }
            } else {
                match self.pending.find(CALL_OPEN) {
                    Some(start) => {
                        out.push_str(&self.pending[..start]);
                        self.pending.drain(..start + CALL_OPEN.len());
                        self.inside = true;
                    }
                    None => {
                        let keep = partial_suffix(&self.pending, CALL_OPEN);
                        let cut = self.pending.len() - keep;
                        out.push_str(&self.pending[..cut]);
                        self.pending.drain(..cut);
                        return out;
                    }
                }
            }
        }
    }

    /// Whatever was held back, at the end of the reply.
    pub fn finish(&mut self) -> String {
        let rest = std::mem::take(&mut self.pending);
        if self.inside {
            String::new()
        } else {
            rest
        }
    }
}

/// Length of the longest suffix of `text` that is a proper prefix of `tag`.
fn partial_suffix(text: &str, tag: &str) -> usize {
    (1..tag.len())
        .rev()
        .find(|&n| {
            text.len() >= n
                && text.is_char_boundary(text.len() - n)
                && tag.starts_with(&text[text.len() - n..])
        })
        .unwrap_or(0)
}

// --------------------------------------------------------------- schemas ---

/// JSON Schema → the OpenAPI subset accepted by Gemini function
/// declarations. Returns `None` for an object without properties (Gemini
/// rejects empty OBJECT schemas; the declaration then omits `parameters`).
pub fn gemini_schema(schema: &Value) -> Option<Value> {
    let object = schema.as_object()?;
    let mut out = Map::new();
    let (ty, nullable) = match object.get("type") {
        Some(Value::String(t)) => (Some(t.clone()), false),
        Some(Value::Array(types)) => {
            let nullable = types.iter().any(|t| t == "null");
            let first = types
                .iter()
                .filter_map(Value::as_str)
                .find(|t| *t != "null")
                .map(str::to_owned);
            (first, nullable)
        }
        _ => (None, false),
    };
    if let Some(description) = object.get("description").and_then(Value::as_str) {
        out.insert("description".into(), description.into());
    }
    if nullable {
        out.insert("nullable".into(), true.into());
    }
    if let Some(values) = object.get("enum").and_then(Value::as_array) {
        let strings: Vec<Value> = values.iter().filter(|v| v.is_string()).cloned().collect();
        if !strings.is_empty() {
            out.insert("type".into(), "string".into());
            out.insert("enum".into(), Value::Array(strings));
            return Some(Value::Object(out));
        }
    }
    let ty = ty.unwrap_or_else(|| "string".into());
    match ty.as_str() {
        "object" => {
            let mut properties = Map::new();
            if let Some(props) = object.get("properties").and_then(Value::as_object) {
                for (name, prop) in props {
                    if let Some(converted) = gemini_schema(prop) {
                        properties.insert(name.clone(), converted);
                    }
                }
            }
            if properties.is_empty() {
                return None;
            }
            let required: Vec<Value> = object
                .get("required")
                .and_then(Value::as_array)
                .map(|r| {
                    r.iter()
                        .filter(|name| name.as_str().is_some_and(|n| properties.contains_key(n)))
                        .cloned()
                        .collect()
                })
                .unwrap_or_default();
            out.insert("type".into(), "object".into());
            out.insert("properties".into(), Value::Object(properties));
            if !required.is_empty() {
                out.insert("required".into(), Value::Array(required));
            }
        }
        "array" => {
            out.insert("type".into(), "array".into());
            let items = object
                .get("items")
                .and_then(gemini_schema)
                .unwrap_or_else(|| json!({"type": "string"}));
            out.insert("items".into(), items);
        }
        "integer" | "number" | "boolean" | "string" => {
            out.insert("type".into(), ty.clone().into());
        }
        _ => {
            out.insert("type".into(), "string".into());
        }
    }
    Some(Value::Object(out))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use orchestrator_core::{ToolCallId, ToolError, ToolErrorKind};

    #[test]
    fn the_note_says_what_the_tools_reach() {
        let tool = |name: &str| ToolDefinition {
            name: name.into(),
            ..ping_tool()
        };
        let note = capabilities_note(&[tool("filesystem.read"), tool("github.pr.list")]).unwrap();
        assert!(
            note.contains("project's files") && note.contains("GitHub"),
            "{note}"
        );
        assert!(note.contains("Conectar ao GitHub") && note.contains("no web browser"));
        assert!(!note.contains("run commands"), "{note}");
        // The connection test's ping, or no tools: nothing to say.
        assert_eq!(capabilities_note(&[ping_tool()]), None);
        assert_eq!(capabilities_note(&[]), None);
    }

    #[test]
    fn names_round_trip() {
        assert_eq!(api_name("filesystem.read"), "filesystem__read");
        assert_eq!(orchestrator_name("filesystem__read"), "filesystem.read");
        assert_eq!(orchestrator_name("custom"), "custom");
    }

    #[test]
    fn results_are_rendered_and_truncated() {
        let now = Utc::now();
        let mut result = ToolResult {
            call_id: ToolCallId::from("c"),
            tool: "x.y".into(),
            ok: true,
            output: json!({"a": 1}),
            error: None,
            started_at: now,
            finished_at: now,
            duration_ms: 1,
        };
        assert_eq!(result_content(&result), ("{\"a\":1}".into(), false));
        result.output = json!("x".repeat(MAX_RESULT_CHARS + 10));
        assert!(result_content(&result).0.contains("[truncated"));
        result.ok = false;
        result.error = Some(ToolError::new(ToolErrorKind::NotFound, "nope"));
        assert_eq!(result_content(&result), ("NOT_FOUND: nope".into(), true));
    }

    #[test]
    fn prompt_calls_are_parsed_and_hidden() {
        let mut id = 0;
        let text = "Vou ler.\n<tool_call>{\"tool\": \"filesystem.read\", \"args\": {\"path\": \"a\"}}</tool_call>\n<tool_call>{bad</tool_call>fim";
        let (calls, visible) = parse_prompt_calls(text, &mut id);
        assert_eq!(visible, "Vou ler.\n\nfim");
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].name, "filesystem.read");
        assert_eq!(calls[0].args, json!({"path": "a"}));
        assert!(calls[0].invalid.is_none());
        assert!(calls[1]
            .invalid
            .as_deref()
            .unwrap()
            .contains("invalid JSON"));
        assert_eq!(calls[1].id, "call_2");
    }

    #[test]
    fn markup_filter_hides_calls_across_chunks() {
        let mut filter = MarkupFilter::default();
        let mut shown = String::new();
        for chunk in [
            "Olá <to",
            "ol_call>{\"tool\":",
            "\"x\"}</tool_",
            "call> tchau <",
            "b>",
        ] {
            shown.push_str(&filter.push(chunk));
        }
        shown.push_str(&filter.finish());
        assert_eq!(shown, "Olá  tchau <b>");
    }

    #[test]
    fn gemini_schema_uses_the_openapi_subset() {
        let schema = json!({
            "type": "object",
            "additionalProperties": false,
            "properties": {
                "path": {"type": ["string", "null"], "default": null, "description": "p"},
                "mode": {"enum": ["merge", "rebase", null], "type": ["string", "null"]},
                "limit": {"type": ["integer", "null"], "format": "uint32", "minimum": 0},
                "env": {"type": "object", "additionalProperties": {"type": "string"}},
                "files": {"type": "array", "items": {"type": "string"}}
            },
            "required": ["path", "env"]
        });
        let converted = gemini_schema(&schema).unwrap();
        assert_eq!(
            converted,
            json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "nullable": true, "description": "p"},
                    "mode": {"type": "string", "nullable": true, "enum": ["merge", "rebase"]},
                    "limit": {"type": "integer", "nullable": true},
                    "files": {"type": "array", "items": {"type": "string"}}
                },
                "required": ["path"]
            })
        );
        assert_eq!(
            gemini_schema(&json!({"type": "object", "properties": {}})),
            None
        );
    }
}
