//! Handoff between AIs (ADR-0013): what one AI leaves so that another can
//! continue the work without the previous conversation.

use crate::ids::{HandoffId, ProviderId, SessionId};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// The packet of the master document (section 17). Plain, editable text:
/// the user reviews it before another AI takes over.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct HandoffPacket {
    pub goal: String,
    /// Where the work stands ("50% concluído", "bloqueado no webhook").
    pub status: String,
    pub completed: Vec<String>,
    pub remaining: Vec<String>,
    /// Paths, relative to the project when inside it.
    pub files: Vec<String>,
    /// Commands run, with their outcome ("pnpm test → saída 1").
    pub commands: Vec<String>,
    pub errors: Vec<String>,
    pub decisions: Vec<String>,
    pub tests: Vec<String>,
    pub next_action: String,
}

impl HandoffPacket {
    /// Every list field with its name, in the master document's order.
    pub fn lists(&self) -> [(&'static str, &Vec<String>); 7] {
        [
            ("completed", &self.completed),
            ("remaining", &self.remaining),
            ("files", &self.files),
            ("commands", &self.commands),
            ("errors", &self.errors),
            ("decisions", &self.decisions),
            ("tests", &self.tests),
        ]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum HandoffStatus {
    /// Saved; no AI has taken it over yet.
    Created,
    /// A session took it over (`HANDOFF_ACCEPTED`).
    Accepted,
}

/// One end of a handoff: a session and the AI behind it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HandoffEnd {
    pub session_id: SessionId,
    pub provider: ProviderId,
    pub model: Option<String>,
    pub title: String,
}

/// A saved handoff.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Handoff {
    pub id: HandoffId,
    pub project_id: Option<String>,
    pub project_path: String,
    pub from: HandoffEnd,
    pub to: Option<HandoffEnd>,
    pub packet: HandoffPacket,
    pub status: HandoffStatus,
    /// True when the narrative fields came from the source AI (the facts
    /// always come from the history).
    pub by_agent: bool,
    pub created_at: DateTime<Utc>,
    pub accepted_at: Option<DateTime<Utc>>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn packet_uses_the_master_document_names() {
        let packet = HandoffPacket {
            goal: "Implementar pagamentos Stripe".into(),
            next_action: "Validar a assinatura do webhook".into(),
            ..Default::default()
        };
        let value = serde_json::to_value(&packet).unwrap();
        assert_eq!(value["goal"], "Implementar pagamentos Stripe");
        assert_eq!(value["nextAction"], "Validar a assinatura do webhook");
        assert_eq!(value["completed"], json!([]));
        // Missing fields read as empty: drafts may be partial.
        let back: HandoffPacket = serde_json::from_value(json!({"goal": "x"})).unwrap();
        assert_eq!(back.goal, "x");
        assert!(back.tests.is_empty());
        let names: Vec<_> = packet.lists().iter().map(|(n, _)| *n).collect();
        assert_eq!(
            names,
            [
                "completed",
                "remaining",
                "files",
                "commands",
                "errors",
                "decisions",
                "tests"
            ]
        );
    }
}
