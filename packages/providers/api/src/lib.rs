//! User-registered AI APIs as Orchestrator providers (ADR-0010).
//!
//! - [`Connection`]: one API the user registered (any vendor, any number).
//!   Native protocols: OpenAI Chat Completions (and compatible APIs),
//!   Anthropic Messages, Google Gemini; anything else through a
//!   [`GenericProfile`] that describes the HTTP/JSON API without code.
//! - [`ApiProvider`]: a connection as an `AIProvider`. Each turn loops model
//!   → tools (executed by the Orchestrator via `TurnContext::call_tool`) →
//!   model, with native function calling or the prompt tool protocol.
//! - [`ConnectionManager`]: persistence (no secrets), registry, test and
//!   model discovery. Keys live in a [`SecretStore`] (the OS vault in the
//!   app) or in an environment variable.

mod anthropic;
mod compaction;
mod config;
mod conversation;
mod cost;
mod gemini;
mod generic;
mod http;
mod jsonpath;
pub mod local;
mod manager;
mod openai;
mod presets;
mod protocol;
mod provider;
mod secrets;
mod tools;

pub use config::{
    ApiKind, CacheTtl, Connection, Credential, CredentialSource, Fallback, GenericAuth,
    GenericProfile, MessageFormat, ModelEntry, ProtocolOptions, RoleNames, StreamFormat, ToolMode,
    DEFAULT_FIRST_RESPONSE_SECS, DEFAULT_MAX_TOOL_ROUNDS,
};
pub use manager::{ConnectionManager, ConnectionView, KeyStatus, ProbeRequest, SaveRequest};
pub use presets::{presets, Preset};
pub use provider::{ApiProvider, TestReport};
pub use secrets::{MemorySecretStore, SecretStore};
