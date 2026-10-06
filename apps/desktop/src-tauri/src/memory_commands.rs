//! Tauri commands of the history, projects and project memory (ADR-0012).
//! They forward to the `MemoryStore`; writes are published as audit events
//! with origin `user`.

use crate::AppState;
use orchestrator_core::{AuditEvent, CallOrigin, EventKind, EventSink};
use orchestrator_memory::{
    Decision, DecisionInput, HistoryPage, HistoryQuery, MemoryEntry, MemoryInput, MemoryOverview,
    Project, ProjectLink, RecentImport, SearchHit,
};
use serde_json::json;
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

/// Projects open side by side in the app, in sidebar order (ADR-0023).
#[tauri::command]
pub fn projects_open(state: State<'_, AppState>) -> Vec<Project> {
    state.store.projects_open()
}

/// Closes a project in the app: it leaves the sidebar, its sessions,
/// memory and history stay. Returns the project to show next, if any.
#[tauri::command]
pub fn project_close(state: State<'_, AppState>, id: String) -> Result<Option<Project>, String> {
    state.store.project_close(&id)
}

/// Orders the open projects as the sidebar shows them.
#[tauri::command]
pub fn projects_reorder(state: State<'_, AppState>, ids: Vec<String>) -> Result<(), String> {
    state.store.projects_reorder(&ids)
}

/// The projects that work with `id` (ADR-0023).
#[tauri::command]
pub fn project_links(state: State<'_, AppState>, id: String) -> Vec<ProjectLink> {
    state.store.project_links(&id)
}

fn link_event(state: &AppState, id: &str, other: &str, note: &str, linked: bool) {
    let name = |id: &str| {
        state
            .store
            .project(id)
            .map(|p| p.name)
            .unwrap_or_else(|| id.to_owned())
    };
    let summary = if linked {
        format!("projetos relacionados: {} ↔ {}", name(id), name(other))
    } else {
        format!("relação desfeita: {} ↔ {}", name(id), name(other))
    };
    state.sink.audit(AuditEvent::new(
        EventKind::ProjectLinked,
        CallOrigin::User,
        summary,
        json!({
            "projectId": id,
            "otherProjectId": other,
            "note": note,
            "linked": linked,
        }),
    ));
}

/// Links two projects, or changes the note of their link
/// (`PROJECT_LINKED`). Returns the links of `id`.
#[tauri::command]
pub fn project_link(
    state: State<'_, AppState>,
    id: String,
    other: String,
    note: Option<String>,
) -> Result<Vec<ProjectLink>, String> {
    let link = state
        .store
        .project_link(&id, &other, note.as_deref().unwrap_or(""))?;
    link_event(&state, &id, &other, &link.note, true);
    Ok(state.store.project_links(&id))
}

/// Unlinks two projects; their data stays (`PROJECT_LINKED`, linked
/// false). Returns the links of `id`.
#[tauri::command]
pub fn project_unlink(
    state: State<'_, AppState>,
    id: String,
    other: String,
) -> Result<Vec<ProjectLink>, String> {
    if state.store.project_unlink(&id, &other)? {
        link_event(&state, &id, &other, "", false);
    }
    Ok(state.store.project_links(&id))
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
