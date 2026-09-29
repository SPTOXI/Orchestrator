//! Router and Council end to end: scripted providers answer the Council,
//! the real `SessionManager` opens the chosen sessions, and the history
//! records every step.

use async_trait::async_trait;
use orchestrator_core::{
    AuditEvent, CallOrigin, EventKind, MemorySink, SessionStatus, TokenUsage, ToolCall,
    ToolDefinition, ToolError, ToolResult,
};
use orchestrator_providers::{
    AIProvider, Completion, CompletionRequest, EchoProvider, ModelInfo, NativeSession,
    ProviderCapabilities, ProviderDescriptor, ProviderError, ProviderRegistry, ProviderStatus,
    SessionManager, SessionSpec, ToolExecutor, TurnContext, TurnInput, TurnOutput,
};
use orchestrator_router::{
    Activity, CouncilMember, CouncilMode, CouncilSettings, DecisionSource, DeliberateRequest,
    ModelRef, RouteRequest, RouteStart, RouterService,
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
        Ok(TurnOutput {
            text: format!("feito: {}", input.text),
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

/// Two "APIs" with three models to choose from, plus the judges.
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

/// The candidate id (`c1`…) the prompt gave to `model`.
fn id_of(prompt: &str, model: &str) -> String {
    prompt
        .lines()
        .find(|l| l.contains(&format!("/ {model} ·")))
        .and_then(|l| l.split(' ').next())
        .unwrap_or_else(|| panic!("{model} not in the prompt:\n{prompt}"))
        .to_owned()
}

fn pick(
    model: &'static str,
    confidence: f64,
) -> impl Fn(&CompletionRequest) -> Result<String, ProviderError> {
    move |request| {
        Ok(format!(
            "{{\"choice\": \"{}\", \"confidence\": {confidence}, \"reason\": \"{model} basta.\"}}",
            id_of(&request.prompt, model)
        ))
    }
}

fn model_ref(provider: &str, model: &str) -> ModelRef {
    ModelRef {
        provider: provider.into(),
        model: model.into(),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn off_mode_answers_with_the_router_and_spends_nothing() {
    let (cloud, cheap) = catalog();
    let h = Harness::new(vec![cloud.clone(), cheap.clone()]);
    let d = h
        .router
        .deliberate(&task("Implemente o endpoint de pagamentos"))
        .await;
    assert_eq!(d.mode, CouncilMode::Off);
    assert_eq!(d.recommendation.activity, Activity::Code);
    let decision = d.decision.unwrap();
    assert_eq!(decision.source, DecisionSource::Router);
    assert_eq!(decision.model_ref, model_ref("cloud", "coder"));
    assert!(decision.reason.starts_with("Maior nota do roteador"));
    assert!(d.votes.is_empty());
    assert_eq!(d.usage, TokenUsage::default());
    assert_eq!(cloud.calls() + cheap.calls(), 0);
    assert!(h.audits(EventKind::CouncilDeliberated).is_empty());
    assert_eq!(h.router.history()[0].id, d.id);
}

#[tokio::test(flavor = "multi_thread")]
async fn the_council_votes_and_bad_answers_do_not_count() {
    let (cloud, cheap) = catalog();
    let judge_a = Arc::new(
        Scripted::new("judge-a", vec![model("j1", 1.0, 1.0, &[])]).answering(pick("mini", 0.9)),
    );
    let judge_b = Arc::new(Scripted::new("judge-b", vec![model("j2", 1.0, 1.0, &[])]).answering(
        |request| {
            Ok(format!(
                "Aqui está:\n```json\n{{\"choice\": \"{}\", \"ranking\": [\"{}\", \"{}\"], \"confidence\": 0.6}}\n```",
                id_of(&request.prompt, "mini"),
                id_of(&request.prompt, "mini"),
                id_of(&request.prompt, "coder"),
            ))
        },
    ));
    let echo: Arc<dyn AIProvider> = Arc::new(EchoProvider::new());
    let h = Harness::new(vec![cloud, cheap, judge_a.clone(), judge_b.clone(), echo]);
    h.council(
        CouncilMode::Suggest,
        &[
            ("judge-a", None),
            ("judge-b", Some("j2")),
            ("echo", None),
            ("gone", None),
        ],
    );

    let d = h
        .router
        .deliberate(&task("Resuma o README em três frases"))
        .await;
    assert_eq!(d.recommendation.activity, Activity::Summary);
    // Every registered model is a candidate, the judges' own included.
    assert_eq!(d.shortlist.len(), 6);
    let decision = d.decision.clone().unwrap();
    assert_eq!(decision.source, DecisionSource::Council);
    assert_eq!(decision.model_ref, model_ref("cheap", "mini"));
    assert_eq!(decision.agreement, Some(1.0));
    assert!(
        decision
            .reason
            .starts_with("2 de 2 membros escolheram este modelo."),
        "{}",
        decision.reason
    );
    assert!(!d.auto_apply, "Sugerir waits for the user");

    // Votes in member order; the echo and the removed provider abstain.
    assert_eq!(d.votes.len(), 4);
    assert_eq!(d.votes[0].choice, Some(model_ref("cheap", "mini")));
    assert_eq!(d.votes[0].confidence, Some(0.9));
    assert_eq!(
        d.votes[1].ranking,
        vec![model_ref("cheap", "mini"), model_ref("cloud", "coder")]
    );
    assert_eq!(d.votes[1].model.as_deref(), Some("j2"));
    assert_eq!(
        d.votes[2].error.as_deref(),
        Some("a resposta não trouxe um objeto JSON")
    );
    assert_eq!(
        d.votes[3].error.as_deref(),
        Some("provider não registrado (conexão removida ou desativada)")
    );
    // Usage and cost of every answer that came back.
    assert_eq!(d.usage.input_tokens, 200 + d.votes[2].usage.input_tokens);
    assert!((d.usage.cost_usd.unwrap() - 0.002).abs() < 1e-12);

    // What a member receives: instructions, the task and short ids; no tools.
    let request = judge_a.requests.lock()[0].clone();
    assert!(request
        .system
        .unwrap()
        .contains("Answer with ONLY one JSON object"));
    assert!(request
        .prompt
        .starts_with("Tarefa:\nResuma o README em três frases"));
    assert!(request.prompt.contains("c1 · "));
    assert!(request.prompt.contains("Atividade: summary"));

    let recorded = h.audits(EventKind::CouncilDeliberated);
    assert_eq!(recorded.len(), 1);
    let data = &recorded[0].data;
    assert_eq!(data["deliberationId"], json!(d.id));
    assert_eq!(data["decision"]["model"], "mini");
    assert_eq!(data["decision"]["source"], "council");
    assert_eq!(data["votes"].as_array().unwrap().len(), 4);
    assert_eq!(data["cached"], false);
    assert!(recorded[0]
        .summary
        .starts_with("Conselho: mini (CHEAP) · 2/4 membros válidos"));
}

#[tokio::test(flavor = "multi_thread")]
async fn the_cache_saves_tokens_until_forced_or_reconfigured() {
    let (cloud, cheap) = catalog();
    let manager = Arc::new(
        Scripted::new("manager", vec![model("m", 1.0, 1.0, &[])]).answering(pick("strong", 0.8)),
    );
    let h = Harness::new(vec![cloud, cheap, manager.clone()]);
    h.council(CouncilMode::Suggest, &[("manager", None)]);

    let first = h
        .router
        .deliberate(&task("Planeje a arquitetura das filas"))
        .await;
    assert_eq!(
        first.decision.as_ref().unwrap().model_ref,
        model_ref("cloud", "strong")
    );
    assert_eq!(first.decision.as_ref().unwrap().reason, "strong basta.");
    assert_eq!(manager.calls(), 1);

    // Same question (spacing and case aside): no new call.
    let again = h
        .router
        .deliberate(&task("  planeje a ARQUITETURA das filas "))
        .await;
    assert_eq!(manager.calls(), 1);
    assert!(again.cached);
    assert_eq!(again.cached_from.as_ref(), Some(&first.id));
    assert_ne!(again.id, first.id);
    assert_eq!(again.usage, TokenUsage::default());
    assert_eq!(again.saved_usage, Some(first.usage));
    assert_eq!(again.decision, first.decision);
    assert!(again
        .notices
        .iter()
        .any(|n| n.contains("nenhum token gasto")));
    let recorded = h.audits(EventKind::CouncilDeliberated);
    assert_eq!(recorded.len(), 2);
    assert_eq!(recorded[1].data["cached"], true);
    assert!(recorded[1].summary.starts_with("Conselho (cache):"));

    // "Deliberar de novo".
    let mut forced = task("Planeje a arquitetura das filas");
    forced.force = true;
    assert!(!h.router.deliberate(&forced).await.cached);
    assert_eq!(manager.calls(), 2);

    // Another question, and new settings, are not served from the cache.
    h.router
        .deliberate(&task("Planeje o cache distribuído"))
        .await;
    assert_eq!(manager.calls(), 3);
    h.council(CouncilMode::Suggest, &[("manager", Some("m"))]);
    h.router
        .deliberate(&task("Planeje a arquitetura das filas"))
        .await;
    assert_eq!(manager.calls(), 4);
}

#[tokio::test(flavor = "multi_thread")]
async fn without_valid_votes_the_router_decides_and_full_waits_for_the_user() {
    let (cloud, cheap) = catalog();
    let failing = Arc::new(
        Scripted::new("failing", vec![model("f", 1.0, 1.0, &[])])
            .answering(|_| Err(ProviderError::unavailable("429 rate limited"))),
    );
    let mut mute = Scripted::new("mute", vec![model("x", 1.0, 1.0, &[])]);
    mute.completion = false;
    let mut down = Scripted::new("down", vec![model("d", 1.0, 1.0, &[])]);
    down.available = false;
    let down = Arc::new(down);
    let h = Harness::new(vec![cloud, cheap, failing, Arc::new(mute), down.clone()]);
    h.council(
        CouncilMode::Full,
        &[("failing", None), ("mute", None), ("down", None)],
    );

    let outcome = h
        .router
        .run(
            &h.sessions,
            &task("Implemente o login"),
            h.dir.path().to_path_buf(),
        )
        .await
        .unwrap();
    let d = &outcome.deliberation;
    assert_eq!(d.decision.as_ref().unwrap().source, DecisionSource::Router);
    assert!(!d.auto_apply);
    assert!(
        outcome.started.is_none(),
        "Full applies only Council decisions"
    );
    assert!(d.notices[0].contains("Nenhum membro do Conselho respondeu de forma válida"));
    let errors: Vec<_> = d.votes.iter().map(|v| v.error.clone().unwrap()).collect();
    assert_eq!(
        errors,
        [
            "429 rate limited",
            "este provider não responde pedidos avulsos",
            "provider indisponível: 401 authentication failed",
        ]
    );
    // The unavailable provider's models were not candidates either.
    assert!(d
        .recommendation
        .excluded
        .iter()
        .any(|e| e.model_ref == model_ref("down", "d") && e.reason.contains("indisponível")));
    assert_eq!(down.calls(), 0);
    assert!(h.sessions.list().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_slow_member_times_out_without_holding_the_others() {
    let (cloud, cheap) = catalog();
    let mut slow =
        Scripted::new("slow", vec![model("s", 1.0, 1.0, &[])]).answering(pick("coder", 1.0));
    slow.delay = Duration::from_secs(60);
    let quick =
        Scripted::new("quick", vec![model("q", 1.0, 1.0, &[])]).answering(pick("coder", 1.0));
    let h = Harness::new(vec![cloud, cheap, Arc::new(slow), Arc::new(quick)]);
    h.council(CouncilMode::Suggest, &[("slow", None), ("quick", None)]);

    let clock = Instant::now();
    let d = h.router.deliberate(&task("Implemente o login")).await;
    assert!(
        clock.elapsed() < Duration::from_secs(15),
        "{:?}",
        clock.elapsed()
    );
    assert_eq!(d.votes[0].error.as_deref(), Some("sem resposta em 5 s"));
    let decision = d.decision.unwrap();
    assert_eq!(decision.model_ref, model_ref("cloud", "coder"));
    assert_eq!(
        decision.reason, "coder basta.",
        "one valid vote = the manager's reason"
    );
}

async fn wait_idle(h: &Harness, id: &orchestrator_core::SessionId) {
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

#[tokio::test(flavor = "multi_thread")]
async fn full_mode_opens_the_session_and_sends_the_task() {
    let (cloud, cheap) = catalog();
    let manager = Arc::new(
        Scripted::new("manager", vec![model("m", 1.0, 1.0, &[])]).answering(pick("strong", 0.9)),
    );
    let h = Harness::new(vec![cloud, cheap, manager]);
    h.council(CouncilMode::Full, &[("manager", None)]);

    let outcome = h
        .router
        .run(
            &h.sessions,
            &task("Depure o erro de timeout no worker\nDetalhes: acontece às vezes."),
            h.dir.path().to_path_buf(),
        )
        .await
        .unwrap();
    let d = &outcome.deliberation;
    assert!(d.auto_apply);
    let started = outcome.started.expect("Full opens the session");
    let session = &started.session;
    assert_eq!(session.provider.as_str(), "cloud");
    assert_eq!(session.model.as_deref(), Some("strong"));
    assert_eq!(session.title, "Depure o erro de timeout no worker");
    assert!(started.turn_id.is_some() && started.send_error.is_none());
    wait_idle(&h, &session.id).await;

    let origin = CallOrigin::Council {
        deliberation_id: Some(d.id.clone()),
    };
    let opened = h.audits(EventKind::SessionStarted);
    assert_eq!(opened[0].origin, origin);
    let decided = h.audits(EventKind::RouteDecided);
    assert_eq!(decided.len(), 1);
    assert_eq!(decided[0].origin, origin);
    assert_eq!(decided[0].data["by"], "council");
    assert_eq!(decided[0].data["followedRecommendation"], true);
    assert_eq!(decided[0].data["activity"], "debug");
    assert!(decided[0].summary.ends_with("pelo Conselho (Full)"));
    let turn = &h.audits(EventKind::TurnCompleted)[0];
    assert_eq!(turn.origin, origin);
    assert_eq!(turn.data["sessionId"], json!(session.id));
}

#[tokio::test(flavor = "multi_thread")]
async fn the_user_follows_or_overrides_a_suggestion() {
    let (cloud, cheap) = catalog();
    let manager = Arc::new(
        Scripted::new("manager", vec![model("m", 1.0, 1.0, &[])]).answering(pick("coder", 0.9)),
    );
    let h = Harness::new(vec![cloud, cheap, manager]);
    h.council(CouncilMode::Suggest, &[("manager", None)]);
    let outcome = h
        .router
        .run(
            &h.sessions,
            &task("Escreva testes para o parser"),
            h.dir.path().to_path_buf(),
        )
        .await
        .unwrap();
    assert!(outcome.started.is_none(), "Sugerir never opens on its own");
    let d = outcome.deliberation;

    let followed = h
        .router
        .start_session(
            &h.sessions,
            RouteStart {
                deliberation_id: Some(d.id.clone()),
                provider: "cloud".into(),
                model: Some("coder".into()),
                title: None,
                task: Some(d.task.clone()),
                send_task: Some(false),
            },
            h.dir.path().to_path_buf(),
            CallOrigin::User,
        )
        .await
        .unwrap();
    assert!(followed.turn_id.is_none());
    assert_eq!(followed.session.title, "Escreva testes para o parser");

    let overridden = h
        .router
        .start_session(
            &h.sessions,
            RouteStart {
                deliberation_id: Some(d.id.clone()),
                provider: "cheap".into(),
                model: None,
                title: Some("Testes baratos".into()),
                task: Some(d.task.clone()),
                send_task: None,
            },
            h.dir.path().to_path_buf(),
            CallOrigin::User,
        )
        .await
        .unwrap();
    assert_eq!(overridden.session.model.as_deref(), Some("mini"));
    assert!(
        overridden.turn_id.is_some(),
        "send_task defaults to the setting (on)"
    );
    wait_idle(&h, &overridden.session.id).await;

    let decided = h.audits(EventKind::RouteDecided);
    assert_eq!(decided.len(), 2);
    assert_eq!(decided[0].data["followedRecommendation"], true);
    assert_eq!(decided[0].data["by"], "user");
    assert!(decided[0]
        .summary
        .ends_with("pelo usuário, seguindo a recomendação"));
    assert_eq!(decided[1].data["followedRecommendation"], false);
    assert_eq!(decided[1].data["recommended"]["model"], "coder");
    assert_eq!(decided[1].origin, CallOrigin::User);

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
    assert_eq!(h.audits(EventKind::RouteDecided).len(), 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn requirements_can_leave_one_or_no_candidate() {
    let (cloud, cheap) = catalog();
    let manager = Arc::new(
        Scripted::new("manager", vec![model("m", 1.0, 1.0, &[])]).answering(pick("coder", 0.9)),
    );
    let mut notools = Scripted::new("notools", vec![model("chat", 0.1, 0.1, &[])]);
    notools.tools = false;
    let h = Harness::new(vec![cloud, cheap, manager.clone(), Arc::new(notools)]);
    h.council(CouncilMode::Full, &[("manager", None)]);

    // Only one model has 1M of context: no need to ask the Council, and
    // Full applies it.
    h.registry.replace(Arc::new(Scripted::new(
        "cloud",
        vec![ModelInfo {
            context_window: Some(1_000_000),
            ..model("strong", 15.0, 75.0, &["código"])
        }],
    )));
    let mut request = task("Implemente a migração");
    request.route.min_context = Some(500_000);
    let d = h.router.deliberate(&request).await;
    assert_eq!(d.shortlist, vec![model_ref("cloud", "strong")]);
    assert!(d.notices[0].contains("o Conselho não precisou ser consultado"));
    assert!(d.auto_apply);
    assert_eq!(manager.calls(), 0);
    assert!(d
        .recommendation
        .excluded
        .iter()
        .any(|e| e.model_ref == model_ref("notools", "chat")
            && e.reason.contains("ferramentas desligadas")));

    // Nothing fits.
    request.route.min_context = Some(2_000_000);
    let none = h.router.deliberate(&request).await;
    assert!(none.decision.is_none());
    assert!(none.notices[0].contains("Nenhum modelo cadastrado atende aos requisitos"));
    let outcome = h
        .router
        .run(&h.sessions, &request, h.dir.path().to_path_buf())
        .await
        .unwrap();
    assert!(outcome.started.is_none());
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
