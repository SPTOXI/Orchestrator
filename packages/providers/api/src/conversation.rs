//! Protocol-neutral conversation kept by the adapter for each session.
//!
//! Assistant messages may carry the provider's own content (`native`),
//! which is sent back unchanged when required (Anthropic thinking blocks
//! with signatures, Gemini thought signatures). History is append-only.
//!
//! It is serializable so a session can be resumed after the app restarts
//! (`AIProvider::snapshot`, ADR-0012).

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    User,
    Assistant,
}

/// A tool the model asked for. `name` is the Orchestrator name
/// (`filesystem.read`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolCallPart {
    pub id: String,
    pub name: String,
    pub args: Value,
    /// False when the id was generated locally (the API sent none).
    pub native_id: bool,
    /// Why the arguments could not be used (invalid JSON, truncated…).
    pub invalid: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolResultPart {
    pub id: String,
    pub name: String,
    pub content: String,
    pub is_error: bool,
    pub native_id: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "camelCase")]
pub enum Part {
    Text(String),
    ToolCall(ToolCallPart),
    ToolResult(ToolResultPart),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Conversation {
    pub system: Option<String>,
    pub messages: Vec<Message>,
    /// Counter for locally generated tool call ids (prompt protocol).
    pub next_call: u32,
    /// The latest summary that replaced the earlier messages (ADR-0018).
    /// It opens the next user message after the compaction.
    pub summary: Option<String>,
    /// How many times this conversation was compacted.
    pub compactions: u32,
    /// Size of the next prompt as the provider reported it (last prompt +
    /// reply + tool results since); 0 = unknown, estimate instead.
    pub last_prompt_tokens: u64,
    /// Estimated size right after the last compaction (system, tools and
    /// summary): what compacting again cannot remove.
    pub floor_tokens: u64,
}
