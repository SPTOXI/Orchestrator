//! Project context attached to the first turn of a session (ADR-0013).
//!
//! The [`SessionManager`](crate::SessionManager) asks a [`ContextSource`]
//! (the Context Builder, in the app) once per session, with the first
//! message as the task. The text goes to the provider in
//! [`TurnInput::context`](crate::TurnInput); the transcript and the history
//! keep only the summary.

use async_trait::async_trait;
use orchestrator_core::{ContextSummary, HandoffId, SessionInfo};
use serde::{Deserialize, Serialize};

/// Context options of one session (`StartRequest.context`), kept with it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ContextOptions {
    /// `Some(false)`: no project context for this session. `None`: the
    /// app setting decides.
    pub enabled: Option<bool>,
    /// Token budget (estimate); `None`: the app setting.
    pub budget: Option<u32>,
    /// A handoff this session takes over: its packet goes in the context.
    pub handoff_id: Option<HandoffId>,
}

/// What the source receives.
#[derive(Debug, Clone)]
pub struct ContextRequest {
    pub session: SessionInfo,
    /// The first message of the session.
    pub task: String,
    pub options: ContextOptions,
    /// The session's provider can ask for tools (the memory tools among
    /// them); when not, the context should not point to them.
    pub tools: bool,
}

/// What goes to the provider, and what is recorded about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttachedContext {
    pub text: String,
    pub summary: ContextSummary,
}

#[async_trait]
pub trait ContextSource: Send + Sync + 'static {
    /// `Ok(None)`: nothing to attach (disabled). `Err`: the context could
    /// not be built; the turn goes on without it and says so.
    async fn build(&self, request: ContextRequest) -> Result<Option<AttachedContext>, String>;
}
