//! Orchestrator Tool Runtime.
//!
//! The single component that executes operations on the operating system.
//! Every caller — the human through the UI today, AI agents from Phase 3 on —
//! sends a [`ToolCall`] to [`ToolRuntime::invoke`] and receives a
//! [`ToolResult`]. Every call is recorded as a `TOOL_CALLED` audit event, plus
//! domain events (`FILE_CHANGED`, `COMMAND_EXECUTED`, …).
//!
//! The runtime contains no command blocklist and no hidden confirmations.
//! The autonomy gate (Assisted / Autonomous / Unrestricted, ADR-0016) sits
//! in front of [`ToolRuntime::invoke`] on the AIs' path only, in the
//! engine; auditing stays on in every mode.

mod catalog;
pub mod filesystem;
pub mod git_tools;
pub mod output;
pub mod package;
pub mod platform;
pub mod process;
pub mod project;
pub mod schema;
pub mod shell;
pub mod terminal;

pub use catalog::CATALOG;
pub use shell::{ShellInfo, ShellKind, ShellRegistry};

use chrono::Utc;
use orchestrator_core::{
    AuditEvent, CallOrigin, EventKind, EventSink, TerminalId, ToolCall, ToolDefinition, ToolError,
    ToolErrorKind, ToolResult, ToolSpec,
};
use orchestrator_git::{DiffOptions, Git, PullOptions, PushOptions, ResetMode};
use parking_lot::RwLock;
use platform::{resolve_cwd, resolve_path};
use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::{json, Map, Value};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Strings longer than this are shortened in audit event arguments.
const AUDIT_MAX_STRING: usize = 512;
/// Bytes of stdout/stderr kept in `COMMAND_EXECUTED` events.
const AUDIT_OUTPUT_TAIL: usize = 4 * 1024;

/// Runtime configuration.
#[derive(Debug, Clone)]
pub struct RuntimeConfig {
    /// Directory used to resolve relative paths and as default working
    /// directory. `project.open` replaces it with the project root.
    pub base_dir: PathBuf,
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        let base_dir = platform::home_dir()
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_else(|| PathBuf::from("."));
        Self { base_dir }
    }
}

struct Inner {
    /// Open project (or the home directory before one is opened).
    base_dir: RwLock<PathBuf>,
    /// System git; `None` when not installed (git tools then fail with SPAWN).
    git: Option<Git>,
    sink: Arc<dyn EventSink>,
    shells: ShellRegistry,
    terminals: terminal::TerminalManager,
    processes: process::ProcessManager,
}

/// Executes tool calls. Cheap to clone (shared state).
#[derive(Clone)]
pub struct ToolRuntime {
    inner: Arc<Inner>,
}

/// Successful dispatch: tool output plus domain events to record.
struct Dispatched {
    output: Value,
    events: Vec<AuditEvent>,
}

impl Dispatched {
    fn new<T: Serialize>(output: &T) -> Result<Self, ToolError> {
        Self::with_events(output, Vec::new())
    }

    fn with_events<T: Serialize>(output: &T, events: Vec<AuditEvent>) -> Result<Self, ToolError> {
        let output = serde_json::to_value(output)
            .map_err(|e| ToolError::internal(format!("cannot serialize tool output: {e}")))?;
        Ok(Self { output, events })
    }
}

impl ToolRuntime {
    /// Creates a runtime with the shells detected on this machine.
    pub fn new(config: RuntimeConfig, sink: Arc<dyn EventSink>) -> Self {
        Self::with_shells(config, sink, ShellRegistry::detect())
    }

    pub fn with_shells(
        config: RuntimeConfig,
        sink: Arc<dyn EventSink>,
        shells: ShellRegistry,
    ) -> Self {
        Self {
            inner: Arc::new(Inner {
                base_dir: RwLock::new(config.base_dir),
                git: Git::detect().ok(),
                terminals: terminal::TerminalManager::new(sink.clone()),
                processes: process::ProcessManager::new(sink.clone()),
                sink,
                shells,
            }),
        }
    }

    pub fn catalog() -> &'static [ToolSpec] {
        CATALOG
    }

    /// Catalog plus the JSON Schema of each tool's arguments, as offered to
    /// AI models (ADR-0010).
    pub fn definitions() -> &'static [ToolDefinition] {
        schema::definitions()
    }

    /// Current base directory: the open project, or the initial directory.
    pub fn base_dir(&self) -> PathBuf {
        self.inner.base_dir.read().clone()
    }

    fn git(&self) -> Result<Git, ToolError> {
        self.inner.git.clone().ok_or_else(|| {
            ToolError::new(ToolErrorKind::Spawn, "git is not installed or not on PATH")
        })
    }

    pub fn shells(&self) -> &ShellRegistry {
        &self.inner.shells
    }

    /// Executes one tool call and records it. Never panics on bad input:
    /// failures come back as `ok: false` with a [`ToolError`].
    pub async fn invoke(&self, call: ToolCall) -> ToolResult {
        let started_at = Utc::now();
        let clock = Instant::now();
        let outcome = self.dispatch(&call).await;
        let duration_ms = clock.elapsed().as_millis() as u64;
        let finished_at = Utc::now();

        let read_only = CATALOG
            .iter()
            .find(|spec| spec.name == call.tool)
            .map(|spec| spec.read_only);
        let (ok, output, error, events) = match outcome {
            Ok(done) => (true, done.output, None, done.events),
            Err(err) => (false, Value::Null, Some(err), Vec::new()),
        };

        self.inner.sink.audit(
            AuditEvent::new(
                EventKind::ToolCalled,
                call.origin.clone(),
                tool_summary(&call, error.as_ref()),
                json!({
                    "tool": call.tool,
                    "readOnly": read_only,
                    "args": summarize_value(&call.args),
                    "ok": ok,
                    "error": error,
                    "durationMs": duration_ms,
                }),
            )
            .with_call(call.id.clone()),
        );
        for event in events {
            self.inner.sink.audit(event.with_call(call.id.clone()));
        }

        ToolResult {
            call_id: call.id,
            tool: call.tool,
            ok,
            output,
            error,
            started_at,
            finished_at,
            duration_ms,
        }
    }

    async fn dispatch(&self, call: &ToolCall) -> Result<Dispatched, ToolError> {
        let inner = &self.inner;
        let base_dir = inner.base_dir.read().clone();
        let base = base_dir.as_path();
        let origin = &call.origin;
        let file_changed = |summary: String, data: Value| {
            AuditEvent::new(EventKind::FileChanged, origin.clone(), summary, data)
        };

        match call.tool.as_str() {
            "filesystem.list" => {
                let args: filesystem::ListArgs = parse(&call.args)?;
                let path = resolve_path(base, &args.path)?;
                Dispatched::new(&blocking(move || filesystem::list(&path)).await?)
            }
            "filesystem.read" => {
                let args: filesystem::ReadArgs = parse(&call.args)?;
                let path = resolve_path(base, &args.path)?;
                let out = blocking(move || filesystem::read(&path, args.encoding, args.max_bytes))
                    .await?;
                Dispatched::new(&out)
            }
            "filesystem.write" => {
                let args: filesystem::WriteArgs = parse(&call.args)?;
                let path = resolve_path(base, &args.path)?;
                let out = blocking(move || filesystem::write(&path, &args)).await?;
                let change = if out.created { "created" } else { "modified" };
                let event = file_changed(
                    format!("{change} {}", out.path),
                    json!({"change": change, "path": out.path, "bytes": out.bytes_written}),
                );
                Dispatched::with_events(&out, vec![event])
            }
            "filesystem.move" => {
                let args: filesystem::MoveArgs = parse(&call.args)?;
                let from = resolve_path(base, &args.from)?;
                let to = resolve_path(base, &args.to)?;
                let out =
                    blocking(move || filesystem::move_path(&from, &to, args.overwrite)).await?;
                let event = file_changed(
                    format!("moved {} -> {}", out.from, out.to),
                    json!({"change": "moved", "from": out.from, "to": out.to, "replaced": out.replaced}),
                );
                Dispatched::with_events(&out, vec![event])
            }
            "filesystem.delete" => {
                let args: filesystem::DeleteArgs = parse(&call.args)?;
                let path = resolve_path(base, &args.path)?;
                let out = blocking(move || filesystem::delete(&path, args.recursive)).await?;
                let event = file_changed(
                    format!("deleted {}", out.path),
                    json!({"change": "deleted", "path": out.path, "kind": out.kind}),
                );
                Dispatched::with_events(&out, vec![event])
            }

            "shell.list" => {
                parse::<Empty>(&call.args)?;
                Dispatched::new(&json!({
                    "default": inner.shells.default_id(),
                    "shells": inner.shells.list(),
                }))
            }
            "shell.execute" => {
                let args: shell::ExecuteArgs = parse(&call.args)?;
                let out = shell::execute(&inner.shells, base, args).await?;
                let event = command_executed(origin, &out);
                Dispatched::with_events(&out, vec![event])
            }

            "terminal.create" => {
                let args: terminal::CreateArgs = parse(&call.args)?;
                let out = inner
                    .terminals
                    .create(&inner.shells, base, args, origin.clone())?;
                Dispatched::new(&out)
            }
            "terminal.write" => {
                let args: terminal::WriteArgs = parse(&call.args)?;
                let runtime = self.inner.clone();
                Dispatched::new(&blocking(move || runtime.terminals.write(args)).await?)
            }
            "terminal.read" => {
                let args: terminal::ReadArgs = parse(&call.args)?;
                Dispatched::new(&inner.terminals.read(args)?)
            }
            "terminal.close" => {
                let args: terminal::IdArgs = parse(&call.args)?;
                Dispatched::new(&inner.terminals.close(&args.id)?)
            }
            "terminal.list" => {
                parse::<Empty>(&call.args)?;
                Dispatched::new(&inner.terminals.list())
            }

            "process.start" => {
                let args: process::StartArgs = parse(&call.args)?;
                let out = inner
                    .processes
                    .start(&inner.shells, base, args, origin.clone())
                    .await?;
                let event = process_started(origin, &out);
                Dispatched::with_events(&out, vec![event])
            }
            "process.stop" => {
                let args: process::StopArgs = parse(&call.args)?;
                Dispatched::new(&inner.processes.stop(args).await?)
            }
            "process.list" => {
                parse::<Empty>(&call.args)?;
                Dispatched::new(&inner.processes.list())
            }
            "process.read" => {
                let args: process::ReadArgs = parse(&call.args)?;
                Dispatched::new(&inner.processes.read(args)?)
            }

            "project.discover" => {
                let args: project::DiscoverArgs = parse(&call.args)?;
                let roots = match args.roots {
                    Some(roots) if !roots.is_empty() => roots
                        .iter()
                        .map(|root| resolve_path(base, root))
                        .collect::<Result<Vec<_>, _>>()?,
                    _ => project::default_roots(),
                };
                let depth = args.max_depth.unwrap_or(project::DEFAULT_MAX_DEPTH).min(12);
                let max_dirs = args.max_dirs.unwrap_or(project::DEFAULT_MAX_DIRS);
                let out = blocking(move || Ok(project::discover(&roots, depth, max_dirs))).await?;
                Dispatched::new(&out)
            }
            "project.profile" => {
                let args: project::PathArgs = parse(&call.args)?;
                let dir = resolve_cwd(base, args.path.as_deref())?;
                let git = inner.git.clone();
                Dispatched::new(&blocking(move || project::profile(&dir, git.as_ref())).await?)
            }
            "project.open" => {
                let args: OpenArgs = parse(&call.args)?;
                let dir = resolve_cwd(base, Some(&args.path))?;
                let git = inner.git.clone();
                let profiled = dir.clone();
                let profile = blocking(move || project::profile(&profiled, git.as_ref())).await?;
                *inner.base_dir.write() = dir;
                let event = AuditEvent::new(
                    EventKind::ProjectOpened,
                    origin.clone(),
                    format!("opened project {} ({})", profile.name, profile.path),
                    json!({
                        "name": profile.name,
                        "path": profile.path,
                        "gitRoot": profile.git.as_ref().map(|g| &g.root),
                        "branch": profile.git.as_ref().and_then(|g| g.branch.as_ref()),
                        "languages": profile.languages,
                        "frameworks": profile.frameworks,
                        "packageManagers": profile.package_managers,
                        "runtimes": profile.runtimes,
                        "databases": profile.databases,
                        "tools": profile.tools,
                        "docker": profile.docker,
                        "monorepo": profile.monorepo,
                    }),
                );
                Dispatched::with_events(&profile, vec![event])
            }

            "git.status" => {
                let args: git_tools::StatusArgs = parse(&call.args)?;
                let dir = resolve_cwd(base, args.path.as_deref())?;
                let git = self.git()?;
                let out = blocking(move || {
                    let status = git.status(&dir).map_err(git_tools::map_error)?;
                    let remotes = git.remotes(&dir).map_err(git_tools::map_error)?;
                    Ok(git_tools::StatusOutput { status, remotes })
                })
                .await?;
                Dispatched::new(&out)
            }
            "git.diff" => {
                let args: git_tools::DiffArgs = parse(&call.args)?;
                let dir = resolve_cwd(base, args.path.as_deref())?;
                let git = self.git()?;
                let options = DiffOptions {
                    staged: args.staged,
                    target: args.target,
                    paths: args.files,
                    context_lines: args.context_lines,
                    max_bytes: Some(args.max_bytes.unwrap_or(git_tools::DEFAULT_DIFF_BYTES)),
                };
                let out = blocking(move || git.diff(&dir, &options).map_err(git_tools::map_error))
                    .await?;
                Dispatched::new(&out)
            }
            "git.log" => {
                let args: git_tools::LogArgs = parse(&call.args)?;
                let dir = resolve_cwd(base, args.path.as_deref())?;
                let git = self.git()?;
                let limit = args
                    .limit
                    .unwrap_or(git_tools::DEFAULT_LOG_LIMIT)
                    .clamp(1, git_tools::MAX_LOG_LIMIT);
                let out = blocking(move || {
                    git.log(&dir, limit, args.reference.as_deref(), args.file.as_deref())
                        .map_err(git_tools::map_error)
                })
                .await?;
                Dispatched::new(&out)
            }
            "git.branch" => {
                let args: git_tools::BranchArgs = parse(&call.args)?;
                let dir = resolve_cwd(base, args.path.as_deref())?;
                let git = self.git()?;
                let out = blocking(move || {
                    if let Some(name) = args.create.as_deref() {
                        git.create_branch(&dir, name, args.start_point.as_deref())
                            .map_err(git_tools::map_error)?;
                    }
                    if let Some(name) = args.delete.as_deref() {
                        git.delete_branch(&dir, name, args.force)
                            .map_err(git_tools::map_error)?;
                    }
                    git.branches(&dir).map_err(git_tools::map_error)
                })
                .await?;
                Dispatched::new(&out)
            }
            "git.checkout" => {
                let args: git_tools::CheckoutArgs = parse(&call.args)?;
                let dir = resolve_cwd(base, args.path.as_deref())?;
                let git = self.git()?;
                let out = blocking(move || {
                    let output = git
                        .checkout(&dir, &args.target, args.create, args.start_point.as_deref())
                        .map_err(git_tools::map_error)?;
                    let status = git.status(&dir).map_err(git_tools::map_error)?;
                    Ok(git_tools::ChangedOutput { output, status })
                })
                .await?;
                Dispatched::new(&out)
            }
            "git.add" => {
                let args: git_tools::AddArgs = parse(&call.args)?;
                let dir = resolve_cwd(base, args.path.as_deref())?;
                let git = self.git()?;
                let out = blocking(move || {
                    git.add(&dir, &args.files, args.all)
                        .map_err(git_tools::map_error)?;
                    git.status(&dir).map_err(git_tools::map_error)
                })
                .await?;
                Dispatched::new(&out)
            }
            "git.commit" => {
                let args: git_tools::CommitArgs = parse(&call.args)?;
                let dir = resolve_cwd(base, args.path.as_deref())?;
                let git = self.git()?;
                let repo = dir.display().to_string();
                let out = blocking(move || {
                    git.commit(&dir, &args.message, args.all, args.amend)
                        .map_err(git_tools::map_error)
                })
                .await?;
                let event = AuditEvent::new(
                    EventKind::GitCommit,
                    origin.clone(),
                    format!(
                        "commit {} on {}: {}",
                        out.short_hash,
                        out.branch.as_deref().unwrap_or("(detached)"),
                        out.subject
                    ),
                    json!({
                        "repo": repo,
                        "hash": out.hash,
                        "branch": out.branch,
                        "subject": out.subject,
                    }),
                );
                Dispatched::with_events(&out, vec![event])
            }
            "git.pull" => {
                let args: git_tools::PullArgs = parse(&call.args)?;
                let dir = resolve_cwd(base, args.path.as_deref())?;
                let git = self.git()?;
                let options = PullOptions {
                    remote: args.remote,
                    branch: args.branch,
                    mode: args.mode,
                    timeout: args.timeout_ms.map(Duration::from_millis),
                };
                let out = blocking(move || {
                    let output = git.pull(&dir, &options).map_err(git_tools::map_error)?;
                    let status = git.status(&dir).map_err(git_tools::map_error)?;
                    Ok(git_tools::ChangedOutput { output, status })
                })
                .await?;
                Dispatched::new(&out)
            }
            "git.push" => {
                let args: git_tools::PushArgs = parse(&call.args)?;
                let dir = resolve_cwd(base, args.path.as_deref())?;
                let git = self.git()?;
                let repo = dir.display().to_string();
                let forced = args.force;
                let options = PushOptions {
                    remote: args.remote,
                    branch: args.branch,
                    set_upstream: args.set_upstream,
                    force: args.force,
                    timeout: args.timeout_ms.map(Duration::from_millis),
                };
                let out = blocking(move || {
                    let output = git.push(&dir, &options).map_err(git_tools::map_error)?;
                    let status = git.status(&dir).map_err(git_tools::map_error)?;
                    Ok(git_tools::ChangedOutput { output, status })
                })
                .await?;
                let target = out
                    .status
                    .upstream
                    .clone()
                    .or_else(|| out.status.branch.clone())
                    .unwrap_or_else(|| "(default)".to_owned());
                let event = AuditEvent::new(
                    EventKind::GitPush,
                    origin.clone(),
                    format!(
                        "push {target}{}",
                        if forced { " (force-with-lease)" } else { "" }
                    ),
                    json!({
                        "repo": repo,
                        "branch": out.status.branch,
                        "upstream": out.status.upstream,
                        "forced": forced,
                    }),
                );
                Dispatched::with_events(&out, vec![event])
            }
            "git.stash" => {
                let args: git_tools::StashArgs = parse(&call.args)?;
                let dir = resolve_cwd(base, args.path.as_deref())?;
                let git = self.git()?;
                let out = blocking(move || {
                    use git_tools::StashAction;
                    let output = match args.action {
                        StashAction::List => None,
                        StashAction::Push => Some(git.stash_push(
                            &dir,
                            args.message.as_deref(),
                            args.include_untracked,
                        )),
                        StashAction::Pop => Some(git.stash_apply(&dir, "pop", args.index)),
                        StashAction::Apply => Some(git.stash_apply(&dir, "apply", args.index)),
                        StashAction::Drop => Some(git.stash_apply(&dir, "drop", args.index)),
                    }
                    .transpose()
                    .map_err(git_tools::map_error)?;
                    let stashes = git.stash_list(&dir).map_err(git_tools::map_error)?;
                    Ok(git_tools::StashOutput { output, stashes })
                })
                .await?;
                Dispatched::new(&out)
            }
            "git.reset" => {
                let args: git_tools::ResetArgs = parse(&call.args)?;
                let dir = resolve_cwd(base, args.path.as_deref())?;
                let git = self.git()?;
                let out = blocking(move || {
                    let output = git
                        .reset(
                            &dir,
                            args.mode.unwrap_or(ResetMode::Mixed),
                            args.target.as_deref(),
                            &args.files,
                        )
                        .map_err(git_tools::map_error)?;
                    let status = git.status(&dir).map_err(git_tools::map_error)?;
                    Ok(git_tools::ChangedOutput { output, status })
                })
                .await?;
                Dispatched::new(&out)
            }

            "package.install" => {
                let args: package::InstallArgs = parse(&call.args)?;
                let dir = resolve_cwd(base, args.path.as_deref())?;
                let manager = self
                    .package_manager(&dir, args.manager.as_deref(), package::SUPPORTED_MANAGERS)
                    .await?;
                let words = package::install_command(&manager, &args.packages, args.dev, &dir)?;
                let out = self.run_words(&dir, &words, args.timeout_ms).await?;
                let event = command_executed(origin, &out);
                let output = package::PackageOutput {
                    manager,
                    command: out.command.clone(),
                    result: Some(out),
                    process: None,
                };
                Dispatched::with_events(&output, vec![event])
            }
            "package.run" => {
                let args: package::RunArgs = parse(&call.args)?;
                let dir = resolve_cwd(base, args.path.as_deref())?;
                let runnable: Vec<&str> = package::SUPPORTED_MANAGERS
                    .iter()
                    .copied()
                    .filter(|m| *m != "pip")
                    .collect();
                let manager = self
                    .package_manager(&dir, args.manager.as_deref(), &runnable)
                    .await?;
                let words = package::run_command(&manager, &args.script, &args.args)?;
                if args.background {
                    let shell = inner.shells.resolve(None)?;
                    let command = package::render(shell.kind, &words)?;
                    let started = inner
                        .processes
                        .start(
                            &inner.shells,
                            base,
                            process::StartArgs {
                                command: command.clone(),
                                cwd: Some(dir.display().to_string()),
                                shell: None,
                                env: Default::default(),
                                name: Some(format!("{manager} {}", args.script)),
                            },
                            origin.clone(),
                        )
                        .await?;
                    let event = process_started(origin, &started);
                    let output = package::PackageOutput {
                        manager,
                        command,
                        result: None,
                        process: Some(started),
                    };
                    return Dispatched::with_events(&output, vec![event]);
                }
                let out = self.run_words(&dir, &words, args.timeout_ms).await?;
                let event = command_executed(origin, &out);
                let output = package::PackageOutput {
                    manager,
                    command: out.command.clone(),
                    result: Some(out),
                    process: None,
                };
                Dispatched::with_events(&output, vec![event])
            }

            "runtime.node" => {
                parse::<Empty>(&call.args)?;
                Dispatched::new(&package::node(&inner.shells, base).await)
            }
            "runtime.python" => {
                parse::<Empty>(&call.args)?;
                Dispatched::new(&package::python_runtime(&inner.shells, base).await)
            }
            "runtime.docker" => {
                parse::<Empty>(&call.args)?;
                Dispatched::new(&package::docker(&inner.shells, base).await)
            }

            other => Err(ToolError::new(
                ToolErrorKind::UnknownTool,
                format!("unknown tool: {other}"),
            )),
        }
    }

    /// Package manager for `dir`: explicit, else detected in its profile.
    async fn package_manager(
        &self,
        dir: &Path,
        requested: Option<&str>,
        allowed: &[&str],
    ) -> Result<String, ToolError> {
        let detected = if requested.is_some() {
            Vec::new()
        } else {
            let dir = dir.to_path_buf();
            blocking(move || project::profile(&dir, None))
                .await?
                .package_managers
        };
        package::choose_manager(requested, &detected, allowed)
    }

    /// Runs command words (quoted for the default shell) in `cwd`.
    async fn run_words(
        &self,
        cwd: &Path,
        words: &[String],
        timeout_ms: Option<u64>,
    ) -> Result<shell::ExecuteOutput, ToolError> {
        let shell = self.inner.shells.resolve(None)?;
        let command = package::render(shell.kind, words)?;
        shell::execute(
            &self.inner.shells,
            cwd,
            shell::ExecuteArgs {
                command,
                cwd: Some(cwd.display().to_string()),
                shell: None,
                env: Default::default(),
                timeout_ms,
                stdin: None,
                max_output_bytes: None,
            },
        )
        .await
    }

    /// Streams human keystrokes into a terminal (ADR-0003: not a tool call).
    pub fn terminal_input(&self, id: &TerminalId, data: &[u8]) -> Result<(), ToolError> {
        self.inner.terminals.input(id, data)
    }

    /// Resizes a terminal's PTY (ADR-0003: not a tool call).
    pub fn terminal_resize(&self, id: &TerminalId, cols: u16, rows: u16) -> Result<(), ToolError> {
        self.inner.terminals.resize(id, cols, rows)
    }

    /// Closes every terminal and kills every managed process.
    pub async fn shutdown(&self) {
        self.inner.terminals.shutdown();
        self.inner.processes.shutdown().await;
    }
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct OpenArgs {
    /// Project folder to open (absolute, or relative to the current base directory).
    path: String,
}

/// `COMMAND_EXECUTED` for a command that ran to completion.
fn command_executed(origin: &CallOrigin, out: &shell::ExecuteOutput) -> AuditEvent {
    let exit = match (out.timed_out, out.exit_code) {
        (true, _) => "timed out".to_owned(),
        (false, Some(code)) => format!("exit {code}"),
        (false, None) => "killed".to_owned(),
    };
    AuditEvent::new(
        EventKind::CommandExecuted,
        origin.clone(),
        format!("{} ({exit})", out.command),
        json!({
            "command": out.command,
            "shell": out.shell,
            "cwd": out.cwd,
            "exitCode": out.exit_code,
            "timedOut": out.timed_out,
            "durationMs": out.duration_ms,
            "stdoutTail": tail(&out.stdout, AUDIT_OUTPUT_TAIL),
            "stderrTail": tail(&out.stderr, AUDIT_OUTPUT_TAIL),
        }),
    )
}

/// `COMMAND_EXECUTED` for a command started as a managed process.
fn process_started(origin: &CallOrigin, out: &process::ProcessInfo) -> AuditEvent {
    AuditEvent::new(
        EventKind::CommandExecuted,
        origin.clone(),
        format!("{} (started, pid {})", out.command, fmt_opt(out.pid)),
        json!({
            "command": out.command,
            "shell": out.shell,
            "cwd": out.cwd,
            "processId": out.id,
            "pid": out.pid,
            "background": true,
        }),
    )
}

/// Arguments of tools that take none.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct Empty {}

fn parse<T: DeserializeOwned>(args: &Value) -> Result<T, ToolError> {
    let args = if args.is_null() {
        Value::Object(Map::new())
    } else {
        args.clone()
    };
    serde_json::from_value(args).map_err(|e| ToolError::invalid_args(e.to_string()))
}

async fn blocking<T, F>(work: F) -> Result<T, ToolError>
where
    F: FnOnce() -> Result<T, ToolError> + Send + 'static,
    T: Send + 'static,
{
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|e| ToolError::internal(format!("blocking task failed: {e}")))?
}

fn fmt_opt<T: std::fmt::Display>(value: Option<T>) -> String {
    value.map_or_else(|| "?".to_owned(), |v| v.to_string())
}

/// One-line description of a call for the history panel.
fn tool_summary(call: &ToolCall, error: Option<&ToolError>) -> String {
    let target = ["path", "from", "command", "id"]
        .iter()
        .find_map(|key| call.args.get(key).and_then(Value::as_str))
        .map(|value| format!(" {}", truncate(value, 120)))
        .unwrap_or_default();
    match error {
        None => format!("{}{target}", call.tool),
        Some(err) => format!(
            "{}{target} failed: {}",
            call.tool,
            truncate(&err.message, 200)
        ),
    }
}

/// Copy of `value` where long strings are shortened, so audit logs do not
/// duplicate file contents (ADR-0005).
fn summarize_value(value: &Value) -> Value {
    match value {
        Value::String(s) if s.len() > AUDIT_MAX_STRING => Value::String(format!(
            "{}…(+{} bytes)",
            truncate(s, AUDIT_MAX_STRING / 2),
            s.len()
        )),
        Value::Array(items) => Value::Array(items.iter().map(summarize_value).collect()),
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(k, v)| (k.clone(), summarize_value(v)))
                .collect(),
        ),
        other => other.clone(),
    }
}

/// First `max` bytes of `s`, cut at a character boundary.
fn truncate(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

/// Last `max` bytes of `s`, cut at a character boundary.
fn tail(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut start = s.len() - max;
    while !s.is_char_boundary(start) {
        start += 1;
    }
    &s[start..]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summarize_shortens_long_strings_only() {
        let long = "x".repeat(2000);
        let value = json!({"path": "/a", "content": long, "nested": [long.clone()]});
        let out = summarize_value(&value);
        assert_eq!(out["path"], "/a");
        let content = out["content"].as_str().unwrap();
        assert!(content.ends_with("…(+2000 bytes)"));
        assert!(content.len() < 400);
        assert!(out["nested"][0].as_str().unwrap().ends_with("bytes)"));
    }

    #[test]
    fn truncate_and_tail_respect_char_boundaries() {
        assert_eq!(truncate("aéb", 2), "a");
        assert_eq!(tail("aéb", 2), "b");
        assert_eq!(tail("abc", 10), "abc");
    }

    #[test]
    fn parse_treats_null_as_empty_object_and_rejects_unknown_fields() {
        assert!(parse::<Empty>(&Value::Null).is_ok());
        let err = parse::<Empty>(&json!({"unexpected": 1})).unwrap_err();
        assert_eq!(err.kind, ToolErrorKind::InvalidArgs);
    }
}
