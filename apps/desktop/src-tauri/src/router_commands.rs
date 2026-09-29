//! Tauri commands of the model router and the Council (ADR-0011). They only
//! forward to the `RouterService`, which records the history. The UI acts
//! as the user; in Full mode the Council opens sessions as itself.

use crate::AppState;
use orchestrator_core::CallOrigin;
use orchestrator_providers::ProviderError;
use orchestrator_router::{
    profiles, ActivityProfile, CouncilSettings, DeliberateRequest, Deliberation, Recommendation,
    RouteRequest, RouteStart, RouteStarted, RunOutcome, MAX_MEMBERS,
};
use serde::Serialize;
use tauri::State;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CouncilView {
    pub settings: CouncilSettings,
    pub activities: &'static [ActivityProfile],
    pub max_members: usize,
    /// Problem loading `council.json`, if any.
    pub warning: Option<String>,
}

/// The router's ranking for a task (no tokens).
#[tauri::command]
pub async fn router_recommend(
    state: State<'_, AppState>,
    request: RouteRequest,
) -> Result<Recommendation, ProviderError> {
    Ok(state.router.recommend(&request).await)
}

#[tauri::command]
pub fn council_get(state: State<'_, AppState>) -> CouncilView {
    CouncilView {
        settings: state.router.settings(),
        activities: profiles(),
        max_members: MAX_MEMBERS,
        warning: state.router_warning.clone(),
    }
}

/// Validates and stores the settings (`COUNCIL_CONFIGURED`).
#[tauri::command]
pub fn council_save(
    state: State<'_, AppState>,
    settings: CouncilSettings,
) -> Result<CouncilSettings, ProviderError> {
    state.router.save_settings(settings, CallOrigin::User)
}

/// Deliberates; in Full mode the Council also opens the session on the open
/// project and sends the task.
#[tauri::command]
pub async fn council_run(
    state: State<'_, AppState>,
    request: DeliberateRequest,
) -> Result<RunOutcome, ProviderError> {
    state
        .router
        .run(&state.sessions, &request, state.runtime.base_dir())
        .await
}

/// Recent deliberations, newest first.
#[tauri::command]
pub fn council_history(state: State<'_, AppState>) -> Vec<Deliberation> {
    state.router.history()
}

/// Opens a session with the model the user approved or picked
/// (`ROUTE_DECIDED`).
#[tauri::command]
pub async fn route_start_session(
    state: State<'_, AppState>,
    request: RouteStart,
) -> Result<RouteStarted, ProviderError> {
    state
        .router
        .start_session(
            &state.sessions,
            request,
            state.runtime.base_dir(),
            CallOrigin::User,
        )
        .await
}
