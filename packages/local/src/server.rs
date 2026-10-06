//! The engine running (ADR-0025): one `llama-server` with one model at a
//! time, started when a session needs it, switched only when no call is
//! using it, and stopped after a while without calls.

use crate::engine::{self, EngineInfo};
use crate::store::GpuMode;
use parking_lot::Mutex;
use serde::Serialize;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

/// Lines of the engine's output kept for the screen and for errors.
const LOG_LINES: usize = 200;

/// Hears about the engine process, to supervise it (ADR-0018): it must
/// not outlive the app.
pub trait ProcessWatch: Send + Sync {
    fn adopt(&self, child: &tokio::process::Child, command: &str);
    fn release(&self, pid: Option<u32>);
}

/// What to run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Launch {
    pub model: String,
    pub file: std::path::PathBuf,
    pub context: u32,
    pub gpu: GpuMode,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", tag = "state")]
pub enum ServerStatus {
    Stopped,
    Starting {
        model: String,
    },
    Ready {
        model: String,
        port: u16,
        context: u32,
    },
    Failed {
        model: String,
        error: String,
    },
}

/// `kind`, not `type`: it travels inside [`crate::LocalEvent`], tagged
/// `type`.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum ServerEvent {
    Starting {
        model: String,
    },
    Ready {
        model: String,
        context: u32,
        gpu: GpuMode,
        ms: u64,
    },
    Stopped {
        model: String,
        reason: String,
    },
    Failed {
        model: String,
        error: String,
    },
}

#[derive(Debug, Clone)]
pub struct ServerOptions {
    /// How long a model may take to load.
    pub startup: Duration,
    /// How often the idle time is checked.
    pub idle_check: Duration,
}

impl Default for ServerOptions {
    fn default() -> Self {
        Self {
            startup: Duration::from_secs(600),
            idle_check: Duration::from_secs(15),
        }
    }
}

struct Running {
    launch: Launch,
    port: u16,
    child: tokio::process::Child,
}

struct Inner {
    slot: tokio::sync::Mutex<Option<Running>>,
    /// Calls using the running model.
    active: AtomicUsize,
    /// Wakes whoever waits for the calls to end.
    idle: Notify,
    last_used: Mutex<Instant>,
    /// Milliseconds without calls before stopping (0 = never).
    idle_ms: AtomicU64,
    status: Mutex<ServerStatus>,
    log: Arc<Mutex<VecDeque<String>>>,
    client: reqwest::Client,
    watch: Option<Arc<dyn ProcessWatch>>,
    events: Arc<dyn Fn(ServerEvent) + Send + Sync>,
    options: ServerOptions,
}

/// The engine's server. Cheap to clone.
#[derive(Clone)]
pub struct Server {
    inner: Arc<Inner>,
}

/// One call's hold on the running model: while it exists, the model is
/// not switched or stopped.
pub struct Lease {
    pub base_url: String,
    inner: Arc<Inner>,
}

impl Drop for Lease {
    fn drop(&mut self) {
        *self.inner.last_used.lock() = Instant::now();
        if self.inner.active.fetch_sub(1, Ordering::SeqCst) == 1 {
            self.inner.idle.notify_waiters();
        }
    }
}

impl Server {
    pub fn new(
        watch: Option<Arc<dyn ProcessWatch>>,
        events: Arc<dyn Fn(ServerEvent) + Send + Sync>,
        options: ServerOptions,
    ) -> Self {
        let client = reqwest::Client::builder()
            .no_proxy()
            .timeout(Duration::from_secs(5))
            .build()
            .unwrap_or_default();
        Self {
            inner: Arc::new(Inner {
                slot: tokio::sync::Mutex::new(None),
                active: AtomicUsize::new(0),
                idle: Notify::new(),
                last_used: Mutex::new(Instant::now()),
                idle_ms: AtomicU64::new(600_000),
                status: Mutex::new(ServerStatus::Stopped),
                log: Arc::default(),
                client,
                watch,
                events,
                options,
            }),
        }
    }

    pub fn status(&self) -> ServerStatus {
        self.inner.status.lock().clone()
    }

    pub fn log(&self) -> Vec<String> {
        self.inner.log.lock().iter().cloned().collect()
    }

    pub fn set_idle(&self, idle: Duration) {
        self.inner.idle_ms.store(
            idle.as_millis().min(u128::from(u64::MAX)) as u64,
            Ordering::Relaxed,
        );
    }

    /// Stops the engine when it has been unused for the idle time. Runs
    /// until the server is dropped.
    pub fn watch_idle(&self) {
        let weak = Arc::downgrade(&self.inner);
        let every = self.inner.options.idle_check;
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(every).await;
                let Some(inner) = weak.upgrade() else { return };
                let ms = inner.idle_ms.load(Ordering::Relaxed);
                if ms == 0 || inner.active.load(Ordering::SeqCst) > 0 {
                    continue;
                }
                if inner.last_used.lock().elapsed() < Duration::from_millis(ms) {
                    continue;
                }
                // Busy (starting or switching): try again later.
                let Ok(mut slot) = inner.slot.try_lock() else {
                    continue;
                };
                if slot.is_some() && inner.active.load(Ordering::SeqCst) == 0 {
                    stop(&inner, &mut slot, "ocioso").await;
                }
            }
        });
    }

    /// A server with `launch` loaded, started (or switched to) if needed.
    pub async fn acquire(
        &self,
        engine: &EngineInfo,
        launch: &Launch,
        cancel: &CancellationToken,
    ) -> Result<Lease, String> {
        let inner = &self.inner;
        let mut slot = tokio::select! {
            s = inner.slot.lock() => s,
            _ = cancel.cancelled() => return Err("cancelado".into()),
        };
        // A process that died on its own is forgotten.
        if let Some(running) = slot.as_mut() {
            if let Ok(Some(status)) = running.child.try_wait() {
                let model = running.launch.model.clone();
                if let Some(w) = &inner.watch {
                    w.release(running.child.id());
                }
                *slot = None;
                *inner.status.lock() = ServerStatus::Stopped;
                (inner.events)(ServerEvent::Stopped {
                    model,
                    reason: format!("o processo terminou ({status})"),
                });
            }
        }
        if let Some(running) = slot.as_ref() {
            if &running.launch == launch {
                inner.active.fetch_add(1, Ordering::SeqCst);
                return Ok(Lease {
                    base_url: format!("http://127.0.0.1:{}/v1", running.port),
                    inner: inner.clone(),
                });
            }
            // Another model: the calls using it finish first.
            loop {
                let notified = inner.idle.notified();
                tokio::pin!(notified);
                notified.as_mut().enable();
                if inner.active.load(Ordering::SeqCst) == 0 {
                    break;
                }
                tokio::select! {
                    _ = notified => {}
                    _ = cancel.cancelled() => return Err("cancelado".into()),
                }
            }
            stop(inner, &mut slot, "troca de modelo").await;
        }
        let running = start(inner, engine, launch, cancel).await?;
        let port = running.port;
        *slot = Some(running);
        inner.active.fetch_add(1, Ordering::SeqCst);
        Ok(Lease {
            base_url: format!("http://127.0.0.1:{port}/v1"),
            inner: inner.clone(),
        })
    }

    /// Stops the engine (waits for nothing: calls in flight fail).
    pub async fn stop(&self, reason: &str) {
        let mut slot = self.inner.slot.lock().await;
        stop(&self.inner, &mut slot, reason).await;
    }

    /// At exit, without waiting: kills the process if no start is under
    /// way (the supervision covers the rest).
    pub fn kill_now(&self) {
        if let Ok(mut slot) = self.inner.slot.try_lock() {
            if let Some(running) = slot.as_mut() {
                let _ = running.child.start_kill();
            }
        }
    }
}

async fn stop(inner: &Inner, slot: &mut Option<Running>, reason: &str) {
    let Some(mut running) = slot.take() else {
        return;
    };
    let pid = running.child.id();
    let _ = running.child.start_kill();
    let _ = tokio::time::timeout(Duration::from_secs(10), running.child.wait()).await;
    if let Some(w) = &inner.watch {
        w.release(pid);
    }
    *inner.status.lock() = ServerStatus::Stopped;
    push_log(
        &inner.log,
        format!("[orchestrator] motor desligado ({reason})"),
    );
    (inner.events)(ServerEvent::Stopped {
        model: running.launch.model,
        reason: reason.to_owned(),
    });
}

fn push_log(log: &Mutex<VecDeque<String>>, line: String) {
    let mut log = log.lock();
    if log.len() >= LOG_LINES {
        log.pop_front();
    }
    log.push_back(line);
}

fn free_port() -> Result<u16, String> {
    let listener = std::net::TcpListener::bind("127.0.0.1:0")
        .map_err(|e| format!("sem porta livre para o motor: {e}"))?;
    listener
        .local_addr()
        .map(|a| a.port())
        .map_err(|e| e.to_string())
}

pub fn arguments(launch: &Launch, port: u16) -> Vec<String> {
    let mut args = vec![
        "-m".to_owned(),
        launch.file.to_string_lossy().into_owned(),
        "--host".into(),
        "127.0.0.1".into(),
        "--port".into(),
        port.to_string(),
        "-c".into(),
        launch.context.to_string(),
        "--alias".into(),
        launch.model.clone(),
        "--jinja".into(),
        "--no-webui".into(),
    ];
    if launch.gpu == GpuMode::Off {
        args.extend(["-ngl".into(), "0".into()]);
    }
    args
}

async fn start(
    inner: &Inner,
    engine: &EngineInfo,
    launch: &Launch,
    cancel: &CancellationToken,
) -> Result<Running, String> {
    let model = launch.model.clone();
    let fail = |error: String| {
        *inner.status.lock() = ServerStatus::Failed {
            model: model.clone(),
            error: error.clone(),
        };
        (inner.events)(ServerEvent::Failed {
            model: model.clone(),
            error: error.clone(),
        });
        Err(error)
    };
    if !launch.file.is_file() {
        return fail(format!(
            "o arquivo do modelo {model} não está mais em {}",
            launch.file.display()
        ));
    }
    let port = match free_port() {
        Ok(p) => p,
        Err(e) => return fail(e),
    };
    let began = Instant::now();
    *inner.status.lock() = ServerStatus::Starting {
        model: model.clone(),
    };
    (inner.events)(ServerEvent::Starting {
        model: model.clone(),
    });
    push_log(
        &inner.log,
        format!(
            "[orchestrator] ligando o motor com {model} (contexto {})",
            launch.context
        ),
    );
    let mut cmd = engine::command(engine);
    cmd.args(arguments(launch, port))
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => return fail(format!("não foi possível iniciar o motor: {e}")),
    };
    if let Some(w) = &inner.watch {
        w.adopt(&child, &format!("llama-server {model}"));
    }
    for pipe in [
        child
            .stdout
            .take()
            .map(|s| Box::new(s) as Box<dyn tokio::io::AsyncRead + Send + Unpin>),
        child
            .stderr
            .take()
            .map(|s| Box::new(s) as Box<dyn tokio::io::AsyncRead + Send + Unpin>),
    ]
    .into_iter()
    .flatten()
    {
        let log = inner.log.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(pipe).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                push_log(&log, line);
            }
        });
    }
    let health = format!("http://127.0.0.1:{port}/health");
    let deadline = began + inner.options.startup;
    loop {
        if let Ok(Some(status)) = child.try_wait() {
            // Let the last lines arrive.
            tokio::time::sleep(Duration::from_millis(200)).await;
            if let Some(w) = &inner.watch {
                w.release(child.id());
            }
            return fail(explain(
                &model,
                &format!("o motor parou ({status})"),
                &inner.log.lock(),
            ));
        }
        if let Ok(response) = inner.client.get(&health).send().await {
            if response.status().is_success() {
                break;
            }
        }
        if Instant::now() >= deadline {
            let _ = child.start_kill();
            if let Some(w) = &inner.watch {
                w.release(child.id());
            }
            return fail(explain(
                &model,
                &format!(
                    "o modelo não carregou em {} minutos",
                    inner.options.startup.as_secs() / 60
                ),
                &inner.log.lock(),
            ));
        }
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_millis(400)) => {}
            _ = cancel.cancelled() => {
                let _ = child.start_kill();
                if let Some(w) = &inner.watch {
                    w.release(child.id());
                }
                *inner.status.lock() = ServerStatus::Stopped;
                return Err("cancelado".into());
            }
        }
    }
    let ms = began.elapsed().as_millis() as u64;
    *inner.status.lock() = ServerStatus::Ready {
        model: model.clone(),
        port,
        context: launch.context,
    };
    push_log(
        &inner.log,
        format!(
            "[orchestrator] {model} pronto em {:.1} s",
            ms as f64 / 1000.0
        ),
    );
    (inner.events)(ServerEvent::Ready {
        model,
        context: launch.context,
        gpu: launch.gpu,
        ms,
    });
    Ok(Running {
        launch: launch.clone(),
        port,
        child,
    })
}

/// An error with a hint from the engine's last lines.
fn explain(model: &str, what: &str, log: &VecDeque<String>) -> String {
    let tail: Vec<&str> = log
        .iter()
        .rev()
        .filter(|l| !l.starts_with("[orchestrator]") && !l.trim().is_empty())
        .take(6)
        .map(String::as_str)
        .collect();
    let text = tail.join("\n").to_lowercase();
    let hint = if text.contains("out of memory")
        || text.contains("failed to allocate")
        || text.contains("unable to allocate")
        || text.contains("cudamalloc failed")
    {
        " — memória insuficiente: diminua o contexto do modelo ou escolha um menor"
    } else if text.contains("unknown model architecture") || text.contains("unknown architecture") {
        " — o motor não conhece este tipo de modelo: atualize o motor"
    } else if text.contains("failed to load model") || text.contains("error loading model") {
        " — o arquivo do modelo não pôde ser lido (incompleto ou de outro formato?)"
    } else {
        ""
    };
    let last = tail.into_iter().rev().collect::<Vec<_>>().join(" | ");
    if last.is_empty() {
        format!("{model}: {what}{hint}")
    } else {
        format!("{model}: {what}{hint}. Últimas linhas do motor: {last}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn passes_the_context_and_the_card_choice() {
        let launch = Launch {
            model: "qwen3-8b".into(),
            file: "/m/q.gguf".into(),
            context: 16384,
            gpu: GpuMode::Auto,
        };
        let args = arguments(&launch, 8123);
        let joined = args.join(" ");
        assert!(joined.contains("-c 16384"));
        assert!(joined.contains("--port 8123"));
        assert!(joined.contains("--alias qwen3-8b"));
        assert!(joined.contains("--jinja"));
        assert!(!joined.contains("-ngl"));
        let off = arguments(
            &Launch {
                gpu: GpuMode::Off,
                ..launch
            },
            1,
        );
        assert!(off.join(" ").ends_with("-ngl 0"));
    }

    #[test]
    fn explains_why_a_model_did_not_load() {
        let mut log = VecDeque::new();
        log.push_back(
            "llama_model_load: error loading model: unknown model architecture: 'qwen9'".to_owned(),
        );
        assert!(explain("m", "o motor parou", &log).contains("atualize o motor"));
        log.push_back(
            "ggml_backend_cuda_buffer_type_alloc_buffer: cudaMalloc failed: out of memory"
                .to_owned(),
        );
        assert!(explain("m", "o motor parou", &log).contains("memória insuficiente"));
        assert_eq!(explain("m", "x", &VecDeque::new()), "m: x");
    }
}
