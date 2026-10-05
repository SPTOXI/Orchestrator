//! Router and Council end to end: scripted providers analyze as Council
//! members (ADR-0024), the real `SessionManager` opens the sessions that
//! carry out the demand and hands them to a reserve when the first member
//! fails, and the history records every step.

use async_trait::async_trait;
use orchestrator_core::{
    AuditEvent, CallOrigin, ContextSummary, EventKind, MemorySink, SessionEvent, SessionId,
    SessionStatus, TokenUsage, ToolCall, ToolDefinition, ToolError, ToolResult, TurnStatus,
};
use orchestrator_providers::{
    AIProvider, AttachedContext, Completion, CompletionRequest, ContextRequest, ContextSource,
    ModelInfo, NativeSession, ProviderCapabilities, ProviderDescriptor, ProviderError,
    ProviderRegistry, ProviderStatus, SessionManager, SessionSpec, ToolExecutor, TurnContext,
    TurnInput, TurnOutput,
};
use orchestrator_router::{
    Activity, CouncilMember, CouncilMode, CouncilSettings, DecisionSource, DeliberateRequest,
    Deliberation, DeliberationStore, ModelRef, PlanSource, RouteRequest, RouteStart, RouterService,
};
use parking_lot::Mutex;
use serde_json::json;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tempfile::TempDir;
use tokio_util::sync::CancellationToken;

type Answer = dyn Fn(&CompletionRequest) -> Result<String, ProviderError> + Send + Sync;

/// A provider whose `complete` answers are scripted.
struct Scripted {
    id: &'static str,
    models: Vec<ModelInfo>,
    tools: bool,
    completion: bool,
    available: bool,
    delay: Duration,
    answer: Box<Answer>,
    /// Every turn fails with this.
    turn_error: Option<&'static str>,
    /// Opening a session fails with this.
    start_error: Option<&'static str>,
    calls: AtomicUsize,
    requests: Mutex<Vec<CompletionRequest>>,
}

impl Scripted {
    fn new(id: &'static str, models: Vec<ModelInfo>) -> Self {
        Self {
            id,
            models,
            tools: true,
            completion: true,
            available: true,
            delay: Duration::ZERO,
            answer: Box::new(|_| Ok("{}".into())),
            turn_error: None,
            start_error: None,
            calls: AtomicUsize::new(0),
            requests: Mutex::new(Vec::new()),
        }
    }

    fn answering(
        mut self,
        answer: impl Fn(&CompletionRequest) -> Result<String, ProviderError> + Send + Sync + 'static,
    ) -> Self {
        self.answer = Box::new(answer);
        self
    }

    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl AIProvider for Scripted {
    fn descriptor(&self) -> ProviderDescriptor {
        ProviderDescriptor {
            id: self.id.into(),
            name: self.id.to_uppercase(),
            vendor: "teste".into(),
            description: String::new(),
        }
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            tool_calls: self.tools,
            completion: self.completion,
            token_usage: true,
            default_model: self.models.first().map(|m| m.id.clone()),
            models: self.models.clone(),
            ..Default::default()
        }
    }

    async fn inspect(&self) -> ProviderStatus {
        ProviderStatus {
            available: self.available,
            version: None,
            authenticated: Some(self.available),
            detail: (!self.available).then(|| "401 authentication failed".into()),
            checked_at: chrono::Utc::now(),
        }
    }

    async fn start(&self, spec: &SessionSpec) -> Result<NativeSession, ProviderError> {
        if let Some(error) = self.start_error {
            return Err(ProviderError::unavailable(error));
        }
        Ok(NativeSession {
            reference: format!("{}-{}", self.id, spec.session_id),
            model: spec
                .model
                .clone()
                .or_else(|| self.models.first().map(|m| m.id.clone())),
            data: json!({}),
        })
    }

    async fn execute(
        &self,
        _native: &NativeSession,
        input: &TurnInput,
        _ctx: &TurnContext,
    ) -> Result<TurnOutput, ProviderError> {
        if let Some(error) = self.turn_error {
            return Err(ProviderError::failed(error));
        }
        Ok(TurnOutput {
            text: format!("{} fez: {}", self.id, input.text),
        })
    }

    async fn complete(
        &self,
        request: &CompletionRequest,
        cancel: &CancellationToken,
    ) -> Result<Completion, ProviderError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.requests.lock().push(request.clone());
        tokio::select! {
            () = tokio::time::sleep(self.delay) => {}
            () = cancel.cancelled() => return Err(ProviderError::cancelled("cancelled")),
        }
        let text = (self.answer)(request)?;
        Ok(Completion {
            text,
            model: request
                .model
                .clone()
                .or_else(|| self.models.first().map(|m| m.id.clone())),
            usage: TokenUsage {
                input_tokens: 100,
                output_tokens: 20,
                cost_usd: Some(0.001),
                ..Default::default()
            },
        })
    }
}

struct NoTools;

#[async_trait]
impl ToolExecutor for NoTools {
    fn tools(&self) -> Vec<ToolDefinition> {
        Vec::new()
    }

    async fn execute(&self, call: ToolCall) -> ToolResult {
        let now = chrono::Utc::now();
        ToolResult {
            call_id: call.id,
            tool: call.tool,
            ok: false,
            output: serde_json::Value::Null,
            error: Some(ToolError::internal("no tools in this test")),
            started_at: now,
            finished_at: now,
            duration_ms: 0,
        }
    }
}

fn model(id: &str, input: f64, output: f64, tags: &[&str]) -> ModelInfo {
    ModelInfo {
        id: id.into(),
        name: id.into(),
        context_window: Some(200_000),
        supports_tools: Some(true),
        input_price: Some(input),
        output_price: Some(output),
        tags: tags.iter().map(|t| (*t).to_owned()).collect(),
    }
}

/// The project context the members get.
struct ProjectNotes;

#[async_trait]
impl ContextSource for ProjectNotes {
    async fn build(&self, request: ContextRequest) -> Result<Option<AttachedContext>, String> {
        Ok(Some(AttachedContext {
            text: format!(
                "## PROJECT
fila-de-emails ({})",
                request.session.project_path.display()
            ),
            summary: ContextSummary::default(),
        }))
    }
}

/// Registered providers (APIs outside the Council among them) and the
/// Council's own.
struct Harness {
    router: RouterService,
    sessions: SessionManager,
    sink: Arc<MemorySink>,
    registry: Arc<ProviderRegistry>,
    dir: TempDir,
}

impl Harness {
    fn new(providers: Vec<Arc<dyn AIProvider>>) -> Self {
        let sink = Arc::new(MemorySink::new());
        let registry = Arc::new(ProviderRegistry::new(sink.clone()));
        for provider in providers {
            registry.register(provider).unwrap();
        }
        let dir = tempfile::tempdir().unwrap();
        let (router, warning) = RouterService::open(
            &dir.path().join("council.json"),
            registry.clone(),
            sink.clone(),
        );
        assert!(warning.is_none());
        let sessions = SessionManager::new(registry.clone(), Arc::new(NoTools), sink.clone());
        sessions.set_context_source(Arc::new(ProjectNotes));
        Self {
            router,
            sessions,
            sink,
            registry,
            dir,
        }
    }

    fn council(&self, mode: CouncilMode, members: &[(&str, Option<&str>)]) {
        self.router
            .save_settings(
                CouncilSettings {
                    mode,
                    members: members
                        .iter()
                        .map(|(provider, model)| CouncilMember {
                            provider: (*provider).into(),
                            model: model.map(str::to_owned),
                        })
                        .collect(),
                    timeout_secs: 5,
                    ..Default::default()
                },
                CallOrigin::User,
            )
            .unwrap();
    }

    fn audits(&self, kind: EventKind) -> Vec<AuditEvent> {
        self.sink
            .audit_events()
            .into_iter()
            .filter(|e| e.kind == kind)
            .collect()
    }
}

fn catalog() -> (Arc<Scripted>, Arc<Scripted>) {
    let cloud = Scripted::new(
        "cloud",
        vec![
            model("strong", 15.0, 75.0, &["código", "raciocínio"]),
            model("coder", 2.5, 10.0, &["código"]),
        ],
    );
    let cheap = Scripted::new(
        "cheap",
        vec![model("mini", 0.15, 0.6, &["barato", "rápido"])],
    );
    (Arc::new(cloud), Arc::new(cheap))
}

fn task(text: &str) -> DeliberateRequest {
    DeliberateRequest {
        route: RouteRequest {
            task: text.into(),
            ..Default::default()
        },
        force: false,
    }
}

fn model_ref(provider: &str, model: &str) -> ModelRef {
    ModelRef {
        provider: provider.into(),
        model: model.into(),
    }
}

fn is_synthesis(request: &CompletionRequest) -> bool {
    request
        .system
        .as_deref()
        .is_some_and(|s| s.contains("relator"))
}

/// Answers like a Council member: an analysis, or the synthesis.
fn member(name: &'static str) -> impl Fn(&CompletionRequest) -> Result<String, ProviderError> {
    move |request| {
        Ok(if is_synthesis(request) {
            format!("Plano de {name}: 1. fila")
        } else {
            format!("Análise de {name}: usar fila")
        })
    }
}

/// A member: one model, answering as `member`.
fn seat(id: &'static str, name: &'static str) -> Scripted {
    Scripted::new(id, vec![model(&format!("{id}-1"), 1.0, 1.0, &[])]).answering(member(name))
}

async fn wait_idle(h: &Harness, id: &SessionId) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let info = h.sessions.info(id).unwrap();
        if info.status == SessionStatus::Idle && info.turns > 0 {
            return;
        }
        assert!(Instant::now() < deadline, "turn did not finish");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// The first message of a session.
fn first_input(h: &Harness, id: &SessionId) -> String {
    h.sessions
        .snapshot(id)
        .unwrap()
        .entries
        .into_iter()
        .find_map(|e| match e.event {
            SessionEvent::TurnStarted { input, .. } => Some(input),
            _ => None,
        })
        .expect("a first message")
}

#[tokio::test(flavor = "multi_thread")]
async fn off_mode_answers_with_the_router_and_spends_nothing() {
    let (cloud, cheap) = catalog();
    let h = Harness::new(vec![cloud.clone(), cheap.clone()]);
    let d = h
        .router
        .deliberate(
            &h.sessions,
            &task("Implemente o endpoint de pagamentos"),
            Some(h.dir.path().to_path_buf()),
        )
        .await;
    assert_eq!(d.mode, CouncilMode::Off);
    assert_eq!(d.recommendation.activity, Activity::Code);
    let decision = d.decision.unwrap();
    assert_eq!(decision.source, DecisionSource::Router);
    assert_eq!(decision.model_ref, model_ref("cloud", "coder"));
    assert!(decision.reason.starts_with("Maior nota do roteador"));
    assert!(d.analyses.is_empty() && d.seats.is_empty());
    assert_eq!(d.usage, TokenUsage::default());
    assert_eq!(cloud.calls() + cheap.calls(), 0);
    assert!(h.audits(EventKind::CouncilDeliberated).is_empty());
    assert_eq!(h.router.history()[0].id, d.id);
}

#[tokio::test(flavor = "multi_thread")]
async fn the_council_analyzes_together_and_never_uses_models_outside_it() {
    let (cloud, cheap) = catalog();
    let claude = Arc::new(seat("claude", "Claude"));
    let gemini = Arc::new(seat("gemini", "Gemini"));
    let h = Harness::new(vec![
        cloud.clone(),
        cheap.clone(),
        claude.clone(),
        gemini.clone(),
    ]);
    h.council(CouncilMode::Suggest, &[("claude", None), ("gemini", None)]);

    let outcome = h
        .router
        .run(
            &h.sessions,
            &task("Corrigir o timeout no worker de e-mails"),
            h.dir.path().to_path_buf(),
        )
        .await;
    assert!(outcome.started.is_none(), "Sugerir waits for the user");
    let d = outcome.deliberation;
    assert!(!d.auto_apply);

    // Each member analyzed the demand with the project context.
    assert_eq!(d.analyses.len(), 2);
    assert_eq!(
        d.analyses[0].text.as_deref(),
        Some("Análise de Claude: usar fila")
    );
    assert_eq!(
        d.analyses[1].text.as_deref(),
        Some("Análise de Gemini: usar fila")
    );
    let asked = gemini.requests.lock()[0].clone();
    assert!(asked.system.unwrap().contains("Conselho de IAs"));
    assert!(asked
        .prompt
        .starts_with("Demanda:\nCorrigir o timeout no worker de e-mails"));
    assert!(
        asked.prompt.contains("## PROJECT\nfila-de-emails"),
        "{}",
        asked.prompt
    );

    // The first member joined both analyses into the plan.
    let plan = d.plan.clone().unwrap();
    assert_eq!(plan.source, PlanSource::Synthesis);
    assert_eq!(plan.by_name.as_deref(), Some("CLAUDE"));
    assert_eq!(plan.text, "Plano de Claude: 1. fila");
    let synthesis = claude.requests.lock()[1].clone();
    assert!(is_synthesis(&synthesis));
    assert!(synthesis
        .prompt
        .contains("### Análise de CLAUDE\nAnálise de Claude: usar fila"));
    assert!(synthesis
        .prompt
        .contains("### Análise de GEMINI\nAnálise de Gemini: usar fila"));
    assert_eq!(gemini.calls(), 1, "only one member writes the synthesis");

    // The first member carries it out; the other is the reserve. Models
    // outside the Council are never asked nor chosen.
    let decision = d.decision.clone().unwrap();
    assert_eq!(decision.source, DecisionSource::Council);
    assert_eq!(decision.model_ref, model_ref("claude", "claude-1"));
    assert_eq!(
        decision.reason,
        "Executa: CLAUDE, o 1º membro disponível na ordem do Conselho. Se falhar, a sessão passa para: GEMINI."
    );
    let seats: Vec<_> = d.seats.iter().map(|s| s.member.provider.as_str()).collect();
    assert_eq!(seats, ["claude", "gemini"]);
    assert_eq!(cloud.calls() + cheap.calls(), 0);

    // Three answers: two analyses and the synthesis.
    assert_eq!(d.usage.input_tokens, 300);
    let recorded = h.audits(EventKind::CouncilDeliberated);
    assert_eq!(recorded.len(), 1);
    assert_eq!(
        recorded[0].summary,
        "Conselho: 2/2 análises · plano conjunto · executa CLAUDE · US$ 0.0030"
    );
    assert_eq!(recorded[0].data["seats"][1]["provider"], "gemini");
    assert_eq!(recorded[0].data["plan"]["source"], "synthesis");

    // The user approves: the session opens with the first member and gets
    // the demand with the plan.
    let started = h
        .router
        .execute(&h.sessions, &d.id, CallOrigin::User)
        .await
        .unwrap();
    assert_eq!(started.session.provider.as_str(), "claude");
    assert_eq!(
        started.session.title,
        "Corrigir o timeout no worker de e-mails"
    );
    assert!(started.skipped.is_empty());
    wait_idle(&h, &started.session.id).await;
    let input = first_input(&h, &started.session.id);
    assert!(input.starts_with("Corrigir o timeout no worker de e-mails\n\n---\nPlano do Conselho (os membros (CLAUDE, GEMINI) analisaram juntos; síntese de CLAUDE):\n\nPlano de Claude: 1. fila"), "{input}");
    let decided = h.audits(EventKind::RouteDecided);
    assert_eq!(decided.len(), 1);
    assert!(decided[0]
        .summary
        .ends_with("pelo Conselho, com a aprovação do usuário"));
    assert_eq!(decided[0].data["reserves"][0]["provider"], "gemini");
    assert!(h
        .sessions
        .list()
        .iter()
        .all(|s| s.provider.as_str() == "claude"));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_member_that_fails_the_analysis_goes_to_the_end_of_the_line() {
    let claude = Arc::new(
        Scripted::new("claude", vec![model("claude-1", 1.0, 1.0, &[])])
            .answering(|_| Err(ProviderError::unavailable("overloaded (http 529)"))),
    );
    let gemini = Arc::new(seat("gemini", "Gemini"));
    let h = Harness::new(vec![claude.clone(), gemini.clone()]);
    h.council(CouncilMode::Full, &[("claude", None), ("gemini", None)]);

    let outcome = h
        .router
        .run(
            &h.sessions,
            &task("Agendar os e-mails"),
            h.dir.path().to_path_buf(),
        )
        .await;
    let d = &outcome.deliberation;
    assert_eq!(
        d.analyses[0].error.as_deref(),
        Some("overloaded (http 529)")
    );
    // One analysis: it is the plan, no synthesis.
    let plan = d.plan.clone().unwrap();
    assert_eq!(plan.source, PlanSource::Single);
    assert_eq!(plan.by_name.as_deref(), Some("GEMINI"));
    assert_eq!(gemini.calls(), 1);
    assert_eq!(
        d.decision.as_ref().unwrap().reason,
        "Executa: GEMINI, o 1º membro disponível na ordem do Conselho. CLAUDE falhou na análise: overloaded (http 529) e fica de reserva. Se falhar, a sessão passa para: CLAUDE."
    );
    // Full carries it out on its own, with the member that answered.
    let started = outcome.started.clone().expect("Full opens the session");
    assert_eq!(started.session.provider.as_str(), "gemini");
    wait_idle(&h, &started.session.id).await;
    assert!(first_input(&h, &started.session.id).contains("Plano do Conselho (análise de GEMINI)"));
    let decided = h.audits(EventKind::RouteDecided);
    assert_eq!(decided[0].data["by"], "council");
    assert_eq!(
        decided[0].origin,
        CallOrigin::Council {
            deliberation_id: Some(d.id.clone())
        }
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn the_session_passes_to_the_next_member_when_the_executor_fails() {
    let mut claude = seat("claude", "Claude");
    claude.turn_error = Some("o servidor está sobrecarregado (http 529)");
    let gemini = seat("gemini", "Gemini");
    let h = Harness::new(vec![Arc::new(claude), Arc::new(gemini)]);
    h.council(CouncilMode::Full, &[("claude", None), ("gemini", None)]);

    let outcome = h
        .router
        .run(
            &h.sessions,
            &task("Agendar os e-mails"),
            h.dir.path().to_path_buf(),
        )
        .await;
    let session = outcome.started.unwrap().session;
    assert_eq!(session.provider.as_str(), "claude");
    wait_idle(&h, &session.id).await;

    // Claude failed the turn: Gemini took over the same session and did it.
    let info = h.sessions.info(&session.id).unwrap();
    assert_eq!(info.provider.as_str(), "gemini");
    assert_eq!(info.last_error, None);
    let entries = h.sessions.snapshot(&session.id).unwrap().entries;
    let completed = entries
        .iter()
        .find_map(|e| match &e.event {
            SessionEvent::TurnCompleted { status, .. } => Some(*status),
            _ => None,
        })
        .unwrap();
    assert_eq!(completed, TurnStatus::Completed);
    assert!(entries.iter().any(|e| matches!(
        &e.event,
        SessionEvent::FailedOver { to_provider, .. } if to_provider.as_str() == "gemini"
    )));
    let failover = h.audits(EventKind::SessionFailover);
    assert_eq!(failover.len(), 1);
    assert_eq!(
        failover[0].summary,
        "CLAUDE falhou; GEMINI assumiu a sessão"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_member_that_cannot_open_the_session_hands_it_to_the_next() {
    let mut claude = seat("claude", "Claude");
    claude.start_error = Some("claude: comando não encontrado");
    let h = Harness::new(vec![Arc::new(claude), Arc::new(seat("gemini", "Gemini"))]);
    h.council(CouncilMode::Full, &[("claude", None), ("gemini", None)]);
    let outcome = h
        .router
        .run(
            &h.sessions,
            &task("Agendar os e-mails"),
            h.dir.path().to_path_buf(),
        )
        .await;
    let started = outcome.started.unwrap();
    assert_eq!(started.session.provider.as_str(), "gemini");
    assert_eq!(started.skipped, ["CLAUDE: claude: comando não encontrado"]);

    // Nobody can open it: Full says why, the deliberation stays.
    let mut lone = seat("lone", "Lone");
    lone.start_error = Some("sem login");
    let h = Harness::new(vec![Arc::new(lone)]);
    h.council(CouncilMode::Full, &[("lone", None)]);
    let outcome = h
        .router
        .run(
            &h.sessions,
            &task("Agendar os e-mails"),
            h.dir.path().to_path_buf(),
        )
        .await;
    assert!(outcome.started.is_none());
    assert_eq!(
        outcome.start_error.as_deref(),
        Some("nenhum membro do Conselho conseguiu abrir a sessão — LONE: sem login")
    );
    assert!(outcome.deliberation.plan.is_some());
}

#[tokio::test(flavor = "multi_thread")]
async fn the_next_member_writes_the_synthesis_when_the_first_cannot() {
    let claude =
        Scripted::new("claude", vec![model("claude-1", 1.0, 1.0, &[])]).answering(|request| {
            if is_synthesis(request) {
                Err(ProviderError::unavailable("rate limited (http 429)"))
            } else {
                Ok("Análise de Claude".into())
            }
        });
    let h = Harness::new(vec![Arc::new(claude), Arc::new(seat("gemini", "Gemini"))]);
    h.council(CouncilMode::Suggest, &[("claude", None), ("gemini", None)]);
    let d = h
        .router
        .deliberate(
            &h.sessions,
            &task("Agendar"),
            Some(h.dir.path().to_path_buf()),
        )
        .await;
    let plan = d.plan.unwrap();
    assert_eq!(plan.source, PlanSource::Synthesis);
    assert_eq!(plan.by_name.as_deref(), Some("GEMINI"));
    assert_eq!(plan.failures, ["CLAUDE: rate limited (http 429)"]);
    // Claude analyzed fine: it still carries out the plan.
    assert_eq!(d.seats[0].member.provider.as_str(), "claude");

    // Nobody can: the analyses side by side.
    let failing = |name: &'static str| {
        Scripted::new(name, vec![model("x", 1.0, 1.0, &[])]).answering(move |request| {
            if is_synthesis(request) {
                Err(ProviderError::unavailable("caiu"))
            } else {
                Ok(format!("Análise {name}"))
            }
        })
    };
    let h = Harness::new(vec![Arc::new(failing("a")), Arc::new(failing("b"))]);
    h.council(CouncilMode::Suggest, &[("a", None), ("b", None)]);
    let d = h
        .router
        .deliberate(
            &h.sessions,
            &task("Agendar"),
            Some(h.dir.path().to_path_buf()),
        )
        .await;
    let plan = d.plan.unwrap();
    assert_eq!(plan.source, PlanSource::Joined);
    assert_eq!(
        plan.text,
        "### Análise de A\nAnálise a\n\n### Análise de B\nAnálise b"
    );
    assert!(d.notices[0].starts_with("Nenhum membro conseguiu juntar as análises"));
}

#[tokio::test(flavor = "multi_thread")]
async fn without_any_analysis_the_demand_goes_alone_and_unusable_members_wait() {
    let failing = Arc::new(
        Scripted::new("failing", vec![model("f", 1.0, 1.0, &[])])
            .answering(|_| Err(ProviderError::unavailable("429 rate limited"))),
    );
    let mut mute = Scripted::new("mute", vec![model("x", 1.0, 1.0, &[])]);
    mute.completion = false;
    let mut down = Scripted::new("down", vec![model("d", 1.0, 1.0, &[])]);
    down.available = false;
    let down = Arc::new(down);
    let h = Harness::new(vec![failing, Arc::new(mute), down.clone()]);
    h.council(
        CouncilMode::Full,
        &[
            ("failing", None),
            ("mute", None),
            ("down", None),
            ("gone", None),
        ],
    );

    let outcome = h
        .router
        .run(
            &h.sessions,
            &task("Implemente o login"),
            h.dir.path().to_path_buf(),
        )
        .await;
    let d = &outcome.deliberation;
    let errors: Vec<_> = d
        .analyses
        .iter()
        .map(|a| a.error.clone().unwrap())
        .collect();
    assert_eq!(
        errors,
        [
            "429 rate limited",
            "este provider não responde pedidos avulsos",
            "provider indisponível: 401 authentication failed",
            "provider não registrado (conexão removida ou desativada)",
        ]
    );
    assert!(d.plan.is_none());
    assert!(d
        .notices
        .iter()
        .any(|n| n.contains("Nenhum membro conseguiu analisar a demanda")));
    assert_eq!(down.calls(), 0);
    // The removed member is left out of the line; the rest wait in order.
    let seats: Vec<_> = d.seats.iter().map(|s| s.member.provider.as_str()).collect();
    assert_eq!(seats, ["failing", "mute", "down"]);
    assert!(d
        .decision
        .as_ref()
        .unwrap()
        .reason
        .contains("porque nenhum membro está melhor"));
    // Full still carries out the bare demand.
    let started = outcome.started.clone().unwrap();
    wait_idle(&h, &started.session.id).await;
    assert_eq!(first_input(&h, &started.session.id), "Implemente o login");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_slow_member_times_out_without_holding_the_others() {
    let mut slow = seat("slow", "Slow");
    slow.delay = Duration::from_secs(60);
    let h = Harness::new(vec![Arc::new(slow), Arc::new(seat("quick", "Quick"))]);
    h.council(CouncilMode::Suggest, &[("slow", None), ("quick", None)]);

    let clock = Instant::now();
    let d = h
        .router
        .deliberate(
            &h.sessions,
            &task("Implemente o login"),
            Some(h.dir.path().to_path_buf()),
        )
        .await;
    assert!(
        clock.elapsed() < Duration::from_secs(15),
        "{:?}",
        clock.elapsed()
    );
    assert_eq!(d.analyses[0].error.as_deref(), Some("sem resposta em 5 s"));
    assert_eq!(d.plan.unwrap().by_name.as_deref(), Some("QUICK"));
    assert_eq!(d.seats[0].member.provider.as_str(), "quick");
}

#[tokio::test(flavor = "multi_thread")]
async fn the_cache_saves_tokens_until_forced_or_reconfigured() {
    let manager = Arc::new(seat("manager", "Manager"));
    let h = Harness::new(vec![manager.clone()]);
    h.council(CouncilMode::Suggest, &[("manager", None)]);
    let here = Some(h.dir.path().to_path_buf());

    let first = h
        .router
        .deliberate(
            &h.sessions,
            &task("Planeje a arquitetura das filas"),
            here.clone(),
        )
        .await;
    assert_eq!(manager.calls(), 1);

    // Same demand (spacing and case aside), same project: no new call.
    let again = h
        .router
        .deliberate(
            &h.sessions,
            &task("  planeje a ARQUITETURA das filas "),
            here.clone(),
        )
        .await;
    assert_eq!(manager.calls(), 1);
    assert!(again.cached);
    assert_eq!(again.cached_from.as_ref(), Some(&first.id));
    assert_eq!(again.usage, TokenUsage::default());
    assert_eq!(again.saved_usage, Some(first.usage));
    assert_eq!(again.plan, first.plan);
    assert!(again
        .notices
        .iter()
        .any(|n| n.contains("nenhum token gasto")));
    let recorded = h.audits(EventKind::CouncilDeliberated);
    assert_eq!(recorded[1].data["cached"], true);
    assert!(recorded[1].summary.starts_with("Conselho (cache):"));

    // "Analisar de novo", another project, another demand and new settings
    // are not served from the cache.
    let mut forced = task("Planeje a arquitetura das filas");
    forced.force = true;
    assert!(
        !h.router
            .deliberate(&h.sessions, &forced, here.clone())
            .await
            .cached
    );
    assert_eq!(manager.calls(), 2);
    h.router
        .deliberate(
            &h.sessions,
            &task("Planeje a arquitetura das filas"),
            Some("/outro".into()),
        )
        .await;
    assert_eq!(manager.calls(), 3);
    h.router
        .deliberate(
            &h.sessions,
            &task("Planeje o cache distribuído"),
            here.clone(),
        )
        .await;
    assert_eq!(manager.calls(), 4);
    h.council(CouncilMode::Suggest, &[("manager", Some("manager-1"))]);
    h.router
        .deliberate(&h.sessions, &task("Planeje a arquitetura das filas"), here)
        .await;
    assert_eq!(manager.calls(), 5);
}

#[tokio::test(flavor = "multi_thread")]
async fn the_user_can_still_pick_any_model_by_hand() {
    let (cloud, cheap) = catalog();
    let h = Harness::new(vec![cloud, cheap, Arc::new(seat("claude", "Claude"))]);
    h.council(CouncilMode::Suggest, &[("claude", None)]);
    let started = h
        .router
        .start_session(
            &h.sessions,
            RouteStart {
                deliberation_id: None,
                provider: "cheap".into(),
                model: None,
                title: Some("Testes baratos".into()),
                task: Some("Escreva testes".into()),
                send_task: None,
            },
            h.dir.path().to_path_buf(),
            CallOrigin::User,
        )
        .await
        .unwrap();
    assert_eq!(started.session.model.as_deref(), Some("mini"));
    assert!(
        started.turn_id.is_some(),
        "send_task defaults to the setting (on)"
    );
    wait_idle(&h, &started.session.id).await;
    let decided = h.audits(EventKind::RouteDecided);
    assert!(decided[0].summary.ends_with("pelo usuário"));

    // A provider that is not registered fails before anything is recorded.
    let missing = h
        .router
        .start_session(
            &h.sessions,
            RouteStart {
                deliberation_id: None,
                provider: "nope".into(),
                model: None,
                title: None,
                task: None,
                send_task: None,
            },
            h.dir.path().to_path_buf(),
            CallOrigin::User,
        )
        .await;
    assert!(missing.is_err());
    assert_eq!(h.audits(EventKind::RouteDecided).len(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn settings_are_validated_persisted_and_recorded() {
    let (cloud, cheap) = catalog();
    let h = Harness::new(vec![cloud, cheap]);
    let invalid = h.router.save_settings(
        CouncilSettings {
            mode: CouncilMode::Full,
            ..Default::default()
        },
        CallOrigin::User,
    );
    assert!(invalid
        .unwrap_err()
        .message
        .contains("pelo menos um membro"));
    assert!(h.audits(EventKind::CouncilConfigured).is_empty());

    h.council(CouncilMode::Suggest, &[("cloud", Some("coder"))]);
    let recorded = h.audits(EventKind::CouncilConfigured);
    assert_eq!(recorded.len(), 1);
    assert_eq!(recorded[0].summary, "Conselho: modo Sugerir · 1 membro");
    assert_eq!(recorded[0].data["members"][0]["model"], "coder");

    // A new service reads the saved file.
    let (reopened, warning) = RouterService::open(
        &h.dir.path().join("council.json"),
        h.registry.clone(),
        h.sink.clone(),
    );
    assert!(warning.is_none());
    assert_eq!(reopened.settings(), h.router.settings());
    assert_eq!(reopened.settings().mode, CouncilMode::Suggest);
}

/// A stored deliberation and its cache key and validity.
type Row = (
    Deliberation,
    Option<(String, chrono::DateTime<chrono::Utc>)>,
);

/// Keeps deliberations like the app's database would: through JSON.
#[derive(Default)]
struct TestStore {
    rows: Mutex<Vec<Row>>,
}

impl DeliberationStore for TestStore {
    fn save(
        &self,
        deliberation: &Deliberation,
        cache: Option<(&str, chrono::DateTime<chrono::Utc>)>,
    ) {
        let json = serde_json::to_value(deliberation).unwrap();
        let back: Deliberation = serde_json::from_value(json).unwrap();
        assert_eq!(&back, deliberation, "a deliberation survives JSON");
        self.rows
            .lock()
            .push((back, cache.map(|(key, until)| (key.to_owned(), until))));
    }

    fn cached(&self, key: &str, now: chrono::DateTime<chrono::Utc>) -> Option<Deliberation> {
        self.rows
            .lock()
            .iter()
            .rev()
            .find(|(_, cache)| {
                cache
                    .as_ref()
                    .is_some_and(|(k, until)| k == key && *until > now)
            })
            .map(|(d, _)| d.clone())
    }

    fn recent(&self, limit: usize) -> Vec<Deliberation> {
        self.rows
            .lock()
            .iter()
            .rev()
            .take(limit)
            .map(|(d, _)| d.clone())
            .collect()
    }

    fn clear_cache(&self) {
        for row in self.rows.lock().iter_mut() {
            row.1 = None;
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn deliberations_and_the_cache_survive_a_restart() {
    let manager = Arc::new(seat("manager", "Manager"));
    let h = Harness::new(vec![manager.clone()]);
    h.council(CouncilMode::Suggest, &[("manager", None)]);
    let store = Arc::new(TestStore::default());
    let open = || {
        RouterService::open(
            &h.dir.path().join("council.json"),
            h.registry.clone(),
            h.sink.clone(),
        )
        .0
        .with_store(store.clone())
    };
    let here = || Some(h.dir.path().to_path_buf());

    let first = open();
    let off_topic = first
        .deliberate(&h.sessions, &task("Resuma o README"), here())
        .await;
    let decided = first
        .deliberate(
            &h.sessions,
            &task("Planeje a arquitetura das filas"),
            here(),
        )
        .await;
    assert_eq!(manager.calls(), 2);
    drop(first);

    // After a restart: the history is back, the same demand costs nothing
    // and a stored deliberation can still be carried out.
    let second = open();
    let ids: Vec<_> = second.history().into_iter().map(|d| d.id).collect();
    assert_eq!(ids, [decided.id.clone(), off_topic.id.clone()]);
    let again = second
        .deliberate(
            &h.sessions,
            &task("Planeje a arquitetura das filas"),
            here(),
        )
        .await;
    assert!(again.cached);
    assert_eq!(again.cached_from.as_ref(), Some(&decided.id));
    assert_eq!(again.plan, decided.plan);
    assert_eq!(manager.calls(), 2);
    let started = second
        .execute(&h.sessions, &decided.id, CallOrigin::User)
        .await
        .unwrap();
    assert_eq!(started.session.provider.as_str(), "manager");

    // New settings drop the stored cache too.
    second
        .save_settings(second.settings(), CallOrigin::User)
        .unwrap();
    let fresh = second
        .deliberate(
            &h.sessions,
            &task("Planeje a arquitetura das filas"),
            here(),
        )
        .await;
    assert!(!fresh.cached);
    assert_eq!(manager.calls(), 3);
    assert_eq!(store.rows.lock().len(), 4);
}

#[test]
fn deliberations_from_before_the_joint_analysis_still_read() {
    // A deliberation stored by 0.1.0, when the Council voted for a model.
    let old = json!({
        "id": "d-1",
        "createdAt": "2026-10-01T12:00:00Z",
        "task": "Resuma o README",
        "mode": "suggest",
        "recommendation": {
            "activity": "summary", "detected": true, "preference": "cost",
            "needsTools": false, "minContext": null, "candidates": [], "excluded": []
        },
        "shortlist": [{"provider": "cheap", "model": "mini"}],
        "votes": [{
            "member": {"provider": "judge", "model": null}, "providerName": "Judge",
            "model": "j1", "choice": {"provider": "cheap", "model": "mini"},
            "ranking": [], "confidence": 0.9, "reason": "basta", "error": null,
            "usage": {"inputTokens": 10, "outputTokens": 2}, "durationMs": 5
        }],
        "decision": {
            "provider": "cheap", "model": "mini", "providerName": "Cheap", "modelName": "mini",
            "source": "council", "reason": "1 de 1", "agreement": 1.0
        },
        "usage": {"inputTokens": 10, "outputTokens": 2},
        "cached": false, "cachedFrom": null, "savedUsage": null,
        "notices": [], "autoApply": false, "durationMs": 7
    });
    let read: Deliberation = serde_json::from_value(old).unwrap();
    assert_eq!(read.votes.len(), 1);
    assert!(read.analyses.is_empty() && read.plan.is_none() && read.seats.is_empty());
    assert_eq!(read.decision.unwrap().agreement, Some(1.0));
}
