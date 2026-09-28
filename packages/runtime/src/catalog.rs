//! The Tool Runtime catalog. Every entry is dispatchable by
//! [`crate::ToolRuntime::invoke`] (enforced by tests).

use orchestrator_core::ToolSpec;

/// A tool that only queries state.
const fn query(name: &'static str, group: &'static str, description: &'static str) -> ToolSpec {
    ToolSpec {
        name,
        group,
        description,
        read_only: true,
    }
}

/// A tool that may change files, processes or repository state.
const fn action(name: &'static str, group: &'static str, description: &'static str) -> ToolSpec {
    ToolSpec {
        name,
        group,
        description,
        read_only: false,
    }
}

pub const CATALOG: &[ToolSpec] = &[
    query(
        "filesystem.list",
        "filesystem",
        "List a directory: { path } -> { path, entries[] } (directories first).",
    ),
    query(
        "filesystem.read",
        "filesystem",
        "Read a file: { path, encoding?: utf8|base64, maxBytes? } -> { content, encoding, size, truncated }.",
    ),
    action(
        "filesystem.write",
        "filesystem",
        "Write a file: { path, content, encoding?, createDirs? = true, append? = false } -> { bytesWritten, created }.",
    ),
    action(
        "filesystem.move",
        "filesystem",
        "Move or rename: { from, to, overwrite? = false } -> { from, to, replaced }.",
    ),
    action(
        "filesystem.delete",
        "filesystem",
        "Delete a file or directory: { path, recursive? = false } -> { path, kind }.",
    ),
    action(
        "shell.execute",
        "shell",
        "Run a command to completion: { command, cwd?, shell?, env?, timeoutMs?, stdin?, maxOutputBytes? } -> { exitCode, stdout, stderr, timedOut, durationMs }.",
    ),
    query(
        "shell.list",
        "shell",
        "List shells available on this machine: {} -> { default, shells[] }.",
    ),
    action(
        "terminal.create",
        "terminal",
        "Open a real terminal (PTY): { shell?, cwd?, cols?, rows?, env? } -> terminal info.",
    ),
    action(
        "terminal.write",
        "terminal",
        "Send input to a terminal ('\\r' is Enter): { id, data } -> { bytesWritten }.",
    ),
    query(
        "terminal.read",
        "terminal",
        "Read terminal output since an offset: { id, since?, maxBytes? } -> { data, next, truncated, alive, exitCode }.",
    ),
    action(
        "terminal.close",
        "terminal",
        "Close a terminal and kill its shell: { id } -> terminal info.",
    ),
    query(
        "terminal.list",
        "terminal",
        "List open terminals: {} -> terminal info[].",
    ),
    action(
        "process.start",
        "process",
        "Start a long-running process: { command, cwd?, shell?, env?, name? } -> process info.",
    ),
    action(
        "process.stop",
        "process",
        "Stop a process and its whole tree: { id, force? = false } -> process info.",
    ),
    query(
        "process.list",
        "process",
        "List managed processes: {} -> process info[].",
    ),
    query(
        "process.read",
        "process",
        "Read process output since an offset: { id, since?, maxBytes? } -> { data, next, truncated, status, exitCode }.",
    ),
    query(
        "project.discover",
        "project",
        "Find project folders: { roots?, maxDepth? = 4, maxDirs? = 20000 } -> { projects[], scannedDirs, truncated }.",
    ),
    query(
        "project.profile",
        "project",
        "PROJECT PROFILE of a folder (default: open project): { path? } -> languages, frameworks, package managers, runtimes, Docker, databases, Git, files.",
    ),
    action(
        "project.open",
        "project",
        "Open a project; it becomes the base directory for relative paths and cwd: { path } -> profile.",
    ),
    query(
        "git.status",
        "git",
        "Repository status: { path? } -> { branch, head, upstream, ahead, behind, files[], clean, remotes[] }.",
    ),
    query(
        "git.diff",
        "git",
        "Unified diff: { path?, staged?, target?, files?, contextLines?, maxBytes? } -> { patch, files[], truncated }.",
    ),
    query(
        "git.log",
        "git",
        "Recent commits: { path?, limit? = 30, ref?, file? } -> commits[].",
    ),
    action(
        "git.branch",
        "git",
        "List branches, optionally creating or deleting one: { path?, create?, startPoint?, delete?, force? } -> branches[].",
    ),
    action(
        "git.checkout",
        "git",
        "Switch to a branch/tag/commit: { path?, target, create?, startPoint? } -> { output, status }.",
    ),
    action(
        "git.add",
        "git",
        "Stage files: { path?, files?, all? } -> status.",
    ),
    action(
        "git.commit",
        "git",
        "Commit staged changes: { path?, message, all?, amend? } -> { hash, shortHash, branch, subject, output }.",
    ),
    action(
        "git.pull",
        "git",
        "Pull from the remote: { path?, remote?, branch?, mode?: merge|rebase|ffOnly, timeoutMs? } -> { output, status }.",
    ),
    action(
        "git.push",
        "git",
        "Push to the remote: { path?, remote?, branch?, setUpstream?, force? (with lease), timeoutMs? } -> { output, status }.",
    ),
    action(
        "git.stash",
        "git",
        "Stash changes: { path?, action: push|pop|apply|drop|list, message?, includeUntracked?, index? } -> { output, stashes[] }.",
    ),
    action(
        "git.reset",
        "git",
        "Reset HEAD or unstage files: { path?, mode?: soft|mixed|hard, target?, files? } -> { output, status }.",
    ),
    action(
        "package.install",
        "package",
        "Install dependencies with the detected manager: { path?, packages?, dev?, manager?, timeoutMs? } -> { manager, command, result }.",
    ),
    action(
        "package.run",
        "package",
        "Run a project script: { script, args?, path?, manager?, background?, timeoutMs? } -> { manager, command, result | process }.",
    ),
    query(
        "runtime.node",
        "runtime",
        "Node.js availability and versions of node, npm, pnpm, yarn, bun: {} -> { available, version, managers }.",
    ),
    query(
        "runtime.python",
        "runtime",
        "Python availability, version and pip: {} -> { available, version, command, pip }.",
    ),
    query(
        "runtime.docker",
        "runtime",
        "Docker availability, daemon state and compose: {} -> { available, version, daemonRunning, compose }.",
    ),
];
