//! Tauri commands of the history, projects and project memory (ADR-0012).
//! They forward to the `MemoryStore`; writes are published as audit events
//! with origin `user`.

use crate::AppState;
use orchestrator_core::{CallOrigin, EventSink};
use orchestrator_memory::{
    Decision, DecisionInput, HistoryPage, HistoryQuery, MemoryEntry, MemoryInput, MemoryOverview,
    Project, RecentImport, SearchHit,
};
use tauri::State;

/// A page of history (filters and cursor in `query`).
#[tauri::command]
pub fn history_query(
    state: State<'_, AppState>,
    query: Option<HistoryQuery>,
) -> Result<HistoryPage, String> {
    state.store.history(&query.unwrap_or_default())
}

#[tauri::command]
pub fn projects_recent(state: State<'_, AppState>, limit: Option<usize>) -> Vec<Project> {
    state.store.projects_recent(limit.unwrap_or(8))
}

/// The project open in the app, as registered in the database.
#[tauri::command]
pub fn project_current(state: State<'_, AppState>) -> Option<Project> {
    state.store.current_project()
}

/// Takes a project off the recent list (memory and history stay).
#[tauri::command]
pub fn project_forget(state: State<'_, AppState>, id: String) -> Result<(), String> {
    state.store.project_forget(&id)
}

/// Imports the recent list the UI kept in `localStorage` before Phase 6.
#[tauri::command]
pub fn projects_import_recent(
    state: State<'_, AppState>,
    list: Vec<RecentImport>,
) -> Result<usize, String> {
    state.store.projects_import_recent(&list)
}

/// L1 and totals of a project (default: the open one).
#[tauri::command]
pub fn memory_overview(
    state: State<'_, AppState>,
    project_id: Option<String>,
) -> Result<Option<MemoryOverview>, String> {
    let id = match project_id.or_else(|| state.store.current_project().map(|p| p.id)) {
        Some(id) => id,
        None => return Ok(None),
    };
    state.store.overview(&id).map(Some)
}

#[tauri::command]
pub fn memory_list(
    state: State<'_, AppState>,
    project_id: String,
) -> Result<Vec<MemoryEntry>, String> {
    state.store.memory_list(&project_id)
}

/// Creates or updates an entry (`MEMORY_SAVED`).
#[tauri::command]
pub fn memory_save(state: State<'_, AppState>, input: MemoryInput) -> Result<MemoryEntry, String> {
    let (entry, event) = state.store.memory_save(input, &CallOrigin::User)?;
    state.sink.audit(event);
    Ok(entry)
}

/// Deletes an entry (`MEMORY_REMOVED`).
#[tauri::command]
pub fn memory_delete(state: State<'_, AppState>, id: String) -> Result<(), String> {
    let event = state.store.memory_delete(&id, &CallOrigin::User)?;
    state.sink.audit(event);
    Ok(())
}

/// L3 search in a project.
#[tauri::command]
pub fn memory_search(
    state: State<'_, AppState>,
    project_id: String,
    text: String,
    limit: Option<usize>,
) -> Result<Vec<SearchHit>, String> {
    state.store.search(&project_id, &text, limit.unwrap_or(30))
}

#[tauri::command]
pub fn decisions_list(
    state: State<'_, AppState>,
    project_id: String,
) -> Result<Vec<Decision>, String> {
    state.store.decisions_list(&project_id)
}

/// Records or updates a decision (`DECISION_SAVED`).
#[tauri::command]
pub fn decision_save(state: State<'_, AppState>, input: DecisionInput) -> Result<Decision, String> {
    let (decision, event) = state.store.decision_save(input, &CallOrigin::User)?;
    state.sink.audit(event);
    Ok(decision)
}
