//! What is kept on disk (ADR-0025), under `<data>/local/`:
//!
//! - `models.json`: the models the user has, where their files are and
//!   the context each one runs with;
//! - `settings.json`: package choice, graphics card and idle time;
//! - `engine.json`: the engine installed (written by [`crate::engine`]).
//!
//! Files are replaced atomically (written aside, then renamed).

use crate::platform::Backend;
use chrono::{DateTime, Utc};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Where a model came from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum ModelSource {
    /// A catalog entry (its id), downloaded from Hugging Face.
    Catalog {
        entry: String,
        repo: String,
        file: String,
    },
    HuggingFace {
        repo: String,
        file: String,
    },
    /// Imported from Ollama (linked or copied from its blob).
    Ollama {
        name: String,
    },
    /// A file the user pointed to, used where it is.
    File,
}

impl ModelSource {
    pub fn kind(&self) -> &'static str {
        match self {
            ModelSource::Catalog { .. } => "catalog",
            ModelSource::HuggingFace { .. } => "huggingface",
            ModelSource::Ollama { .. } => "ollama",
            ModelSource::File => "file",
        }
    }
}

/// A model on this computer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalModel {
    /// Unique, made from the name; the model id sessions use.
    pub id: String,
    pub name: String,
    /// The GGUF file; for a model in parts, every part (first first).
    pub files: Vec<PathBuf>,
    pub size: u64,
    pub sha256: Option<String>,
    pub source: ModelSource,
    /// Context it runs with (tokens), chosen by the user or the default.
    pub context: u32,
    /// Context it was trained for.
    #[serde(default)]
    pub trained_context: Option<u32>,
    #[serde(default)]
    pub architecture: Option<String>,
    #[serde(default)]
    pub size_label: Option<String>,
    #[serde(default)]
    pub quantization: Option<String>,
    /// The chat template handles tools (native tool calls).
    #[serde(default)]
    pub tools: bool,
    /// The file has a chat template at all.
    #[serde(default)]
    pub chat_template: bool,
    #[serde(default)]
    pub kv_bytes_per_token: Option<u64>,
    pub added_at: DateTime<Utc>,
}

impl LocalModel {
    pub fn main_file(&self) -> Option<&Path> {
        self.files.first().map(PathBuf::as_path)
    }

    /// Memory it needs at its context, roughly: weights + KV cache + a
    /// margin for the engine.
    pub fn memory_needed(&self) -> u64 {
        let kv = self.kv_bytes_per_token.unwrap_or(0) * u64::from(self.context);
        self.size + kv + 512 * 1024 * 1024
    }
}

/// `models.json` as this version writes it. 2: `tools` tells apart chat
/// templates that only read tools from a message.
pub const MODELS_VERSION: u32 = 2;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelsFile {
    #[serde(default = "one")]
    pub version: u32,
    #[serde(default)]
    pub models: Vec<LocalModel>,
}

impl Default for ModelsFile {
    fn default() -> Self {
        Self {
            version: MODELS_VERSION,
            models: Vec::new(),
        }
    }
}

fn one() -> u32 {
    1
}

/// Graphics card use.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GpuMode {
    /// llama.cpp puts on the card what fits.
    #[default]
    Auto,
    /// Processor only (`-ngl 0`).
    Off,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    /// `None`: chosen by the computer (ADR-0025).
    #[serde(default)]
    pub backend: Option<Backend>,
    #[serde(default)]
    pub gpu: GpuMode,
    /// Minutes without calls before the engine is turned off (0 = never).
    #[serde(default = "idle")]
    pub idle_minutes: u32,
}

fn idle() -> u32 {
    10
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            backend: None,
            gpu: GpuMode::Auto,
            idle_minutes: idle(),
        }
    }
}

impl Settings {
    pub fn validate(&self) -> Result<(), String> {
        if self.idle_minutes > 24 * 60 {
            return Err("o tempo ocioso vai de 0 a 1.440 minutos".into());
        }
        Ok(())
    }
}

/// Reads a JSON file; a missing file is the default. A file that cannot be
/// read is kept aside (`.unreadable-<time>`) and the default is used, so the
/// next save does not destroy it (ADR-0022).
pub fn read_json<T: DeserializeOwned + Default>(path: &Path) -> (T, Option<String>) {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return (T::default(), None),
        Err(e) => {
            return (
                T::default(),
                Some(format!("não foi possível ler {}: {e}", path.display())),
            )
        }
    };
    match serde_json::from_str(&text) {
        Ok(value) => (value, None),
        Err(e) => {
            let mut aside = path.as_os_str().to_owned();
            aside.push(format!(".unreadable-{}", Utc::now().format("%Y%m%d%H%M%S")));
            let _ = std::fs::copy(path, PathBuf::from(&aside));
            (
                T::default(),
                Some(format!(
                    "{} não pôde ser lido ({e}); o original ficou em {}",
                    path.display(),
                    PathBuf::from(aside).display()
                )),
            )
        }
    }
}

pub fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("não foi possível criar {}: {e}", parent.display()))?;
    }
    let text = serde_json::to_string_pretty(value).map_err(|e| e.to_string())?;
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    std::fs::write(&tmp, text)
        .map_err(|e| format!("não foi possível gravar {}: {e}", path.display()))?;
    std::fs::rename(&tmp, path)
        .map_err(|e| format!("não foi possível gravar {}: {e}", path.display()))
}

/// An id for `name` that no model in `taken` has: lowercase letters,
/// digits, `.` and `-`.
pub fn unique_id(name: &str, taken: &[String]) -> String {
    let mut base = String::new();
    for c in name.to_lowercase().chars() {
        if c.is_ascii_alphanumeric() || c == '.' {
            base.push(c);
        } else if !base.ends_with('-') {
            base.push('-');
        }
    }
    let mut base = base
        .trim_matches(['-', '.'])
        .chars()
        .take(60)
        .collect::<String>();
    if base.is_empty() {
        base = "modelo".into();
    }
    let mut id = base.clone();
    let mut n = 2;
    while taken.contains(&id) {
        id = format!("{base}-{n}");
        n += 1;
    }
    id
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_readable_and_unique() {
        let taken = vec!["phi4-mini-latest".to_owned()];
        assert_eq!(unique_id("phi4-mini:latest", &taken), "phi4-mini-latest-2");
        assert_eq!(
            unique_id("Qwen3-8B-Q4_K_M.gguf", &[]),
            "qwen3-8b-q4-k-m.gguf"
        );
        assert_eq!(
            unique_id("hf.co/bartowski/Qwen3:Q4", &[]),
            "hf.co-bartowski-qwen3-q4"
        );
        assert_eq!(unique_id("ⓧⓧ", &[]), "modelo");
    }

    #[test]
    fn an_unreadable_file_is_kept_aside() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("models.json");
        let (file, warning) = read_json::<ModelsFile>(&path);
        assert!(file.models.is_empty() && warning.is_none());
        std::fs::write(&path, "{ não é json").unwrap();
        let (file, warning) = read_json::<ModelsFile>(&path);
        assert!(file.models.is_empty());
        assert!(warning.unwrap().contains("unreadable"));
        let kept = std::fs::read_dir(dir.path())
            .unwrap()
            .flatten()
            .any(|e| e.file_name().to_string_lossy().contains("unreadable"));
        assert!(kept);
        write_json(&path, &ModelsFile::default()).unwrap();
        assert_eq!(read_json::<ModelsFile>(&path).0.version, MODELS_VERSION);
    }
}
