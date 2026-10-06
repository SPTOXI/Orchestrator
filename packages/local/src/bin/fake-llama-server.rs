//! A stand-in for llama.cpp's `llama-server`, for the tests (ADR-0025):
//! the same arguments, `/health` and a streamed `/v1/chat/completions`
//! that says which model and context it was started with.
//!
//! - `FAKE_LLAMA_FAIL=1`: exits at once, as llama.cpp does with a model it
//!   does not know.
//! - `FAKE_LLAMA_LOAD_MS=<ms>`: `/health` answers 503 (loading) that long.
//! - `FAKE_LLAMA_STARTS=<file>`: one line per start (`alias context gpu`).
//! - `FAKE_LLAMA_SMALL_CONTEXT=1`: requests are refused as too long for
//!   the context, with llama.cpp's message.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::time::{Duration, Instant};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--version") {
        println!("version: 0 (fake-llama-server)");
        return;
    }
    let value = |flag: &str| {
        args.iter()
            .position(|a| a == flag)
            .and_then(|i| args.get(i + 1))
            .cloned()
    };
    if std::env::var_os("FAKE_LLAMA_FAIL").is_some() {
        eprintln!("llama_model_load: error loading model: unknown model architecture: 'fake9'");
        std::process::exit(1);
    }
    let port = value("--port").unwrap_or_else(|| "8080".into());
    let alias = value("--alias").unwrap_or_else(|| "model".into());
    let context = value("-c").unwrap_or_else(|| "0".into());
    let gpu = value("-ngl").unwrap_or_else(|| "auto".into());
    if let Some(path) = std::env::var_os("FAKE_LLAMA_STARTS") {
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
        {
            let _ = writeln!(f, "{alias} {context} {gpu}");
        }
    }
    let load = std::env::var("FAKE_LLAMA_LOAD_MS")
        .ok()
        .and_then(|v| v.parse().ok())
        .map(Duration::from_millis)
        .unwrap_or_default();
    let started = Instant::now();
    let listener = TcpListener::bind(format!("127.0.0.1:{port}")).expect("port");
    eprintln!("main: server is listening on http://127.0.0.1:{port}");
    for stream in listener.incoming().flatten() {
        let (alias, context) = (alias.clone(), context.clone());
        let ready = started.elapsed() >= load;
        std::thread::spawn(move || {
            let _ = serve(stream, ready, &alias, &context);
        });
    }
}

fn serve(mut stream: TcpStream, ready: bool, alias: &str, context: &str) -> std::io::Result<()> {
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut line = String::new();
    reader.read_line(&mut line)?;
    let path = line.split_whitespace().nth(1).unwrap_or("/").to_owned();
    let mut length = 0usize;
    loop {
        let mut header = String::new();
        reader.read_line(&mut header)?;
        if header.trim().is_empty() {
            break;
        }
        if let Some(v) = header.to_ascii_lowercase().strip_prefix("content-length:") {
            length = v.trim().parse().unwrap_or(0);
        }
    }
    let mut body = vec![0u8; length];
    reader.read_exact(&mut body)?;
    let respond = |stream: &mut TcpStream, status: &str, kind: &str, text: &str| {
        write!(
            stream,
            "HTTP/1.1 {status}\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{text}",
            text.len()
        )
    };
    match path.as_str() {
        "/health" if ready => respond(
            &mut stream,
            "200 OK",
            "application/json",
            r#"{"status":"ok"}"#,
        ),
        "/health" => respond(
            &mut stream,
            "503 Service Unavailable",
            "application/json",
            r#"{"error":{"message":"Loading model"}}"#,
        ),
        "/v1/chat/completions" if std::env::var_os("FAKE_LLAMA_SMALL_CONTEXT").is_some() => {
            respond(
                &mut stream,
                "400 Bad Request",
                "application/json",
                &format!(
                    r#"{{"error":{{"code":400,"message":"request (44340 tokens) exceeds the available context size ({context} tokens), try increasing it","type":"exceed_context_size_error"}}}}"#
                ),
            )
        }
        "/v1/chat/completions" => {
            let request: serde_json::Value = serde_json::from_slice(&body).unwrap_or_default();
            let tools = request["tools"].as_array().map_or(0, |t| t.len());
            let text = format!("modelo {alias}, contexto {context}, {tools} ferramentas");
            if request["stream"] == true {
                let chunk = |delta: serde_json::Value| {
                    format!(
                        "data: {}\n\n",
                        serde_json::json!({"choices": [{"index": 0, "delta": delta, "finish_reason": null}]})
                    )
                };
                let mut out = chunk(serde_json::json!({"role": "assistant", "content": text}));
                out.push_str(&format!(
                    "data: {}\n\n",
                    serde_json::json!({"choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}]})
                ));
                out.push_str(&format!(
                    "data: {}\n\ndata: [DONE]\n\n",
                    serde_json::json!({"choices": [], "usage": {"prompt_tokens": 10, "completion_tokens": 5}})
                ));
                respond(&mut stream, "200 OK", "text/event-stream", &out)
            } else {
                let reply = serde_json::json!({
                    "choices": [{"index": 0, "message": {"role": "assistant", "content": text}, "finish_reason": "stop"}],
                    "usage": {"prompt_tokens": 10, "completion_tokens": 5}
                });
                respond(
                    &mut stream,
                    "200 OK",
                    "application/json",
                    &reply.to_string(),
                )
            }
        }
        _ => respond(&mut stream, "404 Not Found", "text/plain", "not found"),
    }
}
