//! Ready-made starting points for the "Adicionar API" form. They are only
//! templates: the user edits anything, and can register any other API with
//! the generic profile.

use crate::config::{
    ApiKind, Connection, Credential, CredentialSource, GenericAuth, GenericProfile, MessageFormat,
    ModelEntry, ProtocolOptions, StreamFormat, DEFAULT_MAX_TOOL_ROUNDS,
};
use serde::Serialize;
use serde_json::json;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Preset {
    pub key: &'static str,
    pub label: &'static str,
    pub hint: &'static str,
    pub connection: Connection,
}

fn base(
    id: &str,
    name: &str,
    kind: ApiKind,
    url: &str,
    credential: CredentialSource,
) -> Connection {
    Connection {
        id: id.into(),
        name: name.into(),
        kind,
        base_url: url.into(),
        credential: Credential {
            source: credential,
            env_var: None,
        },
        headers: Default::default(),
        extra_body: serde_json::Value::Null,
        models: Vec::new(),
        default_model: None,
        tool_mode: None,
        max_tool_rounds: DEFAULT_MAX_TOOL_ROUNDS,
        max_output_tokens: None,
        options: ProtocolOptions::default(),
        generic: None,
        enabled: true,
        notes: None,
        first_response_secs: None,
        fallback: None,
    }
}

fn claude_models() -> Vec<ModelEntry> {
    [
        "claude-opus-5-5",
        "claude-sonnet-5-5",
        "claude-haiku-4-5",
        "claude-fable-5-1",
    ]
    .iter()
    .map(|id| {
        let mut model = ModelEntry::new(*id);
        model.supports_tools = Some(true);
        if let Some(reference) = crate::anthropic::reference(id) {
            reference.fill(&mut model);
        }
        model
    })
    .collect()
}

pub fn presets() -> Vec<Preset> {
    let mut anthropic = base(
        "anthropic",
        "Anthropic (Claude)",
        ApiKind::Anthropic,
        "https://api.anthropic.com/v1",
        CredentialSource::Vault,
    );
    anthropic.models = claude_models();
    anthropic.default_model = Some("claude-opus-5-5".into());

    let ollama_native = {
        let mut c = base(
            "ollama-nativo",
            "Ollama (API nativa)",
            ApiKind::Generic,
            "http://localhost:11434",
            CredentialSource::None,
        );
        c.generic = Some(GenericProfile {
            path: "/api/chat".into(),
            auth: GenericAuth::None,
            message_format: MessageFormat::Chat,
            body: json!({
                "model": "{{model}}",
                "messages": "{{messages}}",
                "stream": "{{stream}}",
            }),
            stream: StreamFormat::Ndjson,
            text_path: "message.content".into(),
            done_path: Some("done".into()),
            done_marker: None,
            input_tokens_path: Some("prompt_eval_count".into()),
            output_tokens_path: Some("eval_count".into()),
            error_path: Some("error".into()),
            models_path: Some("/api/tags".into()),
            models_list_path: Some("models".into()),
            model_id_field: Some("name".into()),
            ..GenericProfile::default()
        });
        c
    };

    vec![
        Preset {
            key: "openai",
            label: "OpenAI",
            hint: "Chat Completions em api.openai.com. Use \"Buscar modelos\" depois de informar a chave.",
            connection: base(
                "openai",
                "OpenAI",
                ApiKind::Openai,
                "https://api.openai.com/v1",
                CredentialSource::Vault,
            ),
        },
        Preset {
            key: "anthropic",
            label: "Anthropic (Claude)",
            hint: "Messages API. Modelos atuais já listados com preços de referência (set/2026).",
            connection: anthropic,
        },
        Preset {
            key: "gemini",
            label: "Google Gemini",
            hint: "Gemini API (generativelanguage.googleapis.com). Use \"Buscar modelos\".",
            connection: base(
                "gemini",
                "Google Gemini",
                ApiKind::Gemini,
                "https://generativelanguage.googleapis.com/v1beta",
                CredentialSource::Vault,
            ),
        },
        Preset {
            key: "openrouter",
            label: "OpenRouter",
            hint: "Centenas de modelos numa chave só (compatível com OpenAI; traz preços).",
            connection: base(
                "openrouter",
                "OpenRouter",
                ApiKind::Openai,
                "https://openrouter.ai/api/v1",
                CredentialSource::Vault,
            ),
        },
        Preset {
            key: "openai-compatible",
            label: "Compatível com OpenAI",
            hint: "DeepSeek, Groq, Mistral, xAI, Together, vLLM, LM Studio… troque a URL base.",
            connection: base(
                "minha-api",
                "Minha API",
                ApiKind::Openai,
                "https://",
                CredentialSource::Vault,
            ),
        },
        Preset {
            key: "ollama",
            label: "Ollama local (compatível)",
            hint: "Modelos locais pelo endpoint compatível com OpenAI; sem chave.",
            connection: base(
                "ollama",
                "Ollama local",
                ApiKind::Openai,
                "http://localhost:11434/v1",
                CredentialSource::None,
            ),
        },
        Preset {
            key: "ollama-native",
            label: "Ollama (API nativa, perfil genérico)",
            hint: "Exemplo de API não compatível descrita por perfil: /api/chat em NDJSON.",
            connection: ollama_native,
        },
        Preset {
            key: "generic",
            label: "Qualquer API (perfil genérico)",
            hint: "Descreva endpoint, autenticação, corpo e onde ler a resposta. Ferramentas via protocolo por prompt.",
            connection: {
                let mut c = base(
                    "api-generica",
                    "API genérica",
                    ApiKind::Generic,
                    "https://",
                    CredentialSource::Vault,
                );
                c.generic = Some(GenericProfile::default());
                c
            },
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presets_are_valid_once_filled() {
        for preset in presets() {
            let mut c = preset.connection;
            if c.base_url == "https://" {
                c.base_url = "https://example.com".into();
            }
            c.validate()
                .unwrap_or_else(|e| panic!("{}: {}", preset.key, e.message));
        }
    }
}
