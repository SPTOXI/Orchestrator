//! Offline models through Ollama (ADR-0021): whether it is installed and
//! running, the models on this machine, downloading one with progress,
//! removing one, and the API connection that makes them providers.
//!
//! Ollama serves its own API (`/api/*`) and an OpenAI-compatible one
//! (`/v1`): the models are used through the latter, like any API
//! connection, with no key and no cost.

use crate::config::{
    ApiKind, Connection, Credential, CredentialSource, ModelEntry, StreamFormat, ToolMode,
};
use crate::http::{read_all, FrameReader, HttpCall, HttpClient};
use orchestrator_providers::ProviderError;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

/// Where Ollama listens unless `OLLAMA_HOST` says otherwise.
pub const DEFAULT_OLLAMA_URL: &str = "http://127.0.0.1:11434";
/// Id of the connection the offline models go into.
pub const OLLAMA_CONNECTION: &str = "ollama";

/// Ollama's address: `OLLAMA_HOST` (`host:port` or a URL) or the default.
pub fn ollama_url() -> String {
    match std::env::var("OLLAMA_HOST").ok().map(|h| h.trim().to_owned()) {
        Some(host) if !host.is_empty() => {
            let host = host.trim_end_matches('/');
            let with_scheme = if host.contains("://") {
                host.to_owned()
            } else {
                format!("http://{host}")
            };
            // `0.0.0.0` is where it listens, not where to reach it.
            with_scheme.replace("://0.0.0.0", "://127.0.0.1")
        }
        _ => DEFAULT_OLLAMA_URL.to_owned(),
    }
}

/// A model recommended for this app, with what it is good at.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogModel {
    pub name: &'static str,
    pub label: &'static str,
    /// Download size, roughly (GB).
    pub size_gb: f32,
    /// Memory it needs to run well (GB of RAM or VRAM), roughly.
    pub memory_gb: u32,
    pub tools: bool,
    pub note: &'static str,
}

/// Models that run on a common computer and work with the Orchestrator's
/// tools (except where said). Sizes are approximate.
pub fn catalog() -> Vec<CatalogModel> {
    vec![
        CatalogModel {
            name: "qwen3:8b",
            label: "Qwen3 8B",
            size_gb: 5.2,
            memory_gb: 8,
            tools: true,
            note: "bom equilíbrio para código e conversa; pensa antes de responder",
        },
        CatalogModel {
            name: "qwen2.5-coder:7b",
            label: "Qwen2.5 Coder 7B",
            size_gb: 4.7,
            memory_gb: 8,
            tools: true,
            note: "focado em código",
        },
        CatalogModel {
            name: "llama3.1:8b",
            label: "Llama 3.1 8B",
            size_gb: 4.9,
            memory_gb: 8,
            tools: true,
            note: "uso geral",
        },
        CatalogModel {
            name: "qwen3:4b",
            label: "Qwen3 4B",
            size_gb: 2.6,
            memory_gb: 4,
            tools: true,
            note: "para computadores mais modestos",
        },
        CatalogModel {
            name: "qwen2.5-coder:14b",
            label: "Qwen2.5 Coder 14B",
            size_gb: 9.0,
            memory_gb: 16,
            tools: true,
            note: "código, melhor que o 7B; pede mais memória",
        },
        CatalogModel {
            name: "gpt-oss:20b",
            label: "gpt-oss 20B (OpenAI)",
            size_gb: 14.0,
            memory_gb: 16,
            tools: true,
            note: "raciocínio forte; para máquinas com 16 GB ou mais",
        },
        CatalogModel {
            name: "deepseek-r1:8b",
            label: "DeepSeek-R1 8B",
            size_gb: 5.2,
            memory_gb: 8,
            tools: false,
            note: "raciocínio; as ferramentas vão por prompt",
        },
        CatalogModel {
            name: "gemma3:4b",
            label: "Gemma 3 4B",
            size_gb: 3.3,
            memory_gb: 6,
            tools: false,
            note: "leve, lê imagens; as ferramentas vão por prompt",
        },
    ]
}

/// A model on this machine.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalModel {
    pub name: String,
    pub size_bytes: u64,
    pub family: Option<String>,
    pub parameter_size: Option<String>,
    pub quantization: Option<String>,
    pub modified_at: Option<String>,
    /// From `/api/show` when Ollama says (`None`: unknown).
    pub tools: Option<bool>,
    pub vision: Option<bool>,
    pub context_window: Option<u64>,
}

/// Is Ollama there?
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OllamaStatus {
    pub url: String,
    /// It answered.
    pub running: bool,
    pub version: Option<String>,
    /// The `ollama` program, when found.
    pub program: Option<String>,
    pub error: Option<String>,
}

/// Download progress, as Ollama reports it.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PullProgress {
    pub model: String,
    pub status: String,
    pub completed: Option<u64>,
    pub total: Option<u64>,
}

#[derive(Clone)]
pub struct Ollama {
    client: HttpClient,
    url: String,
}

#[derive(Deserialize)]
struct Tags {
    #[serde(default)]
    models: Vec<TagModel>,
}

#[derive(Deserialize)]
struct TagModel {
    name: String,
    #[serde(default)]
    size: u64,
    #[serde(default)]
    modified_at: Option<String>,
    #[serde(default)]
    details: TagDetails,
}

#[derive(Deserialize, Default)]
struct TagDetails {
    family: Option<String>,
    parameter_size: Option<String>,
    quantization_level: Option<String>,
}

impl Ollama {
    pub fn new(url: impl Into<String>) -> Result<Self, ProviderError> {
        Ok(Self {
            client: HttpClient::new()?,
            url: url.into().trim_end_matches('/').to_owned(),
        })
    }

    pub fn url(&self) -> &str {
        &self.url
    }

    pub async fn status(&self) -> OllamaStatus {
        let cancel = CancellationToken::new();
        let asked = tokio::time::timeout(
            Duration::from_secs(3),
            self.client
                .json(&HttpCall::get(format!("{}/api/version", self.url)), &cancel),
        )
        .await;
        let (running, version, error) = match asked {
            Ok(Ok(value)) => (
                true,
                value.get("version").and_then(Value::as_str).map(str::to_owned),
                None,
            ),
            Ok(Err(err)) => (false, None, Some(err.message)),
            Err(_) => (false, None, Some("não respondeu em 3 s".into())),
        };
        OllamaStatus {
            url: self.url.clone(),
            running,
            version,
            program: find_program().map(|p| p.display().to_string()),
            error,
        }
    }

    /// The models on this machine, with what `/api/show` says of each.
    pub async fn models(&self) -> Result<Vec<LocalModel>, ProviderError> {
        let cancel = CancellationToken::new();
        let value = self
            .client
            .json(&HttpCall::get(format!("{}/api/tags", self.url)), &cancel)
            .await?;
        let tags: Tags = serde_json::from_value(value)
            .map_err(|e| ProviderError::failed(format!("unexpected /api/tags answer: {e}")))?;
        let mut models = Vec::with_capacity(tags.models.len());
        for tag in tags.models {
            let (tools, vision, context_window) = self.show(&tag.name, &cancel).await;
            models.push(LocalModel {
                name: tag.name,
                size_bytes: tag.size,
                family: tag.details.family,
                parameter_size: tag.details.parameter_size,
                quantization: tag.details.quantization_level,
                modified_at: tag.modified_at,
                tools,
                vision,
                context_window,
            });
        }
        models.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(models)
    }

    /// Capabilities (newer Ollama) and context length of a model.
    async fn show(
        &self,
        name: &str,
        cancel: &CancellationToken,
    ) -> (Option<bool>, Option<bool>, Option<u64>) {
        let call = HttpCall::post(format!("{}/api/show", self.url), json!({"model": name}));
        let Ok(value) = self.client.json(&call, cancel).await else {
            return (None, None, None);
        };
        let capabilities: Option<Vec<String>> = value
            .get("capabilities")
            .and_then(Value::as_array)
            .map(|caps| {
                caps.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect()
            });
        let has = |cap: &str| capabilities.as_ref().map(|c| c.iter().any(|x| x == cap));
        let context_window = value.get("model_info").and_then(Value::as_object).and_then(|info| {
            info.iter()
                .find(|(key, _)| key.ends_with(".context_length"))
                .and_then(|(_, v)| v.as_u64())
        });
        (has("tools"), has("vision"), context_window)
    }

    /// Downloads `model`, telling `progress` as it goes; cancelling stops
    /// the download (Ollama keeps what it got and resumes next time).
    pub async fn pull(
        &self,
        model: &str,
        progress: &(dyn Fn(PullProgress) + Send + Sync),
        cancel: &CancellationToken,
    ) -> Result<(), ProviderError> {
        let model = model.trim();
        if model.is_empty() || model.contains(char::is_whitespace) {
            return Err(ProviderError::invalid("nome de modelo inválido"));
        }
        let call = HttpCall::post(
            format!("{}/api/pull", self.url),
            json!({"model": model, "stream": true}),
        );
        let response = self.client.send(&call, cancel).await?;
        let mut reader = FrameReader::new(response, StreamFormat::Ndjson);
        while let Some(frame) = reader.next(cancel).await? {
            let Ok(value) = serde_json::from_str::<Value>(&frame.data) else {
                continue;
            };
            if let Some(error) = value.get("error").and_then(Value::as_str) {
                return Err(ProviderError::failed(format!("Ollama: {error}")));
            }
            let status = value
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            let done = status == "success";
            progress(PullProgress {
                model: model.to_owned(),
                status,
                completed: value.get("completed").and_then(Value::as_u64),
                total: value.get("total").and_then(Value::as_u64),
            });
            if done {
                return Ok(());
            }
        }
        Err(ProviderError::failed(
            "o download terminou sem confirmação do Ollama",
        ))
    }

    pub async fn delete(&self, model: &str) -> Result<(), ProviderError> {
        let cancel = CancellationToken::new();
        let call = HttpCall::delete(format!("{}/api/delete", self.url), json!({"model": model}));
        let response = self.client.send(&call, &cancel).await?;
        read_all(response, &cancel).await.map(|_| ())
    }
}

/// The connection that holds the offline models: OpenAI-compatible, no
/// key, no cost, every model on (`existing` keeps the user's choices: the
/// default model, the models turned off, the fallback).
pub fn connection(url: &str, models: &[LocalModel], existing: Option<&Connection>) -> Connection {
    let mut conn = existing.cloned().unwrap_or_else(|| Connection {
        id: OLLAMA_CONNECTION.into(),
        name: "Modelos offline (Ollama)".into(),
        kind: ApiKind::Openai,
        base_url: String::new(),
        credential: Credential {
            source: CredentialSource::None,
            env_var: None,
        },
        headers: Default::default(),
        extra_body: Value::Null,
        models: Vec::new(),
        default_model: None,
        tool_mode: Some(ToolMode::Native),
        max_tool_rounds: crate::config::DEFAULT_MAX_TOOL_ROUNDS,
        max_output_tokens: None,
        options: Default::default(),
        generic: None,
        enabled: true,
        notes: Some("Criada pelo Orchestrator em Configurações → Modelos offline.".into()),
        // Local models take a while to load into memory.
        first_response_secs: Some(600),
        fallback: None,
    });
    conn.base_url = format!("{}/v1", url.trim_end_matches('/'));
    let previous = std::mem::take(&mut conn.models);
    conn.models = models
        .iter()
        .map(|local| {
            let mut entry = previous
                .iter()
                .find(|m| m.id == local.name)
                .cloned()
                .unwrap_or_else(|| ModelEntry::new(&local.name));
            entry.supports_tools = local.tools.or(entry.supports_tools);
            entry.supports_vision = local.vision.or(entry.supports_vision);
            entry.context_window = local
                .context_window
                .map(|c| c.min(u64::from(u32::MAX)) as u32)
                .or(entry.context_window);
            entry.input_price = Some(0.0);
            entry.output_price = Some(0.0);
            for tag in ["local", "offline"] {
                if !entry.tags.iter().any(|t| t == tag) {
                    entry.tags.push(tag.into());
                }
            }
            entry
        })
        .collect();
    if conn
        .default_model
        .as_ref()
        .is_some_and(|d| conn.model(d).is_none())
    {
        conn.default_model = None;
    }
    conn
}

/// The `ollama` program: on the PATH, or where the installers put it.
pub fn find_program() -> Option<PathBuf> {
    let exe = if cfg!(windows) { "ollama.exe" } else { "ollama" };
    if let Some(path) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path) {
            let candidate = dir.join(exe);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    let mut known = Vec::new();
    if cfg!(windows) {
        if let Some(local) = std::env::var_os("LOCALAPPDATA") {
            known.push(PathBuf::from(local).join("Programs/Ollama/ollama.exe"));
        }
    } else if cfg!(target_os = "macos") {
        known.push(PathBuf::from("/Applications/Ollama.app/Contents/Resources/ollama"));
        known.push(PathBuf::from("/opt/homebrew/bin/ollama"));
        known.push(PathBuf::from("/usr/local/bin/ollama"));
    } else {
        known.push(PathBuf::from("/usr/local/bin/ollama"));
        known.push(PathBuf::from("/usr/bin/ollama"));
    }
    known.into_iter().find(|p| p.is_file())
}
