//! The Tool Runtime catalog. Every entry is dispatchable by
//! [`crate::ToolRuntime::invoke`] (enforced by tests).

use orchestrator_core::ToolSpec;

const fn spec(name: &'static str, group: &'static str, description: &'static str) -> ToolSpec {
    ToolSpec {
        name,
        group,
        description,
    }
}

pub const CATALOG: &[ToolSpec] = &[
    spec(
        "filesystem.list",
        "filesystem",
        "List a directory: { path } -> { path, entries[] } (directories first).",
    ),
    spec(
        "filesystem.read",
        "filesystem",
        "Read a file: { path, encoding?: utf8|base64, maxBytes? } -> { content, encoding, size, truncated }.",
    ),
    spec(
        "filesystem.write",
        "filesystem",
        "Write a file: { path, content, encoding?, createDirs? = true, append? = false } -> { bytesWritten, created }.",
    ),
    spec(
        "filesystem.move",
        "filesystem",
        "Move or rename: { from, to, overwrite? = false } -> { from, to, replaced }.",
    ),
    spec(
        "filesystem.delete",
        "filesystem",
        "Delete a file or directory: { path, recursive? = false } -> { path, kind }.",
    ),
    spec(
        "shell.execute",
        "shell",
        "Run a command to completion: { command, cwd?, shell?, env?, timeoutMs?, stdin?, maxOutputBytes? } -> { exitCode, stdout, stderr, timedOut, durationMs }.",
    ),
    spec(
        "shell.list",
        "shell",
        "List shells available on this machine: {} -> { default, shells[] }.",
    ),
    spec(
        "terminal.create",
        "terminal",
        "Open a real terminal (PTY): { shell?, cwd?, cols?, rows?, env? } -> terminal info.",
    ),
    spec(
        "terminal.write",
        "terminal",
        "Send input to a terminal ('\\r' is Enter): { id, data } -> { bytesWritten }.",
    ),
    spec(
        "terminal.read",
        "terminal",
        "Read terminal output since an offset: { id, since?, maxBytes? } -> { data, next, truncated, alive, exitCode }.",
    ),
    spec(
        "terminal.close",
        "terminal",
        "Close a terminal and kill its shell: { id } -> terminal info.",
    ),
    spec("terminal.list", "terminal", "List open terminals: {} -> terminal info[]."),
    spec(
        "process.start",
        "process",
        "Start a long-running process: { command, cwd?, shell?, env?, name? } -> process info.",
    ),
    spec(
        "process.stop",
        "process",
        "Stop a process and its whole tree: { id, force? = false } -> process info.",
    ),
    spec("process.list", "process", "List managed processes: {} -> process info[]."),
    spec(
        "process.read",
        "process",
        "Read process output since an offset: { id, since?, maxBytes? } -> { data, next, truncated, status, exitCode }.",
    ),
];
