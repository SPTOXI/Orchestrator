//! Orchestrator engine (ADR-0013): what turns the project's memory into
//! work for the AIs.
//!
//! - [`ContextBuilder`]: the context of each session — task, working
//!   memory, relevant project memory, files, errors, history, Git state and
//!   handoff — within a token budget; installed in the `SessionManager` as
//!   its `ContextSource` (first turn of every session).
//! - [`EngineTools`]: the app's tools plus the memory tools for AI agents
//!   (`memory.*`, `decision.*`).
//! - [`HandoffService`]: one AI hands its work to another through a
//!   `HandoffPacket`, never the conversation.
//! - [`TaskService`]: the project's tasks — states, dependencies and the
//!   session that works on one (ADR-0014).
//!
//! Depends on `core`, `providers`, `memory` and `git`; the app wires it.

mod builder;
mod handoff;
pub mod packet;
mod persistence;
mod settings;
mod task;
mod text;
mod tools;

pub use builder::{BuildRequest, ContextBuilder, ContextPack, ContextSection, SectionKind};
pub use handoff::{
    CreateRequest, HandoffDraft, HandoffService, PrepareRequest, PreviewRequest, StartHandoff,
    StartedHandoff,
};
pub use persistence::StoreSessions;
pub use settings::{ContextSettings, DEFAULT_BUDGET, MAX_BUDGET, MIN_BUDGET};
pub use task::{
    as_task_text, next_states, OpenedTask, StartTaskSession, StartedTask, SubtaskProgress, TaskRef,
    TaskService, TaskView, MAX_DEPENDENCIES, MAX_FILES, MAX_TEXT, MAX_TITLE,
};
pub use text::{clip, estimate_tokens};
pub use tools::{definitions as memory_tool_definitions, EngineTools};
