//! The Agent Manager end to end (ADR-0015): an agent executing a task in a
//! real project (Git, database, Tool Runtime), the turn ceiling with its
//! automatic handoff, the file locks between two agents and delegation.

use async_trait::async_trait;
use orchestrator_agents::{
    AgentService, AgentSettings, AgentSlot, AgentTools, LockManager, StartAgent,
};
use orchestrator_core::{
    Agent, AgentId, AgentStatus, AuditEvent, CallOrigin, EventKind, EventSink, FileLock,
    ProviderId, SessionId, StreamEvent, TaskInput, TaskStatus, ToolCall, ToolDefinition,
    ToolErrorKind, ToolResult,
};
use orchestrator_engine::{
    ContextBuilder, EngineTools, HandoffService, StoreSessions, TaskService,
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
                // Slow enough for the test to see it working.
                tokio::time::sleep(Duration::from_millis(20)).await;
                Ok(TurnOutput {
                    text: "ainda trabalhando".into(),
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
                assert!(sub.ok, "{sub:?}");
                let done = ctx
                    .call_tool("agent.finish", json!({"result": "dividi o trabalho"}))
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
    let tools = AgentTools::new(
        Arc::new(EngineTools::new(
            Arc::new(RuntimeTools(runtime.clone())),
            store.clone(),
            sink.clone(),
        )),
        store.clone(),
        locks.clone(),
        slot.clone(),
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
        sessions,
        store.clone(),
        tasks.clone(),
        handoffs,
        locks.clone(),
        sink.clone(),
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
}

#[tokio::test]
async fn an_agent_delegates_a_subtask_to_a_subagent() {
    let world = world().await;
    world
        .agents
        .save_settings(AgentSettings {
            max_parallel: 1,
            max_turns: 4,
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
    // With one slot, the subagent only started once the parent was done.
    let sub = world.wait_final(&sub.id).await;
    assert_eq!(sub.status, AgentStatus::Done);
    assert_eq!(
        world.store.task(subtask.id.as_str()).unwrap().status,
        TaskStatus::Review
    );
}
