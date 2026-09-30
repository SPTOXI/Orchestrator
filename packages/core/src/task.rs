//! Tasks (ADR-0014): every relevant piece of work in a project, with its
//! state, priority, dependencies and the AI chosen for it.
//!
//! The contract mirrors section 12 of the master document. Rules (valid
//! transitions, dependencies, readiness) live in the engine; the store
//! keeps the rows.

use crate::ids::{ProviderId, SessionId, TaskId};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// State of a task (master document, section 12).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum TaskStatus {
    Todo,
    InProgress,
    /// Stopped by something outside the project's tasks; only the user
    /// sets and clears it (ADR-0014).
    Blocked,
    Review,
    Done,
    Cancelled,
}

impl TaskStatus {
    /// Order the board and the panel use: what is moving comes first.
    pub fn rank(self) -> u8 {
        match self {
            Self::InProgress => 0,
            Self::Review => 1,
            Self::Blocked => 2,
            Self::Todo => 3,
            Self::Done => 4,
            Self::Cancelled => 5,
        }
    }

    /// Nothing else is expected to happen to the task.
    pub fn is_final(self) -> bool {
        matches!(self, Self::Done | Self::Cancelled)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Todo => "TODO",
            Self::InProgress => "IN_PROGRESS",
            Self::Blocked => "BLOCKED",
            Self::Review => "REVIEW",
            Self::Done => "DONE",
            Self::Cancelled => "CANCELLED",
        }
    }
}

#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize,
)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum TaskPriority {
    Low,
    #[default]
    Normal,
    High,
    Urgent,
}

impl TaskPriority {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Low => "LOW",
            Self::Normal => "NORMAL",
            Self::High => "HIGH",
            Self::Urgent => "URGENT",
        }
    }
}

/// A task of a project.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Task {
    pub id: TaskId,
    pub project_id: String,
    pub title: String,
    pub description: String,
    pub status: TaskStatus,
    pub priority: TaskPriority,
    /// AI chosen for the task (by the user or the router).
    pub provider: Option<ProviderId>,
    pub model: Option<String>,
    /// Who executes it. Always `None` until agents exist (Fase 8b).
    pub agent: Option<String>,
    /// This task is a subtask of that one.
    pub parent_task: Option<TaskId>,
    /// Tasks that must reach `DONE` before this one can start.
    pub dependencies: Vec<TaskId>,
    /// Paths relevant to the work, relative to the project.
    pub files: Vec<String>,
    /// Sessions opened for this task, oldest first.
    pub sessions: Vec<SessionId>,
    /// What the task delivered, written when it is concluded.
    pub result: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
}

/// What the UI sends to create or edit a task. Missing fields keep the
/// stored value on an edit, and take the default on a creation.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct TaskInput {
    /// Task to edit; `None` creates one.
    pub id: Option<TaskId>,
    /// Project of a new task; the stored one wins on an edit.
    pub project_id: Option<String>,
    pub title: Option<String>,
    pub description: Option<String>,
    pub priority: Option<TaskPriority>,
    pub provider: Option<ProviderId>,
    /// An empty string clears the model.
    pub model: Option<String>,
    pub parent_task: Option<TaskId>,
    pub dependencies: Option<Vec<TaskId>>,
    pub files: Option<Vec<String>>,
    pub result: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn task_uses_the_master_document_names() {
        let task = Task {
            id: TaskId::new(),
            project_id: "p1".into(),
            title: "Implementar autenticação OAuth".into(),
            description: String::new(),
            status: TaskStatus::InProgress,
            priority: TaskPriority::High,
            provider: Some(ProviderId::from("nuvem-a")),
            model: None,
            agent: None,
            parent_task: None,
            dependencies: Vec::new(),
            files: vec!["src/auth.ts".into()],
            sessions: Vec::new(),
            result: String::new(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            started_at: None,
            finished_at: None,
        };
        let value = serde_json::to_value(&task).unwrap();
        assert_eq!(value["status"], "IN_PROGRESS");
        assert_eq!(value["priority"], "HIGH");
        assert_eq!(value["parentTask"], json!(null));
        assert_eq!(value["files"], json!(["src/auth.ts"]));

        // The UI may send only what changed.
        let input: TaskInput = serde_json::from_value(json!({"title": "Outro"})).unwrap();
        assert_eq!(input.title.as_deref(), Some("Outro"));
        assert!(input.dependencies.is_none());
    }

    #[test]
    fn panel_order_puts_moving_work_first() {
        let mut all = [
            TaskStatus::Done,
            TaskStatus::Todo,
            TaskStatus::InProgress,
            TaskStatus::Cancelled,
            TaskStatus::Review,
            TaskStatus::Blocked,
        ];
        all.sort_by_key(|s| s.rank());
        assert_eq!(
            all,
            [
                TaskStatus::InProgress,
                TaskStatus::Review,
                TaskStatus::Blocked,
                TaskStatus::Todo,
                TaskStatus::Done,
                TaskStatus::Cancelled
            ]
        );
        assert!(TaskStatus::Done.is_final() && !TaskStatus::Blocked.is_final());
    }
}
