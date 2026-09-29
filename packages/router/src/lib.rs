//! Orchestrator model router and Council (ADR-0011).
//!
//! - [`rank`]: the router. Scores every registered model for a task by
//!   rules (activity tags, price, context, tools, quality and speed hints),
//!   without spending tokens, and explains each score.
//! - The Council: 1 to 5 AI models (provider + model) deliberate over the
//!   router's best candidates through [`AIProvider::complete`] (no session,
//!   no tools) and vote; a weighted Borda count decides.
//! - [`RouterService`]: settings (`council.json`), availability, cache,
//!   history, `COUNCIL_*` / `ROUTE_DECIDED` events and the start of sessions
//!   with the chosen model — on the user's approval (mode Sugerir) or on its
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

pub use activity::{detect, normalize, profiles, Activity, ActivityProfile};
pub use catalog::{Availability, CatalogModel};
pub use council::{parse_ballot, tally, Ballot, Decision, DecisionSource, Deliberation, Vote};
pub use score::{
    rank, Candidate, Criteria, Excluded, ModelRef, Preference, Recommendation, RouteRequest,
};
pub use service::{DeliberateRequest, RouteStart, RouteStarted, RouterService, RunOutcome};
pub use settings::{CouncilMember, CouncilMode, CouncilSettings, MAX_MEMBERS};
