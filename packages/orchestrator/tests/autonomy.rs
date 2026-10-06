//! The autonomy gate end to end (ADR-0016): sessions of the `echo`
//! provider asking the real Tool Runtime for things, through the gate, in
//! each mode — with requests answered, denied, cancelled, settled by a
//! mode change and held by a pause.

use async_trait::async_trait;
use orchestrator_core::{
    ApprovalAnswer, AuditEvent, AutonomyMode, CallOrigin, Decision, EventKind, EventSink,
    PolicyRule, SessionId, StreamEvent, ToolCall, ToolDefinition, ToolResult, TurnStatus,
};
use orchestrator_engine::{
    ApprovalView, AutonomyGate, AutonomyService, EngineTools, StoreSessions,
};
use orchestrator_memory::MemoryStore;
use orchestrator_providers::{
    EchoProvider, ManagerConfig, ProviderRegistry, SessionManager, StartRequest, ToolExecutor,
    TurnResult,
};
use orchestrator_runtime::{RuntimeConfig, ToolRuntime};
use parking_lot::Mutex;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
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

struct World {
    dir: TempDir,
    root: PathBuf,
    project_id: String,
    sink: Arc<StoreSink>,
    sessions: SessionManager,
    autonomy: AutonomyService,
    gate: Arc<AutonomyGate>,
}

async fn world() -> World {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("proj");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("README.md"), "# proj\n").unwrap();
    std::fs::write(root.join(".env"), "TOKEN=segredo\n").unwrap();

    let store = Arc::new(MemoryStore::in_memory());
    let sink = Arc::new(StoreSink {
        store: store.clone(),
        events: Mutex::new(Vec::new()),
    });
    let runtime = ToolRuntime::new(
        RuntimeConfig {
            base_dir: root.clone(),
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
    let project = store.current_project().expect("project registered");

    let registry = Arc::new(ProviderRegistry::new(sink.clone()));
    registry
        .register(Arc::new(
            EchoProvider::new().with_chunk_delay(Duration::ZERO),
        ))
        .unwrap();
    let (autonomy, warning) = AutonomyService::new(store.clone(), sink.clone(), None);
    assert!(warning.is_none());
    let base = runtime.clone();
    autonomy.set_workdir(Arc::new(move || base.base_dir()));
    let gate = Arc::new(AutonomyGate::new(
        Arc::new(EngineTools::new(
            Arc::new(RuntimeTools(runtime.clone())),
            store.clone(),
            sink.clone(),
        )),
        autonomy.clone(),
    ));
    let sessions = SessionManager::with_store(
        registry,
        gate.clone(),
        sink.clone(),
        ManagerConfig {
            cancel_grace: Duration::from_millis(200),
            log_capacity: 1_000,
        },
        Arc::new(StoreSessions(store.clone())),
    );
    World {
        root: PathBuf::from(&project.path),
        project_id: project.id,
        dir,
        sink,
        sessions,
        autonomy,
        gate,
    }
}

fn tool(name: &str, args: Value) -> String {
    format!("/tool {name} {args}")
}

impl World {
    async fn session(&self) -> SessionId {
        self.sessions
            .start(
                StartRequest {
                    provider: Some("echo".into()),
                    ..Default::default()
                },
                self.root.clone(),
                CallOrigin::User,
            )
            .await
            .unwrap()
            .id
    }

    /// Sends a message and returns the running turn.
    fn send(&self, id: &SessionId, text: String) -> tokio::task::JoinHandle<TurnResult> {
        let sessions = self.sessions.clone();
        let id = id.clone();
        tokio::spawn(async move { sessions.execute(&id, text, CallOrigin::User).await.unwrap() })
    }

    async fn say(&self, id: &SessionId, text: String) -> String {
        let result = self.send(id, text).await.unwrap();
        assert_eq!(result.status, TurnStatus::Completed, "{result:?}");
        result.text
    }

    /// The next request, as soon as there is one.
    async fn request(&self) -> ApprovalView {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(first) = self.autonomy.pending().into_iter().next() {
                return first;
            }
            assert!(Instant::now() < deadline, "no authorization request came");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    fn answer(&self, request: &ApprovalView, answer: ApprovalAnswer, note: Option<&str>) {
        self.autonomy
            .answer(&request.request.id, answer, note, CallOrigin::User)
            .unwrap();
    }

    fn mode(&self, mode: AutonomyMode) {
        self.autonomy
            .set_project_mode(&self.project_id, Some(mode), CallOrigin::User)
            .unwrap();
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn assisted_reads_freely_and_asks_before_acting() {
    let w = world().await;
    assert_eq!(
        w.autonomy.mode_of(Some(&w.project_id)),
        AutonomyMode::Assisted
    );
    let s = w.session().await;

    let read = w
        .say(&s, tool("filesystem.read", json!({"path": "README.md"})))
        .await;
    assert!(read.contains("✓ filesystem.read"), "{read}");
    assert!(w.sink.of(EventKind::ApprovalRequested).is_empty());

    let turn = w.send(
        &s,
        tool(
            "filesystem.write",
            json!({"path": "notas.md", "content": "oi"}),
        ),
    );
    let request = w.request().await;
    assert_eq!(request.request.summary, "Escrever notas.md (2 B)");
    assert!(
        request.request.reason.contains("Modo Assistido, regra 5"),
        "{}",
        request.request.reason
    );
    assert_eq!(request.request.session_id, s);
    assert_eq!(request.project_name.as_deref(), Some("proj"));
    assert!(
        !w.root.join("notas.md").exists(),
        "nothing runs before the answer"
    );

    w.answer(&request, ApprovalAnswer::Approve, None);
    let result = turn.await.unwrap();
    assert!(
        result.text.contains("✓ filesystem.write"),
        "{}",
        result.text
    );
    assert_eq!(
        std::fs::read_to_string(w.root.join("notas.md")).unwrap(),
        "oi"
    );

    // Request, answer and execution are one story in the history.
    let asked = &w.sink.of(EventKind::ApprovalRequested)[0];
    let decided = &w.sink.of(EventKind::ApprovalDecided)[0];
    assert_eq!(decided.data["answer"], "approved");
    assert_eq!(decided.origin, CallOrigin::User);
    let executed = w
        .sink
        .of(EventKind::ToolCalled)
        .into_iter()
        .find(|e| e.data["tool"] == "filesystem.write")
        .unwrap();
    assert_eq!(asked.call_id, executed.call_id);
    assert_eq!(decided.call_id, executed.call_id);

    // Environment files and what is outside the project are asked for,
    // even to read.
    let secret = w.send(&s, tool("filesystem.read", json!({"path": ".env"})));
    let request = w.request().await;
    assert!(
        request.request.reason.contains("regra 1"),
        "{:?}",
        request.request
    );
    w.answer(&request, ApprovalAnswer::Deny, None);
    let text = secret.await.unwrap().text;
    assert!(!text.contains("segredo"), "{text}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_denial_reaches_the_ai_with_the_reason_and_is_recorded() {
    let w = world().await;
    let s = w.session().await;
    let turn = w.send(
        &s,
        tool(
            "shell.execute",
            json!({"command": "echo feito > saida.txt"}),
        ),
    );
    let request = w.request().await;
    assert_eq!(request.request.summary, "Executar `echo feito > saida.txt`");
    w.answer(
        &request,
        ApprovalAnswer::Deny,
        Some("use o pnpm, não o shell"),
    );
    let text = turn.await.unwrap().text;
    assert!(text.contains("Denied"), "{text}");
    assert!(text.contains("use o pnpm, não o shell"), "{text}");
    assert!(!w.root.join("saida.txt").exists());

    // Never a silent refusal: the gate records the call it refused.
    let refused = w
        .sink
        .of(EventKind::ToolCalled)
        .into_iter()
        .find(|e| e.data["tool"] == "shell.execute")
        .expect("TOOL_CALLED for the refused call");
    assert_eq!(refused.data["ok"], false);
    assert_eq!(refused.data["error"]["kind"], "DENIED");
    assert_eq!(refused.data["autonomy"]["mode"], "assisted");
    assert_eq!(refused.data["autonomy"]["rule"], 5);
    assert!(w.sink.of(EventKind::CommandExecuted).is_empty());
    let decided = &w.sink.of(EventKind::ApprovalDecided)[0];
    assert_eq!(decided.data["answer"], "denied");
    assert_eq!(decided.data["note"], "use o pnpm, não o shell");
}

#[tokio::test(flavor = "multi_thread")]
async fn allowing_for_the_session_covers_the_same_rule_tool_and_command_only() {
    let w = world().await;
    let s = w.session().await;
    let echo = || tool("shell.execute", json!({"command": "echo oi"}));

    let turn = w.send(&s, echo());
    let request = w.request().await;
    assert_eq!(request.request.command.as_deref(), Some("echo oi"));
    w.answer(&request, ApprovalAnswer::ApproveSession, None);
    assert!(turn.await.unwrap().text.contains("✓ shell.execute"));
    assert_eq!(w.autonomy.grants().len(), 1);

    // The same command, in the same session: no question.
    let again = w.say(&s, echo()).await;
    assert!(again.contains("✓ shell.execute"), "{again}");
    assert_eq!(w.sink.of(EventKind::ApprovalRequested).len(), 1);

    // Another command asks again.
    let other = w.send(&s, tool("shell.execute", json!({"command": "echo tchau"})));
    let request = w.request().await;
    w.answer(&request, ApprovalAnswer::Approve, None);
    other.await.unwrap();

    // Another session asks again.
    let s2 = w.session().await;
    let turn = w.send(&s2, echo());
    let request = w.request().await;
    assert_eq!(request.request.session_id, s2);
    w.answer(&request, ApprovalAnswer::Approve, None);
    turn.await.unwrap();

    // Revoked, it asks again.
    let grant = w.autonomy.grants()[0].id.clone();
    assert!(w.autonomy.revoke(&grant));
    let turn = w.send(&s, echo());
    let request = w.request().await;
    w.answer(&request, ApprovalAnswer::Approve, None);
    turn.await.unwrap();
    assert_eq!(w.sink.of(EventKind::ApprovalRequested).len(), 4);
}

#[tokio::test(flavor = "multi_thread")]
async fn autonomous_follows_the_users_rules_and_denies_on_the_spot() {
    let w = world().await;
    w.mode(AutonomyMode::Autonomous);
    w.autonomy
        .save_rules(
            &[
                PolicyRule::tools(&["shell.execute"], Decision::Deny)
                    .with_command("curl *")
                    .with_note("sem rede"),
                PolicyRule::tools(&[], Decision::Allow),
            ],
            CallOrigin::User,
        )
        .unwrap();
    let s = w.session().await;

    let wrote = w
        .say(
            &s,
            tool(
                "filesystem.write",
                json!({"path": "src/a.ts", "content": "x"}),
            ),
        )
        .await;
    assert!(wrote.contains("✓ filesystem.write"), "{wrote}");

    let denied = w
        .say(
            &s,
            tool(
                "shell.execute",
                json!({"command": "echo a && curl example.com"}),
            ),
        )
        .await;
    assert!(denied.contains("Denied"), "{denied}");
    assert!(denied.contains("regra 1"), "{denied}");
    assert!(denied.contains("sem rede"), "{denied}");
    assert!(w.sink.of(EventKind::ApprovalRequested).is_empty());
    assert!(w.sink.of(EventKind::CommandExecuted).is_empty());

    let changed = w.sink.of(EventKind::AutonomyChanged);
    assert_eq!(changed.len(), 2);
    assert_eq!(changed[0].data["scope"], "project");
    assert_eq!(changed[1].data["scope"], "rules");
}

#[tokio::test(flavor = "multi_thread")]
async fn unrestricted_evaluates_nothing() {
    let w = world().await;
    w.mode(AutonomyMode::Unrestricted);
    // Even a rule list that denies everything is not looked at.
    w.autonomy
        .save_rules(
            &[PolicyRule::tools(&["*"], Decision::Deny)],
            CallOrigin::User,
        )
        .unwrap();
    let s = w.session().await;
    let outside = w.dir.path().join("fora.txt");
    let text = w
        .say(
            &s,
            tool(
                "filesystem.write",
                json!({"path": outside, "content": "livre"}),
            ),
        )
        .await;
    assert!(text.contains("✓ filesystem.write"), "{text}");
    let text = w
        .say(&s, tool("filesystem.read", json!({"path": ".env"})))
        .await;
    assert!(text.contains("segredo"), "{text}");
    let text = w
        .say(&s, tool("filesystem.delete", json!({"path": outside})))
        .await;
    assert!(text.contains("✓ filesystem.delete"), "{text}");
    assert!(!outside.exists());
    assert!(w.sink.of(EventKind::ApprovalRequested).is_empty());
    // Observability is not a restriction: everything is still recorded.
    assert!(w.sink.of(EventKind::ToolCalled).len() >= 3);
    assert!(!w.sink.of(EventKind::FileChanged).is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn cancelling_the_turn_withdraws_the_request() {
    let w = world().await;
    let s = w.session().await;
    let turn = w.send(
        &s,
        tool("filesystem.write", json!({"path": "x.txt", "content": "x"})),
    );
    w.request().await;
    w.sessions.cancel(&s).await.unwrap();
    let result = turn.await.unwrap();
    assert_eq!(result.status, TurnStatus::Cancelled, "{result:?}");
    assert!(w.autonomy.pending().is_empty());
    assert!(!w.root.join("x.txt").exists());
    let decided = &w.sink.of(EventKind::ApprovalDecided)[0];
    assert_eq!(decided.data["answer"], "cancelled");
    let refused = w
        .sink
        .of(EventKind::ToolCalled)
        .into_iter()
        .find(|e| e.data["tool"] == "filesystem.write")
        .unwrap();
    assert_eq!(refused.data["error"]["kind"], "CANCELLED");
}

#[tokio::test(flavor = "multi_thread")]
async fn changing_the_mode_settles_the_waiting_requests() {
    let w = world().await;
    let s = w.session().await;
    let turn = w.send(
        &s,
        tool(
            "filesystem.write",
            json!({"path": "liberado.txt", "content": "ok"}),
        ),
    );
    w.request().await;
    // Whoever grants Unrestricted does not approve the queue one by one.
    w.mode(AutonomyMode::Unrestricted);
    let text = turn.await.unwrap().text;
    assert!(text.contains("✓ filesystem.write"), "{text}");
    assert!(w.root.join("liberado.txt").exists());
    assert!(w.autonomy.pending().is_empty());
    let decided = &w.sink.of(EventKind::ApprovalDecided)[0];
    assert_eq!(decided.data["by"], "rules");

    // And a rule that now denies denies what was waiting.
    w.mode(AutonomyMode::Autonomous);
    w.autonomy
        .save_rules(&[PolicyRule::tools(&[], Decision::Ask)], CallOrigin::User)
        .unwrap();
    let turn = w.send(
        &s,
        tool(
            "filesystem.write",
            json!({"path": "nao.txt", "content": "x"}),
        ),
    );
    w.request().await;
    w.autonomy
        .save_rules(&[PolicyRule::tools(&[], Decision::Deny)], CallOrigin::User)
        .unwrap();
    let text = turn.await.unwrap().text;
    assert!(text.contains("Denied"), "{text}");
    assert!(!w.root.join("nao.txt").exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn pause_holds_every_ai_call_until_resume() {
    let w = world().await;
    w.mode(AutonomyMode::Unrestricted);
    let s = w.session().await;
    assert!(w.autonomy.pause_all(CallOrigin::User));
    let turn = w.send(
        &s,
        tool("filesystem.write", json!({"path": "p.txt", "content": "p"})),
    );
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(!turn.is_finished(), "a paused AI does not act");
    assert!(!w.root.join("p.txt").exists());
    assert!(w.autonomy.resume_all(CallOrigin::User));
    let text = turn.await.unwrap().text;
    assert!(text.contains("✓ filesystem.write"), "{text}");
    assert!(w.root.join("p.txt").exists());
    assert_eq!(w.sink.of(EventKind::ExecutionPaused).len(), 1);
    assert_eq!(w.sink.of(EventKind::ExecutionResumed).len(), 1);

    // Cancelling a paused turn releases it without executing.
    w.autonomy.pause_all(CallOrigin::User);
    let turn = w.send(
        &s,
        tool("filesystem.write", json!({"path": "q.txt", "content": "q"})),
    );
    tokio::time::sleep(Duration::from_millis(100)).await;
    w.sessions.cancel(&s).await.unwrap();
    let result = turn.await.unwrap();
    assert_eq!(result.status, TurnStatus::Cancelled);
    assert!(!w.root.join("q.txt").exists());
    w.autonomy.resume_all(CallOrigin::User);
}

#[tokio::test(flavor = "multi_thread")]
async fn the_user_is_never_gated() {
    let w = world().await;
    let result = w
        .gate
        .execute(ToolCall::new(
            "filesystem.write",
            json!({"path": "do-usuario.txt", "content": "meu"}),
            CallOrigin::User,
        ))
        .await;
    assert!(result.ok, "{result:?}");
    assert!(w.autonomy.pending().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn trying_rules_shows_which_one_decides() {
    let w = world().await;
    let draft = vec![
        PolicyRule::tools(&[], Decision::Allow).with_command("npm test*"),
        PolicyRule::tools(&[], Decision::Ask).with_command("rm *"),
        PolicyRule::tools(&[], Decision::Allow),
    ];
    let trial = w
        .autonomy
        .trial(
            Some(&w.project_id),
            Some(AutonomyMode::Autonomous),
            Some(draft),
            "shell.execute",
            &json!({"command": "npm test && rm -rf dist"}),
        )
        .unwrap();
    assert_eq!(trial.decision, Decision::Ask);
    assert_eq!(trial.rule, Some(2));
    assert_eq!(trial.targets.len(), 2);
    assert_eq!(trial.targets[0].rule, Some(1));
    assert!(trial.reason.contains("regra 2"), "{}", trial.reason);

    let unrestricted = w
        .autonomy
        .trial(
            Some(&w.project_id),
            Some(AutonomyMode::Unrestricted),
            None,
            "filesystem.delete",
            &json!({"path": "/"}),
        )
        .unwrap();
    assert_eq!(unrestricted.decision, Decision::Allow);
    assert!(unrestricted.targets.is_empty());

    let overview = w.autonomy.overview(Some(&w.project_id));
    assert_eq!(overview.mode, AutonomyMode::Assisted);
    assert_eq!(overview.project_mode, None);
    assert_eq!(overview.assisted_rules.len(), 5);
}
