//! Where the GitHub token comes from (ADR-0017): the OS vault (handed in by
//! the app), `GH_TOKEN`/`GITHUB_TOKEN`, then `gh auth token`.

use std::fmt;
use std::process::{Command, Stdio};

/// A token. `Debug` never shows it; only [`Secret::expose`] does, and only
/// the HTTP client calls it.
#[derive(Clone, PartialEq, Eq)]
pub struct Secret(String);

impl Secret {
    /// `None` for an empty or blank value.
    pub fn new(value: impl Into<String>) -> Option<Self> {
        let value = value.into().trim().to_owned();
        (!value.is_empty()).then_some(Self(value))
    }

    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret(***)")
    }
}

/// Where the token in use came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TokenSource {
    /// Saved by the user in the OS vault.
    Vault,
    /// An environment variable (its name).
    Env(String),
    /// `gh auth token` (GitHub CLI).
    Gh,
}

impl TokenSource {
    /// `vault`, `env:GH_TOKEN`, `gh`.
    pub fn label(&self) -> String {
        match self {
            Self::Vault => "vault".to_owned(),
            Self::Env(name) => format!("env:{name}"),
            Self::Gh => "gh".to_owned(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Credential {
    pub token: Secret,
    pub source: TokenSource,
}

/// Variables read, in order.
pub const TOKEN_VARS: [&str; 2] = ["GH_TOKEN", "GITHUB_TOKEN"];

/// The first token found: `vault`, then the environment, then the GitHub CLI
/// for `host`.
pub fn resolve_token(vault: Option<&Secret>, host: &str) -> Option<Credential> {
    resolve_with(vault, |name| std::env::var(name).ok(), || gh_token(host))
}

/// [`resolve_token`] with the environment and the CLI injected (tests).
pub fn resolve_with(
    vault: Option<&Secret>,
    env: impl Fn(&str) -> Option<String>,
    gh: impl FnOnce() -> Option<String>,
) -> Option<Credential> {
    if let Some(token) = vault {
        return Some(Credential {
            token: token.clone(),
            source: TokenSource::Vault,
        });
    }
    for name in TOKEN_VARS {
        if let Some(token) = env(name).and_then(Secret::new) {
            return Some(Credential {
                token,
                source: TokenSource::Env(name.to_owned()),
            });
        }
    }
    gh().and_then(Secret::new).map(|token| Credential {
        token,
        source: TokenSource::Gh,
    })
}

/// `gh auth token --hostname <host>`, when the GitHub CLI is installed and
/// logged in. Never prompts.
fn gh_token(host: &str) -> Option<String> {
    let program = which::which("gh").ok()?;
    let mut cmd = Command::new(program);
    cmd.args(["auth", "token", "--hostname", host])
        .env("GH_PROMPT_DISABLED", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let output = cmd.output().ok()?;
    if !output.status.success() {
        return None;
    }
    let token = String::from_utf8(output.stdout).ok()?;
    let token = token.trim();
    (!token.is_empty()).then(|| token.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secrets_never_show() {
        let secret = Secret::new(" ghp_abc ").unwrap();
        assert_eq!(secret.expose(), "ghp_abc");
        assert_eq!(format!("{secret:?}"), "Secret(***)");
        assert!(Secret::new("   ").is_none());
        let credential = Credential {
            token: secret,
            source: TokenSource::Vault,
        };
        assert!(!format!("{credential:?}").contains("ghp_abc"));
    }

    #[test]
    fn the_vault_wins_then_the_environment_then_gh() {
        let vault = Secret::new("do-cofre");
        let env = |name: &str| match name {
            "GITHUB_TOKEN" => Some("do-ambiente".to_owned()),
            "GH_TOKEN" => Some("  ".to_owned()),
            _ => None,
        };
        let found = resolve_with(vault.as_ref(), env, || panic!("gh não deveria rodar")).unwrap();
        assert_eq!(found.source, TokenSource::Vault);
        assert_eq!(found.token.expose(), "do-cofre");

        let found = resolve_with(None, env, || panic!("gh não deveria rodar")).unwrap();
        assert_eq!(found.source, TokenSource::Env("GITHUB_TOKEN".into()));
        assert_eq!(found.source.label(), "env:GITHUB_TOKEN");

        let found = resolve_with(None, |_| None, || Some("do-gh\n".into())).unwrap();
        assert_eq!(found.source, TokenSource::Gh);
        assert_eq!(found.token.expose(), "do-gh");

        assert!(resolve_with(None, |_| None, || None).is_none());
    }
}
