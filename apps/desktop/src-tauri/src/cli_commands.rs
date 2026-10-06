//! Tauri commands of the subscriptions through CLIs (ADR-0021): each CLI's
//! state (installed, version, logged in) and its settings. Installing and
//! logging in run in a terminal of the app, where the user sees and
//! answers everything.

use crate::AppState;
use orchestrator_provider_cli::{CliKind, CliSettings, CliStatus};
use tauri::State;

#[tauri::command]
pub async fn clis_list(state: State<'_, AppState>) -> Result<Vec<CliStatus>, String> {
    Ok(state.clis.view().await)
}

#[tauri::command]
pub async fn cli_save(
    state: State<'_, AppState>,
    kind: CliKind,
    settings: CliSettings,
) -> Result<Vec<CliStatus>, String> {
    state.clis.save(kind, settings)?;
    Ok(state.clis.view().await)
}
