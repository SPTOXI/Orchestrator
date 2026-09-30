//! Autonomy (ADR-0016): the user decides what an AI may do on its own.
//!
//! - [`AutonomyMode`]: Assisted, Autonomous or Unrestricted.
//! - [`PolicyRule`]: one rule of the user's policy (Autonomous) or of the
//!   fixed Assisted list; the first rule that matches decides.
//! - [`ApprovalRequest`] / [`ApprovalAnswer`]: what the Orchestrator asks
//!   the user when the decision is to ask, and what the user answers.
//!
//! The rules themselves (matching, targets, the gate) belong to the engine;
//! this crate only says what they are.

use crate::ids::{AgentId, ApprovalId, ProviderId, SessionId, TaskId, ToolCallId};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// How much an AI may do without asking (master document, section 10).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AutonomyMode {
    /// Asks for authorization when necessary: every action, and reading
    /// outside the project or environment files.
    Assisted,
    /// Executes according to the policies the user configured.
    Autonomous,
    /// No operational restriction imposed by the Orchestrator. Everything
    /// is still recorded.
    Unrestricted,
}

impl AutonomyMode {
    pub const ALL: [Self; 3] = [Self::Assisted, Self::Autonomous, Self::Unrestricted];

    /// Name the UI and the history show.
    pub fn label(self) -> &'static str {
        match self {
            Self::Assisted => "Assistido",
            Self::Autonomous => "Autônomo",
            Self::Unrestricted => "Acesso Irrestrito",
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Assisted => "assisted",
            Self::Autonomous => "autonomous",
            Self::Unrestricted => "unrestricted",
        }
    }
}

/// What a rule decides about a call.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Decision {
    Allow,
    Ask,
    Deny,
}

impl Decision {
    /// `deny` > `ask` > `allow`: when a call has several targets, the most
    /// restrictive decision wins.
    pub fn strictness(self) -> u8 {
        match self {
            Self::Allow => 0,
            Self::Ask => 1,
            Self::Deny => 2,
        }
    }

    pub fn strictest(self, other: Self) -> Self {
        if other.strictness() > self.strictness() {
            other
        } else {
            self
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Allow => "permitir",
            Self::Ask => "perguntar",
            Self::Deny => "negar",
        }
    }
}

/// Whether a rule is about tools that only query state or tools that act.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RuleAccess {
    /// Tools marked `readOnly` (queries).
    Read,
    /// Every other tool (actions).
    Write,
}

/// Whether a rule is about targets inside or outside the project folder.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RuleWhere {
    Inside,
    Outside,
}

/// One rule. Empty fields match everything; the first rule that matches a
/// target decides it (ADR-0016).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PolicyRule {
    /// Tool name patterns: `git.push`, `filesystem.*` or `*`. Empty: any.
    #[serde(default)]
    pub tools: Vec<String>,
    #[serde(default)]
    pub access: Option<RuleAccess>,
    #[serde(default, rename = "where")]
    pub location: Option<RuleWhere>,
    /// Pattern of a command (`shell.execute`, `process.start`) or of what
    /// is typed into a terminal (`terminal.write`); `*` matches anything.
    #[serde(default)]
    pub command: Option<String>,
    /// Path pattern, as in `.gitignore`: without `/` it matches the file
    /// name; `*` stays inside a folder, `**` crosses folders.
    #[serde(default)]
    pub path: Option<String>,
    pub decision: Decision,
    /// Why the rule exists, in the user's words.
    #[serde(default)]
    pub note: Option<String>,
}

impl PolicyRule {
    /// A rule about some tools, with nothing else to match.
    pub fn tools(tools: &[&str], decision: Decision) -> Self {
        Self {
            tools: tools.iter().map(|t| (*t).to_owned()).collect(),
            access: None,
            location: None,
            command: None,
            path: None,
            decision,
            note: None,
        }
    }

    pub fn with_access(mut self, access: RuleAccess) -> Self {
        self.access = Some(access);
        self
    }

    pub fn with_location(mut self, location: RuleWhere) -> Self {
        self.location = Some(location);
        self
    }

    pub fn with_command(mut self, pattern: &str) -> Self {
        self.command = Some(pattern.to_owned());
        self
    }

    pub fn with_path(mut self, pattern: &str) -> Self {
        self.path = Some(pattern.to_owned());
        self
    }

    pub fn with_note(mut self, note: &str) -> Self {
        self.note = Some(note.to_owned());
        self
    }

    /// The rule as a sentence: "comando `rm *` → perguntar".
    pub fn describe(&self) -> String {
        let mut parts = Vec::new();
        let tools = if self.tools.is_empty() || self.tools.iter().any(|t| t == "*") {
            None
        } else {
            Some(self.tools.join(", "))
        };
        match self.access {
            Some(RuleAccess::Read) => parts.push("consultas".to_owned()),
            Some(RuleAccess::Write) => parts.push("ações".to_owned()),
            None => {}
        }
        if let Some(tools) = tools {
            parts.push(tools);
        }
        if let Some(command) = &self.command {
            parts.push(format!("comando `{command}`"));
        }
        if let Some(path) = &self.path {
            parts.push(format!("caminho `{path}`"));
        }
        match self.location {
            Some(RuleWhere::Inside) => parts.push("dentro do projeto".to_owned()),
            Some(RuleWhere::Outside) => parts.push("fora do projeto".to_owned()),
            None => {}
        }
        let what = if parts.is_empty() {
            "qualquer chamada".to_owned()
        } else {
            parts.join(" · ")
        };
        format!("{what} → {}", self.decision.label())
    }
}

/// What the Orchestrator asks the user when the decision is to ask.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApprovalRequest {
    pub id: ApprovalId,
    pub call_id: ToolCallId,
    pub tool: String,
    /// What the AI wants, in words: "Executar `npm test`".
    pub summary: String,
    /// The command, the paths, the beginning of what would be written.
    pub detail: Option<String>,
    /// Why it asks: the mode and the rule.
    pub reason: String,
    pub mode: AutonomyMode,
    /// Number (1-based) of the rule that asked, in the mode's list.
    pub rule: Option<u32>,
    /// Command the call carries, for "Permitir nesta sessão".
    pub command: Option<String>,
    pub session_id: SessionId,
    pub agent_id: Option<AgentId>,
    pub agent_title: Option<String>,
    pub task_id: Option<TaskId>,
    pub project_id: Option<String>,
    pub provider: Option<ProviderId>,
    pub requested_at: DateTime<Utc>,
}

/// The user's answer to an [`ApprovalRequest`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ApprovalAnswer {
    /// Execute this call.
    Approve,
    /// Execute, and do not ask again in this session what the same rule
    /// would ask about the same tool (and the same command).
    ApproveSession,
    /// Refuse; the AI receives `DENIED` with the user's note.
    Deny,
}

impl ApprovalAnswer {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Approve => "approve",
            Self::ApproveSession => "approveSession",
            Self::Deny => "deny",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn modes_and_decisions_use_stable_names() {
        assert_eq!(
            serde_json::to_value(AutonomyMode::Unrestricted).unwrap(),
            json!("unrestricted")
        );
        assert_eq!(
            serde_json::to_value(ApprovalAnswer::ApproveSession).unwrap(),
            json!("approveSession")
        );
        for mode in AutonomyMode::ALL {
            assert_eq!(serde_json::to_value(mode).unwrap(), json!(mode.as_str()));
        }
        assert_eq!(AutonomyMode::Unrestricted.label(), "Acesso Irrestrito");
    }

    #[test]
    fn the_most_restrictive_decision_wins() {
        assert_eq!(Decision::Allow.strictest(Decision::Ask), Decision::Ask);
        assert_eq!(Decision::Deny.strictest(Decision::Ask), Decision::Deny);
        assert_eq!(Decision::Allow.strictest(Decision::Allow), Decision::Allow);
    }

    #[test]
    fn a_rule_reads_as_a_sentence_and_keeps_where_in_json() {
        let rule = PolicyRule::tools(&["*"], Decision::Ask).with_location(RuleWhere::Outside);
        assert_eq!(rule.describe(), "fora do projeto → perguntar");
        let value = serde_json::to_value(&rule).unwrap();
        assert_eq!(value["where"], json!("outside"));
        let back: PolicyRule = serde_json::from_value(value).unwrap();
        assert_eq!(back, rule);

        let command = PolicyRule::tools(&[], Decision::Ask).with_command("rm *");
        assert_eq!(command.describe(), "comando `rm *` → perguntar");
        let reads = PolicyRule::tools(&[], Decision::Allow).with_access(RuleAccess::Read);
        assert_eq!(reads.describe(), "consultas → permitir");
        assert_eq!(
            PolicyRule::tools(&["git.push", "git.reset"], Decision::Ask).describe(),
            "git.push, git.reset → perguntar"
        );

        // A typo in a hand-edited file is an error, not a rule that
        // silently matches everything.
        assert!(serde_json::from_value::<PolicyRule>(
            json!({"tools": ["x"], "decision": "ask", "comand": "rm *"})
        )
        .is_err());
    }
}
