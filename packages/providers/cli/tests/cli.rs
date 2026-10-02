//! CLI providers end to end (ADR-0021) with a fake CLI that speaks each
//! one's format: sessions with the real Tool Runtime behind the
//! Orchestrator's MCP URL, resuming, a tool the executor refuses, a CLI
//! without login, cancelling, the Council's one-off request, and the
//! settings that register and remove the providers.

use async_trait::async_trait;
use orchestrator_core::{
    MemorySink, SessionEvent, SessionId, SessionInfo, SessionStatus, ToolCall, ToolDefinition,
    ToolError, ToolErrorKind, ToolResult, TurnStatus,
};
use orchestrator_mcp::ToolServer;
use orchestrator_provider_cli::{CliKind, CliManager, CliSettings, ALL};
use orchestrator_providers::{
    CompletionRequest, ManagerConfig, ProviderRegistry, SessionManager, StartRequest, ToolExecutor,
};
use orchestrator_runtime::{RuntimeConfig, ToolRuntime};
use serde_json::Value;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;

/// The runtime's tools, with writes refused (as the autonomy gate would
/// when the user says no).
struct Tools(ToolRuntime);

#[async_trait]
impl ToolExecutor for Tools {
    fn tools(&self) -> Vec<ToolDefinition> {
        ToolRuntime::definitions().to_vec()
    }

    async fn execute(&self, call: ToolCall) -> ToolResult {
        if call.tool == "filesystem.write" {
            let now = chrono::Utc::now();
            return ToolResult {
                call_id: call.id,
                tool: call.tool,
                ok: false,
                output: Value::Null,
                error: Some(ToolError::new(ToolErrorKind::Denied, "o usuário negou")),
                started_at: now,
                finished_at: now,
                duration_ms: 0,
            };
        }
        self.0.invoke(call).await
    }
}

struct Harness {
    sessions: SessionManager,
    registry: Arc<ProviderRegistry>,
    manager: CliManager,
    dir: tempfile::TempDir,
    log: std::path::PathBuf,
}

async fn harness() -> Harness {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("hello.txt"), "olá do projeto").unwrap();
    let sink = Arc::new(MemorySink::new());
    let registry = Arc::new(ProviderRegistry::new(sink.clone()));
    let runtime = ToolRuntime::new(
        RuntimeConfig {
            base_dir: dir.path().to_path_buf(),
        },
        sink.clone(),
    );
    let sessions = SessionManager::with_config(
        registry.clone(),
        Arc::new(Tools(runtime)),
        sink,
        ManagerConfig {
            cancel_grace: Duration::from_millis(500),
            log_capacity: 2_000,
        },
    );
    let server = ToolServer::start().await.unwrap();
    let (manager, warning) = CliManager::open(
        Some(&dir.path().join("data/clis.json")),
        registry.clone(),
        server,
        dir.path().join("data/cli"),
    );
    assert!(warning.is_none());
    // One log for the whole test binary (the variable is process-wide).
    static LOG: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();
    let log = LOG
        .get_or_init(|| {
            let path = std::env::temp_dir().join(format!("fake-cli-{}.log", std::process::id()));
            // SAFETY (test): set once, before any CLI runs.
            unsafe { std::env::set_var("FAKE_CLI_LOG", &path) };
            path
        })
        .clone();
    for kind in ALL {
        manager
            .save(
                kind,
                CliSettings {
                    enabled: true,
                    program: Some(env!("CARGO_BIN_EXE_fake-cli").into()),
                    ..Default::default()
                },
            )
            .unwrap();
    }
    Harness {
        sessions,
        registry,
        manager,
        dir,
        log,
    }
}

impl Harness {
    async fn start(&self, provider: &str) -> SessionInfo {
        self.sessions
            .start(
                StartRequest {
                    provider: Some(provider.into()),
                    ..Default::default()
                },
                self.dir.path().to_path_buf(),
                orchestrator_core::CallOrigin::User,
            )
            .await
            .unwrap()
    }

    async fn turn(&self, id: &SessionId, text: &str) -> (TurnStatus, Option<String>, String) {
        self.sessions
            .send(id, text.into(), orchestrator_core::CallOrigin::User)
            .await
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(30);
        while self.sessions.info(id).unwrap().status == SessionStatus::Running {
            assert!(Instant::now() < deadline, "turn did not finish");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let events: Vec<SessionEvent> = self
            .sessions
            .snapshot(id)
            .unwrap()
            .entries
            .into_iter()
            .map(|e| e.event)
            .collect();
        let (status, error) = events
            .iter()
            .rev()
            .find_map(|e| match e {
                SessionEvent::TurnCompleted { status, error, .. } => Some((*status, error.clone())),
                _ => None,
            })
            .unwrap();
        // Text of the last turn only.
        let last_start = events
            .iter()
            .rposition(|e| matches!(e, SessionEvent::TurnStarted { .. }))
            .unwrap_or(0);
        let text = events[last_start..]
            .iter()
            .filter_map(|e| match e {
                SessionEvent::TextDelta { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect::<String>();
        (status, error, text)
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn each_cli_works_through_the_orchestrators_tools() {
    let h = harness().await;
    for kind in ALL {
        let session = h.start(kind.id()).await;
        let (status, error, text) = h.turn(&session.id, "LEIA hello.txt").await;
        assert_eq!(status, TurnStatus::Completed, "{kind:?}: {error:?}");
        let name = match kind {
            CliKind::ClaudeCode => "claude",
            CliKind::Codex => "codex",
            CliKind::Gemini => "gemini",
        };
        assert!(text.contains(&format!("{name}: nova")), "{kind:?}: {text}");
        assert!(
            text.contains("filesystem__read ok") && text.contains("olá do projeto"),
            "{kind:?} read the file through the Orchestrator: {text}"
        );
        assert!(
            !text.contains(" 0 ferramentas"),
            "{kind:?} got the tools: {text}"
        );

        // The second turn resumes the CLI's session; a write the executor
        // refuses comes back to the CLI as an error.
        let (status, _, text) = h.turn(&session.id, "ESCREVA novo.txt").await;
        assert_eq!(status, TurnStatus::Completed);
        assert!(
            text.contains(&format!("{name}: retomado")),
            "{kind:?}: {text}"
        );
        assert!(
            text.contains("filesystem__write falhou") && text.contains("negou"),
            "{text}"
        );
        assert!(!h.dir.path().join("novo.txt").exists());

        let info = h.sessions.info(&session.id).unwrap();
        assert!(info.usage.input_tokens > 0, "{kind:?} reported usage");
        assert_eq!(
            info.usage.cost_usd,
            Some(0.0),
            "{kind:?}: no cost per token"
        );
    }

    // Every run kept the CLIs' own tools off and pointed them at us.
    let runs: Vec<Value> = std::fs::read_to_string(&h.log)
        .unwrap()
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect();
    let args = |v: &Value| -> Vec<String> { serde_json::from_value(v["args"].clone()).unwrap() };
    let claude = runs
        .iter()
        .map(args)
        .find(|a| a.contains(&"--mcp-config".to_owned()))
        .unwrap();
    assert!(claude
        .windows(2)
        .any(|w| w[0] == "--tools" && w[1].is_empty()));
    let codex = runs
        .iter()
        .map(args)
        .find(|a| a.first().map(String::as_str) == Some("exec"))
        .unwrap();
    assert!(codex
        .windows(2)
        .any(|w| w[0] == "-s" && w[1] == "read-only"));
    let gemini = runs
        .iter()
        .find_map(|r| r["geminiSettings"].as_str().map(str::to_owned))
        .unwrap();
    assert!(
        gemini.contains("run_shell_command"),
        "Gemini's own tools excluded: {gemini}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn no_login_cancel_council_and_settings() {
    let h = harness().await;

    // Not logged in: the turn fails saying where to log in.
    for kind in ALL {
        let session = h.start(kind.id()).await;
        let (status, error, _) = h.turn(&session.id, "LOGIN").await;
        assert_eq!(status, TurnStatus::Failed);
        let error = error.unwrap();
        assert!(
            error.contains("Assinaturas") && error.contains(kind.login_command()),
            "{kind:?}: {error}"
        );
    }

    // Cancelling kills the CLI.
    let session = h.start("codex").await;
    h.sessions
        .send(
            &session.id,
            "DORMIR".into(),
            orchestrator_core::CallOrigin::User,
        )
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(500)).await;
    h.sessions.cancel(&session.id).await.unwrap();
    let started = Instant::now();
    while h.sessions.info(&session.id).unwrap().status == SessionStatus::Running {
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "cancel did not stop the CLI"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    // The Council's one-off request: no tools, nothing kept.
    let provider = h.registry.get(&"claude-code".into()).unwrap();
    let answer = provider
        .complete(
            &CompletionRequest {
                model: None,
                system: Some("Responda JSON.".into()),
                prompt: "olá".into(),
            },
            &CancellationToken::new(),
        )
        .await
        .unwrap();
    assert!(
        answer.text.contains("sem ferramentas") && answer.text.contains("com instruções"),
        "{}",
        answer.text
    );

    // Status, and turning a CLI off removes the provider.
    let view = h.manager.view().await;
    let codex = view.iter().find(|s| s.kind == CliKind::Codex).unwrap();
    assert_eq!(codex.version.as_deref(), Some("9.9.9"));
    assert!(codex.settings.enabled);
    h.manager
        .save(CliKind::Codex, CliSettings::default())
        .unwrap();
    assert!(h.registry.get(&"codex".into()).is_none());
    assert!(h.registry.get(&"claude-code".into()).is_some());
    let caps = provider.capabilities();
    assert_eq!(caps.default_model.as_deref(), Some("sonnet"));
    assert!(caps.cost, "a subscription turn costs US$ 0");
}

/// The real Claude Code against a fake Anthropic API (run by hand:
/// `REAL_CLAUDE=/path/to/claude ANTHROPIC_BASE_URL=… cargo test -- --ignored`).
#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn the_real_claude_code_uses_the_orchestrators_tools() {
    let Ok(program) = std::env::var("REAL_CLAUDE") else {
        return;
    };
    let h = harness().await;
    h.manager
        .save(
            CliKind::ClaudeCode,
            CliSettings {
                enabled: true,
                program: Some(program),
                models: vec!["claude-fake".into()],
                ..Default::default()
            },
        )
        .unwrap();
    let session = h.start("claude-code").await;
    let (status, error, text) = h.turn(&session.id, "Leia o arquivo hello.txt").await;
    assert_eq!(status, TurnStatus::Completed, "{error:?}");
    assert!(
        text.contains("Li pelo Orchestrator") && text.contains("olá do projeto"),
        "{text}"
    );
    let (status, error, text) = h.turn(&session.id, "De novo").await;
    assert_eq!(status, TurnStatus::Completed, "{error:?}");
    assert!(text.contains("Li pelo Orchestrator"), "resumed: {text}");
}
