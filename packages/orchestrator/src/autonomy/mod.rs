//! Autonomy (ADR-0016): Assisted, Autonomous and Unrestricted, the
//! requests for the user's authorization and the pause.
//!
//! - [`policy`]: the targets of a call and the rule that decides each one;
//!   the fixed Assisted rules and the default Autonomous ones.
//! - [`describe`]: what a call asks for, in the user's words.
//! - `settings`: `<app-data>/autonomy.json`.
//! - [`AutonomyService`]: modes, rules, requests, session grants, pause.
//! - [`AutonomyGate`]: the outermost executor of the sessions.

pub mod describe;
mod gate;
pub mod policy;
mod service;
mod settings;

pub use gate::AutonomyGate;
pub use policy::{assisted_rules, default_rules, Scope, TargetPath, Unit, Verdict};
pub use service::{
    ApprovalView, AutonomyOverview, AutonomyService, CallContext, SessionGrant, Trial, TrialTarget,
    Workdir,
};
pub use settings::AutonomySettings;
