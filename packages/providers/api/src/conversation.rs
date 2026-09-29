//! Protocol-neutral conversation kept by the adapter for each session.
//!
//! Assistant messages may carry the provider's own content (`native`),
//! which is sent back unchanged when required (Anthropic thinking blocks
//! with signatures, Gemini thought signatures). History is append-only.

use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    User,
    Assistant,
}

/// A tool the model asked for. `name` is the Orchestrator name
/// (`filesystem.read`).
#[derive(Debug, Clone, PartialEq)]
pub struct ToolCallPart {
    pub id: String,
    pub name: String,
    pub args: Value,
    /// False when the id was generated locally (the API sent none).
    pub native_id: bool,
    /// Why the arguments could not be used (invalid JSON, truncated…).
    pub invalid: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ToolResultPart {
    pub id: String,
    pub name: String,
    pub content: String,
    pub is_error: bool,
    pub native_id: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Part {
    Text(String),
    ToolCall(ToolCallPart),
    ToolResult(ToolResultPart),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Message {
    pub role: Role,
    pub parts: Vec<Part>,
    /// Provider-native content of an assistant message.
    pub native: Option<Value>,
}

impl Message {
    pub fn user(text: impl Into<String>) -> Self {
        Self {
            role: Role::User,
            parts: vec![Part::Text(text.into())],
            native: None,
        }
    }

    /// Concatenated text parts.
    pub fn text(&self) -> String {
        self.parts
            .iter()
            .filter_map(|p| match p {
                Part::Text(t) => Some(t.as_str()),
                _ => None,
            })
            .collect()
    }

    pub fn tool_calls(&self) -> impl Iterator<Item = &ToolCallPart> {
        self.parts.iter().filter_map(|p| match p {
            Part::ToolCall(c) => Some(c),
            _ => None,
        })
    }

    pub fn tool_results(&self) -> impl Iterator<Item = &ToolResultPart> {
        self.parts.iter().filter_map(|p| match p {
            Part::ToolResult(r) => Some(r),
            _ => None,
        })
    }
}

#[derive(Debug, Clone, Default)]
pub struct Conversation {
    pub system: Option<String>,
    pub messages: Vec<Message>,
    /// Counter for locally generated tool call ids (prompt protocol).
    pub next_call: u32,
}
