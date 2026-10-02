//! The three CLIs: how to install and log in, how to run one turn, and
//! how to read what they print.

use orchestrator_core::TokenUsage;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CliKind {
    /// Claude Code: a Claude Pro/Max subscription (or an API key).
    ClaudeCode,
    /// Codex CLI: a ChatGPT Plus/Pro/Business subscription.
    Codex,
    /// Gemini CLI: a Google account (free tier, Google AI Pro/Ultra).
    Gemini,
}

pub const ALL: [CliKind; 3] = [CliKind::ClaudeCode, CliKind::Codex, CliKind::Gemini];

/// Name of the Orchestrator's MCP server inside the CLIs.
pub const SERVER: &str = "orchestrator";

/// Gemini CLI's own tools, turned off so every action goes through the
/// Orchestrator's.
const GEMINI_OWN_TOOLS: [&str; 13] = [
    "run_shell_command",
    "write_file",
    "replace",
    "read_file",
    "read_many_files",
    "list_directory",
    "glob",
    "grep_search",
    "search_file_content",
    "web_fetch",
    "google_web_search",
    "save_memory",
    "write_todos",
];

impl CliKind {
    pub fn id(self) -> &'static str {
        match self {
            Self::ClaudeCode => "claude-code",
            Self::Codex => "codex",
            Self::Gemini => "gemini-cli",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        ALL.into_iter().find(|k| k.id() == id)
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::ClaudeCode => "Claude Code (assinatura)",
            Self::Codex => "ChatGPT · Codex (assinatura)",
            Self::Gemini => "Gemini CLI (conta Google)",
        }
    }

    pub fn vendor(self) -> &'static str {
        match self {
            Self::ClaudeCode => "Anthropic",
            Self::Codex => "OpenAI",
            Self::Gemini => "Google",
        }
    }

    pub fn program(self) -> &'static str {
        match self {
            Self::ClaudeCode => "claude",
            Self::Codex => "codex",
            Self::Gemini => "gemini",
        }
    }

    pub fn subscription(self) -> &'static str {
        match self {
            Self::ClaudeCode => "Claude Pro ou Max (ou uma chave da API da Anthropic)",
            Self::Codex => "ChatGPT Plus, Pro, Business ou Enterprise",
            Self::Gemini => "conta Google (gratuita, Google AI Pro ou Ultra)",
        }
    }

    pub fn install_command(self) -> &'static str {
        match self {
            Self::ClaudeCode => "npm install -g @anthropic-ai/claude-code",
            Self::Codex => "npm install -g @openai/codex",
            Self::Gemini => "npm install -g @google/gemini-cli",
        }
    }

    pub fn login_command(self) -> &'static str {
        match self {
            Self::ClaudeCode => "claude auth login",
            Self::Codex => "codex login",
            // The first interactive run offers "Login with Google".
            Self::Gemini => "gemini",
        }
    }

    pub fn login_hint(self) -> &'static str {
        match self {
            Self::ClaudeCode => "abre o navegador para entrar na sua conta Claude",
            Self::Codex => "abre o navegador para entrar na sua conta ChatGPT",
            Self::Gemini => {
                "abre o Gemini CLI: escolha \"Login with Google\", entre no navegador e depois feche com /quit"
            }
        }
    }

    /// Model aliases offered by default (the CLI's own default when empty).
    pub fn default_models(self) -> Vec<String> {
        match self {
            Self::ClaudeCode => vec!["sonnet".into(), "opus".into(), "haiku".into()],
            Self::Codex | Self::Gemini => Vec::new(),
        }
    }
}

/// Everything one run needs.
pub struct RunPlan<'a> {
    pub kind: CliKind,
    pub model: Option<&'a str>,
    /// Our session id (a UUID): Claude Code's and Gemini's session id.
    pub session: &'a str,
    /// The CLI already has this session (resume instead of creating it).
    pub resume: bool,
    /// Codex's thread id, once known.
    pub thread: Option<&'a str>,
    /// The Orchestrator's MCP URL for this turn; `None`: no tools.
    pub mcp_url: Option<&'a str>,
    /// The CLI keeps its own tools (outside the Orchestrator's gate).
    pub own_tools: bool,
    pub project: &'a Path,
    /// Scratch folder for this run's files.
    pub scratch: &'a Path,
    /// System instructions (Claude Code takes them apart).
    pub system: Option<&'a str>,
    /// One-off request: nothing kept.
    pub ephemeral: bool,
    pub extra_args: &'a [String],
}

/// Arguments and environment of one run; the prompt goes on stdin.
pub struct Invocation {
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
}

pub fn invocation(plan: &RunPlan<'_>) -> std::io::Result<Invocation> {
    std::fs::create_dir_all(plan.scratch)?;
    // Gemini CLI ignores settings in a folder others can write to; the
    // run's files are the user's only.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let private = std::fs::Permissions::from_mode(0o700);
        std::fs::set_permissions(plan.scratch, private.clone())?;
        if let Some(parent) = plan.scratch.parent() {
            let _ = std::fs::set_permissions(parent, private);
        }
    }
    let mut args: Vec<String> = Vec::new();
    let mut env = Vec::new();
    match plan.kind {
        CliKind::ClaudeCode => {
            args.extend(
                [
                    "-p",
                    "--output-format",
                    "stream-json",
                    "--verbose",
                    "--include-partial-messages",
                ]
                .map(String::from),
            );
            if let Some(model) = plan.model {
                args.extend(["--model".into(), model.into()]);
            }
            if plan.ephemeral {
                args.push("--no-session-persistence".into());
            } else if plan.resume {
                args.extend(["--resume".into(), plan.session.into()]);
            } else {
                args.extend(["--session-id".into(), plan.session.into()]);
            }
            if let Some(system) = plan.system.filter(|s| !s.trim().is_empty()) {
                let file = plan.scratch.join("system.md");
                std::fs::write(&file, system)?;
                args.extend([
                    "--append-system-prompt-file".into(),
                    file.display().to_string(),
                ]);
            }
            match plan.mcp_url {
                Some(url) => {
                    let file = plan.scratch.join("mcp.json");
                    std::fs::write(
                        &file,
                        json!({"mcpServers": {SERVER: {"type": "http", "url": url}}}).to_string(),
                    )?;
                    args.extend([
                        "--mcp-config".into(),
                        file.display().to_string(),
                        "--strict-mcp-config".into(),
                    ]);
                    args.extend(["--allowedTools".into(), format!("mcp__{SERVER}")]);
                    if !plan.own_tools {
                        args.extend(["--tools".into(), String::new()]);
                    }
                    args.extend(["--permission-mode".into(), "dontAsk".into()]);
                }
                None => args.extend(["--tools".into(), String::new()]),
            }
        }
        CliKind::Codex => {
            args.push("exec".into());
            args.extend([
                "--json".into(),
                "--skip-git-repo-check".into(),
                "--color".into(),
                "never".into(),
            ]);
            args.extend(["-C".into(), plan.project.display().to_string()]);
            if !plan.own_tools {
                // Codex's own shell may only read; changes go through the
                // Orchestrator's tools.
                args.extend(["-s".into(), "read-only".into()]);
            }
            args.extend(["-c".into(), "approval_policy=never".into()]);
            if let Some(model) = plan.model {
                args.extend(["-m".into(), model.into()]);
            }
            if let Some(url) = plan.mcp_url {
                // Not valid TOML, so Codex takes it as a plain string (no
                // quotes to survive Windows' command line).
                args.extend(["-c".into(), format!("mcp_servers.{SERVER}.url={url}")]);
            }
            if plan.ephemeral {
                args.push("--ephemeral".into());
            }
            if let (true, Some(thread)) = (plan.resume, plan.thread) {
                args.extend(["resume".into(), thread.into()]);
            }
            args.push("-".into());
        }
        CliKind::Gemini => {
            args.extend(["-o".into(), "stream-json".into(), "--skip-trust".into()]);
            if let Some(model) = plan.model {
                args.extend(["-m".into(), model.into()]);
            }
            if !plan.ephemeral {
                if plan.resume {
                    args.extend(["--resume".into(), plan.session.into()]);
                } else {
                    args.extend(["--session-id".into(), plan.session.into()]);
                }
            }
            let mut settings = json!({});
            if let Some(url) = plan.mcp_url {
                settings["mcpServers"] =
                    json!({SERVER: {"httpUrl": url, "trust": true, "timeout": 600_000}});
                args.extend(["--allowed-mcp-server-names".into(), SERVER.into()]);
            }
            if !plan.own_tools {
                settings["tools"] = json!({"exclude": GEMINI_OWN_TOOLS});
            }
            let file = plan.scratch.join("gemini-settings.json");
            std::fs::write(&file, settings.to_string())?;
            env.push((
                "GEMINI_CLI_SYSTEM_SETTINGS_PATH".into(),
                file.display().to_string(),
            ));
            // Headless mode needs -p; the message itself comes on stdin.
            args.extend(["-p".into(), String::new()]);
        }
    }
    args.extend(plan.extra_args.iter().cloned());
    Ok(Invocation { args, env })
}

/// What a line of the CLI's output means.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// The CLI's session or thread id.
    Session(String),
    /// Text as it arrives.
    Text(String),
    /// A whole message (used only when no streamed text came).
    Message(String),
    Reasoning(String),
    /// A tool the CLI is calling (shown; the call itself reaches us
    /// through MCP when it is ours).
    Tool(String),
    Usage(TokenUsage),
    /// The run's final answer.
    Final(String),
    /// Something the user should know that does not end the turn.
    Notice(String),
    Error(String),
}

/// Reads one line of output.
pub fn parse(kind: CliKind, line: &str) -> Vec<Event> {
    let Ok(v) = serde_json::from_str::<Value>(line.trim()) else {
        return Vec::new();
    };
    let s = |p: &str| v.pointer(p).and_then(Value::as_str).map(str::to_owned);
    let n = |val: &Value, p: &str| val.pointer(p).and_then(Value::as_u64).unwrap_or(0);
    match kind {
        CliKind::ClaudeCode => match v.get("type").and_then(Value::as_str) {
            Some("system") if s("/subtype").as_deref() == Some("init") => {
                s("/session_id").map(Event::Session).into_iter().collect()
            }
            Some("stream_event") => match (
                s("/event/type").as_deref(),
                s("/event/delta/type").as_deref(),
            ) {
                (Some("content_block_delta"), Some("text_delta")) => s("/event/delta/text")
                    .map(Event::Text)
                    .into_iter()
                    .collect(),
                (Some("content_block_delta"), Some("thinking_delta")) => s("/event/delta/thinking")
                    .map(Event::Reasoning)
                    .into_iter()
                    .collect(),
                _ => Vec::new(),
            },
            Some("assistant") => {
                let mut out = Vec::new();
                for block in v
                    .pointer("/message/content")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default()
                {
                    match block.get("type").and_then(Value::as_str) {
                        Some("text") => {
                            if let Some(t) = block.get("text").and_then(Value::as_str) {
                                out.push(Event::Message(t.to_owned()));
                            }
                        }
                        Some("tool_use") => {
                            if let Some(t) = block.get("name").and_then(Value::as_str) {
                                out.push(Event::Tool(t.to_owned()));
                            }
                        }
                        _ => {}
                    }
                }
                out
            }
            Some("result") => {
                let mut out = Vec::new();
                if let Some(usage) = v.get("usage") {
                    out.push(Event::Usage(TokenUsage {
                        input_tokens: n(usage, "/input_tokens")
                            + n(usage, "/cache_read_input_tokens")
                            + n(usage, "/cache_creation_input_tokens"),
                        output_tokens: n(usage, "/output_tokens"),
                        cached_input_tokens: n(usage, "/cache_read_input_tokens"),
                        cache_write_tokens: n(usage, "/cache_creation_input_tokens"),
                        ..Default::default()
                    }));
                }
                let error = v.get("is_error").and_then(Value::as_bool).unwrap_or(false)
                    || s("/subtype").is_some_and(|st| st.starts_with("error"));
                let text = s("/result").unwrap_or_default();
                if error {
                    out.push(Event::Error(if text.is_empty() {
                        s("/subtype").unwrap_or_else(|| "a CLI terminou com erro".into())
                    } else {
                        text
                    }));
                } else {
                    out.push(Event::Final(text));
                }
                out
            }
            _ => Vec::new(),
        },
        CliKind::Codex => match v.get("type").and_then(Value::as_str) {
            Some("thread.started") => s("/thread_id").map(Event::Session).into_iter().collect(),
            Some("item.completed") => match s("/item/type").as_deref() {
                Some("agent_message") => s("/item/text").map(Event::Message).into_iter().collect(),
                Some("reasoning") => s("/item/text").map(Event::Reasoning).into_iter().collect(),
                Some("mcp_tool_call") => s("/item/tool").map(Event::Tool).into_iter().collect(),
                Some("command_execution") => s("/item/command")
                    .map(|c| Event::Notice(format!("o Codex rodou por conta própria: {c}")))
                    .into_iter()
                    .collect(),
                Some("error") => s("/item/message").map(Event::Notice).into_iter().collect(),
                _ => Vec::new(),
            },
            Some("turn.completed") => {
                let usage = v.get("usage").cloned().unwrap_or(Value::Null);
                vec![Event::Usage(TokenUsage {
                    input_tokens: n(&usage, "/input_tokens"),
                    output_tokens: n(&usage, "/output_tokens"),
                    cached_input_tokens: n(&usage, "/cached_input_tokens"),
                    ..Default::default()
                })]
            }
            Some("turn.failed") => vec![Event::Error(
                s("/error/message").unwrap_or_else(|| "o Codex falhou".into()),
            )],
            Some("error") => s("/message").map(Event::Notice).into_iter().collect(),
            _ => Vec::new(),
        },
        CliKind::Gemini => match v.get("type").and_then(Value::as_str) {
            Some("init") => s("/session_id").map(Event::Session).into_iter().collect(),
            Some("message") if s("/role").as_deref() == Some("assistant") => {
                let text = s("/content").unwrap_or_default();
                if v.get("delta").and_then(Value::as_bool).unwrap_or(false) {
                    vec![Event::Text(text)]
                } else {
                    vec![Event::Message(text)]
                }
            }
            Some("tool_use") => s("/tool_name").map(Event::Tool).into_iter().collect(),
            Some("error") => s("/message").map(Event::Notice).into_iter().collect(),
            Some("result") => {
                let mut out = Vec::new();
                if let Some(stats) = v.get("stats") {
                    out.push(Event::Usage(TokenUsage {
                        input_tokens: n(stats, "/input_tokens"),
                        output_tokens: n(stats, "/output_tokens"),
                        cached_input_tokens: n(stats, "/cached"),
                        ..Default::default()
                    }));
                }
                if s("/status").as_deref() == Some("error") {
                    out.push(Event::Error(
                        s("/error/message").unwrap_or_else(|| "o Gemini CLI falhou".into()),
                    ));
                } else {
                    out.push(Event::Final(String::new()));
                }
                out
            }
            _ => Vec::new(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_three_formats() {
        let claude = |l: &str| parse(CliKind::ClaudeCode, l);
        assert_eq!(
            claude(r#"{"type":"system","subtype":"init","session_id":"abc"}"#),
            vec![Event::Session("abc".into())]
        );
        assert_eq!(
            claude(
                r#"{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Oi"}}}"#
            ),
            vec![Event::Text("Oi".into())]
        );
        let result = claude(
            r#"{"type":"result","subtype":"success","is_error":false,"result":"Pronto","usage":{"input_tokens":10,"cache_read_input_tokens":90,"output_tokens":5}}"#,
        );
        assert_eq!(result[1], Event::Final("Pronto".into()));
        let Event::Usage(u) = &result[0] else {
            panic!()
        };
        assert_eq!(
            (u.input_tokens, u.cached_input_tokens, u.output_tokens),
            (100, 90, 5)
        );
        assert!(matches!(
            claude(
                r#"{"type":"result","subtype":"error_during_execution","is_error":true,"result":""}"#
            )[0],
            Event::Error(_)
        ));

        let codex = |l: &str| parse(CliKind::Codex, l);
        assert_eq!(
            codex(r#"{"type":"thread.started","thread_id":"t1"}"#),
            vec![Event::Session("t1".into())]
        );
        assert_eq!(
            codex(
                r#"{"type":"item.completed","item":{"id":"i","type":"agent_message","text":"Feito"}}"#
            ),
            vec![Event::Message("Feito".into())]
        );
        assert!(matches!(
            codex(r#"{"type":"turn.failed","error":{"message":"x"}}"#)[0],
            Event::Error(_)
        ));
        assert!(matches!(
            codex(r#"{"type":"error","message":"Reconnecting... 2/5"}"#)[0],
            Event::Notice(_)
        ));

        let gemini = |l: &str| parse(CliKind::Gemini, l);
        assert_eq!(
            gemini(r#"{"type":"message","role":"assistant","content":"Olá","delta":true}"#),
            vec![Event::Text("Olá".into())]
        );
        assert!(gemini(r#"{"type":"message","role":"user","content":"x"}"#).is_empty());
        let end = gemini(
            r#"{"type":"result","status":"success","stats":{"input_tokens":3,"output_tokens":2,"cached":1}}"#,
        );
        assert_eq!(end.len(), 2);
        assert!(gemini("not json").is_empty());
    }

    #[test]
    fn invocations_keep_the_cli_inside_the_orchestrator() {
        let dir = tempfile::tempdir().unwrap();
        let plan = |kind, resume| RunPlan {
            kind,
            model: Some("m"),
            session: "0190-uuid",
            resume,
            thread: Some("t-1"),
            mcp_url: Some("http://127.0.0.1:9/t/x"),
            own_tools: false,
            project: dir.path(),
            scratch: dir.path(),
            system: Some("regras"),
            ephemeral: false,
            extra_args: &[],
        };
        let claude = invocation(&plan(CliKind::ClaudeCode, false)).unwrap().args;
        let joined = claude.join(" ");
        assert!(
            joined.contains("--session-id 0190-uuid") && joined.contains("--strict-mcp-config")
        );
        assert!(joined.contains("--allowedTools mcp__orchestrator"));
        assert!(claude
            .windows(2)
            .any(|w| w[0] == "--tools" && w[1].is_empty()));
        let mcp: Value =
            serde_json::from_str(&std::fs::read_to_string(dir.path().join("mcp.json")).unwrap())
                .unwrap();
        assert_eq!(
            mcp["mcpServers"]["orchestrator"]["url"],
            "http://127.0.0.1:9/t/x"
        );
        assert!(invocation(&plan(CliKind::ClaudeCode, true))
            .unwrap()
            .args
            .join(" ")
            .contains("--resume 0190-uuid"));

        let codex = invocation(&plan(CliKind::Codex, true))
            .unwrap()
            .args
            .join(" ");
        assert!(
            codex.contains("-s read-only")
                && codex.contains("mcp_servers.orchestrator.url=http://127.0.0.1:9/t/x")
        );
        assert!(codex.ends_with("resume t-1 -"));

        let gemini = invocation(&plan(CliKind::Gemini, false)).unwrap();
        assert!(gemini.args.join(" ").contains("--session-id 0190-uuid"));
        let (_, path) = &gemini.env[0];
        let settings: Value =
            serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        assert_eq!(
            settings["mcpServers"]["orchestrator"]["httpUrl"],
            "http://127.0.0.1:9/t/x"
        );
        assert!(settings["tools"]["exclude"].as_array().unwrap().len() > 5);
    }
}
