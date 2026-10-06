//! `github.*`, `git.remotes` and `git.fetch` through `ToolRuntime::invoke`
//! (ADR-0017), with a real repository and a fake GitHub API.

use orchestrator_core::{CallOrigin, EventKind, MemorySink, ToolCall, ToolErrorKind, ToolResult};
use orchestrator_git::github::{GitHubSettings, Secret};
use orchestrator_runtime::{RuntimeConfig, ToolRuntime};
use parking_lot::Mutex;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::process::Command;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

#[derive(Debug, Clone)]
struct Recorded {
    method: String,
    path: String,
    headers: HashMap<String, String>,
    body: Value,
}

/// A tiny GitHub: one repository `time/app` with pull requests kept in memory.
async fn fake_github() -> (String, Arc<Mutex<Vec<Recorded>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let log: Arc<Mutex<Vec<Recorded>>> = Arc::default();
    let pulls: Arc<Mutex<Vec<Value>>> = Arc::default();
    let requests = log.clone();
    tokio::spawn(async move {
        while let Ok((mut socket, _)) = listener.accept().await {
            let log = log.clone();
            let pulls = pulls.clone();
            tokio::spawn(async move {
                let Some(r) = read_request(&mut socket).await else {
                    return;
                };
                log.lock().push(r.clone());
                let (status, body) = route(&r, &pulls);
                let body = if body.is_null() {
                    String::new()
                } else {
                    body.to_string()
                };
                let head = format!(
                    "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = socket.write_all(head.as_bytes()).await;
                let _ = socket.write_all(body.as_bytes()).await;
                let _ = socket.shutdown().await;
            });
        }
    });
    (url, requests)
}

fn route(r: &Recorded, pulls: &Mutex<Vec<Value>>) -> (u16, Value) {
    let path = r.path.split('?').next().unwrap();
    let query = r.path.split_once('?').map(|(_, q)| q).unwrap_or("");
    match (r.method.as_str(), path) {
        ("GET", "/user") => (
            200,
            json!({"login": "eu", "html_url": "https://github.com/eu"}),
        ),
        ("GET", "/repos/time/app") => (
            200,
            json!({"name": "app", "full_name": "time/app", "owner": {"login": "time"},
                   "html_url": "https://github.com/time/app", "default_branch": "main", "private": false}),
        ),
        ("POST", "/repos/time/app/pulls") => {
            let number = pulls.lock().len() as u64 + 1;
            let pull = json!({
                "number": number, "title": r.body["title"], "state": "open", "draft": r.body["draft"],
                "user": {"login": "eu"}, "html_url": format!("https://github.com/time/app/pull/{number}"),
                "head": {"ref": r.body["head"], "sha": "abc", "repo": {"owner": {"login": "time"}}},
                "base": {"ref": r.body["base"]}
            });
            pulls.lock().push(pull.clone());
            (201, pull)
        }
        ("GET", "/repos/time/app/pulls") => {
            let all = pulls.lock().clone();
            let wanted: Vec<Value> = all
                .into_iter()
                .filter(|p| {
                    !query.contains("head=")
                        || query.contains(&format!(
                            "head=time%3A{}",
                            p["head"]["ref"].as_str().unwrap()
                        ))
                })
                .collect();
            (200, json!(wanted))
        }
        ("GET", p) if p.starts_with("/repos/time/app/pulls/") => {
            let n: usize = p.rsplit('/').next().unwrap().parse().unwrap_or(0);
            match pulls.lock().get(n.wrapping_sub(1)) {
                Some(pull) => (200, pull.clone()),
                None => (404, json!({"message": "Not Found"})),
            }
        }
        ("PUT", p) if p.ends_with("/merge") => {
            (200, json!({"sha": "m1", "merged": true, "message": "ok"}))
        }
        ("DELETE", _) => (204, Value::Null),
        ("GET", p) if p.ends_with("/check-runs") => (
            200,
            json!({"check_runs": [{"name": "ci", "status": "completed", "conclusion": "success"}]}),
        ),
        ("GET", p) if p.ends_with("/status") => (200, json!({"state": "success", "statuses": []})),
        ("POST", "/repos/time/app/issues") => (
            201,
            json!({"number": 40, "title": r.body["title"], "state": "open", "user": {"login": "eu"},
                   "html_url": "https://github.com/time/app/issues/40"}),
        ),
        _ => (404, json!({"message": "Not Found"})),
    }
}

async fn read_request(socket: &mut tokio::net::TcpStream) -> Option<Recorded> {
    let mut buffer = Vec::new();
    let mut chunk = [0u8; 8192];
    let header_end = loop {
        let n = socket.read(&mut chunk).await.ok()?;
        if n == 0 {
            return None;
        }
        buffer.extend_from_slice(&chunk[..n]);
        if let Some(pos) = buffer.windows(4).position(|w| w == b"\r\n\r\n") {
            break pos + 4;
        }
    };
    let head = String::from_utf8_lossy(&buffer[..header_end]).into_owned();
    let mut lines = head.split("\r\n");
    let mut first = lines.next()?.split_whitespace();
    let method = first.next()?.to_owned();
    let path = first.next()?.to_owned();
    let mut headers = HashMap::new();
    for line in lines {
        if let Some((name, value)) = line.split_once(':') {
            headers.insert(name.trim().to_ascii_lowercase(), value.trim().to_owned());
        }
    }
    let length: usize = headers
        .get("content-length")
        .and_then(|l| l.parse().ok())
        .unwrap_or(0);
    let mut body = buffer[header_end..].to_vec();
    while body.len() < length {
        let n = socket.read(&mut chunk).await.ok()?;
        if n == 0 {
            break;
        }
        body.extend_from_slice(&chunk[..n]);
    }
    Some(Recorded {
        method,
        path,
        headers,
        body: serde_json::from_slice(&body).unwrap_or(Value::Null),
    })
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

/// A project whose `origin` is github.com/time/app for reading, but pushes
/// to a local bare repository (so `git push -u` works offline), plus a
/// `backup` remote that fetch can reach.
fn project() -> (tempfile::TempDir, std::path::PathBuf) {
    let root = tempfile::tempdir().unwrap();
    let bare = root.path().join("remote.git");
    let project = root.path().join("app");
    fs::create_dir(&project).unwrap();
    let init = Command::new("git")
        .args(["init", "-q", "--bare"])
        .arg(&bare)
        .output()
        .unwrap();
    assert!(init.status.success());
    sh(&project, &["init", "-q"]);
    sh(&project, &["symbolic-ref", "HEAD", "refs/heads/main"]);
    sh(&project, &["config", "user.name", "Orchestrator Test"]);
    sh(&project, &["config", "user.email", "test@orchestrator.dev"]);
    sh(&project, &["config", "commit.gpgsign", "false"]);
    fs::write(project.join("README.md"), "app\n").unwrap();
    sh(&project, &["add", "."]);
    sh(&project, &["commit", "-q", "-m", "inicial"]);
    sh(
        &project,
        &["remote", "add", "origin", "https://github.com/time/app.git"],
    );
    sh(
        &project,
        &[
            "remote",
            "set-url",
            "--push",
            "origin",
            bare.to_str().unwrap(),
        ],
    );
    sh(
        &project,
        &["remote", "add", "backup", bare.to_str().unwrap()],
    );
    sh(&project, &["checkout", "-q", "-b", "feat"]);
    fs::write(project.join("retry.ts"), "export const retry = 3;\n").unwrap();
    sh(&project, &["add", "."]);
    sh(&project, &["commit", "-q", "-m", "Retentativas no gateway"]);
    (root, project)
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

fn runtime(project: &Path, api: &str) -> (ToolRuntime, Arc<MemorySink>) {
    let sink = Arc::new(MemorySink::new());
    let runtime = ToolRuntime::new(
        RuntimeConfig {
            base_dir: project.to_path_buf(),
        },
        sink.clone(),
    );
    runtime.set_github_settings(GitHubSettings {
        host: "github.com".into(),
        api_url: Some(api.to_owned()),
    });
    runtime.set_github_token(Secret::new("ghp_do_cofre"));
    (runtime, sink)
}

#[tokio::test(flavor = "multi_thread")]
async fn from_a_local_branch_to_a_merged_pull_request() {
    let (api, requests) = fake_github().await;
    let (_root, project) = project();
    let (runtime, sink) = runtime(&project, &api);

    let remotes = ok(&runtime, "git.remotes", json!({})).await;
    let remote = |name: &str| {
        remotes["remotes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["name"] == name)
            .cloned()
            .unwrap()
    };
    assert_eq!(remote("origin")["github"]["owner"], "time");
    assert!(
        remote("backup")["github"].is_null(),
        "backup não é do GitHub"
    );

    let status = ok(&runtime, "github.status", json!({})).await;
    assert_eq!(status["authenticated"], true);
    assert_eq!(status["tokenSource"], "vault");
    assert_eq!(status["account"]["login"], "eu");
    assert_eq!(status["repo"]["fullName"], "time/app");
    assert_eq!(status["remote"], "origin");
    assert_eq!(status["branch"], "feat");
    assert!(status["pull"].is_null());

    // Not on GitHub yet: refused, with the way out.
    let refused = call(
        &runtime,
        "github.pr.create",
        json!({"title": "Retentativas"}),
    )
    .await;
    let error = refused.error.unwrap();
    assert_eq!(error.kind, ToolErrorKind::InvalidArgs);
    assert!(error.message.contains("git.push"), "{}", error.message);

    ok(
        &runtime,
        "git.push",
        json!({"setUpstream": true, "remote": "origin", "branch": "feat"}),
    )
    .await;
    let pull = ok(
        &runtime,
        "github.pr.create",
        json!({"title": "Retentativas", "body": "Três tentativas."}),
    )
    .await;
    assert_eq!(
        (
            pull["number"].as_u64(),
            pull["head"].as_str(),
            pull["base"].as_str()
        ),
        (Some(1), Some("feat"), Some("main"))
    );

    let status = ok(&runtime, "github.status", json!({})).await;
    assert_eq!(status["pull"]["number"], 1);
    assert_eq!(status["checks"]["state"], "success");

    // A new commit that was not pushed blocks the next pull request.
    fs::write(project.join("retry.ts"), "export const retry = 5;\n").unwrap();
    sh(&project, &["commit", "-qam", "Cinco tentativas"]);
    let refused = call(&runtime, "github.pr.create", json!({"title": "Outro"})).await;
    assert!(refused.error.unwrap().message.contains("1 commit(s)"));

    let merged = ok(
        &runtime,
        "github.pr.merge",
        json!({"number": 1, "method": "squash", "deleteBranch": true}),
    )
    .await;
    assert_eq!(
        (
            merged["merged"].as_bool(),
            merged["branchDeleted"].as_bool()
        ),
        (Some(true), Some(true))
    );
    let issue = ok(
        &runtime,
        "github.issue.create",
        json!({"title": "Medir o timeout"}),
    )
    .await;
    assert_eq!(issue["number"], 40);

    // History: what happened on GitHub, and never the token.
    let events = sink.audit_events();
    let of = |kind: EventKind| {
        events
            .iter()
            .filter(|e| e.kind == kind)
            .cloned()
            .collect::<Vec<_>>()
    };
    let created = of(EventKind::GithubPrCreated);
    assert_eq!(created.len(), 1);
    assert_eq!(created[0].data["url"], "https://github.com/time/app/pull/1");
    let merged = of(EventKind::GithubPrMerged);
    assert_eq!(
        (
            merged[0].data["method"].as_str(),
            merged[0].data["title"].as_str()
        ),
        (Some("squash"), Some("Retentativas"))
    );
    assert_eq!(of(EventKind::GithubIssueCreated).len(), 1);
    let history = serde_json::to_string(&events).unwrap();
    assert!(!history.contains("ghp_do_cofre"));

    let requests = requests.lock().clone();
    assert!(requests
        .iter()
        .all(|r| r.headers["authorization"] == "Bearer ghp_do_cofre"));
    let merge = requests.iter().find(|r| r.method == "PUT").unwrap();
    assert_eq!(merge.body["merge_method"], "squash");
    assert!(requests
        .iter()
        .any(|r| r.method == "DELETE" && r.path == "/repos/time/app/git/refs/heads/feat"));
}

#[tokio::test(flavor = "multi_thread")]
async fn arguments_repos_and_fetch() {
    let (api, _) = fake_github().await;
    let (_root, project) = project();
    let (runtime, _) = runtime(&project, &api);

    // An explicit repository needs no remote; a bad one is refused.
    let list = ok(
        &runtime,
        "github.pr.list",
        json!({"repo": "time/app", "state": "all"}),
    )
    .await;
    assert_eq!(list["repo"], "time/app");
    let bad = call(&runtime, "github.pr.list", json!({"repo": "sem-barra"})).await;
    assert_eq!(bad.error.unwrap().kind, ToolErrorKind::InvalidArgs);
    let unknown = call(&runtime, "github.pr.get", json!({"number": 99})).await;
    assert_eq!(unknown.error.unwrap().kind, ToolErrorKind::NotFound);
    let blank = call(
        &runtime,
        "github.pr.comment",
        json!({"number": 1, "body": "  "}),
    )
    .await;
    assert_eq!(blank.error.unwrap().kind, ToolErrorKind::InvalidArgs);

    // A project without a GitHub remote says so.
    sh(&project, &["remote", "remove", "origin"]);
    let none = call(&runtime, "github.pr.list", json!({})).await;
    let error = none.error.unwrap();
    assert_eq!(error.kind, ToolErrorKind::NotFound);
    assert!(error.message.contains("dono/nome"));
    let status = ok(&runtime, "github.status", json!({})).await;
    assert!(status["repoError"]
        .as_str()
        .unwrap()
        .contains("remoto do GitHub"));
    assert_eq!(status["authenticated"], true);

    // Fetch from a remote it can reach.
    let fetched = ok(
        &runtime,
        "git.fetch",
        json!({"remote": "backup", "prune": true}),
    )
    .await;
    assert_eq!(fetched["status"]["branch"], "feat");
    let bad = call(&runtime, "git.fetch", json!({"remote": "--all"})).await;
    assert_eq!(bad.error.unwrap().kind, ToolErrorKind::InvalidArgs);
}
