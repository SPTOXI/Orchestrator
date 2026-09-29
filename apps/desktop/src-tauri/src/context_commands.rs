//! Tauri commands of the Context Builder and of handoffs between AIs
//! (ADR-0013). They forward to the engine; the UI acts as the user.

use crate::AppState;
use orchestrator_core::{CallOrigin, Handoff, HandoffId};
use orchestrator_engine::{
    ContextPack, ContextSettings, CreateRequest, HandoffDraft, PrepareRequest, PreviewRequest,
    StartHandoff, StartedHandoff, DEFAULT_BUDGET, MAX_BUDGET, MIN_BUDGET,
};
use orchestrator_providers::ProviderError;
use serde::Serialize;
use tauri::State;

/// What a session would receive for a task or handoff (nothing is sent).
#[tauri::command]
pub async fn context_preview(
    state: State<'_, AppState>,
    request: Option<PreviewRequest>,
) -> Result<ContextPack, String> {
    let builder = state.builder.clone();
    let open = state.runtime.base_dir();
    let request = request.unwrap_or_default();
    // SQLite and `git status` block: off the async threads.
    tauri::async_runtime::spawn_blocking(move || builder.preview(request, &open))
        .await
        .map_err(|e| e.to_string())?
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextSettingsView {
    pub settings: ContextSettings,
    pub min_budget: u32,
    pub max_budget: u32,
    pub default_budget: u32,
    /// Problem reading `context.json`, if any.
    pub warning: Option<String>,
}

#[tauri::command]
pub fn context_settings_get(state: State<'_, AppState>) -> ContextSettingsView {
    ContextSettingsView {
        settings: state.builder.settings(),
        min_budget: MIN_BUDGET,
        max_budget: MAX_BUDGET,
        default_budget: DEFAULT_BUDGET,
        warning: state.context_warning.clone(),
    }
}

#[tauri::command]
pub fn context_settings_save(
    state: State<'_, AppState>,
    settings: ContextSettings,
) -> Result<ContextSettings, String> {
    state.builder.save_settings(settings)
}

/// A handoff draft from a session (asks its AI when `askAgent`).
#[tauri::command]
pub async fn handoff_prepare(
    state: State<'_, AppState>,
    request: PrepareRequest,
) -> Result<HandoffDraft, ProviderError> {
    state.handoffs.prepare(request).await
}

/// Saves a reviewed packet (`HANDOFF_CREATED`).
#[tauri::command]
pub fn handoff_create(
    state: State<'_, AppState>,
    request: CreateRequest,
) -> Result<Handoff, ProviderError> {
    state.handoffs.create(request, CallOrigin::User)
}

/// Another AI takes over in a new session (`HANDOFF_ACCEPTED`).
#[tauri::command]
pub async fn handoff_start(
    state: State<'_, AppState>,
    request: StartHandoff,
) -> Result<StartedHandoff, ProviderError> {
    state.handoffs.start(request, CallOrigin::User).await
}

/// Handoffs of a project (all with `None`), newest first.
#[tauri::command]
pub fn handoffs_list(state: State<'_, AppState>, project_id: Option<String>) -> Vec<Handoff> {
    state.handoffs.list(project_id.as_deref())
}

#[tauri::command]
pub fn handoff_get(state: State<'_, AppState>, id: String) -> Option<Handoff> {
    state.handoffs.get(&HandoffId::from(id))
}
