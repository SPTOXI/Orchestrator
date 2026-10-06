//! Data returned by Git operations (serialized to the UI and agents).

use serde::{Deserialize, Serialize};

/// Kind of change of a file, in the index (staged) or in the worktree.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ChangeKind {
    Modified,
    Added,
    Deleted,
    Renamed,
    Copied,
    TypeChanged,
    Untracked,
    Conflicted,
}

impl ChangeKind {
    /// Maps a porcelain status letter (`M`, `A`, `D`, …); `.` means none.
    pub fn from_code(code: char) -> Option<Self> {
        Some(match code {
            'M' => Self::Modified,
            'A' => Self::Added,
            'D' => Self::Deleted,
            'R' => Self::Renamed,
            'C' => Self::Copied,
            'T' => Self::TypeChanged,
            'U' => Self::Conflicted,
            '?' => Self::Untracked,
            _ => return None,
        })
    }
}

/// One changed path in `git status`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileChange {
    /// Path relative to the repository root, `/`-separated.
    pub path: String,
    /// Source path of a rename/copy.
    pub original_path: Option<String>,
    /// Change recorded in the index (what the next commit contains).
    pub staged: Option<ChangeKind>,
    /// Change in the working tree not yet staged.
    pub unstaged: Option<ChangeKind>,
    /// Unresolved merge conflict.
    pub conflicted: bool,
}

/// Result of `git status`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub root: String,
    /// Current branch; `None` when HEAD is detached.
    pub branch: Option<String>,
    /// Full commit id of HEAD; `None` before the first commit.
    pub head: Option<String>,
    pub detached: bool,
    pub upstream: Option<String>,
    pub ahead: u32,
    pub behind: u32,
    pub files: Vec<FileChange>,
    pub clean: bool,
}

impl Status {
    pub fn count(&self, predicate: impl Fn(&FileChange) -> bool) -> u32 {
        self.files.iter().filter(|f| predicate(f)).count() as u32
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Commit {
    pub hash: String,
    pub short_hash: String,
    pub parents: Vec<String>,
    pub author: String,
    pub email: String,
    /// Author date, ISO 8601.
    pub date: String,
    pub subject: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Branch {
    /// Short name (`main`, `origin/main`).
    pub name: String,
    pub remote: bool,
    pub current: bool,
    pub upstream: Option<String>,
    /// Abbreviated commit id of the branch tip.
    pub commit: String,
    /// Committer date of the tip, ISO 8601.
    pub date: String,
    pub subject: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Remote {
    pub name: String,
    pub url: String,
}

/// Per-file line counts of a diff.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiffFile {
    pub path: String,
    pub original_path: Option<String>,
    /// `None` for binary files.
    pub additions: Option<u32>,
    pub deletions: Option<u32>,
    pub binary: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Diff {
    /// Unified diff text.
    pub patch: String,
    pub files: Vec<DiffFile>,
    /// True when `patch` was cut at the requested size.
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StashEntry {
    /// `stash@{0}`.
    pub reference: String,
    pub index: u32,
    pub message: String,
    pub date: String,
}

/// Output of a Git command that changes state (commit, pull, push, …).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandOutput {
    pub stdout: String,
    pub stderr: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommitResult {
    pub hash: String,
    pub short_hash: String,
    pub branch: Option<String>,
    pub subject: String,
    pub output: CommandOutput,
}
