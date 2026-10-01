//! Processes never outlive the app (ADR-0018): on Linux and macOS the next
//! start ends what a crash left behind; on Windows a Job Object ends them
//! with the app (tested in `supervisor.rs`).

#![cfg(unix)]

use orchestrator_core::{CallOrigin, EventKind, MemorySink, ToolCall};
use orchestrator_runtime::{RuntimeConfig, ToolRuntime};
use serde_json::{json, Value};
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

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

async fn start(runtime: &ToolRuntime, command: &str) -> Value {
    let result = runtime
        .invoke(ToolCall::new(
            "process.start",
            json!({"shell": "sh", "command": command}),
            CallOrigin::User,
        ))
        .await;
    assert!(result.ok, "{:?}", result.error);
    result.output
}

fn registered(path: &Path) -> Vec<Value> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

fn alive(pid: u32) -> bool {
    // A zombie still answers `kill(pid, 0)`; `ps` tells it apart.
    std::process::Command::new("ps")
        .args(["-o", "stat=", "-p", &pid.to_string()])
        .output()
        .map(|out| {
            let stat = String::from_utf8_lossy(&out.stdout);
            let stat = stat.trim();
            !stat.is_empty() && !stat.starts_with('Z')
        })
        .unwrap_or(false)
}

async fn eventually(mut condition: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if condition() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    condition()
}

#[tokio::test(flavor = "multi_thread")]
async fn the_next_start_ends_what_a_crash_left_behind() {
    let dir = tempfile::tempdir().unwrap();
    let registry = dir.path().join("data/processes.json");

    // The first run: a server that keeps running when the app dies.
    let (first, _) = runtime_in(dir.path());
    assert!(first.open_process_registry(&registry).is_empty());
    let started = start(&first, "sleep 60").await;
    let pid = started["pid"].as_u64().unwrap() as u32;
    let entries = registered(&registry);
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0]["pid"], pid);
    assert_eq!(entries[0]["command"], "sleep 60");
    // The app dies: no shutdown.
    std::mem::forget(first);
    assert!(alive(pid));

    // The next run ends it and says so.
    let (second, sink) = runtime_in(dir.path());
    let orphans = second.open_process_registry(&registry);
    assert_eq!(orphans.len(), 1);
    assert_eq!(orphans[0].pid, pid);
    assert!(eventually(|| !alive(pid)).await, "the orphan was ended");
    let exited: Vec<_> = sink
        .audit_events()
        .into_iter()
        .filter(|e| e.kind == EventKind::ProcessExited)
        .collect();
    assert_eq!(exited.len(), 1);
    assert_eq!(exited[0].data["reason"], "orphan");
    assert_eq!(exited[0].data["command"], "sleep 60");
    assert!(registered(&registry).is_empty(), "nothing left to reap");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_reused_pid_is_never_killed_and_ended_processes_leave_the_registry() {
    let dir = tempfile::tempdir().unwrap();
    let registry = dir.path().join("processes.json");

    // A process the Orchestrator did not start, under a pid the registry
    // names with another start time.
    let stranger = std::process::Command::new("sleep")
        .arg("30")
        .spawn()
        .unwrap();
    let pid = stranger.id();
    std::fs::write(
        &registry,
        serde_json::to_vec(&json!([{"pid": pid, "identity": "outro processo",
            "command": "sleep 30", "startedAt": "2026-01-01T00:00:00Z"}]))
        .unwrap(),
    )
    .unwrap();
    let (runtime, _) = runtime_in(dir.path());
    assert!(runtime.open_process_registry(&registry).is_empty());
    assert!(alive(pid), "not ours: left alone");
    let mut stranger = stranger;
    stranger.kill().unwrap();
    stranger.wait().unwrap();

    // A process that ends on its own leaves the registry.
    start(&runtime, "sleep 0.2").await;
    assert_eq!(registered(&registry).len(), 1);
    assert!(eventually(|| registered(&registry).is_empty()).await);
}
