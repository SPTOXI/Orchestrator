//! Any HTTP/JSON chat API, described by a [`GenericProfile`] (ADR-0010).
//! Tools use the prompt protocol, so the API only needs to take text in and
//! give text out.

use crate::config::{
    Connection, GenericAuth, GenericProfile, MessageFormat, ModelEntry, StreamFormat,
};
use crate::conversation::{Message, Role};
use crate::http::{Frame, HttpCall};
use crate::jsonpath::{self, TemplateVars};
use crate::protocol::{
    query_sep, with_extra_body, with_headers, Decoder, Delta, Protocol, Reply, Request, Stop,
};
use orchestrator_providers::ProviderError;
use serde_json::{json, Value};

pub struct Generic;

fn profile(conn: &Connection) -> GenericProfile {
    conn.generic.clone().unwrap_or_default()
}

/// Applies the profile's authentication to a call.
fn authenticate(mut call: HttpCall, auth: &GenericAuth, key: Option<&str>) -> HttpCall {
    let Some(key) = key else {
        return call;
    };
    match auth {
        GenericAuth::Bearer => call
            .headers
            .push(("Authorization".into(), format!("Bearer {key}"))),
        GenericAuth::Header { name, prefix } => {
            call.headers.push((name.clone(), format!("{prefix}{key}")))
        }
        GenericAuth::Query { param } => {
            let sep = query_sep(&call.url);
            call.url = format!("{}{sep}{param}={key}", call.url);
        }
        GenericAuth::None => {}
    }
    call
}

/// Conversation flattened into one text (message format `prompt`).
pub fn flatten(system: Option<&str>, messages: &[Message]) -> String {
    let mut text = String::new();
    if let Some(system) = system {
        text.push_str("### System\n");
        text.push_str(system);
        text.push_str("\n\n");
    }
    for message in messages {
        text.push_str(match message.role {
            Role::User => "### User\n",
            Role::Assistant => "### Assistant\n",
        });
        text.push_str(&message.text());
        text.push_str("\n\n");
    }
    text.push_str("### Assistant\n");
    text
}

impl Protocol for Generic {
    fn request(&self, req: &Request<'_>) -> Result<HttpCall, ProviderError> {
        let profile = profile(req.conn);
        let mut messages = Vec::new();
        if let Some(system) = req.system {
            messages.push(json!({"role": profile.roles.system, "content": system}));
        }
        for message in req.messages {
            let role = match message.role {
                Role::User => &profile.roles.user,
                Role::Assistant => &profile.roles.assistant,
            };
            messages.push(json!({"role": role, "content": message.text()}));
        }
        let prompt = match profile.message_format {
            MessageFormat::Prompt => flatten(req.system, req.messages),
            MessageFormat::Chat => req
                .messages
                .iter()
                .rev()
                .find(|m| m.role == Role::User)
                .map(Message::text)
                .unwrap_or_default(),
        };
        let vars = TemplateVars {
            model: &req.model.id,
            system: req.system.unwrap_or(""),
            prompt: &prompt,
            messages: Value::Array(messages),
            stream: req.stream,
            max_tokens: req.model.max_output_tokens.or(req.conn.max_output_tokens),
        };
        let body = with_extra_body(jsonpath::render(&profile.body, &vars), req);
        let url = format!(
            "{}{}",
            req.conn.base(),
            jsonpath::interpolate(&profile.path, &vars)
        );
        let call = with_headers(HttpCall::post(url, body), Vec::new(), req.conn);
        Ok(authenticate(call, &profile.auth, req.key))
    }

    fn stream_format(&self, conn: &Connection, stream: bool) -> StreamFormat {
        if stream {
            profile(conn).stream
        } else {
            StreamFormat::None
        }
    }

    fn streams(&self, conn: &Connection) -> bool {
        profile(conn).stream != StreamFormat::None
    }

    fn decoder(&self, req: &Request<'_>) -> Box<dyn Decoder> {
        Box::new(GenericDecoder {
            profile: profile(req.conn),
            ..Default::default()
        })
    }

    fn models_request(&self, conn: &Connection, key: Option<&str>) -> Option<HttpCall> {
        let profile = profile(conn);
        let path = profile
            .models_path
            .as_deref()
            .filter(|p| !p.trim().is_empty())?;
        let call = with_headers(
            HttpCall::get(format!("{}{path}", conn.base())),
            Vec::new(),
            conn,
        );
        Some(authenticate(call, &profile.auth, key))
    }

    fn parse_models(&self, conn: &Connection, body: &Value) -> Vec<ModelEntry> {
        let profile = profile(conn);
        let list = profile
            .models_list_path
            .as_deref()
            .map_or(Some(body), |path| jsonpath::get(body, path))
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        list.iter()
            .filter_map(|item| {
                let id = match profile.model_id_field.as_deref().filter(|f| !f.is_empty()) {
                    Some(field) => jsonpath::get_text(item, field)?,
                    None => item.as_str()?.to_owned(),
                };
                Some(ModelEntry::new(id))
            })
            .collect()
    }
}

#[derive(Default)]
struct GenericDecoder {
    profile: GenericProfile,
    text: String,
    usage: orchestrator_core::TokenUsage,
    done: bool,
}

impl GenericDecoder {
    fn value(&mut self, value: &Value) -> Result<Vec<Delta>, ProviderError> {
        if let Some(path) = self.profile.error_path.as_deref() {
            if let Some(message) = jsonpath::get_text(value, path) {
                return Err(ProviderError::failed(format!("API error: {message}")));
            }
        }
        let mut deltas = Vec::new();
        if let Some(text) = jsonpath::get_text(value, &self.profile.text_path) {
            if !text.is_empty() {
                self.text.push_str(&text);
                deltas.push(Delta::Text(text));
            }
        }
        if let Some(path) = self.profile.input_tokens_path.as_deref() {
            if let Some(n) = jsonpath::get_u64(value, path) {
                self.usage.input_tokens = n;
            }
        }
        if let Some(path) = self.profile.output_tokens_path.as_deref() {
            if let Some(n) = jsonpath::get_u64(value, path) {
                self.usage.output_tokens = n;
            }
        }
        if let Some(path) = self.profile.done_path.as_deref() {
            if jsonpath::get(value, path).and_then(Value::as_bool) == Some(true) {
                self.done = true;
            }
        }
        Ok(deltas)
    }
}

impl Decoder for GenericDecoder {
    fn feed(&mut self, frame: &Frame) -> Result<Vec<Delta>, ProviderError> {
        let data = frame.data.trim();
        if data.is_empty() {
            return Ok(Vec::new());
        }
        if self.profile.done_marker.as_deref() == Some(data) {
            self.done = true;
            return Ok(Vec::new());
        }
        let value: Value = serde_json::from_str(data).map_err(|e| {
            ProviderError::failed(format!(
                "response is not JSON ({e}): {}",
                crate::http::truncate(data, 200)
            ))
        })?;
        self.value(&value)
    }

    fn done(&self) -> bool {
        self.done
    }

    fn finish(self: Box<Self>) -> Result<Reply, ProviderError> {
        Ok(Reply {
            text: self.text,
            usage: self.usage,
            stop: Stop::End,
            ..Default::default()
        })
    }
}
