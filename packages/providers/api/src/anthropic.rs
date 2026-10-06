//! Anthropic Messages API (raw HTTP: there is no official Rust SDK).
//!
//! - Assistant content is kept block by block (`native`) and sent back
//!   unchanged, so `thinking` blocks keep their signatures and the history
//!   stays append-only.
//! - `stop_reason: "refusal"` is handled before anything is used.
//! - Server-side fallback on refusals (`fallbacks: "default"`) is opted in
//!   by default for the models that support it on api.anthropic.com
//!   (connection option `refusalFallback`).

use crate::config::{CacheTtl, Connection, ModelEntry, StreamFormat};
use crate::conversation::{Role, ToolCallPart};
use crate::http::{Frame, HttpCall};
use crate::jsonpath;
use crate::protocol::{
    parse_args, query_sep, with_extra_body, with_headers, Decoder, Delta, Protocol, Reply, Request,
    Stop,
};
use crate::tools::{api_name, orchestrator_name};
use orchestrator_providers::ProviderError;
use serde_json::{json, Value};
use std::collections::BTreeMap;

pub const API_VERSION: &str = "2023-06-01";
/// Output cap when the connection/model sets none (streaming requests).
pub const DEFAULT_MAX_TOKENS: u32 = 64_000;
/// Output cap for non-streaming requests (keeps them under HTTP timeouts).
pub const DEFAULT_MAX_TOKENS_NO_STREAM: u32 = 16_000;
const FALLBACK_BETA: &str = "server-side-fallback-2026-07-01";
/// Models that accept `fallbacks: "default"` on the Claude API.
const FALLBACK_MODELS: &[&str] = &[
    "claude-fable-5-1",
    "claude-opus-5-5",
    "claude-opus-5",
    "claude-sonnet-5-5",
];

pub struct Anthropic;

/// Blocks that accept a `cache_control` marker.
const CACHEABLE: &[&str] = &["text", "tool_result", "tool_use", "image", "document"];

/// Marks the last block of the last message, so the next request reads the
/// whole conversation up to here from the cache.
fn mark_last_block(messages: &mut [Value], marker: &Value) {
    let block = messages
        .last_mut()
        .and_then(|message| message.get_mut("content"))
        .and_then(Value::as_array_mut)
        .and_then(|content| content.last_mut());
    if let Some(block) = block {
        let kind = block.get("type").and_then(Value::as_str).unwrap_or("");
        if CACHEABLE.contains(&kind) {
            block["cache_control"] = marker.clone();
        }
    }
}

fn auth_headers(key: Option<&str>) -> Vec<(String, String)> {
    let mut headers = vec![("anthropic-version".to_owned(), API_VERSION.to_owned())];
    if let Some(key) = key {
        headers.push(("x-api-key".to_owned(), key.to_owned()));
    }
    headers
}

fn uses_fallback(req: &Request<'_>) -> bool {
    req.conn.options.refusal_fallback != Some(false)
        && req.conn.base().contains("://api.anthropic.com")
        && FALLBACK_MODELS.contains(&req.model.id.as_str())
}

impl Protocol for Anthropic {
    fn request(&self, req: &Request<'_>) -> Result<HttpCall, ProviderError> {
        let mut messages = Vec::new();
        for message in req.messages {
            match message.role {
                Role::User => {
                    let mut content = Vec::new();
                    for result in message.tool_results() {
                        content.push(json!({
                            "type": "tool_result",
                            "tool_use_id": result.id,
                            "content": result.content,
                            "is_error": result.is_error,
                        }));
                    }
                    let text = message.text();
                    if !text.is_empty() {
                        content.push(json!({"type": "text", "text": text}));
                    }
                    if !content.is_empty() {
                        messages.push(json!({"role": "user", "content": content}));
                    }
                }
                Role::Assistant => {
                    let content = match &message.native {
                        Some(native) => native.clone(),
                        None => {
                            let mut blocks = Vec::new();
                            let text = message.text();
                            if !text.is_empty() {
                                blocks.push(json!({"type": "text", "text": text}));
                            }
                            for call in message.tool_calls() {
                                blocks.push(json!({
                                    "type": "tool_use",
                                    "id": call.id,
                                    "name": api_name(&call.name),
                                    "input": call.args,
                                }));
                            }
                            Value::Array(blocks)
                        }
                    };
                    if content.as_array().is_some_and(|c| !c.is_empty()) {
                        messages.push(json!({"role": "assistant", "content": content}));
                    }
                }
            }
        }
        let max_tokens = req
            .model
            .max_output_tokens
            .or(req.conn.max_output_tokens)
            .unwrap_or(if req.stream {
                DEFAULT_MAX_TOKENS
            } else {
                DEFAULT_MAX_TOKENS_NO_STREAM
            });
        // Prompt cache (ADR-0018): the tools (the same in every session of
        // this model), the system prompt and the conversation so far.
        let cached = req.cache_key.is_some() && req.conn.options.prompt_cache();
        let marker = cached.then(|| match req.conn.options.cache_ttl.unwrap_or_default() {
            CacheTtl::FiveMinutes => json!({"type": "ephemeral"}),
            CacheTtl::OneHour => json!({"type": "ephemeral", "ttl": "1h"}),
        });
        if let Some(marker) = &marker {
            mark_last_block(&mut messages, marker);
        }
        let mut body = json!({
            "model": req.model.id,
            "max_tokens": max_tokens,
            "messages": messages,
            "stream": req.stream,
        });
        if let Some(system) = req.system {
            body["system"] = match &marker {
                Some(marker) => json!([{"type": "text", "text": system, "cache_control": marker}]),
                None => system.into(),
            };
        }
        if !req.tools.is_empty() {
            let eager = req.stream && req.conn.options.eager_tool_streaming != Some(false);
            let mut tools: Vec<Value> = req
                .tools
                .iter()
                .map(|tool| {
                    let mut entry = json!({
                        "name": api_name(&tool.name),
                        "description": tool.description,
                        "input_schema": tool.parameters,
                    });
                    if eager {
                        entry["eager_input_streaming"] = true.into();
                    }
                    entry
                })
                .collect();
            if let (Some(marker), Some(last)) = (&marker, tools.last_mut()) {
                last["cache_control"] = marker.clone();
            }
            body["tools"] = Value::Array(tools);
        }
        let mut headers = auth_headers(req.key);
        if uses_fallback(req) {
            body["fallbacks"] = "default".into();
            // Merge with a user-provided beta header instead of replacing it.
            let user_beta = req
                .conn
                .headers
                .iter()
                .find(|(name, _)| name.eq_ignore_ascii_case("anthropic-beta"))
                .map(|(_, value)| value.clone());
            match user_beta {
                Some(existing) if !existing.contains(FALLBACK_BETA) => {
                    let mut conn = req.conn.clone();
                    conn.headers
                        .retain(|name, _| !name.eq_ignore_ascii_case("anthropic-beta"));
                    headers.push((
                        "anthropic-beta".into(),
                        format!("{existing},{FALLBACK_BETA}"),
                    ));
                    let body = with_extra_body(body, req);
                    return Ok(with_headers(
                        HttpCall::post(format!("{}/messages", req.conn.base()), body),
                        headers,
                        &conn,
                    ));
                }
                Some(_) => {}
                None => headers.push(("anthropic-beta".into(), FALLBACK_BETA.into())),
            }
        }
        let body = with_extra_body(body, req);
        Ok(with_headers(
            HttpCall::post(format!("{}/messages", req.conn.base()), body),
            headers,
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

    fn decoder(&self, req: &Request<'_>) -> Box<dyn Decoder> {
        Box::new(AnthropicDecoder {
            requested: req.model.id.clone(),
            ..Default::default()
        })
    }

    fn models_request(&self, conn: &Connection, key: Option<&str>) -> Option<HttpCall> {
        Some(with_headers(
            HttpCall::get(format!("{}/models?limit=1000", conn.base())),
            auth_headers(key),
            conn,
        ))
    }

    fn next_models_page(
        &self,
        conn: &Connection,
        key: Option<&str>,
        body: &Value,
    ) -> Option<HttpCall> {
        if body.get("has_more").and_then(Value::as_bool) != Some(true) {
            return None;
        }
        let last = body.get("last_id").and_then(Value::as_str)?;
        let url = format!("{}/models?limit=1000", conn.base());
        Some(with_headers(
            HttpCall::get(format!("{url}{}after_id={last}", query_sep(&url))),
            auth_headers(key),
            conn,
        ))
    }

    fn parse_models(&self, _conn: &Connection, body: &Value) -> Vec<ModelEntry> {
        body.get("data")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| {
                        let id = item.get("id").and_then(Value::as_str)?;
                        let mut model = ModelEntry::new(id);
                        model.name = item
                            .get("display_name")
                            .and_then(Value::as_str)
                            .map(str::to_owned);
                        model.context_window =
                            jsonpath::get_u64(item, "max_input_tokens").map(|n| n as u32);
                        model.max_output_tokens =
                            jsonpath::get_u64(item, "max_tokens").map(|n| n as u32);
                        model.supports_tools = Some(true);
                        Some(model)
                    })
                    .collect()
            })
            .unwrap_or_default()
    }
}

#[derive(Default)]
struct Block {
    value: Value,
    partial_json: String,
}

#[derive(Default)]
struct AnthropicDecoder {
    requested: String,
    blocks: BTreeMap<u64, Block>,
    stop_reason: Option<String>,
    refusal_category: Option<String>,
    usage: orchestrator_core::TokenUsage,
    served: Option<String>,
    done: bool,
}

impl AnthropicDecoder {
    fn set_input_usage(&mut self, usage: &Value) {
        let input = jsonpath::get_u64(usage, "input_tokens").unwrap_or(0);
        let read = jsonpath::get_u64(usage, "cache_read_input_tokens").unwrap_or(0);
        let written = jsonpath::get_u64(usage, "cache_creation_input_tokens").unwrap_or(0);
        if input + read + written > 0 {
            self.usage.input_tokens = input + read + written;
            self.usage.cached_input_tokens = read;
            self.usage.cache_write_tokens = written;
        }
        if let Some(output) = jsonpath::get_u64(usage, "output_tokens") {
            self.usage.output_tokens = output;
        }
    }

    fn event(&mut self, event: &Value) -> Result<Vec<Delta>, ProviderError> {
        let mut deltas = Vec::new();
        match event.get("type").and_then(Value::as_str).unwrap_or("") {
            "message_start" => {
                if let Some(message) = event.get("message") {
                    if let Some(model) = message.get("model").and_then(Value::as_str) {
                        self.served = Some(model.to_owned());
                    }
                    if let Some(usage) = message.get("usage") {
                        self.set_input_usage(usage);
                    }
                }
            }
            "content_block_start" => {
                let index = event.get("index").and_then(Value::as_u64).unwrap_or(0);
                let block = event
                    .get("content_block")
                    .cloned()
                    .unwrap_or_else(|| json!({}));
                if block.get("type").and_then(Value::as_str) == Some("text") {
                    if let Some(text) = block.get("text").and_then(Value::as_str) {
                        if !text.is_empty() {
                            deltas.push(Delta::Text(text.to_owned()));
                        }
                    }
                }
                self.blocks.insert(
                    index,
                    Block {
                        value: block,
                        partial_json: String::new(),
                    },
                );
            }
            "content_block_delta" => {
                let index = event.get("index").and_then(Value::as_u64).unwrap_or(0);
                let Some(delta) = event.get("delta") else {
                    return Ok(deltas);
                };
                let block = self.blocks.entry(index).or_default();
                let append = |value: &mut Value, field: &str, extra: &str| {
                    let current = value.get(field).and_then(Value::as_str).unwrap_or("");
                    value[field] = format!("{current}{extra}").into();
                };
                match delta.get("type").and_then(Value::as_str).unwrap_or("") {
                    "text_delta" => {
                        let text = delta.get("text").and_then(Value::as_str).unwrap_or("");
                        append(&mut block.value, "text", text);
                        if !text.is_empty() {
                            deltas.push(Delta::Text(text.to_owned()));
                        }
                    }
                    "thinking_delta" => {
                        let text = delta.get("thinking").and_then(Value::as_str).unwrap_or("");
                        append(&mut block.value, "thinking", text);
                        if !text.is_empty() {
                            deltas.push(Delta::Reasoning(text.to_owned()));
                        }
                    }
                    "signature_delta" => {
                        let signature =
                            delta.get("signature").and_then(Value::as_str).unwrap_or("");
                        append(&mut block.value, "signature", signature);
                    }
                    "input_json_delta" => {
                        if let Some(part) = delta.get("partial_json").and_then(Value::as_str) {
                            block.partial_json.push_str(part);
                        }
                    }
                    _ => {}
                }
            }
            "content_block_stop" => {
                let index = event.get("index").and_then(Value::as_u64).unwrap_or(0);
                if let Some(block) = self.blocks.get_mut(&index) {
                    if block.value.get("type").and_then(Value::as_str) == Some("tool_use")
                        && !block.partial_json.is_empty()
                    {
                        let (input, invalid) = parse_args(&block.partial_json);
                        match invalid {
                            None => block.value["input"] = input,
                            Some(reason) => block.value["_invalid"] = reason.into(),
                        }
                    }
                }
            }
            "message_delta" => {
                if let Some(reason) = jsonpath::get_text(event, "delta.stop_reason") {
                    self.stop_reason = Some(reason);
                }
                if let Some(category) = jsonpath::get_text(event, "delta.stop_details.category")
                    .or_else(|| jsonpath::get_text(event, "stop_details.category"))
                {
                    self.refusal_category = Some(category);
                }
                if let Some(usage) = event.get("usage") {
                    self.set_input_usage(usage);
                }
            }
            "message_stop" => self.done = true,
            "error" => {
                let message =
                    jsonpath::get_text(event, "error.message").unwrap_or_else(|| event.to_string());
                let kind = jsonpath::get_text(event, "error.type").unwrap_or_default();
                return Err(ProviderError::failed(format!(
                    "API error ({kind}): {message}"
                )));
            }
            // A whole (non-streaming) message.
            "message" => {
                self.served = event
                    .get("model")
                    .and_then(Value::as_str)
                    .map(str::to_owned);
                if let Some(usage) = event.get("usage") {
                    self.set_input_usage(usage);
                }
                self.stop_reason = jsonpath::get_text(event, "stop_reason");
                self.refusal_category = jsonpath::get_text(event, "stop_details.category");
                if let Some(content) = event.get("content").and_then(Value::as_array) {
                    for (index, block) in content.iter().enumerate() {
                        match block.get("type").and_then(Value::as_str) {
                            Some("text") => {
                                if let Some(text) = block.get("text").and_then(Value::as_str) {
                                    deltas.push(Delta::Text(text.to_owned()));
                                }
                            }
                            Some("thinking") => {
                                if let Some(text) = block.get("thinking").and_then(Value::as_str) {
                                    if !text.is_empty() {
                                        deltas.push(Delta::Reasoning(text.to_owned()));
                                    }
                                }
                            }
                            _ => {}
                        }
                        self.blocks.insert(
                            index as u64,
                            Block {
                                value: block.clone(),
                                partial_json: String::new(),
                            },
                        );
                    }
                }
                self.done = true;
            }
            _ => {}
        }
        Ok(deltas)
    }
}

impl Decoder for AnthropicDecoder {
    fn feed(&mut self, frame: &Frame) -> Result<Vec<Delta>, ProviderError> {
        let data = frame.data.trim();
        if data.is_empty() {
            return Ok(Vec::new());
        }
        let event: Value = serde_json::from_str(data).map_err(|e| {
            ProviderError::failed(format!(
                "invalid JSON from the API ({e}): {}",
                crate::http::truncate(data, 200)
            ))
        })?;
        self.event(&event)
    }

    fn done(&self) -> bool {
        self.done
    }

    fn finish(self: Box<Self>) -> Result<Reply, ProviderError> {
        let mut blocks: Vec<Value> = self.blocks.into_values().map(|b| b.value).collect();
        let mut notices = Vec::new();

        // Server-side fallback: blocks before the last `fallback` marker come
        // from the model that declined. Keep its text, drop the rest (thinking,
        // tool calls, unknown blocks) and the marker itself.
        if let Some(boundary) = blocks
            .iter()
            .rposition(|b| b.get("type").and_then(Value::as_str) == Some("fallback"))
        {
            let marker = &blocks[boundary];
            let from = jsonpath::get_text(marker, "from.model").unwrap_or_else(|| "?".into());
            let to = jsonpath::get_text(marker, "to.model").unwrap_or_else(|| "?".into());
            notices.push(format!(
                "{from} recusou; {to} continuou a resposta (fallback)"
            ));
            blocks = blocks
                .into_iter()
                .enumerate()
                .filter(|(i, b)| {
                    *i > boundary
                        || (*i < boundary && b.get("type").and_then(Value::as_str) == Some("text"))
                })
                .map(|(_, b)| b)
                .collect();
        }

        let mut text = String::new();
        let mut tool_calls = Vec::new();
        let mut native = Vec::new();
        for mut block in blocks {
            match block.get("type").and_then(Value::as_str) {
                Some("text") => {
                    let body = block.get("text").and_then(Value::as_str).unwrap_or("");
                    if body.is_empty() {
                        continue; // the API rejects empty text blocks
                    }
                    text.push_str(body);
                }
                Some("tool_use") => {
                    let invalid = block
                        .as_object_mut()
                        .and_then(|o| o.remove("_invalid"))
                        .and_then(|v| v.as_str().map(str::to_owned));
                    if block.get("input").is_none() {
                        block["input"] = json!({});
                    }
                    tool_calls.push(ToolCallPart {
                        id: jsonpath::get_text(&block, "id").unwrap_or_default(),
                        name: orchestrator_name(
                            &jsonpath::get_text(&block, "name").unwrap_or_default(),
                        ),
                        args: if invalid.is_some() {
                            Value::Null
                        } else {
                            block["input"].clone()
                        },
                        native_id: true,
                        invalid,
                    });
                }
                _ => {}
            }
            native.push(block);
        }

        if let Some(served) = &self.served {
            if !self.requested.is_empty() && served != &self.requested && notices.is_empty() {
                notices.push(format!(
                    "respondido por {served} (pedido: {})",
                    self.requested
                ));
            }
        }
        let stop = match self.stop_reason.as_deref() {
            Some("tool_use") => Stop::ToolUse,
            Some("max_tokens") => Stop::MaxTokens,
            Some("refusal") => Stop::Refusal(
                self.refusal_category
                    .unwrap_or_else(|| "sem categoria".into()),
            ),
            Some("end_turn") | Some("stop_sequence") | None => {
                if tool_calls.is_empty() {
                    Stop::End
                } else {
                    Stop::ToolUse
                }
            }
            Some(other) => Stop::Other(other.to_owned()),
        };
        Ok(Reply {
            text,
            tool_calls,
            native: Some(Value::Array(native)),
            usage: self.usage,
            stop,
            notices,
            served_model: self.served,
            served_by: None,
        })
    }
}

/// Reference prices and limits of a Claude model (USD per million tokens;
/// Anthropic first-party rates, as of September 2026). Used to pre-fill
/// models found by "Buscar modelos"; the user can edit them.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Reference {
    pub input: f64,
    pub output: f64,
    /// Cache reads (ADR-0018).
    pub cache_read: f64,
    pub context: u32,
    pub tags: &'static [&'static str],
}

impl Reference {
    /// Fills what `model` does not say yet.
    pub fn fill(&self, model: &mut ModelEntry) {
        model.input_price.get_or_insert(self.input);
        model.output_price.get_or_insert(self.output);
        model.cached_input_price.get_or_insert(self.cache_read);
        model.context_window.get_or_insert(self.context);
        if model.tags.is_empty() {
            model.tags = self.tags.iter().map(|t| (*t).to_owned()).collect();
        }
    }
}

pub fn reference(model: &str) -> Option<Reference> {
    let r = |input, output, cache_read, context, tags| Reference {
        input,
        output,
        cache_read,
        context,
        tags,
    };
    Some(match model {
        "claude-fable-5-1" => r(
            10.0,
            50.0,
            0.25,
            1_000_000,
            &["máxima capacidade", "raciocínio"],
        ),
        "claude-fable-5" => r(
            10.0,
            50.0,
            1.0,
            1_000_000,
            &["máxima capacidade", "raciocínio"],
        ),
        "claude-opus-5-5" => r(
            4.0,
            20.0,
            0.2,
            1_000_000,
            &["código", "raciocínio", "agentes"],
        ),
        "claude-opus-5" | "claude-opus-4-8" | "claude-opus-4-7" | "claude-opus-4-6" => {
            r(5.0, 25.0, 0.5, 1_000_000, &["código", "raciocínio"])
        }
        "claude-sonnet-5-5" | "claude-sonnet-5" => {
            r(2.0, 10.0, 0.2, 1_000_000, &["código", "rápido"])
        }
        "claude-sonnet-4-6" => r(3.0, 15.0, 0.3, 1_000_000, &["código", "rápido"]),
        "claude-haiku-4-5" => r(1.0, 5.0, 0.1, 200_000, &["barato", "rápido"]),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Connection;
    use crate::conversation::Message;

    fn conn(base: &str, headers: &[(&str, &str)]) -> Connection {
        let mut conn: Connection = serde_json::from_value(json!({
            "id": "c", "name": "C", "kind": "anthropic", "baseUrl": base,
            "models": [{"id": "claude-opus-5-5"}]
        }))
        .unwrap();
        for (name, value) in headers {
            conn.headers.insert((*name).into(), (*value).into());
        }
        conn
    }

    fn call(conn: &Connection) -> HttpCall {
        let model = conn.models[0].clone();
        Anthropic
            .request(&Request {
                conn,
                key: Some("k"),
                model: &model,
                system: None,
                messages: &[Message::user("oi")],
                tools: &[],
                stream: true,
                cache_key: None,
            })
            .unwrap()
    }

    fn header<'a>(call: &'a HttpCall, name: &str) -> Vec<&'a str> {
        call.headers
            .iter()
            .filter(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
            .collect()
    }

    #[test]
    fn server_side_fallback_is_opted_in_on_the_claude_api() {
        let direct = call(&conn("https://api.anthropic.com/v1", &[]));
        assert_eq!(direct.body.as_ref().unwrap()["fallbacks"], "default");
        assert_eq!(header(&direct, "anthropic-beta"), vec![FALLBACK_BETA]);
        assert_eq!(direct.url, "https://api.anthropic.com/v1/messages");

        let merged = call(&conn(
            "https://api.anthropic.com/v1",
            &[("anthropic-beta", "outra-beta")],
        ));
        assert_eq!(
            header(&merged, "anthropic-beta"),
            vec![format!("outra-beta,{FALLBACK_BETA}").as_str()]
        );

        let proxy = call(&conn("https://proxy.example.com/v1", &[]));
        assert!(proxy.body.as_ref().unwrap().get("fallbacks").is_none());
        assert!(header(&proxy, "anthropic-beta").is_empty());

        let mut off = conn("https://api.anthropic.com/v1", &[]);
        off.options.refusal_fallback = Some(false);
        assert!(call(&off).body.as_ref().unwrap().get("fallbacks").is_none());
    }

    fn frame(value: Value) -> Frame {
        Frame {
            event: None,
            data: value.to_string(),
        }
    }

    #[test]
    fn a_mid_stream_fallback_keeps_text_and_drops_the_declined_attempt() {
        let mut decoder = Box::new(AnthropicDecoder {
            requested: "claude-opus-5-5".into(),
            ..Default::default()
        });
        let events = [
            json!({"type": "message_start", "message": {"model": "claude-opus-5", "usage": {"input_tokens": 3}}}),
            json!({"type": "content_block_start", "index": 0, "content_block": {"type": "thinking", "thinking": "", "signature": ""}}),
            json!({"type": "content_block_start", "index": 1, "content_block": {"type": "text", "text": "parcial "}}),
            json!({"type": "content_block_start", "index": 2, "content_block": {"type": "tool_use", "id": "t0", "name": "x__y", "input": {}}}),
            json!({"type": "content_block_start", "index": 3, "content_block": {"type": "fallback", "from": {"model": "claude-opus-5-5"}, "to": {"model": "claude-opus-5"}}}),
            json!({"type": "content_block_start", "index": 4, "content_block": {"type": "text", "text": ""}}),
            json!({"type": "content_block_delta", "index": 4, "delta": {"type": "text_delta", "text": "continuação"}}),
            json!({"type": "message_delta", "delta": {"stop_reason": "end_turn"}, "usage": {"output_tokens": 9}}),
            json!({"type": "message_stop"}),
        ];
        for event in events {
            decoder.feed(&frame(event)).unwrap();
        }
        assert!(decoder.done());
        let reply = decoder.finish().unwrap();
        assert_eq!(reply.text, "parcial continuação");
        assert!(
            reply.tool_calls.is_empty(),
            "the declined attempt's tool call never runs"
        );
        assert_eq!(
            reply.native.unwrap(),
            json!([{"type": "text", "text": "parcial "}, {"type": "text", "text": "continuação"}])
        );
        assert!(reply.notices[0].contains("claude-opus-5-5 recusou"));
        assert_eq!(reply.stop, Stop::End);
    }
}
