//! `web.fetch`, `http.request`, `secrets.list` and secrets in
//! `shell.execute` through `ToolRuntime::invoke` (ADR-0020), against a
//! local HTTP server.

use orchestrator_core::{CallOrigin, MemorySink, ToolCall, ToolErrorKind, ToolResult};
use orchestrator_git::github::Secret;
use orchestrator_runtime::web::Secrets;
use orchestrator_runtime::{RuntimeConfig, ToolRuntime};
use parking_lot::Mutex;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

const KEY: &str = "sk-segredo-muito-secreto";

#[derive(Debug, Clone)]
struct Recorded {
    method: String,
    path: String,
    headers: HashMap<String, String>,
    body: String,
}

async fn read_request(socket: &mut tokio::net::TcpStream) -> Option<Recorded> {
    let mut buffer = Vec::new();
    let mut chunk = [0u8; 8192];
    let header_end = loop {
        let n = socket.read(&mut chunk).await.ok()?;
        if n == 0 {
            return None;
        }
        buffer.extend_from_slice(&chunk[..n]);
        if let Some(pos) = buffer.windows(4).position(|w| w == b"\r\n\r\n") {
            break pos + 4;
        }
    };
    let head = String::from_utf8_lossy(&buffer[..header_end]).into_owned();
    let mut lines = head.split("\r\n");
    let mut first = lines.next()?.split_whitespace();
    let method = first.next()?.to_owned();
    let path = first.next()?.to_owned();
    let mut headers = HashMap::new();
    for line in lines {
        if let Some((name, value)) = line.split_once(':') {
            headers.insert(name.trim().to_ascii_lowercase(), value.trim().to_owned());
        }
    }
    let length: usize = headers
        .get("content-length")
        .and_then(|l| l.parse().ok())
        .unwrap_or(0);
    let mut body = buffer[header_end..].to_vec();
    while body.len() < length {
        let n = socket.read(&mut chunk).await.ok()?;
        if n == 0 {
            break;
        }
        body.extend_from_slice(&chunk[..n]);
    }
    Some(Recorded {
        method,
        path,
        headers,
        body: String::from_utf8_lossy(&body).into_owned(),
    })
}

/// (status, content type, extra headers, body) for a request.
fn route(r: &Recorded) -> (u16, &'static str, Vec<(&'static str, String)>, String) {
    match (r.method.as_str(), r.path.as_str()) {
        ("GET", "/pagina") => (
            200,
            "text/html; charset=utf-8",
            vec![],
            "<html><head><title>Notícias</title><script>x()</script></head>\
             <body><h1>Manchete</h1><p>Texto &amp; mais.</p></body></html>"
                .into(),
        ),
        ("GET", "/antiga") => (
            302,
            "text/plain",
            vec![("location", "/pagina".into())],
            String::new(),
        ),
        ("GET", path) if path.starts_with("/eco") => (
            200,
            "application/json",
            vec![(
                "x-eco",
                r.headers.get("authorization").cloned().unwrap_or_default(),
            )],
            json!({
                "authorization": r.headers.get("authorization"),
                "query": path,
            })
            .to_string(),
        ),
        ("POST", "/itens") => (
            201,
            "application/json",
            vec![],
            json!({"criado": serde_json::from_str::<Value>(&r.body).unwrap_or(Value::Null),
                   "tipo": r.headers.get("content-type")})
            .to_string(),
        ),
        ("DELETE", "/itens/7") => (204, "text/plain", vec![], String::new()),
        ("GET", "/grande") => (200, "text/plain", vec![], "x".repeat(3000)),
        ("GET", "/imagem") => (200, "image/png", vec![], "\u{89}PNG....".into()),
        _ => (
            404,
            "application/json",
            vec![],
            json!({"erro": "não existe"}).to_string(),
        ),
    }
}

async fn server() -> (String, Arc<Mutex<Vec<Recorded>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let log: Arc<Mutex<Vec<Recorded>>> = Arc::default();
    let requests = log.clone();
    tokio::spawn(async move {
        while let Ok((mut socket, _)) = listener.accept().await {
            let log = log.clone();
            tokio::spawn(async move {
                let Some(r) = read_request(&mut socket).await else {
                    return;
                };
                log.lock().push(r.clone());
                let (status, kind, extra, body) = route(&r);
                let extra: String = extra.iter().map(|(n, v)| format!("{n}: {v}\r\n")).collect();
                let head = format!(
                    "HTTP/1.1 {status} X\r\nContent-Type: {kind}\r\nContent-Length: {}\r\n{extra}Connection: close\r\n\r\n",
                    body.len()
                );
                let _ = socket.write_all(head.as_bytes()).await;
                let _ = socket.write_all(body.as_bytes()).await;
                let _ = socket.shutdown().await;
            });
        }
    });
    (url, requests)
}

fn runtime() -> (ToolRuntime, Arc<MemorySink>, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let sink = Arc::new(MemorySink::new());
    let runtime = ToolRuntime::new(
        RuntimeConfig {
            base_dir: dir.path().to_path_buf(),
        },
        sink.clone(),
    );
    let mut secrets = Secrets::new();
    secrets.insert("MINHA_API".into(), Secret::new(KEY).unwrap());
    runtime.set_secrets(secrets);
    (runtime, sink, dir)
}

async fn call(runtime: &ToolRuntime, tool: &str, args: Value) -> ToolResult {
    runtime
        .invoke(ToolCall::new(tool, args, CallOrigin::User))
        .await
}

async fn ok(runtime: &ToolRuntime, tool: &str, args: Value) -> Value {
    let result = call(runtime, tool, args).await;
    assert!(result.ok, "{tool} failed: {:?}", result.error);
    result.output
}

#[tokio::test(flavor = "multi_thread")]
async fn pages_and_apis_with_secrets_that_never_show() {
    let (url, requests) = server().await;
    let (runtime, sink, _dir) = runtime();

    // A page, through a redirect, as readable text.
    let page = ok(
        &runtime,
        "web.fetch",
        json!({"url": format!("{url}/antiga")}),
    )
    .await;
    assert_eq!(page["status"], 200);
    assert_eq!(page["url"], format!("{url}/pagina"));
    assert_eq!(page["title"], "Notícias");
    assert_eq!(page["content"], "Manchete\nTexto & mais.");
    let raw = ok(
        &runtime,
        "web.fetch",
        json!({"url": format!("{url}/pagina"), "format": "raw"}),
    )
    .await;
    assert!(raw["content"]
        .as_str()
        .unwrap()
        .contains("<h1>Manchete</h1>"));

    // The secret goes to the server, never back: the echo comes masked, in
    // the body, in the headers and in the final URL.
    let echo = ok(
        &runtime,
        "http.request",
        json!({
            "url": format!("{url}/eco?k={{{{secret:MINHA_API}}}}"),
            "headers": {"Authorization": "Bearer {{secret:MINHA_API}}"}
        }),
    )
    .await;
    let sent = requests.lock().last().cloned().unwrap();
    assert_eq!(sent.headers["authorization"], format!("Bearer {KEY}"));
    assert_eq!(sent.path, format!("/eco?k={KEY}"));
    assert_eq!(echo["body"]["authorization"], "Bearer ***");
    assert_eq!(echo["body"]["query"], "/eco?k=***");
    assert_eq!(echo["headers"]["x-eco"], "Bearer ***");
    assert_eq!(echo["url"], format!("{url}/eco?k=***"));

    // POST with JSON (and a secret inside it), DELETE, a 404 that is not an error.
    let created = ok(
        &runtime,
        "http.request",
        json!({"method": "POST", "url": format!("{url}/itens"),
               "json": {"nome": "teste", "chave": "{{secret:MINHA_API}}"}}),
    )
    .await;
    assert_eq!(created["status"], 201);
    assert_eq!(created["ok"], true);
    assert_eq!(
        created["body"]["criado"],
        json!({"nome": "teste", "chave": "***"})
    );
    assert_eq!(created["body"]["tipo"], "application/json");
    let deleted = ok(
        &runtime,
        "http.request",
        json!({"method": "DELETE", "url": format!("{url}/itens/7")}),
    )
    .await;
    assert_eq!(deleted["status"], 204);
    assert_eq!(deleted["body"], Value::Null);
    let missing = ok(&runtime, "web.fetch", json!({"url": format!("{url}/nada")})).await;
    assert_eq!(
        (missing["status"].clone(), missing["ok"].clone()),
        (json!(404), json!(false))
    );

    // Limits and binary bodies.
    let big = ok(
        &runtime,
        "web.fetch",
        json!({"url": format!("{url}/grande"), "maxBytes": 1000}),
    )
    .await;
    assert_eq!(
        (big["truncated"].clone(), big["bytes"].clone()),
        (json!(true), json!(1000))
    );
    let image = ok(
        &runtime,
        "web.fetch",
        json!({"url": format!("{url}/imagem")}),
    )
    .await;
    assert_eq!(
        (image["binary"].clone(), image["content"].clone()),
        (json!(true), json!(""))
    );

    // What the AI may use: names only.
    let list = ok(&runtime, "secrets.list", json!({})).await;
    assert_eq!(list["names"], json!(["MINHA_API"]));

    // The history keeps the placeholder, never the value.
    let history = serde_json::to_string(&sink.audit_events()).unwrap();
    assert!(history.contains("{{secret:MINHA_API}}"));
    assert!(!history.contains(KEY), "a secret value reached the history");
}

#[tokio::test(flavor = "multi_thread")]
async fn bad_requests_say_why() {
    let (url, _requests) = server().await;
    let (runtime, _sink, _dir) = runtime();
    let unknown = call(
        &runtime,
        "http.request",
        json!({"url": format!("{url}/eco"), "headers": {"X": "{{secret:OUTRO}}"}}),
    )
    .await;
    let err = unknown.error.unwrap();
    assert_eq!(err.kind, ToolErrorKind::InvalidArgs);
    assert!(
        err.message.contains("OUTRO") && err.message.contains("MINHA_API"),
        "{}",
        err.message
    );

    let file = call(&runtime, "web.fetch", json!({"url": "file:///etc/hosts"})).await;
    assert_eq!(file.error.unwrap().kind, ToolErrorKind::InvalidArgs);

    // Nothing listens on port 9: a transport error, with no secret in it.
    let down = call(
        &runtime,
        "web.fetch",
        json!({"url": "http://127.0.0.1:9/?k={{secret:MINHA_API}}"}),
    )
    .await;
    let err = down.error.unwrap();
    assert_eq!(err.kind, ToolErrorKind::Io);
    assert!(!err.message.contains(KEY), "{}", err.message);
}

#[tokio::test(flavor = "multi_thread")]
async fn commands_get_secrets_in_their_environment() {
    let (runtime, sink, _dir) = runtime();
    let args = if cfg!(windows) {
        json!({"command": "echo %TOKEN%", "shell": "cmd", "env": {"TOKEN": "{{secret:MINHA_API}}"}})
    } else {
        json!({"command": "echo \"token=$TOKEN\"", "env": {"TOKEN": "{{secret:MINHA_API}}"}})
    };
    let out = ok(&runtime, "shell.execute", args).await;
    let stdout = out["stdout"].as_str().unwrap();
    assert!(stdout.contains("***"), "{stdout}");
    assert!(!stdout.contains(KEY));
    let history = serde_json::to_string(&sink.audit_events()).unwrap();
    assert!(!history.contains(KEY));
}
