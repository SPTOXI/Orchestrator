//! `RouterService`: the router, the Council (ADR-0011, ADR-0024), its
//! settings, cache and history, and the sessions they open.

use crate::activity::words;
use crate::cache::DeliberationCache;
use crate::catalog::{catalog, Availability, CatalogModel};
use crate::council::{
    analysis_prompt, clip_analysis, clip_plan, execution_message, joined_plan, synthesis_prompt,
    Analysis, Decision, DecisionSource, Deliberation, Plan, PlanSource, Seat, ANALYSIS_SYSTEM,
    SYNTHESIS_SYSTEM,
};
use crate::score::{rank, ModelRef, Recommendation, RouteRequest};
use crate::settings::{self, CouncilMember, CouncilMode, CouncilSettings};
use crate::store::DeliberationStore;
use chrono::Utc;
use orchestrator_core::{
    AuditEvent, CallOrigin, DeliberationId, EventKind, EventSink, ProviderId, SessionInfo,
    TokenUsage, TurnId,
};
use orchestrator_providers::{
    CompletionRequest, ProviderError, ProviderRegistry, ProviderStatus, Reserve, SessionManager,
    StartRequest,
};
use parking_lot::{Mutex, RwLock};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::hash_map::DefaultHasher;
use std::collections::VecDeque;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

/// Deliberations kept for the UI.
const HISTORY: usize = 50;
/// Task characters recorded in the history.
const AUDIT_TASK_CHARS: usize = 500;
/// Longest session title taken from a task.
const TITLE_CHARS: usize = 60;

/// A deliberation request.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct DeliberateRequest {
    #[serde(flatten)]
    pub route: RouteRequest,
    /// Ignore the cache ("Analisar de novo").
    pub force: bool,
}

/// Opens a session with a model the user picked.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RouteStart {
    /// Deliberation that recommended a model, if any.
    #[serde(default)]
    pub deliberation_id: Option<DeliberationId>,
    pub provider: ProviderId,
    /// `None` = the provider's default model.
    #[serde(default)]
    pub model: Option<String>,
    /// `None` = taken from the task.
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub task: Option<String>,
    /// Send the task as the first message; `None` = the Council setting.
    #[serde(default)]
    pub send_task: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RouteStarted {
    pub session: SessionInfo,
    /// Turn that carries the task, when it was sent.
    pub turn_id: Option<TurnId>,
    /// Why the task could not be sent (the session is open anyway).
    pub send_error: Option<String>,
    /// Council members that could not open the session, and why: the next
    /// one in line opened it (ADR-0024).
    pub skipped: Vec<String>,
}

/// A deliberation and, in Full mode, the session it opened.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunOutcome {
    pub deliberation: Deliberation,
    pub started: Option<RouteStarted>,
    /// Why Full mode could not open the session.
    pub start_error: Option<String>,
}

pub struct RouterService {
    registry: Arc<ProviderRegistry>,
    sink: Arc<dyn EventSink>,
    path: PathBuf,
    settings: RwLock<CouncilSettings>,
    availability: Availability,
    cache: DeliberationCache,
    history: Mutex<VecDeque<Deliberation>>,
    /// Deliberations kept between runs (ADR-0012).
    store: Option<Arc<dyn DeliberationStore>>,
}

/// A member's answer to one request.
struct Answer {
    text: String,
    model: Option<String>,
    usage: TokenUsage,
}

/// Asks a member, without a session and without tools.
async fn ask(
    registry: &ProviderRegistry,
    member: &CouncilMember,
    known: Option<ProviderStatus>,
    system: &str,
    prompt: String,
    timeout: Duration,
) -> Result<Answer, String> {
    let provider = registry
        .get(&member.provider)
        .ok_or("provider não registrado (conexão removida ou desativada)")?;
    if !provider.capabilities().completion {
        return Err("este provider não responde pedidos avulsos".into());
    }
    if let Some(status) = known.filter(|s| !s.available) {
        return Err(format!(
            "provider indisponível: {}",
            status.detail.unwrap_or_default()
        ));
    }
    let request = CompletionRequest {
        model: member.model.clone(),
        system: Some(system.into()),
        prompt,
    };
    let cancel = CancellationToken::new();
    match tokio::time::timeout(timeout, provider.complete(&request, &cancel)).await {
        Ok(Ok(answer)) if answer.text.trim().is_empty() => Err("resposta vazia".into()),
        Ok(Ok(answer)) => Ok(Answer {
            text: answer.text,
            model: answer.model,
            usage: answer.usage,
        }),
        Ok(Err(err)) => Err(err.message),
        Err(_) => {
            cancel.cancel();
            Err(format!("sem resposta em {} s", timeout.as_secs()))
        }
    }
}

/// `Claude` or, for a member with its own model, `Claude (claude-opus)`.
fn label(provider_name: &str, model: Option<&str>) -> String {
    match model {
        Some(model) => format!("{provider_name} ({model})"),
        None => provider_name.to_owned(),
    }
}

fn seat_label(seat: &Seat) -> String {
    label(&seat.provider_name, seat.member.model.as_deref())
}

fn analysis_label(analysis: &Analysis) -> String {
    label(&analysis.provider_name, analysis.member.model.as_deref())
}

impl RouterService {
    /// Loads the settings from `path` (`council.json`). An unreadable file
    /// turns the Council off and comes back as a warning.
    pub fn open(
        path: &Path,
        registry: Arc<ProviderRegistry>,
        sink: Arc<dyn EventSink>,
    ) -> (Self, Option<String>) {
        let (settings, warning) = settings::load(path);
        (
            Self {
                registry,
                sink,
                path: path.to_path_buf(),
                settings: RwLock::new(settings),
                availability: Availability::default(),
                cache: DeliberationCache::default(),
                history: Mutex::new(VecDeque::new()),
                store: None,
            },
            warning,
        )
    }

    /// Keeps deliberations in `store`: the history starts with the stored
    /// ones and the cache survives restarts (ADR-0012).
    pub fn with_store(mut self, store: Arc<dyn DeliberationStore>) -> Self {
        *self.history.lock() = store.recent(HISTORY).into();
        self.store = Some(store);
        self
    }

    pub fn settings(&self) -> CouncilSettings {
        self.settings.read().clone()
    }

    /// Validates, stores and records the settings (`COUNCIL_CONFIGURED`).
    /// Cached deliberations are dropped: they were made by other members.
    pub fn save_settings(
        &self,
        settings: CouncilSettings,
        origin: CallOrigin,
    ) -> Result<CouncilSettings, ProviderError> {
        settings.validate()?;
        settings::save(&self.path, &settings)?;
        *self.settings.write() = settings.clone();
        self.cache.clear();
        if let Some(store) = &self.store {
            store.clear_cache();
        }
        let members: Vec<String> = settings.members.iter().map(member_label).collect();
        self.sink.audit(AuditEvent::new(
            EventKind::CouncilConfigured,
            origin,
            format!(
                "Conselho: modo {} · {} membro{}",
                settings.mode.label(),
                settings.members.len(),
                if settings.members.len() == 1 { "" } else { "s" }
            ),
            json!({
                "mode": settings.mode,
                "members": settings.members,
                "memberLabels": members,
                "cacheMinutes": settings.cache_minutes,
                "timeoutSecs": settings.timeout_secs,
                "preference": settings.preference,
                "sendTask": settings.send_task,
            }),
        ));
        Ok(settings)
    }

    /// Forgets a provider's availability (e.g. its connection was edited).
    pub fn forget_availability(&self, id: &ProviderId) {
        self.availability.forget(id);
    }

    /// Every registered model with the cached availability.
    pub fn catalog(&self) -> Vec<CatalogModel> {
        catalog(&self.registry, &self.availability)
    }

    /// The router's ranking (no tokens). Providers not inspected in the
    /// last minutes are inspected first.
    pub async fn recommend(&self, request: &RouteRequest) -> Recommendation {
        self.availability.refresh(&self.registry).await;
        rank(&self.catalog(), request, self.settings.read().preference)
    }

    /// Newest first.
    pub fn history(&self) -> Vec<Deliberation> {
        self.history.lock().iter().cloned().collect()
    }

    pub fn deliberation(&self, id: &DeliberationId) -> Option<Deliberation> {
        self.history.lock().iter().find(|d| &d.id == id).cloned()
    }

    /// With the Council off, the router's best model. With it on (Sugerir
    /// and Full), the members analyze the demand together with the context
    /// of `project_path`, one of them writes the plan and the members are
    /// lined up to carry it out (ADR-0024).
    pub async fn deliberate(
        &self,
        sessions: &SessionManager,
        request: &DeliberateRequest,
        project_path: Option<PathBuf>,
    ) -> Deliberation {
        let started = Instant::now();
        let settings = self.settings();
        let recommendation = self.recommend(&request.route).await;
        let task = request.route.task.trim().to_owned();
        let mut deliberation = Deliberation {
            id: DeliberationId::new(),
            created_at: Utc::now(),
            task: task.clone(),
            mode: settings.mode,
            recommendation: recommendation.clone(),
            shortlist: Vec::new(),
            votes: Vec::new(),
            analyses: Vec::new(),
            plan: None,
            seats: Vec::new(),
            project_path: project_path.clone(),
            decision: None,
            usage: TokenUsage::default(),
            cached: false,
            cached_from: None,
            saved_usage: None,
            notices: Vec::new(),
            auto_apply: false,
            duration_ms: 0,
        };

        if settings.mode == CouncilMode::Off {
            match recommendation.candidates.first() {
                Some(best) => {
                    deliberation.decision = Some(Decision {
                        model_ref: best.model_ref.clone(),
                        provider_name: best.provider_name.clone(),
                        model_name: best.model_name.clone(),
                        source: DecisionSource::Router,
                        reason: format!("Maior nota do roteador ({}).", best.score),
                        agreement: None,
                    })
                }
                None => deliberation
                    .notices
                    .push(if recommendation.excluded.is_empty() {
                        "Nenhum modelo cadastrado: adicione uma API em AI PROVIDERS.".into()
                    } else {
                        "Nenhum modelo cadastrado atende aos requisitos (veja os excluídos).".into()
                    }),
            }
            return self.finish(deliberation, started, false, None);
        }

        let ttl = Duration::from_secs(u64::from(settings.cache_minutes) * 60);
        let key = cache_key(&task, project_path.as_deref(), &settings);
        let hit = if settings.cache_minutes > 0 && !request.force {
            self.cache.get(key, ttl).or_else(|| {
                let store = self.store.as_ref()?;
                store.cached(&format!("{key:016x}"), Utc::now())
            })
        } else {
            None
        };
        let mut cache_for = None;
        match hit {
            Some(hit) => {
                deliberation.analyses = hit.analyses;
                deliberation.plan = hit.plan;
                deliberation.cached = true;
                deliberation.cached_from = Some(hit.id);
                deliberation.saved_usage = Some(hit.usage);
                deliberation.notices = hit.notices;
                deliberation.notices.push(format!(
                    "Análise do cache do Conselho (validade de {} min): nenhum token gasto. \
                     \"Analisar de novo\" consulta os membros outra vez.",
                    settings.cache_minutes
                ));
            }
            None => {
                let context = match &project_path {
                    Some(path) => match sessions.project_context(path.clone(), &task).await {
                        Ok(built) => built.map(|c| c.text),
                        Err(err) => {
                            deliberation.notices.push(format!(
                                "Contexto do projeto indisponível ({err}): os membros analisaram só pela demanda."
                            ));
                            None
                        }
                    },
                    None => {
                        deliberation.notices.push(
                            "Nenhum projeto aberto: os membros analisaram só pela demanda.".into(),
                        );
                        None
                    }
                };
                let question = analysis_prompt(&task, context.as_deref());
                deliberation.analyses = self.analyze(&settings, &question).await;
                deliberation.plan = self
                    .synthesize(&settings, &task, &deliberation.analyses)
                    .await;
                for analysis in &deliberation.analyses {
                    deliberation.usage += analysis.usage;
                }
                if let Some(plan) = &deliberation.plan {
                    deliberation.usage += plan.usage;
                    if plan.source == PlanSource::Joined {
                        deliberation.notices.push(format!(
                            "Nenhum membro conseguiu juntar as análises ({}): o plano são as análises lado a lado.",
                            plan.failures.join(" · ")
                        ));
                    }
                } else {
                    deliberation.notices.push(
                        "Nenhum membro conseguiu analisar a demanda: ela segue sem plano para quem executa."
                            .into(),
                    );
                }
            }
        }

        deliberation.seats = self.seats(&settings, &deliberation.analyses);
        match deliberation.seats.first() {
            Some(first) => {
                deliberation.decision = Some(Decision {
                    model_ref: ModelRef {
                        provider: first.member.provider.clone(),
                        model: first.member.model.clone().unwrap_or_else(|| {
                            self.registry
                                .get(&first.member.provider)
                                .and_then(|p| p.capabilities().default_model)
                                .unwrap_or_default()
                        }),
                    },
                    provider_name: first.provider_name.clone(),
                    model_name: first.model_name.clone(),
                    source: DecisionSource::Council,
                    reason: seats_reason(&deliberation.seats),
                    agreement: None,
                });
                deliberation.auto_apply = settings.mode == CouncilMode::Full;
            }
            None => deliberation.notices.push(
                "Nenhum membro do Conselho está cadastrado: revise os membros em Conselho.".into(),
            ),
        }
        if !deliberation.cached && deliberation.plan.is_some() && settings.cache_minutes > 0 {
            self.cache.put(key, deliberation.clone());
            cache_for = Some((key, ttl));
        }
        self.finish(deliberation, started, true, cache_for)
    }

    /// Every member analyzes the demand, in parallel. Analyses come back in
    /// Council order.
    async fn analyze(&self, settings: &CouncilSettings, question: &str) -> Vec<Analysis> {
        let timeout = Duration::from_secs(u64::from(settings.timeout_secs));
        let mut jobs = JoinSet::new();
        for (index, member) in settings.members.iter().cloned().enumerate() {
            let registry = self.registry.clone();
            let known = self.availability.get(&member.provider);
            let question = question.to_owned();
            jobs.spawn(async move {
                let started = Instant::now();
                let mut analysis = Analysis {
                    provider_name: registry
                        .get(&member.provider)
                        .map_or_else(|| member.provider.to_string(), |p| p.descriptor().name),
                    model: member.model.clone(),
                    member: member.clone(),
                    text: None,
                    error: None,
                    usage: TokenUsage::default(),
                    duration_ms: 0,
                };
                match ask(
                    &registry,
                    &member,
                    known,
                    ANALYSIS_SYSTEM,
                    question,
                    timeout,
                )
                .await
                {
                    Ok(answer) => {
                        analysis.text = Some(clip_analysis(&answer.text));
                        analysis.usage = answer.usage;
                        if answer.model.is_some() {
                            analysis.model = answer.model;
                        }
                    }
                    Err(error) => analysis.error = Some(error),
                }
                analysis.duration_ms = started.elapsed().as_millis() as u64;
                (index, analysis)
            });
        }
        let mut answers = Vec::new();
        while let Some(joined) = jobs.join_next().await {
            match joined {
                Ok(answer) => answers.push(answer),
                Err(err) => eprintln!("[orchestrator] council member task failed: {err}"),
            }
        }
        answers.sort_by_key(|(index, _)| *index);
        answers.into_iter().map(|(_, analysis)| analysis).collect()
    }

    /// The plan: with two or more analyses, the first member that answered
    /// joins them (the next one if it fails); with one, that analysis.
    async fn synthesize(
        &self,
        settings: &CouncilSettings,
        task: &str,
        analyses: &[Analysis],
    ) -> Option<Plan> {
        let done: Vec<(&Analysis, String)> = analyses
            .iter()
            .filter_map(|a| a.text.clone().map(|text| (a, text)))
            .collect();
        let labeled: Vec<(String, String)> = done
            .iter()
            .map(|(a, text)| (analysis_label(a), text.clone()))
            .collect();
        match done.as_slice() {
            [] => None,
            [(only, text)] => Some(Plan {
                text: clip_plan(text),
                source: PlanSource::Single,
                by: Some(only.member.clone()),
                by_name: Some(analysis_label(only)),
                usage: TokenUsage::default(),
                failures: Vec::new(),
            }),
            _ => {
                let timeout = Duration::from_secs(u64::from(settings.timeout_secs));
                let prompt = synthesis_prompt(task, &labeled);
                let mut failures = Vec::new();
                for (analysis, _) in &done {
                    let known = self.availability.get(&analysis.member.provider);
                    match ask(
                        &self.registry,
                        &analysis.member,
                        known,
                        SYNTHESIS_SYSTEM,
                        prompt.clone(),
                        timeout,
                    )
                    .await
                    {
                        Ok(answer) => {
                            return Some(Plan {
                                text: clip_plan(&answer.text),
                                source: PlanSource::Synthesis,
                                by: Some(analysis.member.clone()),
                                by_name: Some(analysis_label(analysis)),
                                usage: answer.usage,
                                failures,
                            })
                        }
                        Err(error) => {
                            failures.push(format!("{}: {error}", analysis_label(analysis)))
                        }
                    }
                }
                Some(Plan {
                    text: joined_plan(&labeled),
                    source: PlanSource::Joined,
                    by: None,
                    by_name: None,
                    usage: TokenUsage::default(),
                    failures,
                })
            }
        }
    }

    /// The members in line to carry out the demand: Council order, with
    /// the ones whose provider is down or that failed the analysis moved to
    /// the end (they stay as reserves). Members no longer registered are
    /// left out.
    fn seats(&self, settings: &CouncilSettings, analyses: &[Analysis]) -> Vec<Seat> {
        let mut ready = Vec::new();
        let mut later = Vec::new();
        for member in &settings.members {
            let Some(provider) = self.registry.get(&member.provider) else {
                continue;
            };
            let model_name = member
                .model
                .clone()
                .or_else(|| provider.capabilities().default_model)
                .unwrap_or_else(|| "modelo padrão".into());
            let down = self
                .availability
                .get(&member.provider)
                .filter(|s| !s.available)
                .map(|s| {
                    format!(
                        "provider indisponível{}",
                        s.detail.map(|d| format!(": {d}")).unwrap_or_default()
                    )
                });
            let failed = analyses
                .iter()
                .find(|a| &a.member == member)
                .and_then(|a| a.error.clone())
                .map(|e| format!("falhou na análise: {e}"));
            let seat = Seat {
                member: member.clone(),
                provider_name: provider.descriptor().name,
                model_name,
                demoted: down.or(failed),
            };
            if seat.demoted.is_some() {
                later.push(seat);
            } else {
                ready.push(seat);
            }
        }
        ready.extend(later);
        ready
    }

    /// Stamps the duration, keeps the deliberation for the UI and, when the
    /// Council was involved, records `COUNCIL_DELIBERATED`.
    fn finish(
        &self,
        mut deliberation: Deliberation,
        started: Instant,
        record: bool,
        cache: Option<(u64, Duration)>,
    ) -> Deliberation {
        deliberation.duration_ms = started.elapsed().as_millis() as u64;
        {
            let mut history = self.history.lock();
            history.push_front(deliberation.clone());
            history.truncate(HISTORY);
        }
        if let Some(store) = &self.store {
            let key = cache.map(|(key, ttl)| {
                let valid = chrono::Duration::from_std(ttl).unwrap_or_default();
                (format!("{key:016x}"), deliberation.created_at + valid)
            });
            store.save(
                &deliberation,
                key.as_ref().map(|(key, until)| (key.as_str(), *until)),
            );
        }
        if record {
            self.audit_deliberation(&deliberation);
        }
        deliberation
    }

    fn audit_deliberation(&self, d: &Deliberation) {
        let answered = d.analyses.iter().filter(|a| a.text.is_some()).count();
        let summary = format!(
            "Conselho{}: {answered}/{} análises · {}{}{}",
            if d.cached { " (cache)" } else { "" },
            d.analyses.len(),
            match d.plan.as_ref().map(|p| p.source) {
                Some(PlanSource::Synthesis) => "plano conjunto",
                Some(PlanSource::Single) => "plano de um membro",
                Some(PlanSource::Joined) => "análises lado a lado",
                None => "sem plano",
            },
            d.seats
                .first()
                .map(|s| format!(" · executa {}", seat_label(s)))
                .unwrap_or_default(),
            d.usage
                .cost_usd
                .map(|c| format!(" · US$ {c:.4}"))
                .unwrap_or_default()
        );
        let analyses: Vec<_> = d
            .analyses
            .iter()
            .map(|a| {
                json!({
                    "provider": a.member.provider,
                    "model": a.model,
                    "error": a.error,
                    "chars": a.text.as_ref().map(|t| t.chars().count()),
                    "usage": a.usage,
                    "durationMs": a.duration_ms,
                })
            })
            .collect();
        let seats: Vec<_> = d
            .seats
            .iter()
            .map(|s| {
                json!({
                    "provider": s.member.provider,
                    "model": s.member.model,
                    "demoted": s.demoted,
                })
            })
            .collect();
        self.sink.audit(AuditEvent::new(
            EventKind::CouncilDeliberated,
            CallOrigin::User,
            summary,
            json!({
                "deliberationId": d.id,
                "task": truncate_chars(&d.task, AUDIT_TASK_CHARS),
                "projectPath": d.project_path,
                "mode": d.mode,
                "analyses": analyses,
                "plan": d.plan.as_ref().map(|p| json!({
                    "source": p.source,
                    "by": p.by,
                    "usage": p.usage,
                    "failures": p.failures,
                })),
                "seats": seats,
                "decision": d.decision.as_ref().map(|x| json!({
                    "provider": x.model_ref.provider,
                    "model": x.model_ref.model,
                    "source": x.source,
                })),
                "usage": d.usage,
                "cached": d.cached,
                "cachedFrom": d.cached_from,
                "autoApply": d.auto_apply,
                "durationMs": d.duration_ms,
            }),
        ));
    }

    /// Records `ROUTE_DECIDED` for a session opened with a chosen model.
    fn record_route(
        &self,
        session: &SessionInfo,
        deliberation: Option<&Deliberation>,
        origin: &CallOrigin,
    ) {
        let chosen = ModelRef {
            provider: session.provider.clone(),
            model: session.model.clone().unwrap_or_default(),
        };
        let recommended = deliberation
            .and_then(|d| d.decision.as_ref())
            .map(|d| d.model_ref.clone());
        let followed = recommended.as_ref().map(|r| *r == chosen);
        let by = match origin {
            CallOrigin::Council { .. } => "council",
            _ => "user",
        };
        let provider_name = self
            .registry
            .get(&session.provider)
            .map_or_else(|| session.provider.to_string(), |p| p.descriptor().name);
        let council = deliberation.is_some_and(|d| !d.seats.is_empty());
        self.sink.audit(AuditEvent::new(
            EventKind::RouteDecided,
            origin.clone(),
            format!(
                "modelo aplicado · {provider_name} / {} · {}",
                chosen.model,
                match (by, council, followed) {
                    ("council", _, _) => "pelo Conselho (Full)",
                    (_, true, _) => "pelo Conselho, com a aprovação do usuário",
                    (_, _, Some(true)) => "pelo usuário, seguindo a recomendação",
                    (_, _, Some(false)) => "pelo usuário, diferente da recomendação",
                    _ => "pelo usuário",
                }
            ),
            json!({
                "deliberationId": deliberation.map(|d| &d.id),
                "sessionId": session.id,
                "provider": chosen.provider,
                "model": chosen.model,
                "by": by,
                "followedRecommendation": followed,
                "recommended": recommended,
                "activity": deliberation.map(|d| d.recommendation.activity),
                "reserves": deliberation.map(|d| d.seats.iter().skip(1).map(|s| &s.member).collect::<Vec<_>>()),
            }),
        ));
    }

    /// Opens a session with the model the user picked (`ROUTE_DECIDED`)
    /// and, if asked, sends the task as its first message.
    pub async fn start_session(
        &self,
        sessions: &SessionManager,
        request: RouteStart,
        project_path: PathBuf,
        origin: CallOrigin,
    ) -> Result<RouteStarted, ProviderError> {
        let task = request
            .task
            .as_deref()
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .map(str::to_owned);
        let title = request
            .title
            .filter(|t| !t.trim().is_empty())
            .or_else(|| task.as_deref().map(title_from_task));
        let deliberation = request
            .deliberation_id
            .as_ref()
            .and_then(|id| self.deliberation(id));
        let session = sessions
            .start(
                StartRequest {
                    provider: Some(request.provider.clone()),
                    title,
                    model: request.model.clone(),
                    instructions: None,
                    // The task is the first message: it gets the project
                    // context like any session (ADR-0013).
                    context: Default::default(),
                    reserves: Vec::new(),
                },
                project_path,
                origin.clone(),
            )
            .await?;
        self.record_route(&session, deliberation.as_ref(), &origin);

        let send = request.send_task.unwrap_or(self.settings.read().send_task);
        let (turn_id, send_error) = match task.filter(|_| send) {
            Some(task) => match sessions.send(&session.id, task, origin).await {
                Ok(turn) => (Some(turn), None),
                Err(err) => (None, Some(err.message)),
            },
            None => (None, None),
        };
        Ok(RouteStarted {
            session,
            turn_id,
            send_error,
            skipped: Vec::new(),
        })
    }

    /// Carries out a deliberation's demand (ADR-0024): opens the session
    /// with the first member in line able to open one, the others as its
    /// reserves, and sends the demand with the Council's plan.
    pub async fn execute(
        &self,
        sessions: &SessionManager,
        id: &DeliberationId,
        origin: CallOrigin,
    ) -> Result<RouteStarted, ProviderError> {
        let deliberation = self.deliberation(id).ok_or_else(|| {
            ProviderError::not_found(
                "deliberação não encontrada (o histórico guarda as 50 últimas)",
            )
        })?;
        if deliberation.seats.is_empty() {
            return Err(ProviderError::invalid(
                "esta deliberação não tem membros do Conselho para executar",
            ));
        }
        let project_path = deliberation.project_path.clone().ok_or_else(|| {
            ProviderError::invalid("esta deliberação não diz o projeto: analise de novo")
        })?;
        let names: Vec<String> = deliberation.seats.iter().map(seat_label).collect();
        let message = execution_message(&deliberation.task, deliberation.plan.as_ref(), &names);
        let title = title_from_task(&deliberation.task);
        let seats = &deliberation.seats;
        let mut skipped = Vec::new();
        for (index, seat) in seats.iter().enumerate() {
            // The ones after it, then the ones that could not open it.
            let reserves: Vec<Reserve> = seats[index + 1..]
                .iter()
                .chain(&seats[..index])
                .map(|s| Reserve {
                    provider: s.member.provider.clone(),
                    model: s.member.model.clone(),
                })
                .collect();
            let opened = sessions
                .start(
                    StartRequest {
                        provider: Some(seat.member.provider.clone()),
                        title: Some(title.clone()),
                        model: seat.member.model.clone(),
                        instructions: None,
                        context: Default::default(),
                        reserves,
                    },
                    project_path.clone(),
                    origin.clone(),
                )
                .await;
            let session = match opened {
                Ok(session) => session,
                Err(err) => {
                    skipped.push(format!("{}: {}", seat_label(seat), err.message));
                    continue;
                }
            };
            self.record_route(&session, Some(&deliberation), &origin);
            let (turn_id, send_error) = match sessions.send(&session.id, message, origin).await {
                Ok(turn) => (Some(turn), None),
                Err(err) => (None, Some(err.message)),
            };
            return Ok(RouteStarted {
                session,
                turn_id,
                send_error,
                skipped,
            });
        }
        Err(ProviderError::unavailable(format!(
            "nenhum membro do Conselho conseguiu abrir a sessão — {}",
            skipped.join(" · ")
        )))
    }

    /// Deliberates and, in Full mode, carries out the demand on its own
    /// (origin `council`).
    pub async fn run(
        &self,
        sessions: &SessionManager,
        request: &DeliberateRequest,
        project_path: PathBuf,
    ) -> RunOutcome {
        let deliberation = self.deliberate(sessions, request, Some(project_path)).await;
        let (started, start_error) = if deliberation.auto_apply {
            let origin = CallOrigin::Council {
                deliberation_id: Some(deliberation.id.clone()),
            };
            match self.execute(sessions, &deliberation.id, origin).await {
                Ok(started) => (Some(started), None),
                Err(err) => (None, Some(err.message)),
            }
        } else {
            (None, None)
        };
        RunOutcome {
            deliberation,
            started,
            start_error,
        }
    }
}

/// Why the first seat carries out the demand, and who backs it up.
fn seats_reason(seats: &[Seat]) -> String {
    let Some(first) = seats.first() else {
        return String::new();
    };
    let mut reason = match &first.demoted {
        None => format!(
            "Executa: {}, o 1º membro disponível na ordem do Conselho.",
            seat_label(first)
        ),
        Some(why) => format!(
            "Executa: {} ({why}), porque nenhum membro está melhor.",
            seat_label(first)
        ),
    };
    for seat in seats.iter().skip(1) {
        if let Some(why) = &seat.demoted {
            reason.push_str(&format!(" {} {why} e fica de reserva.", seat_label(seat)));
        }
    }
    let reserves: Vec<String> = seats.iter().skip(1).map(seat_label).collect();
    if reserves.is_empty() {
        reason.push_str(" Sem reserva: o Conselho tem um membro disponível.");
    } else {
        reason.push_str(&format!(
            " Se falhar, a sessão passa para: {}.",
            reserves.join(", ")
        ));
    }
    reason
}

fn member_label(member: &CouncilMember) -> String {
    format!(
        "{} / {}",
        member.provider,
        member.model.as_deref().unwrap_or("modelo padrão")
    )
}

fn truncate_chars(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        text.to_owned()
    } else {
        let mut out: String = text.chars().take(max).collect();
        out.push('…');
        out
    }
}

/// First line of the task, shortened.
fn title_from_task(task: &str) -> String {
    let line = task.lines().find(|l| !l.trim().is_empty()).unwrap_or(task);
    truncate_chars(line.trim(), TITLE_CHARS)
}

/// Same demand, same project and same members (in the same order) → same
/// key.
fn cache_key(task: &str, project: Option<&Path>, settings: &CouncilSettings) -> u64 {
    let mut hasher = DefaultHasher::new();
    words(task).hash(&mut hasher);
    project.hash(&mut hasher);
    settings.members.hash(&mut hasher);
    hasher.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn titles_come_from_the_first_line_of_the_task() {
        assert_eq!(
            title_from_task("\n  Corrigir o login \nmais detalhes"),
            "Corrigir o login"
        );
        let long = "a".repeat(80);
        assert_eq!(title_from_task(&long).chars().count(), TITLE_CHARS + 1);
    }

    fn seat(name: &str, demoted: Option<&str>) -> Seat {
        Seat {
            member: CouncilMember {
                provider: name.to_lowercase().into(),
                model: None,
            },
            provider_name: name.into(),
            model_name: "padrão".into(),
            demoted: demoted.map(str::to_owned),
        }
    }

    #[test]
    fn the_reason_says_who_executes_and_who_backs_it_up() {
        assert_eq!(
            seats_reason(&[seat("Claude", None), seat("Gemini", None)]),
            "Executa: Claude, o 1º membro disponível na ordem do Conselho. Se falhar, a sessão passa para: Gemini."
        );
        assert_eq!(
            seats_reason(&[
                seat("Gemini", None),
                seat("Claude", Some("falhou na análise: sobrecarregado"))
            ]),
            "Executa: Gemini, o 1º membro disponível na ordem do Conselho. Claude falhou na análise: sobrecarregado e fica de reserva. Se falhar, a sessão passa para: Claude."
        );
        assert_eq!(
            seats_reason(&[seat("Claude", None)]),
            "Executa: Claude, o 1º membro disponível na ordem do Conselho. Sem reserva: o Conselho tem um membro disponível."
        );
    }
}
