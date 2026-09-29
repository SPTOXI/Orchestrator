//! Google Gemini API (`generateContent` / `streamGenerateContent`).
//!
//! Model turns are kept as the API's own `parts` (`native`) and sent back
//! unchanged, preserving `thoughtSignature`s that function calling needs.

use crate::config::{Connection, ModelEntry, StreamFormat};
use crate::conversation::{Role, ToolCallPart};
use crate::http::{Frame, HttpCall};
use crate::jsonpath;
use crate::protocol::{
    query_sep, result_value, with_extra_body, with_headers, Decoder, Delta, Protocol, Reply,
    Request, Stop,
};
use crate::tools::{api_name, gemini_schema, orchestrator_name};
use orchestrator_providers::ProviderError;
use serde_json::{json, Value};

pub struct Gemini;

/// Appends a content, merging with the previous one when the role repeats
/// (e.g. tool results followed by a new user message).
fn push_content(contents: &mut Vec<Value>, role: &str, parts: Vec<Value>) {
    if let Some(last) = contents.last_mut() {
        if last["role"] == role {
            if let Some(existing) = last["parts"].as_array_mut() {
                existing.extend(parts);
                return;
            }
        }
    }
    contents.push(json!({"role": role, "parts": parts}));
}

fn model_path(model: &str) -> &str {
    model.strip_prefix("models/").unwrap_or(model)
}

fn auth(key: Option<&str>) -> Vec<(String, String)> {
    key.map(|k| vec![("x-goog-api-key".to_owned(), k.to_owned())])
        .unwrap_or_default()
}

impl Protocol for Gemini {
    fn request(&self, req: &Request<'_>) -> Result<HttpCall, ProviderError> {
        let mut contents = Vec::new();
        for message in req.messages {
            match message.role {
                Role::User => {
                    let mut parts = Vec::new();
                    for result in message.tool_results() {
                        let response = if result.is_error {
                            json!({"error": result.content})
                        } else {
                            json!({"output": result_value(&result.content)})
                        };
                        let mut call = json!({
                            "name": api_name(&result.name),
                            "response": response,
                        });
                        if result.native_id {
                            call["id"] = result.id.clone().into();
                        }
                        parts.push(json!({"functionResponse": call}));
                    }
                    let text = message.text();
                    if !text.is_empty() {
                        parts.push(json!({"text": text}));
                    }
                    if !parts.is_empty() {
                        push_content(&mut contents, "user", parts);
                    }
                }
                Role::Assistant => {
                    let parts = match &message.native {
                        Some(native) => native.clone(),
                        None => {
                            let mut parts = Vec::new();
                            let text = message.text();
                            if !text.is_empty() {
                                parts.push(json!({"text": text}));
                            }
                            for call in message.tool_calls() {
                                let mut function = json!({
                                    "name": api_name(&call.name),
                                    "args": call.args,
                                });
                                if call.native_id {
                                    function["id"] = call.id.clone().into();
                                }
                                parts.push(json!({"functionCall": function}));
                            }
                            Value::Array(parts)
                        }
                    };
                    if let Some(parts) = parts.as_array().filter(|p| !p.is_empty()) {
                        push_content(&mut contents, "model", parts.clone());
                    }
                }
            }
        }
        let mut body = json!({"contents": contents});
        if let Some(system) = req.system {
            body["systemInstruction"] = json!({"parts": [{"text": system}]});
        }
        if !req.tools.is_empty() {
            let declarations: Vec<Value> = req
                .tools
                .iter()
                .map(|tool| {
                    let mut declaration = json!({
                        "name": api_name(&tool.name),
                        "description": tool.description,
                    });
                    if let Some(parameters) = gemini_schema(&tool.parameters) {
                        declaration["parameters"] = parameters;
                    }
                    declaration
                })
                .collect();
            body["tools"] = json!([{"functionDeclarations": declarations}]);
        }
        if let Some(max) = req.model.max_output_tokens.or(req.conn.max_output_tokens) {
            body["generationConfig"] = json!({"maxOutputTokens": max});
        }
        let body = with_extra_body(body, req);
        let url = if req.stream {
            format!(
                "{}/models/{}:streamGenerateContent?alt=sse",
                req.conn.base(),
                model_path(&req.model.id)
            )
        } else {
            format!(
                "{}/models/{}:generateContent",
                req.conn.base(),
                model_path(&req.model.id)
            )
        };
        Ok(with_headers(
            HttpCall::post(url, body),
            auth(req.key),
            req.conn,
        ))
    }

    fn stream_format(&self, _conn: &Connection, stream: bool) -> StreamFormat {
        if stream {
            StreamFormat::Sse
        } else {
            StreamFormat::None
        }
    }

    fn decoder(&self, _req: &Request<'_>) -> Box<dyn Decoder> {
        Box::<GeminiDecoder>::default()
    }

    fn models_request(&self, conn: &Connection, key: Option<&str>) -> Option<HttpCall> {
        Some(with_headers(
            HttpCall::get(format!("{}/models?pageSize=1000", conn.base())),
            auth(key),
            conn,
        ))
    }

    fn next_models_page(
        &self,
        conn: &Connection,
        key: Option<&str>,
        body: &Value,
    ) -> Option<HttpCall> {
        let token = body.get("nextPageToken").and_then(Value::as_str)?;
        let url = format!("{}/models?pageSize=1000", conn.base());
        Some(with_headers(
            HttpCall::get(format!("{url}{}pageToken={token}", query_sep(&url))),
            auth(key),
            conn,
        ))
    }

    fn parse_models(&self, _conn: &Connection, body: &Value) -> Vec<ModelEntry> {
        body.get("models")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter(|item| {
                        item.get("supportedGenerationMethods")
                            .and_then(Value::as_array)
                            .is_none_or(|methods| methods.iter().any(|m| m == "generateContent"))
                    })
                    .filter_map(|item| {
                        let name = item.get("name").and_then(Value::as_str)?;
                        let mut model = ModelEntry::new(model_path(name));
                        model.name = item
                            .get("displayName")
                            .and_then(Value::as_str)
                            .map(str::to_owned);
                        model.context_window =
                            jsonpath::get_u64(item, "inputTokenLimit").map(|n| n as u32);
                        model.max_output_tokens =
                            jsonpath::get_u64(item, "outputTokenLimit").map(|n| n as u32);
                        Some(model)
                    })
                    .collect()
            })
            .unwrap_or_default()
    }
}

#[derive(Default)]
struct GeminiDecoder {
    text: String,
    parts: Vec<Value>,
    calls: Vec<ToolCallPart>,
    finish: Option<String>,
    blocked: Option<String>,
    usage: orchestrator_core::TokenUsage,
    served: Option<String>,
}

/// A plain text part (no signature, not a thought) that can be merged
/// with the previous one.
fn plain_text(part: &Value) -> Option<&str> {
    let object = part.as_object()?;
    if object.len() == 1 {
        object.get("text").and_then(Value::as_str)
    } else {
        None
    }
}

impl GeminiDecoder {
    fn chunk(&mut self, chunk: &Value) -> Result<Vec<Delta>, ProviderError> {
        if let Some(error) = chunk.get("error").filter(|e| !e.is_null()) {
            let message = jsonpath::get_text(error, "message").unwrap_or_else(|| error.to_string());
            return Err(ProviderError::failed(format!("API error: {message}")));
        }
        if let Some(version) = chunk.get("modelVersion").and_then(Value::as_str) {
            self.served = Some(version.to_owned());
        }
        if let Some(reason) = jsonpath::get_text(chunk, "promptFeedback.blockReason") {
            self.blocked = Some(reason);
        }
        if let Some(usage) = chunk.get("usageMetadata") {
            let prompt = jsonpath::get_u64(usage, "promptTokenCount").unwrap_or(0);
            let candidates = jsonpath::get_u64(usage, "candidatesTokenCount").unwrap_or(0);
            let thoughts = jsonpath::get_u64(usage, "thoughtsTokenCount").unwrap_or(0);
            self.usage.input_tokens = prompt;
            self.usage.output_tokens = candidates + thoughts;
            self.usage.reasoning_tokens = thoughts;
            self.usage.cached_input_tokens =
                jsonpath::get_u64(usage, "cachedContentTokenCount").unwrap_or(0);
        }
        let mut deltas = Vec::new();
        let Some(candidate) = chunk.get("candidates").and_then(|c| c.get(0)) else {
            return Ok(deltas);
        };
        if let Some(reason) = candidate.get("finishReason").and_then(Value::as_str) {
            self.finish = Some(reason.to_owned());
        }
        let parts = jsonpath::get(candidate, "content.parts")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        for part in parts {
            if let Some(text) = part.get("text").and_then(Value::as_str) {
                if part.get("thought").and_then(Value::as_bool) == Some(true) {
                    if !text.is_empty() {
                        deltas.push(Delta::Reasoning(text.to_owned()));
                    }
                } else if !text.is_empty() {
                    self.text.push_str(text);
                    deltas.push(Delta::Text(text.to_owned()));
                }
            }
            if let Some(call) = part.get("functionCall") {
                let id = call.get("id").and_then(Value::as_str).map(str::to_owned);
                self.calls.push(ToolCallPart {
                    native_id: id.is_some(),
                    id: id.unwrap_or_else(|| format!("gen_{}", self.calls.len() + 1)),
                    name: orchestrator_name(&jsonpath::get_text(call, "name").unwrap_or_default()),
                    args: call.get("args").cloned().unwrap_or_else(|| json!({})),
                    invalid: None,
                });
            }
            // Keep the parts for the history; merge consecutive plain text.
            match (plain_text(&part), self.parts.last_mut()) {
                (Some(more), Some(last)) if plain_text(last).is_some() => {
                    let merged = format!("{}{more}", plain_text(last).unwrap_or(""));
                    *last = json!({"text": merged});
                }
                _ => self.parts.push(part),
            }
        }
        Ok(deltas)
    }
}

impl Decoder for GeminiDecoder {
    fn feed(&mut self, frame: &Frame) -> Result<Vec<Delta>, ProviderError> {
        let data = frame.data.trim();
        if data.is_empty() {
            return Ok(Vec::new());
        }
        let value: Value = serde_json::from_str(data).map_err(|e| {
            ProviderError::failed(format!(
                "invalid JSON from the API ({e}): {}",
                crate::http::truncate(data, 200)
            ))
        })?;
        // Non-streaming `streamGenerateContent` without `alt=sse` returns
        // an array of chunks.
        match value {
            Value::Array(chunks) => {
                let mut deltas = Vec::new();
                for chunk in &chunks {
                    deltas.extend(self.chunk(chunk)?);
                }
                Ok(deltas)
            }
            chunk => self.chunk(&chunk),
        }
    }

    fn finish(self: Box<Self>) -> Result<Reply, ProviderError> {
        let blocked = |reason: &str| {
            matches!(
                reason,
                "SAFETY"
                    | "PROHIBITED_CONTENT"
                    | "BLOCKLIST"
                    | "RECITATION"
                    | "SPII"
                    | "IMAGE_SAFETY"
            )
        };
        let stop = match (&self.blocked, self.finish.as_deref()) {
            (Some(reason), _) => Stop::Refusal(reason.clone()),
            (None, Some(reason)) if blocked(reason) => Stop::Refusal(reason.to_owned()),
            (None, Some("MAX_TOKENS")) => Stop::MaxTokens,
            _ if !self.calls.is_empty() => Stop::ToolUse,
            (None, Some("STOP")) | (None, None) => Stop::End,
            (None, Some(other)) => Stop::Other(other.to_owned()),
        };
        Ok(Reply {
            text: self.text,
            tool_calls: self.calls,
            native: Some(Value::Array(self.parts)),
            usage: self.usage,
            stop,
            notices: Vec::new(),
            served_model: self.served,
        })
    }
}
