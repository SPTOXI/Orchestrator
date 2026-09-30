//! What each API protocol implements: build the request, decode the
//! response (streamed or not) and list models.

use crate::config::{ApiKind, Connection, ModelEntry, StreamFormat};
use crate::conversation::{Message, ToolCallPart};
use crate::http::{Frame, HttpCall};
use crate::jsonpath;
use orchestrator_core::{TokenUsage, ToolDefinition};
use orchestrator_providers::ProviderError;
use serde_json::Value;

/// One model call.
pub struct Request<'a> {
    pub conn: &'a Connection,
    pub key: Option<&'a str>,
    pub model: &'a ModelEntry,
    pub system: Option<&'a str>,
    pub messages: &'a [Message],
    /// Native tools (empty in prompt/none modes).
    pub tools: &'a [ToolDefinition],
    pub stream: bool,
    /// The session the request belongs to: its requests share the
    /// vendor's prompt cache (markers on Anthropic, `prompt_cache_key` on
    /// OpenAI). `None` for one-off requests (completions, connection
    /// tests): a prompt that is never repeated would only pay for the
    /// cache write.
    pub cache_key: Option<&'a str>,
}

/// Incremental output for the UI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Delta {
    Text(String),
    Reasoning(String),
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum Stop {
    #[default]
    End,
    ToolUse,
    /// Output cut at the token limit (tool arguments may be incomplete).
    MaxTokens,
    /// The model or a safety system declined; the payload says why.
    Refusal(String),
    Other(String),
}

/// A decoded model reply.
#[derive(Debug, Clone, Default)]
pub struct Reply {
    pub text: String,
    pub tool_calls: Vec<ToolCallPart>,
    /// Provider-native assistant content to send back unchanged.
    pub native: Option<Value>,
    pub usage: TokenUsage,
    pub stop: Stop,
    /// Things worth telling the user (fallback model, truncation…).
    pub notices: Vec<String>,
    /// Model that actually answered, when the API says.
    pub served_model: Option<String>,
}

pub trait Decoder: Send {
    /// Consumes one frame (SSE event, NDJSON line or whole body).
    fn feed(&mut self, frame: &Frame) -> Result<Vec<Delta>, ProviderError>;
    /// True once the protocol signalled the end of the reply.
    fn done(&self) -> bool {
        false
    }
    fn finish(self: Box<Self>) -> Result<Reply, ProviderError>;
}

pub trait Protocol: Send + Sync {
    fn request(&self, req: &Request<'_>) -> Result<HttpCall, ProviderError>;
    fn stream_format(&self, conn: &Connection, stream: bool) -> StreamFormat;
    /// Whether this connection streams at all.
    fn streams(&self, _conn: &Connection) -> bool {
        true
    }
    fn decoder(&self, req: &Request<'_>) -> Box<dyn Decoder>;
    /// `GET` listing models, when the API has one.
    fn models_request(&self, conn: &Connection, key: Option<&str>) -> Option<HttpCall>;
    fn parse_models(&self, conn: &Connection, body: &Value) -> Vec<ModelEntry>;
    /// Next page of a paginated model list.
    fn next_models_page(
        &self,
        _conn: &Connection,
        _key: Option<&str>,
        _body: &Value,
    ) -> Option<HttpCall> {
        None
    }
}

pub fn protocol(kind: ApiKind) -> &'static dyn Protocol {
    match kind {
        ApiKind::Openai => &crate::openai::OpenAi,
        ApiKind::Anthropic => &crate::anthropic::Anthropic,
        ApiKind::Gemini => &crate::gemini::Gemini,
        ApiKind::Generic => &crate::generic::Generic,
    }
}

/// Merges the connection's and the model's extra body fields.
pub fn with_extra_body(mut body: Value, req: &Request<'_>) -> Value {
    jsonpath::merge(&mut body, &req.conn.extra_body);
    jsonpath::merge(&mut body, &req.model.extra_body);
    body
}

/// Adds protocol headers, then the user's (a user header with the same
/// name replaces the protocol one).
pub fn with_headers(
    mut call: HttpCall,
    protocol: Vec<(String, String)>,
    conn: &Connection,
) -> HttpCall {
    for (name, value) in protocol {
        if !conn
            .headers
            .keys()
            .any(|user| user.eq_ignore_ascii_case(&name))
        {
            call.headers.push((name, value));
        }
    }
    for (name, value) in &conn.headers {
        call.headers.push((name.clone(), value.clone()));
    }
    call
}

/// Parses tool arguments accumulated from a stream (`""` means `{}`).
pub fn parse_args(raw: &str) -> (Value, Option<String>) {
    if raw.trim().is_empty() {
        return (Value::Object(Default::default()), None);
    }
    match serde_json::from_str::<Value>(raw) {
        Ok(value @ Value::Object(_)) => (value, None),
        Ok(other) => (
            Value::Null,
            Some(format!("arguments must be a JSON object, got {other}")),
        ),
        Err(err) => (
            Value::Null,
            Some(format!("arguments are not valid JSON: {err}")),
        ),
    }
}

/// A result string as JSON when it is JSON, else as a string.
pub fn result_value(content: &str) -> Value {
    serde_json::from_str(content).unwrap_or_else(|_| Value::String(content.to_owned()))
}

/// `?` or `&`, to append a query parameter.
pub fn query_sep(url: &str) -> char {
    if url.contains('?') {
        '&'
    } else {
        '?'
    }
}
