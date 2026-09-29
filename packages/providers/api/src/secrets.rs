//! Where API keys live. The app plugs the OS vault in (`keyring`); tests
//! use [`MemorySecretStore`]. Keys never reach files, events or the UI.

use parking_lot::Mutex;
use std::collections::HashMap;

/// Storage for connection secrets, keyed by connection id.
pub trait SecretStore: Send + Sync + 'static {
    fn get(&self, key: &str) -> Result<Option<String>, String>;
    fn set(&self, key: &str, secret: &str) -> Result<(), String>;
    fn delete(&self, key: &str) -> Result<(), String>;
    /// Human readable name of the backend (shown in the UI).
    fn describe(&self) -> String;
}

/// In-memory store (tests; never used for real keys).
#[derive(Default)]
pub struct MemorySecretStore {
    secrets: Mutex<HashMap<String, String>>,
}

impl MemorySecretStore {
    pub fn new() -> Self {
        Self::default()
    }
}

impl SecretStore for MemorySecretStore {
    fn get(&self, key: &str) -> Result<Option<String>, String> {
        Ok(self.secrets.lock().get(key).cloned())
    }

    fn set(&self, key: &str, secret: &str) -> Result<(), String> {
        self.secrets
            .lock()
            .insert(key.to_owned(), secret.to_owned());
        Ok(())
    }

    fn delete(&self, key: &str) -> Result<(), String> {
        self.secrets.lock().remove(key);
        Ok(())
    }

    fn describe(&self) -> String {
        "memória (testes)".into()
    }
}
