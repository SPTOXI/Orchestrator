//! GitHub over its REST API v3 (ADR-0017).
//!
//! The local Git of this crate stays synchronous and driven by the `git`
//! executable; this module is an asynchronous HTTP client of the GitHub API
//! (`reqwest`). The Tool Runtime exposes it as the `github.*` tools.
//!
//! The token is a [`Secret`]: it never appears in `Debug` output, errors or
//! the values returned by the client.

mod auth;
mod client;
mod remote;
mod types;

pub use auth::{resolve_token, resolve_with, Credential, Secret, TokenSource};
pub use client::{
    GitHubClient, IssueFilter, MergeMethod, MergeRequest, NewIssue, NewPull, PullFilter,
};
pub use remote::{parse_remote_url, pick_remote, RepoRef};
pub use types::*;

use serde::{Deserialize, Serialize};
use std::fmt;

/// Host of github.com.
pub const DEFAULT_HOST: &str = "github.com";
/// API of github.com.
pub const DEFAULT_API: &str = "https://api.github.com";

/// Which GitHub the Orchestrator talks to (`<app-data>/github.json`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct GitHubSettings {
    /// Host of the remotes that are GitHub (`github.com`, or the host of a
    /// GitHub Enterprise server).
    pub host: String,
    /// API base; empty means `https://api.github.com` for github.com and
    /// `https://<host>/api/v3` for anything else.
    pub api_url: Option<String>,
}

impl Default for GitHubSettings {
    fn default() -> Self {
        Self {
            host: DEFAULT_HOST.to_owned(),
            api_url: None,
        }
    }
}

impl GitHubSettings {
    /// The host, trimmed and lowercase (`github.com` when empty).
    pub fn host(&self) -> String {
        let host = self.host.trim().trim_end_matches('/').to_ascii_lowercase();
        if host.is_empty() {
            DEFAULT_HOST.to_owned()
        } else {
            host
        }
    }

    /// Base URL of the REST API, without a trailing slash.
    pub fn api_base(&self) -> String {
        if let Some(url) = self
            .api_url
            .as_deref()
            .map(str::trim)
            .filter(|u| !u.is_empty())
        {
            return url.trim_end_matches('/').to_owned();
        }
        let host = self.host();
        if host == DEFAULT_HOST {
            DEFAULT_API.to_owned()
        } else {
            format!("https://{host}/api/v3")
        }
    }

    /// Rejects values that cannot work, saying which.
    pub fn validate(&self) -> Result<(), String> {
        let host = self.host();
        if host.contains('/') || host.contains(' ') || host.contains(':') {
            return Err(format!(
                "host inválido: \"{}\" (use só o nome, como github.com)",
                self.host
            ));
        }
        if let Some(url) = self
            .api_url
            .as_deref()
            .map(str::trim)
            .filter(|u| !u.is_empty())
        {
            if !(url.starts_with("https://") || url.starts_with("http://")) {
                return Err(format!(
                    "apiUrl inválida: \"{url}\" (precisa começar com https://)"
                ));
            }
        }
        Ok(())
    }
}

/// Why a GitHub operation failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum GitHubErrorKind {
    /// No token in the vault, the environment or the GitHub CLI.
    NoToken,
    /// 401: the token is invalid or expired.
    Unauthorized,
    /// 403: the token cannot do this.
    Forbidden,
    /// The API rate limit was reached.
    RateLimited,
    /// 404: no such repository, pull request or issue, or no access to it.
    NotFound,
    /// 422 (or a request we refuse before sending): the message says why.
    Invalid,
    /// The API could not be reached.
    Network,
    /// The API answered with an unexpected error.
    Server,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitHubError {
    pub kind: GitHubErrorKind,
    pub message: String,
}

impl GitHubError {
    pub fn new(kind: GitHubErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    pub fn no_token() -> Self {
        Self::new(
            GitHubErrorKind::NoToken,
            "Nenhum token do GitHub. Conecte na aba GitHub (o token vai para o cofre do sistema), \
             defina GH_TOKEN ou GITHUB_TOKEN, ou entre com `gh auth login`.",
        )
    }
}

impl fmt::Display for GitHubError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for GitHubError {}

pub type GitHubResult<T> = std::result::Result<T, GitHubError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_pick_the_api() {
        assert_eq!(GitHubSettings::default().api_base(), DEFAULT_API);
        let enterprise = GitHubSettings {
            host: " GHE.Example.com/ ".into(),
            api_url: None,
        };
        assert_eq!(enterprise.host(), "ghe.example.com");
        assert_eq!(enterprise.api_base(), "https://ghe.example.com/api/v3");
        let custom = GitHubSettings {
            host: "github.com".into(),
            api_url: Some("http://127.0.0.1:9/api/".into()),
        };
        assert_eq!(custom.api_base(), "http://127.0.0.1:9/api");
        assert!(custom.validate().is_ok());
        assert!(GitHubSettings {
            host: "https://github.com".into(),
            api_url: None
        }
        .validate()
        .is_err());
        assert!(GitHubSettings {
            host: "github.com".into(),
            api_url: Some("ftp://x".into())
        }
        .validate()
        .is_err());
        let parsed: GitHubSettings = serde_json::from_str("{}").unwrap();
        assert_eq!(parsed, GitHubSettings::default());
    }
}
