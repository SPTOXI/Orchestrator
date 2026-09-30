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
//! - [`Task`]: a piece of work of the project, with state and dependencies
//!   (ADR-0014).
//! - [`Agent`] / [`FileLock`]: who executes a task, and the files it holds
//!   while it does (ADR-0015).
//! - [`AutonomyMode`] / [`PolicyRule`] / [`ApprovalRequest`]: what an AI
//!   may do on its own, and what the user is asked (ADR-0016).

pub mod agent;
pub mod autonomy;
pub mod context;
pub mod event;
pub mod handoff;
pub mod ids;
pub mod project;
pub mod session;
pub mod task;
pub mod tool;

pub use agent::{Agent, AgentStatus, FileLock};
pub use autonomy::{
    ApprovalAnswer, ApprovalRequest, AutonomyMode, Decision, PolicyRule, RuleAccess, RuleWhere,
};
pub use context::{ContextSectionSummary, ContextSummary};
pub use event::{
    AuditEvent, EventKind, EventSink, MemorySink, NullSink, OutputStream, StreamEvent,
};
pub use handoff::{Handoff, HandoffEnd, HandoffPacket, HandoffStatus};
pub use ids::{
    AgentId, ApprovalId, DeliberationId, EventId, HandoffId, ProcessId, ProviderId, SessionId,
    TaskId, TerminalId, ToolCallId, TurnId,
};
pub use project::{
    DockerInfo, GitRemote, GitSummary, ProjectCandidate, ProjectProfile, RuntimeRequirement,
};
pub use session::{
    NoticeLevel, SessionEvent, SessionInfo, SessionLogEntry, SessionStatus, TokenUsage, TurnStatus,
};
pub use task::{Task, TaskInput, TaskPriority, TaskStatus};
pub use tool::{
    CallOrigin, ToolCall, ToolDefinition, ToolError, ToolErrorKind, ToolResult, ToolSpec,
};
