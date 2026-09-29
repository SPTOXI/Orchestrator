//! Context settings: `<app-data>/context.json` (ADR-0013). Configuration
//! stays in files, like `council.json` (ADR-0012).

use serde::{Deserialize, Serialize};
use std::path::Path;

pub const DEFAULT_BUDGET: u32 = 1_500;
pub const MIN_BUDGET: u32 = 300;
pub const MAX_BUDGET: u32 = 8_000;
const FILE_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ContextSettings {
    /// Attach the project context to the first turn of every session
    /// (a session can opt out).
    pub auto_attach: bool,
    /// Default budget in estimated tokens.
    pub budget_tokens: u32,
}

impl Default for ContextSettings {
    fn default() -> Self {
        Self {
            auto_attach: true,
            budget_tokens: DEFAULT_BUDGET,
        }
    }
}

impl ContextSettings {
    pub fn validate(&self) -> Result<(), String> {
        if !(MIN_BUDGET..=MAX_BUDGET).contains(&self.budget_tokens) {
            return Err(format!(
                "o orçamento de contexto vai de {MIN_BUDGET} a {MAX_BUDGET} tokens"
            ));
        }
        Ok(())
    }
}

#[derive(Serialize, Deserialize)]
struct SettingsFile {
    version: u32,
    context: ContextSettings,
}

/// Reads the settings; a missing file gives the defaults and an invalid one
/// the defaults plus a warning.
pub fn load(path: &Path) -> (ContextSettings, Option<String>) {
    match std::fs::read_to_string(path) {
        Ok(text) => match serde_json::from_str::<SettingsFile>(&text) {
            Ok(file) => match file.context.validate() {
                Ok(()) => (file.context, None),
                Err(err) => (
                    ContextSettings::default(),
                    Some(format!("{} ignored ({err})", path.display())),
                ),
            },
            Err(err) => (
                ContextSettings::default(),
                Some(format!("{} is not valid ({err})", path.display())),
            ),
        },
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            (ContextSettings::default(), None)
        }
        Err(err) => (
            ContextSettings::default(),
            Some(format!("cannot read {}: {err}", path.display())),
        ),
    }
}

/// Writes the settings atomically.
pub fn save(path: &Path, settings: &ContextSettings) -> Result<(), String> {
    let io = |e: std::io::Error| format!("cannot write {}: {e}", path.display());
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(io)?;
    }
    let text = serde_json::to_string_pretty(&SettingsFile {
        version: FILE_VERSION,
        context: *settings,
    })
    .map_err(|e| e.to_string())?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, text).map_err(io)?;
    std::fs::rename(&tmp, path).map_err(io)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_and_validation() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("context.json");
        assert_eq!(load(&path), (ContextSettings::default(), None));
        let settings = ContextSettings {
            auto_attach: false,
            budget_tokens: 900,
        };
        save(&path, &settings).unwrap();
        assert_eq!(load(&path), (settings, None));
        assert!(ContextSettings {
            budget_tokens: 100,
            ..settings
        }
        .validate()
        .is_err());
        std::fs::write(&path, r#"{"version":1,"context":{"budgetTokens":99999}}"#).unwrap();
        let (loaded, warning) = load(&path);
        assert_eq!(loaded, ContextSettings::default());
        assert!(warning.unwrap().contains("ignored"));
    }
}
