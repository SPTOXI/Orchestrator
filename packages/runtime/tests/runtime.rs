//! End-to-end tests of the Tool Runtime through `ToolRuntime::invoke`, the
//! same entry point used by the desktop UI and, later, by AI agents.

use orchestrator_core::{
    CallOrigin, EventKind, MemorySink, StreamEvent, ToolCall, ToolErrorKind, ToolResult,
};
use orchestrator_runtime::{RuntimeConfig, ToolRuntime, CATALOG};
use serde_json::{json, Value};
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

#[cfg(windows)]
const SHELL: &str = "cmd";
#[cfg(not(windows))]
const SHELL: &str = "sh";

fn sleep_command(secs: u32) -> String {
    if cfg!(windows) {
        format!("ping -n {} 127.0.0.1 >nul", secs + 1)
    } else {
        format!("sleep {secs}")
    }
}

fn runtime_in(dir: &Path) -> (ToolRuntime, Arc<MemorySink>) {
    let sink = Arc::new(MemorySink::new());
    let runtime = ToolRuntime::new(
        RuntimeConfig {
            base_dir: dir.to_path_buf(),
        },
        sink.clone(),
    );
    (runtime, sink)
}

async fn call(runtime: &ToolRuntime, tool: &str, args: Value) -> ToolResult {
    runtime
        .invoke(ToolCall::new(tool, args, CallOrigin::User))
        .await
}

async fn ok(runtime: &ToolRuntime, tool: &str, args: Value) -> Value {
    let result = call(runtime, tool, args).await;
    assert!(result.ok, "{tool} failed: {:?}", result.error);
    result.output
}

async fn eventually<F: FnMut() -> bool>(timeout: Duration, mut condition: F) -> bool {
    let start = Instant::now();
    loop {
        if condition() {
            return true;
        }
        if start.elapsed() > timeout {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn every_catalog_tool_is_dispatchable() {
    let dir = tempfile::tempdir().unwrap();
    let (runtime, _) = runtime_in(dir.path());
    for spec in CATALOG {
        let result = call(&runtime, spec.name, json!({"__probe": true})).await;
        let kind = result.error.map(|e| e.kind);
        assert_ne!(
            kind,
            Some(ToolErrorKind::UnknownTool),
            "{} is in the catalog but not dispatched",
            spec.name
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn unknown_tool_fails_and_is_still_audited() {
    let dir = tempfile::tempdir().unwrap();
    let (runtime, sink) = runtime_in(dir.path());
    let result = call(&runtime, "nope.nothing", Value::Null).await;
    assert!(!result.ok);
    assert_eq!(result.error.unwrap().kind, ToolErrorKind::UnknownTool);

    let events = sink.audit_events();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].kind, EventKind::ToolCalled);
    assert_eq!(events[0].data["ok"], false);
    assert_eq!(events[0].call_id.as_ref(), Some(&result.call_id));
}

#[tokio::test(flavor = "multi_thread")]
async fn filesystem_tools_roundtrip_and_emit_file_changed() {
    let dir = tempfile::tempdir().unwrap();
    let (runtime, sink) = runtime_in(dir.path());

    // Relative paths resolve against the runtime base directory.
    let written = ok(
        &runtime,
        "filesystem.write",
        json!({"path": "src/main.txt", "content": "x".repeat(2048)}),
    )
    .await;
    assert_eq!(written["created"], true);
    assert!(dir.path().join("src/main.txt").is_file());

    let listed = ok(&runtime, "filesystem.list", json!({"path": "src"})).await;
    assert_eq!(listed["entries"][0]["name"], "main.txt");
    assert_eq!(listed["entries"][0]["kind"], "file");

    let read = ok(
        &runtime,
        "filesystem.read",
        json!({"path": "src/main.txt", "maxBytes": 10}),
    )
    .await;
    assert_eq!(read["content"], "xxxxxxxxxx");
    assert_eq!(read["truncated"], true);

    ok(
        &runtime,
        "filesystem.move",
        json!({"from": "src/main.txt", "to": "lib/moved.txt"}),
    )
    .await;
    ok(
        &runtime,
        "filesystem.delete",
        json!({"path": "lib", "recursive": true}),
    )
    .await;
    assert!(!dir.path().join("lib").exists());

    let events = sink.audit_events();
    let changes: Vec<_> = events
        .iter()
        .filter(|e| e.kind == EventKind::FileChanged)
        .map(|e| e.data["change"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(changes, vec!["created", "moved", "deleted"]);

    // Long content is summarized in the audit log, not duplicated.
    let write_event = events
        .iter()
        .find(|e| e.kind == EventKind::ToolCalled && e.data["tool"] == "filesystem.write")
        .unwrap();
    assert!(write_event.data["args"]["content"]
        .as_str()
        .unwrap()
        .ends_with("(+2048 bytes)"));
}

#[tokio::test(flavor = "multi_thread")]
async fn invalid_args_are_reported() {
    let dir = tempfile::tempdir().unwrap();
    let (runtime, _) = runtime_in(dir.path());
    let result = call(&runtime, "filesystem.read", json!({"pth": "typo"})).await;
    assert_eq!(result.error.unwrap().kind, ToolErrorKind::InvalidArgs);
}

#[tokio::test(flavor = "multi_thread")]
async fn shell_list_reports_default() {
    let dir = tempfile::tempdir().unwrap();
    let (runtime, _) = runtime_in(dir.path());
    let out = ok(&runtime, "shell.list", json!({})).await;
    let default = out["default"].as_str().unwrap();
    let shells = out["shells"].as_array().unwrap();
    assert!(shells.iter().any(|s| s["id"] == default));
}

#[tokio::test(flavor = "multi_thread")]
async fn shell_execute_captures_output_exit_code_and_cwd() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("marker.txt"), "").unwrap();
    let (runtime, sink) = runtime_in(dir.path());

    let listing = if cfg!(windows) { "dir /b" } else { "ls" };
    let out = ok(
        &runtime,
        "shell.execute",
        json!({
            "shell": SHELL,
            "command": format!("echo hello && echo oops 1>&2 && {listing} && exit 3"),
        }),
    )
    .await;
    assert_eq!(out["exitCode"], 3, "{out}");
    let stdout = out["stdout"].as_str().unwrap();
    assert!(stdout.contains("hello"), "{stdout}");
    assert!(
        stdout.contains("marker.txt"),
        "cwd must default to base dir: {stdout}"
    );
    assert!(out["stderr"].as_str().unwrap().contains("oops"));
    assert_eq!(out["timedOut"], false);

    let executed = sink
        .audit_events()
        .into_iter()
        .find(|e| e.kind == EventKind::CommandExecuted)
        .expect("COMMAND_EXECUTED event");
    assert_eq!(executed.data["exitCode"], 3);
    assert!(executed.data["stdoutTail"]
        .as_str()
        .unwrap()
        .contains("hello"));
}

#[tokio::test(flavor = "multi_thread")]
async fn shell_execute_passes_env_and_stdin() {
    let dir = tempfile::tempdir().unwrap();
    let (runtime, _) = runtime_in(dir.path());
    let command = if cfg!(windows) {
        "echo %ORCH_TEST_VAR%"
    } else {
        "echo $ORCH_TEST_VAR && cat"
    };
    let out = ok(
        &runtime,
        "shell.execute",
        json!({
            "shell": SHELL,
            "command": command,
            "env": {"ORCH_TEST_VAR": "from-env"},
            "stdin": "from-stdin",
        }),
    )
    .await;
    let stdout = out["stdout"].as_str().unwrap();
    assert!(stdout.contains("from-env"), "{stdout}");
    if !cfg!(windows) {
        assert!(stdout.contains("from-stdin"), "{stdout}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn shell_execute_times_out_and_truncates() {
    let dir = tempfile::tempdir().unwrap();
    let (runtime, _) = runtime_in(dir.path());

    let started = Instant::now();
    let out = ok(
        &runtime,
        "shell.execute",
        json!({"shell": SHELL, "command": sleep_command(30), "timeoutMs": 300}),
    )
    .await;
    assert_eq!(out["timedOut"], true);
    assert_eq!(out["exitCode"], Value::Null);
    assert!(started.elapsed() < Duration::from_secs(10));

    let out = ok(
        &runtime,
        "shell.execute",
        json!({"shell": SHELL, "command": "echo 0123456789abcdef", "maxOutputBytes": 4}),
    )
    .await;
    assert_eq!(out["stdout"], "0123");
    assert_eq!(out["stdoutTruncated"], true);
}

#[tokio::test(flavor = "multi_thread")]
async fn missing_working_directory_is_not_found() {
    let dir = tempfile::tempdir().unwrap();
    let (runtime, _) = runtime_in(dir.path());
    let result = call(
        &runtime,
        "shell.execute",
        json!({"command": "echo hi", "cwd": "does/not/exist"}),
    )
    .await;
    assert_eq!(result.error.unwrap().kind, ToolErrorKind::NotFound);
}

#[tokio::test(flavor = "multi_thread")]
async fn process_lifecycle_start_read_list_stop() {
    let dir = tempfile::tempdir().unwrap();
    let (runtime, sink) = runtime_in(dir.path());

    let started = ok(
        &runtime,
        "process.start",
        json!({
            "shell": SHELL,
            "command": format!("echo ready && {}", sleep_command(60)),
            "name": "sleeper",
        }),
    )
    .await;
    let id = started["id"].as_str().unwrap().to_owned();
    assert_eq!(started["status"], "running");
    assert_eq!(started["name"], "sleeper");

    let mut next = 0;
    let seen = {
        let runtime = runtime.clone();
        let id = id.clone();
        let mut data = String::new();
        let deadline = Instant::now() + Duration::from_secs(15);
        while Instant::now() < deadline && !data.contains("ready") {
            let read = ok(&runtime, "process.read", json!({"id": id, "since": next})).await;
            data.push_str(read["data"].as_str().unwrap());
            next = read["next"].as_u64().unwrap();
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        data.contains("ready")
    };
    assert!(seen, "process output must be readable");

    let listed = ok(&runtime, "process.list", json!({})).await;
    assert_eq!(listed.as_array().unwrap().len(), 1);

    let stopped = ok(&runtime, "process.stop", json!({"id": id})).await;
    assert_eq!(stopped["status"], "stopped");

    // Stopping again is idempotent.
    let again = ok(&runtime, "process.stop", json!({"id": id})).await;
    assert_eq!(again["status"], "stopped");

    assert!(
        eventually(Duration::from_secs(5), || {
            sink.audit_events()
                .iter()
                .any(|e| e.kind == EventKind::ProcessExited && e.data["stopped"] == true)
        })
        .await
    );
    let streams = sink.stream_events();
    assert!(streams
        .iter()
        .any(|e| matches!(e, StreamEvent::ProcessOutput { data, .. } if data.contains("ready"))));
    assert!(streams
        .iter()
        .any(|e| matches!(e, StreamEvent::ProcessExited { stopped: true, .. })));
}

#[tokio::test(flavor = "multi_thread")]
async fn process_exit_code_is_recorded() {
    let dir = tempfile::tempdir().unwrap();
    let (runtime, _) = runtime_in(dir.path());
    let started = ok(
        &runtime,
        "process.start",
        json!({"shell": SHELL, "command": "exit 7"}),
    )
    .await;
    let id = started["id"].as_str().unwrap().to_owned();

    let mut last = Value::Null;
    let deadline = Instant::now() + Duration::from_secs(15);
    while Instant::now() < deadline {
        last = ok(&runtime, "process.read", json!({"id": id})).await;
        if last["status"] != "running" {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(last["status"], "exited");
    assert_eq!(last["exitCode"], 7);
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn process_stop_kills_the_whole_tree() {
    let dir = tempfile::tempdir().unwrap();
    let (runtime, _) = runtime_in(dir.path());
    let started = ok(
        &runtime,
        "process.start",
        json!({"shell": "sh", "command": "sleep 300 & echo child=$!; wait"}),
    )
    .await;
    let id = started["id"].as_str().unwrap().to_owned();

    let mut grandchild: Option<i32> = None;
    let deadline = Instant::now() + Duration::from_secs(15);
    while grandchild.is_none() && Instant::now() < deadline {
        let read = ok(&runtime, "process.read", json!({"id": id})).await;
        grandchild = read["data"]
            .as_str()
            .unwrap()
            .lines()
            .find_map(|l| l.trim().strip_prefix("child=")?.parse().ok());
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let grandchild = grandchild.expect("grandchild pid");
    // SAFETY: signal 0 only checks that the process exists.
    assert_eq!(unsafe { libc::kill(grandchild, 0) }, 0);

    ok(&runtime, "process.stop", json!({"id": id})).await;

    let gone = eventually(Duration::from_secs(5), || {
        // SAFETY: signal 0 only checks that the process exists.
        unsafe { libc::kill(grandchild, 0) != 0 }
    })
    .await;
    assert!(
        gone,
        "grandchild {grandchild} must be killed with its parent"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn terminal_runs_commands_in_a_real_pty() {
    let dir = tempfile::tempdir().unwrap();
    let (runtime, sink) = runtime_in(dir.path());

    let created = ok(
        &runtime,
        "terminal.create",
        json!({"shell": SHELL, "cols": 100, "rows": 20}),
    )
    .await;
    let id = created["id"].as_str().unwrap().to_owned();
    assert_eq!(created["alive"], true);
    assert_eq!(created["cols"], 100);

    // The typed text is echoed back by the terminal, so the command computes
    // a value that does not appear literally in the input.
    let command = if cfg!(windows) {
        "echo orchestrator-4^2\r"
    } else {
        "echo orchestrator-$((40+2))\r"
    };
    ok(
        &runtime,
        "terminal.write",
        json!({"id": id, "data": command}),
    )
    .await;

    let mut output = String::new();
    let mut next = 0;
    let deadline = Instant::now() + Duration::from_secs(15);
    while Instant::now() < deadline && !output.contains("orchestrator-42") {
        let read = ok(&runtime, "terminal.read", json!({"id": id, "since": next})).await;
        output.push_str(read["data"].as_str().unwrap());
        next = read["next"].as_u64().unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(
        output.contains("orchestrator-42"),
        "terminal output: {output:?}"
    );

    runtime
        .terminal_resize(&id.clone().into(), 80, 24)
        .expect("resize");
    let listed = ok(&runtime, "terminal.list", json!({})).await;
    assert_eq!(listed[0]["cols"], 80);
    assert_eq!(listed[0]["rows"], 24);

    ok(&runtime, "terminal.close", json!({"id": id})).await;
    let listed = ok(&runtime, "terminal.list", json!({})).await;
    assert_eq!(listed.as_array().unwrap().len(), 0);
    let closed = eventually(Duration::from_secs(10), || {
        sink.audit_events()
            .iter()
            .any(|e| e.kind == EventKind::TerminalExited && e.data["closed"] == true)
    })
    .await;
    assert!(closed, "closing a terminal must be recorded as closed");

    let result = call(&runtime, "terminal.write", json!({"id": id, "data": "x"})).await;
    assert_eq!(result.error.unwrap().kind, ToolErrorKind::NotFound);

    assert!(sink
        .stream_events()
        .iter()
        .any(|e| matches!(e, StreamEvent::TerminalOutput { .. })));
}

#[tokio::test(flavor = "multi_thread")]
async fn terminal_reports_shell_exit() {
    let dir = tempfile::tempdir().unwrap();
    let (runtime, sink) = runtime_in(dir.path());
    let created = ok(&runtime, "terminal.create", json!({"shell": SHELL})).await;
    let id = created["id"].as_str().unwrap().to_owned();

    ok(
        &runtime,
        "terminal.write",
        json!({"id": id, "data": "exit 3\r"}),
    )
    .await;

    let exited = eventually(Duration::from_secs(15), || {
        sink.stream_events().iter().any(|e| {
            matches!(
                e,
                StreamEvent::TerminalExited {
                    exit_code: Some(3),
                    ..
                }
            )
        })
    })
    .await;
    assert!(exited, "terminal exit must be reported with its exit code");

    let read = ok(&runtime, "terminal.read", json!({"id": id})).await;
    assert_eq!(read["alive"], false);
    assert_eq!(read["exitCode"], 3);

    let result = call(&runtime, "terminal.write", json!({"id": id, "data": "x"})).await;
    assert_eq!(result.error.unwrap().kind, ToolErrorKind::NotRunning);
    assert!(sink
        .audit_events()
        .iter()
        .any(|e| e.kind == EventKind::TerminalExited));
}

#[tokio::test(flavor = "multi_thread")]
async fn shutdown_stops_processes_and_terminals() {
    let dir = tempfile::tempdir().unwrap();
    let (runtime, _) = runtime_in(dir.path());
    ok(
        &runtime,
        "process.start",
        json!({"shell": SHELL, "command": sleep_command(60)}),
    )
    .await;
    ok(&runtime, "terminal.create", json!({"shell": SHELL})).await;

    runtime.shutdown().await;

    let processes = ok(&runtime, "process.list", json!({})).await;
    assert_eq!(processes[0]["status"], "stopped");
    let terminals = ok(&runtime, "terminal.list", json!({})).await;
    assert_eq!(terminals.as_array().unwrap().len(), 0);
}
