//! Tauri commands of the AI Provider Layer (ADR-0009). They only forward to
//! the `ProviderRegistry` / `SessionManager`, which record the history.
//! The UI always acts as the user.

use crate::AppState;
use async_trait::async_trait;
use orchestrator_core::{
    CallOrigin, ProviderId, SessionId, SessionInfo, ToolCall, ToolResult, ToolSpec, TurnId,
};
use orchestrator_providers::{
    ProviderError, ProviderInfo, ProviderStatus, SessionSnapshot, StartRequest, ToolExecutor,
};
use orchestrator_runtime::ToolRuntime;
use serde::Serialize;
use tauri::State;

/// Provider tool calls run through the same audited `invoke` as the UI's.
pub struct RuntimeTools(pub ToolRuntime);

#[async_trait]
impl ToolExecutor for RuntimeTools {
    fn catalog(&self) -> Vec<ToolSpec> {
        ToolRuntime::catalog().to_vec()
    }

    async fn execute(&self, call: ToolCall) -> ToolResult {
        self.0.invoke(call).await
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProvidersView {
    pub providers: Vec<ProviderInfo>,
    pub active: Option<ProviderId>,
}

fn providers_view(state: &AppState) -> ProvidersView {
    let registry = state.sessions.registry();
    ProvidersView {
        providers: registry.list(),
        active: registry.active_id(),
    }
}

#[tauri::command]
pub fn providers_list(state: State<'_, AppState>) -> ProvidersView {
    providers_view(&state)
}

#[tauri::command]
pub async fn provider_inspect(
    state: State<'_, AppState>,
    id: String,
) -> Result<ProviderStatus, ProviderError> {
    state
        .sessions
        .registry()
        .inspect(&ProviderId::from(id))
        .await
}

/// Changes the active provider (`PROVIDER_SWITCHED`).
#[tauri::command]
pub fn provider_select(
    state: State<'_, AppState>,
    id: String,
) -> Result<ProvidersView, ProviderError> {
    state
        .sessions
        .registry()
        .select(&ProviderId::from(id), CallOrigin::User)?;
    Ok(providers_view(&state))
}

#[tauri::command]
pub fn sessions_list(state: State<'_, AppState>) -> Vec<SessionInfo> {
    state.sessions.list()
}

/// Opens a session on the open project (the runtime base directory).
#[tauri::command]
pub async fn session_start(
    state: State<'_, AppState>,
    request: Option<StartRequest>,
) -> Result<SessionInfo, ProviderError> {
    state
        .sessions
        .start(
            request.unwrap_or_default(),
            state.runtime.base_dir(),
            CallOrigin::User,
        )
        .await
}

/// Session info plus transcript (merge with live events by `seq`).
#[tauri::command]
pub fn session_get(
    state: State<'_, AppState>,
    id: String,
) -> Result<SessionSnapshot, ProviderError> {
    state.sessions.snapshot(&SessionId::from(id))
}

/// Starts a streaming turn; progress arrives as `session` stream events.
#[tauri::command]
pub async fn session_send(
    state: State<'_, AppState>,
    id: String,
    input: String,
) -> Result<TurnId, ProviderError> {
    state
        .sessions
        .send(&SessionId::from(id), input, CallOrigin::User)
        .await
}

#[tauri::command]
pub async fn session_cancel(
    state: State<'_, AppState>,
    id: String,
) -> Result<SessionInfo, ProviderError> {
    state.sessions.cancel(&SessionId::from(id)).await
}

#[tauri::command]
pub async fn session_close(
    state: State<'_, AppState>,
    id: String,
) -> Result<SessionInfo, ProviderError> {
    state
        .sessions
        .close(&SessionId::from(id), CallOrigin::User)
        .await
}

#[tauri::command]
pub async fn session_resume(
    state: State<'_, AppState>,
    id: String,
) -> Result<SessionInfo, ProviderError> {
    state
        .sessions
        .resume(&SessionId::from(id), CallOrigin::User)
        .await
}

/// Opens a subagent session (`spawnAgent`).
#[tauri::command]
pub async fn session_spawn(
    state: State<'_, AppState>,
    parent_id: String,
    request: Option<StartRequest>,
) -> Result<SessionInfo, ProviderError> {
    state
        .sessions
        .spawn(
            &SessionId::from(parent_id),
            request.unwrap_or_default(),
            CallOrigin::User,
        )
        .await
}
