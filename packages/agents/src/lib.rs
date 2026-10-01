//! Agent Manager, subagents and the File Lock Manager (ADR-0015).
//!
//! - [`AgentService`]: the queue and the life of an agent — one task, one
//!   session, one outcome. It drives the turns, stops at the ceiling, and
//!   leaves a handoff when it stops before finishing.
//! - [`LockManager`]: two agents never change the same file at the same
//!   time (section 13 of the master document). Only agents are locked; the
//!   user is never blocked out of their own project.
//! - [`AgentTools`]: `agent.finish` and `agent.delegate`, plus the lock
//!   check, wrapped around the app's tool executor.
//!
//! Agents are disposable; the task, the memory and the history are not.
//! Depends on `core`, `engine`, `providers` and `memory`; the app wires it.

mod locks;
mod service;
mod settings;
mod tools;

pub use locks::{normalize as normalize_path, LockManager};
pub use service::{
    AgentDeps, AgentService, AgentView, BudgetView, StartAgent, MAX_DEPTH, MAX_RESULT,
};
pub use settings::{
    load as load_settings, save as save_settings, AgentSettings, DEFAULT_PARALLEL,
    DEFAULT_SUBAGENTS, DEFAULT_TURNS, MAX_PARALLEL, MAX_SUBAGENTS, MAX_TURNS,
};
pub use tools::{definitions as agent_tool_definitions, AgentSlot, AgentTools};
