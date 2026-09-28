//! Operating-system specific helpers: home directory, path resolution,
//! process-group setup and process-tree termination.

use orchestrator_core::{ToolError, ToolErrorKind};
use std::io;
use std::path::{Path, PathBuf};

/// `CREATE_NO_WINDOW`: prevents a console window from popping up when the
/// desktop (GUI) app spawns console programs on Windows.
#[cfg(windows)]
pub(crate) const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// The current user's home directory.
pub fn home_dir() -> Option<PathBuf> {
    let var = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    std::env::var_os(var)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

/// Resolves a user/agent supplied path: `~` expands to the home directory,
/// absolute paths are kept, relative paths are joined to `base`.
pub fn resolve_path(base: &Path, input: &str) -> Result<PathBuf, ToolError> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err(ToolError::invalid_args("path must not be empty"));
    }
    if trimmed == "~" {
        return home_dir().ok_or_else(|| ToolError::not_found("home directory is not set"));
    }
    if let Some(rest) = trimmed
        .strip_prefix("~/")
        .or_else(|| trimmed.strip_prefix("~\\"))
    {
        let home = home_dir().ok_or_else(|| ToolError::not_found("home directory is not set"))?;
        return Ok(home.join(rest));
    }
    let path = Path::new(trimmed);
    Ok(if path.is_absolute() {
        path.to_path_buf()
    } else {
        base.join(path)
    })
}

/// Resolves an optional working directory and checks that it is a directory.
pub fn resolve_cwd(base: &Path, input: Option<&str>) -> Result<PathBuf, ToolError> {
    let cwd = match input {
        Some(value) if !value.trim().is_empty() => resolve_path(base, value)?,
        _ => base.to_path_buf(),
    };
    if !cwd.is_dir() {
        return Err(ToolError::not_found(format!(
            "working directory does not exist: {}",
            cwd.display()
        )));
    }
    Ok(cwd)
}

/// Maps an I/O error to a tool error, keeping the OS message.
pub fn io_error(action: &str, path: &Path, err: io::Error) -> ToolError {
    let kind = match err.kind() {
        io::ErrorKind::NotFound => ToolErrorKind::NotFound,
        io::ErrorKind::AlreadyExists => ToolErrorKind::AlreadyExists,
        io::ErrorKind::PermissionDenied => ToolErrorKind::PermissionDenied,
        _ => ToolErrorKind::Io,
    };
    ToolError::new(kind, format!("{action} {}: {err}", path.display()))
}

/// Prepares a command so that it and all its descendants can be terminated
/// together, and so that no console window appears on Windows.
pub(crate) fn configure_process_tree(cmd: &mut tokio::process::Command) {
    #[cfg(unix)]
    {
        // New process group whose id equals the child's pid.
        cmd.process_group(0);
    }
    #[cfg(windows)]
    {
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
}

/// How to terminate a process tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Termination {
    /// Ask politely (SIGTERM on Unix). Windows has no equivalent for console
    /// programs, so this is the same as `Kill` there.
    Graceful,
    /// Terminate immediately (SIGKILL / `taskkill /F`).
    Kill,
}

/// Terminates the process tree rooted at `pid` (which must have been started
/// with [`configure_process_tree`]).
pub(crate) fn terminate_tree(pid: u32, how: Termination) -> io::Result<()> {
    #[cfg(unix)]
    {
        let signal = match how {
            Termination::Graceful => libc::SIGTERM,
            Termination::Kill => libc::SIGKILL,
        };
        let pgid = libc::pid_t::try_from(pid)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "pid out of range"))?;
        // SAFETY: `kill` has no memory-safety preconditions; a negative pid
        // targets the process group created by `configure_process_tree`.
        let rc = unsafe { libc::kill(-pgid, signal) };
        if rc == 0 {
            return Ok(());
        }
        let err = io::Error::last_os_error();
        if err.raw_os_error() == Some(libc::ESRCH) {
            // Already gone.
            return Ok(());
        }
        Err(err)
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let _ = how;
        let output = std::process::Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .creation_flags(CREATE_NO_WINDOW)
            .output()?;
        // Exit code 128: process not found (already exited).
        if output.status.success() || output.status.code() == Some(128) {
            Ok(())
        } else {
            Err(io::Error::other(
                String::from_utf8_lossy(&output.stderr).trim().to_owned(),
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_paths_join_base() {
        let base = std::env::temp_dir();
        assert_eq!(resolve_path(&base, "a/b").unwrap(), base.join("a/b"));
    }

    #[test]
    fn absolute_paths_are_kept() {
        let abs = std::env::temp_dir().join("x");
        let base = Path::new("/somewhere/else");
        assert_eq!(resolve_path(base, abs.to_str().unwrap()).unwrap(), abs);
    }

    #[test]
    fn tilde_expands_to_home() {
        if let Some(home) = home_dir() {
            let base = Path::new("/");
            assert_eq!(resolve_path(base, "~").unwrap(), home);
            assert_eq!(resolve_path(base, "~/docs").unwrap(), home.join("docs"));
        }
    }

    #[test]
    fn empty_path_is_invalid() {
        let err = resolve_path(Path::new("/"), "  ").unwrap_err();
        assert_eq!(err.kind, ToolErrorKind::InvalidArgs);
    }

    #[test]
    fn missing_cwd_is_not_found() {
        let err = resolve_cwd(Path::new("/"), Some("/definitely/not/here/42")).unwrap_err();
        assert_eq!(err.kind, ToolErrorKind::NotFound);
    }
}
