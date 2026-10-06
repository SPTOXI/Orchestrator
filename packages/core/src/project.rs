//! Project contracts (Phase 2): discovery results and the PROJECT PROFILE.
//!
//! Detection itself lives in the Tool Runtime (ADR-0008); these types are
//! what the UI shows and what agents receive as context.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// A folder that looks like a project, found by `project.discover`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectCandidate {
    pub name: String,
    pub path: String,
    /// Marker files that identified it (`.git`, `package.json`, …).
    pub markers: Vec<String>,
    pub is_git_repo: bool,
}

/// A runtime the project needs, with the version it asks for, if declared
/// (`engines.node`, `.nvmrc`, `.python-version`, …).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeRequirement {
    pub name: String,
    pub version: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DockerInfo {
    /// Dockerfiles, relative to the project root.
    pub dockerfiles: Vec<String>,
    /// Compose files, relative to the project root.
    pub compose_files: Vec<String>,
    /// Images referenced by compose files.
    pub images: Vec<String>,
}

impl DockerInfo {
    pub fn is_used(&self) -> bool {
        !self.dockerfiles.is_empty() || !self.compose_files.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitRemote {
    pub name: String,
    pub url: String,
}

/// Git state of a project at profiling time.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitSummary {
    /// Repository root (may be a parent of the project folder).
    pub root: String,
    /// Current branch; `None` when HEAD is detached.
    pub branch: Option<String>,
    /// Abbreviated commit id of HEAD; `None` before the first commit.
    pub head: Option<String>,
    pub upstream: Option<String>,
    pub ahead: u32,
    pub behind: u32,
    pub remotes: Vec<GitRemote>,
    pub staged: u32,
    pub modified: u32,
    pub deleted: u32,
    pub untracked: u32,
    pub conflicted: u32,
    pub clean: bool,
}

/// PROJECT PROFILE (section 7 of the master document). Every conclusion is
/// backed by an entry in `markers`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectProfile {
    pub name: String,
    pub path: String,
    pub git: Option<GitSummary>,
    /// Most relevant first.
    pub languages: Vec<String>,
    pub frameworks: Vec<String>,
    /// Primary first (the one `package.*` tools use).
    pub package_managers: Vec<String>,
    pub runtimes: Vec<RuntimeRequirement>,
    pub docker: DockerInfo,
    pub databases: Vec<String>,
    /// ORMs, test runners and similar tooling (Prisma, Vitest, …).
    pub tools: Vec<String>,
    /// Relevant files that exist, relative to the project root.
    pub important_files: Vec<String>,
    /// `package.json` scripts.
    pub scripts: BTreeMap<String, String>,
    pub monorepo: bool,
    /// Evidence behind the profile (files and dependencies found).
    pub markers: Vec<String>,
    pub detected_at: DateTime<Utc>,
}
