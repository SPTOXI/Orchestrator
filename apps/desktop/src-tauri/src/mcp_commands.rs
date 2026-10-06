//! Tauri commands of the MCP servers (ADR-0021): list, add or edit,
//! import from Claude/Cursor JSON, restart, remove, and turn tools off.
//! Connecting runs in the background; the UI polls the list.

use crate::AppState;
use orchestrator_mcp::{ServerConfig, ServerView};
use tauri::State;

#[tauri::command]
pub fn mcp_list(state: State<'_, AppState>) -> Vec<ServerView> {
    state.mcp.view()
}

/// Saves a server and (re)connects it in the background.
#[tauri::command]
pub fn mcp_save(
    state: State<'_, AppState>,
    server: ServerConfig,
    previous_id: Option<String>,
) -> Result<Vec<ServerView>, String> {
    let id = server.id.clone();
    state.mcp.save(server, previous_id.as_deref())?;
    let mcp = state.mcp.clone();
    tauri::async_runtime::spawn(async move { mcp.restart(&id).await });
    Ok(state.mcp.view())
}

#[tauri::command]
pub fn mcp_import(state: State<'_, AppState>, text: String) -> Result<Vec<String>, String> {
    let added = state.mcp.import(&text)?;
    for id in &added {
        let mcp = state.mcp.clone();
        let id = id.clone();
        tauri::async_runtime::spawn(async move { mcp.restart(&id).await });
    }
    Ok(added)
}

#[tauri::command]
pub fn mcp_restart(state: State<'_, AppState>, id: String) {
    let mcp = state.mcp.clone();
    tauri::async_runtime::spawn(async move { mcp.restart(&id).await });
}

#[tauri::command]
pub fn mcp_delete(state: State<'_, AppState>, id: String) -> Result<Vec<ServerView>, String> {
    state.mcp.delete(&id)?;
    Ok(state.mcp.view())
}

/// Turns one of a server's tools off (or back on) for the AIs.
#[tauri::command]
pub fn mcp_set_tool(
    state: State<'_, AppState>,
    id: String,
    tool: String,
    enabled: bool,
) -> Result<Vec<ServerView>, String> {
    let mut server = state
        .mcp
        .servers()
        .into_iter()
        .find(|s| s.id == id)
        .ok_or_else(|| format!("servidor {id} não encontrado"))?;
    server.disabled_tools.retain(|t| t != &tool);
    if !enabled {
        server.disabled_tools.push(tool);
    }
    mcp_save(state, server, None)
}
