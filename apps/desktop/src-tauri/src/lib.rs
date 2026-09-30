//! Orchestrator desktop shell.
//!
//! This crate is only an IPC bridge (ADR-0001, ADR-0003, ADR-0009,
//! ADR-0011, ADR-0013): it turns Tauri commands into `ToolRuntime` /
//! `SessionManager` / `RouterService` / engine calls and runtime events into
//! Tauri events. It contains no domain logic.

mod commands;
mod context_commands;
mod memory_commands;
mod persistence;
mod provider_commands;
mod router_commands;
mod task_commands;
mod vault;

use orchestrator_core::{AuditEvent, EventSink, StreamEvent};
use orchestrator_engine::{
    ContextBuilder, EngineTools, HandoffService, StoreSessions, TaskService,
};
use orchestrator_memory::{HistoryQuery, MemoryStore};
use orchestrator_provider_api::ConnectionManager;
use orchestrator_providers::{EchoProvider, ManagerConfig, ProviderRegistry, SessionManager};
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

/// Publishes runtime events to the webview and records audit events in
/// the database (ADR-0012).
pub struct DesktopSink {
    app: AppHandle,
    store: Arc<MemoryStore>,
}

impl DesktopSink {
    /// The most recent `limit` events, oldest first.
    pub fn recent(&self, limit: usize) -> Vec<AuditEvent> {
        self.store
            .history(&HistoryQuery {
                limit: Some(limit),
                ..Default::default()
            })
            .map(|page| page.events)
            .unwrap_or_default()
    }
}

impl EventSink for DesktopSink {
    fn audit(&self, event: AuditEvent) {
        // Recording may cause follow-ups (PROJECT_CREATED, the detected
        // stack as memory), published and recorded in turn.
        let follow = self.store.record(&event);
        if let Err(err) = self.app.emit(AUDIT_EVENT, &event) {
            eprintln!("[orchestrator] cannot emit audit event: {err}");
        }
        for next in follow {
            self.audit(next);
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
    /// Local database: history, projects, sessions, memory (ADR-0012).
    pub store: Arc<MemoryStore>,
    /// Problem opening or migrating the database, if any.
    pub store_warning: Option<String>,
    /// Model router and Council (ADR-0011).
    pub router: RouterService,
    /// Problem loading `council.json`, if any.
    pub router_warning: Option<String>,
    /// Context Builder (ADR-0013).
    pub builder: Arc<ContextBuilder>,
    /// Problem loading `context.json`, if any.
    pub context_warning: Option<String>,
    /// Handoffs between AIs (ADR-0013).
    pub handoffs: HandoffService,
    /// Tasks of the project (ADR-0014).
    pub tasks: TaskService,
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
            let (store, store_warning) = MemoryStore::open(&data_dir.join("orchestrator.db"));
            let store = Arc::new(store);
            if let Some(warning) = &store_warning {
                eprintln!("[orchestrator] {warning}");
            }
            // The Phase 1–5 history (ADR-0005) moves into the database once.
            match store.import_jsonl(&data_dir.join("audit.jsonl")) {
                Ok(0) => {}
                Ok(n) => eprintln!("[orchestrator] imported {n} events from audit.jsonl"),
                Err(err) => eprintln!("[orchestrator] {err}"),
            }
            let sink = Arc::new(DesktopSink {
                app: app.handle().clone(),
                store: store.clone(),
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
            let router =
                router.with_store(Arc::new(persistence::StoreDeliberations(store.clone())));
            if let Some(warning) = &router_warning {
                eprintln!("[orchestrator] {warning}");
            }
            // AI agents get the runtime's tools plus the memory tools; every
            // session gets the project context on its first turn (ADR-0013).
            let tools = EngineTools::new(
                Arc::new(RuntimeTools(runtime.clone())),
                store.clone(),
                sink.clone(),
            );
            let sessions = SessionManager::with_store(
                registry,
                Arc::new(tools),
                sink.clone(),
                ManagerConfig::default(),
                Arc::new(StoreSessions(store.clone())),
            );
            let (builder, context_warning) =
                ContextBuilder::new(store.clone(), Some(data_dir.join("context.json")));
            let builder = Arc::new(builder);
            if let Some(warning) = &context_warning {
                eprintln!("[orchestrator] {warning}");
            }
            sessions.set_context_source(builder.clone());
            let handoffs = HandoffService::new(
                sessions.clone(),
                store.clone(),
                builder.clone(),
                sink.clone(),
            );
            let tasks = TaskService::new(
                sessions.clone(),
                store.clone(),
                builder.clone(),
                sink.clone(),
            );
            app.manage(AppState {
                runtime,
                sessions,
                connections,
                connection_warnings,
                store,
                store_warning,
                router,
                router_warning,
                builder,
                context_warning,
                handoffs,
                tasks,
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
            provider_commands::session_context_get,
            provider_commands::session_context_set,
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
            memory_commands::history_query,
            memory_commands::projects_recent,
            memory_commands::project_current,
            memory_commands::project_forget,
            memory_commands::projects_import_recent,
            memory_commands::memory_overview,
            memory_commands::memory_list,
            memory_commands::memory_save,
            memory_commands::memory_delete,
            memory_commands::memory_search,
            memory_commands::decisions_list,
            memory_commands::decision_save,
            context_commands::context_preview,
            context_commands::context_settings_get,
            context_commands::context_settings_save,
            context_commands::handoff_prepare,
            context_commands::handoff_create,
            context_commands::handoff_start,
            context_commands::handoffs_list,
            context_commands::handoff_get,
            task_commands::tasks_list,
            task_commands::task_get,
            task_commands::task_save,
            task_commands::task_status,
            task_commands::task_start_session,
            task_commands::task_context,
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
