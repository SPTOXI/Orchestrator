//! Tauri commands of the development rules and skills (ADR-0021). The UI
//! edits the Orchestrator's own; a project's files are only read.

use crate::AppState;
use orchestrator_engine::{GuidanceSettings, GuidanceView, SkillDoc, SkillInfo, SkillInput};
use std::path::PathBuf;
use tauri::State;

/// The open project, whose files and skills are shown.
fn project(state: &AppState) -> Option<PathBuf> {
    state.store.current_project().map(|p| PathBuf::from(p.path))
}

#[tauri::command]
pub async fn guidance_get(state: State<'_, AppState>) -> Result<GuidanceView, String> {
    let guidance = state.guidance.clone();
    let project = project(&state);
    tauri::async_runtime::spawn_blocking(move || guidance.view(project.as_deref()))
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn guidance_settings_save(
    state: State<'_, AppState>,
    settings: GuidanceSettings,
) -> Result<GuidanceSettings, String> {
    state.guidance.save_settings(settings)
}

/// The user's rules for every project.
#[tauri::command]
pub fn rules_save(state: State<'_, AppState>, text: String) -> Result<(), String> {
    state.guidance.save_user_rules(&text)
}

#[tauri::command]
pub fn skill_get(state: State<'_, AppState>, name: String) -> Result<SkillDoc, String> {
    state
        .guidance
        .skill(&name, project(&state).as_deref())
        .ok_or_else(|| format!("skill {name} não encontrada"))
}

#[tauri::command]
pub fn skill_save(state: State<'_, AppState>, skill: SkillInput) -> Result<SkillInfo, String> {
    state.guidance.save_skill(&skill)
}

#[tauri::command]
pub fn skill_delete(state: State<'_, AppState>, name: String) -> Result<(), String> {
    state.guidance.delete_skill(&name)
}

#[tauri::command]
pub fn skill_set_enabled(
    state: State<'_, AppState>,
    name: String,
    enabled: bool,
) -> Result<GuidanceSettings, String> {
    state.guidance.set_skill_enabled(&name, enabled)
}
