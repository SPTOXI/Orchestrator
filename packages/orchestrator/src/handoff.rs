//! Handoff between AIs (ADR-0013): prepare a packet from a session (facts
//! from the history, narrative from its AI), save it, and have another AI
//! take over in a new session that receives the packet, never the
//! conversation.

use crate::builder::ContextBuilder;
use crate::packet::{self, normalize, AGENT_PROMPT};
use crate::text::clip;
use chrono::Utc;
use orchestrator_core::{
    AuditEvent, CallOrigin, EventKind, EventSink, Handoff, HandoffEnd, HandoffId, HandoffPacket,
    HandoffStatus, NoticeLevel, ProviderId, SessionEvent, SessionId, SessionInfo, SessionStatus,
    TokenUsage, TurnId, TurnStatus,
};
use orchestrator_memory::MemoryStore;
use orchestrator_providers::{
    ContextOptions, ProviderError, ProviderErrorKind, SessionManager, StartRequest,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::path::{Path, PathBuf};
use std::sync::Arc;

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PrepareRequest {
    pub session_id: SessionId,
    /// Ask the session's AI for the narrative (one turn).
    #[serde(default = "default_true")]
    pub ask_agent: bool,
}

/// A packet to review before saving.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HandoffDraft {
    pub from: HandoffEnd,
    pub project_path: String,
    pub packet: HandoffPacket,
    /// True when the source AI wrote the narrative.
    pub by_agent: bool,
    /// Why the AI's narrative is missing, etc.
    pub notes: Vec<String>,
    /// What asking the AI cost.
    pub usage: Option<TokenUsage>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateRequest {
    pub session_id: SessionId,
    pub packet: HandoffPacket,
    #[serde(default)]
    pub by_agent: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StartHandoff {
    pub handoff_id: HandoffId,
    pub provider: ProviderId,
    pub model: Option<String>,
    pub title: Option<String>,
    /// Context budget of the new session; `None` = the setting.
    pub budget: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StartedHandoff {
    pub handoff: Handoff,
    pub session: SessionInfo,
    /// Turn that carries the first message, when it could be sent.
    pub turn_id: Option<TurnId>,
    pub send_error: Option<String>,
}

fn end_of(info: &SessionInfo) -> HandoffEnd {
    HandoffEnd {
        session_id: info.id.clone(),
        provider: info.provider.clone(),
        model: info.model.clone(),
        title: info.title.clone(),
    }
}

fn internal(message: String) -> ProviderError {
    ProviderError::internal(message)
}

pub struct HandoffService {
    sessions: SessionManager,
    store: Arc<MemoryStore>,
    builder: Arc<ContextBuilder>,
    sink: Arc<dyn EventSink>,
}

impl HandoffService {
    pub fn new(
        sessions: SessionManager,
        store: Arc<MemoryStore>,
        builder: Arc<ContextBuilder>,
        sink: Arc<dyn EventSink>,
    ) -> Self {
        Self {
            sessions,
            store,
            builder,
            sink,
        }
    }

    pub fn builder(&self) -> &Arc<ContextBuilder> {
        &self.builder
    }

    /// Handoffs of a project (all with `None`), newest first.
    pub fn list(&self, project_id: Option<&str>) -> Vec<Handoff> {
        self.store.handoffs_list(project_id, 50)
    }

    pub fn get(&self, id: &HandoffId) -> Option<Handoff> {
        self.store.handoff(id.as_str())
    }

    /// A draft from the session: facts from the history and, when asked and
    /// possible, the narrative from its AI. Nothing is saved.
    pub async fn prepare(&self, request: PrepareRequest) -> Result<HandoffDraft, ProviderError> {
        let info = self.sessions.info(&request.session_id)?;
        let root = info.project_path.clone();
        let mut notes = Vec::new();
        let facts = match self.store.session_facts(info.id.as_str()) {
            Ok(facts) => Some(facts),
            Err(err) => {
                notes.push(format!("histórico da sessão indisponível: {err}"));
                None
            }
        };
        let decisions = facts
            .as_ref()
            .and_then(|f| f.project_id.as_deref())
            .and_then(|project| self.store.decisions_list(project).ok())
            .unwrap_or_default();
        let mut from_history = match &facts {
            Some(facts) => packet::from_facts(facts, &root, &decisions),
            None => HandoffPacket {
                goal: info.title.clone(),
                ..Default::default()
            },
        };
        // A session that took over a handoff carries on the same work: its
        // first message is the takeover instruction, not the goal.
        let taken_over = self
            .sessions
            .context_options(&info.id)
            .ok()
            .and_then(|options| options.handoff_id)
            .and_then(|id| self.store.handoff(id.as_str()));
        if let Some(previous) = taken_over {
            from_history.goal = previous.packet.goal;
        }

        let mut by_agent = false;
        let mut usage = None;
        let packet = if request.ask_agent {
            match self.ask_agent(&info).await {
                Ok((answer, spent)) => {
                    by_agent = true;
                    usage = Some(spent);
                    packet::merge(from_history, answer)
                }
                Err((message, spent)) => {
                    usage = spent;
                    notes.push(message);
                    from_history
                }
            }
        } else {
            from_history
        };
        let info = self.sessions.info(&info.id)?;
        Ok(HandoffDraft {
            from: end_of(&info),
            project_path: root.to_string_lossy().into_owned(),
            packet,
            by_agent,
            notes,
            usage,
        })
    }

    /// One turn in the source session asking for the narrative as JSON.
    async fn ask_agent(
        &self,
        info: &SessionInfo,
    ) -> Result<(HandoffPacket, TokenUsage), (String, Option<TokenUsage>)> {
        match info.status {
            SessionStatus::Running => {
                return Err((
                    "a sessão está com um turno em andamento; espere ou cancele para pedir o \
                     resumo à IA"
                        .into(),
                    None,
                ))
            }
            SessionStatus::Closed => {
                self.sessions
                    .resume(&info.id, CallOrigin::System)
                    .await
                    .map_err(|e| {
                        (
                            format!(
                                "a sessão não pôde ser retomada para a IA escrever o resumo \
                                 ({}); complete os campos",
                                e.message
                            ),
                            None,
                        )
                    })?;
            }
            SessionStatus::Idle => {}
        }
        let result = self
            .sessions
            .execute(&info.id, AGENT_PROMPT.into(), CallOrigin::System)
            .await
            .map_err(|e| {
                (
                    format!(
                        "a IA da sessão não respondeu ({}); complete os campos",
                        e.message
                    ),
                    None,
                )
            })?;
        if result.status != TurnStatus::Completed {
            return Err((
                format!(
                    "a IA da sessão não respondeu ({}); complete os campos",
                    result.error.as_deref().unwrap_or("turno interrompido")
                ),
                Some(result.usage),
            ));
        }
        packet::parse_agent(&result.text)
            .map(|packet| (packet, result.usage))
            .map_err(|e| {
                (
                    format!("a resposta da IA não pôde ser usada: {e}; complete os campos"),
                    Some(result.usage),
                )
            })
    }

    /// Saves a reviewed packet (`HANDOFF_CREATED`).
    pub fn create(
        &self,
        request: CreateRequest,
        origin: CallOrigin,
    ) -> Result<Handoff, ProviderError> {
        let info = self.sessions.info(&request.session_id)?;
        let packet = normalize(request.packet);
        if packet.goal.is_empty() {
            return Err(ProviderError::invalid("o objetivo (goal) é obrigatório"));
        }
        let project_path = info.project_path.to_string_lossy().into_owned();
        let project_id = self
            .store
            .session_project_id(info.id.as_str())
            .or_else(|| self.store.project_by_path(&project_path).map(|p| p.id));
        let handoff = Handoff {
            id: HandoffId::new(),
            project_id,
            project_path,
            from: end_of(&info),
            to: None,
            packet,
            status: HandoffStatus::Created,
            by_agent: request.by_agent,
            created_at: Utc::now(),
            accepted_at: None,
        };
        self.store.handoff_save(&handoff).map_err(internal)?;
        let _ = self.sessions.annotate(
            &info.id,
            SessionEvent::Notice {
                turn_id: None,
                level: NoticeLevel::Info,
                message: format!("Handoff criado: {}", clip(&handoff.packet.goal, 120)),
            },
        );
        let p = &handoff.packet;
        self.sink.audit(AuditEvent::new(
            EventKind::HandoffCreated,
            origin,
            format!("handoff created · {}", clip(&p.goal, 120)),
            json!({
                "handoffId": handoff.id,
                "projectId": handoff.project_id,
                "projectPath": handoff.project_path,
                "fromSession": info.id,
                "provider": info.provider,
                "model": info.model,
                "goal": p.goal,
                "nextAction": p.next_action,
                "byAgent": handoff.by_agent,
                "completed": p.completed.len(),
                "remaining": p.remaining.len(),
                "files": p.files.len(),
                "errors": p.errors.len(),
            }),
        ));
        Ok(handoff)
    }

    /// Another AI takes over: a new session with the handoff in its context
    /// and a first message pointing at the next action (`HANDOFF_ACCEPTED`).
    pub async fn start(
        &self,
        request: StartHandoff,
        origin: CallOrigin,
    ) -> Result<StartedHandoff, ProviderError> {
        let handoff = self
            .store
            .handoff(request.handoff_id.as_str())
            .ok_or_else(|| {
                ProviderError::new(
                    ProviderErrorKind::NotFound,
                    format!("handoff {} não encontrado", request.handoff_id),
                )
            })?;
        if handoff.status == HandoffStatus::Accepted {
            return Err(ProviderError::invalid("este handoff já foi assumido"));
        }
        let title = request
            .title
            .filter(|t| !t.trim().is_empty())
            .unwrap_or_else(|| format!("Handoff · {}", clip(&handoff.packet.goal, 50)));
        let session = self
            .sessions
            .start(
                StartRequest {
                    provider: Some(request.provider),
                    title: Some(title),
                    model: request.model,
                    instructions: None,
                    context: ContextOptions {
                        enabled: Some(true),
                        budget: request.budget,
                        handoff_id: Some(handoff.id.clone()),
                    },
                    reserves: Vec::new(),
                },
                PathBuf::from(&handoff.project_path),
                origin.clone(),
            )
            .await?;
        let accepted =
            match self
                .store
                .handoff_accept(handoff.id.as_str(), end_of(&session), Utc::now())
            {
                Ok(accepted) => accepted,
                Err(err) => {
                    let _ = self.sessions.close(&session.id, CallOrigin::System).await;
                    return Err(ProviderError::invalid(err));
                }
            };
        let event = SessionEvent::HandedOff {
            handoff_id: handoff.id.clone(),
            from_session: handoff.from.session_id.clone(),
            to_session: session.id.clone(),
            provider: session.provider.clone(),
        };
        // The source may be gone (another installation's database).
        let _ = self
            .sessions
            .annotate(&handoff.from.session_id, event.clone());
        let _ = self.sessions.annotate(&session.id, event);
        self.sink.audit(AuditEvent::new(
            EventKind::HandoffAccepted,
            origin.clone(),
            format!(
                "handoff accepted · {} → {}",
                clip(&handoff.packet.goal, 100),
                session.provider
            ),
            json!({
                "handoffId": handoff.id,
                "projectId": handoff.project_id,
                "projectPath": handoff.project_path,
                "fromSession": handoff.from.session_id,
                "fromProvider": handoff.from.provider,
                "toSession": session.id,
                "provider": session.provider,
                "model": session.model,
            }),
        ));
        let (turn_id, send_error) = match self
            .sessions
            .send(&session.id, packet::first_message(&accepted), origin)
            .await
        {
            Ok(turn) => (Some(turn), None),
            Err(err) => (None, Some(err.message)),
        };
        Ok(StartedHandoff {
            handoff: accepted,
            session: self.sessions.info(&session.id)?,
            turn_id,
            send_error,
        })
    }
}

/// What `context_preview` receives.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PreviewRequest {
    /// Default: the open project.
    pub project_path: Option<String>,
    pub task: Option<String>,
    pub handoff_id: Option<HandoffId>,
    pub budget: Option<u32>,
    pub session_id: Option<SessionId>,
}

impl ContextBuilder {
    /// What a session would receive (nothing is sent or recorded).
    pub fn preview(
        &self,
        request: PreviewRequest,
        open_project: &Path,
    ) -> Result<crate::builder::ContextPack, String> {
        let handoff = match &request.handoff_id {
            Some(id) => Some(
                self.store()
                    .handoff(id.as_str())
                    .ok_or_else(|| format!("handoff {id} não encontrado"))?,
            ),
            None => None,
        };
        let project_path = request
            .project_path
            .map(PathBuf::from)
            .or_else(|| handoff.as_ref().map(|h| PathBuf::from(&h.project_path)))
            .unwrap_or_else(|| open_project.to_path_buf());
        Ok(self.build(&crate::builder::BuildRequest {
            project_path,
            task: request.task.unwrap_or_default(),
            handoff,
            budget: request.budget,
            session_id: request.session_id.map(|id| id.to_string()),
            no_tools: false,
        }))
    }
}
