//! Tauri commands of the Task Manager (ADR-0014). They forward to the
//! engine; the UI acts as the user.

use crate::AppState;
use orchestrator_core::{CallOrigin, Task, TaskId, TaskInput, TaskStatus};
use orchestrator_engine::{ContextPack, StartTaskSession, StartedTask, TaskView};
use orchestrator_providers::ProviderError;
use tauri::State;

/// Tasks of a project (the open one with `None`), in panel order.
#[tauri::command]
pub fn tasks_list(state: State<'_, AppState>, project_id: Option<String>) -> Vec<TaskView> {
    let project = project_id.or_else(|| state.store.current_project().map(|p| p.id));
    state.tasks.list(project.as_deref())
}

#[tauri::command]
pub fn task_get(state: State<'_, AppState>, id: String) -> Option<TaskView> {
    state.tasks.get(&TaskId::from(id))
}

/// Creates a task (no `id`) or edits one.
#[tauri::command]
pub fn task_save(state: State<'_, AppState>, input: TaskInput) -> Result<Task, ProviderError> {
    let mut input = input;
    if input.id.is_none() && input.project_id.is_none() {
        input.project_id = state.store.current_project().map(|p| p.id);
    }
    state.tasks.save(input, CallOrigin::User)
}

/// Moves a task to another state (`TASK_STARTED`, `TASK_COMPLETED` or
/// `TASK_UPDATED`).
#[tauri::command]
pub fn task_status(
    state: State<'_, AppState>,
    id: String,
    status: TaskStatus,
) -> Result<Task, ProviderError> {
    state
        .tasks
        .set_status(&TaskId::from(id), status, CallOrigin::User)
}

/// Opens a session to work on the task, with the context built from it.
#[tauri::command]
pub async fn task_start_session(
    state: State<'_, AppState>,
    request: StartTaskSession,
) -> Result<StartedTask, ProviderError> {
    state.tasks.start_session(request, CallOrigin::User).await
}

/// What a session opened for this task would receive (nothing is sent).
#[tauri::command]
pub async fn task_context(
    state: State<'_, AppState>,
    id: String,
) -> Result<ContextPack, ProviderError> {
    let tasks = state.tasks.clone();
    // SQLite and `git status` block: off the async threads.
    tauri::async_runtime::spawn_blocking(move || tasks.context(&TaskId::from(id)))
        .await
        .map_err(|e| ProviderError::internal(e.to_string()))?
}
