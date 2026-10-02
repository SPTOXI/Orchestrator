//! The Orchestrator's tools over MCP (ADR-0021), for the AIs that run as
//! CLI programs on the user's subscription (Claude Code, Codex, Gemini
//! CLI): they get one URL per turn, `http://127.0.0.1:<port>/t/<token>`,
//! and every tool they call there goes through that turn's
//! [`TurnContext`] — the autonomy gate, the locks, the history — exactly
//! as an API model's would.
//!
//! Streamable HTTP, answering with JSON (no server-initiated stream). Only
//! loopback, and only with the turn's random token.

use orchestrator_providers::TurnContext;
use parking_lot::Mutex;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

/// Largest request body read (bytes).
const MAX_BODY: usize = 16 * 1024 * 1024;

/// The tool name a CLI sees: `.` is not allowed everywhere, `__` is.
pub fn wire_name(name: &str) -> String {
    name.replace('.', "__")
}

fn tool_name(wire: &str) -> String {
    wire.replace("__", ".")
}

/// The local MCP endpoint. Cheap to clone.
#[derive(Clone)]
pub struct ToolServer {
    addr: SocketAddr,
    routes: Arc<Mutex<HashMap<String, TurnContext>>>,
}

/// A turn's URL; dropping it closes the URL.
pub struct Route {
    pub url: String,
    token: String,
    routes: Arc<Mutex<HashMap<String, TurnContext>>>,
}

impl Drop for Route {
    fn drop(&mut self) {
        self.routes.lock().remove(&self.token);
    }
}

impl ToolServer {
    /// Listens on a free loopback port.
    pub async fn start() -> std::io::Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let addr = listener.local_addr()?;
        let routes: Arc<Mutex<HashMap<String, TurnContext>>> = Arc::default();
        let server = Self { addr, routes };
        let accepting = server.clone();
        tokio::spawn(async move {
            loop {
                let Ok((socket, _)) = listener.accept().await else {
                    return;
                };
                let this = accepting.clone();
                tokio::spawn(async move {
                    let _ = this.serve(socket).await;
                });
            }
        });
        Ok(server)
    }

    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// A URL for one turn's tools.
    pub fn route(&self, ctx: TurnContext) -> Route {
        // Two v7 ids: 148 random bits.
        let token = format!(
            "{}{}",
            uuid::Uuid::now_v7().simple(),
            uuid::Uuid::now_v7().simple()
        );
        self.routes.lock().insert(token.clone(), ctx);
        Route {
            url: format!("http://{}/t/{token}", self.addr),
            token,
            routes: self.routes.clone(),
        }
    }

    async fn serve(&self, mut socket: TcpStream) -> std::io::Result<()> {
        // Keep-alive: several requests may come on one connection.
        loop {
            let Some((method, path, body)) = read_request(&mut socket).await? else {
                return Ok(());
            };
            let ctx = path
                .strip_prefix("/t/")
                .map(|t| t.trim_end_matches('/'))
                .and_then(|token| self.routes.lock().get(token).cloned());
            let (status, payload) = match (method.as_str(), ctx) {
                (_, None) => (404, json!({"error": "unknown or finished turn"}).to_string()),
                ("POST", Some(ctx)) => match serde_json::from_slice::<Value>(&body) {
                    Ok(message) => match handle(&ctx, message).await {
                        Some(reply) => (200, reply.to_string()),
                        None => (202, String::new()),
                    },
                    Err(err) => (
                        400,
                        json!({"jsonrpc": "2.0", "id": null, "error": {"code": -32700, "message": err.to_string()}})
                            .to_string(),
                    ),
                },
                ("DELETE", Some(_)) => (200, String::new()),
                // No server-initiated stream.
                (_, Some(_)) => (405, String::new()),
            };
            let reason = match status {
                200 => "OK",
                202 => "Accepted",
                400 => "Bad Request",
                404 => "Not Found",
                _ => "Method Not Allowed",
            };
            let head = format!(
                "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nMcp-Session-Id: orchestrator\r\n\r\n",
                payload.len()
            );
            socket.write_all(head.as_bytes()).await?;
            socket.write_all(payload.as_bytes()).await?;
            socket.flush().await?;
        }
    }
}

/// `(method, path, body)` of the next request; `None` when the client
/// closed the connection.
async fn read_request(
    socket: &mut TcpStream,
) -> std::io::Result<Option<(String, String, Vec<u8>)>> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 8192];
    let head_end = loop {
        if let Some(at) = find(&buf, b"\r\n\r\n") {
            break at;
        }
        let n = socket.read(&mut chunk).await?;
        if n == 0 {
            return Ok(None);
        }
        buf.extend_from_slice(&chunk[..n]);
        if buf.len() > 64 * 1024 && find(&buf, b"\r\n\r\n").is_none() {
            return Ok(None);
        }
    };
    let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
    let mut lines = head.lines();
    let mut first = lines.next().unwrap_or_default().split_whitespace();
    let method = first.next().unwrap_or_default().to_owned();
    let path = first.next().unwrap_or_default().to_owned();
    let length = lines
        .filter_map(|l| l.split_once(':'))
        .find(|(k, _)| k.trim().eq_ignore_ascii_case("content-length"))
        .and_then(|(_, v)| v.trim().parse::<usize>().ok())
        .unwrap_or(0)
        .min(MAX_BODY);
    let mut body = buf[head_end + 4..].to_vec();
    while body.len() < length {
        let n = socket.read(&mut chunk).await?;
        if n == 0 {
            break;
        }
        body.extend_from_slice(&chunk[..n]);
    }
    body.truncate(length);
    Ok(Some((method, path, body)))
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

/// Answers one JSON-RPC message (or a batch); `None` for notifications.
async fn handle(ctx: &TurnContext, message: Value) -> Option<Value> {
    if let Value::Array(items) = message {
        let mut replies = Vec::new();
        for item in items {
            if let Some(reply) = Box::pin(handle(ctx, item)).await {
                replies.push(reply);
            }
        }
        return (!replies.is_empty()).then_some(Value::Array(replies));
    }
    let id = message.get("id").cloned()?;
    let method = message
        .get("method")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let params = message.get("params").cloned().unwrap_or(Value::Null);
    let result = match method {
        "initialize" => Ok(json!({
            "protocolVersion": params.get("protocolVersion").cloned().unwrap_or(json!(crate::PROTOCOL_VERSION)),
            "capabilities": {"tools": {"listChanged": false}},
            "serverInfo": {"name": "orchestrator", "version": env!("CARGO_PKG_VERSION")},
            "instructions": "The Orchestrator's tools: files, commands, terminals, git, GitHub, the internet, \
                             the project memory, skills and MCP servers. Every action goes through them; the \
                             Orchestrator decides what needs the user's authorization."
        })),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({
            "tools": ctx.tools().iter().map(|t| json!({
                "name": wire_name(&t.name),
                "description": t.description,
                "inputSchema": t.parameters,
                "annotations": {"readOnlyHint": t.read_only},
            })).collect::<Vec<_>>()
        })),
        "tools/call" => {
            let name = params
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let args = params.get("arguments").cloned().unwrap_or(json!({}));
            let result = ctx.call_tool(&tool_name(name), args).await;
            let (text, is_error) = match (&result.ok, &result.error) {
                (true, _) => (
                    match &result.output {
                        Value::String(s) => s.clone(),
                        other => serde_json::to_string_pretty(other).unwrap_or_default(),
                    },
                    false,
                ),
                (false, Some(err)) => (
                    format!(
                        "{}: {}",
                        serde_json::to_string(&err.kind)
                            .unwrap_or_default()
                            .trim_matches('"'),
                        err.message
                    ),
                    true,
                ),
                (false, None) => ("the tool failed".to_owned(), true),
            };
            Ok(json!({"content": [{"type": "text", "text": text}], "isError": is_error}))
        }
        "resources/list" => Ok(json!({"resources": []})),
        "prompts/list" => Ok(json!({"prompts": []})),
        other => Err(json!({"code": -32601, "message": format!("method not found: {other}")})),
    };
    Some(match result {
        Ok(result) => json!({"jsonrpc": "2.0", "id": id, "result": result}),
        Err(error) => json!({"jsonrpc": "2.0", "id": id, "error": error}),
    })
}
