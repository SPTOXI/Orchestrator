//! `RouterService`: the router, the Council, its settings, cache and
//! history, and the start of sessions with the chosen model (ADR-0011).

use crate::activity::words;
use crate::cache::DeliberationCache;
use crate::catalog::{catalog, Availability, CatalogModel};
use crate::council::{
    parse_ballot, prompt, tally, Ballot, Decision, DecisionSource, Deliberation, Vote, SYSTEM,
};
use crate::score::{rank, Candidate, ModelRef, Recommendation, RouteRequest};
use crate::settings::{self, CouncilMember, CouncilMode, CouncilSettings};
use crate::store::DeliberationStore;
use chrono::Utc;
use orchestrator_core::{
    AuditEvent, CallOrigin, DeliberationId, EventKind, EventSink, ProviderId, SessionInfo,
    TokenUsage, TurnId,
};
use orchestrator_providers::{
    CompletionRequest, ProviderError, ProviderRegistry, SessionManager, StartRequest,
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
    /// Ignore the cache ("Deliberar de novo").
    pub force: bool,
}

/// Opens a session with a chosen model.
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
}

/// A deliberation and, in Full mode, the session it opened.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunOutcome {
    pub deliberation: Deliberation,
    pub started: Option<RouteStarted>,
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
                "shortlist": settings.shortlist,
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

    /// Picks a model for a task: the router ranks, and the Council (modes
    /// Sugerir and Full) deliberates over the best candidates.
    pub async fn deliberate(&self, request: &DeliberateRequest) -> Deliberation {
        let started = Instant::now();
        let settings = self.settings();
        let recommendation = self.recommend(&request.route).await;
        let k = usize::from(settings.shortlist).min(recommendation.candidates.len());
        let shortlist: Vec<&Candidate> = recommendation.candidates.iter().take(k).collect();
        let mut deliberation = Deliberation {
            id: DeliberationId::new(),
            created_at: Utc::now(),
            task: request.route.task.trim().to_owned(),
            mode: settings.mode,
            shortlist: shortlist.iter().map(|c| c.model_ref.clone()).collect(),
            recommendation: recommendation.clone(),
            votes: Vec::new(),
            decision: None,
            usage: TokenUsage::default(),
            cached: false,
            cached_from: None,
            saved_usage: None,
            notices: Vec::new(),
            auto_apply: false,
            duration_ms: 0,
        };

        if shortlist.is_empty() {
            deliberation
                .notices
                .push(if recommendation.excluded.is_empty() {
                    "Nenhum modelo cadastrado: adicione uma API em AI PROVIDERS.".into()
                } else {
                    "Nenhum modelo cadastrado atende aos requisitos (veja os excluídos).".into()
                });
            return self.finish(deliberation, started, false, None);
        }
        let router_decision = |reason: String| Decision {
            model_ref: shortlist[0].model_ref.clone(),
            provider_name: shortlist[0].provider_name.clone(),
            model_name: shortlist[0].model_name.clone(),
            source: DecisionSource::Router,
            reason,
            agreement: None,
        };
        if settings.mode == CouncilMode::Off {
            deliberation.decision = Some(router_decision(format!(
                "Maior nota do roteador ({}).",
                shortlist[0].score
            )));
            return self.finish(deliberation, started, false, None);
        }
        if shortlist.len() == 1 {
            deliberation.decision = Some(router_decision(
                "Único modelo que atende aos requisitos.".into(),
            ));
            deliberation.notices.push(
                "Só um modelo atende aos requisitos: o Conselho não precisou ser consultado."
                    .into(),
            );
            deliberation.auto_apply = settings.mode == CouncilMode::Full;
            return self.finish(deliberation, started, false, None);
        }

        let ttl = Duration::from_secs(u64::from(settings.cache_minutes) * 60);
        let key = cache_key(&request.route.task, &recommendation, &shortlist, &settings);
        if settings.cache_minutes > 0 && !request.force {
            let hit = self.cache.get(key, ttl).or_else(|| {
                let store = self.store.as_ref()?;
                store.cached(&format!("{key:016x}"), Utc::now())
            });
            if let Some(hit) = hit {
                deliberation.votes = hit.votes;
                deliberation.decision = hit.decision;
                deliberation.cached = true;
                deliberation.cached_from = Some(hit.id);
                deliberation.saved_usage = Some(hit.usage);
                deliberation.notices = hit.notices;
                deliberation.notices.push(format!(
                    "Resposta do cache do Conselho (validade de {} min): nenhum token gasto.",
                    settings.cache_minutes
                ));
                deliberation.auto_apply = settings.mode == CouncilMode::Full;
                return self.finish(deliberation, started, true, None);
            }
        }
        let mut cache_for = None;

        let question = prompt(&request.route.task, &recommendation, &shortlist);
        let (votes, ballots) = self.ask_members(&settings, &question, &shortlist).await;
        deliberation.votes = votes;
        for vote in &deliberation.votes {
            deliberation.usage += vote.usage;
        }
        match tally(&ballots, shortlist.len()) {
            Some(result) => {
                let winner = shortlist[result.winner];
                let valid = ballots.len();
                let supporters: Vec<&Vote> = deliberation
                    .votes
                    .iter()
                    .filter(|v| v.choice.as_ref() == Some(&winner.model_ref))
                    .collect();
                let first_reason = supporters
                    .iter()
                    .find_map(|v| v.reason.clone())
                    .or_else(|| deliberation.votes.iter().find_map(|v| v.reason.clone()));
                let reason = if valid == 1 {
                    first_reason.unwrap_or_else(|| "Escolha do gerenciador.".into())
                } else {
                    let head = format!(
                        "{} de {} membros escolheram este modelo.",
                        supporters.len(),
                        valid
                    );
                    match first_reason {
                        Some(reason) => format!("{head} {reason}"),
                        None => head,
                    }
                };
                deliberation.decision = Some(Decision {
                    model_ref: winner.model_ref.clone(),
                    provider_name: winner.provider_name.clone(),
                    model_name: winner.model_name.clone(),
                    source: DecisionSource::Council,
                    reason,
                    agreement: Some(result.agreement),
                });
                deliberation.auto_apply = settings.mode == CouncilMode::Full;
                if settings.cache_minutes > 0 {
                    self.cache.put(key, deliberation.clone());
                    cache_for = Some((key, ttl));
                }
            }
            None => {
                deliberation.decision = Some(router_decision(format!(
                    "Maior nota do roteador ({}).",
                    shortlist[0].score
                )));
                deliberation.notices.push(
                    "Nenhum membro do Conselho respondeu de forma válida: valendo a recomendação do roteador. O modo Full não aplica sozinho neste caso."
                        .into(),
                );
            }
        }
        self.finish(deliberation, started, true, cache_for)
    }

    /// Asks every member in parallel. Returns the votes (member order) and
    /// the ballots that count.
    async fn ask_members(
        &self,
        settings: &CouncilSettings,
        question: &str,
        shortlist: &[&Candidate],
    ) -> (Vec<Vote>, Vec<Ballot>) {
        let timeout = Duration::from_secs(u64::from(settings.timeout_secs));
        let refs: Vec<ModelRef> = shortlist.iter().map(|c| c.model_ref.clone()).collect();
        let mut jobs = JoinSet::new();
        for (index, member) in settings.members.iter().cloned().enumerate() {
            let registry = self.registry.clone();
            let known = self.availability.get(&member.provider);
            let question = question.to_owned();
            let count = refs.len();
            jobs.spawn(async move {
                let started = Instant::now();
                let mut vote = Vote {
                    provider_name: registry
                        .get(&member.provider)
                        .map_or_else(|| member.provider.to_string(), |p| p.descriptor().name),
                    member: member.clone(),
                    model: member.model.clone(),
                    choice: None,
                    ranking: Vec::new(),
                    confidence: None,
                    reason: None,
                    error: None,
                    usage: TokenUsage::default(),
                    duration_ms: 0,
                };
                let ballot = match registry.get(&member.provider) {
                    None => Err("provider não registrado (conexão removida ou desativada)".into()),
                    Some(provider) if !provider.capabilities().completion => {
                        Err("este provider não responde pedidos avulsos".into())
                    }
                    Some(_) if known.as_ref().is_some_and(|s| !s.available) => Err(format!(
                        "provider indisponível: {}",
                        known.and_then(|s| s.detail).unwrap_or_default()
                    )),
                    Some(provider) => {
                        let request = CompletionRequest {
                            model: member.model.clone(),
                            system: Some(SYSTEM.into()),
                            prompt: question,
                        };
                        let cancel = CancellationToken::new();
                        match tokio::time::timeout(timeout, provider.complete(&request, &cancel))
                            .await
                        {
                            Ok(Ok(answer)) => {
                                vote.usage = answer.usage;
                                if answer.model.is_some() {
                                    vote.model = answer.model;
                                }
                                parse_ballot(&answer.text, count)
                            }
                            Ok(Err(err)) => Err(err.message),
                            Err(_) => {
                                cancel.cancel();
                                Err(format!("sem resposta em {} s", timeout.as_secs()))
                            }
                        }
                    }
                };
                vote.duration_ms = started.elapsed().as_millis() as u64;
                (index, vote, ballot)
            });
        }
        let mut answers = Vec::new();
        while let Some(joined) = jobs.join_next().await {
            match joined {
                Ok(answer) => answers.push(answer),
                Err(err) => eprintln!("[orchestrator] council member task failed: {err}"),
            }
        }
        answers.sort_by_key(|(index, _, _)| *index);
        let mut votes = Vec::new();
        let mut ballots = Vec::new();
        for (_, mut vote, ballot) in answers {
            match ballot {
                Ok(ballot) => {
                    vote.choice = Some(refs[ballot.choice].clone());
                    vote.ranking = ballot.ranking.iter().map(|i| refs[*i].clone()).collect();
                    vote.confidence = ballot.confidence;
                    vote.reason = ballot.reason.clone();
                    ballots.push(ballot);
                }
                Err(error) => vote.error = Some(error),
            }
            votes.push(vote);
        }
        (votes, ballots)
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
        let valid = d.votes.iter().filter(|v| v.error.is_none()).count();
        let summary = match &d.decision {
            Some(decision) => format!(
                "Conselho{}: {} ({}) · {}/{} membros válidos{}",
                if d.cached { " (cache)" } else { "" },
                decision.model_name,
                decision.provider_name,
                valid,
                d.votes.len(),
                d.usage
                    .cost_usd
                    .map(|c| format!(" · US$ {c:.4}"))
                    .unwrap_or_default()
            ),
            None => "Conselho sem decisão".into(),
        };
        let candidates: Vec<_> = d
            .shortlist
            .iter()
            .filter_map(|r| d.recommendation.find(r))
            .map(|c| json!({"provider": c.model_ref.provider, "model": c.model_ref.model, "score": c.score}))
            .collect();
        let votes: Vec<_> = d
            .votes
            .iter()
            .map(|v| {
                json!({
                    "provider": v.member.provider,
                    "model": v.model,
                    "choice": v.choice,
                    "confidence": v.confidence,
                    "reason": v.reason,
                    "error": v.error,
                    "usage": v.usage,
                    "durationMs": v.duration_ms,
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
                "activity": d.recommendation.activity,
                "preference": d.recommendation.preference,
                "needsTools": d.recommendation.needs_tools,
                "mode": d.mode,
                "candidates": candidates,
                "votes": votes,
                "decision": d.decision.as_ref().map(|x| json!({
                    "provider": x.model_ref.provider,
                    "model": x.model_ref.model,
                    "source": x.source,
                    "agreement": x.agreement,
                })),
                "usage": d.usage,
                "cached": d.cached,
                "cachedFrom": d.cached_from,
                "autoApply": d.auto_apply,
                "durationMs": d.duration_ms,
            }),
        ));
    }

    /// Opens a session with the chosen model (`ROUTE_DECIDED`) and, if asked,
    /// sends the task as its first message.
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
                },
                project_path,
                origin.clone(),
            )
            .await?;

        let chosen = ModelRef {
            provider: session.provider.clone(),
            model: session.model.clone().unwrap_or_default(),
        };
        let recommended = deliberation
            .as_ref()
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
        self.sink.audit(AuditEvent::new(
            EventKind::RouteDecided,
            origin.clone(),
            format!(
                "modelo aplicado · {provider_name} / {} · {}",
                chosen.model,
                match (by, followed) {
                    ("council", _) => "pelo Conselho (Full)",
                    (_, Some(true)) => "pelo usuário, seguindo a recomendação",
                    (_, Some(false)) => "pelo usuário, diferente da recomendação",
                    _ => "pelo usuário",
                }
            ),
            json!({
                "deliberationId": request.deliberation_id,
                "sessionId": session.id,
                "provider": chosen.provider,
                "model": chosen.model,
                "by": by,
                "followedRecommendation": followed,
                "recommended": recommended,
                "activity": deliberation.as_ref().map(|d| d.recommendation.activity),
            }),
        ));

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
        })
    }

    /// Deliberates and, in Full mode with a Council decision, opens the
    /// session on its own (origin `council`).
    pub async fn run(
        &self,
        sessions: &SessionManager,
        request: &DeliberateRequest,
        project_path: PathBuf,
    ) -> Result<RunOutcome, ProviderError> {
        let deliberation = self.deliberate(request).await;
        let started = match (&deliberation.decision, deliberation.auto_apply) {
            (Some(decision), true) => Some(
                self.start_session(
                    sessions,
                    RouteStart {
                        deliberation_id: Some(deliberation.id.clone()),
                        provider: decision.model_ref.provider.clone(),
                        model: Some(decision.model_ref.model.clone()),
                        title: None,
                        task: Some(deliberation.task.clone()),
                        send_task: None,
                    },
                    project_path,
                    CallOrigin::Council {
                        deliberation_id: Some(deliberation.id.clone()),
                    },
                )
                .await?,
            ),
            _ => None,
        };
        Ok(RunOutcome {
            deliberation,
            started,
        })
    }
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

/// Same question, same candidates (data included) and same members → same
/// key.
fn cache_key(
    task: &str,
    recommendation: &Recommendation,
    shortlist: &[&Candidate],
    settings: &CouncilSettings,
) -> u64 {
    let mut hasher = DefaultHasher::new();
    words(task).hash(&mut hasher);
    recommendation.activity.hash(&mut hasher);
    recommendation.preference.hash(&mut hasher);
    recommendation.needs_tools.hash(&mut hasher);
    recommendation.min_context.hash(&mut hasher);
    for candidate in shortlist {
        candidate.model_ref.hash(&mut hasher);
        candidate.input_price.map(f64::to_bits).hash(&mut hasher);
        candidate.output_price.map(f64::to_bits).hash(&mut hasher);
        candidate.context_window.hash(&mut hasher);
        candidate.supports_tools.hash(&mut hasher);
        candidate.tags.hash(&mut hasher);
    }
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
}
