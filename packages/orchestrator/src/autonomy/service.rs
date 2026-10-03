//! The autonomy service (ADR-0016): the mode of each call, the user's
//! rules, the requests waiting for the user, what the user allowed for a
//! session, and the pause.
//!
//! Only the user changes any of this, through the app's commands: no AI
//! tool reaches here.

use super::describe;
use super::policy::{self, assisted_rules, default_rules, evaluate, Scope, Unit, Verdict};
use super::settings::{self, AutonomySettings};
use crate::text::clip;
use crate::tools::audit_args;
use chrono::{DateTime, Utc};
use orchestrator_core::{
    Agent, ApprovalAnswer, ApprovalId, ApprovalRequest, AuditEvent, AutonomyMode, CallOrigin,
    Decision, EventKind, EventSink, PolicyRule, SessionId, ToolCall, ToolDefinition, ToolError,
    ToolErrorKind, ToolResult,
};
use orchestrator_memory::MemoryStore;
use parking_lot::{Mutex, RwLock};
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::{oneshot, watch};
use tokio_util::sync::CancellationToken;

/// Where the runtime puts relative paths now (the open project, or the
/// initial folder).
pub type Workdir = Arc<dyn Fn() -> PathBuf + Send + Sync>;

/// Who is behind a call, and where it lands.
#[derive(Debug, Clone)]
pub struct CallContext {
    pub session: Option<SessionId>,
    /// The agent conducting the session, if one is.
    pub agent: Option<Agent>,
    pub project_id: Option<String>,
    pub mode: AutonomyMode,
    /// Where the mode came from: `agent`, `project` or `default`.
    pub mode_source: &'static str,
    pub scope: Scope,
}

/// What "Permitir nesta sessão" covers: the same rule, the same tool and,
/// with a command, the same command.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct GrantKey {
    tool: String,
    mode: AutonomyMode,
    rule: Option<usize>,
    command: Option<String>,
}

/// Something the user allowed for the rest of a session.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionGrant {
    pub id: String,
    pub session_id: SessionId,
    pub tool: String,
    pub mode: AutonomyMode,
    /// Number (1-based) of the rule that asked; `None`: no rule matched.
    pub rule: Option<u32>,
    pub command: Option<String>,
    pub agent_title: Option<String>,
    pub granted_at: DateTime<Utc>,
}

/// A request as the UI lists it.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApprovalView {
    #[serde(flatten)]
    pub request: ApprovalRequest,
    pub project_name: Option<String>,
}

/// The autonomy of a project, as the UI shows it.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AutonomyOverview {
    pub project_id: Option<String>,
    /// Mode that applies to the project's sessions.
    pub mode: AutonomyMode,
    /// Mode chosen for the project; `None`: the default applies.
    pub project_mode: Option<AutonomyMode>,
    pub default_mode: AutonomyMode,
    /// The user's rules (Autonomous).
    pub rules: Vec<PolicyRule>,
    /// The fixed rules of the Assisted mode.
    pub assisted_rules: Vec<PolicyRule>,
    /// What "Restaurar padrão" puts back.
    pub default_rules: Vec<PolicyRule>,
    pub paused_all: bool,
    pub paused_agents: Vec<String>,
    pub pending: usize,
    pub grants: Vec<SessionGrant>,
    pub warning: Option<String>,
}

/// One target of a trial, as the UI shows it.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrialTarget {
    pub path: String,
    pub inside: bool,
    pub command: Option<String>,
    pub decision: Decision,
    pub rule: Option<u32>,
}

/// "Experimentar": which rule decides a call, and why.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Trial {
    pub mode: AutonomyMode,
    pub decision: Decision,
    pub rule: Option<u32>,
    pub reason: String,
    pub targets: Vec<TrialTarget>,
    pub opaque: bool,
    /// The tool is not in the catalog (the call would fail).
    pub unknown_tool: bool,
}

/// What the gate hears back from a request.
#[derive(Debug)]
pub enum Reply {
    Allowed,
    Denied(String),
}

struct Pending {
    request: ApprovalRequest,
    call: ToolCall,
    read_only: bool,
    asking: Vec<GrantKey>,
    reply: oneshot::Sender<Reply>,
    since: Instant,
}

#[derive(Default)]
struct State {
    pending: Vec<Pending>,
    grants: Vec<(GrantKey, SessionGrant)>,
    paused_all: bool,
    paused_agents: HashSet<String>,
}

struct Inner {
    store: Arc<MemoryStore>,
    sink: Arc<dyn EventSink>,
    settings: RwLock<AutonomySettings>,
    path: Option<PathBuf>,
    warning: Mutex<Option<String>>,
    workdir: RwLock<Workdir>,
    /// Tool name → `readOnly`, from the executor the gate wraps.
    tools: RwLock<HashMap<String, bool>>,
    state: Mutex<State>,
    /// Bumped on every change a waiting call may care about.
    changed: watch::Sender<u64>,
}

#[derive(Clone)]
pub struct AutonomyService {
    inner: Arc<Inner>,
}

fn rule_number(index: Option<usize>) -> Option<u32> {
    index.map(|i| i as u32 + 1)
}

impl AutonomyService {
    /// Reads `path` (defaults, and a warning, when it is unusable).
    pub fn new(
        store: Arc<MemoryStore>,
        sink: Arc<dyn EventSink>,
        path: Option<PathBuf>,
    ) -> (Self, Option<String>) {
        let (settings, warning) = match &path {
            Some(path) => settings::load(path),
            None => (AutonomySettings::default(), None),
        };
        let fallback = std::env::current_dir().unwrap_or_default();
        let service = Self {
            inner: Arc::new(Inner {
                store,
                sink,
                settings: RwLock::new(settings),
                path,
                warning: Mutex::new(warning.clone()),
                workdir: RwLock::new(Arc::new(move || fallback.clone())),
                tools: RwLock::new(HashMap::new()),
                state: Mutex::new(State::default()),
                changed: watch::channel(0).0,
            }),
        };
        (service, warning)
    }

    /// Names where the runtime resolves relative paths (the app passes the
    /// runtime's base directory).
    pub fn set_workdir(&self, workdir: Workdir) {
        *self.inner.workdir.write() = workdir;
    }

    /// Learns which tools exist and which of them only query state.
    pub fn learn_tools(&self, definitions: &[ToolDefinition]) {
        let mut tools = self.inner.tools.write();
        for def in definitions {
            tools.insert(def.name.clone(), def.read_only);
        }
    }

    /// `Some(read_only)` for a known tool.
    pub fn read_only(&self, tool: &str) -> Option<bool> {
        self.inner.tools.read().get(tool).copied()
    }

    pub fn settings(&self) -> AutonomySettings {
        self.inner.settings.read().clone()
    }

    /// Mode of a project's sessions (the default without a project).
    pub fn mode_of(&self, project_id: Option<&str>) -> AutonomyMode {
        self.inner.settings.read().mode_of(project_id)
    }

    // ---- who and where ----------------------------------------------

    pub fn context_of(&self, call: &ToolCall) -> CallContext {
        let session = match &call.origin {
            CallOrigin::Agent { session_id, .. } => session_id.clone(),
            _ => None,
        };
        let store = &self.inner.store;
        let agent = session
            .as_ref()
            .and_then(|s| store.agent_of_session(s.as_str()));
        let project_id = agent.as_ref().map(|a| a.project_id.clone()).or_else(|| {
            session
                .as_ref()
                .and_then(|s| store.session_project_id(s.as_str()))
        });
        let settings = self.inner.settings.read();
        let (mode, mode_source) = match agent.as_ref().and_then(|a| a.autonomy) {
            Some(mode) => (mode, "agent"),
            None => match project_id
                .as_deref()
                .and_then(|id| settings.projects.get(id))
            {
                Some(mode) => (*mode, "project"),
                None => (settings.default_mode, "default"),
            },
        };
        drop(settings);
        let project_root = project_id
            .as_deref()
            .and_then(|id| store.project(id))
            .map(|p| PathBuf::from(p.path));
        // A session works in its own project (ADR-0023).
        let workdir = call
            .workspace
            .clone()
            .unwrap_or_else(|| (self.inner.workdir.read())());
        CallContext {
            session,
            agent,
            project_id,
            mode,
            mode_source,
            scope: Scope {
                project_root,
                workdir,
            },
        }
    }

    fn rules_for(&self, mode: AutonomyMode) -> Vec<PolicyRule> {
        match mode {
            AutonomyMode::Assisted => assisted_rules(),
            AutonomyMode::Autonomous => self.inner.settings.read().rules.clone(),
            AutonomyMode::Unrestricted => Vec::new(),
        }
    }

    /// The decision of the mode's rules about a call. Never called in
    /// Unrestricted, where nothing is evaluated.
    pub fn judge(&self, call: &ToolCall, read_only: bool, context: &CallContext) -> Verdict {
        evaluate(
            &self.rules_for(context.mode),
            &call.tool,
            read_only,
            &call.args,
            &context.scope,
        )
    }

    /// Why the verdict is what it is, in the user's words.
    pub fn reason(&self, mode: AutonomyMode, source: &str, verdict: &Verdict) -> String {
        let whose = match (mode, source) {
            (AutonomyMode::Assisted, "agent") => "Modo Assistido (dado a este agente)",
            (AutonomyMode::Assisted, _) => "Modo Assistido",
            (AutonomyMode::Autonomous, "agent") => "Suas regras (Autônomo, dado a este agente)",
            (AutonomyMode::Autonomous, _) => "Suas regras (Autônomo)",
            (AutonomyMode::Unrestricted, _) => return "Acesso Irrestrito".to_owned(),
        };
        let rules = self.rules_for(mode);
        match verdict.rule.and_then(|i| rules.get(i).map(|r| (i, r))) {
            Some((index, rule)) => {
                let note = rule
                    .note
                    .as_deref()
                    .map(|n| format!(" ({n})"))
                    .unwrap_or_default();
                format!("{whose}, regra {}: {}{note}", index + 1, rule.describe())
            }
            None => format!(
                "{whose}: nenhuma regra decide esta chamada, e na dúvida o Orchestrator pergunta"
            ),
        }
    }

    fn keys(call: &ToolCall, mode: AutonomyMode, asking: &[&Unit]) -> Vec<GrantKey> {
        let mut keys: Vec<GrantKey> = Vec::new();
        for unit in asking {
            let key = GrantKey {
                tool: call.tool.clone(),
                mode,
                rule: unit.rule,
                command: unit.command.clone(),
            };
            if !keys.contains(&key) {
                keys.push(key);
            }
        }
        keys
    }

    /// Whether everything the verdict would ask about was already allowed
    /// for this session.
    pub fn granted(&self, call: &ToolCall, context: &CallContext, verdict: &Verdict) -> bool {
        let Some(session) = &context.session else {
            return false;
        };
        let asking: Vec<&Unit> = verdict.asking().collect();
        if asking.is_empty() {
            return false;
        }
        let keys = Self::keys(call, context.mode, &asking);
        let state = self.inner.state.lock();
        keys.iter().all(|key| {
            state
                .grants
                .iter()
                .any(|(k, g)| k == key && &g.session_id == session)
        })
    }

    // ---- requests ----------------------------------------------------

    /// Opens a request and returns what the gate waits on.
    pub fn ask(
        &self,
        call: &ToolCall,
        read_only: bool,
        context: &CallContext,
        verdict: &Verdict,
    ) -> (ApprovalId, oneshot::Receiver<Reply>) {
        let (reply, answer) = oneshot::channel();
        let asking: Vec<&Unit> = verdict.asking().collect();
        let commands: Vec<String> = asking.iter().filter_map(|u| u.command.clone()).collect();
        let request = ApprovalRequest {
            id: ApprovalId::new(),
            call_id: call.id.clone(),
            tool: call.tool.clone(),
            summary: describe::summary(call),
            detail: describe::detail(call),
            reason: self.reason(context.mode, context.mode_source, verdict),
            mode: context.mode,
            rule: rule_number(verdict.rule),
            command: (!commands.is_empty()).then(|| commands.join(" · ")),
            session_id: context
                .session
                .clone()
                .unwrap_or_else(|| SessionId::from("")),
            agent_id: context.agent.as_ref().map(|a| a.id.clone()),
            agent_title: context.agent.as_ref().map(|a| a.title.clone()),
            task_id: context.agent.as_ref().map(|a| a.task.clone()),
            project_id: context.project_id.clone(),
            provider: match &call.origin {
                CallOrigin::Agent { provider, .. } => provider.clone(),
                _ => None,
            },
            requested_at: Utc::now(),
        };
        let id = request.id.clone();
        let who = request
            .agent_title
            .as_deref()
            .map(|t| format!("O agente \"{t}\""))
            .unwrap_or_else(|| "Uma sessão".to_owned());
        self.audit(
            AuditEvent::new(
                EventKind::ApprovalRequested,
                call.origin.clone(),
                format!("{who} pede: {}", request.summary),
                json!({
                    "approvalId": request.id,
                    "tool": request.tool,
                    "summary": request.summary,
                    "reason": request.reason,
                    "mode": request.mode,
                    "rule": request.rule,
                    "sessionId": context.session,
                    "agentId": request.agent_id,
                    "taskId": request.task_id,
                }),
            )
            .with_call(call.id.clone()),
        );
        self.inner.state.lock().pending.push(Pending {
            request,
            call: call.clone(),
            read_only,
            asking: Self::keys(call, context.mode, &asking),
            reply,
            since: Instant::now(),
        });
        self.bump();
        (id, answer)
    }

    /// Requests waiting for the user, oldest first.
    pub fn pending(&self) -> Vec<ApprovalView> {
        let requests: Vec<ApprovalRequest> = self
            .inner
            .state
            .lock()
            .pending
            .iter()
            .map(|p| p.request.clone())
            .collect();
        requests
            .into_iter()
            .map(|request| ApprovalView {
                project_name: request
                    .project_id
                    .as_deref()
                    .and_then(|id| self.inner.store.project(id))
                    .map(|p| p.name),
                request,
            })
            .collect()
    }

    /// The user's answer.
    pub fn answer(
        &self,
        id: &ApprovalId,
        answer: ApprovalAnswer,
        note: Option<&str>,
        origin: CallOrigin,
    ) -> Result<ApprovalRequest, String> {
        let pending = {
            let mut state = self.inner.state.lock();
            let index = state
                .pending
                .iter()
                .position(|p| &p.request.id == id)
                .ok_or("este pedido já foi respondido ou cancelado")?;
            let pending = state.pending.remove(index);
            if answer == ApprovalAnswer::ApproveSession
                && !pending.request.session_id.as_str().is_empty()
            {
                for key in &pending.asking {
                    let grant = SessionGrant {
                        id: ApprovalId::new().to_string(),
                        session_id: pending.request.session_id.clone(),
                        tool: key.tool.clone(),
                        mode: key.mode,
                        rule: rule_number(key.rule),
                        command: key.command.clone(),
                        agent_title: pending.request.agent_title.clone(),
                        granted_at: Utc::now(),
                    };
                    state.grants.push((key.clone(), grant));
                }
            }
            pending
        };
        let note = note.map(str::trim).filter(|n| !n.is_empty());
        let reply = match answer {
            ApprovalAnswer::Approve | ApprovalAnswer::ApproveSession => Reply::Allowed,
            ApprovalAnswer::Deny => Reply::Denied(match note {
                Some(note) => format!("O usuário negou: {}", clip(note, 500)),
                None => "O usuário negou esta chamada.".to_owned(),
            }),
        };
        let (label, key) = match answer {
            ApprovalAnswer::Approve => ("Permitido", "approved"),
            ApprovalAnswer::ApproveSession => ("Permitido nesta sessão", "approvedSession"),
            ApprovalAnswer::Deny => ("Negado", "denied"),
        };
        self.decided(&pending, label, key, "user", note, origin);
        let request = pending.request.clone();
        let _ = pending.reply.send(reply);
        self.bump();
        Ok(request)
    }

    /// The turn behind a request was cancelled: the request goes away.
    pub fn withdraw(&self, id: &ApprovalId) {
        let pending = {
            let mut state = self.inner.state.lock();
            state
                .pending
                .iter()
                .position(|p| &p.request.id == id)
                .map(|index| state.pending.remove(index))
        };
        if let Some(pending) = pending {
            self.decided(
                &pending,
                "Cancelado",
                "cancelled",
                "turn",
                None,
                CallOrigin::System,
            );
            self.bump();
        }
    }

    fn decided(
        &self,
        pending: &Pending,
        label: &str,
        answer: &str,
        by: &str,
        note: Option<&str>,
        origin: CallOrigin,
    ) {
        self.audit(
            AuditEvent::new(
                EventKind::ApprovalDecided,
                origin,
                format!("{label}: {}", pending.request.summary),
                json!({
                    "approvalId": pending.request.id,
                    "tool": pending.request.tool,
                    "answer": answer,
                    "by": by,
                    "note": note,
                    "waitedMs": pending.since.elapsed().as_millis() as u64,
                    "sessionId": pending.request.session_id,
                    "agentId": pending.request.agent_id,
                }),
            )
            .with_call(pending.request.call_id.clone()),
        );
    }

    /// After the mode or the rules change: what is now allowed runs, what
    /// is now denied is denied, the rest keeps waiting. Session grants
    /// were about the old rules, so they go.
    fn settle(&self) {
        let pending = {
            let mut state = self.inner.state.lock();
            state.grants.clear();
            std::mem::take(&mut state.pending)
        };
        let mut keep = Vec::new();
        for item in pending {
            let context = self.context_of(&item.call);
            let (decision, reason) = if context.mode == AutonomyMode::Unrestricted {
                (Decision::Allow, "Acesso Irrestrito".to_owned())
            } else {
                let verdict = self.judge(&item.call, item.read_only, &context);
                let reason = self.reason(context.mode, context.mode_source, &verdict);
                (verdict.decision, reason)
            };
            match decision {
                Decision::Ask => keep.push(item),
                Decision::Allow => {
                    self.decided(
                        &item,
                        "Permitido",
                        "approved",
                        "rules",
                        None,
                        CallOrigin::User,
                    );
                    let _ = item.reply.send(Reply::Allowed);
                }
                Decision::Deny => {
                    self.decided(&item, "Negado", "denied", "rules", None, CallOrigin::User);
                    let _ = item.reply.send(Reply::Denied(format!("Negado: {reason}")));
                }
            }
        }
        // Requests that arrived meanwhile stay after the old ones.
        let mut state = self.inner.state.lock();
        keep.append(&mut state.pending);
        state.pending = keep;
        drop(state);
        self.bump();
    }

    pub fn grants(&self) -> Vec<SessionGrant> {
        self.inner
            .state
            .lock()
            .grants
            .iter()
            .map(|(_, g)| g.clone())
            .collect()
    }

    pub fn revoke(&self, grant_id: &str) -> bool {
        let mut state = self.inner.state.lock();
        let before = state.grants.len();
        state.grants.retain(|(_, g)| g.id != grant_id);
        before != state.grants.len()
    }

    // ---- the user's choices ------------------------------------------

    fn store_settings(&self, settings: AutonomySettings) -> Result<(), String> {
        if let Some(path) = &self.inner.path {
            settings::save(path, &settings)?;
        }
        *self.inner.settings.write() = settings;
        *self.inner.warning.lock() = None;
        Ok(())
    }

    fn changed_event(&self, summary: String, data: Value, origin: CallOrigin) {
        self.audit(AuditEvent::new(
            EventKind::AutonomyChanged,
            origin,
            summary,
            data,
        ));
    }

    /// The mode of a project; `None` goes back to the default.
    pub fn set_project_mode(
        &self,
        project_id: &str,
        mode: Option<AutonomyMode>,
        origin: CallOrigin,
    ) -> Result<AutonomyMode, String> {
        let project = self
            .inner
            .store
            .project(project_id)
            .ok_or("projeto não registrado")?;
        let mut settings = self.settings();
        let previous = settings.mode_of(Some(project_id));
        match mode {
            Some(mode) => settings.projects.insert(project_id.to_owned(), mode),
            None => settings.projects.remove(project_id),
        };
        let now = settings.mode_of(Some(project_id));
        self.store_settings(settings)?;
        self.changed_event(
            format!(
                "{}: {} → {}{}",
                project.name,
                previous.label(),
                now.label(),
                if mode.is_none() { " (padrão)" } else { "" }
            ),
            json!({
                "scope": "project",
                "projectId": project_id,
                "mode": now,
                "previous": previous,
                "chosen": mode.is_some(),
            }),
            origin,
        );
        self.settle();
        Ok(now)
    }

    /// Mode of the projects the user has not chosen one for.
    pub fn set_default_mode(&self, mode: AutonomyMode, origin: CallOrigin) -> Result<(), String> {
        let mut settings = self.settings();
        let previous = settings.default_mode;
        settings.default_mode = mode;
        self.store_settings(settings)?;
        self.changed_event(
            format!("modo padrão: {} → {}", previous.label(), mode.label()),
            json!({"scope": "default", "mode": mode, "previous": previous}),
            origin,
        );
        self.settle();
        Ok(())
    }

    /// The user's rules (Autonomous). Returns them tidied.
    pub fn save_rules(
        &self,
        rules: &[PolicyRule],
        origin: CallOrigin,
    ) -> Result<Vec<PolicyRule>, String> {
        let rules = policy::validate(rules)?;
        let mut settings = self.settings();
        settings.rules = rules.clone();
        self.store_settings(settings)?;
        self.changed_event(
            format!(
                "regras do modo Autônomo salvas ({} regra{})",
                rules.len(),
                if rules.len() == 1 { "" } else { "s" }
            ),
            json!({"scope": "rules", "rules": rules.len()}),
            origin,
        );
        self.settle();
        Ok(rules)
    }

    pub fn reset_rules(&self, origin: CallOrigin) -> Result<Vec<PolicyRule>, String> {
        self.save_rules(&default_rules(), origin)
    }

    // ---- pause (section 11) ------------------------------------------

    pub fn paused_all(&self) -> bool {
        self.inner.state.lock().paused_all
    }

    pub fn is_agent_paused(&self, agent_id: &str) -> bool {
        self.inner.state.lock().paused_agents.contains(agent_id)
    }

    /// Whether a call of `agent_id` (or of a session without an agent)
    /// must wait.
    pub fn is_paused(&self, agent_id: Option<&str>) -> bool {
        let state = self.inner.state.lock();
        state.paused_all || agent_id.is_some_and(|id| state.paused_agents.contains(id))
    }

    fn pause_event(&self, kind: EventKind, summary: String, data: Value, origin: CallOrigin) {
        self.audit(AuditEvent::new(kind, origin, summary, data));
    }

    /// Holds every call of every AI until [`Self::resume_all`].
    pub fn pause_all(&self, origin: CallOrigin) -> bool {
        let changed = !std::mem::replace(&mut self.inner.state.lock().paused_all, true);
        if changed {
            self.pause_event(
                EventKind::ExecutionPaused,
                "IAs pausadas".to_owned(),
                json!({"scope": "all"}),
                origin,
            );
            self.bump();
        }
        changed
    }

    pub fn resume_all(&self, origin: CallOrigin) -> bool {
        let changed = std::mem::replace(&mut self.inner.state.lock().paused_all, false);
        if changed {
            self.pause_event(
                EventKind::ExecutionResumed,
                "IAs retomadas".to_owned(),
                json!({"scope": "all"}),
                origin,
            );
            self.bump();
        }
        changed
    }

    pub fn pause_agent(&self, agent: &Agent, origin: CallOrigin) -> bool {
        let changed = self
            .inner
            .state
            .lock()
            .paused_agents
            .insert(agent.id.to_string());
        if changed {
            self.pause_event(
                EventKind::ExecutionPaused,
                format!("agente pausado: {}", agent.title),
                json!({"scope": "agent", "agentId": agent.id, "taskId": agent.task}),
                origin,
            );
            self.bump();
        }
        changed
    }

    pub fn resume_agent(&self, agent: &Agent, origin: CallOrigin) -> bool {
        let changed = self
            .inner
            .state
            .lock()
            .paused_agents
            .remove(agent.id.as_str());
        if changed {
            self.pause_event(
                EventKind::ExecutionResumed,
                format!("agente retomado: {}", agent.title),
                json!({"scope": "agent", "agentId": agent.id, "taskId": agent.task}),
                origin,
            );
            self.bump();
        }
        changed
    }

    /// An agent ended: nothing of it is paused any more.
    pub fn forget_agent(&self, agent_id: &str) {
        if self.inner.state.lock().paused_agents.remove(agent_id) {
            self.bump();
        }
    }

    /// Waits while the call must wait. `false`: cancelled meanwhile.
    pub async fn wait_while_paused(
        &self,
        agent_id: Option<&str>,
        cancel: &CancellationToken,
    ) -> bool {
        let mut changes = self.inner.changed.subscribe();
        loop {
            if cancel.is_cancelled() {
                return false;
            }
            if !self.is_paused(agent_id) {
                return true;
            }
            tokio::select! {
                changed = changes.changed() => {
                    if changed.is_err() {
                        return true;
                    }
                }
                () = cancel.cancelled() => return false,
            }
        }
    }

    // ---- what the UI reads -------------------------------------------

    pub fn overview(&self, project_id: Option<&str>) -> AutonomyOverview {
        let settings = self.settings();
        let state = self.inner.state.lock();
        let mut paused_agents: Vec<String> = state.paused_agents.iter().cloned().collect();
        paused_agents.sort();
        AutonomyOverview {
            project_id: project_id.map(str::to_owned),
            mode: settings.mode_of(project_id),
            project_mode: project_id.and_then(|id| settings.projects.get(id).copied()),
            default_mode: settings.default_mode,
            rules: settings.rules.clone(),
            assisted_rules: assisted_rules(),
            default_rules: default_rules(),
            paused_all: state.paused_all,
            paused_agents,
            pending: state.pending.len(),
            grants: state.grants.iter().map(|(_, g)| g.clone()).collect(),
            warning: self.inner.warning.lock().clone(),
        }
    }

    /// "Experimentar": judges a call as the gate would, for a project, a
    /// mode and (optionally) rules not saved yet.
    pub fn trial(
        &self,
        project_id: Option<&str>,
        mode: Option<AutonomyMode>,
        rules: Option<Vec<PolicyRule>>,
        tool: &str,
        args: &Value,
    ) -> Result<Trial, String> {
        let tool = tool.trim();
        if tool.is_empty() {
            return Err("diga qual ferramenta".to_owned());
        }
        let mode = mode.unwrap_or_else(|| self.mode_of(project_id));
        let rules = match (mode, rules) {
            (AutonomyMode::Autonomous, Some(rules)) => policy::validate(&rules)?,
            (mode, _) => self.rules_for(mode),
        };
        let read_only = self.read_only(tool);
        let scope = Scope {
            project_root: project_id
                .and_then(|id| self.inner.store.project(id))
                .map(|p| PathBuf::from(p.path)),
            workdir: (self.inner.workdir.read())(),
        };
        if mode == AutonomyMode::Unrestricted {
            return Ok(Trial {
                mode,
                decision: Decision::Allow,
                rule: None,
                reason: "Acesso Irrestrito: nada é avaliado".to_owned(),
                targets: Vec::new(),
                opaque: false,
                unknown_tool: read_only.is_none(),
            });
        }
        let verdict = evaluate(&rules, tool, read_only.unwrap_or(false), args, &scope);
        let describe_rule = |index: Option<usize>| match index.and_then(|i| rules.get(i)) {
            Some(rule) => format!("regra {}: {}", index.unwrap_or(0) + 1, rule.describe()),
            None => "nenhuma regra casou; na dúvida, perguntar".to_owned(),
        };
        Ok(Trial {
            mode,
            decision: verdict.decision,
            rule: rule_number(verdict.rule),
            reason: describe_rule(verdict.rule),
            targets: verdict
                .units
                .iter()
                .map(|u| TrialTarget {
                    path: u.path.shown.clone(),
                    inside: u.path.inside,
                    command: u.command.clone(),
                    decision: u.decision,
                    rule: rule_number(u.rule),
                })
                .collect(),
            opaque: verdict.opaque,
            unknown_tool: read_only.is_none(),
        })
    }

    // ---- records -----------------------------------------------------

    /// A call the gate did not let through: it never reaches the runtime,
    /// so it is recorded here. A refusal is never silent.
    pub fn refused(
        &self,
        call: ToolCall,
        read_only: bool,
        error: ToolError,
        context: &CallContext,
        rule: Option<usize>,
        started_at: DateTime<Utc>,
    ) -> ToolResult {
        let finished_at = Utc::now();
        let duration_ms = (finished_at - started_at).num_milliseconds().max(0) as u64;
        self.audit(
            AuditEvent::new(
                EventKind::ToolCalled,
                call.origin.clone(),
                format!(
                    "{} {}: {}",
                    call.tool,
                    refusal_word(error.kind),
                    clip(&error.message, 200)
                ),
                json!({
                    "tool": call.tool,
                    "readOnly": read_only,
                    "args": audit_args(&call.args),
                    "ok": false,
                    "error": error,
                    "durationMs": duration_ms,
                    "autonomy": {
                        "mode": context.mode,
                        "rule": rule_number(rule),
                    },
                }),
            )
            .with_call(call.id.clone()),
        );
        ToolResult {
            call_id: call.id,
            tool: call.tool,
            ok: false,
            output: Value::Null,
            error: Some(error),
            started_at,
            finished_at,
            duration_ms,
        }
    }

    fn audit(&self, event: AuditEvent) {
        self.inner.sink.audit(event);
    }

    fn bump(&self) {
        self.inner.changed.send_modify(|n| *n = n.wrapping_add(1));
    }
}

fn refusal_word(kind: ToolErrorKind) -> &'static str {
    match kind {
        ToolErrorKind::Denied => "negado",
        ToolErrorKind::Cancelled => "cancelado",
        _ => "recusado",
    }
}
