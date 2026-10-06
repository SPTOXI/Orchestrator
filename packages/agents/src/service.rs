//! Agent Manager (ADR-0015): the queue, the turns and the end of an agent.
//!
//! An agent is one task, one session and one outcome. It is disposable:
//! what it produced is on the task, in the project's memory and in the
//! history — never only here. What it may do is the autonomy gate's
//! decision (ADR-0016), in the mode of its project or the one the user
//! gave it; how much it runs is the turn and parallelism ceilings; and the
//! user can pause it, resume it and stop it.

use crate::locks::LockManager;
use crate::settings::{self, AgentSettings};
use chrono::Utc;
use orchestrator_core::{
    Agent, AgentId, AgentStatus, AuditEvent, AutonomyMode, CallOrigin, EventKind, EventSink,
    ProviderId, SessionEvent, SessionId, TaskId, TaskInput, TaskStatus, TurnStatus,
};
use orchestrator_engine::{
    ApprovalView, AutonomyService, CreateRequest, HandoffService, PrepareRequest, StartTaskSession,
    TaskService,
};
use orchestrator_memory::MemoryStore;
use orchestrator_providers::{ProviderError, SessionManager};
use parking_lot::{Mutex, RwLock};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

/// Characters of a result or of a delegated description.
pub const MAX_RESULT: usize = 4_000;
/// Agent → subagent, and no further (ADR-0015).
pub const MAX_DEPTH: usize = 2;

/// What the Orchestrator tells the agent between turns. Short on purpose:
/// the context already went with the first turn.
const CONTINUE: &str = "Continue a task. Quando ela estiver pronta, chame a ferramenta \
                        `agent.finish` com o que foi feito. Se não der para continuar, chame \
                        `agent.finish` explicando o que falta.";

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StartAgent {
    pub task_id: TaskId,
    /// `None`: the task's provider, or the active one.
    pub provider: Option<ProviderId>,
    pub model: Option<String>,
    /// Turn ceiling for this agent; `None` = the setting.
    pub max_turns: Option<u32>,
    /// Autonomy mode granted to this agent and its subagents; `None`: the
    /// project's (ADR-0016).
    #[serde(default)]
    pub autonomy: Option<AutonomyMode>,
    /// Cost ceiling of this agent (USD); `None`: the setting (ADR-0018).
    #[serde(default)]
    pub max_cost_usd: Option<f64>,
}

/// An agent with what the panel and the board show but the agent does not
/// store.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentView {
    #[serde(flatten)]
    pub agent: Agent,
    /// Why a queued agent has not started ("espera o agente X").
    pub waiting: Option<String>,
    /// Task the agent executes, as the board shows it.
    pub task_title: String,
    pub task_status: TaskStatus,
    /// Held by a pause (its own, or of every AI).
    pub paused: bool,
    /// What it is waiting for the user to authorize, if anything.
    pub approval: Option<String>,
    /// Mode its calls are judged by now.
    pub mode: AutonomyMode,
    /// What its session cost so far (`None`: no price, or not started).
    pub cost_usd: Option<f64>,
    /// 1 = next to start, while queued (ADR-0018).
    pub queue_position: Option<u32>,
}

/// What the AIs of a project spent today, against its daily budget
/// (ADR-0018).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BudgetView {
    /// Since local midnight.
    pub spent_today_usd: f64,
    pub budget_usd: Option<f64>,
    /// Calls with tokens but no price: the real spending is higher.
    pub unpriced: u64,
    /// No agent starts, and running ones stop at their next turn.
    pub exhausted: bool,
}

/// How an agent's work ended.
enum Ending {
    /// `agent.finish`: the agent reported what it did.
    Done(String),
    Failed(String),
    /// A cost ceiling or the project's daily budget (ADR-0018): `reason`
    /// is `costCeiling` or `dailyBudget`.
    Spent {
        reason: &'static str,
        message: String,
    },
    Stopped,
}

struct Live {
    cancel: CancellationToken,
    /// Result reported through `agent.finish`, if it was.
    finish: Mutex<Option<String>>,
}

struct Inner {
    sessions: SessionManager,
    store: Arc<MemoryStore>,
    tasks: TaskService,
    handoffs: HandoffService,
    locks: Arc<LockManager>,
    autonomy: AutonomyService,
    sink: Arc<dyn EventSink>,
    settings: RwLock<AgentSettings>,
    settings_path: Option<PathBuf>,
    live: Mutex<HashMap<AgentId, Arc<Live>>>,
    /// Serializes `pump`.
    pumping: Mutex<()>,
    /// Where agents run. The app sets it, because its commands are not
    /// polled inside the async runtime; tests use the ambient one.
    runtime: Mutex<Option<tokio::runtime::Handle>>,
}

#[derive(Clone)]
pub struct AgentService {
    inner: Arc<Inner>,
}

fn invalid(message: impl Into<String>) -> ProviderError {
    ProviderError::invalid(message)
}

fn not_found(id: &AgentId) -> ProviderError {
    ProviderError::new(
        orchestrator_providers::ProviderErrorKind::NotFound,
        format!("agente {id} não encontrado"),
    )
}

/// Dollars for a message, as the UI writes them: cents, or tenths of a
/// cent below one dollar ("0,104"), with a decimal comma.
fn usd(value: f64) -> String {
    let text = if value.abs() < 1.0 {
        format!("{value:.3}")
    } else {
        format!("{value:.2}")
    };
    text.replace('.', ",")
}

/// Local midnight, as the start of "today" for the daily budget.
fn today_start() -> chrono::DateTime<Utc> {
    let now = chrono::Local::now();
    now.date_naive()
        .and_hms_opt(0, 0, 0)
        .and_then(|midnight| midnight.and_local_timezone(chrono::Local).earliest())
        .map(|midnight| midnight.with_timezone(&Utc))
        .unwrap_or_else(|| Utc::now() - chrono::Duration::hours(24))
}

fn clip(text: &str, max: usize) -> String {
    orchestrator_engine::clip(text, max)
}

/// What the Agent Manager works with, wired by the app.
pub struct AgentDeps {
    pub sessions: SessionManager,
    pub store: Arc<MemoryStore>,
    pub tasks: TaskService,
    pub handoffs: HandoffService,
    pub locks: Arc<LockManager>,
    /// Pause, and the mode an agent's calls are judged by (ADR-0016).
    pub autonomy: AutonomyService,
    pub sink: Arc<dyn EventSink>,
}

impl AgentService {
    /// Reads the settings from `settings_path` (defaults plus a warning
    /// when the file is unusable).
    pub fn new(deps: AgentDeps, settings_path: Option<PathBuf>) -> (Self, Option<String>) {
        let AgentDeps {
            sessions,
            store,
            tasks,
            handoffs,
            locks,
            autonomy,
            sink,
        } = deps;
        let (settings, warning) = match &settings_path {
            Some(path) => settings::load(path),
            None => (AgentSettings::default(), None),
        };
        let service = Self {
            inner: Arc::new(Inner {
                sessions,
                store,
                tasks,
                handoffs,
                locks,
                autonomy,
                sink,
                settings: RwLock::new(settings),
                settings_path,
                live: Mutex::new(HashMap::new()),
                pumping: Mutex::new(()),
                runtime: Mutex::new(None),
            }),
        };
        (service, warning)
    }

    /// Names the async runtime the agents run on. Without it, the service
    /// uses the runtime of whoever calls it, which is enough for tests but
    /// not for the app: a Tauri command is not polled inside one.
    pub fn set_runtime(&self, handle: tokio::runtime::Handle) {
        *self.inner.runtime.lock() = Some(handle);
    }

    pub fn settings(&self) -> AgentSettings {
        self.inner.settings.read().clone()
    }

    pub fn save_settings(&self, settings: AgentSettings) -> Result<AgentSettings, ProviderError> {
        settings.validate().map_err(invalid)?;
        if let Some(path) = &self.inner.settings_path {
            settings::save(path, &settings).map_err(ProviderError::internal)?;
        }
        *self.inner.settings.write() = settings.clone();
        // A wider ceiling may release what is queued.
        self.pump();
        Ok(settings)
    }

    /// Agents of a project (the open one is chosen by the caller), in board
    /// order.
    pub fn list(&self, project_id: Option<&str>) -> Vec<AgentView> {
        let agents = self.inner.store.agents_list(project_id);
        let pending = self.inner.autonomy.pending();
        let plan = self.plan();
        agents
            .into_iter()
            .map(|a| self.view(a, &pending, &plan))
            .collect()
    }

    pub fn get(&self, id: &AgentId) -> Option<AgentView> {
        let pending = self.inner.autonomy.pending();
        let plan = self.plan();
        self.inner
            .store
            .agent(id.as_str())
            .map(|a| self.view(a, &pending, &plan))
    }

    /// Locks held in a project, for the panel.
    pub fn locks(&self, project_id: Option<&str>) -> Vec<orchestrator_core::FileLock> {
        self.inner.locks.list(project_id)
    }

    /// Queues an agent for a task. It starts when there is a slot and the
    /// files of its task are free.
    pub fn start(&self, request: StartAgent) -> Result<Agent, ProviderError> {
        let agent = self.queue(request, None)?;
        self.pump();
        Ok(agent)
    }

    /// Ends an agent the way the user asked (`Cancel`): the running turn is
    /// cancelled, the locks fall and a handoff keeps the work reachable.
    pub async fn stop(&self, id: &AgentId, origin: CallOrigin) -> Result<Agent, ProviderError> {
        let agent = self
            .inner
            .store
            .agent(id.as_str())
            .ok_or_else(|| not_found(id))?;
        if agent.status.is_final() {
            return Ok(agent);
        }
        let live = self.inner.live.lock().get(id).cloned();
        match live {
            Some(live) => {
                live.cancel.cancel();
                if let Some(session) = &agent.session {
                    let _ = self.inner.sessions.cancel(session).await;
                }
                // The agent's own task records the ending.
                Ok(agent)
            }
            None => {
                // Queued, or left behind by a restart: end it here.
                let ended = self.conclude(&agent.id, Ending::Stopped, origin).await;
                self.pump();
                ended
            }
        }
    }

    /// `Stop All Agents` (master document, section 11). Returns how many
    /// agents were asked to stop.
    pub async fn stop_all(
        &self,
        project_id: Option<&str>,
        origin: CallOrigin,
    ) -> Result<usize, ProviderError> {
        let live: Vec<AgentId> = self
            .inner
            .store
            .agents_live()
            .into_iter()
            .filter(|a| project_id.is_none_or(|p| a.project_id == p))
            .map(|a| a.id)
            .collect();
        let asked = live.len();
        for id in live {
            let _ = self.stop(&id, origin.clone()).await;
        }
        Ok(asked)
    }

    /// `Pause` (section 11): the agent stops at its next tool call or its
    /// next turn, whichever comes first, keeping its slot and its files.
    pub fn pause(&self, id: &AgentId, origin: CallOrigin) -> Result<AgentView, ProviderError> {
        let agent = self
            .inner
            .store
            .agent(id.as_str())
            .ok_or_else(|| not_found(id))?;
        if agent.status != AgentStatus::Running {
            return Err(invalid("só um agente em execução pode ser pausado"));
        }
        self.inner.autonomy.pause_agent(&agent, origin);
        self.get(id).ok_or_else(|| not_found(id))
    }

    pub fn resume(&self, id: &AgentId, origin: CallOrigin) -> Result<AgentView, ProviderError> {
        let agent = self
            .inner
            .store
            .agent(id.as_str())
            .ok_or_else(|| not_found(id))?;
        self.inner.autonomy.resume_agent(&agent, origin);
        self.get(id).ok_or_else(|| not_found(id))
    }

    /// Pauses every AI: agents, their queue and the sessions the user
    /// drives (their calls wait in the gate).
    pub fn pause_all(&self, origin: CallOrigin) -> bool {
        self.inner.autonomy.pause_all(origin)
    }

    pub fn resume_all(&self, origin: CallOrigin) -> bool {
        let changed = self.inner.autonomy.resume_all(origin);
        // What was queued during the pause may start now.
        self.pump();
        changed
    }

    /// On startup: nothing is running, so no agent may stay `RUNNING` and
    /// no file may stay locked (ADR-0015).
    pub fn recover(&self) -> usize {
        let closed = self
            .inner
            .store
            .agents_recover("o Orchestrator foi encerrado enquanto este agente trabalhava")
            .unwrap_or(0);
        if closed > 0 {
            self.inner.sink.audit(AuditEvent::new(
                EventKind::AgentFinished,
                CallOrigin::System,
                format!(
                    "{closed} agente{} encerrado{} com o app; arquivos liberados",
                    if closed == 1 { "" } else { "s" },
                    if closed == 1 { "" } else { "s" }
                ),
                json!({"agents": closed, "reason": "restart"}),
            ));
        }
        closed
    }

    /// `agent.finish`: the agent of `session` reports what it did. The loop
    /// sees it after the turn and ends the agent.
    pub fn report_finish(&self, session: &SessionId, result: &str) -> Result<Agent, ProviderError> {
        let agent = self
            .inner
            .store
            .agent_of_session(session.as_str())
            .ok_or_else(|| invalid("esta sessão não é de um agente"))?;
        if agent.status != AgentStatus::Running {
            return Err(invalid("este agente não está em execução"));
        }
        let live = self
            .inner
            .live
            .lock()
            .get(&agent.id)
            .cloned()
            .ok_or_else(|| invalid("este agente não está em execução"))?;
        *live.finish.lock() = Some(clip(result.trim(), MAX_RESULT));
        Ok(agent)
    }

    /// `agent.delegate`: a subtask of the agent's task, with a subagent
    /// queued for it.
    pub fn delegate(
        &self,
        session: &SessionId,
        title: &str,
        description: &str,
        files: Vec<String>,
    ) -> Result<(orchestrator_core::Task, Agent), ProviderError> {
        let parent = self
            .inner
            .store
            .agent_of_session(session.as_str())
            .ok_or_else(|| invalid("esta sessão não é de um agente"))?;
        if parent.status != AgentStatus::Running {
            return Err(invalid("este agente não está em execução"));
        }
        if self.depth_of(&parent) + 1 >= MAX_DEPTH {
            return Err(invalid(
                "um subagente não cria outro subagente: traga a subtask para o agente principal",
            ));
        }
        let children = self
            .inner
            .store
            .agents_list(Some(&parent.project_id))
            .into_iter()
            .filter(|a| a.parent_agent.as_ref() == Some(&parent.id))
            .count();
        let limit = self.settings().max_subagents as usize;
        if limit == 0 {
            return Err(invalid(
                "a delegação está desligada nas configurações dos agentes: faça a subtask \
                 neste agente",
            ));
        }
        if children >= limit {
            return Err(invalid(format!("no máximo {limit} subagentes por agente")));
        }
        let origin = self.origin_of(&parent);
        let task = self.inner.tasks.save(
            TaskInput {
                project_id: Some(parent.project_id.clone()),
                title: Some(title.to_owned()),
                description: Some(description.to_owned()),
                parent_task: Some(parent.task.clone()),
                files: Some(files),
                provider: Some(parent.provider.clone()),
                model: parent.model.clone(),
                ..Default::default()
            },
            origin.clone(),
        )?;
        let agent = self.queue(
            StartAgent {
                task_id: task.id.clone(),
                provider: Some(parent.provider.clone()),
                model: parent.model.clone(),
                max_turns: None,
                // The grant is for that line of work.
                autonomy: parent.autonomy,
                max_cost_usd: parent.max_cost_usd,
            },
            Some(parent.id.clone()),
        )?;
        self.pump();
        Ok((task, agent))
    }

    // ---- internals -------------------------------------------------

    fn view(
        &self,
        agent: Agent,
        pending: &[ApprovalView],
        plan: &[(Agent, Option<String>)],
    ) -> AgentView {
        let task = self.inner.store.task(agent.task.as_str());
        // Its place in the queue and what keeps it there.
        let waiting = plan
            .iter()
            .position(|(queued, _)| queued.id == agent.id)
            .map(|index| (plan[index].1.clone(), index as u32 + 1));
        let autonomy = &self.inner.autonomy;
        let paused = agent.status == AgentStatus::Running
            && (autonomy.paused_all() || autonomy.is_agent_paused(agent.id.as_str()));
        let approval = pending
            .iter()
            .find(|p| p.request.agent_id.as_ref() == Some(&agent.id))
            .map(|p| p.request.summary.clone());
        let mode = agent
            .autonomy
            .unwrap_or_else(|| autonomy.mode_of(Some(&agent.project_id)));
        let cost_usd = agent
            .session
            .as_ref()
            .and_then(|id| self.inner.sessions.info(id).ok())
            .and_then(|info| info.usage.cost_usd);
        let (waiting, queue_position) = match waiting {
            Some((reason, position)) => (reason, Some(position)),
            None => (None, None),
        };
        AgentView {
            waiting,
            paused,
            approval,
            mode,
            cost_usd,
            queue_position,
            task_title: task
                .as_ref()
                .map(|t| t.title.clone())
                .unwrap_or_else(|| agent.title.clone()),
            task_status: task.as_ref().map_or(TaskStatus::Todo, |t| t.status),
            agent,
        }
    }

    /// The queue in start order (ADR-0018): task priority, then arrival.
    /// Each agent comes with what keeps it waiting (`None`: it starts now),
    /// as if those ahead of it started — an agent that cannot start does
    /// not hold the ones behind it.
    fn plan(&self) -> Vec<(Agent, Option<String>)> {
        let settings = self.settings();
        let paused = self.inner.autonomy.paused_all();
        let live = self.inner.store.agents_live();
        let spawned: Vec<AgentId> = self.inner.live.lock().keys().cloned().collect();
        let mut running = 0u32;
        let mut by_provider: HashMap<ProviderId, u32> = HashMap::new();
        let mut queued = Vec::new();
        for agent in live {
            if agent.status == AgentStatus::Running || spawned.contains(&agent.id) {
                running += 1;
                *by_provider.entry(agent.provider.clone()).or_default() += 1;
            } else {
                let priority = self
                    .inner
                    .store
                    .task(agent.task.as_str())
                    .map(|t| t.priority)
                    .unwrap_or_default();
                queued.push((std::cmp::Reverse(priority), agent));
            }
        }
        // Stable: same priority keeps the order of arrival.
        queued.sort_by_key(|a| a.0);
        let mut budgets: HashMap<String, Option<String>> = HashMap::new();
        let mut plan = Vec::new();
        for (_, agent) in queued {
            let hold = if paused {
                Some("as IAs estão pausadas".to_owned())
            } else if let Some(lock) = self.files_held(&agent) {
                Some(lock)
            } else if let Some(over) = budgets
                .entry(agent.project_id.clone())
                .or_insert_with(|| self.over_budget(&agent.project_id, &settings))
                .clone()
            {
                Some(over)
            } else if by_provider.get(&agent.provider).copied().unwrap_or(0)
                >= settings.provider_limit(agent.provider.as_str())
            {
                let limit = settings.provider_limit(agent.provider.as_str());
                Some(format!(
                    "{limit} agente{} de {} em execução (o limite dele é {limit})",
                    if limit == 1 { "" } else { "s" },
                    agent.provider
                ))
            } else if running >= settings.max_parallel {
                Some(format!(
                    "{} agente{} em execução (o limite é {})",
                    running,
                    if running == 1 { "" } else { "s" },
                    settings.max_parallel
                ))
            } else {
                running += 1;
                *by_provider.entry(agent.provider.clone()).or_default() += 1;
                None
            };
            plan.push((agent, hold));
        }
        plan
    }

    /// Who holds a file the agent's task declares.
    fn files_held(&self, agent: &Agent) -> Option<String> {
        let task = self.inner.store.task(agent.task.as_str())?;
        let project = self.inner.store.project(&task.project_id)?;
        self.inner
            .locks
            .blocked_by(
                &task.project_id,
                &PathBuf::from(&project.path),
                &task.files,
                agent,
            )
            .map(|lock| format!("{} está com \"{}\"", lock.path, lock.agent_title))
    }

    /// Why the project's daily budget keeps agents from running, if it does.
    fn over_budget(&self, project_id: &str, settings: &AgentSettings) -> Option<String> {
        let budget = settings.daily_budget_usd?;
        let spent = self
            .inner
            .store
            .spend(Some(project_id), today_start())
            .cost_usd;
        (spent >= budget).then(|| {
            format!(
                "o orçamento diário do projeto acabou (US$ {} de US$ {})",
                usd(spent),
                usd(budget)
            )
        })
    }

    /// The project's spending today against its daily budget.
    pub fn budget(&self, project_id: &str) -> BudgetView {
        let settings = self.settings();
        let spend = self.inner.store.spend(Some(project_id), today_start());
        BudgetView {
            spent_today_usd: spend.cost_usd,
            budget_usd: settings.daily_budget_usd,
            unpriced: spend.unpriced,
            exhausted: settings
                .daily_budget_usd
                .is_some_and(|budget| spend.cost_usd >= budget),
        }
    }

    /// Whether a running agent must stop before its next turn: its own cost
    /// ceiling or the project's daily budget. `unpriced` remembers that the
    /// session was told its cost is unknown.
    fn spent(&self, agent: &Agent, session: &SessionId, unpriced: &mut bool) -> Option<Ending> {
        if let Some(ceiling) = agent.max_cost_usd {
            let usage = self
                .inner
                .sessions
                .info(session)
                .map(|info| info.usage)
                .unwrap_or_default();
            match usage.cost_usd {
                Some(cost) if cost >= ceiling => {
                    return Some(Ending::Spent {
                        reason: "costCeiling",
                        message: format!(
                            "o agente chegou ao teto de custo de US$ {} (gastou US$ {})",
                            usd(ceiling),
                            usd(cost)
                        ),
                    })
                }
                None if usage.total_tokens() > 0 && !*unpriced => {
                    *unpriced = true;
                    let _ = self.inner.sessions.annotate(
                        session,
                        SessionEvent::Notice {
                            turn_id: None,
                            level: orchestrator_core::NoticeLevel::Warning,
                            message: "O modelo deste agente não tem preço configurado: o teto de \
                                      custo não se aplica (informe o preço na conexão)."
                                .into(),
                        },
                    );
                }
                _ => {}
            }
        }
        self.over_budget(&agent.project_id, &self.settings())
            .map(|message| Ending::Spent {
                reason: "dailyBudget",
                message,
            })
    }

    fn depth_of(&self, agent: &Agent) -> usize {
        let mut depth = 0;
        let mut current = agent.parent_agent.clone();
        while let Some(id) = current {
            depth += 1;
            if depth >= MAX_DEPTH {
                break;
            }
            current = self
                .inner
                .store
                .agent(id.as_str())
                .and_then(|a| a.parent_agent);
        }
        depth
    }

    fn origin_of(&self, agent: &Agent) -> CallOrigin {
        CallOrigin::Agent {
            agent_id: agent.id.to_string(),
            session_id: agent.session.clone(),
            provider: Some(agent.provider.clone()),
        }
    }

    /// Creates the agent row in `QUEUED`, after checking that the task can
    /// actually be worked on.
    fn queue(&self, request: StartAgent, parent: Option<AgentId>) -> Result<Agent, ProviderError> {
        let task = self
            .inner
            .store
            .task(request.task_id.as_str())
            .ok_or_else(|| invalid("task não encontrada"))?;
        if task.status.is_final() {
            return Err(invalid(
                "esta task está encerrada; reabra antes de pôr um agente nela",
            ));
        }
        if task.status == TaskStatus::Blocked {
            return Err(invalid(
                "esta task está bloqueada; desbloqueie antes de pôr um agente nela",
            ));
        }
        let pending: Vec<String> = task
            .dependencies
            .iter()
            .filter_map(|id| self.inner.store.task(id.as_str()))
            .filter(|dep| dep.status != TaskStatus::Done)
            .map(|dep| format!("\"{}\"", dep.title))
            .collect();
        if !pending.is_empty() {
            return Err(invalid(format!("esta task espera: {}", pending.join("; "))));
        }
        if let Some(busy) = self
            .inner
            .store
            .agents_of_task(task.id.as_str())
            .into_iter()
            .find(|a| a.status.is_live())
        {
            return Err(invalid(format!(
                "esta task já tem um agente {}",
                match busy.status {
                    AgentStatus::Running => "em execução",
                    _ => "na fila",
                }
            )));
        }
        let provider = match request.provider.or_else(|| task.provider.clone()) {
            Some(id) => self.inner.sessions.registry().require(&id)?.descriptor().id,
            None => {
                self.inner
                    .sessions
                    .registry()
                    .active()
                    .ok_or_else(|| invalid("nenhum provider de IA está ativo"))?
                    .descriptor()
                    .id
            }
        };
        let now = Utc::now();
        let agent = Agent {
            id: AgentId::new(),
            project_id: task.project_id.clone(),
            task: task.id.clone(),
            title: clip(&task.title, 120),
            provider,
            model: request.model.or_else(|| task.model.clone()),
            session: None,
            parent_agent: parent,
            status: AgentStatus::Queued,
            tools: Vec::new(),
            context: None,
            turns: 0,
            max_turns: request.max_turns.unwrap_or(self.settings().max_turns),
            files: Vec::new(),
            result: String::new(),
            error: None,
            handoff: None,
            autonomy: request.autonomy,
            max_cost_usd: request.max_cost_usd.or(self.settings().max_cost_usd),
            created_at: now,
            updated_at: now,
            started_at: None,
            finished_at: None,
        };
        self.inner
            .store
            .agent_save(&agent)
            .map_err(ProviderError::internal)?;
        Ok(agent)
    }

    /// Starts every queued agent that has a slot and free files.
    ///
    /// Agents run as tasks of the async runtime ([`Self::set_runtime`]), so
    /// this does nothing when there is none (the agent stays queued and the
    /// next pump takes it).
    fn pump(&self) {
        let runtime = self
            .inner
            .runtime
            .lock()
            .clone()
            .or_else(|| tokio::runtime::Handle::try_current().ok());
        let Some(runtime) = runtime else {
            return;
        };
        // One pump at a time: the plan counts the slots it fills.
        let _pumping = self.inner.pumping.lock();
        for (agent, hold) in self.plan() {
            if hold.is_some() || self.inner.live.lock().contains_key(&agent.id) {
                continue;
            }
            let live = Arc::new(Live {
                cancel: CancellationToken::new(),
                finish: Mutex::new(None),
            });
            self.inner.live.lock().insert(agent.id.clone(), live);
            let service = self.clone();
            let id = agent.id.clone();
            runtime.spawn(async move { service.run(id).await });
        }
    }

    /// One agent, from the first turn to the end.
    async fn run(self, id: AgentId) {
        let ending = self.work(&id).await;
        let origin = self
            .inner
            .store
            .agent(id.as_str())
            .map(|a| self.origin_of(&a))
            .unwrap_or(CallOrigin::System);
        let _ = self.conclude(&id, ending, origin).await;
        self.inner.live.lock().remove(&id);
        self.pump();
    }

    async fn work(&self, id: &AgentId) -> Ending {
        let Some(agent) = self.inner.store.agent(id.as_str()) else {
            return Ending::Failed("o agente sumiu do banco".into());
        };
        let Some(live) = self.inner.live.lock().get(id).cloned() else {
            return Ending::Failed("o agente não está mais em execução".into());
        };
        let Some(task) = self.inner.store.task(agent.task.as_str()) else {
            return Ending::Failed("a task deste agente sumiu".into());
        };
        let Some(project) = self.inner.store.project(&task.project_id) else {
            return Ending::Failed("o projeto desta task não está registrado".into());
        };

        // The files the task declares are held before the AI is told
        // anything: an agent that cannot have them does not start.
        if let Some(held) =
            self.inner
                .locks
                .take(&agent, &PathBuf::from(&project.path), &task.files)
        {
            self.inner.locks.release(&agent);
            return Ending::Failed(format!(
                "{} está com o agente \"{}\"",
                held.path, held.agent_title
            ));
        }

        let opened = match self
            .inner
            .tasks
            .open_session(
                StartTaskSession {
                    task_id: agent.task.clone(),
                    provider: Some(agent.provider.clone()),
                    model: agent.model.clone(),
                    budget: None,
                },
                self.origin_of(&agent),
            )
            .await
        {
            Ok(opened) => opened,
            Err(err) => return Ending::Failed(err.message),
        };

        let mut agent = match self.inner.store.agent(id.as_str()) {
            Some(fresh) => fresh,
            None => return Ending::Failed("o agente sumiu do banco".into()),
        };
        let now = Utc::now();
        agent.session = Some(opened.session.id.clone());
        agent.status = AgentStatus::Running;
        agent.started_at = Some(now);
        agent.updated_at = now;
        agent.tools = self.inner.sessions.tool_names();
        let _ = self.inner.store.agent_save(&agent);
        self.record(
            EventKind::AgentStarted,
            &agent,
            format!("agente em execução · {}", agent.provider),
            json!({
                "sessionId": opened.session.id,
                "files": agent.files,
                "autonomy": agent.autonomy,
                "mode": agent
                    .autonomy
                    .unwrap_or_else(|| self.inner.autonomy.mode_of(Some(&agent.project_id))),
            }),
            self.origin_of(&agent),
        );
        let _ = self.inner.sessions.annotate(
            &opened.session.id,
            SessionEvent::Notice {
                turn_id: None,
                level: orchestrator_core::NoticeLevel::Info,
                message: format!(
                    "Agente conduzindo esta sessão (até {} turnos). Ele termina chamando \
                     `agent.finish`.",
                    agent.max_turns
                ),
            },
        );

        let mut message = opened.first_message;
        let mut unpriced = false;
        loop {
            if live.cancel.is_cancelled() {
                return Ending::Stopped;
            }
            // Paused (this agent, or every AI): the next turn waits.
            if !self
                .inner
                .autonomy
                .wait_while_paused(Some(id.as_str()), &live.cancel)
                .await
            {
                return Ending::Stopped;
            }
            // Its cost ceiling, or the project's daily budget (ADR-0018).
            if let Some(ending) = self.spent(&agent, &opened.session.id, &mut unpriced) {
                return ending;
            }
            // The count goes up before the turn, so the panel shows the
            // turn the agent is on, not the one it finished.
            agent.turns += 1;
            agent.updated_at = Utc::now();
            let _ = self.inner.store.agent_save(&agent);
            let turn = self
                .inner
                .sessions
                .execute(&opened.session.id, message, self.origin_of(&agent))
                .await;
            agent.updated_at = Utc::now();
            if agent.context.is_none() {
                agent.context = self.context_of(&opened.session.id);
            }
            // The row carries what it holds now: the tools may have locked
            // more files during the turn.
            if let Some(fresh) = self.inner.store.agent(id.as_str()) {
                agent.files = fresh.files;
            }
            let _ = self.inner.store.agent_save(&agent);

            if let Some(result) = live.finish.lock().take() {
                return Ending::Done(result);
            }
            if live.cancel.is_cancelled() {
                return Ending::Stopped;
            }
            match turn {
                Ok(result) => match result.status {
                    TurnStatus::Completed => {}
                    TurnStatus::Cancelled => return Ending::Stopped,
                    TurnStatus::Failed => {
                        return Ending::Failed(
                            result.error.unwrap_or_else(|| "o turno falhou".into()),
                        )
                    }
                },
                Err(err) => return Ending::Failed(err.message),
            }
            if agent.turns >= agent.max_turns {
                return Ending::Failed(format!(
                    "o agente chegou ao teto de {} turnos sem chamar agent.finish",
                    agent.max_turns
                ));
            }
            message = CONTINUE.to_owned();
        }
    }

    /// The context the session received on its first turn, from its own
    /// transcript (it is not built twice).
    fn context_of(&self, session: &SessionId) -> Option<orchestrator_core::ContextSummary> {
        let snapshot = self.inner.sessions.snapshot(session).ok()?;
        snapshot
            .entries
            .into_iter()
            .find_map(|entry| match entry.event {
                SessionEvent::ContextAttached { summary, .. } => Some(summary),
                _ => None,
            })
    }

    /// Writes the outcome: the task, the locks, the handoff and the event.
    async fn conclude(
        &self,
        id: &AgentId,
        ending: Ending,
        origin: CallOrigin,
    ) -> Result<Agent, ProviderError> {
        let mut agent = self
            .inner
            .store
            .agent(id.as_str())
            .ok_or_else(|| not_found(id))?;
        if agent.status.is_final() {
            return Ok(agent);
        }
        let now = Utc::now();
        match &ending {
            Ending::Done(result) => {
                agent.status = AgentStatus::Done;
                agent.result = result.clone();
            }
            Ending::Failed(error) | Ending::Spent { message: error, .. } => {
                agent.status = AgentStatus::Failed;
                agent.error = Some(clip(error, 500));
            }
            Ending::Stopped => agent.status = AgentStatus::Stopped,
        }
        agent.finished_at = Some(now);
        agent.updated_at = now;

        // The work goes on the task before the agent is gone.
        match &ending {
            Ending::Done(result) => {
                let _ = self.inner.tasks.save(
                    TaskInput {
                        id: Some(agent.task.clone()),
                        result: Some(result.clone()),
                        ..Default::default()
                    },
                    origin.clone(),
                );
                // The agent does not conclude the task: a person reviews it.
                let _ =
                    self.inner
                        .tasks
                        .set_status(&agent.task, TaskStatus::Review, origin.clone());
            }
            // It stopped mid-work: a handoff keeps the work reachable by
            // another AI, which is what the handoff is for.
            Ending::Failed(_) | Ending::Spent { .. } | Ending::Stopped => {
                agent.handoff = self.handoff_for(&agent, origin.clone()).await;
            }
        }

        self.inner.locks.release(&agent);
        self.inner.autonomy.forget_agent(agent.id.as_str());
        agent.files.clear();
        self.inner
            .store
            .agent_save(&agent)
            .map_err(ProviderError::internal)?;
        let outcome = match &ending {
            Ending::Done(_) => "concluiu",
            Ending::Failed(_) => "falhou",
            Ending::Spent { reason, .. } if *reason == "dailyBudget" => {
                "parou: orçamento diário do projeto"
            }
            Ending::Spent { .. } => "parou: teto de custo",
            Ending::Stopped => "parado pelo usuário",
        };
        let reason = match &ending {
            Ending::Spent { reason, .. } => Some(*reason),
            _ => None,
        };
        self.record(
            EventKind::AgentFinished,
            &agent,
            format!("agente {outcome}"),
            json!({
                "outcome": agent.status,
                "turns": agent.turns,
                "result": clip(&agent.result, 300),
                "error": agent.error,
                "handoffId": agent.handoff,
                "sessionId": agent.session,
                "reason": reason,
            }),
            origin,
        );
        Ok(agent)
    }

    /// Facts-only handoff (no AI turn): what was done, what is left and the
    /// agent's own ending.
    async fn handoff_for(
        &self,
        agent: &Agent,
        origin: CallOrigin,
    ) -> Option<orchestrator_core::HandoffId> {
        let session = agent.session.clone()?;
        let draft = self
            .inner
            .handoffs
            .prepare(PrepareRequest {
                session_id: session.clone(),
                ask_agent: false,
            })
            .await
            .ok()?;
        let mut packet = draft.packet;
        let ending = match &agent.error {
            Some(error) => format!("O agente parou: {error}"),
            None => "O agente foi parado pelo usuário.".to_owned(),
        };
        packet.status = match packet.status.trim() {
            "" => ending,
            current => format!("{current} · {ending}"),
        };
        if packet.goal.trim().is_empty() {
            packet.goal = agent.title.clone();
        }
        if packet.next_action.trim().is_empty() {
            packet.next_action = format!("Retomar \"{}\" de onde o agente parou.", agent.title);
        }
        self.inner
            .handoffs
            .create(
                CreateRequest {
                    session_id: session,
                    packet,
                    by_agent: false,
                },
                origin,
            )
            .ok()
            .map(|handoff| handoff.id)
    }

    fn record(
        &self,
        kind: EventKind,
        agent: &Agent,
        what: String,
        extra: serde_json::Value,
        origin: CallOrigin,
    ) {
        let mut data = json!({
            "agentId": agent.id,
            "projectId": agent.project_id,
            "taskId": agent.task,
            "title": agent.title,
            "provider": agent.provider,
            "model": agent.model,
            "status": agent.status,
            "parentAgent": agent.parent_agent,
        });
        if let (Some(data), Some(extra)) = (data.as_object_mut(), extra.as_object()) {
            for (key, value) in extra {
                data.insert(key.clone(), value.clone());
            }
        }
        self.inner.sink.audit(AuditEvent::new(
            kind,
            origin,
            format!("{what} · {}", clip(&agent.title, 100)),
            data,
        ));
    }
}
