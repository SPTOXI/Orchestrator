//! Agent settings: `<app-data>/agents.json` (ADR-0015). Configuration
//! stays in files, like `context.json` and `council.json`.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

/// Agents running at the same time.
pub const DEFAULT_PARALLEL: u32 = 2;
pub const MAX_PARALLEL: u32 = 8;
/// Turns an agent may spend before it stops on its own.
pub const DEFAULT_TURNS: u32 = 12;
pub const MAX_TURNS: u32 = 50;
/// Subagents one agent may create (ADR-0015); 0 turns delegation off.
pub const DEFAULT_SUBAGENTS: u32 = 5;
pub const MAX_SUBAGENTS: u32 = 10;
const FILE_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AgentSettings {
    /// How many agents may run at the same time.
    pub max_parallel: u32,
    /// Turns an agent may spend before the Orchestrator stops it. Not a
    /// permission policy (that is the autonomy mode, ADR-0016): it says how
    /// much an agent runs, the same in every mode.
    pub max_turns: u32,
    /// Subagents one agent may create; 0 = no delegation (ADR-0018).
    pub max_subagents: u32,
    /// Agents of one provider (connection id) running at the same time,
    /// under `max_parallel` — for the account's rate limits (ADR-0018).
    pub provider_limits: BTreeMap<String, u32>,
    /// What one agent may spend (USD) before it stops; `None` = no ceiling.
    pub max_cost_usd: Option<f64>,
    /// What the AIs of a project may spend in a day (USD) before agents
    /// stop starting and running ones stop; `None` = no budget. The user's
    /// own sessions are never blocked.
    pub daily_budget_usd: Option<f64>,
}

impl Default for AgentSettings {
    fn default() -> Self {
        Self {
            max_parallel: DEFAULT_PARALLEL,
            max_turns: DEFAULT_TURNS,
            max_subagents: DEFAULT_SUBAGENTS,
            provider_limits: BTreeMap::new(),
            max_cost_usd: None,
            daily_budget_usd: None,
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
        if self.max_subagents > MAX_SUBAGENTS {
            return Err(format!(
                "os subagentes por agente vão de 0 a {MAX_SUBAGENTS}"
            ));
        }
        for (provider, limit) in &self.provider_limits {
            if provider.trim().is_empty() {
                return Err("limite por provider sem o nome do provider".into());
            }
            if !(1..=MAX_PARALLEL).contains(limit) {
                return Err(format!(
                    "o limite de {provider} vai de 1 a {MAX_PARALLEL} agentes"
                ));
            }
        }
        for (name, value) in [
            ("o teto de custo por agente", self.max_cost_usd),
            ("o orçamento diário", self.daily_budget_usd),
        ] {
            if let Some(value) = value {
                if !value.is_finite() || value <= 0.0 {
                    return Err(format!("{name} precisa ser maior que zero"));
                }
            }
        }
        Ok(())
    }

    /// Agents of `provider` that may run at the same time.
    pub fn provider_limit(&self, provider: &str) -> u32 {
        self.provider_limits
            .get(provider)
            .copied()
            .unwrap_or(self.max_parallel)
            .min(self.max_parallel)
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
        agents: settings.clone(),
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
            max_subagents: 0,
            provider_limits: BTreeMap::from([("openai".into(), 1)]),
            max_cost_usd: Some(1.5),
            daily_budget_usd: Some(10.0),
        };
        save(&path, &settings).unwrap();
        assert_eq!(load(&path), (settings.clone(), None));
        assert_eq!(settings.provider_limit("openai"), 1);
        assert_eq!(settings.provider_limit("outro"), 4);
        let bad = |change: fn(&mut AgentSettings)| {
            let mut s = settings.clone();
            change(&mut s);
            s.validate().is_err()
        };
        assert!(bad(|s| s.max_parallel = 0));
        assert!(bad(|s| s.max_turns = MAX_TURNS + 1));
        assert!(bad(|s| s.max_subagents = MAX_SUBAGENTS + 1));
        assert!(bad(|s| {
            s.provider_limits.insert("x".into(), 0);
        }));
        assert!(bad(|s| s.max_cost_usd = Some(0.0)));
        assert!(bad(|s| s.daily_budget_usd = Some(f64::NAN)));
        // A Phase 8b file gets the new defaults.
        std::fs::write(
            &path,
            r#"{"version":1,"agents":{"maxParallel":3,"maxTurns":5}}"#,
        )
        .unwrap();
        let (old, warning) = load(&path);
        assert_eq!(
            (old.max_parallel, old.max_subagents),
            (3, DEFAULT_SUBAGENTS)
        );
        assert_eq!(warning, None);
        std::fs::write(&path, r#"{"version":1,"agents":{"maxParallel":99}}"#).unwrap();
        let (loaded, warning) = load(&path);
        assert_eq!(loaded, AgentSettings::default());
        assert!(warning.unwrap().contains("ignored"));
    }
}
