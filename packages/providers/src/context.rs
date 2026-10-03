//! What a provider receives while running a turn.
//!
//! [`TurnContext::call_tool`] is the only way a provider acts on the system:
//! the Orchestrator builds the [`ToolCall`] (with the session as origin) and
//! executes it through a [`ToolExecutor`] — the Tool Runtime in the app.

use async_trait::async_trait;
use chrono::Utc;
use orchestrator_core::{
    CallOrigin, NoticeLevel, ProviderId, SessionEvent, SessionId, TokenUsage, ToolCall,
    ToolDefinition, ToolError, ToolErrorKind, ToolResult, TurnId,
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
    /// Tools the provider may request, with the JSON Schema of their
    /// arguments (ADR-0010).
    fn tools(&self) -> Vec<ToolDefinition>;

    async fn execute(&self, call: ToolCall) -> ToolResult;

    /// Executes a call on behalf of a turn that may be cancelled while the
    /// call waits (for the user's authorization or a pause, ADR-0016).
    /// Executors that never wait just run the call.
    async fn execute_with(&self, call: ToolCall, cancel: CancellationToken) -> ToolResult {
        let _ = cancel;
        self.execute(call).await
    }
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
    pub fn tools(&self) -> Vec<ToolDefinition> {
        self.tools.tools()
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

    /// The conversation was compacted (ADR-0018): recorded in the
    /// transcript and, by the session manager, in the history.
    pub fn compacted(&self, compaction: Compaction) {
        self.observer.event(SessionEvent::Compacted {
            turn_id: self.turn_id.clone(),
            automatic: compaction.automatic,
            before_tokens: compaction.before_tokens,
            after_tokens: compaction.after_tokens,
            messages: compaction.messages,
            summary: compaction.summary,
        });
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

    /// The turn's cancellation token (to pass into I/O helpers).
    pub fn cancellation(&self) -> CancellationToken {
        self.cancel.clone()
    }

    /// Asks the Orchestrator to execute a tool and waits for the result.
    ///
    /// The call runs in its own task: if the turn is abandoned meanwhile, the
    /// tool still finishes and is recorded. After cancellation no new tool
    /// is started (`CANCELLED` result).
    pub async fn call_tool(&self, tool: &str, args: Value) -> ToolResult {
        // The session's project, not whichever one the app shows now
        // (ADR-0023).
        let call = ToolCall::new(
            tool,
            args,
            CallOrigin::session(&self.session_id, &self.provider),
        )
        .in_workspace(&self.project_path);
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
        let cancel = self.cancel.clone();
        let task = tokio::spawn(async move {
            let result = tools.execute_with(call, cancel).await;
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

/// What a compaction did (`TurnContext::compacted`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Compaction {
    pub automatic: bool,
    pub before_tokens: u64,
    pub after_tokens: u64,
    pub messages: u32,
    pub summary: String,
}
