//! Process manager: long-running processes (`npm run dev`, watchers, servers).
//!
//! Tools: `process.start`, `process.stop`, `process.list`, `process.read`.

use crate::output::{OutputBuffer, OutputChunk, Utf8Decoder};
use crate::platform::{configure_process_tree, resolve_cwd, terminate_tree, Termination};
use crate::shell::ShellRegistry;
use chrono::{DateTime, Utc};
use orchestrator_core::{
    AuditEvent, CallOrigin, EventKind, EventSink, OutputStream, ProcessId, StreamEvent, ToolError,
    ToolErrorKind,
};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::HashMap;
use std::path::Path;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::sync::watch;

/// Time a process gets to exit after a graceful stop before it is killed.
const STOP_GRACE: Duration = Duration::from_secs(5);
/// How long to wait for output pipes to close after the process exited.
const PIPE_DRAIN_GRACE: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ProcessStatus {
    Running,
    /// Ended on its own.
    Exited,
    /// Ended because `process.stop` was called.
    Stopped,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcessInfo {
    pub id: ProcessId,
    pub name: String,
    pub command: String,
    pub cwd: String,
    pub shell: String,
    pub pid: Option<u32>,
    pub status: ProcessStatus,
    pub exit_code: Option<i32>,
    pub started_at: DateTime<Utc>,
    pub ended_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StartArgs {
    pub command: String,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub shell: Option<String>,
    #[serde(default)]
    pub env: HashMap<String, String>,
    /// Display name (default: the command).
    #[serde(default)]
    pub name: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StopArgs {
    pub id: ProcessId,
    /// Kill immediately instead of asking the process to terminate first.
    #[serde(default)]
    pub force: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReadArgs {
    pub id: ProcessId,
    #[serde(default)]
    pub since: Option<u64>,
    #[serde(default)]
    pub max_bytes: Option<usize>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcessRead {
    pub id: ProcessId,
    pub status: ProcessStatus,
    pub exit_code: Option<i32>,
    #[serde(flatten)]
    pub chunk: OutputChunk,
}

#[derive(Debug)]
struct ProcessState {
    status: ProcessStatus,
    exit_code: Option<i32>,
    ended_at: Option<DateTime<Utc>>,
    stop_requested: bool,
}

struct ManagedProcess {
    id: ProcessId,
    name: String,
    command: String,
    cwd: String,
    shell: String,
    pid: Option<u32>,
    started_at: DateTime<Utc>,
    origin: CallOrigin,
    state: Mutex<ProcessState>,
    output: Mutex<OutputBuffer>,
    exited: watch::Receiver<bool>,
}

impl ManagedProcess {
    fn info(&self) -> ProcessInfo {
        let state = self.state.lock();
        ProcessInfo {
            id: self.id.clone(),
            name: self.name.clone(),
            command: self.command.clone(),
            cwd: self.cwd.clone(),
            shell: self.shell.clone(),
            pid: self.pid,
            status: state.status,
            exit_code: state.exit_code,
            started_at: self.started_at,
            ended_at: state.ended_at,
        }
    }

    fn is_running(&self) -> bool {
        self.state.lock().status == ProcessStatus::Running
    }

    async fn wait_exit(&self, timeout: Duration) -> bool {
        let mut rx = self.exited.clone();
        tokio::time::timeout(timeout, rx.wait_for(|done| *done))
            .await
            .map(|r| r.is_ok())
            .unwrap_or(false)
    }
}

/// Owns every process started through `process.start`.
pub struct ProcessManager {
    sink: Arc<dyn EventSink>,
    processes: Mutex<HashMap<ProcessId, Arc<ManagedProcess>>>,
}

impl ProcessManager {
    pub fn new(sink: Arc<dyn EventSink>) -> Self {
        Self {
            sink,
            processes: Mutex::new(HashMap::new()),
        }
    }

    fn get(&self, id: &ProcessId) -> Result<Arc<ManagedProcess>, ToolError> {
        self.processes
            .lock()
            .get(id)
            .cloned()
            .ok_or_else(|| ToolError::not_found(format!("unknown process: {id}")))
    }

    pub async fn start(
        &self,
        registry: &ShellRegistry,
        base_dir: &Path,
        args: StartArgs,
        origin: CallOrigin,
    ) -> Result<ProcessInfo, ToolError> {
        if args.command.trim().is_empty() {
            return Err(ToolError::invalid_args("command must not be empty"));
        }
        let shell = registry.resolve(args.shell.as_deref())?;
        let cwd = resolve_cwd(base_dir, args.cwd.as_deref())?;

        let mut cmd = shell.build_command(&args.command);
        cmd.current_dir(&cwd)
            .envs(&args.env)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(false);
        configure_process_tree(&mut cmd);

        let mut child = cmd.spawn().map_err(|e| {
            ToolError::new(
                ToolErrorKind::Spawn,
                format!("failed to start {}: {e}", shell.path.display()),
            )
        })?;

        let (exited_tx, exited_rx) = watch::channel(false);
        let process = Arc::new(ManagedProcess {
            id: ProcessId::new(),
            name: args
                .name
                .filter(|n| !n.trim().is_empty())
                .unwrap_or_else(|| args.command.clone()),
            command: args.command,
            cwd: cwd.display().to_string(),
            shell: shell.id,
            pid: child.id(),
            started_at: Utc::now(),
            origin,
            state: Mutex::new(ProcessState {
                status: ProcessStatus::Running,
                exit_code: None,
                ended_at: None,
                stop_requested: false,
            }),
            output: Mutex::new(OutputBuffer::default()),
            exited: exited_rx,
        });
        self.processes
            .lock()
            .insert(process.id.clone(), process.clone());

        let readers: Vec<_> =
            [
                child.stdout.take().map(|r| {
                    spawn_reader(r, OutputStream::Stdout, process.clone(), self.sink.clone())
                }),
                child.stderr.take().map(|r| {
                    spawn_reader(r, OutputStream::Stderr, process.clone(), self.sink.clone())
                }),
            ]
            .into_iter()
            .flatten()
            .collect();

        let sink = self.sink.clone();
        let watched = process.clone();
        tokio::spawn(async move {
            let status = child.wait().await;
            for reader in readers {
                let abort = reader.abort_handle();
                if tokio::time::timeout(PIPE_DRAIN_GRACE, reader)
                    .await
                    .is_err()
                {
                    abort.abort();
                }
            }
            let exit_code = status.ok().and_then(|s| s.code());
            let stopped = {
                let mut state = watched.state.lock();
                state.status = if state.stop_requested {
                    ProcessStatus::Stopped
                } else {
                    ProcessStatus::Exited
                };
                state.exit_code = exit_code;
                state.ended_at = Some(Utc::now());
                state.stop_requested
            };
            sink.stream(StreamEvent::ProcessExited {
                process_id: watched.id.clone(),
                exit_code,
                stopped,
            });
            let how = exit_code.map_or_else(
                || "terminated by signal".to_owned(),
                |c| format!("exit code {c}"),
            );
            sink.audit(AuditEvent::new(
                EventKind::ProcessExited,
                watched.origin.clone(),
                format!(
                    "process {} {} ({how})",
                    watched.name,
                    if stopped { "stopped" } else { "exited" }
                ),
                json!({
                    "processId": watched.id,
                    "command": watched.command,
                    "exitCode": exit_code,
                    "stopped": stopped,
                }),
            ));
            let _ = exited_tx.send(true);
        });

        Ok(process.info())
    }

    pub async fn stop(&self, args: StopArgs) -> Result<ProcessInfo, ToolError> {
        let process = self.get(&args.id)?;
        if !process.is_running() {
            return Ok(process.info());
        }
        process.state.lock().stop_requested = true;
        let Some(pid) = process.pid else {
            return Err(ToolError::new(
                ToolErrorKind::Internal,
                "process has no pid; cannot stop it",
            ));
        };
        let io = |e: std::io::Error| {
            ToolError::new(
                ToolErrorKind::Io,
                format!("failed to stop process {pid}: {e}"),
            )
        };
        if args.force {
            terminate_tree(pid, Termination::Kill).map_err(io)?;
        } else {
            terminate_tree(pid, Termination::Graceful).map_err(io)?;
            if !process.wait_exit(STOP_GRACE).await {
                terminate_tree(pid, Termination::Kill).map_err(io)?;
            }
        }
        if !process.wait_exit(STOP_GRACE).await {
            return Err(ToolError::new(
                ToolErrorKind::Io,
                format!("process {pid} did not exit after being killed"),
            ));
        }
        Ok(process.info())
    }

    pub fn list(&self) -> Vec<ProcessInfo> {
        let mut list: Vec<_> = self.processes.lock().values().map(|p| p.info()).collect();
        list.sort_by(|a, b| a.started_at.cmp(&b.started_at).then(a.id.cmp(&b.id)));
        list
    }

    pub fn read(&self, args: ReadArgs) -> Result<ProcessRead, ToolError> {
        let process = self.get(&args.id)?;
        let chunk = process.output.lock().read(args.since, args.max_bytes);
        let state = process.state.lock();
        Ok(ProcessRead {
            id: process.id.clone(),
            status: state.status,
            exit_code: state.exit_code,
            chunk,
        })
    }

    /// Kills every running process (used when the app exits).
    pub async fn shutdown(&self) {
        let running: Vec<_> = self
            .processes
            .lock()
            .values()
            .filter(|p| p.is_running())
            .cloned()
            .collect();
        for process in running {
            process.state.lock().stop_requested = true;
            if let Some(pid) = process.pid {
                let _ = terminate_tree(pid, Termination::Kill);
            }
            process.wait_exit(Duration::from_secs(2)).await;
        }
    }
}

fn spawn_reader<R>(
    mut reader: R,
    stream: OutputStream,
    process: Arc<ManagedProcess>,
    sink: Arc<dyn EventSink>,
) -> tokio::task::JoinHandle<()>
where
    R: AsyncRead + Unpin + Send + 'static,
{
    tokio::spawn(async move {
        let mut decoder = Utf8Decoder::default();
        let mut buf = vec![0u8; 16 * 1024];
        loop {
            let (text, done) = match reader.read(&mut buf).await {
                Ok(0) | Err(_) => (decoder.finish(), true),
                Ok(n) => (decoder.decode(&buf[..n]), false),
            };
            if !text.is_empty() {
                // Emit while holding the buffer lock so events and offsets
                // stay in the same order across stdout and stderr.
                let mut output = process.output.lock();
                let offset = output.push(&text);
                sink.stream(StreamEvent::ProcessOutput {
                    process_id: process.id.clone(),
                    stream,
                    offset,
                    data: text,
                });
            }
            if done {
                break;
            }
        }
    })
}
