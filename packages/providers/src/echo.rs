//! `echo`: development provider without AI or network (ADR-0009).
//!
//! Exercises the whole provider contract — streaming, tool calls through
//! the Orchestrator, cancellation, failure, resume, subagents and token
//! usage — in tests and in development builds of the app.
//!
//! Input:
//! - `/help` — lists the commands;
//! - `/tool <name> [json args]` — asks the Orchestrator to run a tool;
//! - `/wait <seconds>` — waits (cancellable);
//! - `/fail [message]` — fails the turn;
//! - `/context` — shows the project context the session received
//!   (ADR-0013);
//! - anything else is echoed back.

use crate::context::TurnContext;
use crate::error::ProviderError;
use crate::provider::{
    AIProvider, Completion, CompletionRequest, ModelInfo, NativeSession, ProviderCapabilities,
    ProviderDescriptor, ProviderStatus, SessionSpec, TurnInput, TurnOutput,
};
use async_trait::async_trait;
use chrono::Utc;
use orchestrator_core::{ProviderId, TokenUsage};
use parking_lot::Mutex;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

const MODEL: &str = "echo-1";
const REFERENCE_PREFIX: &str = "echo-";
/// Characters of a tool output quoted in the answer (the full result is in
/// the transcript's tool call).
const PREVIEW_CHARS: usize = 300;

const HELP: &str = concat!(
    "Echo é um provider de desenvolvimento: não usa IA nem rede.\n",
    "Comandos:\n",
    "• /tool <ferramenta> [args JSON] — pede ao Orchestrator para executar uma ferramenta, ",
    "ex.: /tool filesystem.list {\"path\": \".\"}\n",
    "• /wait <segundos> — espera (use Cancelar para interromper)\n",
    "• /fail [mensagem] — faz o turno falhar\n",
    "• /context — mostra o contexto do projeto que a sessão recebeu\n",
    "• /help — mostra esta ajuda\n",
    "Qualquer outro texto volta como eco.",
);

pub struct EchoProvider {
    id: ProviderId,
    name: String,
    chunk_delay: Duration,
    /// Project context received by each session (native reference).
    contexts: Mutex<HashMap<String, String>>,
}

impl EchoProvider {
    pub const ID: &'static str = "echo";

    pub fn new() -> Self {
        Self::with_identity(Self::ID, "Echo")
    }

    /// Same behavior under another id (tests with several providers).
    pub fn with_identity(id: &str, name: &str) -> Self {
        Self {
            id: ProviderId::from(id),
            name: name.to_owned(),
            chunk_delay: Duration::from_millis(25),
            contexts: Mutex::new(HashMap::new()),
        }
    }

    /// Delay between streamed chunks.
    pub fn with_chunk_delay(mut self, delay: Duration) -> Self {
        self.chunk_delay = delay;
        self
    }

    async fn run(
        &self,
        native: &NativeSession,
        input: &TurnInput,
        ctx: &TurnContext,
        streaming: bool,
    ) -> Result<TurnOutput, ProviderError> {
        if let Some(context) = &input.context {
            self.contexts
                .lock()
                .insert(native.reference.clone(), context.clone());
        }
        let command = Command::parse(&input.text)?;
        ctx.report_usage(TokenUsage {
            // A real model reads the context too.
            input_tokens: estimate_tokens(&input.text)
                + input.context.as_deref().map_or(0, estimate_tokens),
            estimated: true,
            ..Default::default()
        });
        let mut out = Output {
            ctx,
            streaming,
            delay: self.chunk_delay,
            text: String::new(),
        };
        match command {
            Command::Help => out.write(HELP).await?,
            Command::Echo(text) => out.write(&format!("Eco: {text}")).await?,
            Command::Fail(message) => return Err(ProviderError::failed(message)),
            Command::Context => {
                let context = self.contexts.lock().get(&native.reference).cloned();
                match context {
                    Some(context) => out.write_block(&context).await?,
                    None => {
                        out.write("Nenhum contexto do projeto foi anexado a esta sessão.")
                            .await?
                    }
                }
            }
            Command::Wait(duration) => {
                out.write(&format!("Aguardando {} s… ", duration.as_secs_f64()))
                    .await?;
                tokio::select! {
                    () = tokio::time::sleep(duration) => {}
                    () = ctx.cancelled() => return Err(ProviderError::cancelled("wait cancelled")),
                }
                out.write("pronto.").await?;
            }
            Command::Tool { name, args } => {
                out.write(&format!("Pedindo ao Orchestrator: `{name}`\n"))
                    .await?;
                let result = ctx.call_tool(&name, args).await;
                let answer = match (&result.error, result.ok) {
                    (None, true) => format!(
                        "✓ {name} ({} ms): {}",
                        result.duration_ms,
                        preview(&result.output)
                    ),
                    (Some(error), _) => {
                        format!("✗ {name}: {:?}: {}", error.kind, error.message)
                    }
                    (None, false) => format!("✗ {name}: falhou sem detalhes"),
                };
                // Tool output arrives in one piece, like a real provider
                // quoting a result.
                out.write_block(&answer).await?;
            }
        }
        ctx.report_usage(TokenUsage {
            output_tokens: estimate_tokens(&out.text),
            estimated: true,
            ..Default::default()
        });
        Ok(TurnOutput { text: out.text })
    }
}

impl Default for EchoProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl AIProvider for EchoProvider {
    fn descriptor(&self) -> ProviderDescriptor {
        ProviderDescriptor {
            id: self.id.clone(),
            name: self.name.clone(),
            vendor: "Orchestrator".into(),
            description: "Provider de desenvolvimento, sem IA e sem rede: devolve a mensagem \
                          e executa /tool, /wait e /fail para exercitar sessões."
                .into(),
        }
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            streaming: true,
            tool_calls: true,
            resume: true,
            cancel: true,
            native_subagents: false,
            reasoning: false,
            token_usage: true,
            cost: false,
            completion: true,
            models: vec![ModelInfo {
                id: MODEL.into(),
                name: "Echo".into(),
                input_price: Some(0.0),
                output_price: Some(0.0),
                tags: vec!["teste".into()],
                ..Default::default()
            }],
            default_model: Some(MODEL.into()),
        }
    }

    async fn inspect(&self) -> ProviderStatus {
        ProviderStatus {
            available: true,
            version: Some(env!("CARGO_PKG_VERSION").into()),
            authenticated: None,
            detail: Some("Embutido no Orchestrator; não usa IA nem rede.".into()),
            checked_at: Utc::now(),
        }
    }

    async fn start(&self, spec: &SessionSpec) -> Result<NativeSession, ProviderError> {
        if let Some(model) = spec.model.as_deref().filter(|m| *m != MODEL) {
            return Err(ProviderError::invalid(format!(
                "unknown model {model}; available: {MODEL}"
            )));
        }
        Ok(NativeSession {
            reference: format!("{REFERENCE_PREFIX}{}", uuid::Uuid::now_v7()),
            model: Some(MODEL.into()),
            data: json!({ "title": spec.title }),
        })
    }

    async fn resume(
        &self,
        native: &NativeSession,
        _spec: &SessionSpec,
    ) -> Result<NativeSession, ProviderError> {
        if native.reference.starts_with(REFERENCE_PREFIX) {
            if let Some(context) = native.data.get("context").and_then(Value::as_str) {
                self.contexts
                    .lock()
                    .insert(native.reference.clone(), context.to_owned());
            }
            Ok(native.clone())
        } else {
            Err(ProviderError::invalid(format!(
                "{} is not an echo session",
                native.reference
            )))
        }
    }

    async fn execute(
        &self,
        native: &NativeSession,
        input: &TurnInput,
        ctx: &TurnContext,
    ) -> Result<TurnOutput, ProviderError> {
        self.run(native, input, ctx, false).await
    }

    /// Keeps the received context with the session (for `/context` after a
    /// restart).
    async fn snapshot(&self, native: &NativeSession) -> NativeSession {
        let mut native = native.clone();
        if let Some(context) = self.contexts.lock().get(&native.reference) {
            if let Some(data) = native.data.as_object_mut() {
                data.insert("context".into(), Value::String(context.clone()));
            }
        }
        native
    }

    /// Echoes the prompt (no AI): exercises the Council plumbing.
    async fn complete(
        &self,
        request: &CompletionRequest,
        _cancel: &CancellationToken,
    ) -> Result<Completion, ProviderError> {
        if let Some(model) = request.model.as_deref().filter(|m| *m != MODEL) {
            return Err(ProviderError::invalid(format!(
                "unknown model {model}; available: {MODEL}"
            )));
        }
        let text = format!("Eco: {}", request.prompt.trim());
        Ok(Completion {
            usage: TokenUsage {
                input_tokens: estimate_tokens(&request.prompt)
                    + request.system.as_deref().map_or(0, estimate_tokens),
                output_tokens: estimate_tokens(&text),
                estimated: true,
                ..Default::default()
            },
            model: Some(MODEL.into()),
            text,
        })
    }

    async fn stream(
        &self,
        native: &NativeSession,
        input: &TurnInput,
        ctx: &TurnContext,
    ) -> Result<TurnOutput, ProviderError> {
        self.run(native, input, ctx, true).await
    }
}

#[derive(Debug, PartialEq)]
enum Command {
    Help,
    Echo(String),
    Tool { name: String, args: Value },
    Wait(Duration),
    Fail(String),
    Context,
}

impl Command {
    fn parse(input: &str) -> Result<Self, ProviderError> {
        let text = input.trim();
        let Some(rest) = text.strip_prefix('/') else {
            return Ok(Self::Echo(text.to_owned()));
        };
        let (word, arg) = match rest.split_once(char::is_whitespace) {
            Some((word, arg)) => (word, arg.trim()),
            None => (rest, ""),
        };
        match word {
            "help" => Ok(Self::Help),
            "context" => Ok(Self::Context),
            "fail" => Ok(Self::Fail(if arg.is_empty() {
                "falha simulada".into()
            } else {
                arg.to_owned()
            })),
            "wait" => {
                let seconds: f64 = if arg.is_empty() {
                    5.0
                } else {
                    arg.parse().map_err(|_| {
                        ProviderError::invalid(format!("/wait: invalid seconds {arg:?}"))
                    })?
                };
                if !seconds.is_finite() || seconds < 0.0 {
                    return Err(ProviderError::invalid(format!(
                        "/wait: invalid seconds {arg:?}"
                    )));
                }
                Ok(Self::Wait(Duration::from_secs_f64(seconds)))
            }
            "tool" => {
                let (name, json_args) = match arg.split_once(char::is_whitespace) {
                    Some((name, json_args)) => (name, json_args.trim()),
                    None => (arg, ""),
                };
                if name.is_empty() {
                    return Err(ProviderError::invalid(
                        "/tool: missing tool name (e.g. /tool filesystem.list {\"path\": \".\"})",
                    ));
                }
                let args = if json_args.is_empty() {
                    json!({})
                } else {
                    serde_json::from_str(json_args).map_err(|e| {
                        ProviderError::invalid(format!("/tool: arguments are not valid JSON: {e}"))
                    })?
                };
                Ok(Self::Tool {
                    name: name.to_owned(),
                    args,
                })
            }
            other => Err(ProviderError::invalid(format!(
                "unknown command /{other} (use /help)"
            ))),
        }
    }
}

/// Collects the answer and, when streaming, emits it word by word (or a
/// block at once).
struct Output<'a> {
    ctx: &'a TurnContext,
    streaming: bool,
    delay: Duration,
    text: String,
}

impl Output<'_> {
    async fn write_block(&mut self, text: &str) -> Result<(), ProviderError> {
        if self.ctx.is_cancelled() {
            return Err(ProviderError::cancelled("output cancelled"));
        }
        self.text.push_str(text);
        if self.streaming {
            self.ctx.emit_text(text);
        }
        Ok(())
    }

    async fn write(&mut self, text: &str) -> Result<(), ProviderError> {
        if !self.streaming {
            self.text.push_str(text);
            return Ok(());
        }
        for chunk in text.split_inclusive(' ') {
            if self.ctx.is_cancelled() {
                return Err(ProviderError::cancelled("output cancelled"));
            }
            self.text.push_str(chunk);
            self.ctx.emit_text(chunk);
            if !self.delay.is_zero() {
                tokio::select! {
                    () = tokio::time::sleep(self.delay) => {}
                    () = self.ctx.cancelled() => return Err(ProviderError::cancelled("output cancelled")),
                }
            }
        }
        Ok(())
    }
}

/// Rough token estimate (≈ 4 characters per token).
fn estimate_tokens(text: &str) -> u64 {
    (text.chars().count() as u64).div_ceil(4)
}

fn preview(value: &Value) -> String {
    let text = value.to_string();
    if text.chars().count() <= PREVIEW_CHARS {
        return text;
    }
    let cut: String = text.chars().take(PREVIEW_CHARS).collect();
    format!("{cut}…")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_commands() {
        assert_eq!(Command::parse(" oi ").unwrap(), Command::Echo("oi".into()));
        assert_eq!(Command::parse("/help").unwrap(), Command::Help);
        assert_eq!(Command::parse("/context").unwrap(), Command::Context);
        assert_eq!(
            Command::parse("/wait 1.5").unwrap(),
            Command::Wait(Duration::from_millis(1500))
        );
        assert_eq!(
            Command::parse("/fail boom").unwrap(),
            Command::Fail("boom".into())
        );
        assert_eq!(
            Command::parse("/tool filesystem.list {\"path\": \".\"}").unwrap(),
            Command::Tool {
                name: "filesystem.list".into(),
                args: json!({"path": "."})
            }
        );
        assert_eq!(
            Command::parse("/tool process.list").unwrap(),
            Command::Tool {
                name: "process.list".into(),
                args: json!({})
            }
        );
    }

    #[test]
    fn rejects_bad_commands() {
        for input in ["/nope", "/tool", "/tool x {bad", "/wait abc", "/wait -1"] {
            assert!(Command::parse(input).is_err(), "{input} should fail");
        }
    }

    #[test]
    fn estimates_tokens() {
        assert_eq!(estimate_tokens(""), 0);
        assert_eq!(estimate_tokens("abcd"), 1);
        assert_eq!(estimate_tokens("abcde"), 2);
    }

    #[tokio::test]
    async fn completes_without_a_session() {
        let echo = EchoProvider::new();
        assert!(echo.capabilities().completion);
        let cancel = CancellationToken::new();
        let answer = echo
            .complete(
                &CompletionRequest {
                    model: None,
                    system: Some("instruções".into()),
                    prompt: " qual modelo? ".into(),
                },
                &cancel,
            )
            .await
            .unwrap();
        assert_eq!(answer.text, "Eco: qual modelo?");
        assert_eq!(answer.model.as_deref(), Some(MODEL));
        assert!(answer.usage.input_tokens > 0 && answer.usage.estimated);

        let wrong = CompletionRequest {
            model: Some("gpt".into()),
            prompt: "x".into(),
            ..Default::default()
        };
        assert!(echo.complete(&wrong, &cancel).await.is_err());
    }
}
