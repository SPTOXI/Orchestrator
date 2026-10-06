//! Projects that work together (ADR-0023), end to end: two projects open
//! side by side, linked; the AI of one asks the AI of the other, which
//! answers from its own project's files, and leaves it a task.

use async_trait::async_trait;
use orchestrator_core::{
    AuditEvent, CallOrigin, EventKind, EventSink, ProviderId, SessionId, StreamEvent, TaskStatus,
    ToolCall, ToolDefinition, ToolResult, TurnStatus,
};
use orchestrator_engine::{
    ContextBuilder, EngineTools, ProjectTools, SectionKind, StoreSessions, TaskService,
};
use orchestrator_memory::MemoryStore;
use orchestrator_providers::{
    AIProvider, ManagerConfig, NativeSession, ProviderCapabilities, ProviderDescriptor,
    ProviderError, ProviderRegistry, ProviderStatus, SessionManager, SessionSpec, StartRequest,
    ToolExecutor, TurnContext, TurnInput, TurnOutput,
};
use orchestrator_runtime::{RuntimeConfig, ToolRuntime};
use parking_lot::Mutex;
use serde_json::{json, Value};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use tempfile::TempDir;

struct StoreSink {
    store: Arc<MemoryStore>,
    events: Mutex<Vec<AuditEvent>>,
}

impl EventSink for StoreSink {
    fn audit(&self, event: AuditEvent) {
        let follow = self.store.record(&event);
        self.events.lock().push(event);
        for next in follow {
            self.audit(next);
        }
    }

    fn stream(&self, _event: StreamEvent) {}
}

struct RuntimeTools(ToolRuntime);

#[async_trait]
impl ToolExecutor for RuntimeTools {
    fn tools(&self) -> Vec<ToolDefinition> {
        ToolRuntime::definitions().to_vec()
    }

    async fn execute(&self, call: ToolCall) -> ToolResult {
        self.0.invoke(call).await
    }
}

fn answer_of(result: &ToolResult) -> String {
    if result.ok {
        result.output["answer"]
            .as_str()
            .unwrap_or_default()
            .to_owned()
    } else {
        format!("ERRO: {}", result.error.as_ref().unwrap().message)
    }
}

/// The app's AI asks the API's; the API's reads its own files to answer.
struct Scripted;

#[async_trait]
impl AIProvider for Scripted {
    fn descriptor(&self) -> ProviderDescriptor {
        ProviderDescriptor {
            id: ProviderId::from("nuvem"),
            name: "Nuvem".into(),
            vendor: "teste".into(),
            description: "roteirizado".into(),
        }
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            tool_calls: true,
            ..Default::default()
        }
    }

    async fn inspect(&self) -> ProviderStatus {
        ProviderStatus {
            available: true,
            version: None,
            authenticated: None,
            detail: None,
            checked_at: chrono::Utc::now(),
        }
    }

    async fn start(&self, _spec: &SessionSpec) -> Result<NativeSession, ProviderError> {
        Ok(NativeSession {
            reference: format!("nuvem-{}", SessionId::new()),
            model: Some("gpt-medio".into()),
            data: json!({}),
        })
    }

    async fn execute(
        &self,
        _native: &NativeSession,
        input: &TurnInput,
        ctx: &TurnContext,
    ) -> Result<TurnOutput, ProviderError> {
        let text = input.text.as_str();
        let reply = if text.starts_with("[Pergunta da IA do projeto app") {
            // Answering: its own project's files, by a relative path.
            let read = ctx
                .call_tool("filesystem.read", json!({"path": "rotas.txt"}))
                .await;
            assert!(read.ok, "{read:?}");
            // It does not ask back.
            let back = ctx
                .call_tool(
                    "projects.ask",
                    json!({"project": "app", "question": "e você?"}),
                )
                .await;
            format!(
                "A rota é {} | perguntar de volta: {}",
                read.output["content"].as_str().unwrap_or_default().trim(),
                answer_of(&back)
            )
        } else if text == "pergunte" {
            let asked = ctx
                .call_tool(
                    "projects.ask",
                    json!({"project": "API", "question": "Qual a rota que lista usuários?"}),
                )
                .await;
            format!("RESPOSTA: {}", answer_of(&asked))
        } else if text == "pergunte a quem não existe" {
            let asked = ctx
                .call_tool("projects.ask", json!({"project": "site", "question": "?"}))
                .await;
            answer_of(&asked)
        } else if text == "quem são" {
            let related = ctx.call_tool("projects.related", json!({})).await;
            assert!(related.ok, "{related:?}");
            related.output.to_string()
        } else if text == "peça" {
            let requested = ctx
                .call_tool(
                    "projects.request",
                    json!({
                        "project": "api",
                        "title": "Paginar /users",
                        "description": "O app precisa de páginas de 50.",
                        "priority": "high"
                    }),
                )
                .await;
            assert!(requested.ok, "{requested:?}");
            requested.output.to_string()
        } else {
            "ok".to_owned()
        };
        Ok(TurnOutput { text: reply })
    }
}

struct World {
    _dir: TempDir,
    sink: Arc<StoreSink>,
    sessions: SessionManager,
    tasks: TaskService,
    builder: Arc<ContextBuilder>,
    app: orchestrator_memory::Project,
    api: orchestrator_memory::Project,
}

/// Opens `dir` in the app and returns the project the store registered.
async fn open(
    runtime: &ToolRuntime,
    store: &MemoryStore,
    dir: &Path,
) -> orchestrator_memory::Project {
    let opened = runtime
        .invoke(ToolCall::new(
            "project.open",
            json!({"path": dir}),
            CallOrigin::User,
        ))
        .await;
    assert!(opened.ok, "{opened:?}");
    store.current_project().expect("project registered")
}

async fn world() -> World {
    let dir = tempfile::tempdir().unwrap();
    let app_dir = dir.path().join("app");
    let api_dir = dir.path().join("api");
    std::fs::create_dir_all(&app_dir).unwrap();
    std::fs::create_dir_all(&api_dir).unwrap();
    std::fs::write(app_dir.join("package.json"), r#"{"name": "app"}"#).unwrap();
    std::fs::write(app_dir.join("rotas.txt"), "nada aqui\n").unwrap();
    std::fs::write(api_dir.join("Cargo.toml"), "[package]\nname = \"api\"\n").unwrap();
    std::fs::write(api_dir.join("rotas.txt"), "GET /users\n").unwrap();

    let store = Arc::new(MemoryStore::in_memory());
    let sink = Arc::new(StoreSink {
        store: store.clone(),
        events: Mutex::new(Vec::new()),
    });
    let runtime = ToolRuntime::new(
        RuntimeConfig {
            base_dir: dir.path().to_path_buf(),
        },
        sink.clone(),
    );
    // The app is opened last: the API is not the project the app shows.
    let api = open(&runtime, &store, &api_dir).await;
    let app = open(&runtime, &store, &app_dir).await;
    assert_eq!((app.name.as_str(), api.name.as_str()), ("app", "api"));
    store
        .project_link(&app.id, &api.id, "o app usa a API")
        .unwrap();

    let registry = Arc::new(ProviderRegistry::new(sink.clone()));
    registry.register(Arc::new(Scripted)).unwrap();
    let projects = Arc::new(ProjectTools::new(
        Arc::new(EngineTools::new(
            Arc::new(RuntimeTools(runtime.clone())),
            store.clone(),
            sink.clone(),
        )),
        store.clone(),
        sink.clone(),
    ));
    let sessions = SessionManager::with_store(
        registry,
        projects.clone(),
        sink.clone(),
        ManagerConfig {
            cancel_grace: Duration::from_millis(200),
            log_capacity: 1_000,
        },
        Arc::new(StoreSessions(store.clone())),
    );
    let builder = Arc::new(ContextBuilder::new(store.clone(), None).0.without_git());
    sessions.set_context_source(builder.clone());
    let tasks = TaskService::new(
        sessions.clone(),
        store.clone(),
        builder.clone(),
        sink.clone(),
    );
    projects.connect(sessions.clone(), tasks.clone());
    World {
        _dir: dir,
        sink,
        sessions,
        tasks,
        builder,
        app,
        api,
    }
}

impl World {
    async fn session_in_app(&self) -> SessionId {
        self.sessions
            .start(
                StartRequest {
                    provider: Some("nuvem".into()),
                    ..Default::default()
                },
                self.app.path.clone().into(),
                CallOrigin::User,
            )
            .await
            .unwrap()
            .id
    }

    async fn say(&self, id: &SessionId, text: &str) -> String {
        let result = self
            .sessions
            .execute(id, text.into(), CallOrigin::User)
            .await
            .unwrap();
        assert_eq!(result.status, TurnStatus::Completed, "{result:?}");
        result.text
    }

    fn events(&self, kind: EventKind) -> Vec<AuditEvent> {
        self.sink
            .events
            .lock()
            .iter()
            .filter(|e| e.kind == kind)
            .cloned()
            .collect()
    }

    fn conversations(&self) -> Vec<orchestrator_core::SessionInfo> {
        self.sessions
            .list()
            .into_iter()
            .filter(|s| s.title == "Conversa com app")
            .collect()
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn the_ai_of_one_project_asks_the_ai_of_a_related_one() {
    let w = world().await;
    let mine = w.session_in_app().await;

    // The API's AI answers from the API's folder, though the app is the
    // project open now, and does not ask back.
    let text = w.say(&mine, "pergunte").await;
    assert!(text.starts_with("RESPOSTA: A rota é GET /users"), "{text}");
    assert!(
        text.contains("perguntar de volta: ERRO") && text.contains("respondendo"),
        "{text}"
    );

    // The conversation lives in the API project, where the user sees it,
    // and a second question goes to the same one.
    let conversation = w.conversations();
    assert_eq!(conversation.len(), 1);
    assert_eq!(
        conversation[0].project_path.to_string_lossy(),
        w.api.path.as_str()
    );
    w.say(&mine, "pergunte").await;
    assert_eq!(w.conversations().len(), 1);
    let asked = w.events(EventKind::ProjectAsked);
    assert_eq!(asked.len(), 2);
    assert_eq!(asked[0].data["projectId"], json!(w.app.id));
    assert_eq!(asked[0].data["targetProjectId"], json!(w.api.id));
    assert_eq!(asked[0].data["targetSessionId"], json!(conversation[0].id));

    // Only related projects can be asked.
    let refused = w.say(&mine, "pergunte a quem não existe").await;
    assert!(
        refused.contains("não é um projeto relacionado") && refused.contains("api"),
        "{refused}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn related_projects_are_known_and_receive_tasks() {
    let w = world().await;
    let mine = w.session_in_app().await;

    let related: Value = serde_json::from_str(&w.say(&mine, "quem são").await).unwrap();
    let projects = related["projects"].as_array().unwrap();
    assert_eq!(projects.len(), 1);
    assert_eq!(projects[0]["path"], json!(w.api.path));
    assert_eq!(projects[0]["relation"], "o app usa a API");
    assert_eq!(projects[0]["openInApp"], true);
    assert_eq!(projects[0]["exists"], true);

    let requested: Value = serde_json::from_str(&w.say(&mine, "peça").await).unwrap();
    let tasks = w.tasks.list(Some(&w.api.id));
    assert_eq!(tasks.len(), 1);
    let task = &tasks[0].task;
    assert_eq!(requested["taskId"], json!(task.id));
    assert_eq!(task.title, "Paginar /users");
    assert_eq!(task.status, TaskStatus::Todo);
    assert!(task.description.contains("O app precisa de páginas de 50."));
    assert!(task.description.contains("projeto app"));
    assert!(w.tasks.list(Some(&w.app.id)).is_empty());

    // The context of the app's sessions names the API and how to reach it.
    let pack = w.builder.build(&orchestrator_engine::BuildRequest {
        project_path: w.app.path.clone().into(),
        ..Default::default()
    });
    let section = pack
        .sections
        .iter()
        .find(|s| s.kind == SectionKind::Related)
        .expect("related projects section");
    assert!(
        section.items[0].contains(&w.api.path),
        "{:?}",
        section.items
    );
    assert!(
        section.items[0].contains("o app usa a API"),
        "{:?}",
        section.items
    );
    assert!(pack.text.contains("projects.ask"), "{}", pack.text);
}
