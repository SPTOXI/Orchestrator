//! The engine end to end (ADR-0013): the Context Builder over a real
//! project (Git, database, Tool Runtime), sessions receiving it, the memory
//! tools of the AIs and a handoff between two AIs.

use async_trait::async_trait;
use orchestrator_core::{
    AuditEvent, CallOrigin, EventKind, EventSink, HandoffStatus, ProviderId, SessionEvent,
    SessionId, SessionStatus, StreamEvent, TaskInput, TaskPriority, TaskStatus, ToolCall,
    ToolDefinition, ToolResult, TurnStatus,
};
use orchestrator_engine::{
    packet, BuildRequest, ContextBuilder, CreateRequest, EngineTools, HandoffService,
    PrepareRequest, SectionKind, StartHandoff, StartTaskSession, StoreSessions, TaskService,
};
use orchestrator_memory::{
    DecisionInput, DecisionStatus, HistoryQuery, MemoryInput, MemoryKind, MemoryStore, Source,
};
use orchestrator_providers::{
    AIProvider, EchoProvider, ManagerConfig, NativeSession, ProviderCapabilities,
    ProviderDescriptor, ProviderError, ProviderRegistry, ProviderStatus, SessionManager,
    SessionSpec, StartRequest, ToolExecutor, TurnContext, TurnInput, TurnOutput,
};
use orchestrator_runtime::{RuntimeConfig, ToolRuntime};
use parking_lot::Mutex;
use serde_json::json;
use std::path::Path;
use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tempfile::TempDir;

/// Records like the app's sink: the event, then its follow-ups.
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

impl StoreSink {
    fn of(&self, kind: EventKind) -> Vec<AuditEvent> {
        self.events
            .lock()
            .iter()
            .filter(|e| e.kind == kind)
            .cloned()
            .collect()
    }
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

/// A provider that works on the project and, when the Orchestrator asks for
/// a handoff, answers with the JSON of the packet.
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
        let text = if input.text.starts_with("The user is handing this work over") {
            json!({
                "goal": "Pagamentos com Stripe",
                "status": "50% concluído",
                "completed": ["checkout", "criação de clientes"],
                "remaining": ["webhook", "cancelamento"],
                "errors": ["Assinatura do webhook inválida"],
                "decisions": ["Stripe SDK"],
                "tests": [],
                "nextAction": "Validar a assinatura do webhook"
            })
            .to_string()
        } else if input.text == "trabalhe" {
            let wrote = ctx
                .call_tool(
                    "filesystem.write",
                    json!({"path": "src/pay.ts", "content": "export const pay = 1;"}),
                )
                .await;
            assert!(wrote.ok, "{wrote:?}");
            ctx.call_tool("shell.execute", json!({"command": "exit 3"}))
                .await;
            "RESPOSTA_PRIVADA_DA_CONVERSA".into()
        } else {
            "ok".into()
        };
        Ok(TurnOutput { text })
    }
}

struct World {
    _dir: TempDir,
    project_path: String,
    store: Arc<MemoryStore>,
    sink: Arc<StoreSink>,
    runtime: ToolRuntime,
    registry: Arc<ProviderRegistry>,
    sessions: SessionManager,
    builder: Arc<ContextBuilder>,
    handoffs: HandoffService,
    tasks: TaskService,
}

fn git(dir: &Path, args: &[&str]) {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .expect("git");
    assert!(out.status.success(), "git {args:?}: {out:?}");
}

async fn world() -> World {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::create_dir_all(root.join("src/api")).unwrap();
    std::fs::write(
        root.join("src/api/webhooks.ts"),
        "// SEGREDO_DO_ARQUIVO\nexport {}",
    )
    .unwrap();
    std::fs::write(root.join("README.md"), "# saas\n").unwrap();
    git(root, &["init", "-q"]);
    git(root, &["symbolic-ref", "HEAD", "refs/heads/main"]);
    git(root, &["config", "user.email", "t@example.com"]);
    git(root, &["config", "user.name", "Teste"]);
    git(root, &["config", "commit.gpgsign", "false"]);
    git(root, &["add", "."]);
    git(root, &["commit", "-q", "-m", "início"]);
    std::fs::write(root.join("README.md"), "# saas\nmudou\n").unwrap();
    std::fs::write(root.join("notas.txt"), "x").unwrap();

    let store = Arc::new(MemoryStore::in_memory());
    let sink = Arc::new(StoreSink {
        store: store.clone(),
        events: Mutex::new(Vec::new()),
    });
    let runtime = ToolRuntime::new(
        RuntimeConfig {
            base_dir: root.to_path_buf(),
        },
        sink.clone(),
    );
    let opened = runtime
        .invoke(ToolCall::new(
            "project.open",
            json!({"path": root}),
            CallOrigin::User,
        ))
        .await;
    assert!(opened.ok, "{opened:?}");
    let project_path = store.current_project().expect("project registered").path;

    let registry = Arc::new(ProviderRegistry::new(sink.clone()));
    registry
        .register(Arc::new(
            EchoProvider::new().with_chunk_delay(Duration::ZERO),
        ))
        .unwrap();
    registry.register(Arc::new(Scripted)).unwrap();
    let tools = EngineTools::new(
        Arc::new(RuntimeTools(runtime.clone())),
        store.clone(),
        sink.clone(),
    );
    let sessions = SessionManager::with_store(
        registry.clone(),
        Arc::new(tools),
        sink.clone(),
        ManagerConfig {
            cancel_grace: Duration::from_millis(200),
            log_capacity: 1_000,
        },
        Arc::new(StoreSessions(store.clone())),
    );
    let builder = Arc::new(ContextBuilder::new(store.clone(), None).0);
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
    World {
        _dir: dir,
        project_path,
        store,
        sink,
        runtime,
        registry,
        sessions,
        builder,
        handoffs,
        tasks,
    }
}

impl World {
    fn project_id(&self) -> String {
        self.store.current_project().unwrap().id
    }

    async fn session(&self, provider: &str) -> SessionId {
        self.sessions
            .start(
                StartRequest {
                    provider: Some(provider.into()),
                    ..Default::default()
                },
                self.project_path.clone().into(),
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

    fn remember(&self, title: &str, content: &str, pinned: bool) -> String {
        self.store
            .memory_save(
                MemoryInput {
                    id: None,
                    project_id: self.project_id(),
                    kind: MemoryKind::Rule,
                    title: title.into(),
                    content: content.into(),
                    tags: vec![],
                    pinned,
                },
                &CallOrigin::User,
            )
            .unwrap()
            .0
            .id
    }

    fn decide(&self, title: &str, decision: &str, status: DecisionStatus) {
        self.store
            .decision_save(
                DecisionInput {
                    id: None,
                    project_id: self.project_id(),
                    title: title.into(),
                    context: String::new(),
                    decision: decision.into(),
                    consequences: String::new(),
                    status,
                },
                &CallOrigin::User,
            )
            .unwrap();
    }

    fn transcript(&self, id: &SessionId) -> Vec<SessionEvent> {
        self.sessions
            .snapshot(id)
            .unwrap()
            .entries
            .into_iter()
            .map(|e| e.event)
            .collect()
    }

    async fn idle(&self, id: &SessionId) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while self.sessions.info(id).unwrap().status == SessionStatus::Running {
            assert!(Instant::now() < deadline, "turn did not finish");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn the_context_is_relevant_and_fits_the_budget() {
    let w = world().await;
    w.remember(
        "Retentativas de pagamento",
        "Chamadas ao gateway usam 3 retentativas com backoff.",
        true,
    );
    w.remember(
        "Webhooks do Stripe",
        "A assinatura do webhook é validada com o segredo do endpoint.",
        false,
    );
    w.remember("Filas de e-mail", "Redis Streams para e-mails.", false);
    w.decide(
        "Stripe como gateway",
        "Usar Stripe Billing",
        DecisionStatus::Accepted,
    );
    w.decide(
        "Migrar para GraphQL",
        "Trocar REST por GraphQL",
        DecisionStatus::Proposed,
    );
    let failed = w
        .runtime
        .invoke(ToolCall::new(
            "shell.execute",
            json!({"command": "exit 2"}),
            CallOrigin::User,
        ))
        .await;
    assert!(failed.ok);
    // An earlier conversation about the same subject.
    let old = w.session("echo").await;
    w.say(&old, "como validar a assinatura do webhook?").await;

    let task = "Corrija a validação da assinatura do webhook em src/api/webhooks.ts";
    let pack = w.builder.build(&BuildRequest {
        project_path: w.project_path.clone().into(),
        task: task.into(),
        ..Default::default()
    });
    // The header points to the memory tools, unless the session has none.
    assert!(pack.text.contains("memory.search"), "{}", pack.text);
    let offline = w.builder.build(&BuildRequest {
        project_path: w.project_path.clone().into(),
        task: task.into(),
        no_tools: true,
        ..Default::default()
    });
    assert!(!offline.text.contains("memory.search"), "{}", offline.text);
    assert!(offline.text.contains("Tools are not available"));
    let kinds: Vec<_> = pack.sections.iter().map(|s| s.kind).collect();
    assert_eq!(
        kinds,
        [
            SectionKind::Task,
            SectionKind::Working,
            SectionKind::Project,
            SectionKind::Files,
            SectionKind::Errors,
            SectionKind::History,
            SectionKind::Git
        ],
        "{}",
        pack.text
    );
    let section = |kind| {
        pack.sections
            .iter()
            .find(|s| s.kind == kind)
            .unwrap()
            .items
            .join("\n")
    };
    let project = section(SectionKind::Project);
    assert!(
        project.contains("[rule, pinned] Retentativas de pagamento"),
        "{project}"
    );
    assert!(project.contains("[rule] Webhooks do Stripe"), "{project}");
    assert!(
        project.contains("[decision, accepted] Stripe como gateway"),
        "{project}"
    );
    assert!(!project.contains("Filas de e-mail"), "unrelated: {project}");
    assert!(
        !project.contains("GraphQL"),
        "unrelated proposal: {project}"
    );
    assert!(section(SectionKind::Files).starts_with("src/api/webhooks.ts (mentioned in the task)"));
    assert!(section(SectionKind::Errors).contains("exit 2"));
    assert!(section(SectionKind::History).contains("como validar a assinatura do webhook"));
    let git = section(SectionKind::Git);
    assert!(git.starts_with("Branch main (no upstream)"), "{git}");
    assert!(
        git.contains("README.md: modified (not staged)") && git.contains("notas.txt: untracked"),
        "{git}"
    );
    assert!(section(SectionKind::Working).contains("$ exit 2 → exit 2 (user"));

    // Paths, never file contents; within the budget.
    assert!(!pack.text.contains("SEGREDO_DO_ARQUIVO"));
    assert!(pack.tokens <= pack.budget && pack.budget == 1_500);
    assert!(pack.omitted.is_empty());
    assert!(pack.text.starts_with("# PROJECT CONTEXT\nProject "));

    // A failure in RECENT ERRORS is not repeated as history.
    let failing = w.builder.build(&BuildRequest {
        project_path: w.project_path.clone().into(),
        task: "exit webhook".into(),
        ..Default::default()
    });
    let text = &failing.text;
    assert!(
        text.contains("## RECENT ERRORS\n- exit 2 (exit 2)"),
        "{text}"
    );
    assert!(!text.contains("[event: exit 2 (exit 2)]"), "{text}");

    // A small budget cuts by priority: history, files, errors, other
    // entries and working memory go before the pinned memory; the task and
    // the Git state stay.
    for i in 0..5 {
        w.remember(
            &format!("Nota fixada {i}"),
            &"detalhe importante ".repeat(20),
            true,
        );
    }
    let full = w.builder.build(&BuildRequest {
        project_path: w.project_path.clone().into(),
        task: task.into(),
        budget: Some(8_000),
        ..Default::default()
    });
    assert!(full.tokens > 600, "{}", full.tokens);
    let small = w.builder.build(&BuildRequest {
        project_path: w.project_path.clone().into(),
        task: task.into(),
        budget: Some(400),
        ..Default::default()
    });
    assert!(
        small.tokens <= 400,
        "{} tokens:\n{}",
        small.tokens,
        small.text
    );
    let kinds: Vec<_> = small.sections.iter().map(|s| s.kind).collect();
    assert_eq!(
        kinds,
        [SectionKind::Task, SectionKind::Project, SectionKind::Git],
        "{:?}\n{}",
        small.omitted,
        small.text
    );
    let omitted = small.omitted.join(" | ");
    for gone in [
        "RELEVANT HISTORY",
        "RELEVANT FILES",
        "RECENT ERRORS",
        "WORKING MEMORY",
        "PROJECT MEMORY",
    ] {
        assert!(omitted.contains(gone), "{omitted}");
    }
    assert!(
        small.text.contains("[rule, pinned]"),
        "pinned entries go last"
    );
    assert!(
        !small.text.contains("[rule] Webhooks"),
        "other entries go before pinned ones"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn sessions_receive_the_context_and_use_the_memory_tools() {
    let w = world().await;
    let user_entry = w.remember("Regra do usuário", "Nunca edite migrações.", true);
    let id = w.session("echo").await;

    // The first turn received the context.
    let context = w.say(&id, "/context").await;
    assert!(context.starts_with("# PROJECT CONTEXT"), "{context}");
    assert!(context.contains("## TASK\nThe task is the user's first message"));
    assert!(context.contains("[rule, pinned] Regra do usuário"));
    assert!(context.contains("## GIT STATE"));
    let built = w
        .store
        .history(&HistoryQuery {
            kinds: vec![EventKind::ContextBuilt],
            ..Default::default()
        })
        .unwrap()
        .events;
    assert_eq!(built.len(), 1);
    assert_eq!(built[0].data["sessionId"], json!(id));
    assert!(built[0].data["tokens"].as_u64().unwrap() > 50);

    // The memory tools are offered and executed by the Orchestrator.
    let saved = w
        .say(
            &id,
            r#"/tool memory.save {"kind": "rule", "title": "Idempotência", "content": "Use chave de idempotência nos pagamentos", "pinned": true}"#,
        )
        .await;
    assert!(saved.contains("✓ memory.save"), "{saved}");
    let entries = w.store.memory_list(&w.project_id()).unwrap();
    let mine = entries.iter().find(|e| e.title == "Idempotência").unwrap();
    assert_eq!((mine.source, mine.pinned), (Source::Agent, true));
    let calls = w.sink.of(EventKind::ToolCalled);
    let save_call = calls
        .iter()
        .find(|e| e.data["tool"] == "memory.save")
        .unwrap();
    assert_eq!(save_call.data["readOnly"], false);
    assert!(matches!(save_call.origin, CallOrigin::Agent { .. }));
    let memory_saved = w.sink.of(EventKind::MemorySaved);
    let event = memory_saved.last().unwrap();
    assert_eq!(event.call_id, save_call.call_id);
    assert!(matches!(event.origin, CallOrigin::Agent { .. }));

    let found = w
        .say(
            &id,
            r#"/tool memory.search {"query": "idempotência pagamentos"}"#,
        )
        .await;
    assert!(
        found.contains("✓ memory.search") && found.contains("Idempotência"),
        "{found}"
    );

    // An AI does not rewrite what the user wrote.
    let refused = w
        .say(
            &id,
            &format!(
                r#"/tool memory.save {{"id": "{user_entry}", "kind": "rule", "title": "x", "content": "y"}}"#
            ),
        )
        .await;
    assert!(
        refused.contains("InvalidArgs") && refused.contains("written by the user"),
        "{refused}"
    );

    let decided = w
        .say(
            &id,
            r#"/tool decision.save {"title": "Filas no Redis", "decision": "Usar Redis Streams"}"#,
        )
        .await;
    assert!(decided.contains("✓ decision.save"), "{decided}");
    let decision = &w.store.decisions_list(&w.project_id()).unwrap()[0];
    assert_eq!(
        (decision.status, decision.source),
        (DecisionStatus::Proposed, Source::Agent)
    );
    let working = w.say(&id, "/tool memory.working").await;
    assert!(working.contains("✓ memory.working"), "{working}");

    // Context once per session: still the first one.
    assert_eq!(w.say(&id, "/context").await, context);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_handoff_passes_the_work_without_the_conversation() {
    let w = world().await;
    let source = w.session("nuvem").await;
    w.say(&source, "Implementar pagamentos Stripe").await;
    assert_eq!(
        w.say(&source, "trabalhe").await,
        "RESPOSTA_PRIVADA_DA_CONVERSA"
    );

    // Draft: narrative from the AI, facts from the history.
    let draft = w
        .handoffs
        .prepare(PrepareRequest {
            session_id: source.clone(),
            ask_agent: true,
        })
        .await
        .unwrap();
    assert!(draft.by_agent, "{:?}", draft.notes);
    let p = &draft.packet;
    assert_eq!(p.goal, "Pagamentos com Stripe");
    assert_eq!(p.status, "50% concluído");
    assert_eq!(p.remaining, ["webhook", "cancelamento"]);
    assert_eq!(p.next_action, "Validar a assinatura do webhook");
    assert_eq!(p.files, ["src/pay.ts (criado)"]);
    assert_eq!(p.commands, ["exit 3 → saída 3"]);
    assert_eq!(p.errors[0], "Assinatura do webhook inválida");
    assert!(
        p.errors.iter().any(|e| e.starts_with("exit 3 (exit 3)")),
        "{:?}",
        p.errors
    );
    assert_eq!(draft.from.provider, ProviderId::from("nuvem"));

    let handoff = w
        .handoffs
        .create(
            CreateRequest {
                session_id: source.clone(),
                packet: draft.packet.clone(),
                by_agent: true,
            },
            CallOrigin::User,
        )
        .unwrap();
    assert_eq!(w.sink.of(EventKind::HandoffCreated).len(), 1);
    assert_eq!(
        w.handoffs.list(Some(&w.project_id())),
        std::slice::from_ref(&handoff)
    );

    // Another AI takes over.
    let started = w
        .handoffs
        .start(
            StartHandoff {
                handoff_id: handoff.id.clone(),
                provider: "echo".into(),
                model: None,
                title: None,
                budget: None,
            },
            CallOrigin::User,
        )
        .await
        .unwrap();
    assert_eq!(started.handoff.status, HandoffStatus::Accepted);
    assert!(started.send_error.is_none());
    let target = started.session.id.clone();
    assert_eq!(started.session.title, "Handoff · Pagamentos com Stripe");
    w.idle(&target).await;
    let accepted = w.sink.of(EventKind::HandoffAccepted);
    assert_eq!(accepted.len(), 1);
    assert_eq!(accepted[0].data["toSession"], json!(target));

    let events = w.transcript(&target);
    assert!(
        matches!(&events[0], SessionEvent::HandedOff { from_session, .. } if *from_session == source)
    );
    assert!(matches!(
        &events[1],
        SessionEvent::TurnStarted { input, .. } if *input == packet::first_message(&started.handoff)
    ));
    assert!(w.transcript(&source).iter().any(|e| matches!(
        e,
        SessionEvent::HandedOff { to_session, .. } if *to_session == target
    )));

    // What the new AI received: the packet, not the conversation.
    let context = w.say(&target, "/context").await;
    assert!(
        context.contains("## HANDOFF\nFrom session \"Nuvem #1\" (nuvem/gpt-medio)"),
        "{context}"
    );
    assert!(context.contains("GOAL: Pagamentos com Stripe"));
    assert!(context.contains("NEXT ACTION: Validar a assinatura do webhook"));
    assert!(context.contains("Take over the work described in HANDOFF"));
    assert!(
        !context.contains("RESPOSTA_PRIVADA_DA_CONVERSA"),
        "{context}"
    );
    assert!(
        !context.contains("Implementar pagamentos Stripe"),
        "{context}"
    );
    let built = w.sink.of(EventKind::ContextBuilt);
    assert_eq!(built.last().unwrap().data["handoffId"], json!(handoff.id));

    // Handing over again from the new session keeps the goal (its first
    // message is the takeover instruction).
    let next = w
        .handoffs
        .prepare(PrepareRequest {
            session_id: target.clone(),
            ask_agent: false,
        })
        .await
        .unwrap();
    assert_eq!(next.packet.goal, "Pagamentos com Stripe");

    // Accepted once.
    let again = w
        .handoffs
        .start(
            StartHandoff {
                handoff_id: handoff.id,
                provider: "echo".into(),
                model: None,
                title: None,
                budget: None,
            },
            CallOrigin::User,
        )
        .await;
    assert!(again.is_err());
}

#[tokio::test(flavor = "multi_thread")]
async fn without_its_ai_the_draft_keeps_the_facts() {
    let w = world().await;
    let source = w.session("nuvem").await;
    w.say(&source, "Implementar pagamentos Stripe").await;
    w.say(&source, "trabalhe").await;
    // The provider is gone (connection removed): often why one hands off.
    w.registry
        .unregister(&ProviderId::from("nuvem"), CallOrigin::User);

    let draft = w
        .handoffs
        .prepare(PrepareRequest {
            session_id: source,
            ask_agent: true,
        })
        .await
        .unwrap();
    assert!(!draft.by_agent);
    assert!(
        draft.notes[0].contains("não respondeu"),
        "{:?}",
        draft.notes
    );
    assert_eq!(draft.packet.goal, "Implementar pagamentos Stripe");
    assert!(draft.packet.status.is_empty() && draft.packet.next_action.is_empty());
    assert_eq!(draft.packet.files, ["src/pay.ts (criado)"]);
    assert!(draft.usage.is_none());

    // An AI that only repeats the request (here the echo provider) gives no
    // narrative either: the request's examples are not taken as facts.
    let echo = w.session("echo").await;
    w.say(&echo, "Revisar o README").await;
    let draft = w
        .handoffs
        .prepare(PrepareRequest {
            session_id: echo,
            ask_agent: true,
        })
        .await
        .unwrap();
    assert!(!draft.by_agent);
    assert!(
        draft.notes[0].contains("só repetiu o modelo do pedido"),
        "{:?}",
        draft.notes
    );
    assert_eq!(draft.packet.goal, "Revisar o README");
    assert!(draft.packet.status.is_empty() && draft.packet.completed.is_empty());
    assert!(draft.usage.is_some(), "the turn was spent and is reported");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_task_orders_the_work_and_carries_it_into_a_session() {
    let w = world().await;
    let project = w.project_id();
    let new = |title: &str| TaskInput {
        project_id: Some(project.clone()),
        title: Some(title.into()),
        ..Default::default()
    };

    // A task needs a title, and the project it belongs to.
    assert!(w
        .tasks
        .save(new("   "), CallOrigin::User)
        .unwrap_err()
        .message
        .contains("título"));

    let schema = w
        .tasks
        .save(new("Modelar as cobranças"), CallOrigin::User)
        .unwrap();
    let api = w
        .tasks
        .save(
            TaskInput {
                description: Some("Aplicar as retentativas no cliente do gateway.".into()),
                files: Some(vec!["src/api/webhooks.ts".into()]),
                priority: Some(TaskPriority::High),
                dependencies: Some(vec![schema.id.clone()]),
                ..new("Expor a API de cobranças")
            },
            CallOrigin::User,
        )
        .unwrap();
    assert_eq!(w.sink.of(EventKind::TaskCreated).len(), 2);

    // A cycle between dependencies is refused, however long.
    let err = w
        .tasks
        .save(
            TaskInput {
                id: Some(schema.id.clone()),
                dependencies: Some(vec![api.id.clone()]),
                ..Default::default()
            },
            CallOrigin::User,
        )
        .unwrap_err();
    assert!(err.message.contains("ciclo"), "{}", err.message);

    // While the dependency is open, the task cannot start — by hand or by
    // opening a session for it.
    let err = w
        .tasks
        .set_status(&api.id, TaskStatus::InProgress, CallOrigin::User)
        .unwrap_err();
    assert!(
        err.message.contains("Modelar as cobranças"),
        "{}",
        err.message
    );
    let err = w
        .tasks
        .start_session(
            StartTaskSession {
                task_id: api.id.clone(),
                provider: Some("echo".into()),
                model: None,
                budget: None,
            },
            CallOrigin::User,
        )
        .await
        .unwrap_err();
    assert!(err.message.contains("espera"), "{}", err.message);

    // The panel says what it waits for and which moves are allowed.
    let view = w.tasks.get(&api.id).unwrap();
    assert_eq!(view.waiting_for.len(), 1);
    assert_eq!(view.waiting_for[0].title, "Modelar as cobranças");
    assert!(!view.can.contains(&TaskStatus::Done));
    // In progress comes first in the panel, then priority.
    let listed = w.tasks.list(Some(&project));
    assert_eq!(listed.len(), 2);
    assert_eq!(listed[0].task.title, "Expor a API de cobranças");

    // With the dependency done, the task starts in a session of its own.
    w.tasks
        .set_status(&schema.id, TaskStatus::InProgress, CallOrigin::User)
        .unwrap();
    w.tasks
        .set_status(&schema.id, TaskStatus::Done, CallOrigin::User)
        .unwrap();
    assert_eq!(w.sink.of(EventKind::TaskCompleted).len(), 1);
    assert!(w.tasks.get(&api.id).unwrap().waiting_for.is_empty());

    let started = w
        .tasks
        .start_session(
            StartTaskSession {
                task_id: api.id.clone(),
                provider: Some("echo".into()),
                model: None,
                budget: None,
            },
            CallOrigin::User,
        )
        .await
        .unwrap();
    assert!(started.send_error.is_none());
    assert_eq!(started.task.status, TaskStatus::InProgress);
    assert!(started.task.started_at.is_some());
    assert_eq!(
        started.task.sessions,
        std::slice::from_ref(&started.session.id)
    );
    w.idle(&started.session.id).await;

    // The session's context came from the task: its words and its files,
    // never a generic "first message".
    let context = w.say(&started.session.id, "/context").await;
    assert!(context.contains("## TASK"), "{context}");
    assert!(
        context.contains("src/api/webhooks.ts (mentioned in the task)"),
        "{context}"
    );
    // The task is in the project's memory, so the search finds it.
    let hits = w.store.search(&project, "cobrancas", 10).unwrap();
    assert!(hits.iter().any(|h| h.kind == "task"), "{hits:?}");
    assert_eq!(
        w.store
            .task_of_session(started.session.id.as_str())
            .map(|t| t.id),
        Some(api.id.clone())
    );

    // Finishing, then reopening: the history keeps both, and a reopened
    // task is open work again.
    w.tasks
        .set_status(&api.id, TaskStatus::Review, CallOrigin::User)
        .unwrap();
    let done = w
        .tasks
        .set_status(&api.id, TaskStatus::Done, CallOrigin::User)
        .unwrap();
    assert!(done.finished_at.is_some());
    assert!(w
        .tasks
        .set_status(&api.id, TaskStatus::Review, CallOrigin::User)
        .is_err());
    let reopened = w
        .tasks
        .set_status(&api.id, TaskStatus::Todo, CallOrigin::User)
        .unwrap();
    assert!(reopened.finished_at.is_none());
    // Only the moves the three master-document events do not cover.
    let updates = w.sink.of(EventKind::TaskUpdated);
    let moves: Vec<_> = updates
        .iter()
        .map(|e| (e.data["from"].clone(), e.data["to"].clone()))
        .collect();
    assert_eq!(
        moves,
        [
            (json!("IN_PROGRESS"), json!("REVIEW")),
            (json!("DONE"), json!("TODO"))
        ]
    );
}
