//! `git.*` tools: arguments and error mapping over `orchestrator-git`
//! (ADR-0007). Every tool takes an optional `path` (a folder inside the
//! repository); the default is the open project.

use orchestrator_core::{ToolError, ToolErrorKind};
use orchestrator_git::{CommandOutput, GitError, PullMode, Remote, ResetMode, StashEntry, Status};
use serde::{Deserialize, Serialize};

/// Default cap of `git.diff` patch text.
pub const DEFAULT_DIFF_BYTES: usize = 1024 * 1024;
pub const DEFAULT_LOG_LIMIT: u32 = 30;
pub const MAX_LOG_LIMIT: u32 = 1000;

pub fn map_error(err: GitError) -> ToolError {
    let kind = match &err {
        GitError::NotInstalled => ToolErrorKind::Spawn,
        GitError::NotARepository(_) => ToolErrorKind::NotFound,
        GitError::Failed { .. } | GitError::TimedOut { .. } => ToolErrorKind::CommandFailed,
        GitError::Invalid(_) => ToolErrorKind::InvalidArgs,
        GitError::Io(_) => ToolErrorKind::Io,
    };
    ToolError::new(kind, err.to_string())
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StatusArgs {
    #[serde(default)]
    pub path: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DiffArgs {
    #[serde(default)]
    pub path: Option<String>,
    /// Staged changes (index vs HEAD) instead of unstaged.
    #[serde(default)]
    pub staged: bool,
    /// Compare against a revision (`HEAD~1`, `main`, …).
    #[serde(default)]
    pub target: Option<String>,
    /// Limit to these files (relative to the repository root).
    #[serde(default)]
    pub files: Vec<String>,
    #[serde(default)]
    pub context_lines: Option<u32>,
    #[serde(default)]
    pub max_bytes: Option<usize>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LogArgs {
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub limit: Option<u32>,
    /// Revision to start from (default HEAD).
    #[serde(default, rename = "ref")]
    pub reference: Option<String>,
    /// Only commits touching this file.
    #[serde(default)]
    pub file: Option<String>,
}

/// `git.branch`: lists branches; optionally creates or deletes one first.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BranchArgs {
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub create: Option<String>,
    #[serde(default)]
    pub start_point: Option<String>,
    #[serde(default)]
    pub delete: Option<String>,
    /// Delete even if not merged.
    #[serde(default)]
    pub force: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CheckoutArgs {
    #[serde(default)]
    pub path: Option<String>,
    /// Branch, tag or commit.
    pub target: String,
    /// Create `target` as a new branch.
    #[serde(default)]
    pub create: bool,
    #[serde(default)]
    pub start_point: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AddArgs {
    #[serde(default)]
    pub path: Option<String>,
    /// Files to stage (relative to the repository root or absolute).
    #[serde(default)]
    pub files: Vec<String>,
    /// Stage everything, including deletions and untracked files.
    #[serde(default)]
    pub all: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CommitArgs {
    #[serde(default)]
    pub path: Option<String>,
    pub message: String,
    /// Stage modified/deleted tracked files first (`--all`).
    #[serde(default)]
    pub all: bool,
    #[serde(default)]
    pub amend: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PullArgs {
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub remote: Option<String>,
    #[serde(default)]
    pub branch: Option<String>,
    /// `merge`, `rebase` or `ffOnly` (default: the user's git config).
    #[serde(default)]
    pub mode: Option<PullMode>,
    #[serde(default)]
    pub timeout_ms: Option<u64>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PushArgs {
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub remote: Option<String>,
    #[serde(default)]
    pub branch: Option<String>,
    #[serde(default)]
    pub set_upstream: bool,
    /// `--force-with-lease`.
    #[serde(default)]
    pub force: bool,
    #[serde(default)]
    pub timeout_ms: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum StashAction {
    Push,
    Pop,
    Apply,
    Drop,
    List,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StashArgs {
    #[serde(default)]
    pub path: Option<String>,
    pub action: StashAction,
    #[serde(default)]
    pub message: Option<String>,
    #[serde(default)]
    pub include_untracked: bool,
    /// Entry for pop/apply/drop (default 0).
    #[serde(default)]
    pub index: u32,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ResetArgs {
    #[serde(default)]
    pub path: Option<String>,
    /// `soft`, `mixed` (default) or `hard`.
    #[serde(default)]
    pub mode: Option<ResetMode>,
    #[serde(default)]
    pub target: Option<String>,
    /// Unstage only these files (mode must be `mixed`).
    #[serde(default)]
    pub files: Vec<String>,
}

/// `git.status`: status plus remotes.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StatusOutput {
    #[serde(flatten)]
    pub status: Status,
    pub remotes: Vec<Remote>,
}

/// Output of operations that change the repository, with the status after.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChangedOutput {
    pub output: CommandOutput,
    pub status: Status,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StashOutput {
    pub output: Option<CommandOutput>,
    pub stashes: Vec<StashEntry>,
}
