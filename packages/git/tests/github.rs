//! The GitHub client end to end against a fake REST API (ADR-0017).

use orchestrator_git::github::{
    GitHubClient, GitHubErrorKind, IssueFilter, MergeMethod, MergeRequest, NewIssue, NewPull,
    PullFilter, RepoRef, Secret,
};
use parking_lot::Mutex;
use serde_json::{json, Value};
use std::collections::HashMap;
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

struct Reply {
    status: u16,
    headers: Vec<(&'static str, String)>,
    body: Value,
}

fn ok(body: Value) -> Reply {
    Reply {
        status: 200,
        headers: Vec::new(),
        body,
    }
}

fn status(code: u16, body: Value) -> Reply {
    Reply {
        status: code,
        headers: Vec::new(),
        body,
    }
}

struct FakeGitHub {
    url: String,
    requests: Arc<Mutex<Vec<Recorded>>>,
}

impl FakeGitHub {
    async fn start(handler: impl Fn(&Recorded) -> Reply + Send + Sync + 'static) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let requests: Arc<Mutex<Vec<Recorded>>> = Arc::default();
        let handler = Arc::new(handler);
        let log = requests.clone();
        tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                let handler = handler.clone();
                let log = log.clone();
                tokio::spawn(async move {
                    let Some(request) = read_request(&mut socket).await else {
                        return;
                    };
                    log.lock().push(request.clone());
                    let reply = handler(&request);
                    let body = if reply.body.is_null() {
                        String::new()
                    } else {
                        reply.body.to_string()
                    };
                    let mut head = format!(
                        "HTTP/1.1 {} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n",
                        reply.status,
                        body.len()
                    );
                    for (name, value) in reply.headers {
                        head.push_str(&format!("{name}: {value}\r\n"));
                    }
                    head.push_str("\r\n");
                    let _ = socket.write_all(head.as_bytes()).await;
                    let _ = socket.write_all(body.as_bytes()).await;
                    let _ = socket.shutdown().await;
                });
            }
        });
        Self { url, requests }
    }

    fn requests(&self) -> Vec<Recorded> {
        self.requests.lock().clone()
    }

    fn client(&self) -> GitHubClient {
        GitHubClient::new(&self.url, Secret::new("ghp_teste").unwrap()).unwrap()
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

fn repo() -> RepoRef {
    RepoRef::parse_full("time/app", "github.com").unwrap()
}

fn pull_json(number: u64, state: &str) -> Value {
    json!({
        "number": number, "title": format!("PR {number}"), "state": state, "draft": false,
        "user": {"login": "eu"}, "html_url": format!("https://github.com/time/app/pull/{number}"),
        "head": {"ref": "feat", "sha": "abc123", "repo": {"owner": {"login": "time"}}},
        "base": {"ref": "main"}, "created_at": "2026-09-30T10:00:00Z", "updated_at": "2026-09-30T11:00:00Z",
        "body": "Descrição", "mergeable": true, "mergeable_state": "clean",
        "commits": 2, "additions": 10, "deletions": 3, "changed_files": 1
    })
}

#[tokio::test(flavor = "multi_thread")]
async fn reads_account_repo_and_pull_requests() {
    let api = FakeGitHub::start(|r| match (r.method.as_str(), r.path.split('?').next().unwrap()) {
        ("GET", "/user") => Reply {
            status: 200,
            headers: vec![("X-OAuth-Scopes", "repo, read:org".into())],
            body: json!({"login": "eu", "name": "Eu Mesmo", "html_url": "https://github.com/eu"}),
        },
        ("GET", "/repos/time/app") => ok(json!({
            "name": "app", "full_name": "time/app", "owner": {"login": "time"},
            "html_url": "https://github.com/time/app", "default_branch": "main", "private": true
        })),
        ("GET", "/repos/time/app/pulls") => ok(json!([pull_json(3, "open")])),
        _ => status(404, json!({"message": "Not Found"})),
    })
    .await;
    let client = api.client();

    let account = client.account().await.unwrap();
    assert_eq!(account.login, "eu");
    assert_eq!(account.scopes, vec!["repo", "read:org"]);
    let info = client.repo(&repo()).await.unwrap();
    assert_eq!((info.default_branch.as_str(), info.private), ("main", true));
    let pulls = client
        .pulls(
            &repo(),
            &PullFilter {
                head: Some("feat".into()),
                limit: Some(500),
                ..PullFilter::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(pulls[0].number, 3);
    assert_eq!(pulls[0].head, "feat");

    let requests = api.requests();
    let first = &requests[0];
    assert_eq!(first.headers["authorization"], "Bearer ghp_teste");
    assert_eq!(first.headers["accept"], "application/vnd.github+json");
    assert_eq!(first.headers["x-github-api-version"], "2022-11-28");
    assert_eq!(first.headers["user-agent"], "Orchestrator");
    let list = &requests[2].path;
    assert!(list.contains("head=time%3Afeat"), "{list}");
    assert!(
        list.contains("per_page=100") && list.contains("state=open"),
        "{list}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_pull_request_comes_with_ci_reviews_and_recent_comments() {
    let api = FakeGitHub::start(|r| {
        let path = r.path.split('?').next().unwrap();
        match path {
            "/repos/time/app/pulls/3" => ok(pull_json(3, "open")),
            "/repos/time/app/pulls/3/reviews" => ok(json!([
                {"user": {"login": "ana"}, "state": "CHANGES_REQUESTED", "body": "falta teste"},
                {"user": {"login": "ana"}, "state": "APPROVED", "body": "ok"}
            ])),
            "/repos/time/app/issues/3/comments" if r.path.contains("page=2") => ok(json!([
                {"user": {"login": "bia"}, "body": "último", "html_url": "u2", "created_at": "t2"}
            ])),
            "/repos/time/app/issues/3/comments" => Reply {
                status: 200,
                headers: vec![(
                    "Link",
                    "<http://x/repos/time/app/issues/3/comments?per_page=100&page=2>; rel=\"last\"".into(),
                )],
                body: json!([{"user": {"login": "bia"}, "body": "primeiro", "html_url": "u1", "created_at": "t1"}]),
            },
            "/repos/time/app/commits/abc123/check-runs" => ok(json!({"total_count": 2, "check_runs": [
                {"name": "build", "status": "completed", "conclusion": "success", "html_url": "https://ci/b"},
                {"name": "test", "status": "completed", "conclusion": "failure", "html_url": "https://ci/t",
                 "output": {"title": "2 testes falharam"}}
            ]})),
            "/repos/time/app/commits/abc123/status" => ok(json!({"state": "success", "statuses": []})),
            _ => status(404, json!({"message": "Not Found"})),
        }
    })
    .await;
    let pull = api.client().pull(&repo(), 3).await.unwrap();
    assert_eq!(pull.summary.title, "PR 3");
    assert_eq!(
        (pull.mergeable, pull.mergeable_state.as_deref()),
        (Some(true), Some("clean"))
    );
    assert_eq!(pull.checks.state, "failure");
    assert_eq!((pull.checks.passed, pull.checks.failed), (1, 1));
    assert_eq!(
        pull.checks.items[1].description.as_deref(),
        Some("2 testes falharam")
    );
    assert_eq!(pull.reviews.len(), 1);
    assert_eq!(pull.reviews[0].state, "approved");
    // The last page of comments is the one shown.
    assert_eq!(pull.comments.len(), 1);
    assert_eq!(pull.comments[0].body, "último");
}

#[tokio::test(flavor = "multi_thread")]
async fn creates_comments_merges_and_opens_issues() {
    let api = FakeGitHub::start(|r| match (r.method.as_str(), r.path.split('?').next().unwrap()) {
        ("POST", "/repos/time/app/pulls") => Reply {
            status: 201,
            headers: Vec::new(),
            body: json!({"number": 9, "title": r.body["title"], "state": "open", "draft": r.body["draft"],
                "user": {"login": "eu"}, "html_url": "https://github.com/time/app/pull/9",
                "head": {"ref": r.body["head"], "sha": "def", "repo": {"owner": {"login": "time"}}},
                "base": {"ref": r.body["base"]}}),
        },
        ("POST", "/repos/time/app/issues/9/comments") => Reply {
            status: 201,
            headers: Vec::new(),
            body: json!({"user": {"login": "eu"}, "body": r.body["body"], "html_url": "c", "created_at": "t"}),
        },
        ("GET", "/repos/time/app/pulls/9") => ok(pull_json(9, "open")),
        ("PUT", "/repos/time/app/pulls/9/merge") => {
            ok(json!({"sha": "merged-sha", "merged": true, "message": "Pull Request successfully merged"}))
        }
        ("DELETE", "/repos/time/app/git/refs/heads/feat") => status(204, Value::Null),
        ("GET", "/repos/time/app/issues") => ok(json!([
            {"number": 1, "title": "Bug", "state": "open", "user": {"login": "ana"},
             "labels": [{"name": "bug"}], "comments": 2, "html_url": "i1"},
            {"number": 9, "title": "PR", "state": "open", "pull_request": {"url": "x"}}
        ])),
        ("POST", "/repos/time/app/issues") => Reply {
            status: 201,
            headers: Vec::new(),
            body: json!({"number": 10, "title": r.body["title"], "state": "open",
                "user": {"login": "eu"}, "labels": [{"name": "bug"}], "html_url": "i10"}),
        },
        _ => status(404, json!({"message": "Not Found"})),
    })
    .await;
    let client = api.client();
    let pull = client
        .create_pull(
            &repo(),
            &NewPull {
                title: "Retentativas".into(),
                body: "Corpo".into(),
                head: "feat".into(),
                base: "main".into(),
                draft: true,
            },
        )
        .await
        .unwrap();
    assert_eq!(
        (pull.number, pull.draft, pull.base.as_str()),
        (9, true, "main")
    );
    let comment = client
        .comment(&repo(), 9, "Pronto para revisão")
        .await
        .unwrap();
    assert_eq!(comment.body, "Pronto para revisão");
    let merged = client
        .merge(
            &repo(),
            9,
            &MergeRequest {
                method: MergeMethod::Squash,
                commit_title: Some("Retentativas (#9)".into()),
                delete_branch: true,
            },
        )
        .await
        .unwrap();
    assert!(merged.merged && merged.branch_deleted);
    assert_eq!(merged.sha, "merged-sha");
    let issues = client
        .issues(&repo(), &IssueFilter::default())
        .await
        .unwrap();
    assert_eq!(issues.len(), 1, "o PR não é uma issue");
    assert_eq!(issues[0].labels, vec!["bug"]);
    let issue = client
        .create_issue(
            &repo(),
            &NewIssue {
                title: "Timeout".into(),
                body: String::new(),
                labels: vec!["bug".into()],
            },
        )
        .await
        .unwrap();
    assert_eq!(issue.number, 10);

    let requests = api.requests();
    let create = requests
        .iter()
        .find(|r| r.method == "POST" && r.path == "/repos/time/app/pulls")
        .unwrap();
    assert_eq!(create.body["head"], "feat");
    assert_eq!(create.body["draft"], true);
    let merge = requests.iter().find(|r| r.method == "PUT").unwrap();
    assert_eq!(
        merge.body,
        json!({"merge_method": "squash", "commit_title": "Retentativas (#9)"})
    );
    assert!(requests.iter().any(|r| r.method == "DELETE"));
    let new_issue = requests.iter().rfind(|r| r.method == "POST").unwrap();
    assert_eq!(new_issue.body["labels"], json!(["bug"]));
}

#[tokio::test(flavor = "multi_thread")]
async fn failures_carry_githubs_reason() {
    let api = FakeGitHub::start(|r| match r.path.split('?').next().unwrap() {
        "/user" => status(401, json!({"message": "Bad credentials"})),
        "/repos/time/app" => status(404, json!({"message": "Not Found"})),
        "/repos/time/app/pulls" => status(
            422,
            json!({"message": "Validation Failed", "errors": [{"message": "No commits between main and feat"}]}),
        ),
        _ => Reply {
            status: 403,
            headers: vec![("x-ratelimit-remaining", "0".into()), ("x-ratelimit-reset", "1790778600".into())],
            body: json!({"message": "API rate limit exceeded"}),
        },
    })
    .await;
    let client = api.client();
    assert_eq!(
        client.account().await.unwrap_err().kind,
        GitHubErrorKind::Unauthorized
    );
    assert_eq!(
        client.repo(&repo()).await.unwrap_err().kind,
        GitHubErrorKind::NotFound
    );
    let err = client
        .create_pull(
            &repo(),
            &NewPull {
                title: "x".into(),
                body: String::new(),
                head: "feat".into(),
                base: "main".into(),
                draft: false,
            },
        )
        .await
        .unwrap_err();
    assert_eq!(err.kind, GitHubErrorKind::Invalid);
    assert!(
        err.message.contains("No commits between"),
        "{}",
        err.message
    );
    assert_eq!(
        client
            .issues(&repo(), &IssueFilter::default())
            .await
            .unwrap_err()
            .kind,
        GitHubErrorKind::RateLimited
    );

    // Nothing listening: a network error, never a panic, never the token.
    let closed =
        GitHubClient::new("http://127.0.0.1:9", Secret::new("ghp_segredo").unwrap()).unwrap();
    let err = closed.account().await.unwrap_err();
    assert_eq!(err.kind, GitHubErrorKind::Network);
    assert!(!err.message.contains("ghp_segredo"));
}
