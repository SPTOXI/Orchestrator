//! Agent settings: `<app-data>/agents.json` (ADR-0015). Configuration
//! stays in files, like `context.json` and `council.json`.

use serde::{Deserialize, Serialize};
use std::path::Path;

/// Agents running at the same time.
pub const DEFAULT_PARALLEL: u32 = 2;
pub const MAX_PARALLEL: u32 = 8;
/// Turns an agent may spend before it stops on its own.
pub const DEFAULT_TURNS: u32 = 12;
pub const MAX_TURNS: u32 = 50;
const FILE_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AgentSettings {
    /// How many agents may run at the same time.
    pub max_parallel: u32,
    /// Turns an agent may spend before the Orchestrator stops it. Not a
    /// permission policy (that is the autonomy mode, ADR-0016): it says how
    /// much an agent runs, the same in every mode.
    pub max_turns: u32,
}

impl Default for AgentSettings {
    fn default() -> Self {
        Self {
            max_parallel: DEFAULT_PARALLEL,
            max_turns: DEFAULT_TURNS,
        }
    }
}

impl AgentSettings {
    pub fn validate(&self) -> Result<(), String> {
        if !(1..=MAX_PARALLEL).contains(&self.max_parallel) {
            return Err(format!("o paralelismo vai de 1 a {MAX_PARALLEL} agentes"));
        }
        if !(1..=MAX_TURNS).contains(&self.max_turns) {
            return Err(format!("o teto de turnos vai de 1 a {MAX_TURNS}"));
        }
        Ok(())
    }
}

#[derive(Serialize, Deserialize)]
struct SettingsFile {
    version: u32,
    agents: AgentSettings,
}

/// Reads the settings; a missing file gives the defaults and an invalid one
/// the defaults plus a warning.
pub fn load(path: &Path) -> (AgentSettings, Option<String>) {
    match std::fs::read_to_string(path) {
        Ok(text) => match serde_json::from_str::<SettingsFile>(&text) {
            Ok(file) => match file.agents.validate() {
                Ok(()) => (file.agents, None),
                Err(err) => (
                    AgentSettings::default(),
                    Some(format!("{} ignored ({err})", path.display())),
                ),
            },
            Err(err) => (
                AgentSettings::default(),
                Some(format!("{} is not valid ({err})", path.display())),
            ),
        },
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => (AgentSettings::default(), None),
        Err(err) => (
            AgentSettings::default(),
            Some(format!("cannot read {}: {err}", path.display())),
        ),
    }
}

/// Writes the settings atomically.
pub fn save(path: &Path, settings: &AgentSettings) -> Result<(), String> {
    let io = |e: std::io::Error| format!("cannot write {}: {e}", path.display());
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(io)?;
    }
    let text = serde_json::to_string_pretty(&SettingsFile {
        version: FILE_VERSION,
        agents: *settings,
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
        let path = dir.path().join("agents.json");
        assert_eq!(load(&path), (AgentSettings::default(), None));
        let settings = AgentSettings {
            max_parallel: 4,
            max_turns: 30,
        };
        save(&path, &settings).unwrap();
        assert_eq!(load(&path), (settings, None));
        assert!(AgentSettings {
            max_parallel: 0,
            ..settings
        }
        .validate()
        .is_err());
        assert!(AgentSettings {
            max_turns: MAX_TURNS + 1,
            ..settings
        }
        .validate()
        .is_err());
        std::fs::write(&path, r#"{"version":1,"agents":{"maxParallel":99}}"#).unwrap();
        let (loaded, warning) = load(&path);
        assert_eq!(loaded, AgentSettings::default());
        assert!(warning.unwrap().contains("ignored"));
    }
}
