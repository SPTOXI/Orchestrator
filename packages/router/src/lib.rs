//! Orchestrator model router and Council (ADR-0011).
//!
//! - [`rank`]: the router. Scores every registered model for a task by
//!   rules (activity tags, price, context, tools, quality and speed hints),
//!   without spending tokens, and explains each score.
//! - The Council (ADR-0024): 1 to 5 AI models (provider + model) analyze
//!   a demand together through [`AIProvider::complete`] (no session, no
//!   tools), with the project context; one of them joins the analyses into
//!   a plan, and the first member available carries it out with the others
//!   as reserves. Models outside the Council are never used.
//! - [`RouterService`]: settings (`council.json`), availability, cache,
//!   history, `COUNCIL_*` / `ROUTE_DECIDED` events and the sessions that
//!   carry out a demand — on the user's approval (mode Sugerir) or on its
//!   own (mode Full).
//!
//! [`AIProvider::complete`]: orchestrator_providers::AIProvider::complete

mod activity;
mod cache;
mod catalog;
mod council;
mod score;
mod service;
mod settings;
mod store;

pub use activity::{detect, normalize, profiles, Activity, ActivityProfile};
pub use catalog::{Availability, CatalogModel};
pub use council::{Analysis, Decision, DecisionSource, Deliberation, Plan, PlanSource, Seat, Vote};
pub use score::{
    rank, Candidate, Criteria, Excluded, ModelRef, Preference, Recommendation, RouteRequest,
};
pub use service::{DeliberateRequest, RouteStart, RouteStarted, RouterService, RunOutcome};
pub use settings::{CouncilMember, CouncilMode, CouncilSettings, MAX_MEMBERS};
pub use store::DeliberationStore;
