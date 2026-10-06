//! The engine's server (ADR-0025) with `fake-llama-server`: a switch
//! waits for the calls on the running model, and an idle engine stops.
//! Its own test binary: the fake server reads environment variables that
//! the other tests set.

use orchestrator_local::engine::EngineInfo;
use orchestrator_local::platform::Backend;
use orchestrator_local::server::{Launch, Server, ServerOptions, ServerStatus};
use orchestrator_local::store::GpuMode;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

const FAKE: &str = env!("CARGO_BIN_EXE_fake-llama-server");

fn fake_engine(dir: &Path) -> EngineInfo {
    EngineInfo {
        tag: "b1".into(),
        backend: Backend::Cpu,
        assets: Vec::new(),
        dir: dir.to_path_buf(),
        server: PathBuf::from(FAKE),
        library_dirs: Vec::new(),
        version: None,
        installed_at: chrono::Utc::now(),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn switching_waits_for_calls_and_idle_stops_the_engine() {
    let dir = tempfile::tempdir().unwrap();
    let engine = fake_engine(dir.path());
    let a = dir.path().join("a.gguf");
    let b = dir.path().join("b.gguf");
    std::fs::write(&a, b"GGUF").unwrap();
    std::fs::write(&b, b"GGUF").unwrap();
    let launch = |model: &str, file: &Path| Launch {
        model: model.into(),
        file: file.to_path_buf(),
        context: 4096,
        gpu: GpuMode::Auto,
    };
    let server = Server::new(
        None,
        Arc::new(|_| {}),
        ServerOptions {
            startup: Duration::from_secs(30),
            idle_check: Duration::from_millis(50),
        },
    );
    let cancel = CancellationToken::new();
    let first = server
        .acquire(&engine, &launch("a", &a), &cancel)
        .await
        .unwrap();
    // Another model waits for the call on "a".
    let switching = {
        let (server, engine, launch_b) = (server.clone(), engine.clone(), launch("b", &b));
        tokio::spawn(async move {
            server
                .acquire(&engine, &launch_b, &CancellationToken::new())
                .await
        })
    };
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert!(
        !switching.is_finished(),
        "switched while a call was using the model"
    );
    assert!(matches!(server.status(), ServerStatus::Ready { ref model, .. } if model == "a"));
    drop(first);
    let second = switching.await.unwrap().unwrap();
    assert!(matches!(server.status(), ServerStatus::Ready { ref model, .. } if model == "b"));
    // Idle: stopped after the call ends.
    server.set_idle(Duration::from_millis(300));
    server.watch_idle();
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(
        matches!(server.status(), ServerStatus::Ready { .. }),
        "a call is still holding it"
    );
    drop(second);
    tokio::time::sleep(Duration::from_millis(900)).await;
    assert_eq!(server.status(), ServerStatus::Stopped);
    // A cancelled wait gives up.
    let blocked = server
        .acquire(&engine, &launch("a", &a), &cancel)
        .await
        .unwrap();
    let giving_up = CancellationToken::new();
    giving_up.cancel();
    assert!(server
        .acquire(&engine, &launch("b", &b), &giving_up)
        .await
        .is_err());
    drop(blocked);
    server.stop("fim do teste").await;
}
