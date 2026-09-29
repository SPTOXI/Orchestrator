//! Provider Sessions: sessions owned by the Orchestrator (ADR-0009).
//!
//! The manager opens native sessions through the provider, runs one turn at
//! a time per session, keeps the transcript, sums token usage, publishes
//! live events and records the history (`SESSION_*`, `TURN_COMPLETED`).

use crate::context::{ToolExecutor, TurnContext, TurnObserver};
use crate::error::{ProviderError, ProviderErrorKind};
use crate::log::SessionLog;
use crate::provider::{AIProvider, NativeSession, SessionSpec, TurnInput, TurnOutput};
use crate::registry::ProviderRegistry;
use chrono::Utc;
use orchestrator_core::{
    AuditEvent, CallOrigin, EventKind, EventSink, NoticeLevel, ProviderId, SessionEvent, SessionId,
    SessionInfo, SessionLogEntry, SessionStatus, StreamEvent, TokenUsage, TurnId, TurnStatus,
};
use parking_lot::{Mutex, RwLock};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

/// Tunables of the session manager.
#[derive(Debug, Clone)]
pub struct ManagerConfig {
    /// After a cancel, how long a provider has to stop before its turn is
    /// abandoned.
    pub cancel_grace: Duration,
    /// Transcript entries kept per session.
    pub log_capacity: usize,
}

impl Default for ManagerConfig {
    fn default() -> Self {
        Self {
            cancel_grace: Duration::from_secs(5),
            log_capacity: 5_000,
        }
    }
}

/// Opens a session (or a subagent session).
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StartRequest {
    /// Provider; `None` = the active one (or the parent's, for a subagent).
    pub provider: Option<ProviderId>,
    pub title: Option<String>,
    pub model: Option<String>,
    pub instructions: Option<String>,
}

/// A session with its transcript.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionSnapshot {
    pub info: SessionInfo,
    pub entries: Vec<SessionLogEntry>,
    /// Sequence number of the newest event included; live events with a
    /// higher `seq` come after this snapshot.
    pub last_seq: u64,
    /// True when old entries were dropped.
    pub truncated: bool,
}

/// Outcome of a turn run with [`SessionManager::execute`].
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnResult {
    pub turn_id: TurnId,
    pub status: TurnStatus,
    /// Assistant text produced (partial when cancelled or failed).
    pub text: String,
    pub error: Option<String>,
    pub usage: TokenUsage,
    pub duration_ms: u64,
}

#[derive(Clone, Copy)]
enum Mode {
    Stream,
    Execute,
}

struct Session {
    id: SessionId,
    /// Instance that served the last turn (to cancel it). Each turn takes the
    /// provider registered now under `info.provider` (see `Inner::provider`).
    provider: Mutex<Arc<dyn AIProvider>>,
    sink: Arc<dyn EventSink>,
    state: Mutex<SessionState>,
}

struct SessionState {
    info: SessionInfo,
    spec: SessionSpec,
    native: NativeSession,
    log: SessionLog,
    running: Option<RunningTurn>,
    children: u32,
}

struct RunningTurn {
    turn_id: TurnId,
    cancel: CancellationToken,
    usage: TokenUsage,
    text: String,
    done: watch::Receiver<bool>,
}

/// Everything a turn task needs, taken while the session is locked.
struct TurnStart {
    provider: Arc<dyn AIProvider>,
    turn_id: TurnId,
    cancel: CancellationToken,
    done: watch::Sender<bool>,
    native: NativeSession,
    project_path: PathBuf,
}

impl Session {
    /// Appends to the transcript and publishes the live event. The caller
    /// holds the state lock, so `seq` order and publish order match.
    fn record(&self, state: &mut SessionState, event: SessionEvent) {
        if let Some(running) = state.running.as_mut() {
            match &event {
                SessionEvent::Usage { turn_id, usage } if *turn_id == running.turn_id => {
                    running.usage += *usage;
                }
                SessionEvent::TextDelta { turn_id, text } if *turn_id == running.turn_id => {
                    running.text.push_str(text);
                }
                _ => {}
            }
        }
        state.info.updated_at = Utc::now();
        let seq = state.log.push(event.clone());
        self.sink.stream(StreamEvent::Session {
            session_id: self.id.clone(),
            seq,
            event,
        });
    }

    fn set_status(&self, state: &mut SessionState, status: SessionStatus) {
        state.info.status = status;
        self.record(state, SessionEvent::StatusChanged { status });
    }
}

/// Feeds provider events of a turn into the session.
struct Recorder(Arc<Session>);

impl TurnObserver for Recorder {
    fn event(&self, event: SessionEvent) {
        let mut state = self.0.state.lock();
        self.0.record(&mut state, event);
    }
}

struct Inner {
    registry: Arc<ProviderRegistry>,
    tools: Arc<dyn ToolExecutor>,
    sink: Arc<dyn EventSink>,
    config: ManagerConfig,
    /// Creation order.
    sessions: RwLock<Vec<Arc<Session>>>,
}

impl Inner {
    /// The session's provider as registered now: an edited connection
    /// (replaced in the registry) applies to open sessions from their next
    /// turn; a removed or disabled one fails the turn with a clear error.
    fn provider(&self, session: &Session) -> Result<Arc<dyn AIProvider>, ProviderError> {
        let id = session.state.lock().info.provider.clone();
        self.registry.get(&id).ok_or_else(|| {
            ProviderError::unavailable(format!(
                "provider {id} is no longer registered (its connection was removed, renamed \
                 or disabled); start a new session"
            ))
        })
    }
}

/// Owns every provider session. Cheap to clone (shared state).
#[derive(Clone)]
pub struct SessionManager {
    inner: Arc<Inner>,
}

impl SessionManager {
    pub fn new(
        registry: Arc<ProviderRegistry>,
        tools: Arc<dyn ToolExecutor>,
        sink: Arc<dyn EventSink>,
    ) -> Self {
        Self::with_config(registry, tools, sink, ManagerConfig::default())
    }

    pub fn with_config(
        registry: Arc<ProviderRegistry>,
        tools: Arc<dyn ToolExecutor>,
        sink: Arc<dyn EventSink>,
        config: ManagerConfig,
    ) -> Self {
        Self {
            inner: Arc::new(Inner {
                registry,
                tools,
                sink,
                config,
                sessions: RwLock::new(Vec::new()),
            }),
        }
    }

    pub fn registry(&self) -> &Arc<ProviderRegistry> {
        &self.inner.registry
    }

    fn session(&self, id: &SessionId) -> Result<Arc<Session>, ProviderError> {
        self.inner
            .sessions
            .read()
            .iter()
            .find(|s| &s.id == id)
            .cloned()
            .ok_or_else(|| ProviderError::not_found(format!("session {id} not found")))
    }

    /// Sessions, newest first.
    pub fn list(&self) -> Vec<SessionInfo> {
        let sessions = self.inner.sessions.read();
        sessions
            .iter()
            .rev()
            .map(|s| s.state.lock().info.clone())
            .collect()
    }

    pub fn info(&self, id: &SessionId) -> Result<SessionInfo, ProviderError> {
        Ok(self.session(id)?.state.lock().info.clone())
    }

    pub fn snapshot(&self, id: &SessionId) -> Result<SessionSnapshot, ProviderError> {
        let session = self.session(id)?;
        let state = session.state.lock();
        Ok(SessionSnapshot {
            info: state.info.clone(),
            entries: state.log.entries(),
            last_seq: state.log.last_seq(),
            truncated: state.log.truncated(),
        })
    }

    /// Opens a session with `request.provider` (default: the active one) on
    /// `project_path`.
    pub async fn start(
        &self,
        request: StartRequest,
        project_path: PathBuf,
        origin: CallOrigin,
    ) -> Result<SessionInfo, ProviderError> {
        let provider = match &request.provider {
            Some(id) => self.inner.registry.require(id)?,
            None => self
                .inner
                .registry
                .active()
                .ok_or_else(|| ProviderError::not_found("no AI provider is registered"))?,
        };
        self.open(provider, request, project_path, None, origin)
            .await
    }

    /// Opens a subagent session of `parent_id` (`spawnAgent`). Same provider
    /// as the parent unless `request.provider` names another one.
    pub async fn spawn(
        &self,
        parent_id: &SessionId,
        request: StartRequest,
        origin: CallOrigin,
    ) -> Result<SessionInfo, ProviderError> {
        let parent = self.session(parent_id)?;
        let provider = match &request.provider {
            Some(id) => self.inner.registry.require(id)?,
            None => self.inner.provider(&parent)?,
        };
        let project_path = parent.state.lock().spec.project_path.clone();
        self.open(provider, request, project_path, Some(parent), origin)
            .await
    }

    async fn open(
        &self,
        provider: Arc<dyn AIProvider>,
        request: StartRequest,
        project_path: PathBuf,
        parent: Option<Arc<Session>>,
        origin: CallOrigin,
    ) -> Result<SessionInfo, ProviderError> {
        let descriptor = provider.descriptor();
        let title = match request.title.map(|t| t.trim().to_owned()) {
            Some(title) if !title.is_empty() => title,
            _ => self.default_title(&descriptor.name, &descriptor.id, parent.as_deref()),
        };
        let id = SessionId::new();
        let spec = SessionSpec {
            session_id: id.clone(),
            project_path: project_path.clone(),
            title: title.clone(),
            model: request.model.filter(|m| !m.trim().is_empty()),
            instructions: request.instructions,
        };

        let native = match &parent {
            Some(parent) if parent.state.lock().info.provider == descriptor.id => {
                let parent_native = parent.state.lock().native.clone();
                provider.spawn_agent(&parent_native, &spec).await?
            }
            _ => provider.start(&spec).await?,
        };

        let now = Utc::now();
        let info = SessionInfo {
            id: id.clone(),
            provider: descriptor.id.clone(),
            title: title.clone(),
            model: native.model.clone().or_else(|| spec.model.clone()),
            project_path: project_path.clone(),
            parent_id: parent.as_ref().map(|p| p.id.clone()),
            status: SessionStatus::Idle,
            native_ref: Some(native.reference.clone()),
            created_at: now,
            updated_at: now,
            turns: 0,
            usage: TokenUsage::default(),
            last_error: None,
        };
        let session = Arc::new(Session {
            id: id.clone(),
            provider: Mutex::new(provider),
            sink: self.inner.sink.clone(),
            state: Mutex::new(SessionState {
                info: info.clone(),
                spec,
                native,
                log: SessionLog::new(self.inner.config.log_capacity),
                running: None,
                children: 0,
            }),
        });
        self.inner.sessions.write().push(session);

        if let Some(parent) = &parent {
            let mut state = parent.state.lock();
            state.children += 1;
            parent.record(
                &mut state,
                SessionEvent::SubagentSpawned {
                    child_id: id.clone(),
                    provider: descriptor.id.clone(),
                    title: title.clone(),
                },
            );
        }

        self.inner.sink.audit(AuditEvent::new(
            EventKind::SessionStarted,
            origin,
            match &parent {
                Some(_) => format!("subagent session started · {} · {title}", descriptor.name),
                None => format!("session started · {} · {title}", descriptor.name),
            },
            json!({
                "sessionId": id,
                "provider": descriptor.id,
                "model": info.model,
                "title": title,
                "projectPath": project_path,
                "parentSessionId": info.parent_id,
                "nativeRef": info.native_ref,
            }),
        ));
        Ok(info)
    }

    fn default_title(&self, name: &str, provider: &ProviderId, parent: Option<&Session>) -> String {
        match parent {
            Some(parent) => {
                let state = parent.state.lock();
                format!("{} › sub {}", state.info.title, state.children + 1)
            }
            None => {
                let count = self
                    .inner
                    .sessions
                    .read()
                    .iter()
                    .filter(|s| {
                        let state = s.state.lock();
                        &state.info.provider == provider && state.info.parent_id.is_none()
                    })
                    .count();
                format!("{name} #{}", count + 1)
            }
        }
    }

    /// Starts a streaming turn in the background and returns its id. Must be
    /// called inside a Tokio runtime.
    pub async fn send(
        &self,
        id: &SessionId,
        input: String,
        origin: CallOrigin,
    ) -> Result<TurnId, ProviderError> {
        let (session, turn) = self.begin(id, &input)?;
        let turn_id = turn.turn_id.clone();
        tokio::spawn(run_turn(
            self.inner.clone(),
            session,
            turn,
            input,
            Mode::Stream,
            origin,
        ));
        Ok(turn_id)
    }

    /// Runs a turn with the provider's `execute` (no incremental output) and
    /// waits for it. The turn keeps running if this future is dropped.
    pub async fn execute(
        &self,
        id: &SessionId,
        input: String,
        origin: CallOrigin,
    ) -> Result<TurnResult, ProviderError> {
        let (session, turn) = self.begin(id, &input)?;
        tokio::spawn(run_turn(
            self.inner.clone(),
            session,
            turn,
            input,
            Mode::Execute,
            origin,
        ))
        .await
        .map_err(|e| ProviderError::internal(format!("turn task failed: {e}")))
    }

    fn begin(
        &self,
        id: &SessionId,
        input: &str,
    ) -> Result<(Arc<Session>, TurnStart), ProviderError> {
        if input.trim().is_empty() {
            return Err(ProviderError::invalid("input is empty"));
        }
        let session = self.session(id)?;
        let provider = self.inner.provider(&session)?;
        let mut state = session.state.lock();
        match state.info.status {
            SessionStatus::Closed => {
                return Err(ProviderError::new(
                    ProviderErrorKind::Closed,
                    "session is closed; resume it first",
                ))
            }
            SessionStatus::Running => {
                return Err(ProviderError::new(
                    ProviderErrorKind::Busy,
                    "a turn is already running in this session",
                ))
            }
            SessionStatus::Idle => {}
        }
        let turn_id = TurnId::new();
        let cancel = CancellationToken::new();
        let (done, done_rx) = watch::channel(false);
        state.running = Some(RunningTurn {
            turn_id: turn_id.clone(),
            cancel: cancel.clone(),
            usage: TokenUsage::default(),
            text: String::new(),
            done: done_rx,
        });
        session.record(
            &mut state,
            SessionEvent::TurnStarted {
                turn_id: turn_id.clone(),
                input: input.to_owned(),
            },
        );
        session.set_status(&mut state, SessionStatus::Running);
        *session.provider.lock() = provider.clone();
        let turn = TurnStart {
            provider,
            turn_id,
            cancel,
            done,
            native: state.native.clone(),
            project_path: state.spec.project_path.clone(),
        };
        drop(state);
        Ok((session, turn))
    }

    /// Cancels the running turn, if any (`cancel`).
    pub async fn cancel(&self, id: &SessionId) -> Result<SessionInfo, ProviderError> {
        let session = self.session(id)?;
        let (native, turn_id) = {
            let state = session.state.lock();
            match &state.running {
                Some(running) => {
                    running.cancel.cancel();
                    (state.native.clone(), running.turn_id.clone())
                }
                None => return Ok(state.info.clone()),
            }
        };
        let provider = session.provider.lock().clone();
        if let Err(err) = provider.cancel(&native).await {
            let mut state = session.state.lock();
            session.record(
                &mut state,
                SessionEvent::Notice {
                    turn_id: Some(turn_id),
                    level: NoticeLevel::Warning,
                    message: format!("provider cancel failed: {err}"),
                },
            );
        }
        self.info(id)
    }

    /// Closes a session, cancelling its running turn first.
    pub async fn close(
        &self,
        id: &SessionId,
        origin: CallOrigin,
    ) -> Result<SessionInfo, ProviderError> {
        let session = self.session(id)?;
        let done = {
            let state = session.state.lock();
            if state.info.status == SessionStatus::Closed {
                return Ok(state.info.clone());
            }
            state.running.as_ref().map(|r| r.done.clone())
        };
        if let Some(mut done) = done {
            self.cancel(id).await?;
            let wait = self.inner.config.cancel_grace + Duration::from_secs(1);
            let _ = tokio::time::timeout(wait, done.wait_for(|finished| *finished)).await;
        }
        let info = {
            let mut state = session.state.lock();
            if state.info.status == SessionStatus::Closed {
                return Ok(state.info.clone());
            }
            session.set_status(&mut state, SessionStatus::Closed);
            state.info.clone()
        };
        self.inner.sink.audit(AuditEvent::new(
            EventKind::SessionClosed,
            origin,
            format!("session closed · {}", info.title),
            json!({
                "sessionId": info.id,
                "provider": info.provider,
                "turns": info.turns,
                "usage": info.usage,
            }),
        ));
        Ok(info)
    }

    /// Reopens a closed session through the provider (`resume`).
    pub async fn resume(
        &self,
        id: &SessionId,
        origin: CallOrigin,
    ) -> Result<SessionInfo, ProviderError> {
        let session = self.session(id)?;
        let (native, spec) = {
            let state = session.state.lock();
            if state.info.status != SessionStatus::Closed {
                return Ok(state.info.clone());
            }
            (state.native.clone(), state.spec.clone())
        };
        let provider = self.inner.provider(&session)?;
        let native = provider.resume(&native, &spec).await?;
        let info = {
            let mut state = session.state.lock();
            if state.info.status != SessionStatus::Closed {
                return Ok(state.info.clone());
            }
            state.info.native_ref = Some(native.reference.clone());
            if native.model.is_some() {
                state.info.model = native.model.clone();
            }
            state.native = native;
            *session.provider.lock() = provider;
            session.set_status(&mut state, SessionStatus::Idle);
            state.info.clone()
        };
        self.inner.sink.audit(AuditEvent::new(
            EventKind::SessionResumed,
            origin,
            format!("session resumed · {}", info.title),
            json!({
                "sessionId": info.id,
                "provider": info.provider,
                "nativeRef": info.native_ref,
            }),
        ));
        Ok(info)
    }

    /// Cancels every running turn (app exit). Best effort.
    pub async fn shutdown(&self) {
        let sessions: Vec<_> = self.inner.sessions.read().clone();
        for session in sessions {
            let native = {
                let state = session.state.lock();
                state.running.as_ref().map(|running| {
                    running.cancel.cancel();
                    state.native.clone()
                })
            };
            if let Some(native) = native {
                let provider = session.provider.lock().clone();
                let _ =
                    tokio::time::timeout(Duration::from_secs(2), provider.cancel(&native)).await;
            }
        }
    }
}

async fn run_turn(
    inner: Arc<Inner>,
    session: Arc<Session>,
    turn: TurnStart,
    input: String,
    mode: Mode,
    origin: CallOrigin,
) -> TurnResult {
    let clock = Instant::now();
    let provider = turn.provider.clone();
    let descriptor = provider.descriptor();
    let ctx = TurnContext::new(
        session.id.clone(),
        descriptor.id.clone(),
        turn.turn_id.clone(),
        turn.project_path.clone(),
        inner.tools.clone(),
        Arc::new(Recorder(session.clone())),
        turn.cancel.clone(),
    );
    let turn_input = TurnInput {
        text: input.clone(),
    };
    // The provider runs in its own task: a panicking adapter fails the turn
    // instead of leaving the session running forever, and an abandoned turn
    // is aborted.
    let task = {
        let provider = provider.clone();
        let native = turn.native.clone();
        let ctx = ctx.clone();
        tokio::spawn(async move {
            match mode {
                Mode::Stream => provider.stream(&native, &turn_input, &ctx).await,
                Mode::Execute => {
                    let output = provider.execute(&native, &turn_input, &ctx).await?;
                    ctx.emit_text(&output.text);
                    Ok(output)
                }
            }
        })
    };
    let abort = task.abort_handle();
    let grace = inner.config.cancel_grace;
    let abandon = async {
        turn.cancel.cancelled().await;
        tokio::time::sleep(grace).await;
    };
    let outcome: Result<TurnOutput, ProviderError> = tokio::select! {
        joined = task => joined.unwrap_or_else(|err| {
            Err(ProviderError::internal(if err.is_panic() {
                format!("{} panicked during the turn", descriptor.name)
            } else {
                "provider task was aborted".to_owned()
            }))
        }),
        () = abandon => {
            abort.abort();
            Err(ProviderError::cancelled(format!(
                "provider did not stop within {} ms; turn abandoned",
                grace.as_millis()
            )))
        }
    };

    let (status, error) = match outcome {
        Ok(_) => (TurnStatus::Completed, None),
        Err(err) if err.kind == ProviderErrorKind::Cancelled || turn.cancel.is_cancelled() => {
            (TurnStatus::Cancelled, None)
        }
        Err(err) => (TurnStatus::Failed, Some(err.message)),
    };
    let duration_ms = clock.elapsed().as_millis() as u64;
    let tool_calls = ctx.tool_call_count();

    let result = {
        let mut state = session.state.lock();
        let (usage, text) = state
            .running
            .take()
            .map(|r| (r.usage, r.text))
            .unwrap_or_default();
        state.info.turns += 1;
        state.info.usage += usage;
        state.info.last_error = error.clone();
        session.record(
            &mut state,
            SessionEvent::TurnCompleted {
                turn_id: turn.turn_id.clone(),
                status,
                error: error.clone(),
                usage,
                duration_ms,
                tool_calls,
            },
        );
        if state.info.status == SessionStatus::Running {
            session.set_status(&mut state, SessionStatus::Idle);
        }
        TurnResult {
            turn_id: turn.turn_id.clone(),
            status,
            text,
            error,
            usage,
            duration_ms,
        }
    };
    let _ = turn.done.send(true);

    let status_label = match status {
        TurnStatus::Completed => "completed",
        TurnStatus::Cancelled => "cancelled",
        TurnStatus::Failed => "failed",
    };
    inner.sink.audit(AuditEvent::new(
        EventKind::TurnCompleted,
        origin,
        format!(
            "{} turn {status_label} · {} tokens · {tool_calls} tool call{}",
            descriptor.name,
            result.usage.total_tokens(),
            if tool_calls == 1 { "" } else { "s" }
        ),
        json!({
            "sessionId": session.id,
            "provider": descriptor.id,
            "turnId": result.turn_id,
            "status": status,
            "error": result.error,
            "durationMs": duration_ms,
            "toolCalls": tool_calls,
            "usage": result.usage,
            "inputChars": input.chars().count(),
        }),
    ));
    result
}
