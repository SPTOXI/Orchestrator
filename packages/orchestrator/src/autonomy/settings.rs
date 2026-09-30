//! `<app-data>/autonomy.json` (ADR-0016): the default mode, the mode of
//! each project and the user's rules. Configuration stays in files, like
//! `context.json` and `agents.json`.

use super::policy::{default_rules, validate};
use orchestrator_core::{AutonomyMode, PolicyRule};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

const FILE_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutonomySettings {
    /// Mode of projects the user has not chosen one for.
    pub default_mode: AutonomyMode,
    /// Mode the user chose for each project, by project id.
    pub projects: BTreeMap<String, AutonomyMode>,
    /// Rules of the Autonomous mode.
    pub rules: Vec<PolicyRule>,
}

impl Default for AutonomySettings {
    fn default() -> Self {
        Self {
            default_mode: AutonomyMode::Assisted,
            projects: BTreeMap::new(),
            rules: default_rules(),
        }
    }
}

impl AutonomySettings {
    /// Mode of a project (the default when there is none, or none chosen).
    pub fn mode_of(&self, project_id: Option<&str>) -> AutonomyMode {
        project_id
            .and_then(|id| self.projects.get(id))
            .copied()
            .unwrap_or(self.default_mode)
    }
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SettingsFile {
    version: u32,
    default_mode: AutonomyMode,
    #[serde(default)]
    projects: BTreeMap<String, AutonomyMode>,
    rules: Vec<PolicyRule>,
}

/// Reads the settings. A missing file gives the defaults; an unusable one
/// gives the defaults — Assisted, never more — and a warning.
pub fn load(path: &Path) -> (AutonomySettings, Option<String>) {
    let fallback = |why: String| (AutonomySettings::default(), Some(why));
    match std::fs::read_to_string(path) {
        Ok(text) => match serde_json::from_str::<SettingsFile>(&text) {
            Ok(file) => match validate(&file.rules) {
                Ok(rules) => (
                    AutonomySettings {
                        default_mode: file.default_mode,
                        projects: file.projects,
                        rules,
                    },
                    None,
                ),
                Err(err) => fallback(format!(
                    "{} ignorado ({err}); valendo o modo Assistido",
                    path.display()
                )),
            },
            Err(err) => fallback(format!(
                "{} não é válido ({err}); valendo o modo Assistido",
                path.display()
            )),
        },
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            (AutonomySettings::default(), None)
        }
        Err(err) => fallback(format!(
            "não foi possível ler {}: {err}; valendo o modo Assistido",
            path.display()
        )),
    }
}

/// Writes the settings atomically.
pub fn save(path: &Path, settings: &AutonomySettings) -> Result<(), String> {
    let io = |e: std::io::Error| format!("não foi possível gravar {}: {e}", path.display());
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(io)?;
    }
    let text = serde_json::to_string_pretty(&SettingsFile {
        version: FILE_VERSION,
        default_mode: settings.default_mode,
        projects: settings.projects.clone(),
        rules: settings.rules.clone(),
    })
    .map_err(|e| e.to_string())?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, text).map_err(io)?;
    std::fs::rename(&tmp, path).map_err(io)
}

#[cfg(test)]
mod tests {
    use super::*;
    use orchestrator_core::Decision;

    #[test]
    fn round_trip_and_a_broken_file_never_opens_permissions() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("autonomy.json");
        assert_eq!(load(&path), (AutonomySettings::default(), None));

        let mut settings = AutonomySettings {
            default_mode: AutonomyMode::Autonomous,
            ..Default::default()
        };
        settings
            .projects
            .insert("p1".into(), AutonomyMode::Unrestricted);
        settings.rules = vec![PolicyRule::tools(&["git.push"], Decision::Deny)];
        save(&path, &settings).unwrap();
        assert_eq!(load(&path), (settings.clone(), None));
        assert_eq!(settings.mode_of(Some("p1")), AutonomyMode::Unrestricted);
        assert_eq!(settings.mode_of(Some("p2")), AutonomyMode::Autonomous);
        assert_eq!(settings.mode_of(None), AutonomyMode::Autonomous);

        std::fs::write(&path, r#"{"version":1,"defaultMode":"unrestricted","rules":[{"tools":["Git Push"],"decision":"allow"}]}"#).unwrap();
        let (loaded, warning) = load(&path);
        assert_eq!(loaded.default_mode, AutonomyMode::Assisted);
        assert!(warning.unwrap().contains("Assistido"));

        std::fs::write(&path, "{ not json").unwrap();
        let (loaded, warning) = load(&path);
        assert_eq!(loaded, AutonomySettings::default());
        assert!(warning.is_some());
    }
}
