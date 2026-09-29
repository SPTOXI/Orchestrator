//! Provider sessions end to end: registry, session manager, the `echo`
//! provider and tool calls executed by the real Tool Runtime.

use async_trait::async_trait;
use orchestrator_core::{
    AuditEvent, CallOrigin, EventKind, MemorySink, ProviderId, SessionEvent, SessionId,
    SessionInfo, SessionStatus, StreamEvent, ToolCall, ToolErrorKind, ToolResult, ToolSpec,
    TurnStatus,
};
use orchestrator_providers::{
    AIProvider, EchoProvider, ManagerConfig, NativeSession, ProviderCapabilities,
    ProviderDescriptor, ProviderErrorKind, ProviderRegistry, ProviderStatus, SessionManager,
    SessionSpec, StartRequest, ToolExecutor, TurnContext, TurnInput, TurnOutput,
};
use orchestrator_runtime::{RuntimeConfig, ToolRuntime};
use parking_lot::Mutex;
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tempfile::TempDir;

/// The app wires the Tool Runtime in the same way (apps/desktop).
struct RuntimeTools(ToolRuntime);

#[async_trait]
impl ToolExecutor for RuntimeTools {
    fn catalog(&self) -> Vec<ToolSpec> {
        ToolRuntime::catalog().to_vec()
    }

    async fn execute(&self, call: ToolCall) -> ToolResult {
        self.0.invoke(call).await
    }
}

struct Harness {
    manager: SessionManager,
    sink: Arc<MemorySink>,
    _dir: TempDir,
}

fn config() -> ManagerConfig {
    ManagerConfig {
        cancel_grace: Duration::from_millis(200),
        log_capacity: 1_000,
    }
}

fn registry(sink: &Arc<MemorySink>, extra: Vec<Arc<dyn AIProvider>>) -> Arc<ProviderRegistry> {
    let registry = Arc::new(ProviderRegistry::new(sink.clone()));
    registry
        .register(Arc::new(
            EchoProvider::new().with_chunk_delay(Duration::ZERO),
        ))
        .unwrap();
    registry
        .register(Arc::new(
            EchoProvider::with_identity("echo-b", "Echo B").with_chunk_delay(Duration::ZERO),
        ))
        .unwrap();
    for provider in extra {
        registry.register(provider).unwrap();
    }
    registry
}

fn harness_with(extra: Vec<Arc<dyn AIProvider>>) -> Harness {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("hello.txt"), "olá do projeto").unwrap();
    let sink = Arc::new(MemorySink::new());
    let runtime = ToolRuntime::new(
        RuntimeConfig {
            base_dir: dir.path().to_path_buf(),
        },
        sink.clone(),
    );
    let manager = SessionManager::with_config(
        registry(&sink, extra),
        Arc::new(RuntimeTools(runtime)),
        sink.clone(),
        config(),
    );
    Harness {
        manager,
        sink,
        _dir: dir,
    }
}

fn harness() -> Harness {
    harness_with(Vec::new())
}

impl Harness {
    async fn start(&self) -> SessionInfo {
        self.manager
            .start(
                StartRequest::default(),
                self._dir.path().to_path_buf(),
                CallOrigin::User,
            )
            .await
            .unwrap()
    }

    async fn send(&self, id: &SessionId, input: &str) {
        self.manager
            .send(id, input.into(), CallOrigin::User)
            .await
            .unwrap();
    }

    /// Waits until no turn is running.
    async fn idle(&self, id: &SessionId) -> SessionInfo {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let info = self.manager.info(id).unwrap();
            if info.status != SessionStatus::Running {
                return info;
            }
            assert!(Instant::now() < deadline, "turn did not finish");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    async fn running(&self, id: &SessionId) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while self.manager.info(id).unwrap().status != SessionStatus::Running {
            assert!(Instant::now() < deadline, "turn did not start");
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }

    fn events(&self, id: &SessionId) -> Vec<SessionEvent> {
        self.manager
            .snapshot(id)
            .unwrap()
            .entries
            .into_iter()
            .map(|e| e.event)
            .collect()
    }

    fn audits(&self, kind: EventKind) -> Vec<AuditEvent> {
        self.sink
            .audit_events()
            .into_iter()
            .filter(|e| e.kind == kind)
            .collect()
    }

    fn last_turn(&self, id: &SessionId) -> (TurnStatus, Option<String>, u32) {
        self.events(id)
            .into_iter()
            .rev()
            .find_map(|e| match e {
                SessionEvent::TurnCompleted {
                    status,
                    error,
                    tool_calls,
                    ..
                } => Some((status, error, tool_calls)),
                _ => None,
            })
            .expect("a completed turn")
    }
}

fn text_of(events: &[SessionEvent]) -> String {
    events
        .iter()
        .filter_map(|e| match e {
            SessionEvent::TextDelta { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn streams_a_turn_and_accounts_usage() {
    let h = harness();
    let info = h.start().await;
    assert_eq!(info.provider, ProviderId::from("echo"));
    assert_eq!(info.status, SessionStatus::Idle);
    assert_eq!(info.model.as_deref(), Some("echo-1"));
    assert_eq!(info.title, "Echo #1");
    assert!(info.native_ref.as_deref().unwrap().starts_with("echo-"));

    h.send(&info.id, "olá mundo").await;
    let done = h.idle(&info.id).await;
    assert_eq!(done.turns, 1);
    assert_eq!(done.usage.input_tokens, 3);
    assert_eq!(done.usage.output_tokens, 4);
    assert!(done.usage.estimated);

    let events = h.events(&info.id);
    assert!(
        matches!(events[0], SessionEvent::TurnStarted { ref input, .. } if input == "olá mundo")
    );
    assert!(matches!(
        events[1],
        SessionEvent::StatusChanged {
            status: SessionStatus::Running
        }
    ));
    assert_eq!(text_of(&events), "Eco: olá mundo");
    // Word fragments are merged into one transcript entry.
    assert_eq!(
        events
            .iter()
            .filter(|e| matches!(e, SessionEvent::TextDelta { .. }))
            .count(),
        1
    );
    assert_eq!(h.last_turn(&info.id).0, TurnStatus::Completed);
    assert!(matches!(
        events.last(),
        Some(SessionEvent::StatusChanged {
            status: SessionStatus::Idle
        })
    ));

    // Live events carry increasing sequence numbers matching the snapshot.
    let seqs: Vec<u64> = h
        .sink
        .stream_events()
        .into_iter()
        .filter_map(|e| match e {
            StreamEvent::Session { seq, .. } => Some(seq),
            _ => None,
        })
        .collect();
    assert!(seqs.windows(2).all(|w| w[0] < w[1]), "{seqs:?}");
    assert_eq!(
        *seqs.last().unwrap(),
        h.manager.snapshot(&info.id).unwrap().last_seq
    );

    assert_eq!(h.audits(EventKind::SessionStarted).len(), 1);
    let turn = &h.audits(EventKind::TurnCompleted)[0];
    assert_eq!(turn.data["status"], "completed");
    assert_eq!(turn.data["usage"]["inputTokens"], 3);
    assert_eq!(turn.origin, CallOrigin::User);
}

#[tokio::test(flavor = "multi_thread")]
async fn tool_calls_run_in_the_runtime_on_behalf_of_the_session() {
    let h = harness();
    let info = h.start().await;
    h.send(&info.id, r#"/tool filesystem.read {"path": "hello.txt"}"#)
        .await;
    h.idle(&info.id).await;

    let events = h.events(&info.id);
    let call = events
        .iter()
        .find_map(|e| match e {
            SessionEvent::ToolCallRequested { call, .. } => Some(call.clone()),
            _ => None,
        })
        .expect("tool call requested");
    assert_eq!(call.tool, "filesystem.read");
    assert_eq!(
        call.origin,
        CallOrigin::session(&info.id, &ProviderId::from("echo"))
    );
    let result = events
        .iter()
        .find_map(|e| match e {
            SessionEvent::ToolCallCompleted { result, .. } => Some(result.clone()),
            _ => None,
        })
        .expect("tool call completed");
    assert!(result.ok, "{result:?}");
    assert_eq!(result.call_id, call.id);
    assert!(text_of(&events).contains("olá do projeto"));
    assert_eq!(h.last_turn(&info.id).2, 1);

    // The runtime audited the call with the session as origin.
    let audited = h
        .audits(EventKind::ToolCalled)
        .into_iter()
        .find(|e| e.call_id.as_ref() == Some(&call.id))
        .expect("TOOL_CALLED for the agent call");
    assert_eq!(audited.origin, call.origin);
    assert_eq!(audited.data["ok"], true);
    assert_eq!(h.audits(EventKind::TurnCompleted)[0].data["toolCalls"], 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn failed_tool_calls_are_reported_to_the_provider() {
    let h = harness();
    let info = h.start().await;
    h.send(&info.id, r#"/tool filesystem.read {"path": "missing.txt"}"#)
        .await;
    h.idle(&info.id).await;
    let events = h.events(&info.id);
    assert!(
        text_of(&events).contains("NotFound"),
        "{}",
        text_of(&events)
    );
    // The turn itself completed: the provider handled the tool error.
    assert_eq!(h.last_turn(&info.id).0, TurnStatus::Completed);
}

#[tokio::test(flavor = "multi_thread")]
async fn cancel_stops_a_running_turn() {
    let h = harness();
    let info = h.start().await;
    h.send(&info.id, "/wait 30").await;
    h.running(&info.id).await;

    let busy = h
        .manager
        .send(&info.id, "outra".into(), CallOrigin::User)
        .await
        .unwrap_err();
    assert_eq!(busy.kind, ProviderErrorKind::Busy);

    let started = Instant::now();
    h.manager.cancel(&info.id).await.unwrap();
    let done = h.idle(&info.id).await;
    assert!(started.elapsed() < Duration::from_secs(5));
    assert_eq!(done.status, SessionStatus::Idle);
    assert_eq!(h.last_turn(&info.id).0, TurnStatus::Cancelled);

    h.send(&info.id, "de novo").await;
    h.idle(&info.id).await;
    assert_eq!(h.last_turn(&info.id).0, TurnStatus::Completed);
    let statuses: Vec<_> = h
        .audits(EventKind::TurnCompleted)
        .iter()
        .map(|e| e.data["status"].clone())
        .collect();
    assert_eq!(statuses, vec![json!("cancelled"), json!("completed")]);
}

#[tokio::test(flavor = "multi_thread")]
async fn failures_are_recorded_and_the_session_survives() {
    let h = harness();
    let info = h.start().await;
    h.send(&info.id, "/fail boom").await;
    let failed = h.idle(&info.id).await;
    assert_eq!(failed.last_error.as_deref(), Some("boom"));
    assert_eq!(
        h.last_turn(&info.id),
        (TurnStatus::Failed, Some("boom".into()), 0)
    );

    h.send(&info.id, "/nope").await;
    h.idle(&info.id).await;
    let (status, error, _) = h.last_turn(&info.id);
    assert_eq!(status, TurnStatus::Failed);
    assert!(error.unwrap().contains("/nope"));

    h.send(&info.id, "ok").await;
    let recovered = h.idle(&info.id).await;
    assert_eq!(recovered.last_error, None);
    assert_eq!(recovered.turns, 3);

    let empty = h
        .manager
        .send(&info.id, "   ".into(), CallOrigin::User)
        .await
        .unwrap_err();
    assert_eq!(empty.kind, ProviderErrorKind::InvalidRequest);
    let unknown = h.manager.info(&SessionId::from("nope")).unwrap_err();
    assert_eq!(unknown.kind, ProviderErrorKind::NotFound);
}

#[tokio::test(flavor = "multi_thread")]
async fn close_cancels_and_resume_reopens() {
    let h = harness();
    let info = h.start().await;
    h.send(&info.id, "/wait 30").await;
    h.running(&info.id).await;

    let closed = h.manager.close(&info.id, CallOrigin::User).await.unwrap();
    assert_eq!(closed.status, SessionStatus::Closed);
    assert_eq!(h.last_turn(&info.id).0, TurnStatus::Cancelled);
    let refused = h
        .manager
        .send(&info.id, "oi".into(), CallOrigin::User)
        .await
        .unwrap_err();
    assert_eq!(refused.kind, ProviderErrorKind::Closed);
    assert_eq!(h.audits(EventKind::SessionClosed).len(), 1);

    let resumed = h.manager.resume(&info.id, CallOrigin::User).await.unwrap();
    assert_eq!(resumed.status, SessionStatus::Idle);
    assert_eq!(resumed.native_ref, info.native_ref);
    assert_eq!(h.audits(EventKind::SessionResumed).len(), 1);
    h.send(&info.id, "voltei").await;
    assert_eq!(h.idle(&info.id).await.turns, 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn execute_returns_the_whole_answer() {
    let h = harness();
    let info = h.start().await;
    let result = h
        .manager
        .execute(&info.id, "abc".into(), CallOrigin::User)
        .await
        .unwrap();
    assert_eq!(result.status, TurnStatus::Completed);
    assert_eq!(result.text, "Eco: abc");
    assert_eq!(result.usage.output_tokens, 2);
    assert_eq!(text_of(&h.events(&info.id)), "Eco: abc");
    assert_eq!(
        h.manager.info(&info.id).unwrap().status,
        SessionStatus::Idle
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn spawn_opens_subagent_sessions() {
    let h = harness();
    let parent = h.start().await;

    let child = h
        .manager
        .spawn(&parent.id, StartRequest::default(), CallOrigin::User)
        .await
        .unwrap();
    assert_eq!(child.parent_id.as_ref(), Some(&parent.id));
    assert_eq!(child.provider, parent.provider);
    assert_eq!(child.title, "Echo #1 › sub 1");
    assert_eq!(child.project_path, parent.project_path);

    // Delegation to another provider.
    let other = h
        .manager
        .spawn(
            &parent.id,
            StartRequest {
                provider: Some(ProviderId::from("echo-b")),
                title: Some("revisão".into()),
                ..Default::default()
            },
            CallOrigin::User,
        )
        .await
        .unwrap();
    assert_eq!(other.provider, ProviderId::from("echo-b"));
    assert_eq!(other.title, "revisão");

    let spawned: Vec<_> = h
        .events(&parent.id)
        .into_iter()
        .filter_map(|e| match e {
            SessionEvent::SubagentSpawned { child_id, .. } => Some(child_id),
            _ => None,
        })
        .collect();
    assert_eq!(spawned, vec![child.id.clone(), other.id.clone()]);
    let started = h.audits(EventKind::SessionStarted);
    assert_eq!(started.len(), 3);
    assert_eq!(started[1].data["parentSessionId"], json!(parent.id));

    // Children work like any session; newest first in the list.
    h.send(&child.id, "sub").await;
    assert_eq!(h.idle(&child.id).await.turns, 1);
    let listed: Vec<_> = h.manager.list().into_iter().map(|s| s.id).collect();
    assert_eq!(listed, vec![other.id, child.id, parent.id]);
}

#[tokio::test(flavor = "multi_thread")]
async fn registry_selects_the_active_provider() {
    let h = harness();
    let registry = h.manager.registry();
    let listed = registry.list();
    assert_eq!(listed.len(), 2);
    assert!(listed[0].active && !listed[1].active);
    assert!(listed[0].capabilities.streaming);

    registry
        .select(&ProviderId::from("echo-b"), CallOrigin::User)
        .unwrap();
    registry
        .select(&ProviderId::from("echo-b"), CallOrigin::User)
        .unwrap();
    let switched = h.audits(EventKind::ProviderSwitched);
    assert_eq!(
        switched.len(),
        1,
        "selecting the active provider is a no-op"
    );
    assert_eq!(switched[0].data, json!({"from": "echo", "to": "echo-b"}));

    let missing = registry
        .select(&ProviderId::from("gemini"), CallOrigin::User)
        .unwrap_err();
    assert_eq!(missing.kind, ProviderErrorKind::NotFound);
    let duplicate = registry
        .register(Arc::new(EchoProvider::new()))
        .unwrap_err();
    assert_eq!(duplicate.kind, ProviderErrorKind::AlreadyExists);

    // New sessions use the active provider.
    assert_eq!(h.start().await.provider, ProviderId::from("echo-b"));
    let status = registry.inspect(&ProviderId::from("echo")).await.unwrap();
    assert!(status.available);

    let empty = SessionManager::new(
        Arc::new(ProviderRegistry::new(h.sink.clone())),
        Arc::new(NoTools),
        h.sink.clone(),
    );
    let none = empty
        .start(StartRequest::default(), ".".into(), CallOrigin::User)
        .await
        .unwrap_err();
    assert_eq!(none.kind, ProviderErrorKind::NotFound);
}

/// Ignores cancellation, then tries a tool after being cancelled.
struct StubbornProvider {
    after_cancel: Arc<Mutex<Option<ToolResult>>>,
}

#[async_trait]
impl AIProvider for StubbornProvider {
    fn descriptor(&self) -> ProviderDescriptor {
        ProviderDescriptor {
            id: ProviderId::from("stubborn"),
            name: "Stubborn".into(),
            vendor: "tests".into(),
            description: String::new(),
        }
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities::default()
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

    async fn start(
        &self,
        _spec: &SessionSpec,
    ) -> Result<NativeSession, orchestrator_providers::ProviderError> {
        Ok(NativeSession {
            reference: "stubborn-1".into(),
            model: None,
            data: Value::Null,
        })
    }

    async fn execute(
        &self,
        _native: &NativeSession,
        _input: &TurnInput,
        ctx: &TurnContext,
    ) -> Result<TurnOutput, orchestrator_providers::ProviderError> {
        ctx.cancelled().await;
        let result = ctx.call_tool("process.list", json!({})).await;
        *self.after_cancel.lock() = Some(result);
        tokio::time::sleep(Duration::from_secs(30)).await;
        Ok(TurnOutput::default())
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_provider_that_ignores_cancel_is_abandoned_and_starts_no_tools() {
    let after_cancel = Arc::new(Mutex::new(None));
    let h = harness_with(vec![Arc::new(StubbornProvider {
        after_cancel: after_cancel.clone(),
    })]);
    let info = h
        .manager
        .start(
            StartRequest {
                provider: Some(ProviderId::from("stubborn")),
                ..Default::default()
            },
            h._dir.path().to_path_buf(),
            CallOrigin::User,
        )
        .await
        .unwrap();
    h.send(&info.id, "go").await;
    h.running(&info.id).await;
    let started = Instant::now();
    h.manager.cancel(&info.id).await.unwrap();
    h.idle(&info.id).await;
    assert!(started.elapsed() < Duration::from_secs(5));
    assert_eq!(h.last_turn(&info.id).0, TurnStatus::Cancelled);

    let result = after_cancel.lock().clone().expect("tool attempted");
    assert_eq!(result.error.unwrap().kind, ToolErrorKind::Cancelled);
    assert!(
        !h.audits(EventKind::ToolCalled)
            .iter()
            .any(|e| e.data["tool"] == "process.list"),
        "a cancelled turn must not start tools"
    );
}

/// Executor whose tool takes a while (to cancel while it runs).
struct SlowTools;

#[async_trait]
impl ToolExecutor for SlowTools {
    fn catalog(&self) -> Vec<ToolSpec> {
        Vec::new()
    }

    async fn execute(&self, call: ToolCall) -> ToolResult {
        tokio::time::sleep(Duration::from_millis(600)).await;
        let now = chrono::Utc::now();
        ToolResult {
            call_id: call.id,
            tool: call.tool,
            ok: true,
            output: json!("done"),
            error: None,
            started_at: now,
            finished_at: now,
            duration_ms: 600,
        }
    }
}

/// Executor for managers that never run tools.
struct NoTools;

#[async_trait]
impl ToolExecutor for NoTools {
    fn catalog(&self) -> Vec<ToolSpec> {
        Vec::new()
    }

    async fn execute(&self, _call: ToolCall) -> ToolResult {
        unreachable!("no tools in this test")
    }
}

/// Calls one slow tool and ignores cancellation.
struct SlowToolProvider;

#[async_trait]
impl AIProvider for SlowToolProvider {
    fn descriptor(&self) -> ProviderDescriptor {
        ProviderDescriptor {
            id: ProviderId::from("slow"),
            name: "Slow".into(),
            vendor: "tests".into(),
            description: String::new(),
        }
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities::default()
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

    async fn start(
        &self,
        _spec: &SessionSpec,
    ) -> Result<NativeSession, orchestrator_providers::ProviderError> {
        Ok(NativeSession {
            reference: "slow-1".into(),
            model: None,
            data: Value::Null,
        })
    }

    async fn execute(
        &self,
        _native: &NativeSession,
        _input: &TurnInput,
        ctx: &TurnContext,
    ) -> Result<TurnOutput, orchestrator_providers::ProviderError> {
        ctx.call_tool("slow.tool", json!({})).await;
        tokio::time::sleep(Duration::from_secs(30)).await;
        Ok(TurnOutput::default())
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_tool_in_flight_finishes_and_is_recorded_after_the_turn_is_abandoned() {
    let sink = Arc::new(MemorySink::new());
    let registry = Arc::new(ProviderRegistry::new(sink.clone()));
    registry.register(Arc::new(SlowToolProvider)).unwrap();
    let manager =
        SessionManager::with_config(registry, Arc::new(SlowTools), sink.clone(), config());
    let info = manager
        .start(StartRequest::default(), ".".into(), CallOrigin::User)
        .await
        .unwrap();
    manager
        .send(&info.id, "go".into(), CallOrigin::User)
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
    manager.cancel(&info.id).await.unwrap();

    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let events: Vec<_> = manager
            .snapshot(&info.id)
            .unwrap()
            .entries
            .into_iter()
            .map(|e| e.event)
            .collect();
        let turn_done = events.iter().position(|e| {
            matches!(
                e,
                SessionEvent::TurnCompleted {
                    status: TurnStatus::Cancelled,
                    ..
                }
            )
        });
        let tool_done = events
            .iter()
            .position(|e| matches!(e, SessionEvent::ToolCallCompleted { result, .. } if result.ok));
        if let (Some(turn_done), Some(tool_done)) = (turn_done, tool_done) {
            assert!(
                tool_done > turn_done,
                "tool finished after the turn was abandoned"
            );
            break;
        }
        assert!(
            Instant::now() < deadline,
            "tool result never recorded: {events:?}"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// Adapter bug: panics in the middle of a turn.
struct PanickingProvider;

#[async_trait]
impl AIProvider for PanickingProvider {
    fn descriptor(&self) -> ProviderDescriptor {
        ProviderDescriptor {
            id: ProviderId::from("panicky"),
            name: "Panicky".into(),
            vendor: "tests".into(),
            description: String::new(),
        }
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities::default()
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

    async fn start(
        &self,
        _spec: &SessionSpec,
    ) -> Result<NativeSession, orchestrator_providers::ProviderError> {
        Ok(NativeSession {
            reference: "panicky-1".into(),
            model: None,
            data: Value::Null,
        })
    }

    async fn execute(
        &self,
        _native: &NativeSession,
        input: &TurnInput,
        ctx: &TurnContext,
    ) -> Result<TurnOutput, orchestrator_providers::ProviderError> {
        ctx.emit_text("antes do erro");
        if input.text == "boom" {
            panic!("adapter bug");
        }
        Ok(TurnOutput { text: "ok".into() })
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_panicking_provider_fails_the_turn_without_wedging_the_session() {
    let h = harness_with(vec![Arc::new(PanickingProvider)]);
    let info = h
        .manager
        .start(
            StartRequest {
                provider: Some(ProviderId::from("panicky")),
                ..Default::default()
            },
            h._dir.path().to_path_buf(),
            CallOrigin::User,
        )
        .await
        .unwrap();
    h.send(&info.id, "boom").await;
    let failed = h.idle(&info.id).await;
    let (status, error, _) = h.last_turn(&info.id);
    assert_eq!(status, TurnStatus::Failed);
    assert!(error.unwrap().contains("panicked"));
    assert!(failed.last_error.is_some());

    // The session keeps working.
    h.send(&info.id, "de novo").await;
    h.idle(&info.id).await;
    assert_eq!(h.last_turn(&info.id).0, TurnStatus::Completed);
}
