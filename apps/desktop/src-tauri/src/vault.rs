//! API keys in the OS vault (ADR-0010): Windows Credential Manager, macOS
//! Keychain, Secret Service on Linux (GNOME Keyring, KWallet…).

use orchestrator_provider_api::SecretStore;

/// Service name the keys are stored under.
pub(crate) const SERVICE: &str = "dev.orchestrator.desktop";

pub struct OsVault;

fn entry(key: &str) -> Result<keyring::Entry, String> {
    keyring::Entry::new(SERVICE, &format!("connection:{key}")).map_err(|e| e.to_string())
}

impl SecretStore for OsVault {
    fn get(&self, key: &str) -> Result<Option<String>, String> {
        match entry(key)?.get_password() {
            Ok(secret) => Ok(Some(secret)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(err) => Err(err.to_string()),
        }
    }

    fn set(&self, key: &str, secret: &str) -> Result<(), String> {
        entry(key)?.set_password(secret).map_err(|e| e.to_string())
    }

    fn delete(&self, key: &str) -> Result<(), String> {
        match entry(key)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(err) => Err(err.to_string()),
        }
    }

    fn describe(&self) -> String {
        match std::env::consts::OS {
            "windows" => "Gerenciador de Credenciais do Windows",
            "macos" => "Keychain do macOS",
            _ => "Secret Service (GNOME Keyring / KWallet)",
        }
        .into()
    }
}
