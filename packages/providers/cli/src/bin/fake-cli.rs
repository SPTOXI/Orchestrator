//! Pretends to be Claude Code, Codex or Gemini CLI for the tests: reads
//! the prompt on stdin, finds the Orchestrator's MCP URL where each CLI
//! gets it, calls tools there and prints events in that CLI's format.
//!
//! The prompt drives it: `LEIA <path>` reads a file through
//! `filesystem.read`; `ESCREVA <path>` writes one (an action, so the
//! autonomy gate decides); `LOGIN` fails as a CLI without login does;
//! `DORMIR` waits (to be cancelled). `FAKE_CLI_LOG` gets every argv.

use serde_json::{json, Value};
use std::io::{Read, Write};
use std::net::TcpStream;

fn post(url: &str, body: &Value) -> Value {
    let rest = url.strip_prefix("http://").unwrap();
    let (host, path) = rest.split_at(rest.find('/').unwrap());
    let mut stream = TcpStream::connect(host).unwrap();
    let body = body.to_string();
    write!(
        stream,
        "POST {path} HTTP/1.1\r\nHost: {host}\r\nContent-Type: application/json\r\nAccept: application/json, text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
    .unwrap();
    let mut response = String::new();
    // The server keeps the connection open; read headers, then the body.
    let mut buf = [0u8; 65536];
    loop {
        let n = stream.read(&mut buf).unwrap_or(0);
        if n == 0 {
            break;
        }
        response.push_str(&String::from_utf8_lossy(&buf[..n]));
        if let Some(at) = response.find("\r\n\r\n") {
            let len: usize = response[..at]
                .lines()
                .find_map(|l| {
                    l.to_lowercase()
                        .strip_prefix("content-length:")
                        .map(|v| v.trim().parse().unwrap())
                })
                .unwrap_or(0);
            if response.len() >= at + 4 + len {
                let body = &response[at + 4..at + 4 + len];
                return serde_json::from_str(body).unwrap_or(Value::Null);
            }
        }
    }
    Value::Null
}

fn arg_after<'a>(args: &'a [String], flag: &str) -> Option<&'a str> {
    args.iter()
        .position(|a| a == flag)
        .and_then(|i| args.get(i + 1))
        .map(String::as_str)
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("--version") {
        println!("9.9.9 (fake)");
        return;
    }
    if let Ok(log) = std::env::var("FAKE_CLI_LOG") {
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(log)
            .unwrap();
        let env = std::env::var("GEMINI_CLI_SYSTEM_SETTINGS_PATH").ok();
        // One write per line: concurrent runs do not interleave.
        let line = format!(
            "{}\n",
            json!({"args": args, "geminiSettings": env.and_then(|p| std::fs::read_to_string(p).ok())})
        );
        file.write_all(line.as_bytes()).unwrap();
    }
    let kind = if args.first().map(String::as_str) == Some("exec") {
        "codex"
    } else if arg_after(&args, "-o") == Some("stream-json") {
        "gemini"
    } else {
        "claude"
    };
    let mut prompt = String::new();
    std::io::stdin().read_to_string(&mut prompt).unwrap();

    let url: Option<String> = match kind {
        "claude" => arg_after(&args, "--mcp-config").map(|file| {
            let config: Value =
                serde_json::from_str(&std::fs::read_to_string(file).unwrap()).unwrap();
            config["mcpServers"]["orchestrator"]["url"]
                .as_str()
                .unwrap()
                .to_owned()
        }),
        "codex" => args.iter().find_map(|a| {
            a.strip_prefix("mcp_servers.orchestrator.url=")
                .map(str::to_owned)
        }),
        _ => std::env::var("GEMINI_CLI_SYSTEM_SETTINGS_PATH")
            .ok()
            .and_then(|p| {
                let settings: Value =
                    serde_json::from_str(&std::fs::read_to_string(p).ok()?).ok()?;
                settings["mcpServers"]["orchestrator"]["httpUrl"]
                    .as_str()
                    .map(str::to_owned)
            }),
    };
    let session = match kind {
        "claude" => arg_after(&args, "--session-id")
            .or(arg_after(&args, "--resume"))
            .unwrap_or("none")
            .to_owned(),
        "gemini" => arg_after(&args, "--session-id")
            .or(arg_after(&args, "--resume"))
            .unwrap_or("none")
            .to_owned(),
        _ => arg_after(&args, "resume")
            .unwrap_or("thread-123")
            .to_owned(),
    };
    let resumed = args.iter().any(|a| a == "--resume" || a == "resume");

    let emit = |v: Value| println!("{v}");
    if prompt.contains("LOGIN") {
        match kind {
            "claude" => emit(
                json!({"type": "result", "subtype": "success", "is_error": true, "result": "Invalid API key · Please run /login"}),
            ),
            "codex" => emit(json!({"type": "turn.failed", "error": {"message": "Not logged in"}})),
            _ => emit(
                json!({"type": "result", "status": "error", "error": {"message": "Please set an Auth method"}}),
            ),
        }
        std::process::exit(1);
    }
    match kind {
        "claude" => emit(json!({"type": "system", "subtype": "init", "session_id": session})),
        "codex" => emit(json!({"type": "thread.started", "thread_id": session})),
        _ => emit(json!({"type": "init", "session_id": session, "model": "fake"})),
    }
    if prompt.contains("DORMIR") {
        std::thread::sleep(std::time::Duration::from_secs(30));
    }

    let mut answer = format!("{kind}: {}", if resumed { "retomado" } else { "nova" });
    if let Some(url) = &url {
        post(
            url,
            &json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": kind}}}),
        );
        let tools = post(
            url,
            &json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"}),
        );
        let count = tools["result"]["tools"].as_array().map_or(0, Vec::len);
        answer.push_str(&format!(" · {count} ferramentas"));
        for line in prompt.lines() {
            let (tool, args) = if let Some(path) = line.strip_prefix("LEIA ") {
                ("filesystem__read", json!({"path": path.trim()}))
            } else if let Some(path) = line.strip_prefix("ESCREVA ") {
                (
                    "filesystem__write",
                    json!({"path": path.trim(), "content": "escrito pela CLI"}),
                )
            } else {
                continue;
            };
            let reply = post(
                url,
                &json!({"jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": {"name": tool, "arguments": args}}),
            );
            let text = reply["result"]["content"][0]["text"]
                .as_str()
                .unwrap_or("?")
                .to_owned();
            let failed = reply["result"]["isError"].as_bool().unwrap_or(false);
            answer.push_str(&format!(
                " · {tool} {}: {}",
                if failed { "falhou" } else { "ok" },
                text.replace('\n', " ")
            ));
        }
    } else {
        answer.push_str(" · sem ferramentas");
    }
    if prompt.contains("<instructions>") || args.iter().any(|a| a == "--append-system-prompt-file")
    {
        answer.push_str(" · com instruções");
    }

    match kind {
        "claude" => {
            for piece in [&answer[..answer.len() / 2], &answer[answer.len() / 2..]] {
                emit(
                    json!({"type": "stream_event", "event": {"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": piece}}}),
                );
            }
            emit(
                json!({"type": "assistant", "message": {"content": [{"type": "text", "text": answer}]}}),
            );
            emit(
                json!({"type": "result", "subtype": "success", "is_error": false, "result": answer,
                        "usage": {"input_tokens": 100, "cache_read_input_tokens": 50, "output_tokens": 20}, "session_id": session}),
            );
        }
        "codex" => {
            emit(json!({"type": "turn.started"}));
            emit(
                json!({"type": "item.completed", "item": {"id": "i0", "type": "reasoning", "text": "pensando"}}),
            );
            emit(
                json!({"type": "item.completed", "item": {"id": "i1", "type": "agent_message", "text": answer}}),
            );
            emit(
                json!({"type": "turn.completed", "usage": {"input_tokens": 80, "cached_input_tokens": 10, "output_tokens": 15}}),
            );
        }
        _ => {
            emit(json!({"type": "message", "role": "user", "content": prompt}));
            emit(json!({"type": "message", "role": "assistant", "content": answer, "delta": true}));
            emit(
                json!({"type": "result", "status": "success", "stats": {"input_tokens": 60, "output_tokens": 12, "cached": 0}}),
            );
        }
    }
}
