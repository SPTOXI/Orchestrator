//! Tauri commands of the autonomy (ADR-0016): the mode, the rules, the
//! requests for authorization and the pause. The UI acts as the user; no
//! AI tool reaches any of this.

use crate::AppState;
use orchestrator_core::{ApprovalAnswer, ApprovalId, AutonomyMode, CallOrigin, PolicyRule};
use orchestrator_engine::{ApprovalView, AutonomyOverview, Trial};
use serde_json::Value;
use tauri::State;

fn project_of(state: &AppState, project_id: Option<String>) -> Option<String> {
    project_id.or_else(|| state.store.current_project().map(|p| p.id))
}

/// The autonomy of a project (the open one with `None`).
#[tauri::command]
pub fn autonomy_get(state: State<'_, AppState>, project_id: Option<String>) -> AutonomyOverview {
    let project = project_of(&state, project_id);
    state.autonomy.overview(project.as_deref())
}

/// The mode of a project; `None` goes back to the default. Without a
/// project, the default mode itself.
#[tauri::command]
pub fn autonomy_set_mode(
    state: State<'_, AppState>,
    project_id: Option<String>,
    mode: Option<AutonomyMode>,
) -> Result<AutonomyOverview, String> {
    let project = project_of(&state, project_id);
    match (&project, mode) {
        (Some(id), mode) => {
            state
                .autonomy
                .set_project_mode(id, mode, CallOrigin::User)?;
        }
        (None, Some(mode)) => state.autonomy.set_default_mode(mode, CallOrigin::User)?,
        (None, None) => return Err("escolha um modo".to_owned()),
    }
    Ok(state.autonomy.overview(project.as_deref()))
}

/// Mode of the projects the user has not chosen one for.
#[tauri::command]
pub fn autonomy_set_default(
    state: State<'_, AppState>,
    project_id: Option<String>,
    mode: AutonomyMode,
) -> Result<AutonomyOverview, String> {
    state.autonomy.set_default_mode(mode, CallOrigin::User)?;
    let project = project_of(&state, project_id);
    Ok(state.autonomy.overview(project.as_deref()))
}

/// The user's rules (Autonomous mode).
#[tauri::command]
pub fn autonomy_save_rules(
    state: State<'_, AppState>,
    project_id: Option<String>,
    rules: Vec<PolicyRule>,
) -> Result<AutonomyOverview, String> {
    state.autonomy.save_rules(&rules, CallOrigin::User)?;
    let project = project_of(&state, project_id);
    Ok(state.autonomy.overview(project.as_deref()))
}

#[tauri::command]
pub fn autonomy_reset_rules(
    state: State<'_, AppState>,
    project_id: Option<String>,
) -> Result<AutonomyOverview, String> {
    state.autonomy.reset_rules(CallOrigin::User)?;
    let project = project_of(&state, project_id);
    Ok(state.autonomy.overview(project.as_deref()))
}

/// "Experimentar": which rule decides a call, with rules not saved yet.
#[tauri::command]
pub fn autonomy_try(
    state: State<'_, AppState>,
    project_id: Option<String>,
    mode: Option<AutonomyMode>,
    rules: Option<Vec<PolicyRule>>,
    tool: String,
    args: Option<Value>,
) -> Result<Trial, String> {
    let project = project_of(&state, project_id);
    state.autonomy.trial(
        project.as_deref(),
        mode,
        rules,
        &tool,
        &args.unwrap_or(Value::Null),
    )
}

/// Requests waiting for the user, oldest first (every project).
#[tauri::command]
pub fn approvals_pending(state: State<'_, AppState>) -> Vec<ApprovalView> {
    state.autonomy.pending()
}

#[tauri::command]
pub fn approval_answer(
    state: State<'_, AppState>,
    id: String,
    answer: ApprovalAnswer,
    note: Option<String>,
) -> Result<(), String> {
    state
        .autonomy
        .answer(
            &ApprovalId::from(id),
            answer,
            note.as_deref(),
            CallOrigin::User,
        )
        .map(|_| ())
}

/// Takes back something allowed for a session.
#[tauri::command]
pub fn autonomy_revoke(state: State<'_, AppState>, grant_id: String) -> bool {
    state.autonomy.revoke(&grant_id)
}

/// `Pause` for every AI (section 11).
#[tauri::command]
pub fn execution_pause(state: State<'_, AppState>) -> bool {
    state.agents.pause_all(CallOrigin::User)
}

/// Async: resuming may start what is queued, on the async runtime.
#[tauri::command]
pub async fn execution_resume(state: State<'_, AppState>) -> Result<bool, String> {
    Ok(state.agents.resume_all(CallOrigin::User))
}
