//! Orchestrator Tool Runtime.
//!
//! The single component that executes operations on the operating system.
//! Every caller — the human through the UI today, AI agents from Phase 3 on —
//! sends a [`ToolCall`] to [`ToolRuntime::invoke`] and receives a
//! [`ToolResult`]. Every call is recorded as a `TOOL_CALLED` audit event, plus
//! domain events (`FILE_CHANGED`, `COMMAND_EXECUTED`, …).
//!
//! The runtime contains no command blocklist and no hidden confirmations.
//! The autonomy gate (Assisted / Autonomous / Unrestricted) is added in
//! Phase 9 in front of [`ToolRuntime::invoke`]; auditing stays on in every
//! mode.

mod catalog;
pub mod filesystem;
pub mod output;
pub mod platform;
pub mod process;
pub mod shell;
pub mod terminal;

pub use catalog::CATALOG;
pub use shell::{ShellInfo, ShellKind, ShellRegistry};

use chrono::Utc;
use orchestrator_core::{
    AuditEvent, EventKind, EventSink, TerminalId, ToolCall, ToolError, ToolErrorKind, ToolResult,
    ToolSpec,
};
use platform::resolve_path;
use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::{json, Map, Value};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

/// Strings longer than this are shortened in audit event arguments.
const AUDIT_MAX_STRING: usize = 512;
/// Bytes of stdout/stderr kept in `COMMAND_EXECUTED` events.
const AUDIT_OUTPUT_TAIL: usize = 4 * 1024;

/// Runtime configuration.
#[derive(Debug, Clone)]
pub struct RuntimeConfig {
    /// Directory used to resolve relative paths and as default working
    /// directory (home directory in Phase 1, project root from Phase 2).
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
    base_dir: PathBuf,
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
                base_dir: config.base_dir,
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

    pub fn base_dir(&self) -> &Path {
        &self.inner.base_dir
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
        let base = inner.base_dir.as_path();
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
                let exit = match (out.timed_out, out.exit_code) {
                    (true, _) => "timed out".to_owned(),
                    (false, Some(code)) => format!("exit {code}"),
                    (false, None) => "killed".to_owned(),
                };
                let event = AuditEvent::new(
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
                );
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
                let event = AuditEvent::new(
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
                );
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

            other => Err(ToolError::new(
                ToolErrorKind::UnknownTool,
                format!("unknown tool: {other}"),
            )),
        }
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

/// Arguments of tools that take none.
#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Empty {}

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
