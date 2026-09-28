//! Shell detection and non-interactive command execution (`shell.execute`).

use crate::platform::{configure_process_tree, resolve_cwd, terminate_tree, Termination};
use orchestrator_core::{ToolError, ToolErrorKind};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};

/// Default cap per output stream for `shell.execute` (ADR-0004).
pub const DEFAULT_MAX_OUTPUT_BYTES: usize = 4 * 1024 * 1024;

/// How long to wait for stdout/stderr to close after the shell exited (a
/// background grandchild may keep the pipes open).
const PIPE_DRAIN_GRACE: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ShellKind {
    Bash,
    Zsh,
    Fish,
    Sh,
    /// PowerShell 7+ (`pwsh`).
    Pwsh,
    /// Windows PowerShell 5.x (`powershell.exe`).
    PowerShell,
    Cmd,
    Wsl,
}

impl ShellKind {
    /// Infers the kind from an executable name such as `/usr/bin/zsh`.
    pub fn from_executable(path: &Path) -> Option<Self> {
        let stem = path.file_stem()?.to_str()?.to_ascii_lowercase();
        Some(match stem.as_str() {
            "bash" => Self::Bash,
            "zsh" => Self::Zsh,
            "fish" => Self::Fish,
            "sh" | "dash" | "ash" => Self::Sh,
            "pwsh" => Self::Pwsh,
            "powershell" => Self::PowerShell,
            "cmd" => Self::Cmd,
            "wsl" => Self::Wsl,
            _ => return None,
        })
    }
}

/// A shell available on this machine.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShellInfo {
    /// Stable identifier used in tool arguments (`bash`, `pwsh`, `git-bash`…).
    pub id: String,
    pub kind: ShellKind,
    /// Display name.
    pub name: String,
    pub path: PathBuf,
}

impl ShellInfo {
    pub fn new(id: &str, kind: ShellKind, name: &str, path: PathBuf) -> Self {
        Self {
            id: id.to_owned(),
            kind,
            name: name.to_owned(),
            path,
        }
    }

    /// Arguments that start an interactive session inside a terminal.
    pub fn interactive_args(&self) -> Vec<String> {
        match self.kind {
            // Login shells load the user's profile (PATH for nvm, pyenv, …),
            // which matters when the app is launched from a GUI.
            ShellKind::Bash | ShellKind::Zsh | ShellKind::Fish => vec!["-l".into()],
            ShellKind::Pwsh | ShellKind::PowerShell => vec!["-NoLogo".into()],
            ShellKind::Sh | ShellKind::Cmd | ShellKind::Wsl => Vec::new(),
        }
    }

    /// Builds a process that runs `command` non-interactively in this shell.
    pub fn build_command(&self, command: &str) -> tokio::process::Command {
        let mut cmd = tokio::process::Command::new(&self.path);
        match self.kind {
            ShellKind::Bash | ShellKind::Zsh | ShellKind::Fish => {
                cmd.args(["-l", "-c", command]);
            }
            ShellKind::Sh => {
                cmd.args(["-c", command]);
            }
            ShellKind::Pwsh | ShellKind::PowerShell => {
                // Force UTF-8 so non-ASCII output survives the pipe.
                let script = format!(
                    "[Console]::OutputEncoding = [System.Text.Encoding]::UTF8; $OutputEncoding = [System.Text.Encoding]::UTF8; {command}"
                );
                cmd.args([
                    "-NoLogo",
                    "-NoProfile",
                    "-NonInteractive",
                    "-Command",
                    &script,
                ]);
            }
            ShellKind::Cmd => {
                cmd.args(["/D", "/S", "/C"]);
                #[cfg(windows)]
                {
                    // cmd.exe does its own parsing: pass the command verbatim,
                    // wrapped in the quotes that `/S` strips.
                    cmd.raw_arg(format!("\"{command}\""));
                }
                #[cfg(not(windows))]
                {
                    cmd.arg(command);
                }
            }
            ShellKind::Wsl => {
                cmd.args(["-e", "bash", "-lc", command]);
            }
        }
        cmd
    }
}

/// Shells detected on this machine, plus the default one.
#[derive(Debug, Clone)]
pub struct ShellRegistry {
    shells: Vec<ShellInfo>,
    default_id: String,
}

impl ShellRegistry {
    /// Detects the shells installed on this machine.
    pub fn detect() -> Self {
        #[cfg(windows)]
        let (shells, default_id) = detect_windows();
        #[cfg(not(windows))]
        let (shells, default_id) = detect_unix();
        Self { shells, default_id }
    }

    /// Builds a registry from explicit shells (the first one is the default
    /// unless `default_id` names another).
    pub fn from_shells(shells: Vec<ShellInfo>, default_id: Option<&str>) -> Self {
        let default_id = default_id
            .map(str::to_owned)
            .or_else(|| shells.first().map(|s| s.id.clone()))
            .unwrap_or_default();
        Self { shells, default_id }
    }

    pub fn list(&self) -> &[ShellInfo] {
        &self.shells
    }

    pub fn default_id(&self) -> &str {
        &self.default_id
    }

    /// Resolves a requested shell: an id (`bash`), a kind name (`powerShell`)
    /// or a path to a shell executable. `None` selects the default shell.
    pub fn resolve(&self, requested: Option<&str>) -> Result<ShellInfo, ToolError> {
        let wanted = match requested.map(str::trim) {
            None | Some("") => self.default_id.as_str(),
            Some(value) => value,
        };
        if let Some(shell) = self
            .shells
            .iter()
            .find(|s| s.id.eq_ignore_ascii_case(wanted))
        {
            return Ok(shell.clone());
        }
        if let Some(shell) = self.shells.iter().find(|s| {
            serde_json::to_value(s.kind)
                .ok()
                .and_then(|v| v.as_str().map(|k| k.eq_ignore_ascii_case(wanted)))
                .unwrap_or(false)
        }) {
            return Ok(shell.clone());
        }
        let path = Path::new(wanted);
        if path.components().count() > 1 && path.is_file() {
            if let Some(kind) = ShellKind::from_executable(path) {
                let name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| wanted.to_owned());
                return Ok(ShellInfo::new(wanted, kind, &name, path.to_path_buf()));
            }
        }
        let known: Vec<&str> = self.shells.iter().map(|s| s.id.as_str()).collect();
        Err(ToolError::not_found(format!(
            "shell not available: {wanted} (available: {})",
            known.join(", ")
        )))
    }
}

#[cfg(not(windows))]
fn detect_unix() -> (Vec<ShellInfo>, String) {
    let mut shells: Vec<ShellInfo> = Vec::new();
    let candidates = [
        ("bash", ShellKind::Bash, "Bash"),
        ("zsh", ShellKind::Zsh, "Zsh"),
        ("fish", ShellKind::Fish, "Fish"),
        ("sh", ShellKind::Sh, "sh"),
        ("pwsh", ShellKind::Pwsh, "PowerShell 7"),
    ];
    for (bin, kind, name) in candidates {
        if let Ok(path) = which::which(bin) {
            shells.push(ShellInfo::new(bin, kind, name, path));
        }
    }

    // Prefer the user's login shell ($SHELL) as default.
    let login = std::env::var_os("SHELL")
        .map(PathBuf::from)
        .filter(|p| p.is_file());
    let mut default_id = None;
    if let Some(path) = login {
        if let Some(kind) = ShellKind::from_executable(&path) {
            match shells.iter().position(|s| s.kind == kind) {
                Some(index) => {
                    // Use the exact binary from $SHELL.
                    shells[index].path = path;
                    default_id = Some(shells[index].id.clone());
                }
                None => {
                    let id = path
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_else(|| "shell".into());
                    shells.insert(0, ShellInfo::new(&id, kind, &id, path));
                    default_id = Some(id);
                }
            }
        }
    }

    if shells.is_empty() {
        shells.push(ShellInfo::new(
            "sh",
            ShellKind::Sh,
            "sh",
            PathBuf::from("/bin/sh"),
        ));
    }
    let default_id = default_id
        .or_else(|| {
            ["bash", "zsh", "sh"]
                .iter()
                .find(|id| shells.iter().any(|s| s.id == **id))
                .map(|id| (*id).to_owned())
        })
        .unwrap_or_else(|| shells[0].id.clone());
    (shells, default_id)
}

#[cfg(windows)]
fn detect_windows() -> (Vec<ShellInfo>, String) {
    let mut shells: Vec<ShellInfo> = Vec::new();
    let system_root = std::env::var_os("SystemRoot")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\Windows"));

    if let Ok(path) = which::which("pwsh") {
        shells.push(ShellInfo::new(
            "pwsh",
            ShellKind::Pwsh,
            "PowerShell 7",
            path,
        ));
    }

    let powershell = which::which("powershell").ok().or_else(|| {
        let p = system_root.join(r"System32\WindowsPowerShell\v1.0\powershell.exe");
        p.is_file().then_some(p)
    });
    if let Some(path) = powershell {
        shells.push(ShellInfo::new(
            "powershell",
            ShellKind::PowerShell,
            "Windows PowerShell",
            path,
        ));
    }

    let cmd = std::env::var_os("ComSpec")
        .map(PathBuf::from)
        .filter(|p| p.is_file())
        .or_else(|| which::which("cmd").ok());
    if let Some(path) = cmd {
        shells.push(ShellInfo::new(
            "cmd",
            ShellKind::Cmd,
            "Command Prompt",
            path,
        ));
    }

    if let Ok(path) = which::which("wsl") {
        shells.push(ShellInfo::new("wsl", ShellKind::Wsl, "WSL", path));
    }

    // Git Bash: <Git>\cmd\git.exe -> <Git>\bin\bash.exe. (A bare `bash.exe` in
    // System32 is the WSL launcher, so it is not used here.)
    let mut git_bash = which::which("git").ok().and_then(|git| {
        let root = git.parent()?.parent()?;
        let bash = root.join(r"bin\bash.exe");
        bash.is_file().then_some(bash)
    });
    if git_bash.is_none() {
        for var in ["ProgramFiles", "ProgramFiles(x86)", "LOCALAPPDATA"] {
            if let Some(base) = std::env::var_os(var).map(PathBuf::from) {
                for sub in [r"Git\bin\bash.exe", r"Programs\Git\bin\bash.exe"] {
                    let candidate = base.join(sub);
                    if candidate.is_file() {
                        git_bash = Some(candidate);
                        break;
                    }
                }
            }
            if git_bash.is_some() {
                break;
            }
        }
    }
    if let Some(path) = git_bash {
        shells.push(ShellInfo::new(
            "git-bash",
            ShellKind::Bash,
            "Git Bash",
            path,
        ));
    }

    if shells.is_empty() {
        shells.push(ShellInfo::new(
            "cmd",
            ShellKind::Cmd,
            "Command Prompt",
            system_root.join(r"System32\cmd.exe"),
        ));
    }
    let default_id = ["pwsh", "powershell", "cmd"]
        .iter()
        .find(|id| shells.iter().any(|s| s.id == **id))
        .map(|id| (*id).to_owned())
        .unwrap_or_else(|| shells[0].id.clone());
    (shells, default_id)
}

/// Arguments of `shell.execute`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExecuteArgs {
    pub command: String,
    #[serde(default)]
    pub cwd: Option<String>,
    /// Shell id, kind or path. Default: the system default shell.
    #[serde(default)]
    pub shell: Option<String>,
    #[serde(default)]
    pub env: HashMap<String, String>,
    /// No timeout when absent.
    #[serde(default)]
    pub timeout_ms: Option<u64>,
    /// Text written to the command's stdin (stdin is empty otherwise).
    #[serde(default)]
    pub stdin: Option<String>,
    /// Cap per output stream. Default: [`DEFAULT_MAX_OUTPUT_BYTES`].
    #[serde(default)]
    pub max_output_bytes: Option<usize>,
}

/// Output of `shell.execute`. A non-zero exit code is not a tool error.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecuteOutput {
    pub command: String,
    pub shell: String,
    pub cwd: String,
    /// `None` when the process was killed by a signal or timed out.
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub stdout_truncated: bool,
    pub stderr_truncated: bool,
    pub timed_out: bool,
    pub duration_ms: u64,
}

#[derive(Debug, Default)]
struct Captured {
    bytes: Vec<u8>,
    truncated: bool,
}

async fn capture<R: AsyncRead + Unpin>(
    mut reader: R,
    limit: usize,
    into: Arc<parking_lot::Mutex<Captured>>,
) {
    let mut buf = vec![0u8; 16 * 1024];
    loop {
        match reader.read(&mut buf).await {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                let mut captured = into.lock();
                let room = limit.saturating_sub(captured.bytes.len());
                if n > room {
                    captured.truncated = true;
                }
                let take = n.min(room);
                captured.bytes.extend_from_slice(&buf[..take]);
                // Keep draining past the limit so the child never blocks on a
                // full pipe.
            }
        }
    }
}

/// Runs a command to completion and captures its output.
pub async fn execute(
    registry: &ShellRegistry,
    base_dir: &Path,
    args: ExecuteArgs,
) -> Result<ExecuteOutput, ToolError> {
    if args.command.trim().is_empty() {
        return Err(ToolError::invalid_args("command must not be empty"));
    }
    let shell = registry.resolve(args.shell.as_deref())?;
    let cwd = resolve_cwd(base_dir, args.cwd.as_deref())?;
    let limit = args.max_output_bytes.unwrap_or(DEFAULT_MAX_OUTPUT_BYTES);

    let mut cmd = shell.build_command(&args.command);
    cmd.current_dir(&cwd)
        .envs(&args.env)
        .stdin(if args.stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    configure_process_tree(&mut cmd);

    let started = Instant::now();
    let mut child = cmd.spawn().map_err(|e| {
        ToolError::new(
            ToolErrorKind::Spawn,
            format!("failed to start {}: {e}", shell.path.display()),
        )
    })?;
    let pid = child.id();

    if let (Some(input), Some(mut stdin)) = (args.stdin, child.stdin.take()) {
        tokio::spawn(async move {
            let _ = stdin.write_all(input.as_bytes()).await;
            let _ = stdin.shutdown().await;
        });
    }

    let stdout = Arc::new(parking_lot::Mutex::new(Captured::default()));
    let stderr = Arc::new(parking_lot::Mutex::new(Captured::default()));
    let out_task = child
        .stdout
        .take()
        .map(|r| tokio::spawn(capture(r, limit, stdout.clone())));
    let err_task = child
        .stderr
        .take()
        .map(|r| tokio::spawn(capture(r, limit, stderr.clone())));

    let mut timed_out = false;
    let status = match args.timeout_ms {
        Some(ms) => match tokio::time::timeout(Duration::from_millis(ms), child.wait()).await {
            Ok(status) => Some(status),
            Err(_) => {
                timed_out = true;
                if let Some(pid) = pid {
                    let _ = terminate_tree(pid, Termination::Kill);
                }
                let _ = child.start_kill();
                Some(child.wait().await)
            }
        },
        None => Some(child.wait().await),
    };
    let status = status.transpose().map_err(|e| {
        ToolError::new(
            ToolErrorKind::Io,
            format!("failed waiting for command: {e}"),
        )
    })?;

    for task in [out_task, err_task].into_iter().flatten() {
        let abort = task.abort_handle();
        if tokio::time::timeout(PIPE_DRAIN_GRACE, task).await.is_err() {
            abort.abort();
        }
    }

    let (stdout, stdout_truncated) = {
        let c = stdout.lock();
        (String::from_utf8_lossy(&c.bytes).into_owned(), c.truncated)
    };
    let (stderr, stderr_truncated) = {
        let c = stderr.lock();
        (String::from_utf8_lossy(&c.bytes).into_owned(), c.truncated)
    };

    Ok(ExecuteOutput {
        command: args.command,
        shell: shell.id,
        cwd: cwd.display().to_string(),
        exit_code: if timed_out {
            None
        } else {
            status.and_then(|s| s.code())
        },
        stdout,
        stderr,
        stdout_truncated,
        stderr_truncated,
        timed_out,
        duration_ms: started.elapsed().as_millis() as u64,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_finds_at_least_one_shell_and_a_valid_default() {
        let registry = ShellRegistry::detect();
        assert!(!registry.list().is_empty());
        let default = registry.resolve(None).unwrap();
        assert_eq!(default.id, registry.default_id());
    }

    #[test]
    fn resolve_by_kind_and_unknown() {
        let registry = ShellRegistry::from_shells(
            vec![ShellInfo::new(
                "git-bash",
                ShellKind::Bash,
                "Git Bash",
                PathBuf::from("/bin/bash"),
            )],
            None,
        );
        assert_eq!(registry.resolve(Some("bash")).unwrap().id, "git-bash");
        assert_eq!(registry.resolve(Some("GIT-BASH")).unwrap().id, "git-bash");
        let err = registry.resolve(Some("nushell")).unwrap_err();
        assert_eq!(err.kind, ToolErrorKind::NotFound);
    }

    #[test]
    fn kind_from_executable() {
        assert_eq!(
            ShellKind::from_executable(Path::new("/usr/bin/zsh")),
            Some(ShellKind::Zsh)
        );
        assert_eq!(
            ShellKind::from_executable(Path::new(
                "C:/Windows/System32/WindowsPowerShell/v1.0/powershell.exe"
            )),
            Some(ShellKind::PowerShell)
        );
        assert_eq!(ShellKind::from_executable(Path::new("/usr/bin/vim")), None);
    }
}
