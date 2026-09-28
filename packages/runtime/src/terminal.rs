//! Real terminals (PTY): ConPTY on Windows, Unix pseudo-terminals elsewhere.
//!
//! Tools: `terminal.create`, `terminal.write`, `terminal.read`,
//! `terminal.close`, `terminal.list`. Human keystrokes and resizes arrive
//! through [`TerminalManager::input`] / [`TerminalManager::resize`] (ADR-0003).

use crate::output::{OutputBuffer, OutputChunk, Utf8Decoder};
use crate::platform::resolve_cwd;
use crate::shell::{ShellInfo, ShellRegistry};
use chrono::{DateTime, Utc};
use orchestrator_core::{
    AuditEvent, CallOrigin, EventKind, EventSink, StreamEvent, TerminalId, ToolError, ToolErrorKind,
};
use parking_lot::Mutex;
use portable_pty::{native_pty_system, ChildKiller, CommandBuilder, MasterPty, PtySize};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::Path;
use std::sync::Arc;

pub const DEFAULT_COLS: u16 = 120;
pub const DEFAULT_ROWS: u16 = 30;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CreateArgs {
    /// Shell id, kind or path. Default: the system default shell.
    #[serde(default)]
    pub shell: Option<String>,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub cols: Option<u16>,
    #[serde(default)]
    pub rows: Option<u16>,
    #[serde(default)]
    pub env: HashMap<String, String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WriteArgs {
    pub id: TerminalId,
    /// Raw input. Use `\r` for Enter (e.g. `"npm test\r"`).
    pub data: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReadArgs {
    pub id: TerminalId,
    #[serde(default)]
    pub since: Option<u64>,
    #[serde(default)]
    pub max_bytes: Option<usize>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct IdArgs {
    pub id: TerminalId,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalInfo {
    pub id: TerminalId,
    pub shell: ShellInfo,
    pub cwd: String,
    pub pid: Option<u32>,
    pub alive: bool,
    pub exit_code: Option<i32>,
    pub cols: u16,
    pub rows: u16,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalRead {
    pub id: TerminalId,
    pub alive: bool,
    pub exit_code: Option<i32>,
    #[serde(flatten)]
    pub chunk: OutputChunk,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WriteOutput {
    pub id: TerminalId,
    pub bytes_written: usize,
}

#[derive(Debug, Clone, Copy)]
struct TerminalState {
    alive: bool,
    /// Set when the terminal was closed by `terminal.close` / shutdown.
    closed: bool,
    exit_code: Option<i32>,
    cols: u16,
    rows: u16,
}

struct Terminal {
    id: TerminalId,
    shell: ShellInfo,
    cwd: String,
    pid: Option<u32>,
    created_at: DateTime<Utc>,
    origin: CallOrigin,
    state: Mutex<TerminalState>,
    output: Mutex<OutputBuffer>,
    writer: Mutex<Option<Box<dyn Write + Send>>>,
    master: Mutex<Option<Box<dyn MasterPty + Send>>>,
    killer: Mutex<Box<dyn ChildKiller + Send + Sync>>,
}

impl Terminal {
    fn info(&self) -> TerminalInfo {
        let state = *self.state.lock();
        TerminalInfo {
            id: self.id.clone(),
            shell: self.shell.clone(),
            cwd: self.cwd.clone(),
            pid: self.pid,
            alive: state.alive,
            exit_code: state.exit_code,
            cols: state.cols,
            rows: state.rows,
            created_at: self.created_at,
        }
    }

    fn write(&self, data: &[u8]) -> Result<usize, ToolError> {
        if !self.state.lock().alive {
            return Err(ToolError::new(
                ToolErrorKind::NotRunning,
                format!("terminal {} has exited", self.id),
            ));
        }
        let mut writer = self.writer.lock();
        let writer = writer.as_mut().ok_or_else(|| {
            ToolError::new(
                ToolErrorKind::NotRunning,
                format!("terminal {} is closed", self.id),
            )
        })?;
        writer
            .write_all(data)
            .and_then(|_| writer.flush())
            .map_err(|e| {
                ToolError::new(ToolErrorKind::Io, format!("terminal write failed: {e}"))
            })?;
        Ok(data.len())
    }
}

/// Owns every open terminal.
pub struct TerminalManager {
    sink: Arc<dyn EventSink>,
    terminals: Mutex<HashMap<TerminalId, Arc<Terminal>>>,
}

impl TerminalManager {
    pub fn new(sink: Arc<dyn EventSink>) -> Self {
        Self {
            sink,
            terminals: Mutex::new(HashMap::new()),
        }
    }

    fn get(&self, id: &TerminalId) -> Result<Arc<Terminal>, ToolError> {
        self.terminals
            .lock()
            .get(id)
            .cloned()
            .ok_or_else(|| ToolError::not_found(format!("unknown terminal: {id}")))
    }

    pub fn create(
        &self,
        registry: &ShellRegistry,
        base_dir: &Path,
        args: CreateArgs,
        origin: CallOrigin,
    ) -> Result<TerminalInfo, ToolError> {
        let shell = registry.resolve(args.shell.as_deref())?;
        let cwd = resolve_cwd(base_dir, args.cwd.as_deref())?;
        let cols = args.cols.filter(|c| *c > 0).unwrap_or(DEFAULT_COLS);
        let rows = args.rows.filter(|r| *r > 0).unwrap_or(DEFAULT_ROWS);

        let spawn_error = |what: &str, e: &dyn std::fmt::Display| {
            ToolError::new(ToolErrorKind::Spawn, format!("{what}: {e}"))
        };

        let pair = native_pty_system()
            .openpty(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| spawn_error("failed to open pty", &e))?;

        let mut cmd = CommandBuilder::new(&shell.path);
        cmd.args(shell.interactive_args());
        cmd.cwd(&cwd);
        if !cfg!(windows) {
            cmd.env("TERM", "xterm-256color");
            cmd.env("COLORTERM", "truecolor");
        }
        for (key, value) in &args.env {
            cmd.env(key, value);
        }

        let mut child = pair
            .slave
            .spawn_command(cmd)
            .map_err(|e| spawn_error(&format!("failed to start {}", shell.path.display()), &e))?;
        // The slave end belongs to the child now; keeping it open would
        // prevent EOF on the reader when the shell exits.
        drop(pair.slave);

        let reader = pair
            .master
            .try_clone_reader()
            .map_err(|e| spawn_error("failed to read pty", &e))?;
        let writer = pair
            .master
            .take_writer()
            .map_err(|e| spawn_error("failed to write pty", &e))?;

        let terminal = Arc::new(Terminal {
            id: TerminalId::new(),
            shell,
            cwd: cwd.display().to_string(),
            pid: child.process_id(),
            created_at: Utc::now(),
            origin,
            state: Mutex::new(TerminalState {
                alive: true,
                closed: false,
                exit_code: None,
                cols,
                rows,
            }),
            output: Mutex::new(OutputBuffer::default()),
            writer: Mutex::new(Some(writer)),
            master: Mutex::new(Some(pair.master)),
            killer: Mutex::new(child.clone_killer()),
        });

        spawn_reader_thread(reader, terminal.clone(), self.sink.clone());

        let sink = self.sink.clone();
        let watched = terminal.clone();
        std::thread::Builder::new()
            .name(format!("pty-wait-{}", terminal.id))
            .spawn(move || {
                let exit_code = child.wait().ok().map(|s| s.exit_code() as i32);
                let closed = {
                    let mut state = watched.state.lock();
                    state.alive = false;
                    state.exit_code = exit_code;
                    state.closed
                };
                // Releasing the pseudo console lets the reader reach EOF on
                // Windows (ConPTY keeps the pipe open otherwise).
                watched.master.lock().take();
                sink.stream(StreamEvent::TerminalExited {
                    terminal_id: watched.id.clone(),
                    exit_code,
                });
                let summary = if closed {
                    format!("terminal {} closed", watched.shell.name)
                } else {
                    let how = exit_code.map_or_else(
                        || "terminated by signal".to_owned(),
                        |c| format!("exit code {c}"),
                    );
                    format!("terminal {} exited ({how})", watched.shell.name)
                };
                sink.audit(AuditEvent::new(
                    EventKind::TerminalExited,
                    watched.origin.clone(),
                    summary,
                    json!({
                        "terminalId": watched.id,
                        "shell": watched.shell.id,
                        "exitCode": exit_code,
                        "closed": closed,
                    }),
                ));
            })
            .map_err(|e| {
                Self::shut(&terminal);
                spawn_error("failed to start pty waiter", &e)
            })?;

        let info = terminal.info();
        self.terminals.lock().insert(terminal.id.clone(), terminal);
        Ok(info)
    }

    pub fn write(&self, args: WriteArgs) -> Result<WriteOutput, ToolError> {
        let terminal = self.get(&args.id)?;
        let bytes_written = terminal.write(args.data.as_bytes())?;
        Ok(WriteOutput {
            id: args.id,
            bytes_written,
        })
    }

    /// Human keystrokes from the UI (streaming channel, ADR-0003).
    pub fn input(&self, id: &TerminalId, data: &[u8]) -> Result<(), ToolError> {
        self.get(id)?.write(data).map(|_| ())
    }

    pub fn resize(&self, id: &TerminalId, cols: u16, rows: u16) -> Result<(), ToolError> {
        if cols == 0 || rows == 0 {
            return Err(ToolError::invalid_args("cols and rows must be positive"));
        }
        let terminal = self.get(id)?;
        let master = terminal.master.lock();
        let Some(master) = master.as_ref() else {
            return Err(ToolError::new(
                ToolErrorKind::NotRunning,
                format!("terminal {id} has exited"),
            ));
        };
        master
            .resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| ToolError::new(ToolErrorKind::Io, format!("resize failed: {e}")))?;
        let mut state = terminal.state.lock();
        state.cols = cols;
        state.rows = rows;
        Ok(())
    }

    pub fn read(&self, args: ReadArgs) -> Result<TerminalRead, ToolError> {
        let terminal = self.get(&args.id)?;
        let chunk = terminal.output.lock().read(args.since, args.max_bytes);
        let state = *terminal.state.lock();
        Ok(TerminalRead {
            id: args.id,
            alive: state.alive,
            exit_code: state.exit_code,
            chunk,
        })
    }

    pub fn list(&self) -> Vec<TerminalInfo> {
        let mut list: Vec<_> = self.terminals.lock().values().map(|t| t.info()).collect();
        list.sort_by(|a, b| a.created_at.cmp(&b.created_at).then(a.id.cmp(&b.id)));
        list
    }

    /// Kills the shell (and, through the closed pty, its foreground jobs) and
    /// forgets the terminal.
    pub fn close(&self, id: &TerminalId) -> Result<TerminalInfo, ToolError> {
        let terminal = self
            .terminals
            .lock()
            .remove(id)
            .ok_or_else(|| ToolError::not_found(format!("unknown terminal: {id}")))?;
        Self::shut(&terminal);
        Ok(terminal.info())
    }

    fn shut(terminal: &Terminal) {
        let alive = {
            let mut state = terminal.state.lock();
            state.closed = true;
            state.alive
        };
        if alive {
            let _ = terminal.killer.lock().kill();
        }
        terminal.writer.lock().take();
        terminal.master.lock().take();
    }

    /// Closes every terminal (used when the app exits).
    pub fn shutdown(&self) {
        let all: Vec<_> = self.terminals.lock().drain().map(|(_, t)| t).collect();
        for terminal in all {
            Self::shut(&terminal);
        }
    }
}

fn spawn_reader_thread(
    mut reader: Box<dyn Read + Send>,
    terminal: Arc<Terminal>,
    sink: Arc<dyn EventSink>,
) {
    let name = format!("pty-read-{}", terminal.id);
    let reading = terminal.clone();
    let spawned = std::thread::Builder::new().name(name).spawn(move || {
        let terminal = reading;
        let mut decoder = Utf8Decoder::default();
        let mut buf = [0u8; 8192];
        loop {
            let (text, done) = match reader.read(&mut buf) {
                Ok(0) => (decoder.finish(), true),
                Ok(n) => (decoder.decode(&buf[..n]), false),
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(_) => (decoder.finish(), true),
            };
            if !text.is_empty() {
                let mut output = terminal.output.lock();
                let offset = output.push(&text);
                sink.stream(StreamEvent::TerminalOutput {
                    terminal_id: terminal.id.clone(),
                    offset,
                    data: text,
                });
            }
            if done {
                break;
            }
        }
    });
    if let Err(err) = spawned {
        // Without a reader the terminal still works for input, but output is
        // lost; make that visible in the output buffer itself.
        terminal.output.lock().push(&format!(
            "[orchestrator] failed to start pty reader: {err}\r\n"
        ));
    }
}
