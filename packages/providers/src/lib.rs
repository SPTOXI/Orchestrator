//! Orchestrator AI Provider Layer (ADR-0009).
//!
//! - [`AIProvider`]: the interface every AI adapter implements
//!   (`start · resume · execute · stream · cancel · spawn_agent · inspect ·
//!   capabilities`).
//! - [`ProviderRegistry`]: registered providers and the active one.
//! - [`SessionManager`]: provider sessions owned by the Orchestrator —
//!   turns, transcript, token usage, live events and history.
//! - [`TurnContext`]: what a provider receives during a turn. Its
//!   [`TurnContext::call_tool`] is the only way a provider acts on the
//!   system; the Orchestrator executes the call through a [`ToolExecutor`]
//!   (the Tool Runtime in the app).
//! - [`EchoProvider`]: development provider without AI.
//!
//! Adapters live in their own crates under `packages/providers/*` and
//! depend only on this crate and `orchestrator-core`; the API connections
//! (OpenAI and compatible, Anthropic, Gemini, generic profile) are in
//! `packages/providers/api` (ADR-0010).

mod context;
mod echo;
mod error;
mod log;
mod manager;
mod provider;
mod registry;
mod store;

pub use context::{ToolExecutor, TurnContext, TurnObserver};
pub use echo::EchoProvider;
pub use error::{ProviderError, ProviderErrorKind};
pub use manager::{ManagerConfig, SessionManager, SessionSnapshot, StartRequest, TurnResult};
pub use provider::{
    AIProvider, Completion, CompletionRequest, ModelInfo, NativeSession, ProviderCapabilities,
    ProviderDescriptor, ProviderStatus, SessionSpec, TurnInput, TurnOutput,
};
pub use registry::{ProviderInfo, ProviderRegistry};
pub use store::{MemorySessionStore, PersistedSession, SessionStore};
