//! A CLI as an `AIProvider` (ADR-0021): each turn runs the CLI once, with
//! the prompt on stdin and the Orchestrator's tools at a local MCP URL; its
//! output becomes the turn's text, reasoning and usage.

use crate::kind::{invocation, parse, CliKind, Event, RunPlan};
use crate::settings::CliSettings;
use async_trait::async_trait;
use chrono::Utc;
use orchestrator_core::{NoticeLevel, ProviderId, TokenUsage};
use orchestrator_mcp::ToolServer;
use orchestrator_providers::{
    AIProvider, Completion, CompletionRequest, ModelInfo, NativeSession, ProviderCapabilities,
    ProviderDescriptor, ProviderError, ProviderStatus, SessionSpec, TurnContext, TurnInput,
    TurnOutput,
};
use parking_lot::Mutex;
use serde_json::{json, Value};
use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio_util::sync::CancellationToken;

/// Lines of stderr kept for the error message.
const STDERR_LINES: usize = 30;

/// What the provider remembers of a session between turns (and, through
/// `snapshot`, across restarts).
#[derive(Debug, Clone, Default)]
struct State {
    /// The CLI has the session: resume it.
    started: bool,
    /// Codex's thread.
    thread: Option<String>,
    /// System instructions (the project context), sent on every run.
    system: Option<String>,
}

pub struct CliProvider {
    kind: CliKind,
    settings: CliSettings,
    server: ToolServer,
    scratch: PathBuf,
    states: Mutex<HashMap<String, State>>,
}

impl CliProvider {
    pub fn new(kind: CliKind, settings: CliSettings, server: ToolServer, scratch: PathBuf) -> Self {
        Self {
            kind,
            settings,
            server,
            scratch,
            states: Mutex::new(HashMap::new()),
        }
    }

    fn models(&self) -> Vec<String> {
        if self.settings.models.is_empty() {
            self.kind.default_models()
        } else {
            self.settings.models.clone()
        }
    }

    fn program(&self) -> Option<PathBuf> {
        crate::find_program(self.kind, self.settings.program.as_deref())
    }

    fn state(&self, native: &NativeSession) -> State {
        if let Some(state) = self.states.lock().get(&native.reference) {
            return state.clone();
        }
        let data = &native.data;
        State {
            started: data
                .get("started")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            thread: data
                .get("thread")
                .and_then(Value::as_str)
                .map(str::to_owned),
            system: data
                .get("system")
                .and_then(Value::as_str)
                .map(str::to_owned),
        }
    }

    /// Runs the CLI once. `on` hears every event; returns the final text.
    async fn run(
        &self,
        plan: RunPlan<'_>,
        prompt: &str,
        cancel: &CancellationToken,
        on: &mut (dyn FnMut(Event) + Send),
    ) -> Result<String, ProviderError> {
        let program = self.program().ok_or_else(|| {
            ProviderError::unavailable(format!(
                "{} não está instalado (instale em Configurações → Assinaturas)",
                self.kind.program()
            ))
        })?;
        let kind = plan.kind;
        let call = invocation(&plan)
            .map_err(|e| ProviderError::internal(format!("cannot prepare the CLI run: {e}")))?;
        let mut command = crate::command_for(&program, &call.args);
        command
            .current_dir(plan.project)
            // A Claude Code started from inside another one (the
            // Orchestrator opened from its terminal) must not think it is
            // nested.
            .env_remove("CLAUDECODE")
            .env_remove("CLAUDE_CODE_ENTRYPOINT")
            .envs(call.env)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let mut child = command.spawn().map_err(|e| {
            ProviderError::unavailable(format!(
                "não foi possível iniciar {}: {e}",
                program.display()
            ))
        })?;
        if let Some(mut stdin) = child.stdin.take() {
            let prompt = prompt.to_owned();
            tokio::spawn(async move {
                let _ = stdin.write_all(prompt.as_bytes()).await;
                let _ = stdin.shutdown().await;
            });
        }
        let stdout = child.stdout.take().expect("piped stdout");
        let stderr = child.stderr.take().expect("piped stderr");
        let errors: Arc<Mutex<VecDeque<String>>> = Arc::default();
        let log = errors.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let mut log = log.lock();
                if log.len() >= STDERR_LINES {
                    log.pop_front();
                }
                log.push_back(line);
            }
        });

        let mut lines = BufReader::new(stdout).lines();
        let mut final_text: Option<String> = None;
        let mut streamed = String::new();
        let mut messages: Vec<String> = Vec::new();
        let mut failure: Option<String> = None;
        loop {
            let line = tokio::select! {
                line = lines.next_line() => line,
                () = cancel.cancelled() => {
                    let _ = child.start_kill();
                    return Err(ProviderError::cancelled("turn cancelled"));
                }
            };
            let Ok(Some(line)) = line else {
                break;
            };
            for event in parse(kind, &line) {
                match &event {
                    Event::Text(t) => streamed.push_str(t),
                    Event::Message(t) => messages.push(t.clone()),
                    Event::Final(t) if !t.is_empty() => final_text = Some(t.clone()),
                    Event::Error(e) => failure = Some(e.clone()),
                    _ => {}
                }
                on(event);
            }
        }
        let status = tokio::select! {
            status = child.wait() => status.ok(),
            () = tokio::time::sleep(Duration::from_secs(10)) => {
                let _ = child.start_kill();
                None
            }
        };
        let success = status.is_some_and(|s| s.success());
        if let Some(failure) = failure {
            return Err(ProviderError::failed(self.explain(&failure, &errors)));
        }
        let text = final_text
            .or_else(|| (!streamed.is_empty()).then(|| streamed.clone()))
            .unwrap_or_else(|| messages.join("\n\n"));
        if !success && text.trim().is_empty() {
            let tail: Vec<String> = errors.lock().iter().cloned().collect();
            let detail = tail.join("\n");
            return Err(ProviderError::failed(self.explain(
                if detail.trim().is_empty() {
                    "a CLI terminou sem responder"
                } else {
                    &detail
                },
                &errors,
            )));
        }
        Ok(text)
    }

    /// An error with what to do about the usual causes.
    fn explain(&self, error: &str, stderr: &Mutex<VecDeque<String>>) -> String {
        let all = format!(
            "{error}\n{}",
            stderr.lock().iter().cloned().collect::<Vec<_>>().join("\n")
        );
        let lower = all.to_lowercase();
        let hint = if lower.contains("login")
            || lower.contains("log in")
            || lower.contains("not logged")
            || lower.contains("authenticat")
            || lower.contains("auth method")
            || lower.contains("api key")
            || lower.contains("unauthorized")
            || lower.contains("401")
        {
            format!(
                " — entre na conta: Configurações → Assinaturas (CLI) → Entrar ({})",
                self.kind.login_command()
            )
        } else if lower.contains("rate limit")
            || lower.contains("usage limit")
            || lower.contains("quota")
            || lower.contains("429")
        {
            " — o limite de uso da assinatura acabou por agora; tente mais tarde ou use outra IA"
                .into()
        } else {
            String::new()
        };
        let mut text: String = error.trim().chars().take(1500).collect();
        text.push_str(&hint);
        text
    }
}

#[async_trait]
impl AIProvider for CliProvider {
    fn descriptor(&self) -> ProviderDescriptor {
        ProviderDescriptor {
            id: ProviderId::from(self.kind.id()),
            name: self.kind.name().into(),
            vendor: self.kind.vendor().into(),
            description: format!(
                "{} pela CLI oficial, com o login da sua assinatura ({}); as ferramentas são as do Orchestrator, por MCP.",
                self.kind.program(),
                self.kind.subscription()
            ),
        }
    }

    fn capabilities(&self) -> ProviderCapabilities {
        let models = self.models();
        ProviderCapabilities {
            streaming: true,
            tool_calls: true,
            resume: true,
            cancel: true,
            native_subagents: false,
            reasoning: true,
            token_usage: true,
            // Subscriptions are not charged per token.
            cost: false,
            completion: true,
            compaction: false,
            default_model: self
                .settings
                .default_model
                .clone()
                .filter(|m| models.contains(m))
                .or_else(|| models.first().cloned()),
            models: models
                .into_iter()
                .map(|id| ModelInfo {
                    name: id.clone(),
                    id,
                    supports_tools: Some(true),
                    tags: vec!["assinatura".into()],
                    ..Default::default()
                })
                .collect(),
        }
    }

    async fn inspect(&self) -> ProviderStatus {
        let status = crate::status(self.kind, self.settings.program.as_deref()).await;
        ProviderStatus {
            available: status.program.is_some() && status.logged_in != Some(false),
            version: status.version,
            authenticated: status.logged_in,
            detail: status.detail,
            checked_at: Utc::now(),
        }
    }

    async fn start(&self, spec: &SessionSpec) -> Result<NativeSession, ProviderError> {
        let reference = uuid::Uuid::now_v7().to_string();
        self.states.lock().insert(
            reference.clone(),
            State {
                system: spec.instructions.clone(),
                ..Default::default()
            },
        );
        Ok(NativeSession {
            reference,
            model: spec
                .model
                .clone()
                .or_else(|| self.capabilities().default_model),
            data: json!({}),
        })
    }

    async fn resume(
        &self,
        native: &NativeSession,
        _spec: &SessionSpec,
    ) -> Result<NativeSession, ProviderError> {
        let state = self.state(native);
        self.states.lock().insert(native.reference.clone(), state);
        Ok(native.clone())
    }

    async fn execute(
        &self,
        native: &NativeSession,
        input: &TurnInput,
        ctx: &TurnContext,
    ) -> Result<TurnOutput, ProviderError> {
        self.stream(native, input, ctx).await
    }

    async fn stream(
        &self,
        native: &NativeSession,
        input: &TurnInput,
        ctx: &TurnContext,
    ) -> Result<TurnOutput, ProviderError> {
        let mut state = self.state(native);
        if let Some(context) = input.context.as_deref().filter(|c| !c.trim().is_empty()) {
            state.system = Some(match state.system.take().filter(|s| !s.is_empty()) {
                Some(base) => format!("{base}\n\n{context}"),
                None => context.to_owned(),
            });
        }
        let route = self.server.route(ctx.clone());
        let scratch = self.scratch.join(&native.reference);
        // Claude Code takes the instructions apart; the others get them
        // with the first message of their session.
        let prompt = match (&state.system, self.kind, state.started) {
            (Some(system), CliKind::Codex | CliKind::Gemini, false) => format!(
                "<instructions>\n{system}\n</instructions>\n\n{}",
                input.text
            ),
            _ => input.text.clone(),
        };
        let plan = RunPlan {
            kind: self.kind,
            model: native.model.as_deref(),
            session: &native.reference,
            resume: state.started,
            thread: state.thread.as_deref(),
            mcp_url: Some(&route.url),
            own_tools: self.settings.own_tools,
            project: ctx.project_path(),
            scratch: &scratch,
            system: state
                .system
                .as_deref()
                .filter(|_| self.kind == CliKind::ClaudeCode),
            ephemeral: false,
            extra_args: &self.settings.extra_args,
        };
        let mut session_seen: Option<String> = None;
        let mut usage = TokenUsage::default();
        let mut streamed_any = false;
        let mut on = |event: Event| match event {
            Event::Session(id) => session_seen = Some(id),
            Event::Text(t) => {
                streamed_any = true;
                ctx.emit_text(&t);
            }
            Event::Message(t) if !streamed_any => {
                ctx.emit_text(&t);
                ctx.emit_text("\n\n");
            }
            Event::Reasoning(t) => ctx.emit_reasoning(&t),
            Event::Usage(u) => usage = u,
            Event::Notice(n) => ctx.notice(NoticeLevel::Info, n),
            _ => {}
        };
        let outcome = self.run(plan, &prompt, &ctx.cancellation(), &mut on).await;
        drop(route);
        let _ = std::fs::remove_dir_all(&scratch);
        if let Some(id) = session_seen {
            state.started = true;
            if self.kind == CliKind::Codex {
                state.thread = Some(id);
            }
        }
        self.states.lock().insert(native.reference.clone(), state);
        if usage != TokenUsage::default() {
            ctx.report_usage(usage);
        }
        Ok(TurnOutput { text: outcome? })
    }

    async fn cancel(&self, _native: &NativeSession) -> Result<(), ProviderError> {
        // The turn's cancellation kills the process.
        Ok(())
    }

    async fn snapshot(&self, native: &NativeSession) -> NativeSession {
        let state = self.state(native);
        let mut native = native.clone();
        native.data = json!({
            "started": state.started,
            "thread": state.thread,
            "system": state.system,
        });
        native
    }

    async fn complete(
        &self,
        request: &CompletionRequest,
        cancel: &CancellationToken,
    ) -> Result<Completion, ProviderError> {
        let scratch = self
            .scratch
            .join(format!("complete-{}", uuid::Uuid::now_v7().simple()));
        let model = request
            .model
            .clone()
            .filter(|m| !m.trim().is_empty())
            .or_else(|| self.capabilities().default_model);
        let session = uuid::Uuid::now_v7().to_string();
        let prompt = match (&request.system, self.kind) {
            (Some(system), CliKind::Codex | CliKind::Gemini) => {
                format!(
                    "<instructions>\n{system}\n</instructions>\n\n{}",
                    request.prompt
                )
            }
            _ => request.prompt.clone(),
        };
        let project = std::env::temp_dir();
        let plan = RunPlan {
            kind: self.kind,
            model: model.as_deref(),
            session: &session,
            resume: false,
            thread: None,
            mcp_url: None,
            own_tools: false,
            project: &project,
            scratch: &scratch,
            system: request
                .system
                .as_deref()
                .filter(|_| self.kind == CliKind::ClaudeCode),
            ephemeral: true,
            extra_args: &self.settings.extra_args,
        };
        let mut usage = TokenUsage::default();
        let mut on = |event: Event| {
            if let Event::Usage(u) = event {
                usage = u;
            }
        };
        let text = self.run(plan, &prompt, cancel, &mut on).await;
        let _ = std::fs::remove_dir_all(&scratch);
        Ok(Completion {
            text: text?,
            model,
            usage,
        })
    }
}

/// Where a run's files go: `<data>/cli`.
pub fn scratch_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("cli")
}
