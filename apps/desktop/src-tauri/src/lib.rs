//! Orchestrator desktop shell.
//!
//! This crate is only an IPC bridge (ADR-0001, ADR-0003, ADR-0009,
//! ADR-0011, ADR-0013, ADR-0015, ADR-0016, ADR-0019): it turns Tauri commands into
//! `ToolRuntime` / `SessionManager` / `RouterService` / engine / agent /
//! autonomy calls and runtime events into Tauri events. It contains no
//! domain logic.

mod agent_commands;
mod autonomy_commands;
mod commands;
mod context_commands;
mod cost_commands;
mod github_commands;
mod memory_commands;
mod persistence;
mod provider_commands;
mod router_commands;
mod task_commands;
mod update_commands;
mod vault;

use orchestrator_agents::{AgentDeps, AgentService, AgentSlot, AgentTools, LockManager};
use orchestrator_core::{AuditEvent, EventSink, StreamEvent};
use orchestrator_engine::{
    AutonomyGate, AutonomyService, ContextBuilder, EngineTools, HandoffService, StoreSessions,
    TaskService,
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
    /// Agents executing tasks, and their file locks (ADR-0015).
    pub agents: AgentService,
    /// Problem loading `agents.json`, if any.
    pub agents_warning: Option<String>,
    /// Autonomy modes, rules, requests and pause (ADR-0016).
    pub autonomy: AutonomyService,
    /// Problem loading `autonomy.json`, if any.
    pub autonomy_warning: Option<String>,
    /// Problem loading `github.json` or reading the token, if any
    /// (ADR-0017).
    pub github_warning: Option<String>,
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
    let mut builder = tauri::Builder::default().plugin(tauri_plugin_dialog::init());
    // Only release builds carry the key that verifies updates (ADR-0019).
    if let Some(pubkey) = update_commands::pubkey() {
        builder = builder.plugin(tauri_plugin_updater::Builder::new().pubkey(pubkey).build());
    }
    let app = builder
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
            // Processes a crash left running end now (Linux/macOS; on
            // Windows a Job Object ends them with the app, ADR-0018).
            let orphans = runtime.open_process_registry(&data_dir.join("processes.json"));
            if !orphans.is_empty() {
                eprintln!(
                    "[orchestrator] {} process(es) left by the previous run were ended",
                    orphans.len()
                );
            }
            // GitHub (ADR-0017): which server, and the token saved in the
            // vault (the environment and the GitHub CLI are read by the
            // runtime itself).
            let (github_settings, mut github_warning) =
                github_commands::load_settings(&github_commands::settings_path(&data_dir));
            runtime.set_github_settings(github_settings);
            match github_commands::vault_token() {
                Ok(token) => runtime.set_github_token(token),
                Err(err) => {
                    github_warning.get_or_insert(format!("token do GitHub no cofre: {err}"));
                }
            }
            if let Some(warning) = &github_warning {
                eprintln!("[orchestrator] {warning}");
            }
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
            // AI agents get the runtime's tools, plus the memory tools
            // (ADR-0013) and the agent tools with the file locks
            // (ADR-0015), all behind the autonomy gate (ADR-0016), which
            // is the outermost: nothing an AI asks for skips it. Every
            // session gets the project context on its first turn.
            let (autonomy, autonomy_warning) = AutonomyService::new(
                store.clone(),
                sink.clone(),
                Some(data_dir.join("autonomy.json")),
            );
            if let Some(warning) = &autonomy_warning {
                eprintln!("[orchestrator] {warning}");
            }
            let base = runtime.clone();
            autonomy.set_workdir(Arc::new(move || base.base_dir()));
            let locks = Arc::new(LockManager::new(store.clone()));
            let agent_slot = AgentSlot::new();
            let tools = AutonomyGate::new(
                Arc::new(AgentTools::new(
                    Arc::new(EngineTools::new(
                        Arc::new(RuntimeTools(runtime.clone())),
                        store.clone(),
                        sink.clone(),
                    )),
                    store.clone(),
                    locks.clone(),
                    agent_slot.clone(),
                    sink.clone(),
                )),
                autonomy.clone(),
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
            sessions.set_compaction(builder.settings().compaction);
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
            let (agents, agents_warning) = AgentService::new(
                AgentDeps {
                    sessions: sessions.clone(),
                    store: store.clone(),
                    tasks: tasks.clone(),
                    handoffs: HandoffService::new(
                        sessions.clone(),
                        store.clone(),
                        builder.clone(),
                        sink.clone(),
                    ),
                    locks,
                    autonomy: autonomy.clone(),
                    sink: sink.clone(),
                },
                Some(data_dir.join("agents.json")),
            );
            if let Some(warning) = &agents_warning {
                eprintln!("[orchestrator] {warning}");
            }
            // Tauri commands are not polled inside the async runtime, so
            // the agents get its handle explicitly (ADR-0015).
            agents.set_runtime(tauri::async_runtime::handle().inner().clone());
            agent_slot.install(agents.clone());
            // Nothing is running after a restart, so no file stays locked.
            agents.recover();
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
                agents,
                agents_warning,
                autonomy,
                autonomy_warning,
                github_warning,
                sink,
                data_dir,
            });
            // Updates (ADR-0019): the version this run is, and a new one
            // in the history when it changed.
            let version = app.package_info().version.to_string();
            let data_dir = app.state::<AppState>().data_dir.clone();
            let (updates, change) = update_commands::Updates::open(&data_dir, &version);
            update_commands::record_change(app.state::<AppState>().sink.as_ref(), change, &version);
            app.manage(updates);
            update_commands::start_auto_check(app.handle().clone());
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
            cost_commands::spend_report,
            cost_commands::agents_budget,
            cost_commands::session_compact,
            agent_commands::agents_list,
            agent_commands::agent_get,
            agent_commands::agent_start,
            agent_commands::agent_stop,
            agent_commands::agents_stop_all,
            agent_commands::agent_locks,
            agent_commands::agent_settings_get,
            agent_commands::agent_settings_save,
            agent_commands::agent_pause,
            agent_commands::agent_resume,
            autonomy_commands::autonomy_get,
            autonomy_commands::autonomy_set_mode,
            autonomy_commands::autonomy_set_default,
            autonomy_commands::autonomy_save_rules,
            autonomy_commands::autonomy_reset_rules,
            autonomy_commands::autonomy_try,
            autonomy_commands::approvals_pending,
            autonomy_commands::approval_answer,
            autonomy_commands::autonomy_revoke,
            autonomy_commands::execution_pause,
            autonomy_commands::execution_resume,
            github_commands::github_settings_get,
            github_commands::github_settings_save,
            github_commands::github_token_save,
            github_commands::github_token_clear,
            github_commands::open_url,
            task_commands::tasks_list,
            task_commands::task_get,
            task_commands::task_save,
            task_commands::task_status,
            task_commands::task_start_session,
            task_commands::task_context,
            update_commands::update_status,
            update_commands::update_check,
            update_commands::update_install,
            update_commands::update_restart,
            update_commands::update_settings_save,
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
