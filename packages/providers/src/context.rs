//! What a provider receives while running a turn.
//!
//! [`TurnContext::call_tool`] is the only way a provider acts on the system:
//! the Orchestrator builds the [`ToolCall`] (with the session as origin) and
//! executes it through a [`ToolExecutor`] — the Tool Runtime in the app.

use async_trait::async_trait;
use chrono::Utc;
use orchestrator_core::{
    CallOrigin, NoticeLevel, ProviderId, SessionEvent, SessionId, TokenUsage, ToolCall, ToolError,
    ToolErrorKind, ToolResult, ToolSpec, TurnId,
};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

/// Executes tool calls on behalf of providers. Implemented over
/// `ToolRuntime::invoke` by the app, so agent calls are audited exactly like
/// the user's.
#[async_trait]
pub trait ToolExecutor: Send + Sync + 'static {
    /// Tools the provider may request.
    fn catalog(&self) -> Vec<ToolSpec>;

    async fn execute(&self, call: ToolCall) -> ToolResult;
}

/// Receives the events of a turn: the session manager (transcript + live
/// stream), or a test.
pub trait TurnObserver: Send + Sync + 'static {
    fn event(&self, event: SessionEvent);
}

/// Context of one turn. Cheap to clone.
#[derive(Clone)]
pub struct TurnContext {
    session_id: SessionId,
    provider: ProviderId,
    turn_id: TurnId,
    project_path: PathBuf,
    tools: Arc<dyn ToolExecutor>,
    observer: Arc<dyn TurnObserver>,
    cancel: CancellationToken,
    tool_calls: Arc<AtomicU32>,
}

impl TurnContext {
    pub fn new(
        session_id: SessionId,
        provider: ProviderId,
        turn_id: TurnId,
        project_path: PathBuf,
        tools: Arc<dyn ToolExecutor>,
        observer: Arc<dyn TurnObserver>,
        cancel: CancellationToken,
    ) -> Self {
        Self {
            session_id,
            provider,
            turn_id,
            project_path,
            tools,
            observer,
            cancel,
            tool_calls: Arc::new(AtomicU32::new(0)),
        }
    }

    pub fn session_id(&self) -> &SessionId {
        &self.session_id
    }

    pub fn turn_id(&self) -> &TurnId {
        &self.turn_id
    }

    /// Project workspace (working directory of relative paths).
    pub fn project_path(&self) -> &Path {
        &self.project_path
    }

    /// Tools the provider may request through [`Self::call_tool`].
    pub fn tools(&self) -> Vec<ToolSpec> {
        self.tools.catalog()
    }

    /// Assistant output (streaming).
    pub fn emit_text(&self, text: &str) {
        if !text.is_empty() {
            self.observer.event(SessionEvent::TextDelta {
                turn_id: self.turn_id.clone(),
                text: text.to_owned(),
            });
        }
    }

    /// Reasoning / thinking output.
    pub fn emit_reasoning(&self, text: &str) {
        if !text.is_empty() {
            self.observer.event(SessionEvent::ReasoningDelta {
                turn_id: self.turn_id.clone(),
                text: text.to_owned(),
            });
        }
    }

    /// Adds usage to the turn. May be called several times; values add up.
    pub fn report_usage(&self, usage: TokenUsage) {
        if !usage.is_empty() {
            self.observer.event(SessionEvent::Usage {
                turn_id: self.turn_id.clone(),
                usage,
            });
        }
    }

    pub fn notice(&self, level: NoticeLevel, message: impl Into<String>) {
        self.observer.event(SessionEvent::Notice {
            turn_id: Some(self.turn_id.clone()),
            level,
            message: message.into(),
        });
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancel.is_cancelled()
    }

    /// Resolves when the turn is cancelled.
    pub async fn cancelled(&self) {
        self.cancel.cancelled().await
    }

    /// Asks the Orchestrator to execute a tool and waits for the result.
    ///
    /// The call runs in its own task: if the turn is abandoned meanwhile, the
    /// tool still finishes and is recorded. After cancellation no new tool
    /// is started (`CANCELLED` result).
    pub async fn call_tool(&self, tool: &str, args: Value) -> ToolResult {
        let call = ToolCall::new(
            tool,
            args,
            CallOrigin::session(&self.session_id, &self.provider),
        );
        self.tool_calls.fetch_add(1, Ordering::Relaxed);
        self.observer.event(SessionEvent::ToolCallRequested {
            turn_id: self.turn_id.clone(),
            call: call.clone(),
        });

        if self.is_cancelled() {
            let result = not_executed(
                &call,
                ToolError::new(
                    ToolErrorKind::Cancelled,
                    "turn cancelled: tool not executed",
                ),
            );
            self.completed(result.clone());
            return result;
        }

        let tools = self.tools.clone();
        let this = self.clone();
        let fallback = call.clone();
        let task = tokio::spawn(async move {
            let result = tools.execute(call).await;
            this.completed(result.clone());
            result
        });
        match task.await {
            Ok(result) => result,
            Err(err) => {
                let result = not_executed(
                    &fallback,
                    ToolError::internal(format!("tool task failed: {err}")),
                );
                self.completed(result.clone());
                result
            }
        }
    }

    fn completed(&self, result: ToolResult) {
        self.observer.event(SessionEvent::ToolCallCompleted {
            turn_id: self.turn_id.clone(),
            result,
        });
    }

    /// Tools requested so far in this turn.
    pub fn tool_call_count(&self) -> u32 {
        self.tool_calls.load(Ordering::Relaxed)
    }
}

fn not_executed(call: &ToolCall, error: ToolError) -> ToolResult {
    let now = Utc::now();
    ToolResult {
        call_id: call.id.clone(),
        tool: call.tool.clone(),
        ok: false,
        output: Value::Null,
        error: Some(error),
        started_at: now,
        finished_at: now,
        duration_ms: 0,
    }
}
