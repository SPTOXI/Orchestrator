//! MCP servers as tools (ADR-0021): a real stdio server (the test binary)
//! and a Streamable HTTP one, through the manager and `McpTools`.

use orchestrator_core::{CallOrigin, EventKind, MemorySink, ToolCall, ToolDefinition, ToolResult};
use orchestrator_mcp::{
    config::import, McpManager, McpTools, ServerConfig, ServerStatus, Transport,
};
use orchestrator_providers::ToolExecutor;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;

struct NoTools;

#[async_trait::async_trait]
impl ToolExecutor for NoTools {
    fn tools(&self) -> Vec<ToolDefinition> {
        Vec::new()
    }
    async fn execute(&self, call: ToolCall) -> ToolResult {
        panic!("unexpected call to {}", call.tool)
    }
}

const TOKEN: &str = "tok-secreto-123";

fn manager(dir: &std::path::Path) -> (McpManager, Arc<MemorySink>) {
    let sink = Arc::new(MemorySink::new());
    let (manager, warning) = McpManager::open(
        Some(&dir.join("mcp.json")),
        sink.clone(),
        Arc::new(|text: &str| Ok(text.replace("{{secret:TOKEN}}", TOKEN))),
        Arc::new(|text: &str| text.replace(TOKEN, "***")),
    );
    assert!(warning.is_none());
    (manager, sink)
}

fn stdio_server() -> ServerConfig {
    ServerConfig {
        id: "teste".into(),
        name: "Servidor de teste".into(),
        transport: Transport::Stdio,
        command: env!("CARGO_BIN_EXE_mcp-test-server").into(),
        args: Vec::new(),
        env: BTreeMap::from([("TEST_TOKEN".to_owned(), "{{secret:TOKEN}}".to_owned())]),
        cwd: None,
        url: String::new(),
        headers: BTreeMap::new(),
        enabled: true,
        timeout_secs: Some(2),
        disabled_tools: vec!["add".into()],
    }
}

async fn call(tools: &McpTools, name: &str, args: Value) -> ToolResult {
    tools
        .execute(ToolCall::new(name, args, CallOrigin::User))
        .await
}

#[tokio::test(flavor = "multi_thread")]
async fn a_stdio_server_becomes_tools() {
    let dir = tempfile::tempdir().unwrap();
    let (mcp, sink) = manager(dir.path());
    mcp.save(stdio_server(), None).unwrap();
    mcp.start_all().await;

    let view = &mcp.view()[0];
    assert_eq!(view.status, ServerStatus::Ready, "{:?}", view.error);
    assert_eq!(view.server_name.as_deref(), Some("test-server"));
    assert!(view.log.iter().any(|l| l.contains("starting")) || view.log.is_empty());
    let names: Vec<&str> = view.tools.iter().map(|t| t.name.as_str()).collect();
    assert!(names.contains(&"mcp.teste.echo"));
    assert!(
        names.contains(&"mcp.teste.weird_name_with_dots"),
        "no `__` or dots inside the tool part: {names:?}"
    );

    let tools = McpTools::new(Arc::new(NoTools), mcp.clone());
    let defs = tools.tools();
    let echo = defs.iter().find(|d| d.name == "mcp.teste.echo").unwrap();
    assert!(echo.read_only, "readOnlyHint");
    assert!(echo.description.starts_with("[MCP: Servidor de teste]"));
    assert!(
        !defs
            .iter()
            .find(|d| d.name == "mcp.teste.fail")
            .unwrap()
            .read_only
    );
    assert!(
        !defs.iter().any(|d| d.name == "mcp.teste.add"),
        "turned off by the user"
    );

    // A call that makes the server ping us first.
    let result = call(&tools, "mcp.teste.echo", json!({"text": "olá"})).await;
    assert!(result.ok, "{:?}", result.error);
    assert_eq!(result.output["text"], "echo: olá");

    // isError comes back as a failure with the server's text.
    let result = call(&tools, "mcp.teste.fail", json!({})).await;
    assert!(!result.ok);
    assert_eq!(result.error.unwrap().message, "it broke");

    // The secret reached the server's environment, masked on the way back.
    let result = call(&tools, "mcp.teste.env", json!({})).await;
    assert_eq!(result.output["text"], "***");
    let history = serde_json::to_string(&sink.audit_events()).unwrap();
    assert!(!history.contains(TOKEN));

    // A call longer than the timeout fails, and the server keeps working.
    let result = call(&tools, "mcp.teste.slow", json!({})).await;
    assert!(result
        .error
        .unwrap()
        .message
        .contains("sem resposta em 2 s"));

    // Cancelling a call.
    let cancel = CancellationToken::new();
    let stop = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(200)).await;
        stop.cancel();
    });
    let result = tools
        .execute_with(
            ToolCall::new("mcp.teste.slow", json!({}), CallOrigin::User),
            cancel,
        )
        .await;
    assert_eq!(
        result.error.unwrap().kind,
        orchestrator_core::ToolErrorKind::Cancelled
    );

    // A tool that appears later.
    assert!(call(&tools, "mcp.teste.grow", json!({})).await.ok);
    let mut grown = false;
    for _ in 0..50 {
        if tools.tools().iter().any(|d| d.name == "mcp.teste.extra") {
            grown = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(grown, "tools/list_changed refreshes the list");

    // Every call is in the history.
    let calls = sink
        .audit_events()
        .into_iter()
        .filter(|e| e.kind == EventKind::ToolCalled)
        .count();
    assert!(calls >= 5);

    // Unknown, disabled, deleted.
    assert!(!call(&tools, "mcp.teste.nope", json!({})).await.ok);
    let mut off = stdio_server();
    off.enabled = false;
    mcp.save(off, None).unwrap();
    mcp.restart("teste").await;
    assert_eq!(mcp.view()[0].status, ServerStatus::Disabled);
    assert!(tools.tools().is_empty());
    mcp.delete("teste").unwrap();
    assert!(mcp.view().is_empty());

    // The file keeps what was saved.
    let (again, _) = manager(dir.path());
    assert!(again.servers().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_server_that_cannot_start_says_why() {
    let dir = tempfile::tempdir().unwrap();
    let (mcp, _) = manager(dir.path());
    let mut broken = stdio_server();
    broken.command = "programa-que-nao-existe-xyz".into();
    mcp.save(broken, None).unwrap();
    mcp.start_all().await;
    let view = &mcp.view()[0];
    assert_eq!(view.status, ServerStatus::Failed);
    assert!(view
        .error
        .as_deref()
        .unwrap()
        .contains("não foi possível iniciar"));

    // Importing Claude Desktop's format; repeated ids get a suffix.
    let added = mcp
        .import(r#"{"mcpServers": {"teste": {"command": "x"}, "Docs": {"type": "http", "url": "https://example.com/mcp"}}}"#)
        .unwrap();
    assert_eq!(added, vec!["docs".to_owned(), "teste-2".to_owned()]);
    assert!(import("{}").is_err());
}

/// A Streamable HTTP MCP server: JSON for most replies, SSE for calls,
/// and a session id it insists on.
async fn http_server() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else {
                return;
            };
            tokio::spawn(async move {
                let mut buf = Vec::new();
                let mut chunk = [0u8; 4096];
                let (head, body) = loop {
                    let n = socket.read(&mut chunk).await.unwrap_or(0);
                    if n == 0 {
                        return;
                    }
                    buf.extend_from_slice(&chunk[..n]);
                    let text = String::from_utf8_lossy(&buf).to_string();
                    if let Some(end) = text.find("\r\n\r\n") {
                        let head = text[..end].to_lowercase();
                        let len: usize = head
                            .lines()
                            .find_map(|l| l.strip_prefix("content-length:"))
                            .map(|v| v.trim().parse().unwrap())
                            .unwrap_or(0);
                        if buf.len() >= end + 4 + len {
                            break (head, text[end + 4..end + 4 + len].to_owned());
                        }
                    }
                };
                let message: Value = serde_json::from_str(&body).unwrap();
                let method = message["method"].as_str().unwrap_or_default();
                let authorized = head.contains(&format!("authorization: bearer {TOKEN}"));
                let (status, kind, extra, payload) = if !authorized {
                    (
                        401,
                        "application/json",
                        String::new(),
                        json!({"error": "no token"}).to_string(),
                    )
                } else if method == "initialize" {
                    (
                        200,
                        "application/json",
                        "Mcp-Session-Id: sess-1\r\n".to_owned(),
                        json!({"jsonrpc": "2.0", "id": message["id"], "result": {
                            "protocolVersion": "2025-06-18", "capabilities": {"tools": {}},
                            "serverInfo": {"name": "http-server", "version": "0.1"}}})
                        .to_string(),
                    )
                } else if !head.contains("mcp-session-id: sess-1") {
                    (400, "application/json", String::new(), "{}".to_owned())
                } else if message.get("id").is_none() {
                    (202, "application/json", String::new(), String::new())
                } else if method == "tools/list" {
                    (
                        200,
                        "application/json",
                        String::new(),
                        json!({"jsonrpc": "2.0", "id": message["id"], "result": {"tools": [
                            {"name": "lookup", "description": "Looks up", "inputSchema": {"type": "object"},
                             "annotations": {"readOnlyHint": true}}]}})
                        .to_string(),
                    )
                } else {
                    let reply = json!({"jsonrpc": "2.0", "id": message["id"], "result": {
                        "content": [{"type": "text", "text": format!("achei {}", message["params"]["arguments"]["q"])}]}});
                    (
                        200,
                        "text/event-stream",
                        String::new(),
                        format!(
                            "event: message\ndata: {}\n\ndata: {}\n\n",
                            json!({"jsonrpc": "2.0", "method": "notifications/progress", "params": {}}),
                            reply
                        ),
                    )
                };
                let response = format!(
                    "HTTP/1.1 {status} X\r\nContent-Type: {kind}\r\n{extra}Content-Length: {}\r\nConnection: close\r\n\r\n{payload}",
                    payload.len()
                );
                let _ = socket.write_all(response.as_bytes()).await;
                let _ = socket.shutdown().await;
            });
        }
    });
    format!("http://{addr}/mcp")
}

#[tokio::test(flavor = "multi_thread")]
async fn a_streamable_http_server_with_a_secret_header() {
    let url = http_server().await;
    let dir = tempfile::tempdir().unwrap();
    let (mcp, _) = manager(dir.path());
    let server = ServerConfig {
        id: "remoto".into(),
        name: "Remoto".into(),
        transport: Transport::Http,
        command: String::new(),
        args: Vec::new(),
        env: BTreeMap::new(),
        cwd: None,
        url: url.clone(),
        headers: BTreeMap::from([(
            "Authorization".to_owned(),
            "Bearer {{secret:TOKEN}}".to_owned(),
        )]),
        enabled: true,
        timeout_secs: None,
        disabled_tools: Vec::new(),
    };
    mcp.save(server.clone(), None).unwrap();
    mcp.start_all().await;
    let view = &mcp.view()[0];
    assert_eq!(view.status, ServerStatus::Ready, "{:?}", view.error);
    let tools = McpTools::new(Arc::new(NoTools), mcp.clone());
    let result = call(&tools, "mcp.remoto.lookup", json!({"q": "x"})).await;
    assert!(result.ok, "{:?}", result.error);
    assert_eq!(result.output["text"], "achei \"x\"");

    // Without the token: refused, with a hint and without the secret.
    let mut bare = server;
    bare.headers.clear();
    mcp.save(bare, None).unwrap();
    mcp.restart("remoto").await;
    let view = &mcp.view()[0];
    assert_eq!(view.status, ServerStatus::Failed);
    assert!(view.error.as_deref().unwrap().contains("HTTP 401"));
}
