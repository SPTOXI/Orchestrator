//! Agents (ADR-0015): who executes a task, in a session, with the tools
//! and the context the Orchestrator gave them.
//!
//! Agents are disposable; the task, the memory and the history are not —
//! nothing the project needs later lives only here. The rules (queue,
//! turns, locks, delegation) are the agent manager's; this crate only says
//! what an agent is.

use crate::autonomy::AutonomyMode;
use crate::context::ContextSummary;
use crate::ids::{AgentId, HandoffId, ProviderId, SessionId, TaskId};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// State of an agent (master document, section 14).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum AgentStatus {
    /// Waiting for a slot, or for the files of its task to be free.
    Queued,
    Running,
    /// Finished and reported a result (`agent.finish`).
    Done,
    /// Ended by an error or by the turn ceiling.
    Failed,
    /// Ended by the user (`Cancel` / `Stop All Agents`).
    Stopped,
}

impl AgentStatus {
    /// Board order: what is working comes first.
    pub fn rank(self) -> u8 {
        match self {
            Self::Running => 0,
            Self::Queued => 1,
            Self::Done => 2,
            Self::Failed => 3,
            Self::Stopped => 4,
        }
    }

    /// The agent will not run again. An agent never restarts: what
    /// restarts is the task, with another agent.
    pub fn is_final(self) -> bool {
        matches!(self, Self::Done | Self::Failed | Self::Stopped)
    }

    /// Queued or running: it still holds a slot and its file locks.
    pub fn is_live(self) -> bool {
        !self.is_final()
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "QUEUED",
            Self::Running => "RUNNING",
            Self::Done => "DONE",
            Self::Failed => "FAILED",
            Self::Stopped => "STOPPED",
        }
    }
}

/// An agent: one task, one session, one outcome.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Agent {
    pub id: AgentId,
    pub project_id: String,
    /// The task it executes. An agent without a task does not exist.
    pub task: TaskId,
    /// Shown in the board and in the panel; taken from the task.
    pub title: String,
    pub provider: ProviderId,
    pub model: Option<String>,
    /// Session it works in; `None` while it is queued.
    pub session: Option<SessionId>,
    /// The agent that delegated this one (subagent).
    pub parent_agent: Option<AgentId>,
    pub status: AgentStatus,
    /// Tools offered to it when it started.
    pub tools: Vec<String>,
    /// Project context it received on its first turn.
    pub context: Option<ContextSummary>,
    /// Turns spent, and the ceiling it was started with.
    pub turns: u32,
    pub max_turns: u32,
    /// Paths it locked, relative to the project.
    pub files: Vec<String>,
    /// What it delivered (`agent.finish`).
    pub result: String,
    /// Why it failed, when it failed.
    pub error: Option<String>,
    /// Handoff created when it stopped before finishing.
    pub handoff: Option<HandoffId>,
    /// Autonomy mode the user granted to this agent (and its subagents);
    /// `None`: the project's (ADR-0016).
    #[serde(default)]
    pub autonomy: Option<AutonomyMode>,
    /// What this agent may spend (USD) before it stops; `None` = no
    /// ceiling (ADR-0018).
    #[serde(default)]
    pub max_cost_usd: Option<f64>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
}

impl Agent {
    /// One line for the panel and the board.
    pub fn progress(&self) -> String {
        match self.status {
            AgentStatus::Queued => "na fila".to_owned(),
            _ => format!("{} de {} turnos", self.turns, self.max_turns),
        }
    }
}

/// A file held by an agent while it works (ADR-0015). One owner per path:
/// the pair `(project_id, path)` is the key in the store.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileLock {
    pub project_id: String,
    /// Path relative to the project.
    pub path: String,
    pub agent_id: AgentId,
    /// Agent's title, so the refusal names something a person recognizes.
    pub agent_title: String,
    pub task: TaskId,
    pub at: DateTime<Utc>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn agent() -> Agent {
        Agent {
            id: AgentId::new(),
            project_id: "p1".into(),
            task: TaskId::from("t1"),
            title: "Aplicar retentativas".into(),
            provider: ProviderId::from("nuvem-a"),
            model: Some("m".into()),
            session: None,
            parent_agent: None,
            status: AgentStatus::Queued,
            tools: vec!["filesystem.write".into()],
            context: None,
            turns: 0,
            max_turns: 12,
            files: Vec::new(),
            result: String::new(),
            error: None,
            handoff: None,
            autonomy: None,
            max_cost_usd: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            started_at: None,
            finished_at: None,
        }
    }

    #[test]
    fn agent_uses_the_master_document_names() {
        let mut agent = agent();
        agent.status = AgentStatus::Running;
        agent.session = Some(SessionId::from("s1"));
        let value = serde_json::to_value(&agent).unwrap();
        assert_eq!(value["status"], "RUNNING");
        assert_eq!(value["task"], "t1");
        assert_eq!(value["session"], "s1");
        assert_eq!(value["parentAgent"], json!(null));
        assert_eq!(value["maxTurns"], 12);
        assert_eq!(value["autonomy"], json!(null));

        // A Phase 8b row has no `autonomy`: it reads as the project's mode.
        let mut old = value.clone();
        old.as_object_mut().unwrap().remove("autonomy");
        let back: Agent = serde_json::from_value(old).unwrap();
        assert_eq!(back.autonomy, None);
        agent.autonomy = Some(crate::AutonomyMode::Unrestricted);
        let value = serde_json::to_value(&agent).unwrap();
        assert_eq!(value["autonomy"], "unrestricted");
    }

    #[test]
    fn board_order_puts_working_agents_first_and_ended_ones_last() {
        let mut all = [
            AgentStatus::Done,
            AgentStatus::Queued,
            AgentStatus::Stopped,
            AgentStatus::Running,
            AgentStatus::Failed,
        ];
        all.sort_by_key(|s| s.rank());
        assert_eq!(
            all,
            [
                AgentStatus::Running,
                AgentStatus::Queued,
                AgentStatus::Done,
                AgentStatus::Failed,
                AgentStatus::Stopped
            ]
        );
        // Queued and running hold a slot; the rest are history.
        assert!(AgentStatus::Queued.is_live() && AgentStatus::Running.is_live());
        assert!(AgentStatus::Failed.is_final() && AgentStatus::Stopped.is_final());
    }

    #[test]
    fn progress_reads_as_the_panel_shows_it() {
        let mut agent = agent();
        assert_eq!(agent.progress(), "na fila");
        agent.status = AgentStatus::Running;
        agent.turns = 3;
        assert_eq!(agent.progress(), "3 de 12 turnos");
    }
}
