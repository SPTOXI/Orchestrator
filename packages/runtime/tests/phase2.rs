//! Phase 2 tools through `ToolRuntime::invoke`: project discovery/profile/
//! open, git.*, package.* and runtime.*.

use orchestrator_core::{CallOrigin, EventKind, MemorySink, ToolCall, ToolErrorKind, ToolResult};
use orchestrator_runtime::{RuntimeConfig, ToolRuntime};
use serde_json::{json, Value};
use std::fs;
use std::path::Path;
use std::process::Command;
use std::sync::Arc;

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

fn sh(dir: &Path, args: &[&str]) {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .expect("git");
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn init_repo(dir: &Path) {
    sh(dir, &["init", "-q"]);
    sh(dir, &["symbolic-ref", "HEAD", "refs/heads/main"]);
    sh(dir, &["config", "user.name", "Orchestrator Test"]);
    sh(dir, &["config", "user.email", "test@orchestrator.dev"]);
    sh(dir, &["config", "commit.gpgsign", "false"]);
    sh(dir, &["config", "core.autocrlf", "false"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn open_project_becomes_the_base_directory() {
    let home = tempfile::tempdir().unwrap();
    let project = home.path().join("meu-saas");
    fs::create_dir(&project).unwrap();
    fs::write(
        project.join("package.json"),
        r#"{"dependencies":{"next":"15"},"scripts":{"dev":"next dev"}}"#,
    )
    .unwrap();
    fs::write(project.join("pnpm-lock.yaml"), "").unwrap();
    init_repo(&project);

    let (runtime, sink) = runtime_in(home.path());
    let profile = ok(&runtime, "project.open", json!({"path": "meu-saas"})).await;
    assert_eq!(profile["name"], "meu-saas");
    assert_eq!(profile["packageManagers"][0], "pnpm");
    assert_eq!(profile["frameworks"][0], "Next.js");
    assert_eq!(profile["git"]["branch"], "main");
    assert_eq!(runtime.base_dir(), project);

    // Relative paths now resolve inside the project.
    ok(
        &runtime,
        "filesystem.write",
        json!({"path": "src/index.ts", "content": "export {}"}),
    )
    .await;
    assert!(project.join("src/index.ts").is_file());

    // project.profile defaults to the open project and sees the new file in git.
    let again = ok(&runtime, "project.profile", json!({})).await;
    assert_eq!(again["path"], project.display().to_string());
    assert!(again["git"]["untracked"].as_u64().unwrap() >= 1);

    let events = sink.audit_events();
    let opened = events
        .iter()
        .find(|e| e.kind == EventKind::ProjectOpened)
        .expect("PROJECT_OPENED");
    assert_eq!(opened.data["name"], "meu-saas");
    assert_eq!(opened.data["branch"], "main");
}

#[tokio::test(flavor = "multi_thread")]
async fn open_rejects_missing_folders_and_keeps_the_base() {
    let home = tempfile::tempdir().unwrap();
    let (runtime, _) = runtime_in(home.path());
    let result = call(&runtime, "project.open", json!({"path": "nao-existe"})).await;
    assert_eq!(result.error.unwrap().kind, ToolErrorKind::NotFound);
    assert_eq!(runtime.base_dir(), home.path());
}

#[tokio::test(flavor = "multi_thread")]
async fn discover_lists_projects_under_roots() {
    let home = tempfile::tempdir().unwrap();
    for (dir, marker) in [
        ("a/api", "go.mod"),
        ("a/web", "package.json"),
        ("b/lib", "Cargo.toml"),
    ] {
        let path = home.path().join(dir);
        fs::create_dir_all(&path).unwrap();
        fs::write(path.join(marker), "").unwrap();
    }
    let (runtime, _) = runtime_in(home.path());
    let out = ok(
        &runtime,
        "project.discover",
        json!({"roots": [home.path().display().to_string()], "maxDepth": 3}),
    )
    .await;
    let names: Vec<_> = out["projects"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["name"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(names, vec!["api", "web", "lib"]);
    assert_eq!(out["truncated"], false);
}

#[tokio::test(flavor = "multi_thread")]
async fn git_workflow_through_tools_records_commit_and_push() {
    let remote = tempfile::tempdir().unwrap();
    sh(remote.path(), &["init", "-q", "--bare"]);
    let repo = tempfile::tempdir().unwrap();
    let root = repo.path();
    init_repo(root);
    sh(
        root,
        &[
            "remote",
            "add",
            "origin",
            &remote.path().display().to_string(),
        ],
    );
    let (runtime, sink) = runtime_in(root);

    fs::write(root.join("README.md"), "# demo\n").unwrap();
    let status = ok(&runtime, "git.status", json!({})).await;
    assert_eq!(status["branch"], "main");
    assert_eq!(status["files"][0]["unstaged"], "untracked");
    assert_eq!(status["remotes"][0]["name"], "origin");

    let staged = ok(&runtime, "git.add", json!({"all": true})).await;
    assert_eq!(staged["files"][0]["staged"], "added");

    let commit = ok(
        &runtime,
        "git.commit",
        json!({"message": "Primeiro commit"}),
    )
    .await;
    assert_eq!(commit["subject"], "Primeiro commit");
    assert_eq!(commit["branch"], "main");

    fs::write(root.join("README.md"), "# demo\nmais\n").unwrap();
    let diff = ok(&runtime, "git.diff", json!({})).await;
    assert!(diff["patch"].as_str().unwrap().contains("+mais"));
    assert_eq!(diff["files"][0]["additions"], 1);

    let stash = ok(
        &runtime,
        "git.stash",
        json!({"action": "push", "message": "wip"}),
    )
    .await;
    assert_eq!(stash["stashes"].as_array().unwrap().len(), 1);
    let popped = ok(&runtime, "git.stash", json!({"action": "pop"})).await;
    assert!(popped["stashes"].as_array().unwrap().is_empty());

    let reset = ok(&runtime, "git.reset", json!({"mode": "hard"})).await;
    assert_eq!(reset["status"]["clean"], true);

    let branches = ok(&runtime, "git.branch", json!({"create": "feature/x"})).await;
    assert!(branches
        .as_array()
        .unwrap()
        .iter()
        .any(|b| b["name"] == "feature/x"));
    let checkout = ok(&runtime, "git.checkout", json!({"target": "feature/x"})).await;
    assert_eq!(checkout["status"]["branch"], "feature/x");
    ok(&runtime, "git.checkout", json!({"target": "main"})).await;

    let pushed = ok(
        &runtime,
        "git.push",
        json!({"remote": "origin", "branch": "main", "setUpstream": true}),
    )
    .await;
    assert_eq!(pushed["status"]["upstream"], "origin/main");

    let log = ok(&runtime, "git.log", json!({"limit": 5})).await;
    assert_eq!(log[0]["subject"], "Primeiro commit");

    let events = sink.audit_events();
    let commit_event = events
        .iter()
        .find(|e| e.kind == EventKind::GitCommit)
        .expect("GIT_COMMIT");
    assert_eq!(commit_event.data["subject"], "Primeiro commit");
    let push_event = events
        .iter()
        .find(|e| e.kind == EventKind::GitPush)
        .expect("GIT_PUSH");
    assert_eq!(push_event.data["upstream"], "origin/main");

    // TOOL_CALLED marks queries and actions.
    let flag = |tool: &str| {
        events
            .iter()
            .find(|e| e.kind == EventKind::ToolCalled && e.data["tool"] == tool)
            .map(|e| e.data["readOnly"].clone())
            .unwrap()
    };
    assert_eq!(flag("git.status"), true);
    assert_eq!(flag("git.commit"), false);
}

#[tokio::test(flavor = "multi_thread")]
async fn git_errors_are_typed() {
    let dir = tempfile::tempdir().unwrap();
    let (runtime, _) = runtime_in(dir.path());
    let not_repo = call(&runtime, "git.status", json!({})).await;
    assert_eq!(not_repo.error.unwrap().kind, ToolErrorKind::NotFound);

    init_repo(dir.path());
    let nothing = call(&runtime, "git.commit", json!({"message": "vazio"})).await;
    assert_eq!(nothing.error.unwrap().kind, ToolErrorKind::CommandFailed);
    let empty = call(&runtime, "git.commit", json!({"message": ""})).await;
    assert_eq!(empty.error.unwrap().kind, ToolErrorKind::InvalidArgs);
    let bad_ref = call(&runtime, "git.checkout", json!({"target": "--orphan"})).await;
    assert_eq!(bad_ref.error.unwrap().kind, ToolErrorKind::InvalidArgs);
}

#[tokio::test(flavor = "multi_thread")]
async fn package_run_uses_the_detected_manager() {
    if which::which("npm").is_err() {
        eprintln!("npm not installed: package.run integration not exercised here");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    fs::write(
        dir.path().join("package.json"),
        r#"{"name":"t","version":"1.0.0","scripts":{"hello":"node -e \"console.log('ola-do-script')\""}}"#,
    )
    .unwrap();
    fs::write(dir.path().join("package-lock.json"), "{}").unwrap();
    let (runtime, sink) = runtime_in(dir.path());

    let out = ok(&runtime, "package.run", json!({"script": "hello"})).await;
    assert_eq!(out["manager"], "npm");
    assert_eq!(out["result"]["exitCode"], 0, "{out}");
    assert!(out["result"]["stdout"]
        .as_str()
        .unwrap()
        .contains("ola-do-script"));
    assert!(sink
        .audit_events()
        .iter()
        .any(|e| e.kind == EventKind::CommandExecuted));
}

#[tokio::test(flavor = "multi_thread")]
async fn package_tools_need_a_manager() {
    let dir = tempfile::tempdir().unwrap();
    let (runtime, _) = runtime_in(dir.path());
    let result = call(&runtime, "package.install", json!({})).await;
    assert_eq!(result.error.unwrap().kind, ToolErrorKind::NotFound);
    let unsupported = call(&runtime, "package.install", json!({"manager": "gradle"})).await;
    assert_eq!(unsupported.error.unwrap().kind, ToolErrorKind::InvalidArgs);
}

#[tokio::test(flavor = "multi_thread")]
async fn runtime_probes_report_structure() {
    let dir = tempfile::tempdir().unwrap();
    let (runtime, _) = runtime_in(dir.path());
    let node = ok(&runtime, "runtime.node", json!({})).await;
    assert!(node["available"].is_boolean());
    assert!(node["managers"].get("npm").is_some());
    if node["available"] == true {
        assert!(node["version"].as_str().unwrap().starts_with('v'));
    }
    let docker = ok(&runtime, "runtime.docker", json!({})).await;
    assert!(docker["daemonRunning"].is_boolean());
    let python = ok(&runtime, "runtime.python", json!({})).await;
    assert!(python["available"].is_boolean());
}
