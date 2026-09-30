//! Which GitHub repository a local remote is (ADR-0017).

use serde::{Deserialize, Serialize};

/// A repository on a GitHub host.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RepoRef {
    pub host: String,
    pub owner: String,
    pub name: String,
}

impl RepoRef {
    /// `owner/name`.
    pub fn full_name(&self) -> String {
        format!("{}/{}", self.owner, self.name)
    }

    /// `owner/name` given by the user or an AI, on `host`.
    pub fn parse_full(value: &str, host: &str) -> Option<Self> {
        let value = value.trim().trim_end_matches(".git");
        let (owner, name) = value.split_once('/')?;
        if !valid_part(owner) || !valid_part(name) {
            return None;
        }
        Some(Self {
            host: host.to_ascii_lowercase(),
            owner: owner.to_owned(),
            name: name.to_owned(),
        })
    }
}

fn valid_part(part: &str) -> bool {
    !part.is_empty()
        && part != "."
        && part != ".."
        && part
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

/// `https://host/owner/name(.git)`, `git@host:owner/name(.git)`,
/// `ssh://git@host(:port)/owner/name(.git)` and `git://host/owner/name`.
pub fn parse_remote_url(url: &str) -> Option<RepoRef> {
    let url = url.trim();
    let (host, path) = if let Some((_, rest)) = url.split_once("://") {
        let (authority, path) = rest.split_once('/')?;
        let authority = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
        let host = authority.split_once(':').map_or(authority, |(h, _)| h);
        (host, path)
    } else {
        // scp-like: [user@]host:owner/name
        let (authority, path) = url.split_once(':')?;
        if authority.contains('/') {
            return None;
        }
        let host = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
        (host, path)
    };
    if host.is_empty() {
        return None;
    }
    let path = path.trim_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path);
    let mut parts = path.split('/');
    let (owner, name) = (parts.next()?, parts.next()?);
    if parts.next().is_some() || !valid_part(owner) || !valid_part(name) {
        return None;
    }
    Some(RepoRef {
        host: host.to_ascii_lowercase(),
        owner: owner.to_owned(),
        name: name.to_owned(),
    })
}

/// The remote that stands for the project on `host`: the remote of the
/// current branch's upstream, else `origin`, else the first one on `host`.
/// `remotes` are `(name, url)` pairs.
pub fn pick_remote(
    remotes: &[(String, String)],
    upstream_remote: Option<&str>,
    host: &str,
) -> Option<(String, RepoRef)> {
    let host = host.to_ascii_lowercase();
    let on_host: Vec<(String, RepoRef)> = remotes
        .iter()
        .filter_map(|(name, url)| {
            parse_remote_url(url)
                .filter(|repo| repo.host == host)
                .map(|repo| (name.clone(), repo))
        })
        .collect();
    let by_name = |wanted: &str| on_host.iter().find(|(name, _)| name == wanted).cloned();
    upstream_remote
        .and_then(by_name)
        .or_else(|| by_name("origin"))
        .or_else(|| on_host.first().cloned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo(host: &str, owner: &str, name: &str) -> Option<RepoRef> {
        Some(RepoRef {
            host: host.into(),
            owner: owner.into(),
            name: name.into(),
        })
    }

    #[test]
    fn parses_every_remote_form() {
        let expected = repo("github.com", "SPTOXI", "Orchestrator");
        for url in [
            "https://github.com/SPTOXI/Orchestrator.git",
            "https://github.com/SPTOXI/Orchestrator",
            "https://github.com/SPTOXI/Orchestrator/",
            "https://user:token@GitHub.com/SPTOXI/Orchestrator.git",
            "git@github.com:SPTOXI/Orchestrator.git",
            "github.com:SPTOXI/Orchestrator",
            "ssh://git@github.com/SPTOXI/Orchestrator.git",
            "ssh://git@github.com:22/SPTOXI/Orchestrator",
            "git://github.com/SPTOXI/Orchestrator.git",
        ] {
            assert_eq!(parse_remote_url(url), expected, "{url}");
        }
        assert_eq!(
            parse_remote_url("https://ghe.example.com/time/app.js.git"),
            repo("ghe.example.com", "time", "app.js")
        );
        for url in [
            "/home/user/repos/app.git",
            "file:///srv/app.git",
            "https://gitlab.com/group/sub/app.git",
            "https://github.com/só-dono",
            "C:\\repos\\app",
            "",
        ] {
            assert_eq!(parse_remote_url(url), None, "{url}");
        }
    }

    #[test]
    fn picks_the_remote_of_the_upstream_then_origin() {
        let remotes = vec![
            ("backup".to_owned(), "/srv/backup.git".to_owned()),
            ("fork".to_owned(), "git@github.com:eu/app.git".to_owned()),
            (
                "origin".to_owned(),
                "https://github.com/time/app.git".to_owned(),
            ),
        ];
        let (name, found) = pick_remote(&remotes, Some("fork"), "github.com").unwrap();
        assert_eq!(
            (name.as_str(), found.full_name().as_str()),
            ("fork", "eu/app")
        );
        let (name, _) = pick_remote(&remotes, Some("backup"), "github.com").unwrap();
        assert_eq!(name, "origin");
        let (name, _) = pick_remote(&remotes[..2], None, "github.com").unwrap();
        assert_eq!(name, "fork");
        assert!(pick_remote(&remotes, None, "ghe.example.com").is_none());
        assert!(pick_remote(&[], None, "github.com").is_none());
    }

    #[test]
    fn full_names_from_the_user() {
        assert_eq!(
            RepoRef::parse_full(" time/app ", "GitHub.com"),
            repo("github.com", "time", "app")
        );
        assert!(RepoRef::parse_full("time", "github.com").is_none());
        assert!(RepoRef::parse_full("time/app/x", "github.com").is_none());
        assert!(RepoRef::parse_full("../app", "github.com").is_none());
    }
}
