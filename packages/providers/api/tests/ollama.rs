//! Offline models (ADR-0021) against a fake Ollama: status, the models
//! with their capabilities, a download with progress, a failed one,
//! removal, and the connection built from them.

mod support;

use orchestrator_provider_api::ollama::{connection, Ollama, PullProgress};
use orchestrator_provider_api::ToolMode;
use parking_lot::Mutex;
use serde_json::json;
use std::sync::Arc;
use support::{FakeApi, Reply};
use tokio_util::sync::CancellationToken;

#[tokio::test(flavor = "multi_thread")]
async fn models_are_listed_downloaded_and_removed() {
    let api = FakeApi::start(|request, _| match (request.method.as_str(), request.path.as_str()) {
        ("GET", "/api/version") => Reply::json(json!({"version": "0.12.3"})),
        ("GET", "/api/tags") => Reply::json(json!({"models": [
            {"name": "qwen3:8b", "size": 5_200_000_000u64, "modified_at": "2026-09-30T10:00:00Z",
             "details": {"family": "qwen3", "parameter_size": "8.2B", "quantization_level": "Q4_K_M"}},
            {"name": "gemma3:4b", "size": 3_300_000_000u64, "details": {}}
        ]})),
        ("POST", "/api/show") if request.body["model"] == "qwen3:8b" => Reply::json(json!({
            "capabilities": ["completion", "tools"],
            "model_info": {"qwen3.context_length": 40960}
        })),
        ("POST", "/api/show") => Reply::json(json!({"capabilities": ["completion", "vision"]})),
        ("POST", "/api/pull") if request.body["model"] == "nao-existe:1b" => {
            Reply::ndjson(vec![json!({"status": "pulling manifest"}), json!({"error": "pull model manifest: file does not exist"})])
        }
        ("POST", "/api/pull") => Reply::ndjson(vec![
            json!({"status": "pulling manifest"}),
            json!({"status": "pulling abc", "digest": "abc", "total": 1000, "completed": 400}),
            json!({"status": "pulling abc", "digest": "abc", "total": 1000, "completed": 1000}),
            json!({"status": "verifying sha256 digest"}),
            json!({"status": "success"}),
        ]),
        ("DELETE", "/api/delete") => Reply::json(json!({})),
        _ => Reply::status(404, json!({"error": "not found"})),
    })
    .await;
    let ollama = Ollama::new(api.url("")).unwrap();

    let status = ollama.status().await;
    assert!(status.running);
    assert_eq!(status.version.as_deref(), Some("0.12.3"));

    let models = ollama.models().await.unwrap();
    assert_eq!(models.len(), 2);
    let gemma = &models[0];
    let qwen = &models[1];
    assert_eq!(qwen.name, "qwen3:8b");
    assert_eq!(qwen.tools, Some(true));
    assert_eq!(qwen.context_window, Some(40960));
    assert_eq!(qwen.parameter_size.as_deref(), Some("8.2B"));
    assert_eq!(gemma.tools, Some(false), "capabilities without tools");
    assert_eq!(gemma.vision, Some(true));

    let seen: Arc<Mutex<Vec<PullProgress>>> = Arc::default();
    let log = seen.clone();
    ollama
        .pull("qwen3:8b", &move |p| log.lock().push(p), &CancellationToken::new())
        .await
        .unwrap();
    let seen = seen.lock().clone();
    assert_eq!(seen.last().unwrap().status, "success");
    assert!(seen.iter().any(|p| p.completed == Some(400) && p.total == Some(1000)));

    let err = ollama
        .pull("nao-existe:1b", &|_| {}, &CancellationToken::new())
        .await
        .unwrap_err();
    assert!(err.message.contains("file does not exist"), "{}", err.message);
    assert!(ollama.pull("dois nomes", &|_| {}, &CancellationToken::new()).await.is_err());

    ollama.delete("gemma3:4b").await.unwrap();
    assert!(api.requests().iter().any(|r| r.method == "DELETE" && r.body["model"] == "gemma3:4b"));

    // The connection: no key, no cost, the capabilities Ollama reported,
    // and the user's choices kept when it is rebuilt.
    let conn = connection(&api.url(""), &models, None);
    assert_eq!(conn.id, "ollama");
    assert_eq!(conn.base_url, format!("{}/v1", api.url("")));
    assert_eq!(conn.tool_mode, Some(ToolMode::Native));
    let entry = conn.model("gemma3:4b").unwrap();
    assert_eq!(entry.supports_tools, Some(false));
    assert_eq!(entry.input_price, Some(0.0));
    assert!(entry.tags.contains(&"offline".to_owned()));
    conn.validate().unwrap();

    let mut edited = conn.clone();
    edited.default_model = Some("qwen3:8b".into());
    edited.models[0].enabled = false;
    let rebuilt = connection(&api.url(""), &models[1..], Some(&edited));
    assert_eq!(rebuilt.default_model.as_deref(), Some("qwen3:8b"));
    assert_eq!(rebuilt.models.len(), 1, "a removed model leaves the list");
    let gone = connection(&api.url(""), &models[..1], Some(&edited));
    assert_eq!(gone.default_model, None, "the default model was removed");
    assert!(!gone.models[0].enabled, "turned off stays off");
}

#[tokio::test(flavor = "multi_thread")]
async fn nothing_listening_is_not_running() {
    let closed = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = closed.local_addr().unwrap().port();
    drop(closed);
    let status = Ollama::new(format!("http://127.0.0.1:{port}")).unwrap().status().await;
    assert!(!status.running);
    assert!(status.error.is_some());
}
