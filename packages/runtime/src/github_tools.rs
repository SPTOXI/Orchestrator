//! `github.*` tools and `git.remotes`/`git.fetch` over `orchestrator-git`
//! (ADR-0017). Every GitHub tool takes an optional `path` (a folder of the
//! project, default: the open project) and `repo` (`owner/name`, default:
//! the project's GitHub remote).

use crate::platform::resolve_cwd;
use crate::{blocking, git_tools, parse, Dispatched, ToolRuntime};
use orchestrator_core::{AuditEvent, CallOrigin, EventKind, ToolCall, ToolError, ToolErrorKind};
use orchestrator_git::github::{
    pick_remote, resolve_token, Account, Checks, Credential, GitHubClient, GitHubError,
    GitHubErrorKind, GitHubSettings, IssueFilter, MergeMethod, MergeRequest, NewIssue, NewPull,
    PullFilter, PullSummary, RepoInfo, RepoRef, Secret,
};
use orchestrator_git::{Git, Status};
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// A resolved token is reused this long (the GitHub CLI is a process).
const CREDENTIAL_TTL: Duration = Duration::from_secs(60);

/// Settings, the token saved in the vault (handed in by the app) and the
/// credential in use.
#[derive(Default)]
pub(crate) struct GitHubState {
    settings: RwLock<GitHubSettings>,
    vault: RwLock<Option<Secret>>,
    cached: RwLock<Option<(Credential, Instant)>>,
}

impl GitHubState {
    pub(crate) fn settings(&self) -> GitHubSettings {
        self.settings.read().clone()
    }

    pub(crate) fn set_settings(&self, settings: GitHubSettings) {
        *self.settings.write() = settings;
        self.forget();
    }

    pub(crate) fn set_vault(&self, token: Option<Secret>) {
        *self.vault.write() = token;
        self.forget();
    }

    fn forget(&self) {
        *self.cached.write() = None;
    }

    /// The credential in use (cached for a minute), or `None`.
    async fn credential(&self) -> Result<Option<Credential>, ToolError> {
        if let Some((credential, at)) = self.cached.read().as_ref() {
            if at.elapsed() < CREDENTIAL_TTL {
                return Ok(Some(credential.clone()));
            }
        }
        let vault = self.vault.read().clone();
        let host = self.settings().host();
        let found = blocking(move || Ok(resolve_token(vault.as_ref(), &host))).await?;
        *self.cached.write() = found.clone().map(|c| (c, Instant::now()));
        Ok(found)
    }

    async fn client(&self) -> Result<(GitHubClient, Credential), ToolError> {
        let credential = self
            .credential()
            .await?
            .ok_or_else(|| map_error(GitHubError::no_token()))?;
        let client = GitHubClient::new(&self.settings().api_base(), credential.token.clone())
            .map_err(map_error)?;
        Ok((client, credential))
    }

    /// A 401 means the cached token no longer works.
    fn check(&self, err: GitHubError) -> ToolError {
        if err.kind == GitHubErrorKind::Unauthorized {
            self.forget();
        }
        map_error(err)
    }
}

pub fn map_error(err: GitHubError) -> ToolError {
    let kind = match err.kind {
        GitHubErrorKind::NoToken | GitHubErrorKind::Unauthorized | GitHubErrorKind::Forbidden => {
            ToolErrorKind::PermissionDenied
        }
        GitHubErrorKind::NotFound => ToolErrorKind::NotFound,
        GitHubErrorKind::Invalid => ToolErrorKind::InvalidArgs,
        GitHubErrorKind::RateLimited | GitHubErrorKind::Network | GitHubErrorKind::Server => {
            ToolErrorKind::CommandFailed
        }
    };
    ToolError::new(kind, err.message)
}

fn invalid(message: impl Into<String>) -> ToolError {
    ToolError::new(ToolErrorKind::InvalidArgs, message.into())
}

// ------------------------------------------------------------- arguments --

#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RemotesArgs {
    /// Repository folder (default: the open project).
    #[serde(default)]
    pub path: Option<String>,
}

#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FetchArgs {
    /// Repository folder (default: the open project).
    #[serde(default)]
    pub path: Option<String>,
    /// Remote to fetch (default: the upstream of the current branch, or origin).
    #[serde(default)]
    pub remote: Option<String>,
    /// Remove remote-tracking references that no longer exist on the remote.
    #[serde(default)]
    pub prune: bool,
    #[serde(default)]
    pub timeout_ms: Option<u64>,
}

#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StatusArgs {
    /// Project folder (default: the open project).
    #[serde(default)]
    pub path: Option<String>,
    /// Repository `owner/name` (default: the project's GitHub remote).
    #[serde(default)]
    pub repo: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum StateFilter {
    #[default]
    Open,
    Closed,
    All,
}

impl StateFilter {
    fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Closed => "closed",
            Self::All => "all",
        }
    }
}

#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PrListArgs {
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub repo: Option<String>,
    /// open (default), closed or all.
    #[serde(default)]
    pub state: StateFilter,
    /// Only pull requests from this branch (`branch` or `owner:branch`).
    #[serde(default)]
    pub head: Option<String>,
    /// Only pull requests into this branch.
    #[serde(default)]
    pub base: Option<String>,
    /// At most this many (default 30, max 100).
    #[serde(default)]
    pub limit: Option<u32>,
}

#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NumberArgs {
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub repo: Option<String>,
    /// Pull request or issue number.
    pub number: u64,
}

#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ChecksArgs {
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub repo: Option<String>,
    /// Commit, branch or tag (default: the current branch as pushed).
    #[serde(default, rename = "ref")]
    pub reference: Option<String>,
}

#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PrCreateArgs {
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub repo: Option<String>,
    pub title: String,
    #[serde(default)]
    pub body: Option<String>,
    /// Branch to merge into (default: the repository's default branch).
    #[serde(default)]
    pub base: Option<String>,
    /// Branch with the changes (default: the current branch, which must
    /// be pushed; `owner:branch` for a fork).
    #[serde(default)]
    pub head: Option<String>,
    #[serde(default)]
    pub draft: bool,
}

#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CommentArgs {
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub repo: Option<String>,
    /// Pull request or issue number.
    pub number: u64,
    pub body: String,
}

#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PrMergeArgs {
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub repo: Option<String>,
    pub number: u64,
    /// merge (default), squash or rebase.
    #[serde(default)]
    pub method: MergeMethod,
    #[serde(default)]
    pub commit_title: Option<String>,
    /// Delete the branch on GitHub after merging (same repository only).
    #[serde(default)]
    pub delete_branch: bool,
}

#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct IssueListArgs {
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub repo: Option<String>,
    /// open (default), closed or all.
    #[serde(default)]
    pub state: StateFilter,
    /// Only issues with all these labels.
    #[serde(default)]
    pub labels: Vec<String>,
    /// At most this many (default 30, max 100).
    #[serde(default)]
    pub limit: Option<u32>,
}

#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct IssueCreateArgs {
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub repo: Option<String>,
    pub title: String,
    #[serde(default)]
    pub body: Option<String>,
    #[serde(default)]
    pub labels: Vec<String>,
}

// --------------------------------------------------------------- outputs --

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteInfo {
    pub name: String,
    pub url: String,
    /// The GitHub repository this remote is, on the configured host.
    pub github: Option<RepoRef>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemotesOutput {
    pub remotes: Vec<RemoteInfo>,
}

/// `github.status`: what the GitHub section of the app shows. Problems go
/// in the `*Error` fields, so the call itself only fails on bad arguments.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StatusOutput {
    pub host: String,
    pub api_url: String,
    pub authenticated: bool,
    /// `vault`, `env:GH_TOKEN`, `env:GITHUB_TOKEN` or `gh`.
    pub token_source: Option<String>,
    pub account: Option<Account>,
    pub account_error: Option<String>,
    pub repo: Option<RepoInfo>,
    pub repo_ref: Option<RepoRef>,
    /// Local remote that stands for the repository.
    pub remote: Option<String>,
    pub repo_error: Option<String>,
    pub branch: Option<String>,
    pub upstream: Option<String>,
    pub ahead: u32,
    pub behind: u32,
    /// Most recent pull request of the current branch (open, merged or
    /// closed).
    pub pull: Option<PullSummary>,
    /// CI of that pull request.
    pub checks: Option<Checks>,
}

// ------------------------------------------------------------ resolution --

/// The repository a call is about, and the local state around it.
struct Target {
    repo: RepoRef,
    /// Local status, when the repository came from the project.
    status: Option<Status>,
    /// `(remote, url)` of the project.
    remotes: Vec<(String, String)>,
}

/// Local status and `(remote, url)` pairs of a repository.
type Local = (Status, Vec<(String, String)>);

fn upstream_remote(status: &Status) -> Option<&str> {
    status
        .upstream
        .as_deref()
        .and_then(|u| u.split_once('/'))
        .map(|(remote, _)| remote)
}

fn local(git: Git, dir: PathBuf) -> impl FnOnce() -> Result<Local, ToolError> {
    move || {
        let status = git.status(&dir).map_err(git_tools::map_error)?;
        let remotes = git
            .remotes(&dir)
            .map_err(git_tools::map_error)?
            .into_iter()
            .map(|r| (r.name, r.url))
            .collect();
        Ok((status, remotes))
    }
}

impl ToolRuntime {
    async fn github_target(
        &self,
        base: &Path,
        path: Option<&str>,
        repo: Option<&str>,
    ) -> Result<Target, ToolError> {
        let host = self.inner.github.settings().host();
        let explicit = match repo.map(str::trim).filter(|r| !r.is_empty()) {
            Some(full) => Some(
                RepoRef::parse_full(full, &host)
                    .ok_or_else(|| invalid(format!("repo inválido: \"{full}\" (use dono/nome)")))?,
            ),
            None => None,
        };
        let project = match (resolve_cwd(base, path), self.git()) {
            (Ok(dir), Ok(git)) => blocking(local(git, dir)).await.ok(),
            _ => None,
        };
        let (status, remotes) = match project {
            Some((status, remotes)) => (Some(status), remotes),
            None => (None, Vec::new()),
        };
        let repo = match explicit {
            Some(repo) => repo,
            None => {
                let upstream = status.as_ref().and_then(upstream_remote);
                pick_remote(&remotes, upstream, &host)
                    .map(|(_, repo)| repo)
                    .ok_or_else(|| {
                        ToolError::new(
                            ToolErrorKind::NotFound,
                            format!(
                                "O projeto não tem um remoto do GitHub ({host}). Informe repo: \"dono/nome\", \
                                 ou adicione o remoto (git remote add origin …)."
                            ),
                        )
                    })?
            }
        };
        Ok(Target {
            repo,
            status,
            remotes,
        })
    }

    pub(crate) async fn github_dispatch(
        &self,
        call: &ToolCall,
        base: &Path,
    ) -> Result<Dispatched, ToolError> {
        let github = &self.inner.github;
        let origin = &call.origin;
        match call.tool.as_str() {
            "github.status" => {
                let args: StatusArgs = parse(&call.args)?;
                Dispatched::new(&self.github_status(base, args).await?)
            }
            "github.pr.list" => {
                let args: PrListArgs = parse(&call.args)?;
                let target = self
                    .github_target(base, args.path.as_deref(), args.repo.as_deref())
                    .await?;
                let (client, _) = github.client().await?;
                let filter = PullFilter {
                    state: Some(args.state.as_str().to_owned()),
                    head: args.head,
                    base: args.base,
                    limit: args.limit,
                };
                let pulls = client
                    .pulls(&target.repo, &filter)
                    .await
                    .map_err(|e| github.check(e))?;
                Dispatched::new(&json!({ "repo": target.repo.full_name(), "pulls": pulls }))
            }
            "github.pr.get" => {
                let args: NumberArgs = parse(&call.args)?;
                let target = self
                    .github_target(base, args.path.as_deref(), args.repo.as_deref())
                    .await?;
                let (client, _) = github.client().await?;
                let pull = client
                    .pull(&target.repo, args.number)
                    .await
                    .map_err(|e| github.check(e))?;
                Dispatched::new(&pull)
            }
            "github.checks" => {
                let args: ChecksArgs = parse(&call.args)?;
                let target = self
                    .github_target(base, args.path.as_deref(), args.repo.as_deref())
                    .await?;
                let reference = match args.reference.filter(|r| !r.trim().is_empty()) {
                    Some(reference) => reference,
                    None => pushed_ref(target.status.as_ref())?,
                };
                let (client, _) = github.client().await?;
                let checks = client
                    .checks(&target.repo, &reference)
                    .await
                    .map_err(|e| github.check(e))?;
                Dispatched::new(&json!({ "ref": reference, "checks": checks }))
            }
            "github.pr.create" => {
                let args: PrCreateArgs = parse(&call.args)?;
                let title = args.title.trim().to_owned();
                if title.is_empty() {
                    return Err(invalid("o título do pull request é obrigatório"));
                }
                let target = self
                    .github_target(base, args.path.as_deref(), args.repo.as_deref())
                    .await?;
                let head = match args
                    .head
                    .map(|h| h.trim().to_owned())
                    .filter(|h| !h.is_empty())
                {
                    Some(head) => head,
                    None => pushed_head(&target)?,
                };
                let (client, _) = github.client().await?;
                let base_branch = match args
                    .base
                    .map(|b| b.trim().to_owned())
                    .filter(|b| !b.is_empty())
                {
                    Some(base) => base,
                    None => {
                        client
                            .repo(&target.repo)
                            .await
                            .map_err(|e| github.check(e))?
                            .default_branch
                    }
                };
                let pull = client
                    .create_pull(
                        &target.repo,
                        &NewPull {
                            title,
                            body: args.body.unwrap_or_default(),
                            head,
                            base: base_branch,
                            draft: args.draft,
                        },
                    )
                    .await
                    .map_err(|e| github.check(e))?;
                let event = pr_created(origin, &target.repo, &pull);
                Dispatched::with_events(&pull, vec![event])
            }
            "github.pr.comment" | "github.issue.comment" => {
                let args: CommentArgs = parse(&call.args)?;
                if args.body.trim().is_empty() {
                    return Err(invalid("o comentário está vazio"));
                }
                let target = self
                    .github_target(base, args.path.as_deref(), args.repo.as_deref())
                    .await?;
                let (client, _) = github.client().await?;
                let comment = client
                    .comment(&target.repo, args.number, &args.body)
                    .await
                    .map_err(|e| github.check(e))?;
                Dispatched::new(&comment)
            }
            "github.pr.merge" => {
                let args: PrMergeArgs = parse(&call.args)?;
                let target = self
                    .github_target(base, args.path.as_deref(), args.repo.as_deref())
                    .await?;
                let (client, _) = github.client().await?;
                let request = MergeRequest {
                    method: args.method,
                    commit_title: args.commit_title,
                    delete_branch: args.delete_branch,
                };
                let merged = client
                    .merge(&target.repo, args.number, &request)
                    .await
                    .map_err(|e| github.check(e))?;
                let event = AuditEvent::new(
                    EventKind::GithubPrMerged,
                    origin.clone(),
                    format!(
                        "merge ({}) do PR #{} em {}: {}",
                        args.method.as_str(),
                        merged.number,
                        target.repo.full_name(),
                        merged.title
                    ),
                    json!({
                        "repo": target.repo.full_name(),
                        "number": merged.number,
                        "title": merged.title,
                        "url": merged.url,
                        "method": args.method.as_str(),
                        "sha": merged.sha,
                        "branchDeleted": merged.branch_deleted,
                    }),
                );
                Dispatched::with_events(&merged, vec![event])
            }
            "github.issue.list" => {
                let args: IssueListArgs = parse(&call.args)?;
                let target = self
                    .github_target(base, args.path.as_deref(), args.repo.as_deref())
                    .await?;
                let (client, _) = github.client().await?;
                let filter = IssueFilter {
                    state: Some(args.state.as_str().to_owned()),
                    labels: args.labels,
                    limit: args.limit,
                };
                let issues = client
                    .issues(&target.repo, &filter)
                    .await
                    .map_err(|e| github.check(e))?;
                Dispatched::new(&json!({ "repo": target.repo.full_name(), "issues": issues }))
            }
            "github.issue.get" => {
                let args: NumberArgs = parse(&call.args)?;
                let target = self
                    .github_target(base, args.path.as_deref(), args.repo.as_deref())
                    .await?;
                let (client, _) = github.client().await?;
                let issue = client
                    .issue(&target.repo, args.number)
                    .await
                    .map_err(|e| github.check(e))?;
                Dispatched::new(&issue)
            }
            "github.issue.create" => {
                let args: IssueCreateArgs = parse(&call.args)?;
                let title = args.title.trim().to_owned();
                if title.is_empty() {
                    return Err(invalid("o título da issue é obrigatório"));
                }
                let target = self
                    .github_target(base, args.path.as_deref(), args.repo.as_deref())
                    .await?;
                let (client, _) = github.client().await?;
                let issue = client
                    .create_issue(
                        &target.repo,
                        &NewIssue {
                            title,
                            body: args.body.unwrap_or_default(),
                            labels: args.labels,
                        },
                    )
                    .await
                    .map_err(|e| github.check(e))?;
                let event = AuditEvent::new(
                    EventKind::GithubIssueCreated,
                    origin.clone(),
                    format!(
                        "issue #{} aberta em {}: {}",
                        issue.number,
                        target.repo.full_name(),
                        issue.title
                    ),
                    json!({
                        "repo": target.repo.full_name(),
                        "number": issue.number,
                        "title": issue.title,
                        "url": issue.url,
                    }),
                );
                Dispatched::with_events(&issue, vec![event])
            }
            other => Err(ToolError::new(
                ToolErrorKind::UnknownTool,
                format!("unknown tool: {other}"),
            )),
        }
    }

    async fn github_status(
        &self,
        base: &Path,
        args: StatusArgs,
    ) -> Result<StatusOutput, ToolError> {
        let github = &self.inner.github;
        let settings = github.settings();
        let mut out = StatusOutput {
            host: settings.host(),
            api_url: settings.api_base(),
            ..StatusOutput::default()
        };
        let target = self
            .github_target(base, args.path.as_deref(), args.repo.as_deref())
            .await;
        if let Ok(target) = &target {
            out.repo_ref = Some(target.repo.clone());
            if let Some(status) = &target.status {
                out.branch = status.branch.clone();
                out.upstream = status.upstream.clone();
                out.ahead = status.ahead;
                out.behind = status.behind;
                let upstream = upstream_remote(status);
                out.remote = pick_remote(&target.remotes, upstream, &out.host)
                    .filter(|(_, repo)| repo == &target.repo)
                    .map(|(name, _)| name);
            }
        }
        if let Err(err) = &target {
            out.repo_error = Some(err.message.clone());
        }
        let credential = github.credential().await?;
        let Some(credential) = credential else {
            out.account_error = Some(GitHubError::no_token().message);
            return Ok(out);
        };
        out.token_source = Some(credential.source.label());
        let client =
            GitHubClient::new(&settings.api_base(), credential.token.clone()).map_err(map_error)?;
        match client.account().await {
            Ok(account) => {
                out.authenticated = true;
                out.account = Some(account);
            }
            Err(err) => {
                out.account_error = Some(github.check(err).message);
                return Ok(out);
            }
        }
        let Ok(target) = target else {
            return Ok(out);
        };
        match client.repo(&target.repo).await {
            Ok(repo) => out.repo = Some(repo),
            Err(err) => {
                out.repo_error = Some(github.check(err).message);
                return Ok(out);
            }
        }
        if let Some(head) = target.status.as_ref().and_then(pushed_branch) {
            // The most recent pull request of the branch, open or not: after
            // a merge the section says so instead of offering a new one.
            let filter = PullFilter {
                state: Some("all".into()),
                head: Some(head_for(&target, &head)),
                base: None,
                limit: Some(1),
            };
            if let Ok(pulls) = client.pulls(&target.repo, &filter).await {
                if let Some(pull) = pulls.into_iter().next() {
                    out.checks = client.checks(&target.repo, &pull.head_sha).await.ok();
                    out.pull = Some(pull);
                }
            }
        }
        Ok(out)
    }

    pub(crate) async fn git_remotes(
        &self,
        base: &Path,
        args: RemotesArgs,
    ) -> Result<RemotesOutput, ToolError> {
        let dir = resolve_cwd(base, args.path.as_deref())?;
        let git = self.git()?;
        let host = self.inner.github.settings().host();
        let remotes = blocking(move || git.remotes(&dir).map_err(git_tools::map_error)).await?;
        Ok(RemotesOutput {
            remotes: remotes
                .into_iter()
                .map(|r| RemoteInfo {
                    github: orchestrator_git::github::parse_remote_url(&r.url)
                        .filter(|repo| repo.host == host),
                    name: r.name,
                    url: r.url,
                })
                .collect(),
        })
    }
}

/// Upstream branch name (without the remote) of a pushed branch.
fn pushed_branch(status: &Status) -> Option<String> {
    status
        .upstream
        .as_deref()
        .and_then(|u| u.split_once('/'))
        .map(|(_, branch)| branch.to_owned())
}

/// `owner:branch` when the branch lives in another repository (a fork).
fn head_for(target: &Target, branch: &str) -> String {
    let fork = target
        .status
        .as_ref()
        .and_then(upstream_remote)
        .and_then(|remote| target.remotes.iter().find(|(name, _)| name == remote))
        .and_then(|(_, url)| orchestrator_git::github::parse_remote_url(url))
        .filter(|repo| !repo.owner.eq_ignore_ascii_case(&target.repo.owner));
    match fork {
        Some(fork) => format!("{}:{branch}", fork.owner),
        None => branch.to_owned(),
    }
}

/// The head of a new pull request from the local branch, which must be on
/// GitHub with nothing left to push.
fn pushed_head(target: &Target) -> Result<String, ToolError> {
    let status = target
        .status
        .as_ref()
        .ok_or_else(|| invalid("sem repositório local: informe head (a branch no GitHub)"))?;
    let branch = status
        .branch
        .clone()
        .ok_or_else(|| invalid("HEAD destacado: faça checkout de uma branch ou informe head"))?;
    let Some(remote_branch) = pushed_branch(status) else {
        return Err(invalid(format!(
            "a branch {branch} ainda não está no GitHub: envie com git.push {{ setUpstream: true }} e tente de novo"
        )));
    };
    if status.ahead > 0 {
        return Err(invalid(format!(
            "a branch {branch} tem {} commit(s) ainda não enviados: envie com git.push antes de abrir o pull request",
            status.ahead
        )));
    }
    Ok(head_for(target, &remote_branch))
}

/// The pushed commit of the current branch (for `github.checks`).
fn pushed_ref(status: Option<&Status>) -> Result<String, ToolError> {
    status.and_then(pushed_branch).ok_or_else(|| {
        invalid("a branch atual não está no GitHub: informe ref (commit, branch ou tag)")
    })
}

fn pr_created(origin: &CallOrigin, repo: &RepoRef, pull: &PullSummary) -> AuditEvent {
    AuditEvent::new(
        EventKind::GithubPrCreated,
        origin.clone(),
        format!(
            "PR #{} aberto em {}: {}",
            pull.number,
            repo.full_name(),
            pull.title
        ),
        json!({
            "repo": repo.full_name(),
            "number": pull.number,
            "title": pull.title,
            "url": pull.url,
            "head": pull.head,
            "base": pull.base,
            "draft": pull.draft,
        }),
    )
}
