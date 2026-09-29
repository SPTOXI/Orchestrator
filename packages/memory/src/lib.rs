//! Orchestrator local database and project memory (ADR-0012).
//!
//! One SQLite database per installation (`<app-data>/orchestrator.db`),
//! with everything that belongs to a project keyed by `project_id`:
//!
//! - **history**: every `AuditEvent`, tagged with its project and session
//!   ([`MemoryStore::record`], [`MemoryStore::history`]);
//! - **projects** opened by the app, with the detected stack;
//! - **sessions** and their transcripts, including the provider state
//!   needed to resume them after a restart;
//! - **L1** working memory, derived from the history;
//! - **L2** project memory entries and **decisions**;
//! - **L3** full-text search over memory, decisions, session messages and
//!   notable events;
//! - **Council deliberations**, which also serve as its cache;
//! - **handoffs** between AIs and the facts of a session they are built
//!   from (ADR-0013).
//!
//! Depends only on `orchestrator-core`: the other crates see it through
//! their own traits, wired by the app.

mod db;
mod deliberations;
mod handoffs;
mod model;
mod notes;
mod search;
mod sessions;
mod store;
mod working;

pub use db::SCHEMA_VERSION;
pub use model::{
    Decision, DecisionInput, DecisionStatus, HistoryPage, HistoryQuery, MemoryEntry, MemoryInput,
    MemoryKind, MemoryOverview, Project, RecentImport, SearchHit, SessionFacts, Source,
    StoredSession, WorkingCommand, WorkingError, WorkingFile, WorkingMemory, WorkingSession,
};
pub use store::MemoryStore;
