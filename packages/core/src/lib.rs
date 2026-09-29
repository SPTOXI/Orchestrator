//! Domain contracts shared by every Orchestrator component.
//!
//! This crate has no I/O. It defines what flows between the UI, the Tool
//! Runtime, the orchestrator engine and AI providers:
//!
//! - [`ToolCall`] / [`ToolResult`]: the only way anything (a human through the
//!   UI, or an AI agent) asks the Orchestrator to touch the operating system.
//! - [`AuditEvent`]: durable, provider-independent history.
//! - [`StreamEvent`]: high-frequency, non-durable output (terminal, process,
//!   provider session).
//! - [`EventSink`]: where the runtime publishes both kinds of events.
//! - [`ProjectProfile`]: what the Orchestrator knows about a project.
//! - [`SessionInfo`] / [`SessionEvent`]: provider sessions (ADR-0009).
//! - [`HandoffPacket`] / [`ContextSummary`]: handoff between AIs and the
//!   project context sent to them (ADR-0013).

pub mod context;
pub mod event;
pub mod handoff;
pub mod ids;
pub mod project;
pub mod session;
pub mod tool;

pub use context::{ContextSectionSummary, ContextSummary};
pub use event::{
    AuditEvent, EventKind, EventSink, MemorySink, NullSink, OutputStream, StreamEvent,
};
pub use handoff::{Handoff, HandoffEnd, HandoffPacket, HandoffStatus};
pub use ids::{
    DeliberationId, EventId, HandoffId, ProcessId, ProviderId, SessionId, TerminalId, ToolCallId,
    TurnId,
};
pub use project::{
    DockerInfo, GitRemote, GitSummary, ProjectCandidate, ProjectProfile, RuntimeRequirement,
};
pub use session::{
    NoticeLevel, SessionEvent, SessionInfo, SessionLogEntry, SessionStatus, TokenUsage, TurnStatus,
};
pub use tool::{
    CallOrigin, ToolCall, ToolDefinition, ToolError, ToolErrorKind, ToolResult, ToolSpec,
};
