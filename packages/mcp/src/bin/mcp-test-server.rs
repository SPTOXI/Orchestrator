//! A tiny stdio MCP server for the tests: `echo` (read-only, and it pings
//! the client before answering), `add` (structured result), `fail`
//! (`isError`), `slow` (waits), `env` (reads `TEST_TOKEN`) and `grow`
//! (adds a tool and says the list changed).

use serde_json::{json, Value};
use std::io::{BufRead, Write};
use std::sync::{Arc, Mutex};

type Out = Arc<Mutex<std::io::Stdout>>;

fn send(out: &mut Out, value: Value) {
    let out = out.lock().unwrap();
    let mut out = out.lock();
    writeln!(out, "{value}").unwrap();
    out.flush().unwrap();
}

fn tools(grown: bool) -> Value {
    let mut list = vec![
        json!({"name": "echo", "description": "Echoes text", "annotations": {"readOnlyHint": true},
               "inputSchema": {"type": "object", "properties": {"text": {"type": "string"}}, "required": ["text"]}}),
        json!({"name": "add", "description": "Adds two numbers",
               "inputSchema": {"type": "object", "properties": {"a": {"type": "number"}, "b": {"type": "number"}}}}),
        json!({"name": "fail", "description": "Always fails", "inputSchema": {"type": "object"}}),
        json!({"name": "slow", "description": "Waits", "inputSchema": {"type": "object"}}),
        json!({"name": "env", "description": "Reads TEST_TOKEN", "inputSchema": {"type": "object"}}),
        json!({"name": "grow", "description": "Adds a tool", "inputSchema": {"type": "object"}}),
        json!({"name": "weird name__with.dots", "description": "Odd name", "inputSchema": {"type": "object"}}),
    ];
    if grown {
        list.push(json!({"name": "extra", "description": "Appeared later", "inputSchema": {"type": "object"}}));
    }
    json!({"tools": list})
}

fn main() {
    eprintln!("mcp-test-server starting");
    let stdin = std::io::stdin();
    let mut out: Out = Arc::new(Mutex::new(std::io::stdout()));
    let mut lines = stdin.lock().lines();
    let mut grown = false;
    while let Some(Ok(line)) = lines.next() {
        let Ok(message) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let Some(method) = message.get("method").and_then(Value::as_str) else {
            continue;
        };
        let Some(id) = message.get("id").cloned() else {
            continue; // a notification
        };
        let result = match method {
            "initialize" => json!({
                "protocolVersion": message["params"]["protocolVersion"],
                "capabilities": {"tools": {"listChanged": true}},
                "serverInfo": {"name": "test-server", "version": "1.2.3"},
                "instructions": "Use echo to test."
            }),
            "tools/list" => tools(grown),
            "tools/call" => {
                let args = &message["params"]["arguments"];
                match message["params"]["name"].as_str().unwrap_or_default() {
                    "echo" => {
                        // Ask the client something first.
                        send(
                            &mut out,
                            json!({"jsonrpc": "2.0", "id": "srv-1", "method": "ping"}),
                        );
                        for reply in lines.by_ref() {
                            let reply: Value =
                                serde_json::from_str(&reply.unwrap()).unwrap_or(Value::Null);
                            if reply["id"] == "srv-1" {
                                break;
                            }
                        }
                        json!({"content": [{"type": "text", "text": format!("echo: {}", args["text"].as_str().unwrap_or(""))}]})
                    }
                    "add" => {
                        let sum =
                            args["a"].as_f64().unwrap_or(0.0) + args["b"].as_f64().unwrap_or(0.0);
                        json!({"content": [{"type": "text", "text": sum.to_string()}], "structuredContent": {"sum": sum}})
                    }
                    "fail" => {
                        json!({"content": [{"type": "text", "text": "it broke"}], "isError": true})
                    }
                    "slow" => {
                        // Answers later, from another thread: the server
                        // keeps serving meanwhile.
                        let mut later = out.clone();
                        let id = id.clone();
                        std::thread::spawn(move || {
                            std::thread::sleep(std::time::Duration::from_secs(5));
                            send(
                                &mut later,
                                json!({"jsonrpc": "2.0", "id": id, "result": {"content": []}}),
                            );
                        });
                        continue;
                    }
                    "env" => {
                        json!({"content": [{"type": "text", "text": std::env::var("TEST_TOKEN").unwrap_or_default()}]})
                    }
                    "grow" => {
                        grown = true;
                        send(
                            &mut out,
                            json!({"jsonrpc": "2.0", "method": "notifications/tools/list_changed"}),
                        );
                        json!({"content": [{"type": "text", "text": "grown"}]})
                    }
                    other => {
                        send(
                            &mut out,
                            json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32602, "message": format!("unknown tool {other}")}}),
                        );
                        continue;
                    }
                }
            }
            _ => {
                send(
                    &mut out,
                    json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32601, "message": "no"}}),
                );
                continue;
            }
        };
        send(
            &mut out,
            json!({"jsonrpc": "2.0", "id": id, "result": result}),
        );
    }
}
