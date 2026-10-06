//! Council settings, kept in `<app-data>/council.json` (no secrets) until
//! the SQLite of Phase 6 (ADR-0011).

use crate::score::Preference;
use orchestrator_core::ProviderId;
use orchestrator_providers::ProviderError;
use serde::{Deserialize, Serialize};
use std::path::Path;

pub const MAX_MEMBERS: usize = 5;
const FILE_VERSION: u32 = 1;

/// How much the Council does on its own.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CouncilMode {
    /// Only the router (no tokens); the user picks.
    #[default]
    Off,
    /// The Council deliberates; the user approves or picks another model.
    Suggest,
    /// The Council decides and the Orchestrator applies the decision.
    Full,
}

impl CouncilMode {
    pub fn label(self) -> &'static str {
        match self {
            CouncilMode::Off => "Desligado",
            CouncilMode::Suggest => "Sugerir",
            CouncilMode::Full => "Full",
        }
    }
}

/// A seat on the Council: a provider and one of its models.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CouncilMember {
    pub provider: ProviderId,
    /// `None` = the provider's default model.
    #[serde(default)]
    pub model: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CouncilSettings {
    pub mode: CouncilMode,
    /// 1 member = the "manager".
    pub members: Vec<CouncilMember>,
    /// Candidates the router hands to the Council.
    pub shortlist: u8,
    /// Deliberation cache lifetime; 0 disables the cache.
    pub cache_minutes: u32,
    /// Time each member has to answer.
    pub timeout_secs: u32,
    /// Default preference; `None` = each activity's own.
    pub preference: Option<Preference>,
    /// Send the task as the first message of the session.
    pub send_task: bool,
}

impl Default for CouncilSettings {
    fn default() -> Self {
        Self {
            mode: CouncilMode::Off,
            members: Vec::new(),
            shortlist: 6,
            cache_minutes: 60,
            timeout_secs: 60,
            preference: None,
            send_task: true,
        }
    }
}

impl CouncilSettings {
    pub fn validate(&self) -> Result<(), ProviderError> {
        if self.members.len() > MAX_MEMBERS {
            return Err(ProviderError::invalid(format!(
                "o Conselho tem no máximo {MAX_MEMBERS} membros"
            )));
        }
        for (i, member) in self.members.iter().enumerate() {
            if member.provider.as_str().trim().is_empty() {
                return Err(ProviderError::invalid("membro sem provider"));
            }
            if self.members[..i].contains(member) {
                return Err(ProviderError::invalid(format!(
                    "membro repetido: {} / {}",
                    member.provider,
                    member.model.as_deref().unwrap_or("modelo padrão")
                )));
            }
        }
        if self.mode != CouncilMode::Off && self.members.is_empty() {
            return Err(ProviderError::invalid(format!(
                "o modo {} precisa de pelo menos um membro no Conselho",
                self.mode.label()
            )));
        }
        if !(2..=10).contains(&self.shortlist) {
            return Err(ProviderError::invalid(
                "a lista curta tem de 2 a 10 candidatos",
            ));
        }
        if self.cache_minutes > 24 * 60 {
            return Err(ProviderError::invalid("o cache vale no máximo 24 h"));
        }
        if !(5..=300).contains(&self.timeout_secs) {
            return Err(ProviderError::invalid(
                "o prazo por membro vai de 5 a 300 s",
            ));
        }
        Ok(())
    }
}

#[derive(Serialize, Deserialize)]
struct SettingsFile {
    version: u32,
    council: CouncilSettings,
}

/// Reads the settings; a missing file gives the defaults and an invalid
/// one the defaults plus a warning.
pub fn load(path: &Path) -> (CouncilSettings, Option<String>) {
    match std::fs::read_to_string(path) {
        Ok(text) => match serde_json::from_str::<SettingsFile>(&text) {
            Ok(file) => match file.council.validate() {
                Ok(()) => (file.council, None),
                Err(err) => (
                    CouncilSettings::default(),
                    Some(format!(
                        "{} ignored ({}); Council turned off",
                        path.display(),
                        err.message
                    )),
                ),
            },
            Err(err) => (
                CouncilSettings::default(),
                Some(format!(
                    "{} is not valid ({err}); Council turned off",
                    path.display()
                )),
            ),
        },
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            (CouncilSettings::default(), None)
        }
        Err(err) => (
            CouncilSettings::default(),
            Some(format!("cannot read {}: {err}", path.display())),
        ),
    }
}

/// Writes the settings atomically.
pub fn save(path: &Path, settings: &CouncilSettings) -> Result<(), ProviderError> {
    let io = |e: std::io::Error| {
        ProviderError::internal(format!("cannot write {}: {e}", path.display()))
    };
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(io)?;
    }
    let text = serde_json::to_string_pretty(&SettingsFile {
        version: FILE_VERSION,
        council: settings.clone(),
    })
    .map_err(|e| ProviderError::internal(e.to_string()))?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, text).map_err(io)?;
    std::fs::rename(&tmp, path).map_err(io)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn member(provider: &str, model: Option<&str>) -> CouncilMember {
        CouncilMember {
            provider: provider.into(),
            model: model.map(str::to_owned),
        }
    }

    #[test]
    fn validation_explains_what_is_wrong() {
        let ok = CouncilSettings {
            mode: CouncilMode::Suggest,
            members: vec![member("a", None), member("a", Some("m2"))],
            ..Default::default()
        };
        ok.validate().unwrap();
        CouncilSettings::default().validate().unwrap();

        let message = |settings: CouncilSettings| settings.validate().unwrap_err().message;
        assert_eq!(
            message(CouncilSettings {
                mode: CouncilMode::Full,
                ..Default::default()
            }),
            "o modo Full precisa de pelo menos um membro no Conselho"
        );
        assert!(message(CouncilSettings {
            members: vec![member("a", None), member("a", None)],
            ..Default::default()
        })
        .starts_with("membro repetido"));
        assert!(message(CouncilSettings {
            members: (0..6).map(|i| member(&format!("p{i}"), None)).collect(),
            ..Default::default()
        })
        .contains("no máximo 5"));
        assert!(message(CouncilSettings {
            shortlist: 1,
            ..Default::default()
        })
        .contains("lista curta"));
        assert!(message(CouncilSettings {
            timeout_secs: 1,
            ..Default::default()
        })
        .contains("prazo"));
    }

    #[test]
    fn round_trips_and_tolerates_bad_files() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("council.json");
        assert_eq!(load(&path), (CouncilSettings::default(), None));

        let settings = CouncilSettings {
            mode: CouncilMode::Full,
            members: vec![member("claude", Some("claude-opus"))],
            preference: Some(Preference::Cost),
            ..Default::default()
        };
        save(&path, &settings).unwrap();
        assert_eq!(load(&path), (settings, None));
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("\"mode\": \"full\""), "{text}");

        std::fs::write(&path, "{ nope").unwrap();
        let (loaded, warning) = load(&path);
        assert_eq!(loaded, CouncilSettings::default());
        assert!(warning.unwrap().contains("Council turned off"));

        // Partial files take the defaults for missing fields.
        std::fs::write(&path, r#"{"version":1,"council":{"mode":"off"}}"#).unwrap();
        assert_eq!(load(&path), (CouncilSettings::default(), None));
    }
}
