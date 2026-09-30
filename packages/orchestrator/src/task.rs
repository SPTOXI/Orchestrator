//! The Task Manager (ADR-0014): the rules around a task — what it may
//! become, what it waits for, and the session that works on it.
//!
//! The store keeps the rows; everything decided here is decided once, so
//! the UI only shows the transitions this service would accept.

use crate::builder::{ContextBuilder, ContextPack};
use crate::text::clip;
use chrono::Utc;
use orchestrator_core::{
    AuditEvent, CallOrigin, EventKind, EventSink, ProviderId, SessionInfo, Task, TaskId, TaskInput,
    TaskPriority, TaskStatus, TurnId,
};
use orchestrator_memory::MemoryStore;
use orchestrator_providers::{ProviderError, ProviderErrorKind, SessionManager, StartRequest};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

/// Characters of the title.
pub const MAX_TITLE: usize = 200;
/// Characters of the description and of the result.
pub const MAX_TEXT: usize = 4_000;
pub const MAX_FILES: usize = 30;
pub const MAX_DEPENDENCIES: usize = 20;

/// Another task, as this one needs to show it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskRef {
    pub id: TaskId,
    pub title: String,
    pub status: TaskStatus,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SubtaskProgress {
    pub done: u32,
    pub total: u32,
}

/// A task with what the panel needs but the task itself does not store.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskView {
    #[serde(flatten)]
    pub task: Task,
    /// Dependencies that have not reached `DONE`: while this is not empty,
    /// the task cannot start.
    pub waiting_for: Vec<TaskRef>,
    pub subtasks: SubtaskProgress,
    /// States this task may move to now.
    pub can: Vec<TaskStatus>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StartTaskSession {
    pub task_id: TaskId,
    /// `None`: the active provider.
    pub provider: Option<ProviderId>,
    pub model: Option<String>,
    /// Context budget of the session; `None` = the setting.
    pub budget: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StartedTask {
    pub task: Task,
    pub session: SessionInfo,
    /// Turn carrying the task as the first message, when it could be sent.
    pub turn_id: Option<TurnId>,
    pub send_error: Option<String>,
}

/// States a task may move to from `from`, in the order the UI shows them
/// (ADR-0014). Ending states are reopened, never edited into.
pub fn next_states(from: TaskStatus) -> Vec<TaskStatus> {
    use TaskStatus::*;
    match from {
        Todo => vec![InProgress, Blocked, Cancelled],
        InProgress => vec![Review, Done, Blocked, Todo, Cancelled],
        Blocked => vec![InProgress, Todo, Cancelled],
        Review => vec![Done, InProgress, Cancelled],
        Done => vec![Todo],
        Cancelled => vec![Todo],
    }
}

fn status_label(status: TaskStatus) -> &'static str {
    match status {
        TaskStatus::Todo => "a fazer",
        TaskStatus::InProgress => "em andamento",
        TaskStatus::Blocked => "bloqueada",
        TaskStatus::Review => "em revisão",
        TaskStatus::Done => "concluída",
        TaskStatus::Cancelled => "cancelada",
    }
}

fn invalid(message: impl Into<String>) -> ProviderError {
    ProviderError::invalid(message)
}

fn not_found(id: &TaskId) -> ProviderError {
    ProviderError::new(
        ProviderErrorKind::NotFound,
        format!("task {id} não encontrada"),
    )
}

/// The task as an AI reads it: the goal, the detail and where to look.
pub fn as_task_text(task: &Task) -> String {
    let mut out = task.title.clone();
    if !task.description.is_empty() {
        out.push_str("\n\n");
        out.push_str(&task.description);
    }
    if !task.files.is_empty() {
        out.push_str("\n\nArquivos: ");
        out.push_str(&task.files.join(", "));
    }
    out
}

/// A session opened for a task, before anything is sent to it.
#[derive(Debug, Clone, PartialEq)]
pub struct OpenedTask {
    pub task: Task,
    pub session: SessionInfo,
    /// What the session must be told first.
    pub first_message: String,
}

/// First message of a session opened for a task.
fn first_message(task: &Task) -> String {
    format!(
        "{}\n\nEsta é a task \"{}\" do projeto. O contexto do projeto está nas suas \
         instruções. Comece confirmando o que já existe antes de mudar qualquer arquivo.",
        as_task_text(task),
        task.title
    )
}

#[derive(Clone)]
pub struct TaskService {
    sessions: SessionManager,
    store: Arc<MemoryStore>,
    builder: Arc<ContextBuilder>,
    sink: Arc<dyn EventSink>,
}

impl TaskService {
    pub fn new(
        sessions: SessionManager,
        store: Arc<MemoryStore>,
        builder: Arc<ContextBuilder>,
        sink: Arc<dyn EventSink>,
    ) -> Self {
        Self {
            sessions,
            store,
            builder,
            sink,
        }
    }

    /// Tasks of a project, in panel order, each with what it waits for.
    pub fn list(&self, project_id: Option<&str>) -> Vec<TaskView> {
        let tasks = self.store.tasks_list(project_id);
        let by_id: HashMap<&str, &Task> = tasks.iter().map(|t| (t.id.as_str(), t)).collect();
        let mut children: HashMap<&str, (u32, u32)> = HashMap::new();
        for task in &tasks {
            if let Some(parent) = &task.parent_task {
                let entry = children.entry(parent.as_str()).or_default();
                entry.1 += 1;
                if task.status == TaskStatus::Done {
                    entry.0 += 1;
                }
            }
        }
        tasks
            .iter()
            .map(|task| {
                let (done, total) = children.get(task.id.as_str()).copied().unwrap_or((0, 0));
                TaskView {
                    waiting_for: pending_of(task, &by_id),
                    subtasks: SubtaskProgress { done, total },
                    can: next_states(task.status),
                    task: task.clone(),
                }
            })
            .collect()
    }

    pub fn get(&self, id: &TaskId) -> Option<TaskView> {
        let task = self.store.task(id.as_str())?;
        let all = self.store.tasks_list(Some(&task.project_id));
        let by_id: HashMap<&str, &Task> = all.iter().map(|t| (t.id.as_str(), t)).collect();
        let (done, total) = all
            .iter()
            .filter(|t| t.parent_task.as_ref() == Some(id))
            .fold((0, 0), |(done, total), t| {
                (done + u32::from(t.status == TaskStatus::Done), total + 1)
            });
        Some(TaskView {
            waiting_for: pending_of(&task, &by_id),
            subtasks: SubtaskProgress { done, total },
            can: next_states(task.status),
            task,
        })
    }

    /// Creates a task (no `id`) or edits one. Only what the input carries
    /// changes; the rest of the task stays as it is.
    pub fn save(&self, input: TaskInput, origin: CallOrigin) -> Result<Task, ProviderError> {
        let now = Utc::now();
        let existing = match &input.id {
            Some(id) => Some(self.store.task(id.as_str()).ok_or_else(|| not_found(id))?),
            None => None,
        };
        let project_id = existing
            .as_ref()
            .map(|t| t.project_id.clone())
            .or_else(|| input.project_id.clone())
            .ok_or_else(|| invalid("a task precisa de um projeto aberto"))?;

        let mut task = existing.clone().unwrap_or_else(|| Task {
            id: TaskId::new(),
            project_id: project_id.clone(),
            title: String::new(),
            description: String::new(),
            status: TaskStatus::Todo,
            priority: TaskPriority::Normal,
            provider: None,
            model: None,
            agent: None,
            parent_task: None,
            dependencies: Vec::new(),
            files: Vec::new(),
            sessions: Vec::new(),
            result: String::new(),
            created_at: now,
            updated_at: now,
            started_at: None,
            finished_at: None,
        });

        if let Some(title) = &input.title {
            let title = title.trim();
            if title.is_empty() {
                return Err(invalid("a task precisa de um título"));
            }
            if title.chars().count() > MAX_TITLE {
                return Err(invalid(format!("o título passa de {MAX_TITLE} caracteres")));
            }
            task.title = title.to_owned();
        }
        if task.title.is_empty() {
            return Err(invalid("a task precisa de um título"));
        }
        if let Some(description) = &input.description {
            task.description = text_within("a descrição", description, MAX_TEXT)?;
        }
        if let Some(result) = &input.result {
            task.result = text_within("o resultado", result, MAX_TEXT)?;
        }
        if let Some(priority) = input.priority {
            task.priority = priority;
        }
        if let Some(provider) = &input.provider {
            task.provider = Some(provider.clone());
        }
        if let Some(model) = &input.model {
            task.model = (!model.trim().is_empty()).then(|| model.trim().to_owned());
        }
        if let Some(files) = &input.files {
            let files: Vec<String> = files
                .iter()
                .map(|f| f.trim().to_owned())
                .filter(|f| !f.is_empty())
                .collect();
            if files.len() > MAX_FILES {
                return Err(invalid(format!("no máximo {MAX_FILES} arquivos por task")));
            }
            task.files = files;
        }

        let project_tasks = self.store.tasks_list(Some(&project_id));
        if let Some(parent) = &input.parent_task {
            self.check_parent(&task.id, parent, &project_tasks)?;
            task.parent_task = Some(parent.clone());
        }
        if let Some(dependencies) = &input.dependencies {
            task.dependencies = self.check_dependencies(&task.id, dependencies, &project_tasks)?;
        }

        task.updated_at = now;
        let created = existing.is_none();
        self.store
            .task_save(&task)
            .map_err(ProviderError::internal)?;
        self.record(
            if created {
                EventKind::TaskCreated
            } else {
                EventKind::TaskUpdated
            },
            &task,
            if created {
                "task criada".to_owned()
            } else {
                "task alterada".to_owned()
            },
            json!({}),
            origin,
        );
        Ok(task)
    }

    /// Moves a task to another state, if the move makes sense from where
    /// it is and nothing it depends on is pending.
    pub fn set_status(
        &self,
        id: &TaskId,
        status: TaskStatus,
        origin: CallOrigin,
    ) -> Result<Task, ProviderError> {
        let mut task = self.store.task(id.as_str()).ok_or_else(|| not_found(id))?;
        self.check_transition(&task, status)?;
        let from = task.status;
        let now = Utc::now();
        task.status = status;
        task.updated_at = now;
        match status {
            TaskStatus::InProgress if task.started_at.is_none() => task.started_at = Some(now),
            TaskStatus::Done | TaskStatus::Cancelled => task.finished_at = Some(now),
            TaskStatus::Todo => {
                // Reopening: the task is open work again.
                task.finished_at = None;
            }
            _ => {}
        }
        self.store
            .task_save(&task)
            .map_err(ProviderError::internal)?;
        let kind = match status {
            TaskStatus::InProgress => EventKind::TaskStarted,
            TaskStatus::Done => EventKind::TaskCompleted,
            _ => EventKind::TaskUpdated,
        };
        self.record(
            kind,
            &task,
            format!("{} → {}", status_label(from), status_label(status)),
            json!({"from": from, "to": status}),
            origin,
        );
        Ok(task)
    }

    /// Opens a session to work on the task: the project context is built
    /// from the task, the task goes as the first message, and the task
    /// moves to `IN_PROGRESS`.
    pub async fn start_session(
        &self,
        request: StartTaskSession,
        origin: CallOrigin,
    ) -> Result<StartedTask, ProviderError> {
        let opened = self.open_session(request, origin.clone()).await?;
        let (turn_id, send_error) = match self
            .sessions
            .send(&opened.session.id, opened.first_message, origin)
            .await
        {
            Ok(turn) => (Some(turn), None),
            Err(err) => (None, Some(err.message)),
        };
        Ok(StartedTask {
            task: opened.task,
            session: self.sessions.info(&opened.session.id)?,
            turn_id,
            send_error,
        })
    }

    /// The session of a task, opened but with nothing sent yet: what
    /// [`Self::start_session`] does before the first message, and what an
    /// agent needs in order to drive the turns itself (ADR-0015).
    pub async fn open_session(
        &self,
        request: StartTaskSession,
        origin: CallOrigin,
    ) -> Result<OpenedTask, ProviderError> {
        let task = self
            .store
            .task(request.task_id.as_str())
            .ok_or_else(|| not_found(&request.task_id))?;
        if task.status.is_final() {
            return Err(invalid(format!(
                "esta task está {}; reabra antes de trabalhar nela",
                status_label(task.status)
            )));
        }
        let project = self
            .store
            .project(&task.project_id)
            .ok_or_else(|| invalid("o projeto desta task não está registrado"))?;
        self.check_ready(&task)?;

        let session = self
            .sessions
            .start(
                StartRequest {
                    provider: request.provider.or_else(|| task.provider.clone()),
                    title: Some(clip(&task.title, 60)),
                    model: request.model.or_else(|| task.model.clone()),
                    instructions: None,
                    context: orchestrator_providers::ContextOptions {
                        enabled: Some(true),
                        budget: request.budget,
                        handoff_id: None,
                    },
                },
                PathBuf::from(&project.path),
                origin.clone(),
            )
            .await?;

        let mut task = task;
        task.sessions.push(session.id.clone());
        if task.provider.is_none() {
            task.provider = Some(session.provider.clone());
            task.model = session.model.clone();
        }
        task.updated_at = Utc::now();
        if task.status != TaskStatus::InProgress {
            task.status = TaskStatus::InProgress;
            if task.started_at.is_none() {
                task.started_at = Some(task.updated_at);
            }
        }
        self.store
            .task_save(&task)
            .map_err(ProviderError::internal)?;
        self.record(
            EventKind::TaskStarted,
            &task,
            format!("sessão aberta para a task · {}", session.provider),
            json!({"sessionId": session.id}),
            origin,
        );
        Ok(OpenedTask {
            first_message: first_message(&task),
            task,
            session,
        })
    }

    /// What a session opened for this task would receive.
    pub fn context(&self, id: &TaskId) -> Result<ContextPack, ProviderError> {
        let task = self.store.task(id.as_str()).ok_or_else(|| not_found(id))?;
        let project = self
            .store
            .project(&task.project_id)
            .ok_or_else(|| invalid("o projeto desta task não está registrado"))?;
        Ok(self.builder.build(&crate::builder::BuildRequest {
            project_path: PathBuf::from(&project.path),
            task: as_task_text(&task),
            handoff: None,
            budget: None,
            session_id: None,
            no_tools: false,
        }))
    }

    fn check_transition(&self, task: &Task, to: TaskStatus) -> Result<(), ProviderError> {
        if task.status == to {
            return Err(invalid(format!("a task já está {}", status_label(to))));
        }
        if !next_states(task.status).contains(&to) {
            return Err(invalid(format!(
                "uma task {} não pode ir para {}",
                status_label(task.status),
                status_label(to)
            )));
        }
        if to == TaskStatus::InProgress {
            self.check_ready(task)?;
        }
        Ok(())
    }

    /// A task only starts when everything it depends on is done.
    fn check_ready(&self, task: &Task) -> Result<(), ProviderError> {
        let pending: Vec<String> = task
            .dependencies
            .iter()
            .filter_map(|id| self.store.task(id.as_str()))
            .filter(|dep| dep.status != TaskStatus::Done)
            .map(|dep| format!("\"{}\" ({})", dep.title, status_label(dep.status)))
            .collect();
        if pending.is_empty() {
            return Ok(());
        }
        Err(invalid(format!("esta task espera: {}", pending.join("; "))))
    }

    fn check_parent(
        &self,
        id: &TaskId,
        parent: &TaskId,
        project_tasks: &[Task],
    ) -> Result<(), ProviderError> {
        if parent == id {
            return Err(invalid("uma task não pode ser subtask dela mesma"));
        }
        let by_id: HashMap<&str, &Task> =
            project_tasks.iter().map(|t| (t.id.as_str(), t)).collect();
        if !by_id.contains_key(parent.as_str()) {
            return Err(invalid("a task-mãe precisa ser do mesmo projeto"));
        }
        // Walking up from the parent must never reach this task.
        let mut seen = HashSet::new();
        let mut current = Some(parent.clone());
        while let Some(step) = current {
            if !seen.insert(step.clone()) {
                break;
            }
            if step == *id {
                return Err(invalid(
                    "isso criaria um ciclo: a task-mãe já está abaixo desta",
                ));
            }
            current = by_id.get(step.as_str()).and_then(|t| t.parent_task.clone());
        }
        Ok(())
    }

    fn check_dependencies(
        &self,
        id: &TaskId,
        wanted: &[TaskId],
        project_tasks: &[Task],
    ) -> Result<Vec<TaskId>, ProviderError> {
        let mut dependencies: Vec<TaskId> = Vec::new();
        for dependency in wanted {
            if dependency == id {
                return Err(invalid("uma task não pode depender dela mesma"));
            }
            if !project_tasks.iter().any(|t| t.id == *dependency) {
                return Err(invalid("a dependência precisa ser do mesmo projeto"));
            }
            if !dependencies.contains(dependency) {
                dependencies.push(dependency.clone());
            }
        }
        if dependencies.len() > MAX_DEPENDENCIES {
            return Err(invalid(format!(
                "no máximo {MAX_DEPENDENCIES} dependências por task"
            )));
        }
        // With the new edges in place, this task must not reach itself.
        let mut edges: HashMap<&str, Vec<&TaskId>> = HashMap::new();
        for task in project_tasks {
            if task.id == *id {
                continue;
            }
            edges.insert(task.id.as_str(), task.dependencies.iter().collect());
        }
        edges.insert(id.as_str(), dependencies.iter().collect());
        if reaches(id.as_str(), id.as_str(), &edges, &mut HashSet::new()) {
            return Err(invalid(
                "isso criaria um ciclo entre as dependências das tasks",
            ));
        }
        Ok(dependencies)
    }

    fn record(
        &self,
        kind: EventKind,
        task: &Task,
        what: String,
        extra: serde_json::Value,
        origin: CallOrigin,
    ) {
        let mut data = json!({
            "taskId": task.id,
            "projectId": task.project_id,
            "title": task.title,
            "status": task.status,
            "priority": task.priority,
            "provider": task.provider,
            "model": task.model,
            "dependencies": task.dependencies.len(),
        });
        if let (Some(data), Some(extra)) = (data.as_object_mut(), extra.as_object()) {
            for (key, value) in extra {
                data.insert(key.clone(), value.clone());
            }
        }
        self.sink.audit(AuditEvent::new(
            kind,
            origin,
            format!("{what} · {}", clip(&task.title, 100)),
            data,
        ));
    }
}

/// Dependencies of `task` that have not reached `DONE`.
fn pending_of(task: &Task, by_id: &HashMap<&str, &Task>) -> Vec<TaskRef> {
    task.dependencies
        .iter()
        .filter_map(|id| by_id.get(id.as_str()))
        .filter(|dep| dep.status != TaskStatus::Done)
        .map(|dep| TaskRef {
            id: dep.id.clone(),
            title: dep.title.clone(),
            status: dep.status,
        })
        .collect()
}

/// Depth-first walk: does `from` reach `target` through dependencies?
fn reaches(
    from: &str,
    target: &str,
    edges: &HashMap<&str, Vec<&TaskId>>,
    seen: &mut HashSet<String>,
) -> bool {
    for next in edges.get(from).into_iter().flatten() {
        if next.as_str() == target {
            return true;
        }
        if seen.insert(next.as_str().to_owned()) && reaches(next.as_str(), target, edges, seen) {
            return true;
        }
    }
    false
}

fn text_within(what: &str, text: &str, max: usize) -> Result<String, ProviderError> {
    let text = text.trim();
    if text.chars().count() > max {
        return Err(invalid(format!("{what} passa de {max} caracteres")));
    }
    Ok(text.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transitions_follow_the_adr() {
        use TaskStatus::*;
        assert_eq!(next_states(Todo), [InProgress, Blocked, Cancelled]);
        // Ended work is reopened, not edited into another state.
        assert_eq!(next_states(Done), [Todo]);
        assert_eq!(next_states(Cancelled), [Todo]);
        // Review goes forward or back, never to blocked.
        assert!(!next_states(Review).contains(&Blocked));
    }

    #[test]
    fn a_task_reads_as_goal_detail_and_files() {
        let mut task = crate::task::tests::task("Aplicar retentativas");
        task.description = "Usar backoff exponencial.".into();
        task.files = vec!["src/pay.ts".into()];
        let text = as_task_text(&task);
        assert_eq!(
            text,
            "Aplicar retentativas\n\nUsar backoff exponencial.\n\nArquivos: src/pay.ts"
        );
        // The first message carries the task and says where to start.
        assert!(first_message(&task).contains("Aplicar retentativas"));
        assert!(first_message(&task).contains("antes de mudar qualquer arquivo"));
    }

    #[test]
    fn a_cycle_is_refused_however_long() {
        let a = TaskId::new();
        let b = TaskId::new();
        let c = TaskId::new();
        let mut edges: HashMap<&str, Vec<&TaskId>> = HashMap::new();
        edges.insert(b.as_str(), vec![&c]);
        edges.insert(c.as_str(), vec![&a]);
        edges.insert(a.as_str(), vec![&b]);
        assert!(reaches(a.as_str(), a.as_str(), &edges, &mut HashSet::new()));
        // Without the last edge there is no way back to `a`.
        edges.insert(c.as_str(), Vec::new());
        assert!(!reaches(
            a.as_str(),
            a.as_str(),
            &edges,
            &mut HashSet::new()
        ));
    }

    pub(super) fn task(title: &str) -> Task {
        Task {
            id: TaskId::new(),
            project_id: "p".into(),
            title: title.into(),
            description: String::new(),
            status: TaskStatus::Todo,
            priority: TaskPriority::Normal,
            provider: None,
            model: None,
            agent: None,
            parent_task: None,
            dependencies: Vec::new(),
            files: Vec::new(),
            sessions: Vec::new(),
            result: String::new(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            started_at: None,
            finished_at: None,
        }
    }
}
