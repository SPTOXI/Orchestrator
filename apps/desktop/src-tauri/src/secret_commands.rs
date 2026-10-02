//! Secrets for the AIs (ADR-0020): API keys and tokens the user saves by
//! name. The values live only in the OS vault and in the runtime's memory;
//! `secrets.json` keeps the names. The AIs write `{{secret:NAME}}` and
//! never see a value; neither does the UI after saving.

use crate::AppState;
use chrono::{DateTime, Utc};
use orchestrator_git::github::Secret;
use orchestrator_runtime::web::Secrets;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tauri::State;

/// Service the values are stored under (account `secret:<NAME>`).
pub(crate) const SERVICE: &str = "dev.orchestrator.desktop";
const MAX_NAME: usize = 64;

fn entry(name: &str) -> Result<keyring::Entry, String> {
    keyring::Entry::new(SERVICE, &format!("secret:{name}")).map_err(|e| e.to_string())
}

pub fn index_path(data_dir: &Path) -> PathBuf {
    data_dir.join("secrets.json")
}

/// One saved secret, without its value.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SecretEntry {
    pub name: String,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Index {
    version: u32,
    secrets: Vec<SecretEntry>,
}

fn read_index(path: &Path) -> Result<Vec<SecretEntry>, String> {
    match std::fs::read_to_string(path) {
        Ok(text) => serde_json::from_str::<Index>(&text)
            .map(|index| index.secrets)
            .map_err(|e| format!("secrets.json ignorado: {e}")),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(err) => Err(format!("secrets.json ilegível: {err}")),
    }
}

fn write_index(path: &Path, secrets: &[SecretEntry]) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let index = Index {
        version: 1,
        secrets: secrets.to_vec(),
    };
    let text = serde_json::to_string_pretty(&index).map_err(|e| e.to_string())?;
    crate::files::write_atomic(path, (text + "\n").as_bytes())
}

/// A name the AIs can write inside `{{secret:…}}`: letters, digits, `_`,
/// `-` and `.`.
pub fn validate_name(name: &str) -> Result<String, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("dê um nome ao segredo (ex.: OPENROUTER_KEY)".into());
    }
    if name.len() > MAX_NAME {
        return Err(format!("o nome tem mais de {MAX_NAME} caracteres"));
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
    {
        return Err("use só letras sem acento, números, _, - e . no nome".into());
    }
    Ok(name.to_owned())
}

/// Every saved secret with its value, for the runtime; the warning names
/// the ones the vault no longer has.
pub fn load(data_dir: &Path) -> (Secrets, Option<String>) {
    let entries = match read_index(&index_path(data_dir)) {
        Ok(entries) => entries,
        // The names are kept before the next save writes over them.
        Err(err) => {
            return (
                Secrets::new(),
                crate::files::guard(&index_path(data_dir), Some(err)),
            )
        }
    };
    let mut secrets = Secrets::new();
    let mut missing = Vec::new();
    for item in entries {
        match entry(&item.name).and_then(|e| e.get_password().map_err(|e| e.to_string())) {
            Ok(value) => match Secret::new(value) {
                Some(secret) => {
                    secrets.insert(item.name, secret);
                }
                None => missing.push(item.name),
            },
            Err(_) => missing.push(item.name),
        }
    }
    let warning = (!missing.is_empty())
        .then(|| format!("segredos sem valor no cofre: {}", missing.join(", ")));
    (secrets, warning)
}

/// What the "Segredos" section shows: names, never values.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SecretsView {
    pub secrets: Vec<SecretItem>,
    /// Name of the vault on this system.
    pub vault: String,
    pub warning: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SecretItem {
    pub name: String,
    pub updated_at: DateTime<Utc>,
    /// The runtime has its value (it was read from the vault).
    pub loaded: bool,
    /// What the AIs write to use it.
    pub placeholder: String,
}

fn vault_name() -> String {
    match std::env::consts::OS {
        "windows" => "Gerenciador de Credenciais do Windows",
        "macos" => "Keychain do macOS",
        _ => "Secret Service (GNOME Keyring / KWallet)",
    }
    .into()
}

/// Reloads the runtime's secrets and describes them.
fn refresh(state: &AppState) -> SecretsView {
    let (secrets, warning) = load(&state.data_dir);
    let entries = read_index(&index_path(&state.data_dir)).unwrap_or_default();
    let view = SecretsView {
        secrets: entries
            .into_iter()
            .map(|item| SecretItem {
                loaded: secrets.contains_key(&item.name),
                placeholder: format!("{{{{secret:{}}}}}", item.name),
                name: item.name,
                updated_at: item.updated_at,
            })
            .collect(),
        vault: vault_name(),
        warning,
    };
    state.runtime.set_secrets(secrets);
    view
}

#[tauri::command]
pub fn secrets_list(state: State<'_, AppState>) -> SecretsView {
    refresh(&state)
}

/// Saves (or replaces) a secret: the value to the vault, the name to
/// `secrets.json`.
#[tauri::command]
pub fn secret_save(
    state: State<'_, AppState>,
    name: String,
    value: String,
) -> Result<SecretsView, String> {
    let name = validate_name(&name)?;
    let secret = Secret::new(value).ok_or("o valor está vazio")?;
    entry(&name)?
        .set_password(secret.expose())
        .map_err(|e| format!("não foi possível salvar no cofre: {e}"))?;
    let path = index_path(&state.data_dir);
    let mut entries = read_index(&path).unwrap_or_default();
    entries.retain(|item| item.name != name);
    entries.push(SecretEntry {
        name,
        updated_at: Utc::now(),
    });
    entries.sort_by(|a, b| a.name.cmp(&b.name));
    write_index(&path, &entries)?;
    Ok(refresh(&state))
}

#[tauri::command]
pub fn secret_delete(state: State<'_, AppState>, name: String) -> Result<SecretsView, String> {
    match entry(&name)?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => {}
        Err(err) => return Err(format!("não foi possível apagar do cofre: {err}")),
    }
    let path = index_path(&state.data_dir);
    let mut entries = read_index(&path).unwrap_or_default();
    entries.retain(|item| item.name != name);
    write_index(&path, &entries)?;
    Ok(refresh(&state))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_the_ais_can_write() {
        assert_eq!(validate_name(" OPENROUTER_KEY ").unwrap(), "OPENROUTER_KEY");
        assert_eq!(validate_name("stripe.live-1").unwrap(), "stripe.live-1");
        assert!(validate_name("").is_err());
        assert!(validate_name("com espaço").is_err());
        assert!(validate_name("chave}}").is_err());
        assert!(validate_name("ação").is_err());
        assert!(validate_name(&"x".repeat(65)).is_err());
    }

    #[test]
    fn the_index_keeps_names_only() {
        let dir = tempfile::tempdir().unwrap();
        let path = index_path(dir.path());
        assert_eq!(read_index(&path).unwrap(), Vec::new());
        let item = SecretEntry {
            name: "A".into(),
            updated_at: Utc::now(),
        };
        write_index(&path, std::slice::from_ref(&item)).unwrap();
        assert_eq!(read_index(&path).unwrap(), vec![item]);
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("\"name\": \"A\"") && !text.contains("value"));
        std::fs::write(&path, "{").unwrap();
        assert!(read_index(&path).is_err());
    }
}
