//! `clis.json`: which CLIs are on, and how to run them.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CliSettings {
    /// Registered as a provider.
    pub enabled: bool,
    /// Path of the program, when it is not on the PATH.
    pub program: Option<String>,
    /// Models offered (aliases or ids); empty: the CLI's suggestions.
    pub models: Vec<String>,
    pub default_model: Option<String>,
    /// Let the CLI use its own tools too (they do not go through the
    /// Orchestrator's autonomy gate, locks or history).
    pub own_tools: bool,
    /// Appended to every run.
    pub extra_args: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CliFile {
    #[serde(default)]
    pub clis: BTreeMap<String, CliSettings>,
}
