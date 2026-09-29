//! Orchestrator desktop shell.
//!
//! This crate is only an IPC bridge (ADR-0001, ADR-0003, ADR-0009,
//! ADR-0011): it turns Tauri commands into `ToolRuntime` / `SessionManager`
//! / `RouterService` calls and runtime events into Tauri events. It contains
//! no domain logic.

mod audit_log;
mod commands;
mod provider_commands;
mod router_commands;
mod vault;

use audit_log::AuditLog;
use orchestrator_core::{AuditEvent, EventSink, StreamEvent};
use orchestrator_provider_api::ConnectionManager;
use orchestrator_providers::{EchoProvider, ProviderRegistry, SessionManager};
use orchestrator_router::RouterService;
use orchestrator_runtime::{RuntimeConfig, ToolRuntime};
use provider_commands::RuntimeTools;
use std::path::PathBuf;
use std::sync::Arc;
use tauri::{AppHandle, Emitter, Manager, RunEvent};

/// Tauri event carrying `StreamEvent`s (terminal/process output, provider
/// session events).
pub const STREAM_EVENT: &str = "runtime://stream";
/// Tauri event carrying `AuditEvent`s (history).
pub const AUDIT_EVENT: &str = "runtime://audit";

/// Publishes runtime events to the webview and records audit events.
pub struct DesktopSink {
    app: AppHandle,
    log: AuditLog,
}

impl DesktopSink {
    pub fn recent(&self, limit: usize) -> Vec<AuditEvent> {
        self.log.recent(limit)
    }

    pub fn audit_log_path(&self) -> String {
        self.log.path().display().to_string()
    }
}

impl EventSink for DesktopSink {
    fn audit(&self, event: AuditEvent) {
        self.log.append(&event);
        if let Err(err) = self.app.emit(AUDIT_EVENT, &event) {
            eprintln!("[orchestrator] cannot emit audit event: {err}");
        }
    }

    fn stream(&self, event: StreamEvent) {
        if let Err(err) = self.app.emit(STREAM_EVENT, &event) {
            eprintln!("[orchestrator] cannot emit stream event: {err}");
        }
    }
}

/// State shared by all commands.
pub struct AppState {
    pub runtime: ToolRuntime,
    pub sessions: SessionManager,
    /// The user's API connections (ADR-0010); `None` if they could not be
    /// initialized.
    pub connections: Option<Arc<ConnectionManager>>,
    pub connection_warnings: Vec<String>,
    /// Model router and Council (ADR-0011).
    pub router: RouterService,
    /// Problem loading `council.json`, if any.
    pub router_warning: Option<String>,
    pub sink: Arc<DesktopSink>,
    pub data_dir: PathBuf,
}

/// The development provider `echo` (no AI) is registered in debug builds or
/// with `ORCHESTRATOR_ECHO_PROVIDER=1` (ADR-0009).
fn echo_provider_enabled() -> bool {
    cfg!(debug_assertions)
        || std::env::var("ORCHESTRATOR_ECHO_PROVIDER").is_ok_and(|value| value == "1")
}

/// Built-in providers. The user's API connections are added by the
/// `ConnectionManager` (ADR-0010).
fn provider_registry(sink: Arc<DesktopSink>) -> Arc<ProviderRegistry> {
    let registry = Arc::new(ProviderRegistry::new(sink));
    if echo_provider_enabled() {
        if let Err(err) = registry.register(Arc::new(EchoProvider::new())) {
            eprintln!("[orchestrator] cannot register the echo provider: {err}");
        }
    }
    registry
}

/// Turns SIGTERM/SIGINT/SIGHUP (logout, `kill`, Ctrl+C under `tauri dev`)
/// into a normal exit, so `RunEvent::Exit` runs and managed processes are
/// stopped instead of being left orphaned.
#[cfg(unix)]
fn exit_on_termination_signals(handle: AppHandle) {
    use tokio::signal::unix::{signal, SignalKind};
    tauri::async_runtime::spawn(async move {
        let (Ok(mut term), Ok(mut int), Ok(mut hup)) = (
            signal(SignalKind::terminate()),
            signal(SignalKind::interrupt()),
            signal(SignalKind::hangup()),
        ) else {
            eprintln!("[orchestrator] cannot install termination signal handlers");
            return;
        };
        tokio::select! {
            _ = term.recv() => {}
            _ = int.recv() => {}
            _ = hup.recv() => {}
        }
        handle.exit(0);
    });
}

pub fn run() {
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let data_dir = app
                .path()
                .app_data_dir()
                .unwrap_or_else(|_| std::env::temp_dir().join("orchestrator"));
            let sink = Arc::new(DesktopSink {
                app: app.handle().clone(),
                log: AuditLog::open(&data_dir.join("audit.jsonl")),
            });
            let runtime = ToolRuntime::new(RuntimeConfig::default(), sink.clone());
            let registry = provider_registry(sink.clone());
            let (connections, connection_warnings) = match ConnectionManager::open(
                &data_dir.join("connections.json"),
                registry.clone(),
                Arc::new(vault::OsVault),
                sink.clone(),
            ) {
                Ok((manager, warnings)) => (Some(Arc::new(manager)), warnings),
                Err(err) => (None, vec![err.message]),
            };
            for warning in &connection_warnings {
                eprintln!("[orchestrator] {warning}");
            }
            let (router, router_warning) = RouterService::open(
                &data_dir.join("council.json"),
                registry.clone(),
                sink.clone(),
            );
            if let Some(warning) = &router_warning {
                eprintln!("[orchestrator] {warning}");
            }
            let sessions = SessionManager::new(
                registry,
                Arc::new(RuntimeTools(runtime.clone())),
                sink.clone(),
            );
            app.manage(AppState {
                runtime,
                sessions,
                connections,
                connection_warnings,
                router,
                router_warning,
                sink,
                data_dir,
            });
            #[cfg(unix)]
            exit_on_termination_signals(app.handle().clone());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::runtime_invoke,
            commands::runtime_tools,
            commands::terminal_input,
            commands::terminal_resize,
            commands::history_recent,
            commands::app_info,
            commands::pick_folder,
            provider_commands::providers_list,
            provider_commands::provider_inspect,
            provider_commands::provider_select,
            provider_commands::sessions_list,
            provider_commands::session_start,
            provider_commands::session_get,
            provider_commands::session_send,
            provider_commands::session_cancel,
            provider_commands::session_close,
            provider_commands::session_resume,
            provider_commands::session_spawn,
            provider_commands::connections_list,
            provider_commands::connection_save,
            provider_commands::connection_delete,
            provider_commands::connection_test,
            provider_commands::connection_models,
            router_commands::router_recommend,
            router_commands::council_get,
            router_commands::council_save,
            router_commands::council_run,
            router_commands::council_history,
            router_commands::route_start_session,
        ])
        .build(tauri::generate_context!())
        .expect("failed to build the Orchestrator desktop app");

    app.run(|handle, event| {
        if let RunEvent::Exit = event {
            // Never leave `npm run dev` & co. orphaned when the app closes.
            if let Some(state) = handle.try_state::<AppState>() {
                tauri::async_runtime::block_on(async {
                    state.sessions.shutdown().await;
                    state.runtime.shutdown().await;
                });
            }
        }
    });
}
