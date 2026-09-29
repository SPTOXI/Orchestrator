//! What was sent to an AI as project context (ADR-0013). The text itself
//! stays with the session; the history keeps this summary.

use crate::ids::HandoffId;
use serde::{Deserialize, Serialize};

/// One section of a context pack, without its text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextSectionSummary {
    /// `task`, `working`, `project`, `files`, `errors`, `history`, `git`,
    /// `handoff`.
    pub kind: String,
    pub title: String,
    /// Items sent.
    pub items: u32,
    pub tokens: u32,
}

/// Summary of a context pack: estimated tokens, sections and what was left
/// out (and why).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ContextSummary {
    /// Estimated tokens of the whole text.
    pub tokens: u32,
    pub budget: u32,
    pub sections: Vec<ContextSectionSummary>,
    /// Human-readable omissions, e.g. "2 itens de histórico (orçamento)".
    pub omitted: Vec<String>,
    /// The handoff this context carries, if any.
    pub handoff_id: Option<HandoffId>,
}
