//! OpenAI Chat Completions — and every API compatible with it (OpenRouter,
//! DeepSeek, Groq, Mistral, Ollama, LM Studio, vLLM…), by base URL.

use crate::config::{Connection, ModelEntry, StreamFormat};
use crate::conversation::{Role, ToolCallPart};
use crate::http::{Frame, HttpCall};
use crate::jsonpath;
use crate::protocol::{
    parse_args, with_extra_body, with_headers, Decoder, Delta, Protocol, Reply, Request, Stop,
};
use crate::tools::{api_name, orchestrator_name};
use orchestrator_providers::ProviderError;
use serde_json::{json, Value};
use std::collections::BTreeMap;

pub struct OpenAi;

impl Protocol for OpenAi {
    fn request(&self, req: &Request<'_>) -> Result<HttpCall, ProviderError> {
        let mut messages = Vec::new();
        if let Some(system) = req.system {
            messages.push(json!({"role": "system", "content": system}));
        }
        for message in req.messages {
            match message.role {
                Role::User => {
                    for result in message.tool_results() {
                        messages.push(json!({
                            "role": "tool",
                            "tool_call_id": result.id,
                            "content": result.content,
                        }));
                    }
                    let text = message.text();
                    if !text.is_empty() {
                        messages.push(json!({"role": "user", "content": text}));
                    }
                }
                Role::Assistant => {
                    let calls: Vec<Value> = message
                        .tool_calls()
                        .map(|call| {
                            json!({
                                "id": call.id,
                                "type": "function",
                                "function": {
                                    "name": api_name(&call.name),
                                    "arguments": call.args.to_string(),
                                }
                            })
                        })
                        .collect();
                    let text = message.text();
                    let mut entry = json!({"role": "assistant", "content": text});
                    if !calls.is_empty() {
                        entry["tool_calls"] = Value::Array(calls);
                        if text.is_empty() {
                            entry["content"] = Value::Null;
                        }
                    }
                    messages.push(entry);
                }
            }
        }
        let mut body = json!({
            "model": req.model.id,
            "messages": messages,
            "stream": req.stream,
        });
        if req.stream && req.conn.options.stream_usage != Some(false) {
            body["stream_options"] = json!({"include_usage": true});
        }
        if !req.tools.is_empty() {
            body["tools"] = Value::Array(
                req.tools
                    .iter()
                    .map(|tool| {
                        json!({
                            "type": "function",
                            "function": {
                                "name": api_name(&tool.name),
                                "description": tool.description,
                                "parameters": tool.parameters,
                            }
                        })
                    })
                    .collect(),
            );
        }
        if let Some(max) = req.model.max_output_tokens.or(req.conn.max_output_tokens) {
            body["max_completion_tokens"] = max.into();
        }
        // The cache is automatic; the key keeps one conversation's requests
        // on the same cache (ADR-0018). Only OpenAI's own API: compatible
        // servers may reject an unknown field.
        if let Some(key) = req.cache_key.filter(|_| {
            req.conn.options.prompt_cache() && req.conn.base().contains("://api.openai.com")
        }) {
            body["prompt_cache_key"] = key.into();
        }
        let body = with_extra_body(body, req);
        let mut headers = Vec::new();
        if let Some(key) = req.key {
            headers.push(("Authorization".to_owned(), format!("Bearer {key}")));
        }
        Ok(with_headers(
            HttpCall::post(format!("{}/chat/completions", req.conn.base()), body),
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
        Box::new(OpenAiDecoder {
            requested: req.model.id.clone(),
            ..Default::default()
        })
    }

    fn models_request(&self, conn: &Connection, key: Option<&str>) -> Option<HttpCall> {
        let mut headers = Vec::new();
        if let Some(key) = key {
            headers.push(("Authorization".to_owned(), format!("Bearer {key}")));
        }
        Some(with_headers(
            HttpCall::get(format!("{}/models", conn.base())),
            headers,
            conn,
        ))
    }

    fn parse_models(&self, _conn: &Connection, body: &Value) -> Vec<ModelEntry> {
        let items = body
            .get("data")
            .or_else(|| body.get("models"))
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        items
            .iter()
            .filter_map(|item| {
                let id = item.get("id").and_then(Value::as_str)?;
                let mut model = ModelEntry::new(id);
                model.name = item.get("name").and_then(Value::as_str).map(str::to_owned);
                model.context_window = jsonpath::get_u64(item, "context_length")
                    .or_else(|| jsonpath::get_u64(item, "context_window"))
                    .map(|n| n as u32);
                // OpenRouter publishes USD per token.
                let per_million = |path: &str| {
                    jsonpath::get_text(item, path)
                        .and_then(|p| p.parse::<f64>().ok())
                        .map(|p| p * 1_000_000.0)
                };
                model.input_price = per_million("pricing.prompt");
                model.output_price = per_million("pricing.completion");
                Some(model)
            })
            .collect()
    }
}

#[derive(Default)]
struct PendingCall {
    id: String,
    name: String,
    arguments: String,
}

#[derive(Default)]
struct OpenAiDecoder {
    requested: String,
    text: String,
    calls: BTreeMap<u64, PendingCall>,
    finish_reason: Option<String>,
    usage: orchestrator_core::TokenUsage,
    served: Option<String>,
    done: bool,
}

impl OpenAiDecoder {
    fn absorb(&mut self, chunk: &Value) -> Result<Vec<Delta>, ProviderError> {
        if let Some(error) = chunk.get("error").filter(|e| !e.is_null()) {
            let message = jsonpath::get_text(error, "message").unwrap_or_else(|| error.to_string());
            return Err(ProviderError::failed(format!("API error: {message}")));
        }
        if let Some(model) = chunk.get("model").and_then(Value::as_str) {
            self.served = Some(model.to_owned());
        }
        if let Some(usage) = chunk.get("usage").filter(|u| u.is_object()) {
            self.usage.input_tokens = jsonpath::get_u64(usage, "prompt_tokens").unwrap_or(0);
            self.usage.output_tokens = jsonpath::get_u64(usage, "completion_tokens").unwrap_or(0);
            self.usage.cached_input_tokens =
                jsonpath::get_u64(usage, "prompt_tokens_details.cached_tokens").unwrap_or(0);
            self.usage.reasoning_tokens =
                jsonpath::get_u64(usage, "completion_tokens_details.reasoning_tokens").unwrap_or(0);
        }
        let mut deltas = Vec::new();
        let Some(choice) = chunk.get("choices").and_then(|c| c.get(0)) else {
            return Ok(deltas);
        };
        // Streaming chunks carry `delta`; whole responses carry `message`.
        let body = choice.get("delta").or_else(|| choice.get("message"));
        if let Some(body) = body {
            for field in ["reasoning_content", "reasoning"] {
                if let Some(text) = body.get(field).and_then(Value::as_str) {
                    if !text.is_empty() {
                        deltas.push(Delta::Reasoning(text.to_owned()));
                    }
                }
            }
            if let Some(text) = body.get("content").and_then(Value::as_str) {
                if !text.is_empty() {
                    self.text.push_str(text);
                    deltas.push(Delta::Text(text.to_owned()));
                }
            }
            if let Some(calls) = body.get("tool_calls").and_then(Value::as_array) {
                for (position, call) in calls.iter().enumerate() {
                    let index = call
                        .get("index")
                        .and_then(Value::as_u64)
                        .unwrap_or(position as u64);
                    let pending = self.calls.entry(index).or_default();
                    if let Some(id) = call.get("id").and_then(Value::as_str) {
                        pending.id = id.to_owned();
                    }
                    if let Some(name) = jsonpath::get_text(call, "function.name") {
                        pending.name.push_str(&name);
                    }
                    if let Some(args) = jsonpath::get(call, "function.arguments") {
                        match args {
                            Value::String(s) => pending.arguments.push_str(s),
                            other => pending.arguments.push_str(&other.to_string()),
                        }
                    }
                }
            }
        }
        if let Some(reason) = choice.get("finish_reason").and_then(Value::as_str) {
            self.finish_reason = Some(reason.to_owned());
        }
        Ok(deltas)
    }
}

impl Decoder for OpenAiDecoder {
    fn feed(&mut self, frame: &Frame) -> Result<Vec<Delta>, ProviderError> {
        let data = frame.data.trim();
        if data == "[DONE]" {
            self.done = true;
            return Ok(Vec::new());
        }
        if data.is_empty() {
            return Ok(Vec::new());
        }
        let chunk: Value = serde_json::from_str(data).map_err(|e| {
            ProviderError::failed(format!(
                "invalid JSON from the API ({e}): {}",
                crate::http::truncate(data, 200)
            ))
        })?;
        self.absorb(&chunk)
    }

    fn done(&self) -> bool {
        self.done
    }

    fn finish(self: Box<Self>) -> Result<Reply, ProviderError> {
        let mut tool_calls = Vec::new();
        for (index, call) in self.calls {
            let (args, invalid) = parse_args(&call.arguments);
            let native_id = !call.id.is_empty();
            tool_calls.push(ToolCallPart {
                id: if native_id {
                    call.id
                } else {
                    format!("call_{index}")
                },
                name: orchestrator_name(&call.name),
                args,
                native_id,
                invalid,
            });
        }
        let stop = match self.finish_reason.as_deref() {
            Some("length") => Stop::MaxTokens,
            Some("content_filter") => Stop::Refusal("content_filter".into()),
            Some("tool_calls") | Some("function_call") => Stop::ToolUse,
            _ if !tool_calls.is_empty() => Stop::ToolUse,
            _ => Stop::End,
        };
        let mut notices = Vec::new();
        if let Some(served) = &self.served {
            if !served.starts_with(&self.requested) && !self.requested.is_empty() {
                notices.push(format!(
                    "respondido por {served} (pedido: {})",
                    self.requested
                ));
            }
        }
        Ok(Reply {
            text: self.text,
            tool_calls,
            native: None,
            usage: self.usage,
            stop,
            notices,
            served_model: self.served,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conversation::Message;
    use serde_json::json;

    fn body(base: &str, options: Value, cache_key: Option<&str>) -> Value {
        let conn: Connection = serde_json::from_value(json!({
            "id": "c", "name": "C", "kind": "openai", "baseUrl": base,
            "options": options, "models": [{"id": "gpt-test"}]
        }))
        .unwrap();
        let model = conn.models[0].clone();
        OpenAi
            .request(&Request {
                conn: &conn,
                key: Some("k"),
                model: &model,
                system: Some("sys"),
                messages: &[Message::user("oi")],
                tools: &[],
                stream: true,
                cache_key,
            })
            .unwrap()
            .body
            .unwrap()
    }

    #[test]
    fn the_cache_key_goes_only_to_openai_for_sessions() {
        let official = "https://api.openai.com/v1";
        assert_eq!(
            body(official, json!({}), Some("sessao-1"))["prompt_cache_key"],
            "sessao-1"
        );
        assert!(body(official, json!({}), None)
            .get("prompt_cache_key")
            .is_none());
        assert!(body(official, json!({"promptCache": false}), Some("s"))
            .get("prompt_cache_key")
            .is_none());
        assert!(
            body("https://openrouter.ai/api/v1", json!({}), Some("s"))
                .get("prompt_cache_key")
                .is_none(),
            "compatible APIs may reject unknown fields"
        );
    }
}
