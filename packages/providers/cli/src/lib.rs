//! AIs on the user's subscriptions through their official CLIs (ADR-0021):
//! Claude Code (Claude Pro/Max), Codex (ChatGPT) and Gemini CLI (Google
//! account). Each one is an `AIProvider`; the CLI runs once per turn with
//! the Orchestrator's tools at a local MCP URL ([`orchestrator_mcp::ToolServer`])
//! and its own tools off, so every action still goes through the autonomy
//! gate, the locks and the history. Logging in is the CLI's own.

mod kind;
mod provider;
mod settings;

pub use kind::{CliKind, ALL};
pub use provider::{scratch_dir, CliProvider};
pub use settings::{CliFile, CliSettings};

use orchestrator_core::{CallOrigin, ProviderId};
use orchestrator_mcp::ToolServer;
use orchestrator_providers::ProviderRegistry;
use parking_lot::RwLock;
use serde::Serialize;
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

/// The program: the configured path, else on the PATH.
pub fn find_program(kind: CliKind, configured: Option<&str>) -> Option<PathBuf> {
    if let Some(path) = configured.map(str::trim).filter(|p| !p.is_empty()) {
        let path = PathBuf::from(path);
        return path.is_file().then_some(path);
    }
    which::which(kind.program()).ok()
}

/// The command for a program. On Windows, npm's `.cmd` shims go through
/// `cmd /C`, and no console window opens.
pub fn command_for(program: &Path, args: &[String]) -> tokio::process::Command {
    let is_script = program
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("cmd") || e.eq_ignore_ascii_case("bat"));
    let mut command = if cfg!(windows) && is_script {
        let mut c = tokio::process::Command::new("cmd");
        c.arg("/C").arg(program);
        c
    } else {
        tokio::process::Command::new(program)
    };
    command.args(args);
    #[cfg(windows)]
    {
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    command
}

/// What the settings page shows about one CLI.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CliStatus {
    pub kind: CliKind,
    pub id: &'static str,
    pub name: &'static str,
    pub subscription: &'static str,
    pub program: Option<String>,
    pub version: Option<String>,
    /// `None`: could not tell.
    pub logged_in: Option<bool>,
    pub detail: Option<String>,
    pub install_command: &'static str,
    pub login_command: &'static str,
    pub login_hint: &'static str,
    pub suggested_models: Vec<String>,
    pub settings: CliSettings,
}

/// Output of a short command, or `None` when it did not run in time.
async fn output(program: &Path, args: &[&str]) -> Option<(bool, String)> {
    let args: Vec<String> = args.iter().map(|a| (*a).to_owned()).collect();
    let mut command = command_for(program, &args);
    command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    let out = tokio::time::timeout(Duration::from_secs(20), command.output())
        .await
        .ok()?
        .ok()?;
    let mut text = String::from_utf8_lossy(&out.stdout).to_string();
    text.push_str(&String::from_utf8_lossy(&out.stderr));
    Some((out.status.success(), text))
}

/// Installed? Which version? Logged in?
pub async fn status(kind: CliKind, configured: Option<&str>) -> CliStatus {
    let mut status = CliStatus {
        kind,
        id: kind.id(),
        name: kind.name(),
        subscription: kind.subscription(),
        program: None,
        version: None,
        logged_in: None,
        detail: None,
        install_command: kind.install_command(),
        login_command: kind.login_command(),
        login_hint: kind.login_hint(),
        suggested_models: kind.default_models(),
        settings: CliSettings::default(),
    };
    let Some(program) = find_program(kind, configured) else {
        status.detail = Some(match configured.filter(|p| !p.trim().is_empty()) {
            Some(path) => format!("{path} não existe"),
            None => format!("{} não foi encontrado no PATH", kind.program()),
        });
        return status;
    };
    status.program = Some(program.display().to_string());
    if let Some((_, text)) = output(&program, &["--version"]).await {
        status.version = text
            .split_whitespace()
            .find(|w| w.chars().next().is_some_and(|c| c.is_ascii_digit()))
            .map(|v| v.trim_end_matches(',').to_owned());
    }
    match kind {
        CliKind::ClaudeCode => {
            if let Some((_, text)) = output(&program, &["auth", "status"]).await {
                let json = text
                    .find('{')
                    .and_then(|at| serde_json::from_str::<Value>(&text[at..]).ok());
                status.logged_in = json
                    .as_ref()
                    .and_then(|v| v.get("loggedIn"))
                    .and_then(Value::as_bool);
                if let Some(method) = json
                    .as_ref()
                    .and_then(|v| v.get("authMethod"))
                    .and_then(Value::as_str)
                {
                    status.detail = Some(format!("login: {method}"));
                }
            }
        }
        CliKind::Codex => {
            if let Some((_, text)) = output(&program, &["login", "status"]).await {
                let lower = text.to_lowercase();
                status.logged_in = if lower.contains("not logged in") {
                    Some(false)
                } else if lower.contains("logged in") {
                    Some(true)
                } else {
                    None
                };
                status.detail = text
                    .lines()
                    .find(|l| !l.trim().is_empty())
                    .map(|l| l.trim().to_owned());
            }
        }
        CliKind::Gemini => {
            let home = std::env::var_os("HOME")
                .or_else(|| std::env::var_os("USERPROFILE"))
                .map(PathBuf::from);
            let oauth = home.map(|h| h.join(".gemini").join("oauth_creds.json"));
            let key = [
                "GEMINI_API_KEY",
                "GOOGLE_API_KEY",
                "GOOGLE_GENAI_USE_VERTEXAI",
            ]
            .iter()
            .find(|k| std::env::var(k).is_ok_and(|v| !v.trim().is_empty()));
            status.logged_in = Some(oauth.as_ref().is_some_and(|p| p.is_file()) || key.is_some());
            status.detail = Some(match (oauth.filter(|p| p.is_file()), key) {
                (Some(_), _) => "login com o Google".into(),
                (None, Some(k)) => format!("chave em {k}"),
                (None, None) => "sem login".into(),
            });
        }
    }
    status
}

/// The CLIs the user turned on, as providers.
pub struct CliManager {
    path: Option<PathBuf>,
    settings: RwLock<CliFile>,
    registry: Arc<ProviderRegistry>,
    server: ToolServer,
    scratch: PathBuf,
}

impl CliManager {
    /// Loads `clis.json` and registers the CLIs that are on.
    pub fn open(
        path: Option<&Path>,
        registry: Arc<ProviderRegistry>,
        server: ToolServer,
        scratch: PathBuf,
    ) -> (Self, Option<String>) {
        let (file, warning) = match path.map(std::fs::read_to_string) {
            Some(Ok(text)) => match serde_json::from_str::<CliFile>(&text) {
                Ok(file) => (file, None),
                Err(err) => (
                    CliFile::default(),
                    Some(format!("clis.json inválido ({err})")),
                ),
            },
            _ => (CliFile::default(), None),
        };
        let manager = Self {
            path: path.map(Path::to_path_buf),
            settings: RwLock::new(file),
            registry,
            server,
            scratch,
        };
        for kind in ALL {
            manager.apply(kind, CallOrigin::System);
        }
        (manager, warning)
    }

    pub fn settings(&self, kind: CliKind) -> CliSettings {
        self.settings
            .read()
            .clis
            .get(kind.id())
            .cloned()
            .unwrap_or_default()
    }

    fn apply(&self, kind: CliKind, origin: CallOrigin) {
        let settings = self.settings(kind);
        let id = ProviderId::from(kind.id());
        if settings.enabled {
            self.registry.replace(Arc::new(CliProvider::new(
                kind,
                settings,
                self.server.clone(),
                self.scratch.clone(),
            )));
        } else if self.registry.get(&id).is_some() {
            self.registry.unregister(&id, origin);
        }
    }

    pub fn save(&self, kind: CliKind, settings: CliSettings) -> Result<(), String> {
        let mut file = self.settings.read().clone();
        file.clis.insert(kind.id().to_owned(), settings);
        if let Some(path) = &self.path {
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
            }
            let text = serde_json::to_string_pretty(&file).map_err(|e| e.to_string())?;
            // Through a rename: never half written (ADR-0022).
            let tmp = path.with_extension("json.tmp");
            std::fs::write(&tmp, text).map_err(|e| e.to_string())?;
            std::fs::rename(&tmp, path).map_err(|e| e.to_string())?;
        }
        *self.settings.write() = file;
        self.apply(kind, CallOrigin::User);
        Ok(())
    }

    pub async fn view(&self) -> Vec<CliStatus> {
        let mut out = Vec::new();
        for kind in ALL {
            let settings = self.settings(kind);
            let mut status = status(kind, settings.program.as_deref()).await;
            status.settings = settings;
            out.push(status);
        }
        out
    }
}
