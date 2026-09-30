//! Tauri commands of the Agent Manager (ADR-0015). They forward to the
//! agents crate; the UI acts as the user.

use crate::AppState;
use orchestrator_agents::{AgentSettings, AgentView, StartAgent};
use orchestrator_core::{Agent, AgentId, FileLock};
use orchestrator_providers::ProviderError;
use tauri::State;

fn project_of(state: &AppState, project_id: Option<String>) -> Option<String> {
    project_id.or_else(|| state.store.current_project().map(|p| p.id))
}

/// Agents of a project (the open one with `None`), in board order.
#[tauri::command]
pub fn agents_list(state: State<'_, AppState>, project_id: Option<String>) -> Vec<AgentView> {
    let project = project_of(&state, project_id);
    state.agents.list(project.as_deref())
}

#[tauri::command]
pub fn agent_get(state: State<'_, AppState>, id: String) -> Option<AgentView> {
    state.agents.get(&AgentId::from(id))
}

/// Queues an agent for a task; it starts when there is a slot and its
/// files are free.
#[tauri::command]
pub fn agent_start(
    state: State<'_, AppState>,
    request: StartAgent,
) -> Result<Agent, ProviderError> {
    state.agents.start(request)
}

/// `Cancel` (master document, section 11).
#[tauri::command]
pub async fn agent_stop(state: State<'_, AppState>, id: String) -> Result<Agent, ProviderError> {
    state
        .agents
        .stop(&AgentId::from(id), orchestrator_core::CallOrigin::User)
        .await
}

/// `Stop All Agents` (section 11): how many were asked to stop.
#[tauri::command]
pub async fn agents_stop_all(
    state: State<'_, AppState>,
    project_id: Option<String>,
) -> Result<usize, ProviderError> {
    let project = project_of(&state, project_id);
    state
        .agents
        .stop_all(project.as_deref(), orchestrator_core::CallOrigin::User)
        .await
}

/// Files held by agents right now, for the panel.
#[tauri::command]
pub fn agent_locks(state: State<'_, AppState>, project_id: Option<String>) -> Vec<FileLock> {
    let project = project_of(&state, project_id);
    state.agents.locks(project.as_deref())
}

#[tauri::command]
pub fn agent_settings_get(state: State<'_, AppState>) -> AgentSettings {
    state.agents.settings()
}

/// Async for the same reason as `agent_start`: a wider ceiling may release
/// what is queued.
#[tauri::command]
pub async fn agent_settings_save(
    state: State<'_, AppState>,
    settings: AgentSettings,
) -> Result<AgentSettings, ProviderError> {
    state.agents.save_settings(settings)
}
