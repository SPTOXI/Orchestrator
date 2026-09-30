//! Local Git for the Orchestrator (ADR-0007).
//!
//! Drives the `git` executable installed on the system, so the user's
//! configuration, credential helpers, SSH keys and hooks apply exactly as in
//! their own terminal. Output is read from machine-readable formats.
//!
//! Every invocation sets `GIT_TERMINAL_PROMPT=0` (a missing credential fails
//! instead of waiting for a terminal that does not exist) and
//! `core.quotepath=false`; queries also set `GIT_OPTIONAL_LOCKS=0` so a
//! background status never competes with the user for `index.lock`.
//!
//! The [`github`] module talks to GitHub over its REST API (ADR-0017).

pub mod github;
pub mod parse;
pub mod types;

pub use types::*;

use std::fmt;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Why a Git operation failed.
#[derive(Debug)]
pub enum GitError {
    /// No `git` executable on PATH.
    NotInstalled,
    /// The directory is not inside a Git repository.
    NotARepository(PathBuf),
    /// Git ran and exited with an error.
    Failed {
        command: String,
        code: Option<i32>,
        stdout: String,
        stderr: String,
    },
    /// Git did not finish within the requested time and was killed.
    TimedOut {
        command: String,
        after: Duration,
    },
    /// Invalid request (e.g. empty commit message).
    Invalid(String),
    Io(io::Error),
}

impl fmt::Display for GitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotInstalled => write!(f, "git is not installed or not on PATH"),
            Self::NotARepository(path) => {
                write!(f, "not a git repository: {}", path.display())
            }
            Self::Failed {
                command,
                code,
                stdout,
                stderr,
            } => {
                let code = code.map_or_else(|| "signal".to_owned(), |c| c.to_string());
                write!(f, "`{command}` failed (exit {code})")?;
                for text in [stderr.trim(), stdout.trim()] {
                    if !text.is_empty() {
                        write!(f, ": {text}")?;
                    }
                }
                Ok(())
            }
            Self::TimedOut { command, after } => {
                write!(f, "`{command}` timed out after {} ms", after.as_millis())
            }
            Self::Invalid(message) => f.write_str(message),
            Self::Io(err) => write!(f, "git I/O error: {err}"),
        }
    }
}

impl std::error::Error for GitError {}

pub type Result<T> = std::result::Result<T, GitError>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Access {
    /// Only reads repository state.
    Query,
    /// May change the repository or working tree.
    Mutate,
}

struct Output {
    code: Option<i32>,
    stdout: String,
    stderr: String,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize, serde::Serialize, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub enum ResetMode {
    Soft,
    Mixed,
    Hard,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize, serde::Serialize, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub enum PullMode {
    Merge,
    Rebase,
    FfOnly,
}

#[derive(Debug, Clone, Default)]
pub struct DiffOptions {
    /// Compare the index with HEAD instead of the worktree with the index.
    pub staged: bool,
    /// Compare against this revision (e.g. `HEAD~1`, `main`).
    pub target: Option<String>,
    pub paths: Vec<String>,
    pub context_lines: Option<u32>,
    /// Cut the patch text at this many bytes.
    pub max_bytes: Option<usize>,
}

#[derive(Debug, Clone, Default)]
pub struct PushOptions {
    pub remote: Option<String>,
    pub branch: Option<String>,
    pub set_upstream: bool,
    /// `--force-with-lease`.
    pub force: bool,
    pub timeout: Option<Duration>,
}

#[derive(Debug, Clone, Default)]
pub struct PullOptions {
    pub remote: Option<String>,
    pub branch: Option<String>,
    pub mode: Option<PullMode>,
    pub timeout: Option<Duration>,
}

/// Handle to the system `git` executable.
#[derive(Debug, Clone)]
pub struct Git {
    program: PathBuf,
}

impl Git {
    /// Finds `git` on PATH.
    pub fn detect() -> Result<Self> {
        which::which("git")
            .map(|program| Self { program })
            .map_err(|_| GitError::NotInstalled)
    }

    pub fn program(&self) -> &Path {
        &self.program
    }

    pub fn version(&self) -> Result<String> {
        let out = self.run_ok(None, &["--version"], Access::Query, None)?;
        Ok(out.stdout.trim().to_owned())
    }

    /// Root of the repository containing `dir`.
    pub fn repo_root(&self, dir: &Path) -> Result<PathBuf> {
        let out = self.run_ok(
            Some(dir),
            &["rev-parse", "--show-toplevel"],
            Access::Query,
            None,
        )?;
        Ok(PathBuf::from(out.stdout.trim()))
    }

    pub fn status(&self, dir: &Path) -> Result<Status> {
        let root = self.repo_root(dir)?;
        let out = self.run_ok(
            Some(dir),
            &[
                "status",
                "--porcelain=v2",
                "--branch",
                "-z",
                "--untracked-files=all",
            ],
            Access::Query,
            None,
        )?;
        Ok(parse::status(&out.stdout, &root.display().to_string()))
    }

    pub fn remotes(&self, dir: &Path) -> Result<Vec<Remote>> {
        let out = self.run_ok(Some(dir), &["remote", "-v"], Access::Query, None)?;
        Ok(parse::remotes(&out.stdout))
    }

    /// Recent commits of `reference` (default HEAD), newest first. An empty
    /// repository has no commits: returns an empty list.
    pub fn log(
        &self,
        dir: &Path,
        limit: u32,
        reference: Option<&str>,
        path: Option<&str>,
    ) -> Result<Vec<Commit>> {
        if reference.is_none() && !self.has_commits(dir)? {
            return Ok(Vec::new());
        }
        let format = format!("--format={}", parse::LOG_FORMAT);
        let count = format!("-n{}", limit.max(1));
        let mut args = vec!["log", count.as_str(), format.as_str()];
        if let Some(reference) = reference {
            check_ref(reference)?;
            args.push(reference);
        }
        args.push("--");
        if let Some(path) = path {
            args.push(path);
        }
        let out = self.run_ok(Some(dir), &args, Access::Query, None)?;
        Ok(parse::log(&out.stdout))
    }

    pub fn has_commits(&self, dir: &Path) -> Result<bool> {
        let out = self.run(
            Some(dir),
            &["rev-parse", "--verify", "--quiet", "HEAD"],
            Access::Query,
            None,
        )?;
        if out.code == Some(0) {
            return Ok(true);
        }
        // Distinguish "no commits yet" from "not a repository".
        self.repo_root(dir)?;
        Ok(false)
    }

    pub fn diff(&self, dir: &Path, options: &DiffOptions) -> Result<Diff> {
        let context = options.context_lines.map(|n| format!("-U{n}"));
        let mut common: Vec<&str> = Vec::new();
        if options.staged {
            common.push("--cached");
        }
        if let Some(target) = options.target.as_deref() {
            check_ref(target)?;
            common.push(target);
        }
        let mut tail: Vec<&str> = vec!["--"];
        tail.extend(options.paths.iter().map(String::as_str));

        let mut patch_args = vec!["diff", "--no-color", "--no-ext-diff"];
        if let Some(context) = context.as_deref() {
            patch_args.push(context);
        }
        patch_args.extend(&common);
        patch_args.extend(&tail);
        let patch = self
            .run_ok(Some(dir), &patch_args, Access::Query, None)?
            .stdout;

        let mut stat_args = vec!["diff", "--numstat", "-z", "--no-ext-diff"];
        stat_args.extend(&common);
        stat_args.extend(&tail);
        let stats = self
            .run_ok(Some(dir), &stat_args, Access::Query, None)?
            .stdout;

        let (patch, truncated) = match options.max_bytes {
            Some(max) if patch.len() > max => {
                let mut end = max;
                while !patch.is_char_boundary(end) {
                    end -= 1;
                }
                (patch[..end].to_owned(), true)
            }
            _ => (patch, false),
        };
        Ok(Diff {
            patch,
            files: parse::numstat(&stats),
            truncated,
        })
    }

    /// Local and remote-tracking branches.
    pub fn branches(&self, dir: &Path) -> Result<Vec<Branch>> {
        let format = format!("--format={}", parse::BRANCH_FORMAT);
        let out = self.run_ok(
            Some(dir),
            &["for-each-ref", &format, "refs/heads", "refs/remotes"],
            Access::Query,
            None,
        )?;
        Ok(parse::branches(&out.stdout))
    }

    pub fn create_branch(&self, dir: &Path, name: &str, start: Option<&str>) -> Result<()> {
        check_ref(name)?;
        let mut args = vec!["branch", name];
        if let Some(start) = start {
            check_ref(start)?;
            args.push(start);
        }
        self.run_ok(Some(dir), &args, Access::Mutate, None)
            .map(|_| ())
    }

    pub fn delete_branch(&self, dir: &Path, name: &str, force: bool) -> Result<()> {
        check_ref(name)?;
        let flag = if force { "-D" } else { "-d" };
        self.run_ok(Some(dir), &["branch", flag, name], Access::Mutate, None)
            .map(|_| ())
    }

    /// Switches to `target` (branch, tag or commit); with `create`, creates
    /// the branch first (optionally from `start`).
    pub fn checkout(
        &self,
        dir: &Path,
        target: &str,
        create: bool,
        start: Option<&str>,
    ) -> Result<CommandOutput> {
        check_ref(target)?;
        let mut args = vec!["checkout"];
        if create {
            args.push("-b");
        }
        args.push(target);
        if let Some(start) = start {
            check_ref(start)?;
            args.push(start);
        }
        self.run_ok(Some(dir), &args, Access::Mutate, None)
            .map(Output::into_command_output)
    }

    /// Stages `paths`, or everything (including deletions) with `all`.
    pub fn add(&self, dir: &Path, paths: &[String], all: bool) -> Result<()> {
        if all {
            return self
                .run_ok(Some(dir), &["add", "--all"], Access::Mutate, None)
                .map(|_| ());
        }
        if paths.is_empty() {
            return Err(GitError::Invalid(
                "nothing to add: pass paths or all".into(),
            ));
        }
        let mut args = vec!["add", "--"];
        args.extend(paths.iter().map(String::as_str));
        self.run_ok(Some(dir), &args, Access::Mutate, None)
            .map(|_| ())
    }

    pub fn commit(
        &self,
        dir: &Path,
        message: &str,
        all: bool,
        amend: bool,
    ) -> Result<CommitResult> {
        if message.trim().is_empty() {
            return Err(GitError::Invalid("commit message must not be empty".into()));
        }
        let mut args = vec!["commit", "-m", message];
        if all {
            args.push("--all");
        }
        if amend {
            args.push("--amend");
        }
        let output = self
            .run_ok(Some(dir), &args, Access::Mutate, None)?
            .into_command_output();
        let head = self.log(dir, 1, None, None)?;
        let commit = head
            .into_iter()
            .next()
            .ok_or_else(|| GitError::Invalid("commit succeeded but HEAD is empty".into()))?;
        let branch = self.status(dir)?.branch;
        Ok(CommitResult {
            hash: commit.hash,
            short_hash: commit.short_hash,
            branch,
            subject: commit.subject,
            output,
        })
    }

    pub fn pull(&self, dir: &Path, options: &PullOptions) -> Result<CommandOutput> {
        let mut args = vec!["pull"];
        match options.mode {
            Some(PullMode::Merge) => args.push("--no-rebase"),
            Some(PullMode::Rebase) => args.push("--rebase"),
            Some(PullMode::FfOnly) => args.push("--ff-only"),
            None => {}
        }
        push_remote_args(&mut args, &options.remote, &options.branch)?;
        self.run_ok(Some(dir), &args, Access::Mutate, options.timeout)
            .map(Output::into_command_output)
    }

    /// `git fetch [--prune] [remote]`: updates remote-tracking references
    /// without touching the working tree (ADR-0017).
    pub fn fetch(
        &self,
        dir: &Path,
        remote: Option<&str>,
        prune: bool,
        timeout: Option<Duration>,
    ) -> Result<CommandOutput> {
        let mut args = vec!["fetch"];
        if prune {
            args.push("--prune");
        }
        if let Some(remote) = remote {
            check_ref(remote)?;
            args.push(remote);
        }
        self.run_ok(Some(dir), &args, Access::Mutate, timeout)
            .map(Output::into_command_output)
    }

    pub fn push(&self, dir: &Path, options: &PushOptions) -> Result<CommandOutput> {
        let mut args = vec!["push"];
        if options.set_upstream {
            args.push("--set-upstream");
        }
        if options.force {
            args.push("--force-with-lease");
        }
        push_remote_args(&mut args, &options.remote, &options.branch)?;
        self.run_ok(Some(dir), &args, Access::Mutate, options.timeout)
            .map(Output::into_command_output)
    }

    pub fn stash_push(
        &self,
        dir: &Path,
        message: Option<&str>,
        include_untracked: bool,
    ) -> Result<CommandOutput> {
        let mut args = vec!["stash", "push"];
        if include_untracked {
            args.push("--include-untracked");
        }
        if let Some(message) = message {
            args.extend(["-m", message]);
        }
        self.run_ok(Some(dir), &args, Access::Mutate, None)
            .map(Output::into_command_output)
    }

    /// `pop`, `apply` or `drop` of `stash@{index}`.
    pub fn stash_apply(&self, dir: &Path, action: &str, index: u32) -> Result<CommandOutput> {
        if !matches!(action, "pop" | "apply" | "drop") {
            return Err(GitError::Invalid(format!("unknown stash action: {action}")));
        }
        let reference = format!("stash@{{{index}}}");
        self.run_ok(
            Some(dir),
            &["stash", action, &reference],
            Access::Mutate,
            None,
        )
        .map(Output::into_command_output)
    }

    pub fn stash_list(&self, dir: &Path) -> Result<Vec<StashEntry>> {
        let format = format!("--format={}", parse::STASH_FORMAT);
        let out = self.run_ok(Some(dir), &["stash", "list", &format], Access::Query, None)?;
        Ok(parse::stashes(&out.stdout))
    }

    /// Moves HEAD (`mode`) to `target` (default HEAD), or, with `paths`,
    /// unstages those paths.
    pub fn reset(
        &self,
        dir: &Path,
        mode: ResetMode,
        target: Option<&str>,
        paths: &[String],
    ) -> Result<CommandOutput> {
        if let Some(target) = target {
            check_ref(target)?;
        }
        if !paths.is_empty() {
            if mode != ResetMode::Mixed {
                return Err(GitError::Invalid(
                    "paths can only be reset with mode \"mixed\" (unstage)".into(),
                ));
            }
            // An unborn branch has no HEAD to reset to: remove from the index.
            let mut args = if self.has_commits(dir)? || target.is_some() {
                let mut args = vec!["reset", "-q"];
                args.extend(target);
                args
            } else {
                vec!["rm", "--cached", "-r", "-q"]
            };
            args.push("--");
            args.extend(paths.iter().map(String::as_str));
            return self
                .run_ok(Some(dir), &args, Access::Mutate, None)
                .map(Output::into_command_output);
        }
        let flag = match mode {
            ResetMode::Soft => "--soft",
            ResetMode::Mixed => "--mixed",
            ResetMode::Hard => "--hard",
        };
        let mut args = vec!["reset", flag];
        args.extend(target);
        self.run_ok(Some(dir), &args, Access::Mutate, None)
            .map(Output::into_command_output)
    }

    fn run_ok(
        &self,
        dir: Option<&Path>,
        args: &[&str],
        access: Access,
        timeout: Option<Duration>,
    ) -> Result<Output> {
        let out = self.run(dir, args, access, timeout)?;
        if out.code == Some(0) {
            return Ok(out);
        }
        if out.stderr.contains("not a git repository") {
            return Err(GitError::NotARepository(
                dir.map(Path::to_path_buf).unwrap_or_default(),
            ));
        }
        Err(GitError::Failed {
            command: format!("git {}", args.join(" ")),
            code: out.code,
            stdout: out.stdout,
            stderr: out.stderr,
        })
    }

    fn run(
        &self,
        dir: Option<&Path>,
        args: &[&str],
        access: Access,
        timeout: Option<Duration>,
    ) -> Result<Output> {
        let mut cmd = Command::new(&self.program);
        if let Some(dir) = dir {
            cmd.arg("-C").arg(dir);
        }
        cmd.args(["-c", "core.quotepath=false", "-c", "color.ui=false"])
            .args(args)
            .env("GIT_TERMINAL_PROMPT", "0")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if access == Access::Query {
            cmd.env("GIT_OPTIONAL_LOCKS", "0");
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }

        let mut child = cmd.spawn().map_err(|err| {
            if err.kind() == io::ErrorKind::NotFound {
                GitError::NotInstalled
            } else {
                GitError::Io(err)
            }
        })?;

        // Drain both pipes concurrently so git never blocks on a full pipe.
        let stdout = child.stdout.take().map(read_all);
        let stderr = child.stderr.take().map(read_all);

        let status = match timeout {
            None => child.wait().map_err(GitError::Io)?,
            Some(limit) => {
                let started = Instant::now();
                loop {
                    if let Some(status) = child.try_wait().map_err(GitError::Io)? {
                        break status;
                    }
                    if started.elapsed() >= limit {
                        let _ = child.kill();
                        let _ = child.wait();
                        return Err(GitError::TimedOut {
                            command: format!("git {}", args.join(" ")),
                            after: limit,
                        });
                    }
                    std::thread::sleep(Duration::from_millis(20));
                }
            }
        };

        let collect = |handle: Option<std::thread::JoinHandle<Vec<u8>>>| {
            handle
                .and_then(|h| h.join().ok())
                .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
                .unwrap_or_default()
        };
        Ok(Output {
            code: status.code(),
            stdout: collect(stdout),
            stderr: collect(stderr),
        })
    }
}

impl Output {
    fn into_command_output(self) -> CommandOutput {
        CommandOutput {
            stdout: self.stdout,
            stderr: self.stderr,
        }
    }
}

fn read_all<R: Read + Send + 'static>(mut reader: R) -> std::thread::JoinHandle<Vec<u8>> {
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = reader.read_to_end(&mut bytes);
        bytes
    })
}

/// Rejects revision/branch names that git would parse as options.
fn check_ref(name: &str) -> Result<()> {
    if name.is_empty() || name.starts_with('-') {
        return Err(GitError::Invalid(format!(
            "invalid reference name: {name:?}"
        )));
    }
    Ok(())
}

fn push_remote_args<'a>(
    args: &mut Vec<&'a str>,
    remote: &'a Option<String>,
    branch: &'a Option<String>,
) -> Result<()> {
    match (remote.as_deref(), branch.as_deref()) {
        (Some(remote), branch) => {
            check_ref(remote)?;
            args.push(remote);
            if let Some(branch) = branch {
                check_ref(branch)?;
                args.push(branch);
            }
            Ok(())
        }
        (None, Some(_)) => Err(GitError::Invalid(
            "a branch requires a remote (e.g. remote: \"origin\")".into(),
        )),
        (None, None) => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn option_like_refs_are_rejected() {
        assert!(check_ref("main").is_ok());
        assert!(check_ref("--orphan").is_err());
        assert!(check_ref("").is_err());
    }

    #[test]
    fn branch_without_remote_is_invalid() {
        let mut args = Vec::new();
        let err = push_remote_args(&mut args, &None, &Some("main".into())).unwrap_err();
        assert!(matches!(err, GitError::Invalid(_)));
    }

    #[test]
    fn failed_error_message_includes_output() {
        let err = GitError::Failed {
            command: "git push".into(),
            code: Some(1),
            stdout: String::new(),
            stderr: "rejected\n".into(),
        };
        assert_eq!(err.to_string(), "`git push` failed (exit 1): rejected");
    }
}
