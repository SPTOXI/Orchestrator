//! Fake HTTP server that imitates AI APIs, plus a session harness wired
//! like the app (SessionManager + real Tool Runtime).

#![allow(dead_code)]

use async_trait::async_trait;
use orchestrator_core::{
    AuditEvent, CallOrigin, EventKind, MemorySink, SessionEvent, SessionId, SessionInfo,
    SessionStatus, ToolCall, ToolDefinition, ToolResult, TurnStatus,
};
use orchestrator_provider_api::{Connection, ConnectionManager, MemorySecretStore, SaveRequest};
use orchestrator_providers::{
    ManagerConfig, ProviderRegistry, SessionManager, SessionStore, StartRequest, ToolExecutor,
};
use orchestrator_runtime::{RuntimeConfig, ToolRuntime};
use parking_lot::Mutex;
use serde_json::Value;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tempfile::TempDir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

#[derive(Debug, Clone)]
pub struct Recorded {
    pub method: String,
    /// Path and query.
    pub path: String,
    /// Lowercase header names.
    pub headers: HashMap<String, String>,
    pub body: Value,
}

#[derive(Debug, Clone)]
pub struct Reply {
    pub status: u16,
    pub content_type: &'static str,
    pub chunks: Vec<String>,
    pub delay: Duration,
}

impl Reply {
    pub fn json(value: Value) -> Self {
        Self {
            status: 200,
            content_type: "application/json",
            chunks: vec![value.to_string()],
            delay: Duration::ZERO,
        }
    }

    pub fn status(status: u16, body: Value) -> Self {
        Self {
            status,
            ..Self::json(body)
        }
    }

    /// `data: …` events (and `event:` names when given).
    pub fn sse(events: Vec<(Option<&str>, Value)>) -> Self {
        let mut chunks: Vec<String> = events
            .into_iter()
            .map(|(name, data)| match name {
                Some(name) => format!("event: {name}\ndata: {data}\n\n"),
                None => format!("data: {data}\n\n"),
            })
            .collect();
        chunks.push(String::new());
        Self {
            status: 200,
            content_type: "text/event-stream",
            chunks,
            delay: Duration::ZERO,
        }
    }

    pub fn sse_done(mut self) -> Self {
        self.chunks.push("data: [DONE]\n\n".into());
        self
    }

    pub fn ndjson(lines: Vec<Value>) -> Self {
        Self {
            status: 200,
            content_type: "application/x-ndjson",
            chunks: lines.into_iter().map(|l| format!("{l}\n")).collect(),
            delay: Duration::ZERO,
        }
    }

    pub fn slow(mut self, delay: Duration) -> Self {
        self.delay = delay;
        self
    }
}

type Handler = Arc<dyn Fn(&Recorded, usize) -> Reply + Send + Sync>;

pub struct FakeApi {
    pub addr: SocketAddr,
    pub requests: Arc<Mutex<Vec<Recorded>>>,
}

impl FakeApi {
    /// Serves `handler(request, index_of_request)`.
    pub async fn start(
        handler: impl Fn(&Recorded, usize) -> Reply + Send + Sync + 'static,
    ) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let requests: Arc<Mutex<Vec<Recorded>>> = Arc::default();
        let handler: Handler = Arc::new(handler);
        let log = requests.clone();
        tokio::spawn(async move {
            loop {
                let Ok((mut socket, _)) = listener.accept().await else {
                    return;
                };
                let handler = handler.clone();
                let log = log.clone();
                tokio::spawn(async move {
                    let Some(request) = read_request(&mut socket).await else {
                        return;
                    };
                    let index = {
                        let mut log = log.lock();
                        log.push(request.clone());
                        log.len() - 1
                    };
                    let reply = handler(&request, index);
                    let head = format!(
                        "HTTP/1.1 {} X\r\nContent-Type: {}\r\nConnection: close\r\n\r\n",
                        reply.status, reply.content_type
                    );
                    if socket.write_all(head.as_bytes()).await.is_err() {
                        return;
                    }
                    for chunk in reply.chunks {
                        if !reply.delay.is_zero() {
                            tokio::time::sleep(reply.delay).await;
                        }
                        if socket.write_all(chunk.as_bytes()).await.is_err() {
                            return;
                        }
                        let _ = socket.flush().await;
                    }
                    let _ = socket.shutdown().await;
                });
            }
        });
        Self { addr, requests }
    }

    pub fn url(&self, path: &str) -> String {
        format!("http://{}{path}", self.addr)
    }

    pub fn requests(&self) -> Vec<Recorded> {
        self.requests.lock().clone()
    }

    /// Requests whose path starts with `prefix`.
    pub fn calls(&self, prefix: &str) -> Vec<Recorded> {
        self.requests()
            .into_iter()
            .filter(|r| r.path.starts_with(prefix))
            .collect()
    }
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
    let body = serde_json::from_slice(&body).unwrap_or(Value::Null);
    Some(Recorded {
        method,
        path,
        headers,
        body,
    })
}

/// The app wires the Tool Runtime the same way.
pub struct RuntimeTools(pub ToolRuntime);

#[async_trait]
impl ToolExecutor for RuntimeTools {
    fn tools(&self) -> Vec<ToolDefinition> {
        ToolRuntime::definitions().to_vec()
    }

    async fn execute(&self, call: ToolCall) -> ToolResult {
        self.0.invoke(call).await
    }
}

pub struct Harness {
    pub sessions: SessionManager,
    pub connections: ConnectionManager,
    pub registry: Arc<ProviderRegistry>,
    pub secrets: Arc<MemorySecretStore>,
    pub sink: Arc<MemorySink>,
    pub dir: TempDir,
}

impl Harness {
    pub fn new() -> Self {
        Self::build(None)
    }

    /// A harness whose sessions are kept in `store` (an app restart is a
    /// new harness on the same store).
    pub fn with_store(store: Arc<dyn SessionStore>) -> Self {
        Self::build(Some(store))
    }

    fn build(store: Option<Arc<dyn SessionStore>>) -> Self {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("hello.txt"), "olá do projeto").unwrap();
        let sink = Arc::new(MemorySink::new());
        let registry = Arc::new(ProviderRegistry::new(sink.clone()));
        let secrets = Arc::new(MemorySecretStore::new());
        let runtime = ToolRuntime::new(
            RuntimeConfig {
                base_dir: dir.path().to_path_buf(),
            },
            sink.clone(),
        );
        let config = ManagerConfig {
            cancel_grace: Duration::from_millis(500),
            log_capacity: 2_000,
        };
        let tools = Arc::new(RuntimeTools(runtime));
        let sessions = match store {
            Some(store) => {
                SessionManager::with_store(registry.clone(), tools, sink.clone(), config, store)
            }
            None => SessionManager::with_config(registry.clone(), tools, sink.clone(), config),
        };
        let (connections, warnings) = ConnectionManager::open(
            &dir.path().join("data/connections.json"),
            registry.clone(),
            secrets.clone(),
            sink.clone(),
        )
        .unwrap();
        assert!(warnings.is_empty(), "{warnings:?}");
        Self {
            sessions,
            connections,
            registry,
            secrets,
            sink,
            dir,
        }
    }

    pub async fn add(&self, connection: Connection, key: Option<&str>) {
        self.connections
            .save(
                SaveRequest {
                    connection,
                    api_key: key.map(str::to_owned),
                    clear_key: false,
                    previous_id: None,
                },
                CallOrigin::User,
            )
            .await
            .unwrap();
    }

    pub async fn start(&self, provider: &str) -> SessionInfo {
        self.sessions
            .start(
                StartRequest {
                    provider: Some(provider.into()),
                    ..Default::default()
                },
                self.dir.path().to_path_buf(),
                CallOrigin::User,
            )
            .await
            .unwrap()
    }

    pub async fn turn(&self, id: &SessionId, input: &str) -> SessionInfo {
        self.sessions
            .send(id, input.into(), CallOrigin::User)
            .await
            .unwrap();
        self.idle(id).await
    }

    pub async fn idle(&self, id: &SessionId) -> SessionInfo {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            let info = self.sessions.info(id).unwrap();
            if info.status != SessionStatus::Running {
                return info;
            }
            assert!(Instant::now() < deadline, "turn did not finish");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    pub fn events(&self, id: &SessionId) -> Vec<SessionEvent> {
        self.sessions
            .snapshot(id)
            .unwrap()
            .entries
            .into_iter()
            .map(|e| e.event)
            .collect()
    }

    pub fn text(&self, id: &SessionId) -> String {
        self.events(id)
            .iter()
            .filter_map(|e| match e {
                SessionEvent::TextDelta { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("|")
    }

    pub fn last_turn(&self, id: &SessionId) -> (TurnStatus, Option<String>) {
        self.events(id)
            .into_iter()
            .rev()
            .find_map(|e| match e {
                SessionEvent::TurnCompleted { status, error, .. } => Some((status, error)),
                _ => None,
            })
            .expect("a completed turn")
    }

    pub fn audits(&self, kind: EventKind) -> Vec<AuditEvent> {
        self.sink
            .audit_events()
            .into_iter()
            .filter(|e| e.kind == kind)
            .collect()
    }
}

pub fn connection(value: Value) -> Connection {
    serde_json::from_value(value).unwrap()
}
