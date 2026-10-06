//! An MCP client (protocol 2025-06-18): JSON-RPC 2.0 over a program's
//! stdin/stdout or over Streamable HTTP. What the Orchestrator needs:
//! `initialize`, `tools/list`, `tools/call`, cancelling a call, answering
//! the server's `ping` and `roots/list`, and noticing `tools/list_changed`.

use crate::config::{ServerConfig, Transport};
use futures_util::StreamExt;
use parking_lot::Mutex;
use serde_json::{json, Value};
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

pub const PROTOCOL_VERSION: &str = "2025-06-18";
/// Starting a server may first download it (`npx`, `uvx`).
pub const INIT_TIMEOUT: Duration = Duration::from_secs(120);
const LIST_TIMEOUT: Duration = Duration::from_secs(60);
/// Lines of the server's stderr kept to show the user.
const STDERR_LINES: usize = 60;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpError {
    pub message: String,
    pub cancelled: bool,
}

impl McpError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            cancelled: false,
        }
    }
}

impl std::fmt::Display for McpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

/// A tool as the server describes it.
#[derive(Debug, Clone, PartialEq)]
pub struct RemoteTool {
    pub name: String,
    pub title: Option<String>,
    pub description: String,
    pub input_schema: Value,
    /// `annotations.readOnlyHint`.
    pub read_only: bool,
}

/// What a call returned.
#[derive(Debug, Clone, PartialEq)]
pub struct CallOutcome {
    pub is_error: bool,
    /// The text parts, joined.
    pub text: String,
    pub structured: Option<Value>,
    /// Non-text parts (images, audio, resources), described.
    pub other: Vec<Value>,
}

type Pending = Arc<Mutex<HashMap<u64, oneshot::Sender<Result<Value, McpError>>>>>;

type SharedStdin = Arc<tokio::sync::Mutex<tokio::process::ChildStdin>>;

struct Stdio {
    stdin: SharedStdin,
    child: Mutex<Option<tokio::process::Child>>,
}

struct Http {
    client: reqwest::Client,
    url: String,
    headers: Vec<(String, String)>,
    session: Mutex<Option<String>>,
    negotiated: Mutex<Option<String>>,
}

enum Channel {
    Stdio(Stdio),
    Http(Http),
}

pub struct McpClient {
    channel: Channel,
    next_id: AtomicU64,
    pending: Pending,
    closed: Arc<AtomicBool>,
    stderr: Arc<Mutex<VecDeque<String>>>,
    tools_changed: Arc<AtomicBool>,
    /// `serverInfo` from `initialize`.
    pub server_info: Value,
    /// `instructions` from `initialize`, if any.
    pub instructions: Option<String>,
}

/// Turns `{{secret:NAME}}` into values; `Err` names what is missing.
pub type Expand = Arc<dyn Fn(&str) -> Result<String, String> + Send + Sync>;

impl McpClient {
    /// Starts (or reaches) the server and runs the handshake.
    pub async fn connect(
        config: &ServerConfig,
        expand: &Expand,
        roots: Vec<String>,
    ) -> Result<Self, McpError> {
        let pending: Pending = Arc::default();
        let closed = Arc::new(AtomicBool::new(false));
        let stderr: Arc<Mutex<VecDeque<String>>> = Arc::default();
        let tools_changed = Arc::new(AtomicBool::new(false));
        let channel = match config.transport {
            Transport::Stdio => {
                let mut env = Vec::new();
                for (key, value) in &config.env {
                    env.push((key.clone(), expand(value).map_err(McpError::new)?));
                }
                let mut command = command_for(&config.command, &config.args)?;
                command
                    .envs(env)
                    .stdin(std::process::Stdio::piped())
                    .stdout(std::process::Stdio::piped())
                    .stderr(std::process::Stdio::piped())
                    .kill_on_drop(true);
                if let Some(cwd) = config.cwd.as_deref().filter(|c| !c.trim().is_empty()) {
                    command.current_dir(cwd);
                }
                let mut child = command.spawn().map_err(|e| {
                    McpError::new(format!("não foi possível iniciar {}: {e}", config.command))
                })?;
                let stdin = child.stdin.take().expect("piped stdin");
                let stdout = child.stdout.take().expect("piped stdout");
                let err = child.stderr.take().expect("piped stderr");
                let log = stderr.clone();
                tokio::spawn(async move {
                    let mut lines = BufReader::new(err).lines();
                    while let Ok(Some(line)) = lines.next_line().await {
                        let mut log = log.lock();
                        if log.len() >= STDERR_LINES {
                            log.pop_front();
                        }
                        log.push_back(line);
                    }
                });
                let stdin: SharedStdin = Arc::new(tokio::sync::Mutex::new(stdin));
                let stdio = Stdio {
                    stdin: stdin.clone(),
                    child: Mutex::new(Some(child)),
                };
                // Replies resolve their request; the server's own requests
                // are answered through a channel to the writer.
                let (reply_tx, mut reply_rx) = tokio::sync::mpsc::unbounded_channel::<Value>();
                let reader_pending = pending.clone();
                let reader_closed = closed.clone();
                let changed = tools_changed.clone();
                let roots_for_reader = roots.clone();
                tokio::spawn(async move {
                    let mut lines = BufReader::new(stdout).lines();
                    while let Ok(Some(line)) = lines.next_line().await {
                        let Ok(message) = serde_json::from_str::<Value>(line.trim()) else {
                            continue;
                        };
                        for message in batch(message) {
                            if let Some(reply) =
                                route(message, &reader_pending, &changed, &roots_for_reader)
                            {
                                let _ = reply_tx.send(reply);
                            }
                        }
                    }
                    reader_closed.store(true, Ordering::SeqCst);
                    for (_, waiter) in reader_pending.lock().drain() {
                        let _ = waiter.send(Err(McpError::new("o servidor MCP terminou")));
                    }
                });
                // Answers to the server's own requests share stdin.
                tokio::spawn(async move {
                    while let Some(reply) = reply_rx.recv().await {
                        if write_line(&stdin, &reply).await.is_err() {
                            break;
                        }
                    }
                });
                let mut client = Self {
                    channel: Channel::Stdio(stdio),
                    next_id: AtomicU64::new(1),
                    pending,
                    closed,
                    stderr,
                    tools_changed,
                    server_info: Value::Null,
                    instructions: None,
                };
                client.handshake(roots).await?;
                return Ok(client);
            }
            Transport::Http => {
                let mut headers = Vec::new();
                for (key, value) in &config.headers {
                    headers.push((key.clone(), expand(value).map_err(McpError::new)?));
                }
                let client = reqwest::Client::builder()
                    .connect_timeout(Duration::from_secs(20))
                    .user_agent(concat!("Orchestrator/", env!("CARGO_PKG_VERSION")))
                    .build()
                    .map_err(|e| McpError::new(e.to_string()))?;
                Channel::Http(Http {
                    client,
                    url: config.url.clone(),
                    headers,
                    session: Mutex::new(None),
                    negotiated: Mutex::new(None),
                })
            }
        };
        let mut client = Self {
            channel,
            next_id: AtomicU64::new(1),
            pending,
            closed,
            stderr,
            tools_changed,
            server_info: Value::Null,
            instructions: None,
        };
        client.handshake(roots).await?;
        Ok(client)
    }

    async fn handshake(&mut self, roots: Vec<String>) -> Result<(), McpError> {
        let _ = roots;
        let result = self
            .request(
                "initialize",
                json!({
                    "protocolVersion": PROTOCOL_VERSION,
                    "capabilities": {"roots": {"listChanged": false}},
                    "clientInfo": {"name": "Orchestrator", "version": env!("CARGO_PKG_VERSION")}
                }),
                INIT_TIMEOUT,
                &CancellationToken::new(),
            )
            .await?;
        if let Channel::Http(http) = &self.channel {
            *http.negotiated.lock() = result
                .get("protocolVersion")
                .and_then(Value::as_str)
                .map(str::to_owned);
        }
        self.server_info = result.get("serverInfo").cloned().unwrap_or(Value::Null);
        self.instructions = result
            .get("instructions")
            .and_then(Value::as_str)
            .map(str::to_owned);
        self.notify("notifications/initialized", json!({})).await
    }

    /// Every tool, following `nextCursor`.
    pub async fn list_tools(&self) -> Result<Vec<RemoteTool>, McpError> {
        let mut tools = Vec::new();
        let mut cursor: Option<String> = None;
        for _ in 0..50 {
            let params = match &cursor {
                Some(c) => json!({"cursor": c}),
                None => json!({}),
            };
            let result = self
                .request(
                    "tools/list",
                    params,
                    LIST_TIMEOUT,
                    &CancellationToken::new(),
                )
                .await?;
            for tool in result
                .get("tools")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default()
            {
                let Some(name) = tool.get("name").and_then(Value::as_str) else {
                    continue;
                };
                tools.push(RemoteTool {
                    name: name.to_owned(),
                    title: tool.get("title").and_then(Value::as_str).map(str::to_owned),
                    description: tool
                        .get("description")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned(),
                    input_schema: tool
                        .get("inputSchema")
                        .cloned()
                        .filter(Value::is_object)
                        .unwrap_or_else(|| json!({"type": "object"})),
                    read_only: tool
                        .pointer("/annotations/readOnlyHint")
                        .and_then(Value::as_bool)
                        .unwrap_or(false),
                });
            }
            cursor = result
                .get("nextCursor")
                .and_then(Value::as_str)
                .filter(|c| !c.is_empty())
                .map(str::to_owned);
            if cursor.is_none() {
                break;
            }
        }
        self.tools_changed.store(false, Ordering::SeqCst);
        Ok(tools)
    }

    pub async fn call_tool(
        &self,
        name: &str,
        arguments: Value,
        timeout: Duration,
        cancel: &CancellationToken,
    ) -> Result<CallOutcome, McpError> {
        let arguments = if arguments.is_null() {
            json!({})
        } else {
            arguments
        };
        let result = self
            .request(
                "tools/call",
                json!({"name": name, "arguments": arguments}),
                timeout,
                cancel,
            )
            .await?;
        let mut text = Vec::new();
        let mut other = Vec::new();
        for part in result
            .get("content")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
        {
            match part.get("type").and_then(Value::as_str) {
                Some("text") => {
                    if let Some(t) = part.get("text").and_then(Value::as_str) {
                        text.push(t.to_owned());
                    }
                }
                Some("resource") => {
                    if let Some(t) = part.pointer("/resource/text").and_then(Value::as_str) {
                        text.push(t.to_owned());
                    } else {
                        other.push(describe(&part));
                    }
                }
                _ => other.push(describe(&part)),
            }
        }
        Ok(CallOutcome {
            is_error: result
                .get("isError")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            text: text.join("\n"),
            structured: result.get("structuredContent").cloned(),
            other,
        })
    }

    /// The server changed its tools since the last `list_tools`.
    pub fn tools_changed(&self) -> bool {
        self.tools_changed.load(Ordering::SeqCst)
    }

    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::SeqCst)
    }

    /// The last lines the server wrote to stderr.
    pub fn stderr(&self) -> Vec<String> {
        self.stderr.lock().iter().cloned().collect()
    }

    /// Stops the server (stdio) and fails what is waiting.
    pub fn close(&self) {
        self.closed.store(true, Ordering::SeqCst);
        if let Channel::Stdio(stdio) = &self.channel {
            if let Some(mut child) = stdio.child.lock().take() {
                let _ = child.start_kill();
            }
        }
        for (_, waiter) in self.pending.lock().drain() {
            let _ = waiter.send(Err(McpError::new("conexão MCP encerrada")));
        }
    }

    async fn notify(&self, method: &str, params: Value) -> Result<(), McpError> {
        let message = json!({"jsonrpc": "2.0", "method": method, "params": params});
        match &self.channel {
            Channel::Stdio(_) => self.write_stdio(message).await,
            Channel::Http(http) => http.post(message, None, &self.pending).await.map(|_| ()),
        }
    }

    async fn write_stdio(&self, message: Value) -> Result<(), McpError> {
        let Channel::Stdio(stdio) = &self.channel else {
            unreachable!()
        };
        if self.is_closed() {
            return Err(McpError::new("o servidor MCP terminou"));
        }
        write_line(&stdio.stdin, &message)
            .await
            .map_err(|e| McpError::new(format!("o servidor MCP não recebe mais: {e}")))
    }

    async fn request(
        &self,
        method: &str,
        params: Value,
        timeout: Duration,
        cancel: &CancellationToken,
    ) -> Result<Value, McpError> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let message = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        let (tx, rx) = oneshot::channel();
        self.pending.lock().insert(id, tx);
        let sent = match &self.channel {
            Channel::Stdio(_) => self.write_stdio(message).await,
            Channel::Http(http) => {
                let pending = self.pending.clone();
                let post = http.post(message, Some(id), &pending);
                tokio::select! {
                    result = post => result.map(|_| ()),
                    () = cancel.cancelled() => Err(McpError { message: "cancelado".into(), cancelled: true }),
                    () = tokio::time::sleep(timeout) => Err(McpError::new(format!("{method}: sem resposta em {} s", timeout.as_secs()))),
                }
            }
        };
        if let Err(err) = sent {
            self.pending.lock().remove(&id);
            if err.cancelled {
                let _ = self
                    .notify(
                        "notifications/cancelled",
                        json!({"requestId": id, "reason": "cancelled by the user"}),
                    )
                    .await;
            }
            return Err(err);
        }
        let outcome = tokio::select! {
            reply = rx => reply.unwrap_or_else(|_| Err(McpError::new("conexão MCP encerrada"))),
            () = cancel.cancelled() => Err(McpError { message: "cancelado".into(), cancelled: true }),
            () = tokio::time::sleep(timeout) => Err(McpError::new(format!("{method}: sem resposta em {} s", timeout.as_secs()))),
        };
        if outcome.is_err() {
            self.pending.lock().remove(&id);
        }
        if matches!(&outcome, Err(e) if e.cancelled || e.message.contains("sem resposta")) {
            let _ = self
                .notify(
                    "notifications/cancelled",
                    json!({"requestId": id, "reason": "cancelled or timed out"}),
                )
                .await;
        }
        outcome
    }
}

impl Drop for McpClient {
    fn drop(&mut self) {
        self.close();
    }
}

/// One JSON-RPC message, one line.
async fn write_line(stdin: &SharedStdin, message: &Value) -> std::io::Result<()> {
    let mut line = message.to_string();
    line.push('\n');
    let mut guard = stdin.lock().await;
    guard.write_all(line.as_bytes()).await?;
    guard.flush().await
}

impl Http {
    /// Posts a message; a reply that comes in the response (JSON or an SSE
    /// stream) is routed to its waiter.
    async fn post(
        &self,
        message: Value,
        id: Option<u64>,
        pending: &Pending,
    ) -> Result<(), McpError> {
        let mut request = self
            .client
            .post(&self.url)
            .header("Accept", "application/json, text/event-stream")
            .header("Content-Type", "application/json")
            .json(&message);
        for (key, value) in &self.headers {
            request = request.header(key, value);
        }
        if let Some(session) = self.session.lock().clone() {
            request = request.header("Mcp-Session-Id", session);
        }
        if let Some(version) = self.negotiated.lock().clone() {
            request = request.header("MCP-Protocol-Version", version);
        }
        let response = request.send().await.map_err(|e| {
            McpError::new(format!(
                "não foi possível falar com {}: {}",
                self.url,
                e.without_url()
            ))
        })?;
        if let Some(session) = response
            .headers()
            .get("mcp-session-id")
            .and_then(|v| v.to_str().ok())
        {
            *self.session.lock() = Some(session.to_owned());
        }
        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            let hint = if status.as_u16() == 401 || status.as_u16() == 403 {
                " — confira o token nos cabeçalhos (pode usar {{secret:NOME}})"
            } else {
                ""
            };
            return Err(McpError::new(format!(
                "HTTP {}{hint}: {}",
                status.as_u16(),
                body.chars().take(300).collect::<String>()
            )));
        }
        if id.is_none() {
            return Ok(());
        }
        let is_sse = response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .is_some_and(|t| t.starts_with("text/event-stream"));
        let changed = Arc::new(AtomicBool::new(false));
        if is_sse {
            let mut stream = response.bytes_stream();
            let mut buffer = String::new();
            let mut data = Vec::new();
            while let Some(chunk) = stream.next().await {
                let chunk = chunk.map_err(|e| McpError::new(e.to_string()))?;
                buffer.push_str(&String::from_utf8_lossy(&chunk));
                while let Some(end) = buffer.find('\n') {
                    let line = buffer[..end].trim_end_matches('\r').to_owned();
                    buffer.drain(..=end);
                    if line.is_empty() {
                        if !data.is_empty() {
                            if let Ok(value) = serde_json::from_str::<Value>(&data.join("\n")) {
                                for message in batch(value) {
                                    route(message, pending, &changed, &[]);
                                }
                            }
                            data.clear();
                        }
                        if id.is_some_and(|id| !pending.lock().contains_key(&id)) {
                            return Ok(());
                        }
                    } else if let Some(value) = line.strip_prefix("data:") {
                        data.push(value.trim_start().to_owned());
                    }
                }
            }
            Ok(())
        } else {
            let text = response
                .text()
                .await
                .map_err(|e| McpError::new(e.to_string()))?;
            if text.trim().is_empty() {
                return Ok(());
            }
            let value: Value = serde_json::from_str(&text)
                .map_err(|e| McpError::new(format!("resposta MCP inválida: {e}")))?;
            for message in batch(value) {
                route(message, pending, &changed, &[]);
            }
            Ok(())
        }
    }
}

/// A JSON-RPC batch, or the single message.
fn batch(value: Value) -> Vec<Value> {
    match value {
        Value::Array(items) => items,
        other => vec![other],
    }
}

/// Delivers a reply to its waiter; returns the answer to a server request.
fn route(
    message: Value,
    pending: &Pending,
    changed: &AtomicBool,
    roots: &[String],
) -> Option<Value> {
    let method = message.get("method").and_then(Value::as_str);
    let id = message.get("id").cloned();
    match (method, id) {
        (None, Some(id)) => {
            let key = id.as_u64()?;
            if let Some(waiter) = pending.lock().remove(&key) {
                let outcome = match message.get("error") {
                    Some(error) => Err(McpError::new(format!(
                        "{} (código {})",
                        error
                            .get("message")
                            .and_then(Value::as_str)
                            .unwrap_or("erro do servidor MCP"),
                        error.get("code").and_then(Value::as_i64).unwrap_or(0)
                    ))),
                    None => Ok(message.get("result").cloned().unwrap_or(Value::Null)),
                };
                let _ = waiter.send(outcome);
            }
            None
        }
        (Some(method), Some(id)) => Some(match method {
            "ping" => json!({"jsonrpc": "2.0", "id": id, "result": {}}),
            "roots/list" => json!({"jsonrpc": "2.0", "id": id, "result": {
                "roots": roots.iter().map(|r| json!({"uri": file_uri(r), "name": r})).collect::<Vec<_>>()
            }}),
            _ => json!({"jsonrpc": "2.0", "id": id, "error": {
                "code": -32601, "message": format!("the Orchestrator does not support {method}")
            }}),
        }),
        (Some("notifications/tools/list_changed"), None) => {
            changed.store(true, Ordering::SeqCst);
            None
        }
        _ => None,
    }
}

fn file_uri(path: &str) -> String {
    let path = path.replace('\\', "/");
    if path.starts_with('/') {
        format!("file://{path}")
    } else {
        format!("file:///{path}")
    }
}

/// A non-text content part, without its (large) data.
fn describe(part: &Value) -> Value {
    let mut out = json!({"type": part.get("type").cloned().unwrap_or(Value::Null)});
    for key in ["mimeType", "uri", "name"] {
        if let Some(v) = part
            .get(key)
            .or_else(|| part.pointer(&format!("/resource/{key}")))
        {
            out[key] = v.clone();
        }
    }
    if let Some(data) = part.get("data").and_then(Value::as_str) {
        out["bytes"] = json!(data.len() * 3 / 4);
    }
    out
}

/// The command to start a server. On Windows, `npx`, `uvx` and other
/// `.cmd` scripts go through `cmd /C`, and no console window opens.
fn command_for(program: &str, args: &[String]) -> Result<tokio::process::Command, McpError> {
    let resolved = which::which(program).unwrap_or_else(|_| program.into());
    let is_script = resolved
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("cmd") || e.eq_ignore_ascii_case("bat"));
    let mut command = if cfg!(windows) && is_script {
        let mut c = tokio::process::Command::new("cmd");
        c.arg("/C").arg(&resolved);
        c
    } else {
        tokio::process::Command::new(&resolved)
    };
    command.args(args);
    #[cfg(windows)]
    {
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    Ok(command)
}
