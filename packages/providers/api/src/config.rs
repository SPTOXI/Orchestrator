//! API connections registered by the user (ADR-0010). Persisted without
//! secrets; the key lives in the OS vault or an environment variable.

use orchestrator_providers::{ModelInfo, ProviderError};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

/// Wire protocol of a connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ApiKind {
    /// OpenAI Chat Completions, and every API compatible with it.
    Openai,
    /// Anthropic Messages API.
    Anthropic,
    /// Google Gemini API.
    Gemini,
    /// Any HTTP/JSON API described by a [`GenericProfile`].
    Generic,
}

impl ApiKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Openai => "OpenAI (compatível)",
            Self::Anthropic => "Anthropic",
            Self::Gemini => "Google Gemini",
            Self::Generic => "API genérica",
        }
    }
}

/// Where the API key comes from. The key itself is never stored here.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CredentialSource {
    /// No key (e.g. a local server).
    #[default]
    None,
    /// OS vault (Credential Manager / Keychain / Secret Service).
    Vault,
    /// Environment variable named in [`Credential::env_var`].
    Env,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Credential {
    #[serde(default)]
    pub source: CredentialSource,
    #[serde(default)]
    pub env_var: Option<String>,
}

/// How tools are offered to the model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ToolMode {
    /// The protocol's function calling (OpenAI, Anthropic, Gemini).
    Native,
    /// Tools described in the system prompt; `<tool_call>` blocks parsed
    /// from the reply. Works with any text model.
    Prompt,
    /// Chat only.
    None,
}

/// A model available through a connection, with what the Orchestrator
/// knows about it.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ModelEntry {
    pub id: String,
    pub name: Option<String>,
    pub context_window: Option<u32>,
    pub max_output_tokens: Option<u32>,
    pub supports_tools: Option<bool>,
    pub supports_vision: Option<bool>,
    /// USD per million input tokens (the APIs do not report prices).
    pub input_price: Option<f64>,
    /// USD per million output tokens.
    pub output_price: Option<f64>,
    /// USD per million input tokens read from the provider's prompt cache.
    /// Unset: the full input price (Anthropic: 10% of it). ADR-0018.
    pub cached_input_price: Option<f64>,
    /// Free labels ("código", "barato", "raciocínio"…).
    pub tags: Vec<String>,
    /// Extra fields merged into the request body for this model.
    pub extra_body: Value,
    #[serde(default = "enabled")]
    pub enabled: bool,
}

fn enabled() -> bool {
    true
}

impl ModelEntry {
    pub fn new(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            enabled: true,
            ..Default::default()
        }
    }

    pub fn display_name(&self) -> String {
        self.name.clone().unwrap_or_else(|| self.id.clone())
    }

    pub fn info(&self) -> ModelInfo {
        ModelInfo {
            id: self.id.clone(),
            name: self.display_name(),
            context_window: self.context_window,
            supports_tools: self.supports_tools,
            input_price: self.input_price,
            output_price: self.output_price,
            tags: self.tags.clone(),
        }
    }
}

/// Protocol-specific switches.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ProtocolOptions {
    /// OpenAI: ask for usage in the stream (`stream_options`). Default on.
    pub stream_usage: Option<bool>,
    /// Anthropic: stream tool inputs as they are generated
    /// (`eager_input_streaming`). Default on.
    pub eager_tool_streaming: Option<bool>,
    /// Anthropic (api.anthropic.com): server-side fallback when a model
    /// declines (`fallbacks: "default"`). Default on for the models that
    /// support it.
    pub refusal_fallback: Option<bool>,
    /// Prompt cache (ADR-0018): `cache_control` markers (Anthropic) and
    /// `prompt_cache_key` (OpenAI's own API). Default on.
    pub prompt_cache: Option<bool>,
    /// Anthropic: how long a cache entry lives. Default 5 minutes.
    pub cache_ttl: Option<CacheTtl>,
}

impl ProtocolOptions {
    pub fn prompt_cache(&self) -> bool {
        self.prompt_cache != Some(false)
    }
}

/// Lifetime of an Anthropic prompt cache entry. Writes cost 1.25× the
/// input price for 5 minutes and 2× for an hour.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum CacheTtl {
    #[default]
    #[serde(rename = "5m")]
    FiveMinutes,
    #[serde(rename = "1h")]
    OneHour,
}

impl CacheTtl {
    /// Cache write price as a multiple of the input price.
    pub fn write_multiplier(self) -> f64 {
        match self {
            Self::FiveMinutes => 1.25,
            Self::OneHour => 2.0,
        }
    }
}

/// How a generic API authenticates.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "type")]
pub enum GenericAuth {
    /// `Authorization: Bearer <key>`.
    #[default]
    Bearer,
    /// `<name>: <prefix><key>`.
    Header {
        name: String,
        #[serde(default)]
        prefix: String,
    },
    /// `?<param>=<key>`.
    Query {
        param: String,
    },
    None,
}

/// Shape of the conversation in a generic request.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MessageFormat {
    /// `[{"role": …, "content": "…"}]` (roles configurable).
    #[default]
    Chat,
    /// One text with the whole conversation.
    Prompt,
}

/// Response framing.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum StreamFormat {
    /// One JSON response.
    #[default]
    None,
    /// Server-sent events (`data: {…}`).
    Sse,
    /// One JSON object per line.
    Ndjson,
}

/// Role names used by [`MessageFormat::Chat`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct RoleNames {
    pub system: String,
    pub user: String,
    pub assistant: String,
}

impl Default for RoleNames {
    fn default() -> Self {
        Self {
            system: "system".into(),
            user: "user".into(),
            assistant: "assistant".into(),
        }
    }
}

/// Describes any HTTP/JSON chat API, without code.
///
/// Paths use dots and numeric indexes (`choices.0.message.content`). In the
/// body template, a string that is exactly `{{messages}}`, `{{stream}}` or
/// `{{maxTokens}}` becomes that JSON value; `{{model}}`, `{{system}}` and
/// `{{prompt}}` are replaced inside any string.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct GenericProfile {
    /// Appended to the base URL; may contain `{{model}}`.
    pub path: String,
    pub auth: GenericAuth,
    pub message_format: MessageFormat,
    pub roles: RoleNames,
    pub body: Value,
    pub stream: StreamFormat,
    /// Text of the reply (whole response, or each stream chunk).
    pub text_path: String,
    /// Stream chunk field that is `true` on the last chunk (NDJSON).
    pub done_path: Option<String>,
    /// SSE `data` value that ends the stream (e.g. `[DONE]`).
    pub done_marker: Option<String>,
    pub input_tokens_path: Option<String>,
    pub output_tokens_path: Option<String>,
    /// Error message in an error body or chunk.
    pub error_path: Option<String>,
    /// `GET` path listing models (optional).
    pub models_path: Option<String>,
    /// Array of models in that response (empty: the response itself).
    pub models_list_path: Option<String>,
    /// Model id field in each item (empty: the item is the id).
    pub model_id_field: Option<String>,
}

impl Default for GenericProfile {
    fn default() -> Self {
        Self {
            path: "/chat".into(),
            auth: GenericAuth::Bearer,
            message_format: MessageFormat::Chat,
            roles: RoleNames::default(),
            body: serde_json::json!({
                "model": "{{model}}",
                "messages": "{{messages}}",
                "stream": "{{stream}}",
            }),
            stream: StreamFormat::None,
            text_path: "message.content".into(),
            done_path: None,
            done_marker: None,
            input_tokens_path: None,
            output_tokens_path: None,
            error_path: Some("error.message".into()),
            models_path: None,
            models_list_path: None,
            model_id_field: None,
        }
    }
}

pub const DEFAULT_MAX_TOOL_ROUNDS: u32 = 50;

fn default_rounds() -> u32 {
    DEFAULT_MAX_TOOL_ROUNDS
}

/// One registered API.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Connection {
    /// Also the provider id (`openai-pessoal`, `claude-trabalho`…).
    pub id: String,
    pub name: String,
    pub kind: ApiKind,
    pub base_url: String,
    #[serde(default)]
    pub credential: Credential,
    /// Extra static headers (not secrets).
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    /// Extra fields merged into every request body.
    #[serde(default)]
    pub extra_body: Value,
    #[serde(default)]
    pub models: Vec<ModelEntry>,
    #[serde(default)]
    pub default_model: Option<String>,
    /// Default: native for OpenAI/Anthropic/Gemini, prompt for generic.
    #[serde(default)]
    pub tool_mode: Option<ToolMode>,
    /// Model → tools → model rounds per turn (visible limit; ADR-0010).
    #[serde(default = "default_rounds")]
    pub max_tool_rounds: u32,
    /// Output cap per request (Anthropic requires one; default 64000).
    #[serde(default)]
    pub max_output_tokens: Option<u32>,
    #[serde(default)]
    pub options: ProtocolOptions,
    #[serde(default)]
    pub generic: Option<GenericProfile>,
    #[serde(default = "enabled")]
    pub enabled: bool,
    #[serde(default)]
    pub notes: Option<String>,
}

impl Connection {
    pub fn tool_mode(&self) -> ToolMode {
        self.tool_mode.unwrap_or(match self.kind {
            ApiKind::Generic => ToolMode::Prompt,
            _ => ToolMode::Native,
        })
    }

    pub fn enabled_models(&self) -> impl Iterator<Item = &ModelEntry> {
        self.models.iter().filter(|m| m.enabled)
    }

    pub fn model(&self, id: &str) -> Option<&ModelEntry> {
        self.models.iter().find(|m| m.id == id)
    }

    /// Model used when a session does not choose one.
    pub fn default_model_id(&self) -> Option<String> {
        self.default_model
            .clone()
            .filter(|id| self.model(id).is_some_and(|m| m.enabled))
            .or_else(|| self.enabled_models().next().map(|m| m.id.clone()))
    }

    /// Base URL without a trailing slash.
    pub fn base(&self) -> &str {
        self.base_url.trim_end_matches('/')
    }

    /// Checks everything that can be checked without the network.
    pub fn validate(&self) -> Result<(), ProviderError> {
        let id_ok = !self.id.is_empty()
            && self.id.len() <= 48
            && self
                .id
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
            && !self.id.starts_with('-');
        if !id_ok {
            return Err(ProviderError::invalid(
                "id: use 1–48 lowercase letters, digits or '-' (e.g. openai-pessoal)",
            ));
        }
        if self.name.trim().is_empty() {
            return Err(ProviderError::invalid("name is required"));
        }
        if !(self.base_url.starts_with("http://") || self.base_url.starts_with("https://")) {
            return Err(ProviderError::invalid(
                "baseUrl must start with http:// or https://",
            ));
        }
        if self.credential.source == CredentialSource::Env
            && self
                .credential
                .env_var
                .as_deref()
                .is_none_or(|v| v.trim().is_empty())
        {
            return Err(ProviderError::invalid(
                "credential: name the environment variable",
            ));
        }
        if self.kind == ApiKind::Generic {
            let profile = self.generic.as_ref().ok_or_else(|| {
                ProviderError::invalid("generic connections need a profile (generic)")
            })?;
            if profile.text_path.trim().is_empty() {
                return Err(ProviderError::invalid("generic.textPath is required"));
            }
            if self.tool_mode() == ToolMode::Native {
                return Err(ProviderError::invalid(
                    "generic APIs use toolMode prompt or none (native function calling is protocol specific)",
                ));
            }
        }
        if !self.extra_body.is_null() && !self.extra_body.is_object() {
            return Err(ProviderError::invalid("extraBody must be a JSON object"));
        }
        let mut seen = std::collections::HashSet::new();
        for model in &self.models {
            if model.id.trim().is_empty() {
                return Err(ProviderError::invalid("every model needs an id"));
            }
            if !seen.insert(model.id.as_str()) {
                return Err(ProviderError::invalid(format!(
                    "model {} is listed twice",
                    model.id
                )));
            }
            if !model.extra_body.is_null() && !model.extra_body.is_object() {
                return Err(ProviderError::invalid(format!(
                    "model {}: extraBody must be a JSON object",
                    model.id
                )));
            }
        }
        if let Some(default) = &self.default_model {
            if self.model(default).is_none() {
                return Err(ProviderError::invalid(format!(
                    "defaultModel {default} is not in the model list"
                )));
            }
        }
        if self.max_tool_rounds == 0 {
            return Err(ProviderError::invalid("maxToolRounds must be at least 1"));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn openai() -> Connection {
        serde_json::from_value(json!({
            "id": "openai-pessoal",
            "name": "OpenAI pessoal",
            "kind": "openai",
            "baseUrl": "https://api.openai.com/v1/",
            "credential": {"source": "vault"},
            "models": [{"id": "gpt-x"}, {"id": "gpt-y", "enabled": false}]
        }))
        .unwrap()
    }

    #[test]
    fn defaults_fill_optional_fields() {
        let c = openai();
        assert_eq!(c.max_tool_rounds, DEFAULT_MAX_TOOL_ROUNDS);
        assert_eq!(c.tool_mode(), ToolMode::Native);
        assert!(c.enabled && c.models[0].enabled && !c.models[1].enabled);
        assert_eq!(c.base(), "https://api.openai.com/v1");
        assert_eq!(c.default_model_id().as_deref(), Some("gpt-x"));
        c.validate().unwrap();
    }

    #[test]
    fn validation_rejects_bad_configs() {
        let mut c = openai();
        c.id = "Com Espaço".into();
        assert!(c.validate().is_err());

        let mut c = openai();
        c.base_url = "api.openai.com".into();
        assert!(c.validate().is_err());

        let mut c = openai();
        c.credential = Credential {
            source: CredentialSource::Env,
            env_var: None,
        };
        assert!(c.validate().is_err());

        let mut c = openai();
        c.default_model = Some("nope".into());
        assert!(c.validate().is_err());

        let mut c = openai();
        c.kind = ApiKind::Generic;
        assert!(c.validate().is_err(), "generic needs a profile");
        c.generic = Some(GenericProfile::default());
        c.validate().unwrap();
        assert_eq!(c.tool_mode(), ToolMode::Prompt);
        c.tool_mode = Some(ToolMode::Native);
        assert!(c.validate().is_err());
    }
}
