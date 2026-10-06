//! The local engine end to end (ADR-0025), with a fake GitHub, a fake
//! Hugging Face and `fake-llama-server` (this crate's test binary) packed
//! as the llama.cpp release.

use orchestrator_core::NullSink;
use orchestrator_local::download::{self, Expected};
use orchestrator_local::gguf::{write_header, HeaderValue};
use orchestrator_local::platform::{System, GIB};
use orchestrator_local::server::{ProcessWatch, ServerEvent, ServerOptions, ServerStatus};
use orchestrator_local::sources::Urls;
use orchestrator_local::store::{GpuMode, ModelSource};
use orchestrator_local::{LocalEngine, LocalEvent};
use orchestrator_provider_api::{
    ConnectionManager, MemorySecretStore, ProbeRequest, SaveRequest, ToolMode,
};
use orchestrator_providers::ProviderRegistry;
use parking_lot::Mutex;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::io::Write;
use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_util::sync::CancellationToken;

const FAKE: &str = env!("CARGO_BIN_EXE_fake-llama-server");

#[derive(Clone)]
enum Body {
    Json(String),
    File(Vec<u8>),
}

type Routes = Arc<Mutex<HashMap<String, Body>>>;

/// A tiny HTTP server: `path → body`; files honor `Range`.
async fn http(routes: HashMap<String, Body>) -> SocketAddr {
    serve(Arc::new(Mutex::new(routes))).await
}

async fn serve(routes: Routes) -> SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        while let Ok((mut socket, _)) = listener.accept().await {
            let routes = routes.clone();
            tokio::spawn(async move {
                let mut head = Vec::new();
                let mut byte = [0u8; 1];
                while !head.ends_with(b"\r\n\r\n") {
                    if socket.read(&mut byte).await.unwrap_or(0) == 0 {
                        return;
                    }
                    head.push(byte[0]);
                }
                let head = String::from_utf8_lossy(&head).to_string();
                let path = head.split_whitespace().nth(1).unwrap_or("/");
                let path = path.split('?').next().unwrap_or(path).to_owned();
                let range: Option<usize> = head.lines().find_map(|l| {
                    l.to_ascii_lowercase()
                        .strip_prefix("range: bytes=")
                        .and_then(|r| r.trim_end_matches('-').parse().ok())
                });
                let found = routes.lock().get(&path).cloned();
                let (status, kind, body, extra) = match found.as_ref() {
                    Some(Body::Json(text)) => (
                        "200 OK",
                        "application/json",
                        text.clone().into_bytes(),
                        String::new(),
                    ),
                    Some(Body::File(bytes)) => match range {
                        Some(start) if start < bytes.len() => (
                            "206 Partial Content",
                            "application/octet-stream",
                            bytes[start..].to_vec(),
                            format!(
                                "Content-Range: bytes {start}-{}/{}\r\n",
                                bytes.len() - 1,
                                bytes.len()
                            ),
                        ),
                        _ => (
                            "200 OK",
                            "application/octet-stream",
                            bytes.clone(),
                            String::new(),
                        ),
                    },
                    None => (
                        "404 Not Found",
                        "text/plain",
                        b"not found".to_vec(),
                        String::new(),
                    ),
                };
                let header = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: {kind}\r\nContent-Length: {}\r\n{extra}Connection: close\r\n\r\n",
                    body.len()
                );
                let _ = socket.write_all(header.as_bytes()).await;
                let _ = socket.write_all(&body).await;
            });
        }
    });
    addr
}

fn sha(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

/// A release file with the fake server as `llama-server`.
fn package(name: &str) -> Vec<u8> {
    let program = std::fs::read(FAKE).unwrap();
    let mut out = Vec::new();
    if name.ends_with(".zip") {
        let mut zip = zip::ZipWriter::new(std::io::Cursor::new(&mut out));
        let options = zip::write::SimpleFileOptions::default().unix_permissions(0o755);
        zip.start_file("llama-server.exe", options).unwrap();
        zip.write_all(&program).unwrap();
        zip.start_file("ggml-base.dll", options).unwrap();
        zip.write_all(b"dll").unwrap();
        zip.finish().unwrap();
    } else {
        let gz = flate2::write::GzEncoder::new(&mut out, flate2::Compression::fast());
        let mut tar = tar::Builder::new(gz);
        let mut header = tar::Header::new_gnu();
        header.set_size(program.len() as u64);
        header.set_mode(0o755);
        header.set_cksum();
        tar.append_data(&mut header, "llama-b9000/llama-server", program.as_slice())
            .unwrap();
        let mut header = tar::Header::new_gnu();
        header.set_size(3);
        header.set_mode(0o644);
        header.set_cksum();
        tar.append_data(&mut header, "llama-b9000/libggml-base.so", &b"lib"[..])
            .unwrap();
        tar.into_inner().unwrap().finish().unwrap();
    }
    out
}

fn model_file(path: &Path, name: &str, context: u32) -> Vec<u8> {
    write_header(
        path,
        &[
            ("general.architecture", HeaderValue::Str("llama".into())),
            ("general.name", HeaderValue::Str(name.into())),
            ("general.file_type", HeaderValue::U32(15)),
            ("llama.context_length", HeaderValue::U32(context)),
            ("llama.block_count", HeaderValue::U32(4)),
            ("llama.embedding_length", HeaderValue::U32(256)),
            ("llama.attention.head_count", HeaderValue::U32(4)),
            (
                "tokenizer.ggml.tokens",
                HeaderValue::Strings(vec!["a".into(), "b".into()]),
            ),
            (
                "tokenizer.chat_template",
                HeaderValue::Str("{% if tools %}tools{% endif %}".into()),
            ),
        ],
    )
    .unwrap();
    std::fs::read(path).unwrap()
}

fn tree(file: &str, bytes: &[u8]) -> Body {
    Body::Json(
        json!([
            {"type": "directory", "path": "docs"},
            {"type": "file", "path": "README.md", "size": 10},
            {"type": "file", "path": "mmproj-F16.gguf", "size": 3, "lfs": {"oid": "00", "size": 3}},
            {"type": "file", "path": file, "size": bytes.len(), "lfs": {"oid": sha(bytes), "size": bytes.len()}}
        ])
        .to_string(),
    )
}

fn this_system() -> System {
    System {
        nvidia: false,
        vulkan: false,
        memory_bytes: Some(16 * GIB),
        ..System::detect()
    }
}

#[derive(Default)]
struct Watch {
    adopted: Mutex<Vec<String>>,
    released: Mutex<usize>,
}

impl ProcessWatch for Watch {
    fn adopt(&self, _child: &tokio::process::Child, command: &str) {
        self.adopted.lock().push(command.to_owned());
    }
    fn release(&self, _pid: Option<u32>) {
        *self.released.lock() += 1;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn installs_the_engine_brings_models_and_serves_sessions() {
    let dir = tempfile::tempdir().unwrap();
    let system = this_system();
    let backend = system.auto_backend().expect("a package for this system");
    let tag = "b9000";
    let mut routes = HashMap::new();
    let mut assets = Vec::new();
    for pattern in system.asset_patterns(backend, tag).unwrap() {
        let orchestrator_local::platform::AssetPattern::Exact(name) = pattern else {
            panic!("only exact names without NVIDIA");
        };
        let bytes = package(&name);
        assets.push(json!({
            "name": name, "size": bytes.len(), "digest": format!("sha256:{}", sha(&bytes)),
            "browser_download_url": format!("http://{{addr}}/dl/{name}")
        }));
        routes.insert(format!("/dl/{name}"), Body::File(bytes));
    }
    let catalog_model = model_file(&dir.path().join("q.gguf"), "Qwen3 4B", 32768);
    routes.insert(
        "/api/models/Qwen/Qwen3-4B-GGUF/tree/main".into(),
        tree("Qwen3-4B-Q4_K_M.gguf", &catalog_model),
    );
    routes.insert(
        "/Qwen/Qwen3-4B-GGUF/resolve/main/Qwen3-4B-Q4_K_M.gguf".into(),
        Body::File(catalog_model.clone()),
    );
    let hf_model = model_file(&dir.path().join("t.gguf"), "Tiny", 8192);
    routes.insert(
        "/api/models/test/Tiny-GGUF/tree/main".into(),
        tree("Tiny-Q8_0.gguf", &hf_model),
    );
    routes.insert(
        "/test/Tiny-GGUF/resolve/main/Tiny-Q8_0.gguf".into(),
        Body::File(hf_model),
    );
    // The release lists URLs on this same server.
    let routes: Routes = Arc::new(Mutex::new(routes));
    let server = serve(routes.clone()).await;
    // As llama.cpp publishes: pre-releases only, newest first; the newest
    // still without this computer's package, and a draft above it.
    let releases = json!([
        {"tag_name": "b9200", "draft": true, "prerelease": true, "assets": assets},
        {"tag_name": "b9100", "draft": false, "prerelease": true, "assets": []},
        {"tag_name": tag, "draft": false, "prerelease": true, "assets": assets}
    ])
    .to_string()
    .replace("{addr}", &server.to_string());
    routes.lock().insert(
        "/repos/ggml-org/llama.cpp/releases".into(),
        Body::Json(releases),
    );
    let base = format!("http://{server}");

    // Ollama's folders, with one model.
    let ollama = dir.path().join("ollama");
    let blob_bytes = model_file(&dir.path().join("o.gguf"), "phi", 131072);
    std::fs::create_dir_all(ollama.join("blobs")).unwrap();
    std::fs::write(
        ollama.join(format!("blobs/sha256-{}", sha(&blob_bytes))),
        &blob_bytes,
    )
    .unwrap();
    let manifest = ollama.join("manifests/registry.ollama.ai/library/phi4-mini/latest");
    std::fs::create_dir_all(manifest.parent().unwrap()).unwrap();
    std::fs::write(
        &manifest,
        json!({"layers": [{"mediaType": "application/vnd.ollama.image.model", "digest": format!("sha256:{}", sha(&blob_bytes))}]}).to_string(),
    )
    .unwrap();

    let events: Arc<Mutex<Vec<LocalEvent>>> = Arc::default();
    let seen = events.clone();
    let watch = Arc::new(Watch::default());
    let root = dir.path().join("data/local");
    let engine = LocalEngine::open_with(
        root.clone(),
        system.clone(),
        Urls {
            github_api: base.clone(),
            engine_repo: "ggml-org/llama.cpp".into(),
            huggingface: base.clone(),
        },
        vec![ollama.clone()],
        Some(watch.clone()),
        Arc::new(move |e| seen.lock().push(e)),
        ServerOptions {
            startup: Duration::from_secs(30),
            idle_check: Duration::from_millis(100),
        },
    );
    assert!(engine
        .readiness()
        .unwrap_err()
        .contains("não está instalado"));

    // The engine: the newest release with this computer's package.
    assert_eq!(engine.latest_engine().await.unwrap(), tag);
    let info = engine.install_engine().await.unwrap();
    assert_eq!(info.tag, tag);
    assert_eq!(info.backend, backend);
    assert!(info.server.starts_with(root.join("engine")));
    assert!(info
        .version
        .as_deref()
        .unwrap_or_default()
        .contains("fake-llama-server"));
    assert!(!info.library_dirs.is_empty());
    assert!(events
        .lock()
        .iter()
        .any(|e| matches!(e, LocalEvent::Progress { key, .. } if key == "engine")));
    assert!(events
        .lock()
        .iter()
        .any(|e| matches!(e, LocalEvent::EngineInstalled { tag, .. } if tag == "b9000")));

    // Models from four places.
    let qwen = engine.download_catalog("qwen3-4b").await.unwrap();
    assert_eq!(qwen.id, "qwen3-4b");
    assert_eq!(
        qwen.context, 16_384,
        "16 GB of memory: 16k, under the 32k it was trained for"
    );
    assert!(qwen.tools);
    assert_eq!(qwen.quantization.as_deref(), Some("Q4_K_M"));
    assert!(
        matches!(&qwen.source, ModelSource::Catalog { file, .. } if file == "Qwen3-4B-Q4_K_M.gguf")
    );
    let tiny = engine
        .download_hf("https://huggingface.co/test/Tiny-GGUF", "Tiny-Q8_0.gguf")
        .await
        .unwrap();
    assert_eq!(tiny.context, 8192, "never past what it was trained for");
    let listed = engine.ollama_models();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].model.name, "phi4-mini:latest");
    let phi = engine.import_ollama("phi4-mini:latest").await.unwrap();
    assert_eq!(std::fs::read(phi.main_file().unwrap()).unwrap(), blob_bytes);
    assert_eq!(
        engine.ollama_models()[0].imported.as_deref(),
        Some(phi.id.as_str())
    );
    let own = dir.path().join("meu-modelo.gguf");
    model_file(&own, "Meu", 4096);
    let mine = engine.add_file(&own).await.unwrap();
    assert_eq!(mine.files, vec![own.clone()]);
    assert_eq!(engine.models().len(), 4);
    assert!(engine
        .status()
        .catalog
        .iter()
        .any(|c| c.model.id == "qwen3-4b" && c.installed.is_some()));
    engine.set_context(&tiny.id, 4096).unwrap();
    assert!(engine
        .set_context(&tiny.id, 9000)
        .unwrap_err()
        .contains("treinado"));
    assert!(engine.readiness().is_ok());

    // The local connection, through the API provider.
    let mut connection = engine.connection(None);
    assert!(connection.local);
    assert_eq!(connection.models.len(), 4);
    assert_eq!(
        connection.model("qwen3-4b").unwrap().context_window,
        Some(16_384)
    );
    connection.tool_mode = Some(ToolMode::None);
    let sink = Arc::new(NullSink);
    let (manager, _) = ConnectionManager::open(
        &dir.path().join("connections.json"),
        Arc::new(ProviderRegistry::new(sink.clone())),
        Arc::new(MemorySecretStore::default()),
        sink,
    )
    .unwrap();
    manager.set_local_endpoint(engine.clone());
    manager
        .save(
            SaveRequest {
                connection: connection.clone(),
                api_key: None,
                clear_key: false,
                previous_id: None,
            },
            orchestrator_core::CallOrigin::User,
        )
        .await
        .unwrap();
    let starts = dir.path().join("starts.log");
    std::env::set_var("FAKE_LLAMA_STARTS", &starts);
    let ask = |model: &str| ProbeRequest {
        connection: connection.clone(),
        api_key: None,
        model: Some(model.to_owned()),
    };
    let report = manager.test(ask(&tiny.id)).await.unwrap();
    assert!(report.ok, "{report:?}");
    assert_eq!(
        report.reply.as_deref(),
        Some(format!("modelo {}, contexto 4096, 0 ferramentas", tiny.id).as_str())
    );
    // Same model: the running engine answers.
    assert!(manager.test(ask(&tiny.id)).await.unwrap().ok);
    // Another model: switched.
    let report = manager.test(ask("qwen3-4b")).await.unwrap();
    assert_eq!(
        report.reply.as_deref(),
        Some("modelo qwen3-4b, contexto 16384, 0 ferramentas")
    );
    let lines = std::fs::read_to_string(&starts).unwrap();
    assert_eq!(
        lines.lines().collect::<Vec<_>>(),
        [
            format!("{} 4096 auto", tiny.id),
            "qwen3-4b 16384 auto".into()
        ]
    );
    assert!(
        matches!(engine.status().server, ServerStatus::Ready { ref model, context: 16_384, .. } if model == "qwen3-4b")
    );
    assert_eq!(watch.adopted.lock().len(), 2);
    assert!(events.lock().iter().any(
        |e| matches!(e, LocalEvent::Server(ServerEvent::Ready { model, .. }) if model == "qwen3-4b")
    ));

    // A model the engine does not know: the reason, with the hint.
    std::env::set_var("FAKE_LLAMA_FAIL", "1");
    let report = manager.test(ask(&phi.id)).await.unwrap();
    std::env::remove_var("FAKE_LLAMA_FAIL");
    let error = report.error.unwrap();
    assert!(error.contains("atualize o motor"), "{error}");
    assert!(engine
        .log()
        .iter()
        .any(|l| l.contains("unknown model architecture")));

    // A request longer than the context: llama.cpp's reason, and where to
    // change it.
    std::env::set_var("FAKE_LLAMA_SMALL_CONTEXT", "1");
    let report = manager.test(ask("qwen3-4b")).await.unwrap();
    std::env::remove_var("FAKE_LLAMA_SMALL_CONTEXT");
    let error = report.error.unwrap();
    assert!(
        error.contains("exceeds the available context size"),
        "{error}"
    );
    assert!(error.contains("Configurações → Modelos locais"), "{error}");

    // Graphics card off: `-ngl 0`.
    let mut settings = engine.status().settings;
    settings.gpu = GpuMode::Off;
    engine.save_settings(settings).unwrap();
    assert!(manager.test(ask("qwen3-4b")).await.unwrap().ok);
    assert!(std::fs::read_to_string(&starts)
        .unwrap()
        .lines()
        .last()
        .unwrap()
        .ends_with("qwen3-4b 16384 0"));

    // Removing: files go, except the user's own.
    engine.remove_model("qwen3-4b").await.unwrap();
    assert!(!root.join("models/qwen3-4b").exists());
    assert!(matches!(engine.status().server, ServerStatus::Stopped));
    engine.remove_model(&mine.id).await.unwrap();
    assert!(own.exists());
    engine.remove_model(&phi.id).await.unwrap();
    assert!(
        ollama
            .join(format!("blobs/sha256-{}", sha(&blob_bytes)))
            .exists(),
        "Ollama's file stays"
    );
    engine.remove_engine().await.unwrap();
    assert!(!root.join("engine").exists());
    assert!(events
        .lock()
        .iter()
        .any(|e| matches!(e, LocalEvent::EngineRemoved { .. })));
    std::env::remove_var("FAKE_LLAMA_STARTS");

    // What survives a restart.
    let again = LocalEngine::open_with(
        root,
        system,
        Urls::default(),
        Vec::new(),
        None,
        Arc::new(|_| {}),
        ServerOptions::default(),
    );
    let ids: Vec<String> = again.models().iter().map(|m| m.id.clone()).collect();
    assert_eq!(ids, vec![tiny.id.clone()]);
    assert_eq!(again.models()[0].context, 4096);
    assert!(again.status().engine.is_none());
}

#[tokio::test]
async fn models_from_an_older_version_get_their_tools_checked_again() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("local");
    // Phi-4-mini's template only reads tools from a message: llama.cpp
    // never fills it, so its tools must go by prompt.
    let phi = root.join("models/phi4-mini-latest/model.gguf");
    std::fs::create_dir_all(phi.parent().unwrap()).unwrap();
    write_header(
        &phi,
        &[
            ("general.architecture", HeaderValue::Str("phi3".into())),
            (
                "tokenizer.chat_template",
                HeaderValue::Str(
                    "{% if message['role'] == 'system' and 'tools' in message %}{{ message['tools'] }}{% endif %}".into(),
                ),
            ),
        ],
    )
    .unwrap();
    let qwen = root.join("models/qwen3-8b/model.gguf");
    std::fs::create_dir_all(qwen.parent().unwrap()).unwrap();
    model_file(&qwen, "Qwen3 8B", 40960);
    let entry = |id: &str, file: &Path| {
        json!({
            "id": id, "name": id, "files": [file], "size": 1, "sha256": null,
            "source": {"kind": "file"}, "context": 16384, "tools": true,
            "chatTemplate": true, "addedAt": "2026-10-06T00:00:00Z"
        })
    };
    std::fs::write(
        root.join("models.json"),
        json!({"version": 1, "models": [entry("phi4-mini-latest", &phi), entry("qwen3-8b", &qwen)]})
            .to_string(),
    )
    .unwrap();

    let open = || {
        LocalEngine::open_with(
            root.clone(),
            this_system(),
            Urls::default(),
            Vec::new(),
            None,
            Arc::new(|_| {}),
            ServerOptions::default(),
        )
    };
    let tools = |engine: &LocalEngine| -> Vec<(String, bool)> {
        engine
            .models()
            .iter()
            .map(|m| (m.id.clone(), m.tools))
            .collect()
    };
    let expected = vec![
        ("phi4-mini-latest".to_owned(), false),
        ("qwen3-8b".to_owned(), true),
    ];
    assert_eq!(tools(&open()), expected);
    let saved: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(root.join("models.json")).unwrap()).unwrap();
    assert_eq!(saved["version"], 2);
    assert_eq!(tools(&open()), expected);
}

#[test]
fn the_local_connection_takes_its_id_back() {
    use orchestrator_local::{local_connection, LOCAL_CONNECTION_NAME};
    use orchestrator_provider_api::ApiKind;
    let model: orchestrator_local::store::LocalModel = serde_json::from_value(json!({
        "id": "qwen3-8b", "name": "Qwen3 8B", "files": [], "size": 1, "sha256": null,
        "source": {"kind": "file"}, "context": 16384, "tools": true,
        "addedAt": "2026-10-06T00:00:00Z"
    }))
    .unwrap();
    let models = [model];
    let ours = local_connection(&models, None);
    assert!(ours.enabled && ours.local);
    assert_eq!(ours.name, LOCAL_CONNECTION_NAME);

    // Made by hand before ADR-0025 with the same id, for Ollama: replaced.
    let mut by_hand = ours.clone();
    by_hand.local = false;
    by_hand.name = "api local".into();
    by_hand.kind = ApiKind::Generic;
    by_hand.base_url = "http://127.0.0.1:11434".into();
    by_hand.enabled = false;
    by_hand.first_response_secs = Some(30);
    by_hand.tool_mode = Some(ToolMode::Prompt);
    let conn = local_connection(&models, Some(&by_hand));
    assert_eq!(conn, ours);

    // Taken over by 0.1.2 to 0.1.4: the old name, settings and off state
    // go; the choice of models stays.
    let mut taken_over = by_hand.clone();
    taken_over.local = true;
    taken_over.kind = ApiKind::Openai;
    taken_over.default_model = Some("qwen3-8b".into());
    taken_over.models = ours.models.clone();
    taken_over.models[0].enabled = false;
    let conn = local_connection(&models, Some(&taken_over));
    assert_eq!(conn.name, LOCAL_CONNECTION_NAME);
    assert!(conn.enabled);
    assert_eq!(conn.first_response_secs, Some(600));
    assert_eq!(conn.tool_mode, Some(ToolMode::Native));
    assert_eq!(conn.default_model.as_deref(), Some("qwen3-8b"));
    assert!(!conn.models[0].enabled);

    // Ours, turned off by the user: stays off.
    let mut off = ours.clone();
    off.enabled = false;
    assert!(!local_connection(&models, Some(&off)).enabled);
}

#[tokio::test]
async fn downloads_continue_and_are_checked() {
    let dir = tempfile::tempdir().unwrap();
    let bytes: Vec<u8> = (0..300_000u32).map(|i| (i % 251) as u8).collect();
    let server = http(HashMap::from([(
        "/f.bin".to_owned(),
        Body::File(bytes.clone()),
    )]))
    .await;
    let url = format!("http://{server}/f.bin");
    let dest = dir.path().join("out/f.bin");
    std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
    // Half of it from an attempt that was cut.
    std::fs::write(download::part_path(&dest), &bytes[..150_000]).unwrap();
    let client = reqwest::Client::new();
    let progress: Arc<Mutex<Vec<u64>>> = Arc::default();
    let seen = progress.clone();
    let digest = download::fetch(
        &client,
        &url,
        &dest,
        &Expected {
            size: Some(bytes.len() as u64),
            sha256: Some(sha(&bytes)),
        },
        &move |done, _| seen.lock().push(done),
        &CancellationToken::new(),
    )
    .await
    .unwrap();
    assert_eq!(digest, sha(&bytes));
    assert_eq!(std::fs::read(&dest).unwrap(), bytes);
    assert_eq!(
        progress.lock().first().copied(),
        Some(150_000),
        "continued, not restarted"
    );
    assert!(!download::part_path(&dest).exists());

    // A file that does not match what was published is not kept.
    let other = dir.path().join("out/g.bin");
    let error = download::fetch(
        &client,
        &url,
        &other,
        &Expected {
            size: None,
            sha256: Some("00".repeat(32)),
        },
        &|_, _| {},
        &CancellationToken::new(),
    )
    .await
    .unwrap_err();
    assert!(error.contains("não confere"), "{error}");
    assert!(!other.exists() && !download::part_path(&other).exists());

    // Cancelled before it starts.
    let cancel = CancellationToken::new();
    cancel.cancel();
    let error = download::fetch(
        &client,
        &url,
        &other,
        &Expected::default(),
        &|_, _| {},
        &cancel,
    )
    .await
    .unwrap_err();
    assert_eq!(error, download::cancelled());
}
