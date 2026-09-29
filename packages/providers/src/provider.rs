//! The `AIProvider` interface (section 18 of the master document).

use crate::context::TurnContext;
use crate::error::ProviderError;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use orchestrator_core::{ProviderId, SessionId};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::PathBuf;

/// Who a provider is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderDescriptor {
    /// Stable id, e.g. `openai-codex`.
    pub id: ProviderId,
    pub name: String,
    pub vendor: String,
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelInfo {
    pub id: String,
    pub name: String,
    /// Context window in tokens, when known.
    pub context_window: Option<u32>,
}

/// What a provider supports. The Orchestrator adapts to it instead of
/// assuming a specific vendor.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderCapabilities {
    /// Emits output incrementally (`stream`).
    pub streaming: bool,
    /// Asks the Orchestrator to execute tools (`TurnContext::call_tool`).
    pub tool_calls: bool,
    /// Can reopen a native session (`resume`).
    pub resume: bool,
    /// Can stop a running turn.
    pub cancel: bool,
    /// Has its own subagents; otherwise `spawn_agent` opens a new session.
    pub native_subagents: bool,
    /// Exposes reasoning / thinking output.
    pub reasoning: bool,
    /// Reports token usage.
    pub token_usage: bool,
    /// Reports cost.
    pub cost: bool,
    pub models: Vec<ModelInfo>,
    pub default_model: Option<String>,
}

/// Result of `inspect`: can this provider be used right now?
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderStatus {
    pub available: bool,
    pub version: Option<String>,
    /// `None` when not applicable or unknown.
    pub authenticated: Option<bool>,
    /// Human readable detail (what is missing, where it was found, …).
    pub detail: Option<String>,
    pub checked_at: DateTime<Utc>,
}

/// The provider's own session (e.g. a Codex thread). Opaque to the
/// Orchestrator, which only stores it to call the provider again.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NativeSession {
    /// Provider-native id, shown to the user and used for `resume`.
    pub reference: String,
    pub model: Option<String>,
    /// Provider-specific state needed to resume.
    #[serde(default)]
    pub data: Value,
}

/// What a session is opened for.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionSpec {
    /// Orchestrator session id (not the native one).
    pub session_id: SessionId,
    /// Project workspace: the working directory of the session.
    pub project_path: PathBuf,
    pub title: String,
    /// Requested model; `None` = provider default.
    pub model: Option<String>,
    /// System instructions (built by the Context Builder from Phase 7 on).
    pub instructions: Option<String>,
}

/// Input of one turn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnInput {
    pub text: String,
}

/// Aggregated output of one turn.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TurnOutput {
    /// Final assistant text.
    pub text: String,
}

/// An AI provider adapter. Implementations live in their own crates
/// (`providers/openai`, `providers/claude`, …) and never touch the operating
/// system: every operation goes through [`TurnContext::call_tool`].
///
/// Text output goes to the context (`emit_text`) only in `stream`; `execute`
/// returns it in [`TurnOutput`] and the Orchestrator records it.
#[async_trait]
pub trait AIProvider: Send + Sync + 'static {
    fn descriptor(&self) -> ProviderDescriptor;

    fn capabilities(&self) -> ProviderCapabilities;

    /// Checks whether the provider can be used (installed, authenticated,
    /// version).
    async fn inspect(&self) -> ProviderStatus;

    /// Opens a native session.
    async fn start(&self, spec: &SessionSpec) -> Result<NativeSession, ProviderError>;

    /// Reopens a native session. Returns the (possibly updated) session.
    async fn resume(
        &self,
        _native: &NativeSession,
        _spec: &SessionSpec,
    ) -> Result<NativeSession, ProviderError> {
        Err(ProviderError::unsupported(format!(
            "{} cannot resume sessions",
            self.descriptor().name
        )))
    }

    /// Runs one turn and returns the complete answer.
    async fn execute(
        &self,
        native: &NativeSession,
        input: &TurnInput,
        ctx: &TurnContext,
    ) -> Result<TurnOutput, ProviderError>;

    /// Runs one turn emitting output as it is produced. The default runs
    /// `execute` and emits the answer at once.
    async fn stream(
        &self,
        native: &NativeSession,
        input: &TurnInput,
        ctx: &TurnContext,
    ) -> Result<TurnOutput, ProviderError> {
        let output = self.execute(native, input, ctx).await?;
        ctx.emit_text(&output.text);
        Ok(output)
    }

    /// Provider-side cleanup when a turn is cancelled (kill a CLI, send an
    /// interrupt, …). The turn also sees the cancellation through its
    /// context.
    async fn cancel(&self, _native: &NativeSession) -> Result<(), ProviderError> {
        Ok(())
    }

    /// Opens a subagent session derived from `parent`. The default opens an
    /// independent session; providers with native subagents override it.
    async fn spawn_agent(
        &self,
        _parent: &NativeSession,
        spec: &SessionSpec,
    ) -> Result<NativeSession, ProviderError> {
        self.start(spec).await
    }
}
