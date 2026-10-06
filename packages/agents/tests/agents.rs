//! The Agent Manager end to end (ADR-0015): an agent executing a task in a
//! real project (Git, database, Tool Runtime), the turn ceiling with its
//! automatic handoff, the file locks between two agents and delegation.

use async_trait::async_trait;
use orchestrator_agents::{
    AgentDeps, AgentService, AgentSettings, AgentSlot, AgentTools, LockManager, StartAgent,
};
use orchestrator_core::{
    Agent, AgentId, AgentStatus, ApprovalAnswer, AuditEvent, AutonomyMode, CallOrigin, EventKind,
    EventSink, FileLock, ProviderId, SessionId, StreamEvent, TaskInput, TaskPriority, TaskStatus,
    TokenUsage, ToolCall, ToolDefinition, ToolErrorKind, ToolResult,
};
use orchestrator_engine::{
    AutonomyGate, AutonomyService, ContextBuilder, EngineTools, HandoffService, StoreSessions,
    TaskService,
};
use orchestrator_memory::MemoryStore;
use orchestrator_providers::{
    AIProvider, ManagerConfig, NativeSession, ProviderCapabilities, ProviderDescriptor,
    ProviderError, ProviderRegistry, ProviderStatus, SessionManager, SessionSpec, ToolExecutor,
    TurnContext, TurnInput, TurnOutput,
};
use orchestrator_runtime::{RuntimeConfig, ToolRuntime};
use parking_lot::Mutex;
use serde_json::json;
use std::collections::HashMap;
use std::path::Path;
use std::process::Command;
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

/// What the AI of a session does, decided by its first message and kept
/// for the rest of the session (the later turns only say "continue").
#[derive(Clone, Copy, PartialEq, Debug)]
enum Script {
    /// Works on the file and reports the result.
    Work,
    /// Never calls `agent.finish`: the ceiling has to stop it.
    Forever,
    /// Writes a file another agent holds.
    Intruder,
    /// Splits the work and ends.
    Delegate,
    /// Works slowly until the turn is cancelled (scheduling tests).
    Slow,
}

/// A provider that acts as the agent asks, so the manager can be tested
/// without a real AI.
#[derive(Default)]
struct Scripted {
    scripts: Mutex<HashMap<String, Script>>,
    /// What the intruder was told when it tried the locked file.
    denial: Mutex<Option<(ToolErrorKind, String)>>,
}

fn classify(text: &str) -> Script {
    if text.contains("sem fim") {
        Script::Forever
    } else if text.contains("mesmo arquivo") {
        Script::Intruder
    } else if text.contains("Dividir") {
        Script::Delegate
    } else if text.contains("devagar") {
        Script::Slow
    } else {
        Script::Work
    }
}

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
        native: &NativeSession,
        input: &TurnInput,
        ctx: &TurnContext,
    ) -> Result<TurnOutput, ProviderError> {
        let script = *self
            .scripts
            .lock()
            .entry(native.reference.clone())
            .or_insert_with(|| classify(&input.text));
        match script {
            Script::Work => {
                let wrote = ctx
                    .call_tool(
                        "filesystem.write",
                        json!({"path": "src/pay.ts", "content": "export const pay = 2;"}),
                    )
                    .await;
                assert!(wrote.ok, "{wrote:?}");
                let done = ctx
                    .call_tool(
                        "agent.finish",
                        json!({"result": "Retentativas aplicadas em src/pay.ts."}),
                    )
                    .await;
                assert!(done.ok, "{done:?}");
                Ok(TurnOutput {
                    text: "pronto".into(),
                })
            }
            Script::Forever => {
                // Slow enough for the test to see it working; each turn
                // costs a cent (ADR-0018).
                tokio::time::sleep(Duration::from_millis(20)).await;
                ctx.report_usage(TokenUsage {
                    input_tokens: 1_000,
                    output_tokens: 100,
                    cost_usd: Some(0.01),
                    ..Default::default()
                });
                Ok(TurnOutput {
                    text: "ainda trabalhando".into(),
                })
            }
            Script::Slow => {
                let deadline = Instant::now() + Duration::from_secs(20);
                while !ctx.is_cancelled() && Instant::now() < deadline {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
                Ok(TurnOutput {
                    text: "parei".into(),
                })
            }
            Script::Intruder => {
                let wrote = ctx
                    .call_tool(
                        "filesystem.write",
                        json!({"path": "src/pay.ts", "content": "invadido"}),
                    )
                    .await;
                let error = wrote.error.clone().expect("a trava recusa a escrita");
                *self.denial.lock() = Some((error.kind, error.message.clone()));
                let done = ctx
                    .call_tool(
                        "agent.finish",
                        json!({"result": format!("não mexi no arquivo: {}", error.message)}),
                    )
                    .await;
                assert!(done.ok, "{done:?}");
                Ok(TurnOutput {
                    text: "recuei".into(),
                })
            }
            Script::Delegate => {
                let sub = ctx
                    .call_tool(
                        "agent.delegate",
                        json!({
                            "title": "Cobrir o webhook com testes",
                            "description": "Escrever os testes do webhook.",
                            "files": ["tests/webhook.test.ts"],
                        }),
                    )
                    .await;
                let result = match &sub.error {
                    None => "dividi o trabalho".to_owned(),
                    Some(error) => {
                        *self.denial.lock() = Some((error.kind, error.message.clone()));
                        format!("não deleguei: {}", error.message)
                    }
                };
                let done = ctx
                    .call_tool("agent.finish", json!({"result": result}))
                    .await;
                assert!(done.ok, "{done:?}");
                Ok(TurnOutput {
                    text: "dividido".into(),
                })
            }
        }
    }
}

struct World {
    _dir: TempDir,
    root: std::path::PathBuf,
    project_id: String,
    store: Arc<MemoryStore>,
    sink: Arc<StoreSink>,
    provider: Arc<Scripted>,
    tasks: TaskService,
    agents: AgentService,
    locks: Arc<LockManager>,
    autonomy: AutonomyService,
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
    let root = dir.path().to_path_buf();
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/pay.ts"), "export const pay = 1;").unwrap();
    std::fs::write(root.join("README.md"), "# loja\n").unwrap();
    git(&root, &["init", "-q"]);
    git(&root, &["symbolic-ref", "HEAD", "refs/heads/main"]);
    git(&root, &["config", "user.email", "t@example.com"]);
    git(&root, &["config", "user.name", "Teste"]);
    git(&root, &["config", "commit.gpgsign", "false"]);
    git(&root, &["add", "."]);
    git(&root, &["commit", "-q", "-m", "início"]);

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
    let project_id = store.current_project().expect("projeto registrado").id;

    let registry = Arc::new(ProviderRegistry::new(sink.clone()));
    let provider = Arc::new(Scripted::default());
    registry.register(provider.clone()).unwrap();

    let locks = Arc::new(LockManager::new(store.clone()));
    let slot = AgentSlot::new();
    // The app's chain, gate included. The tests of the Phase 8b behaviour
    // run in Unrestricted; the ones about autonomy give the agent a mode.
    let (autonomy, _) = AutonomyService::new(store.clone(), sink.clone(), None);
    autonomy
        .set_default_mode(AutonomyMode::Unrestricted, CallOrigin::User)
        .unwrap();
    let base = runtime.clone();
    autonomy.set_workdir(Arc::new(move || base.base_dir()));
    let tools = AutonomyGate::new(
        Arc::new(AgentTools::new(
            Arc::new(EngineTools::new(
                Arc::new(RuntimeTools(runtime.clone())),
                store.clone(),
                sink.clone(),
            )),
            store.clone(),
            locks.clone(),
            slot.clone(),
            sink.clone(),
        )),
        autonomy.clone(),
    );
    let sessions = SessionManager::with_store(
        registry,
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
    let tasks = TaskService::new(
        sessions.clone(),
        store.clone(),
        builder.clone(),
        sink.clone(),
    );
    let handoffs = HandoffService::new(sessions.clone(), store.clone(), builder, sink.clone());
    let (agents, warning) = AgentService::new(
        AgentDeps {
            sessions,
            store: store.clone(),
            tasks: tasks.clone(),
            handoffs,
            locks: locks.clone(),
            autonomy: autonomy.clone(),
            sink: sink.clone(),
        },
        None,
    );
    assert!(warning.is_none());
    slot.install(agents.clone());

    World {
        _dir: dir,
        root,
        project_id,
        store,
        sink,
        provider,
        tasks,
        agents,
        locks,
        autonomy,
    }
}

impl World {
    fn task(&self, title: &str, files: &[&str]) -> orchestrator_core::Task {
        self.tasks
            .save(
                TaskInput {
                    project_id: Some(self.project_id.clone()),
                    title: Some(title.into()),
                    files: Some(files.iter().map(|f| (*f).to_owned()).collect()),
                    ..Default::default()
                },
                CallOrigin::User,
            )
            .expect("task criada")
    }

    /// Waits for an agent to reach a final state (or for the time to run
    /// out, which fails the test with what it was doing).
    async fn wait_final(&self, id: &AgentId) -> Agent {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            let agent = self.store.agent(id.as_str()).expect("agente no banco");
            if agent.status.is_final() {
                return agent;
            }
            assert!(
                Instant::now() < deadline,
                "o agente não terminou: {:?}",
                agent.status
            );
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }

    async fn wait_until(&self, what: &str, mut ready: impl FnMut(&World) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(20);
        while !ready(self) {
            assert!(Instant::now() < deadline, "tempo esgotado esperando {what}");
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }
}

#[tokio::test]
async fn an_agent_executes_a_task_and_leaves_it_for_review() {
    let world = world().await;
    let task = world.task("Aplicar retentativas no pagamento", &["src/pay.ts"]);

    let agent = world
        .agents
        .start(StartAgent {
            task_id: task.id.clone(),
            provider: None,
            model: None,
            max_turns: None,
            autonomy: None,
            max_cost_usd: None,
        })
        .expect("agente na fila");
    assert_eq!(agent.status, AgentStatus::Queued);
    assert_eq!(agent.session, None);

    let done = world.wait_final(&agent.id).await;
    assert_eq!(done.status, AgentStatus::Done);
    assert_eq!(done.result, "Retentativas aplicadas em src/pay.ts.");
    assert_eq!(done.turns, 1);
    assert!(done.session.is_some(), "o agente trabalha numa sessão");
    // It received the project context built from its task, and the tools.
    let context = done.context.expect("contexto do primeiro turno");
    assert!(context.tokens > 0 && !context.sections.is_empty());
    assert!(done.tools.iter().any(|t| t == "agent.finish"));
    assert!(done.tools.iter().any(|t| t == "memory.search"));

    // The work is on the task, and a person reviews it: the agent does not
    // conclude anything.
    let task = world.store.task(task.id.as_str()).unwrap();
    assert_eq!(task.status, TaskStatus::Review);
    assert_eq!(task.result, "Retentativas aplicadas em src/pay.ts.");
    assert_eq!(task.sessions.len(), 1);
    // It really changed the project.
    let written = std::fs::read_to_string(world.root.join("src/pay.ts")).unwrap();
    assert_eq!(written, "export const pay = 2;");
    // Nothing stays locked, and no handoff is needed when it finished.
    assert!(world.locks.list(Some(&world.project_id)).is_empty());
    assert_eq!(done.handoff, None);

    let started = world.sink.of(EventKind::AgentStarted);
    let finished = world.sink.of(EventKind::AgentFinished);
    assert_eq!((started.len(), finished.len()), (1, 1));
    assert_eq!(finished[0].data["outcome"], json!("DONE"));
    assert_eq!(finished[0].data["turns"], json!(1));
    // Its own tools are in the history like any other (ADR-0016).
    let finish = world
        .sink
        .of(EventKind::ToolCalled)
        .into_iter()
        .find(|e| e.data["tool"] == "agent.finish")
        .expect("TOOL_CALLED do agent.finish");
    assert_eq!(finish.data["ok"], json!(true));
}

#[tokio::test]
async fn the_turn_ceiling_stops_an_agent_and_leaves_a_handoff() {
    let world = world().await;
    let task = world.task("Tarefa sem fim", &[]);

    let agent = world
        .agents
        .start(StartAgent {
            task_id: task.id.clone(),
            provider: None,
            model: None,
            max_turns: Some(2),
            autonomy: None,
            max_cost_usd: None,
        })
        .expect("agente na fila");
    let ended = world.wait_final(&agent.id).await;

    assert_eq!(ended.status, AgentStatus::Failed);
    assert_eq!(ended.turns, 2, "parou no teto, não antes nem depois");
    assert!(
        ended.error.as_deref().unwrap_or_default().contains("teto"),
        "{:?}",
        ended.error
    );
    // It stopped mid-work, so another AI can pick it up.
    let handoff_id = ended.handoff.expect("handoff automático");
    let handoff = world.store.handoff(handoff_id.as_str()).expect("handoff");
    assert!(!handoff.by_agent, "os fatos, sem gastar um turno de IA");
    assert!(!handoff.packet.goal.is_empty());
    assert!(handoff.packet.status.contains("O agente parou"));
    // The task is still open work, and nothing stayed locked.
    let task = world.store.task(task.id.as_str()).unwrap();
    assert_eq!(task.status, TaskStatus::InProgress);
    assert!(world.locks.list(None).is_empty());
}

#[tokio::test]
async fn two_agents_never_change_the_same_file() {
    let world = world().await;
    // An agent at work, holding the file while it works.
    let held = world.task("Reescrever o pagamento, tarefa sem fim", &["src/pay.ts"]);
    let owner = world
        .agents
        .start(StartAgent {
            task_id: held.id.clone(),
            provider: None,
            model: None,
            max_turns: Some(500),
            autonomy: None,
            max_cost_usd: None,
        })
        .unwrap();
    world
        .wait_until("o primeiro agente travar o arquivo", |w| {
            w.locks
                .list(Some(&w.project_id))
                .iter()
                .any(|l| l.path == "src/pay.ts")
        })
        .await;

    // A task that declares the same file does not even start.
    let queued_task = world.task("Outra coisa no pagamento", &["src/pay.ts"]);
    let queued = world
        .agents
        .start(StartAgent {
            task_id: queued_task.id.clone(),
            provider: None,
            model: None,
            max_turns: Some(2),
            autonomy: None,
            max_cost_usd: None,
        })
        .unwrap();
    let view = world.agents.get(&queued.id).expect("agente na fila");
    assert_eq!(view.agent.status, AgentStatus::Queued);
    assert_eq!(view.task_title, "Outra coisa no pagamento");
    assert!(
        view.waiting
            .clone()
            .unwrap_or_default()
            .contains("src/pay.ts"),
        "a fila diz o que está no caminho: {:?}",
        view.waiting
    );
    assert!(world
        .store
        .agent(queued.id.as_str())
        .unwrap()
        .session
        .is_none());

    // The user stops the first one (section 11 of the master document).
    world
        .agents
        .stop(&owner.id, CallOrigin::User)
        .await
        .expect("parar o agente");
    let owner = world.wait_final(&owner.id).await;
    assert_eq!(owner.status, AgentStatus::Stopped);
    assert!(owner.handoff.is_some(), "quem para no meio deixa handoff");

    // With the file free, the queue moves on its own.
    let moved = world.wait_final(&queued.id).await;
    assert_eq!(moved.status, AgentStatus::Done);
    assert!(moved.session.is_some());
    assert!(world.locks.list(None).is_empty());
}

#[tokio::test]
async fn a_write_on_a_file_another_agent_holds_is_refused_with_the_reason() {
    let world = world().await;
    // An agent at work holding `src/pay.ts`, put there by hand so the test
    // does not depend on two agents racing.
    let owner_task = world.task("Dono do arquivo", &["src/pay.ts"]);
    let owner = Agent {
        id: AgentId::new(),
        project_id: world.project_id.clone(),
        task: owner_task.id.clone(),
        title: "Dono do arquivo".into(),
        provider: ProviderId::from("nuvem"),
        model: None,
        session: Some(SessionId::from("s-dono")),
        parent_agent: None,
        status: AgentStatus::Running,
        tools: Vec::new(),
        context: None,
        turns: 0,
        max_turns: 12,
        files: Vec::new(),
        result: String::new(),
        error: None,
        handoff: None,
        autonomy: None,
        max_cost_usd: None,
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
        started_at: Some(chrono::Utc::now()),
        finished_at: None,
    };
    world.store.agent_save(&owner).unwrap();
    assert!(world
        .locks
        .take(&owner, &world.root, &["src/pay.ts".to_owned()])
        .is_none());

    // The other agent does not declare the file: it tries to write it in
    // the middle of the turn.
    let task = world.task("Mexer no mesmo arquivo sem avisar", &[]);
    let intruder = world
        .agents
        .start(StartAgent {
            task_id: task.id.clone(),
            provider: None,
            model: None,
            max_turns: Some(2),
            autonomy: None,
            max_cost_usd: None,
        })
        .unwrap();
    let ended = world.wait_final(&intruder.id).await;
    assert_eq!(ended.status, AgentStatus::Done);

    let (kind, message) = world.provider.denial.lock().clone().expect("recusa");
    assert_eq!(kind, ToolErrorKind::Locked);
    assert!(
        message.contains("src/pay.ts") && message.contains("Dono do arquivo"),
        "{message}"
    );
    // The file is untouched and the intruder never took the lock.
    let content = std::fs::read_to_string(world.root.join("src/pay.ts")).unwrap();
    assert_eq!(content, "export const pay = 1;");
    assert!(!ended.files.iter().any(|f| f == "src/pay.ts"));
    // The owner still has it: a refusal does not move a lock.
    let locks: Vec<FileLock> = world.locks.list(Some(&world.project_id));
    assert_eq!(locks.len(), 1);
    assert_eq!(locks[0].agent_id, owner.id);
    // And the refusal is in the history, not only in the transcript.
    let refused = world
        .sink
        .of(EventKind::ToolCalled)
        .into_iter()
        .find(|e| e.data["tool"] == "filesystem.write" && e.data["ok"] == json!(false))
        .expect("a recusa da trava fica no histórico");
    assert_eq!(refused.data["error"]["kind"], json!("LOCKED"));
}

#[tokio::test]
async fn an_agent_delegates_a_subtask_to_a_subagent() {
    let world = world().await;
    world
        .agents
        .save_settings(AgentSettings {
            max_parallel: 1,
            max_turns: 4,
            ..Default::default()
        })
        .unwrap();
    let task = world.task("Dividir o trabalho do webhook", &[]);
    let parent = world
        .agents
        .start(StartAgent {
            task_id: task.id.clone(),
            provider: None,
            model: None,
            max_turns: None,
            autonomy: Some(AutonomyMode::Autonomous),
            max_cost_usd: None,
        })
        .unwrap();
    let parent = world.wait_final(&parent.id).await;
    assert_eq!(parent.status, AgentStatus::Done);

    // The subtask hangs from the task, and its agent from the agent.
    let subtask = world
        .store
        .tasks_list(Some(&world.project_id))
        .into_iter()
        .find(|t| t.parent_task.as_ref() == Some(&task.id))
        .expect("subtask criada");
    assert_eq!(subtask.title, "Cobrir o webhook com testes");
    assert_eq!(subtask.files, ["tests/webhook.test.ts"]);

    let sub = world
        .store
        .agents_list(Some(&world.project_id))
        .into_iter()
        .find(|a| a.parent_agent.as_ref() == Some(&parent.id))
        .expect("subagente criado");
    assert_eq!(sub.task, subtask.id);
    // The mode granted to the parent is for its line of work.
    assert_eq!(sub.autonomy, Some(AutonomyMode::Autonomous));
    // With one slot, the subagent only started once the parent was done.
    let sub = world.wait_final(&sub.id).await;
    assert_eq!(sub.status, AgentStatus::Done);
    assert_eq!(
        world.store.task(subtask.id.as_str()).unwrap().status,
        TaskStatus::Review
    );
}

fn start(
    world: &World,
    task: &orchestrator_core::Task,
    max_turns: Option<u32>,
    autonomy: Option<AutonomyMode>,
) -> Agent {
    world
        .agents
        .start(StartAgent {
            task_id: task.id.clone(),
            provider: None,
            model: None,
            max_turns,
            autonomy,
            max_cost_usd: None,
        })
        .expect("agente na fila")
}

#[tokio::test]
async fn an_assisted_agent_waits_for_the_users_authorization() {
    let world = world().await;
    let task = world.task("Aplicar retentativas", &["src/pay.ts"]);
    let agent = start(&world, &task, None, Some(AutonomyMode::Assisted));

    world
        .wait_until("o pedido de autorização", |w| {
            !w.autonomy.pending().is_empty()
        })
        .await;
    let request = world.autonomy.pending().remove(0);
    assert_eq!(request.request.agent_id.as_ref(), Some(&agent.id));
    assert_eq!(
        request.request.agent_title.as_deref(),
        Some("Aplicar retentativas")
    );
    assert_eq!(request.request.task_id.as_ref(), Some(&task.id));
    assert_eq!(request.request.summary, "Escrever src/pay.ts (21 B)");
    let view = world.agents.get(&agent.id).unwrap();
    assert_eq!(view.agent.status, AgentStatus::Running);
    assert_eq!(view.approval.as_deref(), Some("Escrever src/pay.ts (21 B)"));
    assert_eq!(view.mode, AutonomyMode::Assisted);
    // Nothing happens before the answer.
    let content = std::fs::read_to_string(world.root.join("src/pay.ts")).unwrap();
    assert_eq!(content, "export const pay = 1;");

    world
        .autonomy
        .answer(
            &request.request.id,
            ApprovalAnswer::Approve,
            None,
            CallOrigin::User,
        )
        .unwrap();
    let done = world.wait_final(&agent.id).await;
    assert_eq!(done.status, AgentStatus::Done);
    let content = std::fs::read_to_string(world.root.join("src/pay.ts")).unwrap();
    assert_eq!(content, "export const pay = 2;");
    // Delivering the result is not an action that needs a yes.
    assert_eq!(world.sink.of(EventKind::ApprovalRequested).len(), 1);
    let started = &world.sink.of(EventKind::AgentStarted)[0];
    assert_eq!(started.data["mode"], json!("assisted"));
    assert_eq!(started.data["autonomy"], json!("assisted"));
}

#[tokio::test]
async fn stopping_an_agent_withdraws_what_it_was_waiting_for() {
    let world = world().await;
    let task = world.task("Aplicar retentativas", &["src/pay.ts"]);
    let agent = start(&world, &task, None, Some(AutonomyMode::Assisted));
    world
        .wait_until("o pedido de autorização", |w| {
            !w.autonomy.pending().is_empty()
        })
        .await;
    world
        .agents
        .stop(&agent.id, CallOrigin::User)
        .await
        .unwrap();
    let ended = world.wait_final(&agent.id).await;
    assert_eq!(ended.status, AgentStatus::Stopped);
    assert!(world.autonomy.pending().is_empty());
    let decided = &world.sink.of(EventKind::ApprovalDecided)[0];
    assert_eq!(decided.data["answer"], json!("cancelled"));
    let content = std::fs::read_to_string(world.root.join("src/pay.ts")).unwrap();
    assert_eq!(content, "export const pay = 1;");
    assert!(world.locks.list(None).is_empty());
}

#[tokio::test]
async fn a_paused_agent_holds_its_next_turn_until_resumed() {
    let world = world().await;
    let task = world.task("Tarefa sem fim", &[]);
    let agent = start(&world, &task, Some(500), None);
    world
        .wait_until("dois turnos", |w| {
            w.store.agent(agent.id.as_str()).unwrap().turns >= 2
        })
        .await;

    let view = world.agents.pause(&agent.id, CallOrigin::User).unwrap();
    assert!(view.paused);
    // The turn in flight ends; no other one begins.
    tokio::time::sleep(Duration::from_millis(150)).await;
    let held = world.store.agent(agent.id.as_str()).unwrap().turns;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let still = world.store.agent(agent.id.as_str()).unwrap();
    assert_eq!(still.turns, held, "um agente pausado não começa turno");
    assert_eq!(
        still.status,
        AgentStatus::Running,
        "pausado não é encerrado"
    );

    let view = world.agents.resume(&agent.id, CallOrigin::User).unwrap();
    assert!(!view.paused);
    world
        .wait_until("o agente voltar a trabalhar", |w| {
            w.store.agent(agent.id.as_str()).unwrap().turns > held
        })
        .await;
    world
        .agents
        .stop(&agent.id, CallOrigin::User)
        .await
        .unwrap();
    assert_eq!(
        world.wait_final(&agent.id).await.status,
        AgentStatus::Stopped
    );

    let paused = world.sink.of(EventKind::ExecutionPaused);
    assert_eq!(paused.len(), 1);
    assert_eq!(paused[0].data["scope"], json!("agent"));
    assert_eq!(world.sink.of(EventKind::ExecutionResumed).len(), 1);

    // Only a running agent can be paused.
    let queued_task = world.task("Outra", &[]);
    world.agents.pause_all(CallOrigin::User);
    let queued = start(&world, &queued_task, None, None);
    assert!(world.agents.pause(&queued.id, CallOrigin::User).is_err());
    world
        .agents
        .stop(&queued.id, CallOrigin::User)
        .await
        .unwrap();
    world.agents.resume_all(CallOrigin::User);
}

#[tokio::test]
async fn while_the_ais_are_paused_the_queue_waits() {
    let world = world().await;
    assert!(world.agents.pause_all(CallOrigin::User));
    let task = world.task("Aplicar retentativas", &["src/pay.ts"]);
    let agent = start(&world, &task, None, None);
    tokio::time::sleep(Duration::from_millis(200)).await;
    let view = world.agents.get(&agent.id).unwrap();
    assert_eq!(view.agent.status, AgentStatus::Queued);
    assert_eq!(view.waiting.as_deref(), Some("as IAs estão pausadas"));

    assert!(world.agents.resume_all(CallOrigin::User));
    let done = world.wait_final(&agent.id).await;
    assert_eq!(done.status, AgentStatus::Done);
}

// ---- scheduling (ADR-0018) --------------------------------------------

impl World {
    fn task_with(&self, title: &str, priority: TaskPriority) -> orchestrator_core::Task {
        self.tasks
            .save(
                TaskInput {
                    project_id: Some(self.project_id.clone()),
                    title: Some(title.into()),
                    priority: Some(priority),
                    ..Default::default()
                },
                CallOrigin::User,
            )
            .expect("task criada")
    }

    fn settings(&self, change: impl FnOnce(&mut AgentSettings)) {
        let mut settings = self.agents.settings();
        change(&mut settings);
        self.agents.save_settings(settings).unwrap();
    }

    fn status(&self, id: &AgentId) -> AgentStatus {
        self.store.agent(id.as_str()).unwrap().status
    }
}

#[tokio::test]
async fn the_queue_follows_priority_and_the_provider_limit() {
    let world = world().await;
    world.settings(|s| {
        s.max_parallel = 3;
        s.provider_limits.insert("nuvem".into(), 1);
    });
    let first = start(
        &world,
        &world.task_with("devagar: primeiro", TaskPriority::Normal),
        None,
        None,
    );
    world
        .wait_until("o primeiro rodando", |w| {
            w.status(&first.id) == AgentStatus::Running
        })
        .await;
    let low = start(
        &world,
        &world.task_with("devagar: baixa", TaskPriority::Low),
        None,
        None,
    );
    let urgent = start(
        &world,
        &world.task_with("devagar: urgente", TaskPriority::Urgent),
        None,
        None,
    );

    // One slot for "nuvem": both wait, the urgent one first, even though
    // it arrived last; the reason names the provider's limit.
    let view = world.agents.get(&urgent.id).unwrap();
    assert_eq!(view.queue_position, Some(1));
    assert!(view
        .waiting
        .as_deref()
        .unwrap()
        .contains("de nuvem em execução (o limite dele é 1)"));
    assert_eq!(world.agents.get(&low.id).unwrap().queue_position, Some(2));
    assert_eq!(world.agents.get(&first.id).unwrap().queue_position, None);

    world
        .agents
        .stop(&first.id, CallOrigin::User)
        .await
        .unwrap();
    world
        .wait_until("o urgente rodando", |w| {
            w.status(&urgent.id) == AgentStatus::Running
        })
        .await;
    assert_eq!(
        world.status(&low.id),
        AgentStatus::Queued,
        "the low one still waits"
    );

    world.agents.stop_all(None, CallOrigin::User).await.unwrap();
    world.wait_final(&urgent.id).await;
    world.wait_final(&low.id).await;
}

#[tokio::test]
async fn a_cost_ceiling_stops_the_agent_with_a_handoff() {
    let world = world().await;
    // A cent per turn: the third turn reaches the ceiling.
    world.settings(|s| s.max_cost_usd = Some(0.025));
    let task = world.task("Trabalho sem fim", &[]);
    let agent = start(&world, &task, Some(20), None);
    assert_eq!(
        agent.max_cost_usd,
        Some(0.025),
        "the setting, kept on the agent"
    );
    let agent = world.wait_final(&agent.id).await;
    assert_eq!(agent.status, AgentStatus::Failed);
    assert_eq!(agent.turns, 3);
    let error = agent.error.unwrap();
    assert!(
        error.contains("teto de custo de US$ 0,025 (gastou US$ 0,030)"),
        "{error}"
    );
    assert!(agent.handoff.is_some(), "the work stays reachable");
    let finished = world.sink.of(EventKind::AgentFinished);
    assert_eq!(finished.last().unwrap().data["reason"], "costCeiling");

    // A ceiling given when the agent starts wins over the setting.
    let task = world.task("Trabalho sem fim de novo", &[]);
    let agent = world
        .agents
        .start(StartAgent {
            task_id: task.id.clone(),
            provider: None,
            model: None,
            max_turns: Some(20),
            autonomy: None,
            max_cost_usd: Some(0.005),
        })
        .unwrap();
    let agent = world.wait_final(&agent.id).await;
    assert_eq!(agent.turns, 1);
}

#[tokio::test]
async fn the_daily_budget_stops_agents_and_holds_the_queue() {
    let world = world().await;
    world.settings(|s| s.daily_budget_usd = Some(0.02));
    let agent = start(&world, &world.task("Trabalho sem fim", &[]), Some(20), None);
    let agent = world.wait_final(&agent.id).await;
    assert_eq!(agent.status, AgentStatus::Failed);
    assert_eq!(agent.turns, 2, "two cents spent, then it stops");
    assert!(agent.error.unwrap().contains("orçamento diário"));
    assert_eq!(
        world.sink.of(EventKind::AgentFinished).last().unwrap().data["reason"],
        "dailyBudget"
    );
    let budget = world.agents.budget(&world.project_id);
    assert!(budget.exhausted);
    assert!((budget.spent_today_usd - 0.02).abs() < 1e-9, "{budget:?}");

    // The next one waits for the budget, with the reason.
    let next = start(
        &world,
        &world.task_with("devagar: depois", TaskPriority::Normal),
        None,
        None,
    );
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(world.status(&next.id), AgentStatus::Queued);
    let waiting = world.agents.get(&next.id).unwrap().waiting.unwrap();
    assert!(
        waiting.contains("orçamento diário do projeto acabou"),
        "{waiting}"
    );

    // A bigger budget releases it.
    world.settings(|s| s.daily_budget_usd = Some(1.0));
    world
        .wait_until("o agente liberado", |w| {
            w.status(&next.id) == AgentStatus::Running
        })
        .await;
    world.agents.stop(&next.id, CallOrigin::User).await.unwrap();
    world.wait_final(&next.id).await;
}

#[tokio::test]
async fn delegation_can_be_turned_off() {
    let world = world().await;
    world.settings(|s| s.max_subagents = 0);
    let task = world.task("Dividir o trabalho", &[]);
    let parent = start(&world, &task, None, None);
    let parent = world.wait_final(&parent.id).await;
    assert_eq!(parent.status, AgentStatus::Done);
    let (kind, message) = world.provider.denial.lock().clone().unwrap();
    assert_eq!(kind, ToolErrorKind::InvalidArgs);
    assert!(message.contains("delegação está desligada"), "{message}");
    assert!(parent.result.contains("não deleguei"));
}
