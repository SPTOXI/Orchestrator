//! HTTP client of the GitHub REST API v3 (ADR-0017).

use super::remote::RepoRef;
use super::types::{
    latest_reviews, number, opt_text, summarize_checks, text, Account, Checks, Comment,
    IssueDetail, IssueSummary, MergeResult, PullDetail, PullSummary, RepoInfo,
};
use super::{GitHubError, GitHubErrorKind, GitHubResult, Secret};
use reqwest::header::{HeaderMap, ACCEPT, AUTHORIZATION, USER_AGENT};
use reqwest::{Method, StatusCode};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::time::Duration;

const API_VERSION: &str = "2022-11-28";
/// Comments shown for a pull request or an issue.
const RECENT_COMMENTS: usize = 20;
/// Largest page GitHub serves.
const MAX_PAGE: u32 = 100;

#[derive(Debug, Clone, Default)]
pub struct PullFilter {
    /// `open` (default), `closed` or `all`.
    pub state: Option<String>,
    /// Branch the changes come from (`branch` or `owner:branch`).
    pub head: Option<String>,
    pub base: Option<String>,
    pub limit: Option<u32>,
}

#[derive(Debug, Clone, Default)]
pub struct IssueFilter {
    /// `open` (default), `closed` or `all`.
    pub state: Option<String>,
    pub labels: Vec<String>,
    pub limit: Option<u32>,
}

#[derive(Debug, Clone)]
pub struct NewPull {
    pub title: String,
    pub body: String,
    /// `branch`, or `owner:branch` for a fork.
    pub head: String,
    pub base: String,
    pub draft: bool,
}

#[derive(Debug, Clone)]
pub struct NewIssue {
    pub title: String,
    pub body: String,
    pub labels: Vec<String>,
}

#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "lowercase")]
pub enum MergeMethod {
    #[default]
    Merge,
    Squash,
    Rebase,
}

impl MergeMethod {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Merge => "merge",
            Self::Squash => "squash",
            Self::Rebase => "rebase",
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct MergeRequest {
    pub method: MergeMethod,
    pub commit_title: Option<String>,
    /// Delete the head branch on GitHub after merging (same repository only).
    pub delete_branch: bool,
}

pub struct GitHubClient {
    http: reqwest::Client,
    api: String,
    token: Secret,
}

impl GitHubClient {
    /// A client of the API at `api_base` (no trailing slash needed).
    pub fn new(api_base: &str, token: Secret) -> GitHubResult<Self> {
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(|err| GitHubError::new(GitHubErrorKind::Network, err.to_string()))?;
        Ok(Self {
            http,
            api: api_base.trim_end_matches('/').to_owned(),
            token,
        })
    }

    async fn send(
        &self,
        method: Method,
        path: &str,
        query: &[(&str, String)],
        body: Option<Value>,
    ) -> GitHubResult<(Value, HeaderMap)> {
        let url = format!("{}{path}", self.api);
        let mut request = self
            .http
            .request(method, &url)
            .header(AUTHORIZATION, format!("Bearer {}", self.token.expose()))
            .header(ACCEPT, "application/vnd.github+json")
            .header("X-GitHub-Api-Version", API_VERSION)
            .header(USER_AGENT, "Orchestrator")
            .query(query);
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request.send().await.map_err(|err| {
            let what = if err.is_timeout() {
                "o GitHub não respondeu a tempo"
            } else {
                "não foi possível falar com o GitHub"
            };
            GitHubError::new(GitHubErrorKind::Network, format!("{what} ({})", self.api))
        })?;
        let status = response.status();
        let headers = response.headers().clone();
        let bytes = response
            .bytes()
            .await
            .map_err(|err| GitHubError::new(GitHubErrorKind::Network, err.to_string()))?;
        let value: Value = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        if status.is_success() {
            Ok((value, headers))
        } else {
            Err(api_error(status, &headers, &value))
        }
    }

    async fn get(&self, path: &str, query: &[(&str, String)]) -> GitHubResult<(Value, HeaderMap)> {
        self.send(Method::GET, path, query, None).await
    }

    fn repo_path(repo: &RepoRef) -> String {
        format!("/repos/{}/{}", repo.owner, repo.name)
    }

    /// The account of the token.
    pub async fn account(&self) -> GitHubResult<Account> {
        let (v, headers) = self.get("/user", &[]).await?;
        let scopes = headers
            .get("x-oauth-scopes")
            .and_then(|h| h.to_str().ok())
            .map(|s| {
                s.split(',')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default();
        Ok(Account {
            login: text(&v, "/login"),
            name: opt_text(&v, "/name"),
            url: text(&v, "/html_url"),
            scopes,
        })
    }

    pub async fn repo(&self, repo: &RepoRef) -> GitHubResult<RepoInfo> {
        let (v, _) = self.get(&Self::repo_path(repo), &[]).await?;
        Ok(RepoInfo::from_api(&v))
    }

    pub async fn pulls(
        &self,
        repo: &RepoRef,
        filter: &PullFilter,
    ) -> GitHubResult<Vec<PullSummary>> {
        let mut query = vec![
            (
                "state",
                filter.state.clone().unwrap_or_else(|| "open".into()),
            ),
            ("per_page", page_size(filter.limit, 30).to_string()),
        ];
        if let Some(head) = filter.head.as_deref().filter(|h| !h.is_empty()) {
            let head = if head.contains(':') {
                head.to_owned()
            } else {
                format!("{}:{head}", repo.owner)
            };
            query.push(("head", head));
        }
        if let Some(base) = filter.base.clone().filter(|b| !b.is_empty()) {
            query.push(("base", base));
        }
        let (v, _) = self
            .get(&format!("{}/pulls", Self::repo_path(repo)), &query)
            .await?;
        Ok(list(&v)
            .iter()
            .map(|p| PullSummary::from_api(p, &repo.owner))
            .collect())
    }

    /// A pull request with its CI, reviews and recent comments.
    pub async fn pull(&self, repo: &RepoRef, pull_number: u64) -> GitHubResult<PullDetail> {
        let base = Self::repo_path(repo);
        let (v, _) = self
            .get(&format!("{base}/pulls/{pull_number}"), &[])
            .await?;
        let summary = PullSummary::from_api(&v, &repo.owner);
        let per_page = [("per_page", MAX_PAGE.to_string())];
        let (reviews, _) = self
            .get(&format!("{base}/pulls/{pull_number}/reviews"), &per_page)
            .await?;
        let comments = self.recent_comments(repo, pull_number).await?;
        let checks = if summary.head_sha.is_empty() {
            summarize_checks(&[], &[])
        } else {
            self.checks(repo, &summary.head_sha).await?
        };
        let merged = summary.state == "merged";
        Ok(PullDetail {
            body: text(&v, "/body"),
            merged,
            merged_at: opt_text(&v, "/merged_at"),
            mergeable: v.get("mergeable").and_then(Value::as_bool),
            mergeable_state: opt_text(&v, "/mergeable_state"),
            commits: number(&v, "/commits"),
            additions: number(&v, "/additions"),
            deletions: number(&v, "/deletions"),
            changed_files: number(&v, "/changed_files"),
            checks,
            reviews: latest_reviews(list(&reviews)),
            comments,
            summary,
        })
    }

    /// The last comments of an issue or pull request, oldest first.
    async fn recent_comments(&self, repo: &RepoRef, issue: u64) -> GitHubResult<Vec<Comment>> {
        let path = format!("{}/issues/{issue}/comments", Self::repo_path(repo));
        let first_page = [("per_page", MAX_PAGE.to_string())];
        let (mut v, headers) = self.get(&path, &first_page).await?;
        if let Some(last) = last_page(&headers).filter(|&last| last > 1) {
            let query = [
                ("per_page", MAX_PAGE.to_string()),
                ("page", last.to_string()),
            ];
            v = self.get(&path, &query).await?.0;
        }
        let all: Vec<Comment> = list(&v).iter().map(Comment::from_api).collect();
        let skip = all.len().saturating_sub(RECENT_COMMENTS);
        Ok(all.into_iter().skip(skip).collect())
    }

    /// CI of a commit, branch or tag.
    pub async fn checks(&self, repo: &RepoRef, reference: &str) -> GitHubResult<Checks> {
        let base = format!("{}/commits/{reference}", Self::repo_path(repo));
        let per_page = [("per_page", MAX_PAGE.to_string())];
        let (runs, _) = self.get(&format!("{base}/check-runs"), &per_page).await?;
        let (status, _) = self.get(&format!("{base}/status"), &per_page).await?;
        let runs = runs
            .get("check_runs")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let statuses = status
            .get("statuses")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        Ok(summarize_checks(&runs, &statuses))
    }

    pub async fn create_pull(&self, repo: &RepoRef, pull: &NewPull) -> GitHubResult<PullSummary> {
        let body = json!({
            "title": pull.title,
            "body": pull.body,
            "head": pull.head,
            "base": pull.base,
            "draft": pull.draft,
        });
        let (v, _) = self
            .send(
                Method::POST,
                &format!("{}/pulls", Self::repo_path(repo)),
                &[],
                Some(body),
            )
            .await?;
        Ok(PullSummary::from_api(&v, &repo.owner))
    }

    /// Comments on an issue or a pull request (same API).
    pub async fn comment(&self, repo: &RepoRef, issue: u64, body: &str) -> GitHubResult<Comment> {
        let (v, _) = self
            .send(
                Method::POST,
                &format!("{}/issues/{issue}/comments", Self::repo_path(repo)),
                &[],
                Some(json!({ "body": body })),
            )
            .await?;
        Ok(Comment::from_api(&v))
    }

    pub async fn merge(
        &self,
        repo: &RepoRef,
        pull_number: u64,
        request: &MergeRequest,
    ) -> GitHubResult<MergeResult> {
        let base = Self::repo_path(repo);
        // The head branch is needed to delete it afterwards.
        let (pull, _) = self
            .get(&format!("{base}/pulls/{pull_number}"), &[])
            .await?;
        let mut body = json!({ "merge_method": request.method.as_str() });
        if let Some(title) = request
            .commit_title
            .as_deref()
            .filter(|t| !t.trim().is_empty())
        {
            body["commit_title"] = json!(title);
        }
        let (v, _) = self
            .send(
                Method::PUT,
                &format!("{base}/pulls/{pull_number}/merge"),
                &[],
                Some(body),
            )
            .await?;
        let mut result = MergeResult {
            merged: v.get("merged").and_then(Value::as_bool).unwrap_or(true),
            sha: text(&v, "/sha"),
            message: text(&v, "/message"),
            branch_deleted: false,
            branch_error: None,
        };
        if request.delete_branch && result.merged {
            let head_owner = text(&pull, "/head/repo/owner/login");
            let head_ref = text(&pull, "/head/ref");
            if head_owner.eq_ignore_ascii_case(&repo.owner) && !head_ref.is_empty() {
                match self
                    .send(
                        Method::DELETE,
                        &format!("{base}/git/refs/heads/{head_ref}"),
                        &[],
                        None,
                    )
                    .await
                {
                    Ok(_) => result.branch_deleted = true,
                    Err(err) => result.branch_error = Some(err.message),
                }
            } else {
                result.branch_error =
                    Some("a branch é de outro repositório (fork): não foi apagada".to_owned());
            }
        }
        Ok(result)
    }

    /// Issues (pull requests, which the API mixes in, are left out).
    pub async fn issues(
        &self,
        repo: &RepoRef,
        filter: &IssueFilter,
    ) -> GitHubResult<Vec<IssueSummary>> {
        let limit = page_size(filter.limit, 30);
        let mut query = vec![
            (
                "state",
                filter.state.clone().unwrap_or_else(|| "open".into()),
            ),
            // Room for the pull requests that will be filtered out.
            ("per_page", MAX_PAGE.to_string()),
        ];
        if !filter.labels.is_empty() {
            query.push(("labels", filter.labels.join(",")));
        }
        let (v, _) = self
            .get(&format!("{}/issues", Self::repo_path(repo)), &query)
            .await?;
        Ok(list(&v)
            .iter()
            .filter(|i| i.get("pull_request").is_none())
            .take(limit as usize)
            .map(IssueSummary::from_api)
            .collect())
    }

    pub async fn issue(&self, repo: &RepoRef, issue: u64) -> GitHubResult<IssueDetail> {
        let (v, _) = self
            .get(&format!("{}/issues/{issue}", Self::repo_path(repo)), &[])
            .await?;
        let comments = self.recent_comments(repo, issue).await?;
        Ok(IssueDetail {
            summary: IssueSummary::from_api(&v),
            body: text(&v, "/body"),
            comments,
        })
    }

    pub async fn create_issue(
        &self,
        repo: &RepoRef,
        issue: &NewIssue,
    ) -> GitHubResult<IssueSummary> {
        let mut body = json!({ "title": issue.title, "body": issue.body });
        if !issue.labels.is_empty() {
            body["labels"] = json!(issue.labels);
        }
        let (v, _) = self
            .send(
                Method::POST,
                &format!("{}/issues", Self::repo_path(repo)),
                &[],
                Some(body),
            )
            .await?;
        Ok(IssueSummary::from_api(&v))
    }
}

fn list(value: &Value) -> &[Value] {
    value.as_array().map(Vec::as_slice).unwrap_or_default()
}

fn page_size(limit: Option<u32>, default: u32) -> u32 {
    limit.unwrap_or(default).clamp(1, MAX_PAGE)
}

/// `page` of the `rel="last"` link, when there are more pages.
fn last_page(headers: &HeaderMap) -> Option<u32> {
    let link = headers.get("link")?.to_str().ok()?;
    link.split(',').find_map(|part| {
        let (url, rel) = part.split_once(';')?;
        if !rel.contains("rel=\"last\"") {
            return None;
        }
        let url = url.trim().trim_start_matches('<').trim_end_matches('>');
        let query = url.split_once('?')?.1;
        query
            .split('&')
            .find_map(|pair| pair.strip_prefix("page="))
            .and_then(|page| page.parse().ok())
    })
}

/// A failed response as a [`GitHubError`] with GitHub's own message.
fn api_error(status: StatusCode, headers: &HeaderMap, body: &Value) -> GitHubError {
    let message = body
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned();
    let details: Vec<String> = body
        .get("errors")
        .and_then(Value::as_array)
        .map(|errors| {
            errors
                .iter()
                .filter_map(|e| {
                    e.get("message")
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                        .or_else(|| {
                            let code = e.get("code").and_then(Value::as_str)?;
                            let field = e.get("field").and_then(Value::as_str).unwrap_or("");
                            Some(format!("{field}: {code}"))
                        })
                })
                .collect()
        })
        .unwrap_or_default();
    let said = if details.is_empty() {
        message.clone()
    } else if message.is_empty() {
        details.join("; ")
    } else {
        format!("{message}: {}", details.join("; "))
    };
    let header = |name: &str| headers.get(name).and_then(|h| h.to_str().ok());
    let rate_limited = header("x-ratelimit-remaining") == Some("0")
        || message.to_ascii_lowercase().contains("rate limit");
    match status.as_u16() {
        401 => GitHubError::new(
            GitHubErrorKind::Unauthorized,
            format!("O GitHub recusou o token (inválido ou expirado): {said}"),
        ),
        403 | 429 if rate_limited => {
            let when = header("x-ratelimit-reset")
                .and_then(|reset| reset.parse::<i64>().ok())
                .and_then(|reset| chrono::DateTime::from_timestamp(reset, 0))
                .map(|at| format!(" Ele volta às {} UTC.", at.format("%H:%M")))
                .or_else(|| header("retry-after").map(|s| format!(" Tente de novo em {s} s.")))
                .unwrap_or_default();
            GitHubError::new(
                GitHubErrorKind::RateLimited,
                format!("Limite de requisições do GitHub atingido.{when}"),
            )
        }
        403 => GitHubError::new(
            GitHubErrorKind::Forbidden,
            format!("O token não tem permissão para isso: {said}"),
        ),
        404 => GitHubError::new(
            GitHubErrorKind::NotFound,
            format!("Não encontrado no GitHub (ou o token não tem acesso): {said}"),
        ),
        405 | 409 | 422 => GitHubError::new(
            GitHubErrorKind::Invalid,
            format!("O GitHub recusou: {said}"),
        ),
        code => GitHubError::new(
            GitHubErrorKind::Server,
            format!("O GitHub respondeu {code}: {said}"),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::header::HeaderValue;

    #[test]
    fn finds_the_last_page() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "link",
            HeaderValue::from_static(
                "<https://api.github.com/repositories/1/issues/2/comments?per_page=100&page=2>; rel=\"next\", \
                 <https://api.github.com/repositories/1/issues/2/comments?per_page=100&page=4>; rel=\"last\"",
            ),
        );
        assert_eq!(last_page(&headers), Some(4));
        assert_eq!(last_page(&HeaderMap::new()), None);
    }

    #[test]
    fn errors_say_what_github_said() {
        let none = HeaderMap::new();
        let err = api_error(
            StatusCode::UNPROCESSABLE_ENTITY,
            &none,
            &serde_json::json!({"message": "Validation Failed",
                "errors": [{"message": "A pull request already exists for time:feat."}]}),
        );
        assert_eq!(err.kind, GitHubErrorKind::Invalid);
        assert!(err.message.contains("already exists"));
        let err = api_error(
            StatusCode::UNPROCESSABLE_ENTITY,
            &none,
            &serde_json::json!({
            "message": "Validation Failed", "errors": [{"resource": "PullRequest", "field": "head", "code": "invalid"}]}),
        );
        assert!(err.message.contains("head: invalid"));
        let mut limited = HeaderMap::new();
        limited.insert("x-ratelimit-remaining", HeaderValue::from_static("0"));
        limited.insert("x-ratelimit-reset", HeaderValue::from_static("1790778600"));
        let err = api_error(
            StatusCode::FORBIDDEN,
            &limited,
            &serde_json::json!({"message": "API rate limit exceeded"}),
        );
        assert_eq!(err.kind, GitHubErrorKind::RateLimited);
        assert!(err.message.contains("UTC"), "{}", err.message);
        assert_eq!(
            api_error(StatusCode::FORBIDDEN, &none, &Value::Null).kind,
            GitHubErrorKind::Forbidden
        );
        assert_eq!(
            api_error(StatusCode::UNAUTHORIZED, &none, &Value::Null).kind,
            GitHubErrorKind::Unauthorized
        );
        assert_eq!(
            api_error(StatusCode::BAD_GATEWAY, &none, &Value::Null).kind,
            GitHubErrorKind::Server
        );
    }
}
