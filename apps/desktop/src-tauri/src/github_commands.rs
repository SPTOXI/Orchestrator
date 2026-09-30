//! GitHub settings and token (ADR-0017). The token goes to the OS vault and
//! into the runtime, never to a file, an event or the UI; the UI only learns
//! whether there is one and where the token in use comes from (the
//! `github.status` tool). Everything else the UI does with GitHub goes
//! through `runtime_invoke`, like any other tool.

use crate::AppState;
use orchestrator_git::github::{GitHubSettings, Secret};
use serde::Serialize;
use std::path::{Path, PathBuf};
use tauri::State;

/// Service and account the token is stored under.
const SERVICE: &str = "dev.orchestrator.desktop";
const ACCOUNT: &str = "github:token";

fn entry() -> Result<keyring::Entry, String> {
    keyring::Entry::new(SERVICE, ACCOUNT).map_err(|e| e.to_string())
}

/// The token saved in the vault, if any.
pub fn vault_token() -> Result<Option<Secret>, String> {
    match entry()?.get_password() {
        Ok(secret) => Ok(Secret::new(secret)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(err) => Err(err.to_string()),
    }
}

pub fn settings_path(data_dir: &Path) -> PathBuf {
    data_dir.join("github.json")
}

/// `github.json`, or the defaults (github.com) with a warning when it is
/// unreadable.
pub fn load_settings(path: &Path) -> (GitHubSettings, Option<String>) {
    let Ok(text) = std::fs::read_to_string(path) else {
        return (GitHubSettings::default(), None);
    };
    match serde_json::from_str::<GitHubSettings>(&text) {
        Ok(settings) if settings.validate().is_ok() => (settings, None),
        Ok(settings) => (
            GitHubSettings::default(),
            settings
                .validate()
                .err()
                .map(|e| format!("github.json ignorado: {e}")),
        ),
        Err(err) => (
            GitHubSettings::default(),
            Some(format!("github.json ignorado: {err}")),
        ),
    }
}

fn save_settings(path: &Path, settings: &GitHubSettings) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let text = serde_json::to_string_pretty(settings).map_err(|e| e.to_string())?;
    std::fs::write(path, text + "\n").map_err(|e| e.to_string())
}

/// What the GitHub tab shows about the connection (never the token).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GitHubSetup {
    pub host: String,
    pub api_url: Option<String>,
    /// The API actually used.
    pub api_base: String,
    /// A token is saved in the vault.
    pub vault_token: bool,
    /// Name of the vault on this system.
    pub vault: String,
    /// `GH_TOKEN` or `GITHUB_TOKEN`, when set.
    pub env_token: Option<String>,
    /// The GitHub CLI is installed.
    pub gh_installed: bool,
    pub warning: Option<String>,
}

fn setup(state: &AppState, warning: Option<String>) -> GitHubSetup {
    let settings = state.runtime.github_settings();
    let (vault_token, vault_warning) = match vault_token() {
        Ok(token) => (token.is_some(), None),
        Err(err) => (false, Some(format!("não foi possível ler o cofre: {err}"))),
    };
    GitHubSetup {
        host: settings.host(),
        api_base: settings.api_base(),
        api_url: settings.api_url.clone(),
        vault_token,
        vault: vault_name(),
        env_token: ["GH_TOKEN", "GITHUB_TOKEN"]
            .into_iter()
            .find(|name| std::env::var(name).is_ok_and(|v| !v.trim().is_empty()))
            .map(str::to_owned),
        gh_installed: which_gh(),
        warning: warning
            .or(vault_warning)
            .or_else(|| state.github_warning.clone()),
    }
}

fn vault_name() -> String {
    match std::env::consts::OS {
        "windows" => "Gerenciador de Credenciais do Windows",
        "macos" => "Keychain do macOS",
        _ => "Secret Service (GNOME Keyring / KWallet)",
    }
    .into()
}

fn which_gh() -> bool {
    std::env::var_os("PATH").is_some_and(|paths| {
        std::env::split_paths(&paths)
            .any(|dir| dir.join("gh").is_file() || dir.join("gh.exe").is_file())
    })
}

#[tauri::command]
pub fn github_settings_get(state: State<'_, AppState>) -> GitHubSetup {
    setup(&state, None)
}

/// Host and API (GitHub Enterprise).
#[tauri::command]
pub fn github_settings_save(
    state: State<'_, AppState>,
    settings: GitHubSettings,
) -> Result<GitHubSetup, String> {
    let settings = GitHubSettings {
        host: settings.host(),
        api_url: settings
            .api_url
            .map(|u| u.trim().to_owned())
            .filter(|u| !u.is_empty()),
    };
    settings.validate()?;
    save_settings(&settings_path(&state.data_dir), &settings)?;
    state.runtime.set_github_settings(settings);
    Ok(setup(&state, None))
}

/// Saves a token in the vault; it wins over the environment and `gh`.
#[tauri::command]
pub fn github_token_save(state: State<'_, AppState>, token: String) -> Result<GitHubSetup, String> {
    let secret = Secret::new(token).ok_or("o token está vazio")?;
    entry()?
        .set_password(secret.expose())
        .map_err(|e| format!("não foi possível salvar no cofre: {e}"))?;
    state.runtime.set_github_token(Some(secret));
    Ok(setup(&state, None))
}

/// Removes the token from the vault (the environment and `gh` still count).
#[tauri::command]
pub fn github_token_clear(state: State<'_, AppState>) -> Result<GitHubSetup, String> {
    match entry()?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => {}
        Err(err) => return Err(err.to_string()),
    }
    state.runtime.set_github_token(None);
    Ok(setup(&state, None))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_file_round_trip_and_bad_files_fall_back() {
        let dir = tempfile::tempdir().unwrap();
        let path = settings_path(dir.path());
        assert_eq!(load_settings(&path), (GitHubSettings::default(), None));
        let enterprise = GitHubSettings {
            host: "ghe.example.com".into(),
            api_url: None,
        };
        save_settings(&path, &enterprise).unwrap();
        assert_eq!(load_settings(&path), (enterprise, None));
        std::fs::write(&path, "{ não é json").unwrap();
        let (settings, warning) = load_settings(&path);
        assert_eq!(settings, GitHubSettings::default());
        assert!(warning.unwrap().contains("github.json"));
        std::fs::write(&path, r#"{"host": "https://x.com"}"#).unwrap();
        assert!(load_settings(&path).1.is_some());
    }
}
