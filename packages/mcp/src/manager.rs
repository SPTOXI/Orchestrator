//! The user's MCP servers (`mcp.json`): started with the app, their tools
//! offered to the AIs as `mcp.<server>.<tool>` through [`McpTools`], behind
//! the autonomy gate like every other tool.

use crate::client::{CallOutcome, Expand, McpClient, McpError, RemoteTool};
use crate::config::{McpFile, ServerConfig};
use async_trait::async_trait;
use chrono::Utc;
use orchestrator_core::{
    AuditEvent, EventKind, EventSink, ToolCall, ToolDefinition, ToolError, ToolErrorKind,
    ToolResult,
};
use orchestrator_providers::ToolExecutor;
use parking_lot::RwLock;
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;
use tokio_util::sync::CancellationToken;

/// Longest tool name the APIs accept once `.` becomes `__` (OpenAI: 64).
const MAX_WIRE_NAME: usize = 64;
/// Longest text an MCP call returns to the AI (characters).
const MAX_OUTPUT: usize = 100_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ServerStatus {
    Disabled,
    Starting,
    Ready,
    Failed,
}

/// A tool of a server, as the AIs get it.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolView {
    /// `mcp.<server>.<tool>`.
    pub name: String,
    /// The server's own name for it.
    pub remote: String,
    pub description: String,
    pub read_only: bool,
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerView {
    pub config: ServerConfig,
    pub status: ServerStatus,
    pub error: Option<String>,
    /// `serverInfo.name` / `version`.
    pub server_name: Option<String>,
    pub server_version: Option<String>,
    pub tools: Vec<ToolView>,
    /// The server's last stderr lines (stdio).
    pub log: Vec<String>,
}

struct Live {
    status: ServerStatus,
    error: Option<String>,
    client: Option<Arc<McpClient>>,
    tools: Vec<(ToolView, RemoteTool)>,
}

impl Live {
    fn new(status: ServerStatus) -> Self {
        Self {
            status,
            error: None,
            client: None,
            tools: Vec::new(),
        }
    }
}

struct Inner {
    path: Option<PathBuf>,
    servers: RwLock<Vec<ServerConfig>>,
    live: RwLock<HashMap<String, Live>>,
    expand: Expand,
    mask: Arc<dyn Fn(&str) -> String + Send + Sync>,
    roots: RwLock<Vec<String>>,
    sink: Arc<dyn EventSink>,
}

/// Cheap to clone.
#[derive(Clone)]
pub struct McpManager {
    inner: Arc<Inner>,
}

impl McpManager {
    /// Loads `mcp.json` (a broken file: no servers and a warning). Call
    /// [`Self::start_all`] on the async runtime to connect.
    pub fn open(
        path: Option<&Path>,
        sink: Arc<dyn EventSink>,
        expand: Expand,
        mask: Arc<dyn Fn(&str) -> String + Send + Sync>,
    ) -> (Self, Option<String>) {
        let (file, warning) = match path.map(std::fs::read_to_string) {
            Some(Ok(text)) => match serde_json::from_str::<McpFile>(&text) {
                Ok(file) => (file, None),
                Err(err) => (
                    McpFile::default(),
                    Some(format!(
                        "mcp.json inválido ({err}); nenhum servidor MCP carregado"
                    )),
                ),
            },
            _ => (McpFile::default(), None),
        };
        let manager = Self {
            inner: Arc::new(Inner {
                path: path.map(Path::to_path_buf),
                servers: RwLock::new(file.servers),
                live: RwLock::new(HashMap::new()),
                expand,
                mask,
                roots: RwLock::new(Vec::new()),
                sink,
            }),
        };
        (manager, warning)
    }

    /// Folders the servers may ask about (`roots/list`): the open project.
    pub fn set_roots(&self, roots: Vec<String>) {
        *self.inner.roots.write() = roots;
    }

    pub fn servers(&self) -> Vec<ServerConfig> {
        self.inner.servers.read().clone()
    }

    pub fn view(&self) -> Vec<ServerView> {
        let live = self.inner.live.read();
        self.servers()
            .into_iter()
            .map(|config| {
                let state = live.get(&config.id);
                let info = state
                    .and_then(|s| s.client.as_ref())
                    .map(|c| c.server_info.clone())
                    .unwrap_or(Value::Null);
                ServerView {
                    status: match state {
                        _ if !config.enabled => ServerStatus::Disabled,
                        Some(s) => s.status,
                        None => ServerStatus::Starting,
                    },
                    error: state.and_then(|s| s.error.clone()),
                    server_name: info.get("name").and_then(Value::as_str).map(str::to_owned),
                    server_version: info
                        .get("version")
                        .and_then(Value::as_str)
                        .map(str::to_owned),
                    tools: state
                        .map(|s| s.tools.iter().map(|(v, _)| v.clone()).collect())
                        .unwrap_or_default(),
                    log: state
                        .and_then(|s| s.client.as_ref())
                        .map(|c| c.stderr())
                        .unwrap_or_default(),
                    config,
                }
            })
            .collect()
    }

    /// Connects every enabled server, in parallel.
    pub async fn start_all(&self) {
        let ids: Vec<String> = self
            .servers()
            .into_iter()
            .filter(|s| s.enabled)
            .map(|s| s.id)
            .collect();
        let tasks: Vec<_> = ids
            .into_iter()
            .map(|id| {
                let this = self.clone();
                tokio::spawn(async move { this.restart(&id).await })
            })
            .collect();
        for task in tasks {
            let _ = task.await;
        }
    }

    /// (Re)connects one server; a disabled one is stopped.
    pub async fn restart(&self, id: &str) {
        self.stop(id);
        let Some(config) = self.servers().into_iter().find(|s| s.id == id) else {
            return;
        };
        if !config.enabled {
            self.inner
                .live
                .write()
                .insert(id.to_owned(), Live::new(ServerStatus::Disabled));
            return;
        }
        self.inner
            .live
            .write()
            .insert(id.to_owned(), Live::new(ServerStatus::Starting));
        let roots = self.inner.roots.read().clone();
        let outcome = async {
            let client = McpClient::connect(&config, &self.inner.expand, roots).await?;
            let tools = client.list_tools().await?;
            Ok::<_, McpError>((client, tools))
        }
        .await;
        let mut live = Live::new(ServerStatus::Ready);
        match outcome {
            Ok((client, tools)) => {
                live.tools = expose(&config, tools);
                live.client = Some(Arc::new(client));
            }
            Err(err) => {
                live.status = ServerStatus::Failed;
                live.error = Some((self.inner.mask)(&err.message));
            }
        }
        // The server may have been removed or changed meanwhile.
        if self.servers().iter().any(|s| s.id == id && *s == config) {
            self.inner.live.write().insert(id.to_owned(), live);
        } else if let Some(client) = live.client {
            client.close();
        }
    }

    fn stop(&self, id: &str) {
        if let Some(live) = self.inner.live.write().remove(id) {
            if let Some(client) = live.client {
                client.close();
            }
        }
    }

    /// Adds or replaces a server (`previous_id` when its id changed) and
    /// saves the file. Connect it with [`Self::restart`].
    pub fn save(&self, config: ServerConfig, previous_id: Option<&str>) -> Result<(), String> {
        config.validate()?;
        let mut servers = self.servers();
        let previous = previous_id.unwrap_or(&config.id).to_owned();
        if previous != config.id && servers.iter().any(|s| s.id == config.id) {
            return Err(format!("já existe um servidor {}", config.id));
        }
        match servers.iter_mut().find(|s| s.id == previous) {
            Some(slot) => *slot = config.clone(),
            None => servers.push(config.clone()),
        }
        self.write(servers)?;
        if previous != config.id {
            self.stop(&previous);
        }
        Ok(())
    }

    /// Adds servers from Claude/Cursor JSON; ids that exist get a suffix.
    pub fn import(&self, text: &str) -> Result<Vec<String>, String> {
        let incoming = crate::config::import(text)?;
        let mut servers = self.servers();
        let mut added = Vec::new();
        for mut config in incoming {
            let base = config.id.clone();
            let mut n = 2;
            while servers.iter().any(|s| s.id == config.id) {
                let suffix = format!("-{n}");
                config.id = format!("{}{suffix}", &base[..base.len().min(24 - suffix.len())]);
                n += 1;
            }
            added.push(config.id.clone());
            servers.push(config);
        }
        self.write(servers)?;
        Ok(added)
    }

    pub fn delete(&self, id: &str) -> Result<(), String> {
        let mut servers = self.servers();
        servers.retain(|s| s.id != id);
        self.write(servers)?;
        self.stop(id);
        Ok(())
    }

    fn write(&self, servers: Vec<ServerConfig>) -> Result<(), String> {
        if let Some(path) = &self.inner.path {
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
            }
            let text = serde_json::to_string_pretty(&McpFile {
                servers: servers.clone(),
            })
            .map_err(|e| e.to_string())?;
            let tmp = path.with_extension("json.tmp");
            std::fs::write(&tmp, text).map_err(|e| e.to_string())?;
            std::fs::rename(&tmp, path).map_err(|e| e.to_string())?;
        }
        *self.inner.servers.write() = servers;
        Ok(())
    }

    /// Stops every server (the app is closing).
    pub fn shutdown(&self) {
        let ids: Vec<String> = self.inner.live.read().keys().cloned().collect();
        for id in ids {
            self.stop(&id);
        }
    }

    /// The tools the AIs get now.
    pub fn definitions(&self) -> Vec<ToolDefinition> {
        let live = self.inner.live.read();
        let mut out = Vec::new();
        for config in self.servers().iter().filter(|s| s.enabled) {
            let Some(state) = live
                .get(&config.id)
                .filter(|s| s.status == ServerStatus::Ready)
            else {
                continue;
            };
            for (view, remote) in state.tools.iter().filter(|(v, _)| v.enabled) {
                out.push(ToolDefinition {
                    name: view.name.clone(),
                    group: "mcp".into(),
                    description: format!(
                        "[MCP: {}] {}",
                        config.label(),
                        if view.description.is_empty() {
                            remote.title.clone().unwrap_or_else(|| remote.name.clone())
                        } else {
                            view.description.clone()
                        }
                    ),
                    read_only: view.read_only,
                    parameters: remote.input_schema.clone(),
                });
            }
        }
        out
    }

    /// Server, client and remote name of an exposed tool.
    fn resolve(&self, name: &str) -> Option<(ServerConfig, Arc<McpClient>, String)> {
        let rest = name.strip_prefix("mcp.")?;
        let (id, _) = rest.split_once('.')?;
        let config = self
            .servers()
            .into_iter()
            .find(|s| s.id == id && s.enabled)?;
        let live = self.inner.live.read();
        let state = live.get(id)?;
        let client = state.client.clone()?;
        let remote = state
            .tools
            .iter()
            .find(|(v, _)| v.name == name && v.enabled)?
            .1
            .name
            .clone();
        Some((config, client, remote))
    }

    async fn call(
        &self,
        name: &str,
        args: Value,
        cancel: &CancellationToken,
    ) -> Result<CallOutcome, ToolError> {
        let (config, client, remote) = self.resolve(name).ok_or_else(|| {
            ToolError::new(
                ToolErrorKind::UnknownTool,
                format!(
                    "{name}: o servidor MCP não está conectado ou a ferramenta não existe mais"
                ),
            )
        })?;
        if client.is_closed() {
            let this = self.clone();
            let id = config.id.clone();
            tokio::spawn(async move { this.restart(&id).await });
            return Err(ToolError::new(
                ToolErrorKind::NotRunning,
                format!("o servidor MCP {} terminou; reiniciando", config.label()),
            ));
        }
        let outcome = client
            .call_tool(&remote, args, config.timeout(), cancel)
            .await
            .map_err(|err| {
                if err.cancelled {
                    ToolError::new(ToolErrorKind::Cancelled, err.message)
                } else {
                    ToolError::new(
                        ToolErrorKind::CommandFailed,
                        (self.inner.mask)(&err.message),
                    )
                }
            })?;
        if client.tools_changed() {
            let this = self.clone();
            let id = config.id;
            tokio::spawn(async move { this.refresh_tools(&id).await });
        }
        Ok(outcome)
    }

    async fn refresh_tools(&self, id: &str) {
        let Some((config, client)) =
            self.servers()
                .into_iter()
                .find(|s| s.id == id)
                .and_then(|c| {
                    let client = self.inner.live.read().get(id)?.client.clone()?;
                    Some((c, client))
                })
        else {
            return;
        };
        if let Ok(tools) = client.list_tools().await {
            if let Some(state) = self.inner.live.write().get_mut(id) {
                state.tools = expose(&config, tools);
            }
        }
    }
}

/// Exposed names: `mcp.<server>.<tool>`, with only the characters every
/// API accepts, no `__` (it means `.` on the wire), and short enough.
fn expose(config: &ServerConfig, tools: Vec<RemoteTool>) -> Vec<(ToolView, RemoteTool)> {
    let budget = MAX_WIRE_NAME - "mcp__".len() - config.id.len() - "__".len();
    let mut taken: Vec<String> = Vec::new();
    let mut out = Vec::new();
    for tool in tools {
        let mut part: String = tool
            .name
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        while part.contains("__") {
            part = part.replace("__", "_");
        }
        let mut part = part.trim_matches('_').to_owned();
        if part.is_empty() {
            part = "tool".into();
        }
        part.truncate(budget);
        let mut name = format!("mcp.{}.{part}", config.id);
        let mut n = 2;
        while taken.contains(&name) {
            let suffix = format!("-{n}");
            let mut base = part.clone();
            base.truncate(budget.saturating_sub(suffix.len()));
            name = format!("mcp.{}.{base}{suffix}", config.id);
            n += 1;
        }
        taken.push(name.clone());
        out.push((
            ToolView {
                name,
                remote: tool.name.clone(),
                description: tool.description.clone(),
                read_only: tool.read_only,
                enabled: !config.disabled_tools.contains(&tool.name),
            },
            tool,
        ));
    }
    out
}

/// The app's tools plus every MCP server's.
pub struct McpTools {
    inner: Arc<dyn ToolExecutor>,
    mcp: McpManager,
}

impl McpTools {
    pub fn new(inner: Arc<dyn ToolExecutor>, mcp: McpManager) -> Self {
        Self { inner, mcp }
    }
}

#[async_trait]
impl ToolExecutor for McpTools {
    fn tools(&self) -> Vec<ToolDefinition> {
        let mut tools = self.inner.tools();
        tools.extend(self.mcp.definitions());
        tools
    }

    async fn execute(&self, call: ToolCall) -> ToolResult {
        self.execute_with(call, CancellationToken::new()).await
    }

    async fn execute_with(&self, call: ToolCall, cancel: CancellationToken) -> ToolResult {
        if !call.tool.starts_with("mcp.") {
            return self.inner.execute_with(call, cancel).await;
        }
        let started_at = Utc::now();
        let clock = Instant::now();
        let read_only = self
            .mcp
            .definitions()
            .iter()
            .find(|d| d.name == call.tool)
            .map(|d| d.read_only);
        let outcome = self.mcp.call(&call.tool, call.args.clone(), &cancel).await;
        let duration_ms = clock.elapsed().as_millis() as u64;
        let mask = &self.mcp.inner.mask;
        let (ok, output, error) = match outcome {
            Ok(result) => {
                let mut text = mask(&result.text);
                if text.chars().count() > MAX_OUTPUT {
                    text = text.chars().take(MAX_OUTPUT).collect::<String>() + "\n[… cortado]";
                }
                if result.is_error {
                    let message = if text.is_empty() {
                        "a ferramenta MCP informou um erro".to_owned()
                    } else {
                        text
                    };
                    (
                        false,
                        Value::Null,
                        Some(ToolError::new(ToolErrorKind::CommandFailed, message)),
                    )
                } else {
                    let mut output = json!({"text": text});
                    if let Some(structured) = result.structured {
                        output["structured"] = structured;
                    }
                    if !result.other.is_empty() {
                        output["other"] = json!(result.other);
                    }
                    (true, output, None)
                }
            }
            Err(error) => (false, Value::Null, Some(error)),
        };
        let summary = match &error {
            None => call.tool.clone(),
            Some(err) => format!(
                "{} failed: {}",
                call.tool,
                err.message.chars().take(200).collect::<String>()
            ),
        };
        self.mcp.inner.sink.audit(
            AuditEvent::new(
                EventKind::ToolCalled,
                call.origin.clone(),
                summary,
                json!({
                    "tool": call.tool,
                    "readOnly": read_only,
                    "args": call.args,
                    "ok": ok,
                    "error": error,
                    "durationMs": duration_ms,
                    "mcp": true,
                }),
            )
            .with_call(call.id.clone()),
        );
        ToolResult {
            call_id: call.id,
            tool: call.tool,
            ok,
            output,
            error,
            started_at,
            finished_at: Utc::now(),
            duration_ms,
        }
    }
}
