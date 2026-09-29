//! `ApiProvider`: one registered API connection as an [`AIProvider`].
//!
//! A turn is a loop: call the model → run the tools it asked for through
//! the Orchestrator (`TurnContext::call_tool`) → send the results back →
//! until the model answers without tools (or `maxToolRounds`, ADR-0010).

use crate::config::{ApiKind, Connection, CredentialSource, ModelEntry, ToolMode};
use crate::conversation::{Conversation, Message, Part, Role, ToolResultPart};
use crate::http::{FrameReader, HttpClient};
use crate::protocol::{protocol, Delta, Reply, Request, Stop};
use crate::secrets::SecretStore;
use crate::tools::{
    parse_prompt_calls, ping_tool, prompt_instructions, prompt_results, rejected, result_content,
    MarkupFilter,
};
use async_trait::async_trait;
use chrono::Utc;
use orchestrator_core::{NoticeLevel, ProviderId, TokenUsage, ToolDefinition};
use orchestrator_providers::{
    AIProvider, Completion, CompletionRequest, NativeSession, ProviderCapabilities,
    ProviderDescriptor, ProviderError, ProviderErrorKind, ProviderStatus, SessionSpec, TurnContext,
    TurnInput, TurnOutput,
};
use parking_lot::Mutex;
use serde::Serialize;
use serde_json::json;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;

const INSPECT_TIMEOUT: Duration = Duration::from_secs(15);
const TEST_TIMEOUT: Duration = Duration::from_secs(90);

/// Conversations by native reference. Every instance of one connection gets
/// the same store from the `ConnectionManager`, so editing the connection
/// (a new instance in the registry) keeps the history of open sessions.
pub(crate) type ConversationStore =
    Arc<Mutex<HashMap<String, Arc<tokio::sync::Mutex<Conversation>>>>>;

pub struct ApiProvider {
    conn: Connection,
    secrets: Arc<dyn SecretStore>,
    client: HttpClient,
    /// Key typed in the UI for an unsaved test (never stored).
    key_override: Option<String>,
    key_cache: Mutex<Option<Option<String>>>,
    conversations: ConversationStore,
}

/// Result of "Testar conexão".
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TestReport {
    pub ok: bool,
    pub model: Option<String>,
    pub served_model: Option<String>,
    pub latency_ms: u64,
    pub reply: Option<String>,
    pub usage: Option<TokenUsage>,
    /// `notTested`, `passed`, `noCall` or `failed`.
    pub tools: String,
    pub tools_detail: Option<String>,
    pub error: Option<String>,
}

impl ApiProvider {
    pub fn new(conn: Connection, secrets: Arc<dyn SecretStore>, client: HttpClient) -> Self {
        Self {
            conn,
            secrets,
            client,
            key_override: None,
            key_cache: Mutex::new(None),
            conversations: ConversationStore::default(),
        }
    }

    pub(crate) fn with_conversations(mut self, store: ConversationStore) -> Self {
        self.conversations = store;
        self
    }

    /// A provider that uses `key` instead of the stored credential.
    pub fn with_key(mut self, key: Option<String>) -> Self {
        self.key_override = key.filter(|k| !k.trim().is_empty());
        self
    }

    pub fn connection(&self) -> &Connection {
        &self.conn
    }

    /// The API key, from the override, the OS vault or the environment.
    pub async fn key(&self) -> Result<Option<String>, ProviderError> {
        if let Some(key) = &self.key_override {
            return Ok(Some(key.clone()));
        }
        if let Some(cached) = self.key_cache.lock().clone() {
            return Ok(cached);
        }
        let key = match self.conn.credential.source {
            CredentialSource::None => None,
            CredentialSource::Env => {
                let var = self.conn.credential.env_var.clone().unwrap_or_default();
                let value = std::env::var(&var)
                    .ok()
                    .filter(|v| !v.trim().is_empty())
                    .ok_or_else(|| {
                        ProviderError::unavailable(format!("environment variable {var} is not set"))
                    })?;
                Some(value)
            }
            CredentialSource::Vault => {
                let secrets = self.secrets.clone();
                let id = self.conn.id.clone();
                let stored = tokio::task::spawn_blocking(move || secrets.get(&id))
                    .await
                    .map_err(|e| ProviderError::internal(format!("vault task failed: {e}")))?
                    .map_err(|e| {
                        ProviderError::unavailable(format!("cannot read the OS vault: {e}"))
                    })?;
                Some(stored.ok_or_else(|| {
                    ProviderError::unavailable(
                        "no API key stored for this connection (edit it and enter the key)",
                    )
                })?)
            }
        };
        *self.key_cache.lock() = Some(key.clone());
        Ok(key)
    }

    fn model_entry(&self, id: &str) -> ModelEntry {
        self.conn
            .model(id)
            .cloned()
            .unwrap_or_else(|| ModelEntry::new(id))
    }

    fn conversation(&self, reference: &str) -> Arc<tokio::sync::Mutex<Conversation>> {
        self.conversations
            .lock()
            .entry(reference.to_owned())
            .or_default()
            .clone()
    }

    /// One request to the model; streamed deltas go to `on_delta`.
    pub(crate) async fn call_model(
        &self,
        model: &ModelEntry,
        system: Option<&str>,
        messages: &[Message],
        tools: &[ToolDefinition],
        on_delta: &(dyn Fn(Delta) + Send + Sync),
        cancel: &CancellationToken,
    ) -> Result<Reply, ProviderError> {
        let key = self.key().await?;
        let protocol = protocol(self.conn.kind);
        let stream = protocol.streams(&self.conn);
        let req = Request {
            conn: &self.conn,
            key: key.as_deref(),
            model,
            system,
            messages,
            tools,
            stream,
        };
        let call = protocol.request(&req)?;
        let response = self.client.send(&call, cancel).await?;
        let mut decoder = protocol.decoder(&req);
        let mut reader = FrameReader::new(response, protocol.stream_format(&self.conn, stream));
        while let Some(frame) = reader.next(cancel).await? {
            for delta in decoder.feed(&frame)? {
                on_delta(delta);
            }
            if decoder.done() {
                break;
            }
        }
        decoder.finish()
    }

    /// Usage plus cost from the model's configured prices.
    fn priced(&self, model: &ModelEntry, mut usage: TokenUsage) -> TokenUsage {
        let input = model
            .input_price
            .map(|p| usage.input_tokens as f64 * p / 1_000_000.0);
        let output = model
            .output_price
            .map(|p| usage.output_tokens as f64 * p / 1_000_000.0);
        if input.is_some() || output.is_some() {
            usage.cost_usd = Some(input.unwrap_or(0.0) + output.unwrap_or(0.0));
        }
        usage
    }

    fn system_prompt(
        &self,
        base: Option<&str>,
        mode: ToolMode,
        tools: &[ToolDefinition],
    ) -> Option<String> {
        let mut system = base.unwrap_or("").to_owned();
        if mode == ToolMode::Prompt && !tools.is_empty() {
            if !system.is_empty() {
                system.push_str("\n\n");
            }
            system.push_str(&prompt_instructions(tools));
        }
        (!system.is_empty()).then_some(system)
    }

    async fn run(
        &self,
        native: &NativeSession,
        input: &TurnInput,
        ctx: &TurnContext,
        streaming: bool,
    ) -> Result<TurnOutput, ProviderError> {
        let conversation = self.conversation(&native.reference);
        let mut conv = conversation.lock().await;
        let model_id = native
            .model
            .clone()
            .or_else(|| self.conn.default_model_id())
            .ok_or_else(|| ProviderError::invalid("this connection has no model"))?;
        let model = self.model_entry(&model_id);
        let mode = self.conn.tool_mode();
        let definitions = if mode == ToolMode::None {
            Vec::new()
        } else {
            ctx.tools()
        };
        let native_tools: &[ToolDefinition] = if mode == ToolMode::Native {
            &definitions
        } else {
            &[]
        };
        let system = self.system_prompt(conv.system.as_deref(), mode, &definitions);
        conv.messages.push(Message::user(input.text.clone()));
        let cancel = ctx.cancellation();
        let mut output = String::new();

        for _round in 0..self.conn.max_tool_rounds {
            let filter = Mutex::new(MarkupFilter::default());
            let on_delta = |delta: Delta| {
                if !streaming {
                    return;
                }
                match delta {
                    Delta::Text(text) => {
                        let visible = if mode == ToolMode::Prompt {
                            filter.lock().push(&text)
                        } else {
                            text
                        };
                        ctx.emit_text(&visible);
                    }
                    Delta::Reasoning(text) => ctx.emit_reasoning(&text),
                }
            };
            let request_messages = if mode == ToolMode::Prompt {
                as_prompt_messages(&conv.messages)
            } else {
                conv.messages.clone()
            };
            let reply = self
                .call_model(
                    &model,
                    system.as_deref(),
                    &request_messages,
                    native_tools,
                    &on_delta,
                    &cancel,
                )
                .await?;
            if streaming && mode == ToolMode::Prompt {
                ctx.emit_text(&filter.lock().finish());
            }
            ctx.report_usage(self.priced(&model, reply.usage));
            for notice in &reply.notices {
                ctx.notice(NoticeLevel::Info, notice.clone());
            }
            if let Stop::Refusal(reason) = &reply.stop {
                return Err(ProviderError::failed(format!(
                    "the model declined the request ({reason})"
                )));
            }

            let (calls, visible) = match mode {
                ToolMode::Prompt => parse_prompt_calls(&reply.text, &mut conv.next_call),
                ToolMode::Native => (reply.tool_calls.clone(), reply.text.clone()),
                ToolMode::None => (Vec::new(), reply.text.clone()),
            };
            let mut parts = Vec::new();
            if !reply.text.is_empty() {
                parts.push(Part::Text(reply.text.clone()));
            }
            parts.extend(calls.iter().cloned().map(Part::ToolCall));
            conv.messages.push(Message {
                role: Role::Assistant,
                parts,
                native: reply.native.clone(),
            });
            if !visible.is_empty() {
                if !output.is_empty() {
                    output.push_str("\n\n");
                }
                output.push_str(&visible);
            }

            if calls.is_empty() {
                if reply.stop == Stop::MaxTokens {
                    ctx.notice(
                        NoticeLevel::Warning,
                        "resposta cortada no limite de tokens de saída (maxOutputTokens da conexão ou do modelo)",
                    );
                }
                return Ok(TurnOutput { text: output });
            }

            let mut results = Vec::new();
            for call in &calls {
                let result = if reply.stop == Stop::MaxTokens {
                    rejected(
                        call,
                        "the reply hit the output token limit before the arguments were complete; retry with a smaller input",
                    )
                } else if let Some(reason) = &call.invalid {
                    rejected(call, reason)
                } else if call.name.is_empty() {
                    rejected(call, "missing tool name")
                } else {
                    let result = ctx.call_tool(&call.name, call.args.clone()).await;
                    let (content, is_error) = result_content(&result);
                    ToolResultPart {
                        id: call.id.clone(),
                        name: call.name.clone(),
                        content,
                        is_error,
                        native_id: call.native_id,
                    }
                };
                results.push(result);
            }
            conv.messages.push(Message {
                role: Role::User,
                parts: results.into_iter().map(Part::ToolResult).collect(),
                native: None,
            });
            if ctx.is_cancelled() {
                return Err(ProviderError::cancelled("turn cancelled"));
            }
        }
        ctx.notice(
            NoticeLevel::Warning,
            format!(
                "limite de {} rodadas de ferramentas atingido neste turno (maxToolRounds, configurável na conexão)",
                self.conn.max_tool_rounds
            ),
        );
        Ok(TurnOutput { text: output })
    }

    /// Models available to this account (not saved).
    pub async fn list_models(&self) -> Result<Vec<ModelEntry>, ProviderError> {
        let key = self.key().await?;
        let protocol = protocol(self.conn.kind);
        let mut call = protocol
            .models_request(&self.conn, key.as_deref())
            .ok_or_else(|| {
                ProviderError::unsupported(
                    "this API has no model list endpoint configured; add the models by hand",
                )
            })?;
        let cancel = CancellationToken::new();
        let mut models = Vec::new();
        for _page in 0..20 {
            let body = self.client.json(&call, &cancel).await?;
            models.extend(protocol.parse_models(&self.conn, &body));
            match protocol.next_models_page(&self.conn, key.as_deref(), &body) {
                Some(next) => call = next,
                None => break,
            }
        }
        if self.conn.kind == ApiKind::Anthropic {
            for model in &mut models {
                if let Some((input, output, context, tags)) = crate::anthropic::reference(&model.id)
                {
                    model.input_price.get_or_insert(input);
                    model.output_price.get_or_insert(output);
                    model.context_window.get_or_insert(context);
                    if model.tags.is_empty() {
                        model.tags = tags.iter().map(|t| (*t).to_owned()).collect();
                    }
                }
            }
        }
        models.sort_by(|a, b| a.id.cmp(&b.id));
        models.dedup_by(|a, b| a.id == b.id);
        Ok(models)
    }

    /// "Testar conexão": a short reply and, if tools are on, one tool call
    /// to a test tool that never touches the system.
    pub async fn test(&self, model: Option<String>) -> TestReport {
        let started = Instant::now();
        let mut report = TestReport {
            tools: "notTested".into(),
            ..Default::default()
        };
        let outcome = tokio::time::timeout(TEST_TIMEOUT, self.test_steps(model, &mut report)).await;
        report.latency_ms = started.elapsed().as_millis() as u64;
        match outcome {
            Ok(Ok(())) => report.ok = true,
            Ok(Err(err)) => report.error = Some(err.message),
            Err(_) => report.error = Some(format!("no answer within {} s", TEST_TIMEOUT.as_secs())),
        }
        report
    }

    async fn test_steps(
        &self,
        model: Option<String>,
        report: &mut TestReport,
    ) -> Result<(), ProviderError> {
        let model_id = model
            .filter(|m| !m.trim().is_empty())
            .or_else(|| self.conn.default_model_id())
            .ok_or_else(|| {
                ProviderError::invalid("add at least one model (or use \"Buscar modelos\")")
            })?;
        report.model = Some(model_id.clone());
        let model = self.model_entry(&model_id);
        let cancel = CancellationToken::new();
        let quiet = |_: Delta| {};

        let reply = self
            .call_model(
                &model,
                Some("This is a connection test from the Orchestrator."),
                &[Message::user("Reply with exactly: OK")],
                &[],
                &quiet,
                &cancel,
            )
            .await?;
        report.reply = Some(crate::http::truncate(reply.text.trim(), 200));
        report.served_model = reply.served_model.clone();
        report.usage = Some(self.priced(&model, reply.usage));
        if let Stop::Refusal(reason) = reply.stop {
            return Err(ProviderError::failed(format!(
                "the model declined ({reason})"
            )));
        }

        let mode = self.conn.tool_mode();
        if mode == ToolMode::None {
            return Ok(());
        }
        let ping = [ping_tool()];
        let (system, tools): (Option<String>, &[ToolDefinition]) = match mode {
            ToolMode::Prompt => (self.system_prompt(None, mode, &ping), &[]),
            _ => (None, &ping),
        };
        let asked = Message::user(
            "Call the tool orchestrator.ping with value \"orchestrator\". Do not answer with text.",
        );
        let result = self
            .call_model(&model, system.as_deref(), &[asked], tools, &quiet, &cancel)
            .await;
        match result {
            Ok(reply) => {
                let calls = match mode {
                    ToolMode::Prompt => parse_prompt_calls(&reply.text, &mut 0).0,
                    _ => reply.tool_calls,
                };
                match calls.iter().find(|c| c.name == "orchestrator.ping") {
                    Some(call) if call.invalid.is_none() => {
                        report.tools = "passed".into();
                        report.tools_detail = Some(format!("args: {}", call.args));
                    }
                    Some(call) => {
                        report.tools = "failed".into();
                        report.tools_detail = call.invalid.clone();
                    }
                    None => {
                        report.tools = "noCall".into();
                        report.tools_detail = Some(
                            "o modelo não chamou a ferramenta de teste (pode não suportar chamadas de ferramenta neste modo)"
                                .into(),
                        );
                    }
                }
            }
            Err(err) => {
                report.tools = "failed".into();
                report.tools_detail = Some(err.message);
            }
        }
        Ok(())
    }

    fn default_system(spec: &SessionSpec) -> String {
        let mut system = format!(
            "You are an AI agent working through the Orchestrator on the user's project at {} ({} {}). \
             Every tool you request is executed by the Orchestrator and recorded in the project history. \
             Answer the user in the language they write in.",
            spec.project_path.display(),
            std::env::consts::OS,
            std::env::consts::ARCH,
        );
        if let Some(instructions) = spec
            .instructions
            .as_deref()
            .filter(|i| !i.trim().is_empty())
        {
            system.push_str("\n\n");
            system.push_str(instructions);
        }
        system
    }
}

/// Text-only view of the conversation for the prompt tool protocol.
fn as_prompt_messages(messages: &[Message]) -> Vec<Message> {
    messages
        .iter()
        .map(|message| {
            let results: Vec<ToolResultPart> = message.tool_results().cloned().collect();
            let mut text = message.text();
            if !results.is_empty() {
                if !text.is_empty() {
                    text.push('\n');
                }
                text.push_str(&prompt_results(&results));
            }
            Message {
                role: message.role,
                parts: vec![Part::Text(text)],
                native: None,
            }
        })
        .collect()
}

#[async_trait]
impl AIProvider for ApiProvider {
    fn descriptor(&self) -> ProviderDescriptor {
        ProviderDescriptor {
            id: ProviderId::from(self.conn.id.as_str()),
            name: self.conn.name.clone(),
            vendor: self.conn.kind.label().into(),
            description: format!("{} · {}", self.conn.kind.label(), self.conn.base()),
        }
    }

    fn capabilities(&self) -> ProviderCapabilities {
        let protocol = protocol(self.conn.kind);
        let generic_usage = self
            .conn
            .generic
            .as_ref()
            .is_some_and(|g| g.input_tokens_path.is_some() || g.output_tokens_path.is_some());
        ProviderCapabilities {
            streaming: protocol.streams(&self.conn),
            tool_calls: self.conn.tool_mode() != ToolMode::None,
            resume: true,
            cancel: true,
            native_subagents: false,
            reasoning: self.conn.kind != ApiKind::Generic,
            token_usage: self.conn.kind != ApiKind::Generic || generic_usage,
            cost: self
                .conn
                .enabled_models()
                .any(|m| m.input_price.is_some() || m.output_price.is_some()),
            completion: true,
            models: self.conn.enabled_models().map(ModelEntry::info).collect(),
            default_model: self.conn.default_model_id(),
        }
    }

    async fn inspect(&self) -> ProviderStatus {
        let status =
            |available: bool, authenticated: Option<bool>, detail: String| ProviderStatus {
                available,
                version: None,
                authenticated,
                detail: Some(detail),
                checked_at: Utc::now(),
            };
        let key = match self.key().await {
            Ok(key) => key,
            Err(err) => return status(false, Some(false), err.message),
        };
        let has_endpoint = protocol(self.conn.kind)
            .models_request(&self.conn, key.as_deref())
            .is_some();
        if !has_endpoint {
            return status(
                true,
                None,
                "credencial configurada; use Testar conexão para validar".into(),
            );
        }
        match tokio::time::timeout(INSPECT_TIMEOUT, self.list_models()).await {
            Ok(Ok(models)) => status(
                true,
                key.as_ref().map(|_| true),
                format!("{} modelos disponíveis na conta", models.len()),
            ),
            Ok(Err(err)) => {
                let auth = (err.kind == ProviderErrorKind::Unavailable
                    && err.message.contains("authentication"))
                .then_some(false);
                status(false, auth, err.message)
            }
            Err(_) => status(
                false,
                None,
                format!("sem resposta em {} s", INSPECT_TIMEOUT.as_secs()),
            ),
        }
    }

    async fn start(&self, spec: &SessionSpec) -> Result<NativeSession, ProviderError> {
        let model = spec
            .model
            .clone()
            .or_else(|| self.conn.default_model_id())
            .ok_or_else(|| {
                ProviderError::invalid(
                    "this connection has no enabled model; add one in its settings",
                )
            })?;
        let reference = format!("{}-{}", self.conn.id, uuid::Uuid::now_v7());
        let conversation = Conversation {
            system: Some(Self::default_system(spec)),
            ..Default::default()
        };
        self.conversations.lock().insert(
            reference.clone(),
            Arc::new(tokio::sync::Mutex::new(conversation)),
        );
        Ok(NativeSession {
            reference,
            model: Some(model),
            data: json!({"connection": self.conn.id}),
        })
    }

    async fn resume(
        &self,
        native: &NativeSession,
        spec: &SessionSpec,
    ) -> Result<NativeSession, ProviderError> {
        let mut conversations = self.conversations.lock();
        conversations
            .entry(native.reference.clone())
            .or_insert_with(|| {
                // After a restart the conversation comes from the snapshot
                // stored with the session (ADR-0012); without one the
                // session starts over with the same system prompt.
                let stored = native
                    .data
                    .get("conversation")
                    .and_then(|value| serde_json::from_value::<Conversation>(value.clone()).ok());
                Arc::new(tokio::sync::Mutex::new(stored.unwrap_or_else(|| {
                    Conversation {
                        system: Some(Self::default_system(spec)),
                        ..Default::default()
                    }
                })))
            });
        Ok(native.clone())
    }

    /// The session with its whole conversation, native parts included, so
    /// it can be resumed after a restart (ADR-0012).
    async fn snapshot(&self, native: &NativeSession) -> NativeSession {
        let mut native = native.clone();
        let conversation = self.conversations.lock().get(&native.reference).cloned();
        if let Some(conversation) = conversation {
            let value = serde_json::to_value(&*conversation.lock().await);
            if let Ok(value) = value {
                if !native.data.is_object() {
                    native.data = json!({});
                }
                native.data["conversation"] = value;
            }
        }
        native
    }

    async fn execute(
        &self,
        native: &NativeSession,
        input: &TurnInput,
        ctx: &TurnContext,
    ) -> Result<TurnOutput, ProviderError> {
        self.run(native, input, ctx, false).await
    }

    async fn stream(
        &self,
        native: &NativeSession,
        input: &TurnInput,
        ctx: &TurnContext,
    ) -> Result<TurnOutput, ProviderError> {
        self.run(native, input, ctx, true).await
    }

    /// One request with no tools and no stored conversation (ADR-0011).
    async fn complete(
        &self,
        request: &CompletionRequest,
        cancel: &CancellationToken,
    ) -> Result<Completion, ProviderError> {
        let model_id = request
            .model
            .clone()
            .filter(|m| !m.trim().is_empty())
            .or_else(|| self.conn.default_model_id())
            .ok_or_else(|| ProviderError::invalid("this connection has no model"))?;
        let model = self.model_entry(&model_id);
        let quiet = |_: Delta| {};
        let reply = self
            .call_model(
                &model,
                request.system.as_deref(),
                &[Message::user(request.prompt.clone())],
                &[],
                &quiet,
                cancel,
            )
            .await?;
        if let Stop::Refusal(reason) = &reply.stop {
            return Err(ProviderError::failed(format!(
                "the model declined the request ({reason})"
            )));
        }
        Ok(Completion {
            text: reply.text,
            model: reply.served_model.or(Some(model_id)),
            usage: self.priced(&model, reply.usage),
        })
    }
}
