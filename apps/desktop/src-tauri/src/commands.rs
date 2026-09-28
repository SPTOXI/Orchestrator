//! Tauri commands (IPC surface). See docs/ipc.md.

use crate::AppState;
use orchestrator_core::{AuditEvent, CallOrigin, TerminalId, ToolCall, ToolResult, ToolSpec};
use orchestrator_runtime::ToolRuntime;
use serde::Serialize;
use serde_json::Value;
use tauri::State;

/// Single gateway for every tool (ADR-0003). The UI always calls as the user.
#[tauri::command]
pub async fn runtime_invoke(
    state: State<'_, AppState>,
    tool: String,
    args: Option<Value>,
) -> Result<ToolResult, String> {
    let call = ToolCall::new(tool, args.unwrap_or(Value::Null), CallOrigin::User);
    Ok(state.runtime.invoke(call).await)
}

/// Native folder picker. A UI interaction only: the chosen folder is then
/// opened through the audited `project.open` tool.
#[tauri::command]
pub async fn pick_folder(app: tauri::AppHandle) -> Result<Option<String>, String> {
    use tauri_plugin_dialog::DialogExt;
    let picked =
        tauri::async_runtime::spawn_blocking(move || app.dialog().file().blocking_pick_folder())
            .await
            .map_err(|e| e.to_string())?;
    picked
        .map(|path| {
            path.into_path()
                .map(|p| p.display().to_string())
                .map_err(|e| e.to_string())
        })
        .transpose()
}

#[tauri::command]
pub fn runtime_tools() -> Vec<ToolSpec> {
    ToolRuntime::catalog().to_vec()
}

/// Human keystrokes for an open terminal (streaming channel, not audited per
/// key; ADR-0003).
#[tauri::command]
pub async fn terminal_input(
    state: State<'_, AppState>,
    id: String,
    data: String,
) -> Result<(), String> {
    let runtime = state.runtime.clone();
    tauri::async_runtime::spawn_blocking(move || {
        runtime.terminal_input(&TerminalId::from(id), data.as_bytes())
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn terminal_resize(
    state: State<'_, AppState>,
    id: String,
    cols: u16,
    rows: u16,
) -> Result<(), String> {
    state
        .runtime
        .terminal_resize(&TerminalId::from(id), cols, rows)
        .map_err(|e| e.to_string())
}

/// Most recent audit events, oldest first.
#[tauri::command]
pub fn history_recent(state: State<'_, AppState>, limit: Option<usize>) -> Vec<AuditEvent> {
    state.sink.recent(limit.unwrap_or(200))
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppInfo {
    pub version: &'static str,
    pub os: &'static str,
    pub arch: &'static str,
    /// Base directory of the runtime: the open project, or the home directory.
    pub base_dir: String,
    pub data_dir: String,
    pub audit_log: String,
    pub default_shell: String,
}

#[tauri::command]
pub fn app_info(state: State<'_, AppState>) -> AppInfo {
    AppInfo {
        version: env!("CARGO_PKG_VERSION"),
        os: std::env::consts::OS,
        arch: std::env::consts::ARCH,
        base_dir: state.runtime.base_dir().display().to_string(),
        data_dir: state.data_dir.display().to_string(),
        audit_log: state.sink.audit_log_path(),
        default_shell: state.runtime.shells().default_id().to_owned(),
    }
}
