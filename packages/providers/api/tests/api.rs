//! API providers end to end against a fake HTTP server: every protocol,
//! tool calls executed by the real Tool Runtime, errors, discovery and the
//! connection manager.

mod support;

use orchestrator_core::{
    CallOrigin, ContextSummary, EventKind, SessionEvent, SessionStatus, TurnStatus,
};
use orchestrator_provider_api::{ConnectionManager, ProbeRequest, SaveRequest, SecretStore};
use orchestrator_providers::{
    AttachedContext, CompletionRequest, ContextRequest, ContextSource, MemorySessionStore,
    ProviderErrorKind, SessionStore,
};
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::{Duration, Instant};
use support::{connection, FakeApi, Harness, Reply};

fn openai_chunk(delta: Value, finish: Option<&str>) -> (Option<&'static str>, Value) {
    (
        None,
        json!({"model": "gpt-test", "choices": [{"index": 0, "delta": delta, "finish_reason": finish}]}),
    )
}

fn openai_usage(input: u64, output: u64) -> (Option<&'static str>, Value) {
    (
        None,
        json!({"choices": [], "usage": {"prompt_tokens": input, "completion_tokens": output}}),
    )
}

#[tokio::test(flavor = "multi_thread")]
async fn openai_streams_text_and_runs_tools_through_the_runtime() {
    let api = FakeApi::start(|_, index| match index {
        0 => Reply::sse(vec![
            openai_chunk(json!({"role": "assistant", "content": "Vou ler"}), None),
            openai_chunk(
                json!({"tool_calls": [{"index": 0, "id": "call_1", "type": "function",
                    "function": {"name": "filesystem__read", "arguments": "{\"pa"}}]}),
                None,
            ),
            openai_chunk(
                json!({"tool_calls": [{"index": 0, "function": {"arguments": "th\":\"hello.txt\"}"}}]}),
                None,
            ),
            openai_chunk(json!({}), Some("tool_calls")),
            openai_usage(10, 5),
        ])
        .sse_done(),
        _ => Reply::sse(vec![
            openai_chunk(json!({"content": "Pronto: olá"}), None),
            openai_chunk(json!({}), Some("stop")),
            openai_usage(20, 4),
        ])
        .sse_done(),
    })
    .await;
    let h = Harness::new();
    h.add(
        connection(json!({
            "id": "openai-teste", "name": "OpenAI teste", "kind": "openai",
            "baseUrl": api.url("/v1"), "credential": {"source": "vault"},
            "models": [{"id": "gpt-test", "inputPrice": 1.0, "outputPrice": 2.0}]
        })),
        Some("sk-test"),
    )
    .await;
    let session = h.start("openai-teste").await;
    let done = h.turn(&session.id, "leia hello.txt").await;
    assert_eq!(h.last_turn(&session.id), (TurnStatus::Completed, None));
    assert_eq!(h.text(&session.id), "Vou ler|Pronto: olá");

    let calls = api.calls("/v1/chat/completions");
    assert_eq!(calls.len(), 2);
    let first = &calls[0];
    assert_eq!(first.headers["authorization"], "Bearer sk-test");
    assert_eq!(first.body["model"], "gpt-test");
    assert_eq!(first.body["stream"], true);
    assert_eq!(first.body["stream_options"]["include_usage"], true);
    assert_eq!(first.body["messages"][0]["role"], "system");
    let tool = first.body["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["function"]["name"] == "filesystem__read")
        .expect("runtime tools offered with API-safe names");
    assert_eq!(tool["function"]["parameters"]["type"], "object");

    let second = &calls[1].body["messages"];
    let assistant = &second[2];
    assert_eq!(assistant["role"], "assistant");
    assert_eq!(assistant["tool_calls"][0]["id"], "call_1");
    assert_eq!(
        assistant["tool_calls"][0]["function"]["arguments"],
        "{\"path\":\"hello.txt\"}"
    );
    let tool_message = &second[3];
    assert_eq!(tool_message["role"], "tool");
    assert_eq!(tool_message["tool_call_id"], "call_1");
    assert!(tool_message["content"]
        .as_str()
        .unwrap()
        .contains("olá do projeto"));

    // Executed by the Tool Runtime on behalf of the session.
    let audited = h.audits(EventKind::ToolCalled);
    let read = audited
        .iter()
        .find(|e| e.data["tool"] == "filesystem.read")
        .expect("TOOL_CALLED");
    assert_eq!(
        read.origin,
        CallOrigin::session(&session.id, &"openai-teste".into())
    );
    // Usage summed over both rounds, cost from the model's prices.
    assert_eq!(done.usage.input_tokens, 30);
    assert_eq!(done.usage.output_tokens, 9);
    let cost = done.usage.cost_usd.unwrap();
    assert!(
        (cost - (30.0 * 1.0 + 9.0 * 2.0) / 1e6).abs() < 1e-12,
        "{cost}"
    );
}

fn anthropic_event(kind: &'static str, data: Value) -> (Option<&'static str>, Value) {
    (Some(kind), data)
}

#[tokio::test(flavor = "multi_thread")]
async fn anthropic_sends_thinking_and_tool_use_back_unchanged() {
    let api = FakeApi::start(|_, index| match index {
        0 => Reply::sse(vec![
            anthropic_event("message_start", json!({"type": "message_start", "message": {
                "model": "claude-test", "usage": {"input_tokens": 12, "cache_read_input_tokens": 3, "output_tokens": 1}}})),
            anthropic_event("content_block_start", json!({"type": "content_block_start", "index": 0,
                "content_block": {"type": "thinking", "thinking": "", "signature": ""}})),
            anthropic_event("content_block_delta", json!({"type": "content_block_delta", "index": 0,
                "delta": {"type": "thinking_delta", "thinking": "pensando"}})),
            anthropic_event("content_block_delta", json!({"type": "content_block_delta", "index": 0,
                "delta": {"type": "signature_delta", "signature": "sig123"}})),
            anthropic_event("content_block_stop", json!({"type": "content_block_stop", "index": 0})),
            anthropic_event("content_block_start", json!({"type": "content_block_start", "index": 1,
                "content_block": {"type": "text", "text": ""}})),
            anthropic_event("content_block_delta", json!({"type": "content_block_delta", "index": 1,
                "delta": {"type": "text_delta", "text": "Vou ler."}})),
            anthropic_event("content_block_stop", json!({"type": "content_block_stop", "index": 1})),
            anthropic_event("content_block_start", json!({"type": "content_block_start", "index": 2,
                "content_block": {"type": "tool_use", "id": "toolu_1", "name": "filesystem__read", "input": {}}})),
            anthropic_event("content_block_delta", json!({"type": "content_block_delta", "index": 2,
                "delta": {"type": "input_json_delta", "partial_json": "{\"path\":"}})),
            anthropic_event("content_block_delta", json!({"type": "content_block_delta", "index": 2,
                "delta": {"type": "input_json_delta", "partial_json": "\"hello.txt\"}"}})),
            anthropic_event("content_block_stop", json!({"type": "content_block_stop", "index": 2})),
            anthropic_event("message_delta", json!({"type": "message_delta",
                "delta": {"stop_reason": "tool_use"}, "usage": {"output_tokens": 30}})),
            anthropic_event("message_stop", json!({"type": "message_stop"})),
        ]),
        _ => Reply::sse(vec![
            anthropic_event("message_start", json!({"type": "message_start", "message": {
                "model": "claude-test", "usage": {"input_tokens": 40, "output_tokens": 1}}})),
            anthropic_event("content_block_start", json!({"type": "content_block_start", "index": 0,
                "content_block": {"type": "text", "text": ""}})),
            anthropic_event("content_block_delta", json!({"type": "content_block_delta", "index": 0,
                "delta": {"type": "text_delta", "text": "Feito."}})),
            anthropic_event("message_delta", json!({"type": "message_delta",
                "delta": {"stop_reason": "end_turn"}, "usage": {"output_tokens": 5}})),
            anthropic_event("message_stop", json!({"type": "message_stop"})),
        ]),
    })
    .await;
    let h = Harness::new();
    h.add(
        connection(json!({
            "id": "claude", "name": "Claude", "kind": "anthropic",
            "baseUrl": api.url("/v1"), "credential": {"source": "vault"},
            "models": [{"id": "claude-test"}]
        })),
        Some("sk-ant"),
    )
    .await;
    let session = h.start("claude").await;
    let done = h.turn(&session.id, "leia").await;
    assert_eq!(h.last_turn(&session.id), (TurnStatus::Completed, None));
    assert_eq!(h.text(&session.id), "Vou ler.|Feito.");
    assert!(h
        .events(&session.id)
        .iter()
        .any(|e| matches!(e, SessionEvent::ReasoningDelta { text, .. } if text == "pensando")));

    let calls = api.calls("/v1/messages");
    assert_eq!(calls.len(), 2);
    let first = &calls[0];
    assert_eq!(first.headers["x-api-key"], "sk-ant");
    assert_eq!(first.headers["anthropic-version"], "2023-06-01");
    assert_eq!(first.body["max_tokens"], 64000);
    assert!(first.body["system"]
        .as_str()
        .unwrap()
        .contains("Orchestrator"));
    assert!(
        first.body.get("fallbacks").is_none(),
        "only on api.anthropic.com"
    );
    let tool = first.body["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["name"] == "filesystem__read")
        .unwrap();
    assert_eq!(tool["input_schema"]["type"], "object");
    assert_eq!(tool["eager_input_streaming"], true);

    let messages = &calls[1].body["messages"];
    assert_eq!(
        messages[1],
        json!({"role": "assistant", "content": [
            {"type": "thinking", "thinking": "pensando", "signature": "sig123"},
            {"type": "text", "text": "Vou ler."},
            {"type": "tool_use", "id": "toolu_1", "name": "filesystem__read", "input": {"path": "hello.txt"}}
        ]}),
        "assistant content goes back unchanged, thinking signature included"
    );
    assert_eq!(messages[2]["role"], "user");
    assert_eq!(messages[2]["content"][0]["type"], "tool_result");
    assert_eq!(messages[2]["content"][0]["tool_use_id"], "toolu_1");
    assert_eq!(messages[2]["content"][0]["is_error"], false);

    assert_eq!(done.usage.input_tokens, 15 + 40);
    assert_eq!(done.usage.cached_input_tokens, 3);
    assert_eq!(done.usage.output_tokens, 35);
}

#[tokio::test(flavor = "multi_thread")]
async fn anthropic_refusal_fails_the_turn_without_running_anything() {
    let api = FakeApi::start(|_, _| {
        Reply::sse(vec![
            anthropic_event("message_start", json!({"type": "message_start", "message": {
                "model": "claude-test", "usage": {"input_tokens": 5}}})),
            anthropic_event("message_delta", json!({"type": "message_delta",
                "delta": {"stop_reason": "refusal", "stop_details": {"type": "refusal", "category": "cyber"}},
                "usage": {"output_tokens": 0}})),
            anthropic_event("message_stop", json!({"type": "message_stop"})),
        ])
    })
    .await;
    let h = Harness::new();
    h.add(
        connection(json!({
            "id": "claude", "name": "Claude", "kind": "anthropic",
            "baseUrl": api.url("/v1"), "models": [{"id": "claude-test"}]
        })),
        None,
    )
    .await;
    let session = h.start("claude").await;
    let info = h.turn(&session.id, "algo").await;
    let (status, error) = h.last_turn(&session.id);
    assert_eq!(status, TurnStatus::Failed);
    assert!(error.unwrap().contains("declined the request (cyber)"));
    assert!(info.last_error.is_some());
    assert!(h.audits(EventKind::ToolCalled).is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn gemini_keeps_thought_signatures_and_uses_its_schema_dialect() {
    let api = FakeApi::start(|_, index| match index {
        0 => Reply::sse(vec![(
            None,
            json!({"candidates": [{"content": {"role": "model", "parts": [
                {"text": "Vou ler"},
                {"functionCall": {"name": "filesystem__read", "args": {"path": "hello.txt"}}, "thoughtSignature": "abc"}
            ]}, "finishReason": "STOP"}],
            "usageMetadata": {"promptTokenCount": 7, "candidatesTokenCount": 3, "thoughtsTokenCount": 2}}),
        )]),
        _ => Reply::sse(vec![(
            None,
            json!({"candidates": [{"content": {"role": "model", "parts": [{"text": "ok"}]}, "finishReason": "STOP"}],
            "usageMetadata": {"promptTokenCount": 20, "candidatesTokenCount": 1}}),
        )]),
    })
    .await;
    let h = Harness::new();
    h.add(
        connection(json!({
            "id": "gemini", "name": "Gemini", "kind": "gemini",
            "baseUrl": api.url("/v1beta"), "credential": {"source": "vault"},
            "models": [{"id": "gemini-test"}]
        })),
        Some("g-key"),
    )
    .await;
    let session = h.start("gemini").await;
    let done = h.turn(&session.id, "leia").await;
    assert_eq!(h.last_turn(&session.id), (TurnStatus::Completed, None));

    let calls = api.calls("/v1beta/models/gemini-test:streamGenerateContent?alt=sse");
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].headers["x-goog-api-key"], "g-key");
    let body = calls[0].body.to_string();
    assert!(
        !body.contains("additionalProperties"),
        "Gemini schema subset"
    );
    assert!(calls[0].body["systemInstruction"]["parts"][0]["text"].is_string());

    let contents = &calls[1].body["contents"];
    assert_eq!(contents[1]["role"], "model");
    assert_eq!(contents[1]["parts"][1]["thoughtSignature"], "abc");
    let response = &contents[2]["parts"][0]["functionResponse"];
    assert_eq!(response["name"], "filesystem__read");
    assert!(response["response"]["output"]["content"]
        .as_str()
        .unwrap()
        .contains("olá do projeto"));
    assert_eq!(done.usage.output_tokens, 5 + 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_generic_api_works_with_the_prompt_tool_protocol() {
    let api = FakeApi::start(|_, index| match index {
        0 => Reply::ndjson(vec![
            json!({"message": {"content": "Vou ler <tool_"}}),
            json!({"message": {"content": "call>{\"tool\":\"filesystem.read\",\"args\":{\"path\":\"hello.txt\"}}</tool_call>"}}),
            json!({"done": true, "prompt_eval_count": 5, "eval_count": 7}),
        ]),
        _ => Reply::ndjson(vec![
            json!({"message": {"content": "Conteúdo: olá"}}),
            json!({"done": true, "prompt_eval_count": 9, "eval_count": 2}),
        ]),
    })
    .await;
    let h = Harness::new();
    let mut preset = orchestrator_provider_api::presets()
        .into_iter()
        .find(|p| p.key == "ollama-native")
        .unwrap()
        .connection;
    preset.base_url = api.url("");
    preset.models = vec![orchestrator_provider_api::ModelEntry::new("llama-test")];
    h.add(preset, None).await;
    let session = h.start("ollama-nativo").await;
    let done = h.turn(&session.id, "leia").await;
    assert_eq!(h.last_turn(&session.id), (TurnStatus::Completed, None));
    let shown = h.text(&session.id);
    assert!(!shown.contains("tool_call"), "markup hidden: {shown}");
    assert!(
        shown.contains("Vou ler") && shown.contains("Conteúdo: olá"),
        "{shown}"
    );

    let calls = api.calls("/api/chat");
    assert_eq!(calls.len(), 2);
    let system = calls[0].body["messages"][0]["content"].as_str().unwrap();
    assert!(system.contains("<tool_call>") && system.contains("filesystem.read"));
    assert_eq!(calls[0].body["stream"], true);
    let messages = calls[1].body["messages"].as_array().unwrap();
    let assistant = messages[messages.len() - 2]["content"].as_str().unwrap();
    assert!(
        assistant.contains("<tool_call>"),
        "the model sees its own call"
    );
    let results = messages.last().unwrap()["content"].as_str().unwrap();
    assert!(results.contains("<tool_result tool=\"filesystem.read\" ok=\"true\">"));
    assert!(results.contains("olá do projeto"));
    assert_eq!(done.usage.input_tokens, 14);
    assert!(h
        .audits(EventKind::ToolCalled)
        .iter()
        .any(|e| e.data["tool"] == "filesystem.read"));
}

#[tokio::test(flavor = "multi_thread")]
async fn http_errors_become_readable_failures() {
    let api = FakeApi::start(|request, _| {
        if request.path.ends_with("/models") {
            Reply::status(
                401,
                json!({"error": {"message": "Incorrect API key provided"}}),
            )
        } else {
            Reply::status(429, json!({"error": {"message": "Rate limit reached"}}))
        }
    })
    .await;
    let h = Harness::new();
    h.add(
        connection(json!({
            "id": "limitada", "name": "Limitada", "kind": "openai",
            "baseUrl": api.url("/v1"), "credential": {"source": "vault"},
            "models": [{"id": "m"}]
        })),
        Some("sk-bad"),
    )
    .await;
    let session = h.start("limitada").await;
    h.turn(&session.id, "oi").await;
    let (status, error) = h.last_turn(&session.id);
    assert_eq!(status, TurnStatus::Failed);
    let error = error.unwrap();
    assert!(
        error.contains("HTTP 429") && error.contains("Rate limit reached"),
        "{error}"
    );

    let provider = h.registry.get(&"limitada".into()).unwrap();
    let status = provider.inspect().await;
    assert!(!status.available);
    assert_eq!(status.authenticated, Some(false));
    assert!(status.detail.unwrap().contains("Incorrect API key"));

    // Nothing listening.
    let closed = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = closed.local_addr().unwrap().port();
    drop(closed);
    h.add(
        connection(json!({
            "id": "offline", "name": "Offline", "kind": "openai",
            "baseUrl": format!("http://127.0.0.1:{port}/v1"), "models": [{"id": "m"}]
        })),
        None,
    )
    .await;
    let session = h.start("offline").await;
    h.turn(&session.id, "oi").await;
    let (_, error) = h.last_turn(&session.id);
    assert!(error.unwrap().contains("cannot reach 127.0.0.1"));
}

#[tokio::test(flavor = "multi_thread")]
async fn missing_credentials_are_reported() {
    let h = Harness::new();
    h.add(
        connection(json!({
            "id": "sem-chave", "name": "Sem chave", "kind": "openai",
            "baseUrl": "http://127.0.0.1:9/v1", "credential": {"source": "vault"},
            "models": [{"id": "m"}]
        })),
        None,
    )
    .await;
    h.add(
        connection(json!({
            "id": "env", "name": "Env", "kind": "openai",
            "baseUrl": "http://127.0.0.1:9/v1",
            "credential": {"source": "env", "envVar": "ORCHESTRATOR_TEST_UNSET_KEY"},
            "models": [{"id": "m"}]
        })),
        None,
    )
    .await;
    for (id, expected) in [
        ("sem-chave", "no API key stored"),
        ("env", "ORCHESTRATOR_TEST_UNSET_KEY is not set"),
    ] {
        let session = h.start(id).await;
        h.turn(&session.id, "oi").await;
        let (_, error) = h.last_turn(&session.id);
        assert!(
            error.as_deref().unwrap_or("").contains(expected),
            "{error:?}"
        );
    }
    let views = h.connections.list().await;
    assert!(views.iter().all(|v| !v.key.present));
}

#[tokio::test(flavor = "multi_thread")]
async fn models_are_discovered_for_each_protocol() {
    let api = FakeApi::start(|request, _| match request.path.as_str() {
        "/openrouter/v1/models" => Reply::json(json!({"data": [
            {"id": "vendor/model-a", "name": "Model A", "context_length": 128000,
             "pricing": {"prompt": "0.000002", "completion": "0.000008"}}
        ]})),
        "/anthropic/v1/models?limit=1000" => Reply::json(json!({
            "data": [{"id": "claude-opus-5-5", "display_name": "Claude Opus 5.5", "max_input_tokens": 1000000, "max_tokens": 128000}],
            "has_more": true, "last_id": "claude-opus-5-5"
        })),
        "/anthropic/v1/models?limit=1000&after_id=claude-opus-5-5" => Reply::json(json!({
            "data": [{"id": "claude-haiku-4-5", "display_name": "Claude Haiku 4.5"}], "has_more": false
        })),
        "/gemini/v1beta/models?pageSize=1000" => Reply::json(json!({"models": [
            {"name": "models/gemini-x", "displayName": "Gemini X", "inputTokenLimit": 1048576,
             "supportedGenerationMethods": ["generateContent", "countTokens"]},
            {"name": "models/embedding-x", "supportedGenerationMethods": ["embedContent"]}
        ]})),
        "/api/tags" => Reply::json(json!({"models": [{"name": "llama3:8b"}, {"name": "qwen:7b"}]})),
        other => Reply::status(404, json!({"error": format!("unexpected {other}")})),
    })
    .await;
    let h = Harness::new();
    let probe = |conn: Value| ProbeRequest {
        connection: connection(conn),
        api_key: Some("k".into()),
        model: None,
    };

    let openrouter = h
        .connections
        .models(probe(json!({"id": "or", "name": "OR", "kind": "openai", "baseUrl": api.url("/openrouter/v1")})))
        .await
        .unwrap();
    assert_eq!(openrouter[0].id, "vendor/model-a");
    assert_eq!(openrouter[0].context_window, Some(128_000));
    assert!((openrouter[0].input_price.unwrap() - 2.0).abs() < 1e-9);
    assert!((openrouter[0].output_price.unwrap() - 8.0).abs() < 1e-9);

    let claude = h
        .connections
        .models(probe(json!({"id": "c", "name": "C", "kind": "anthropic", "baseUrl": api.url("/anthropic/v1")})))
        .await
        .unwrap();
    let ids: Vec<_> = claude.iter().map(|m| m.id.as_str()).collect();
    assert_eq!(
        ids,
        vec!["claude-haiku-4-5", "claude-opus-5-5"],
        "both pages"
    );
    let opus = claude.iter().find(|m| m.id == "claude-opus-5-5").unwrap();
    assert_eq!(opus.input_price, Some(4.0), "reference price filled");
    assert_eq!(opus.context_window, Some(1_000_000));

    let gemini = h
        .connections
        .models(probe(
            json!({"id": "g", "name": "G", "kind": "gemini", "baseUrl": api.url("/gemini/v1beta")}),
        ))
        .await
        .unwrap();
    assert_eq!(gemini.len(), 1);
    assert_eq!(gemini[0].id, "gemini-x");

    let mut ollama = orchestrator_provider_api::presets()
        .into_iter()
        .find(|p| p.key == "ollama-native")
        .unwrap()
        .connection;
    ollama.base_url = api.url("");
    let local = h
        .connections
        .models(ProbeRequest {
            connection: ollama,
            api_key: None,
            model: None,
        })
        .await
        .unwrap();
    assert_eq!(local.len(), 2);

    // A generic API without a models endpoint says so.
    let err = h
        .connections
        .models(probe(json!({
            "id": "x", "name": "X", "kind": "generic", "baseUrl": api.url(""),
            "generic": {"textPath": "text"}
        })))
        .await
        .unwrap_err();
    assert_eq!(err.kind, ProviderErrorKind::Unsupported);
}

#[tokio::test(flavor = "multi_thread")]
async fn the_manager_persists_connections_but_never_keys() {
    let h = Harness::new();
    let conn = || {
        connection(json!({
            "id": "openai-pessoal", "name": "OpenAI pessoal", "kind": "openai",
            "baseUrl": "https://api.openai.com/v1", "credential": {"source": "vault"},
            "models": [{"id": "m1"}]
        }))
    };
    h.add(conn(), Some("sk-secret-123")).await;
    let path = h.dir.path().join("data/connections.json");
    let file = std::fs::read_to_string(&path).unwrap();
    assert!(file.contains("openai-pessoal") && !file.contains("sk-secret-123"));
    assert_eq!(
        h.secrets.get("openai-pessoal").unwrap().as_deref(),
        Some("sk-secret-123")
    );
    assert!(h.registry.get(&"openai-pessoal".into()).is_some());
    let views = h.connections.list().await;
    assert!(views[0].key.present);

    // Reopening registers it again.
    let registry = std::sync::Arc::new(orchestrator_providers::ProviderRegistry::new(
        h.sink.clone(),
    ));
    let (_reopened, warnings) =
        ConnectionManager::open(&path, registry.clone(), h.secrets.clone(), h.sink.clone())
            .unwrap();
    assert!(warnings.is_empty());
    assert!(registry.get(&"openai-pessoal".into()).is_some());

    // Rename keeps the key under the new id.
    let mut renamed = conn();
    renamed.id = "openai-casa".into();
    h.connections
        .save(
            SaveRequest {
                connection: renamed,
                api_key: None,
                clear_key: false,
                previous_id: Some("openai-pessoal".into()),
            },
            CallOrigin::User,
        )
        .await
        .unwrap();
    assert!(h.registry.get(&"openai-pessoal".into()).is_none());
    assert!(h.registry.get(&"openai-casa".into()).is_some());
    assert_eq!(h.secrets.get("openai-pessoal").unwrap(), None);
    assert_eq!(
        h.secrets.get("openai-casa").unwrap().as_deref(),
        Some("sk-secret-123")
    );

    // Conflicts and invalid input are rejected.
    let err = h
        .connections
        .save(
            SaveRequest {
                connection: connection(
                    json!({"id": "x", "name": "", "kind": "openai", "baseUrl": "https://a"}),
                ),
                api_key: None,
                clear_key: false,
                previous_id: None,
            },
            CallOrigin::User,
        )
        .await
        .unwrap_err();
    assert_eq!(err.kind, ProviderErrorKind::InvalidRequest);

    h.connections
        .remove("openai-casa", CallOrigin::User)
        .await
        .unwrap();
    assert!(h.registry.get(&"openai-casa".into()).is_none());
    assert_eq!(h.secrets.get("openai-casa").unwrap(), None);
    assert!(h.connections.list().await.is_empty());

    let saved = h.audits(EventKind::ConnectionSaved);
    assert_eq!(saved.len(), 2);
    assert_eq!(h.audits(EventKind::ConnectionRemoved).len(), 1);
    let history = serde_json::to_string(&h.sink.audit_events()).unwrap();
    assert!(
        !history.contains("sk-secret-123"),
        "keys never reach the history"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn test_connection_checks_text_and_tool_calls() {
    let api = FakeApi::start(|request, _| {
        let asked_tool = request.body["tools"].is_array();
        if asked_tool {
            Reply::sse(vec![
                openai_chunk(
                    json!({"tool_calls": [{"index": 0, "id": "c1",
                        "function": {"name": "orchestrator__ping", "arguments": "{\"value\":\"orchestrator\"}"}}]}),
                    Some("tool_calls"),
                ),
                openai_usage(3, 3),
            ])
            .sse_done()
        } else {
            Reply::sse(vec![
                openai_chunk(json!({"content": "OK"}), Some("stop")),
                openai_usage(2, 1),
            ])
            .sse_done()
        }
    })
    .await;
    let h = Harness::new();
    let report = h
        .connections
        .test(ProbeRequest {
            connection: connection(json!({
                "id": "nova", "name": "Nova", "kind": "openai", "baseUrl": api.url("/v1"),
                "credential": {"source": "vault"}, "models": [{"id": "m"}]
            })),
            api_key: Some("typed-in-form".into()),
            model: None,
        })
        .await
        .unwrap();
    assert!(report.ok, "{report:?}");
    assert_eq!(report.reply.as_deref(), Some("OK"));
    assert_eq!(report.tools, "passed");
    assert_eq!(
        api.requests()[0].headers["authorization"],
        "Bearer typed-in-form"
    );
    assert!(
        h.audits(EventKind::ToolCalled).is_empty(),
        "the test tool never runs"
    );
    // The typed key was not stored.
    assert_eq!(h.secrets.get("nova").unwrap(), None);
}

#[tokio::test(flavor = "multi_thread")]
async fn the_round_limit_ends_the_turn_with_a_notice() {
    let api = FakeApi::start(|_, index| {
        Reply::sse(vec![
            openai_chunk(
                json!({"tool_calls": [{"index": 0, "id": format!("c{index}"),
                    "function": {"name": "process__list", "arguments": "{}"}}]}),
                Some("tool_calls"),
            ),
            openai_usage(1, 1),
        ])
        .sse_done()
    })
    .await;
    let h = Harness::new();
    h.add(
        connection(json!({
            "id": "loop", "name": "Loop", "kind": "openai", "baseUrl": api.url("/v1"),
            "models": [{"id": "m"}], "maxToolRounds": 2
        })),
        None,
    )
    .await;
    let session = h.start("loop").await;
    h.turn(&session.id, "vai").await;
    assert_eq!(h.last_turn(&session.id).0, TurnStatus::Completed);
    assert_eq!(api.calls("/v1/chat/completions").len(), 2);
    assert!(h.events(&session.id).iter().any(|e| matches!(
        e,
        SessionEvent::Notice { message, .. } if message.contains("maxToolRounds")
    )));
}

#[tokio::test(flavor = "multi_thread")]
async fn invalid_tool_arguments_go_back_as_errors_without_running() {
    let api = FakeApi::start(|_, index| match index {
        0 => Reply::sse(vec![openai_chunk(
            json!({"tool_calls": [{"index": 0, "id": "c1",
                "function": {"name": "filesystem__read", "arguments": "{\"path\": "}}]}),
            Some("tool_calls"),
        )])
        .sse_done(),
        _ => Reply::sse(vec![openai_chunk(
            json!({"content": "desculpe"}),
            Some("stop"),
        )])
        .sse_done(),
    })
    .await;
    let h = Harness::new();
    h.add(
        connection(json!({
            "id": "o", "name": "O", "kind": "openai", "baseUrl": api.url("/v1"), "models": [{"id": "m"}]
        })),
        None,
    )
    .await;
    let session = h.start("o").await;
    h.turn(&session.id, "vai").await;
    assert_eq!(h.last_turn(&session.id).0, TurnStatus::Completed);
    let second = &api.calls("/v1/chat/completions")[1].body["messages"];
    let tool = second
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["role"] == "tool")
        .unwrap();
    assert!(tool["content"]
        .as_str()
        .unwrap()
        .starts_with("INVALID_ARGS"));
    assert!(h.audits(EventKind::ToolCalled).is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn cancel_stops_a_streaming_reply() {
    let api = FakeApi::start(|_, _| {
        let chunks: Vec<_> = (0..40)
            .map(|i| openai_chunk(json!({"content": format!("parte {i} ")}), None))
            .collect();
        Reply::sse(chunks).slow(Duration::from_millis(200))
    })
    .await;
    let h = Harness::new();
    h.add(
        connection(json!({
            "id": "lenta", "name": "Lenta", "kind": "openai", "baseUrl": api.url("/v1"), "models": [{"id": "m"}]
        })),
        None,
    )
    .await;
    let session = h.start("lenta").await;
    h.sessions
        .send(&session.id, "conte".into(), CallOrigin::User)
        .await
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while !h.text(&session.id).contains("parte 1") {
        assert!(Instant::now() < deadline, "no streamed text");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let started = Instant::now();
    h.sessions.cancel(&session.id).await.unwrap();
    h.idle(&session.id).await;
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "{:?}",
        started.elapsed()
    );
    assert_eq!(h.last_turn(&session.id).0, TurnStatus::Cancelled);
    assert!(!h.text(&session.id).contains("parte 39"));
}

#[tokio::test(flavor = "multi_thread")]
async fn editing_a_connection_keeps_open_sessions_and_removing_it_ends_them() {
    let api = FakeApi::start(|_, index| {
        Reply::sse(vec![
            openai_chunk(
                json!({"content": format!("resposta {index}")}),
                Some("stop"),
            ),
            openai_usage(1, 1),
        ])
        .sse_done()
    })
    .await;
    let h = Harness::new();
    let original = connection(json!({
        "id": "edit", "name": "Antes", "kind": "openai", "baseUrl": api.url("/v1"),
        "credential": {"source": "vault"},
        "models": [{"id": "m1"}, {"id": "m2"}], "defaultModel": "m1"
    }));
    h.add(original.clone(), Some("chave-1")).await;
    let session = h.start("edit").await;
    h.turn(&session.id, "primeira").await;

    // New key and an extra header: the open session uses them next turn,
    // with the conversation so far.
    let mut edited = original.clone();
    edited.name = "Depois".into();
    edited.headers.insert("x-edited".into(), "sim".into());
    h.connections
        .save(
            SaveRequest {
                connection: edited,
                api_key: Some("chave-2".into()),
                clear_key: false,
                previous_id: Some("edit".into()),
            },
            CallOrigin::User,
        )
        .await
        .unwrap();
    h.turn(&session.id, "segunda").await;
    assert_eq!(h.last_turn(&session.id).0, TurnStatus::Completed);
    let calls = api.calls("/v1/chat/completions");
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].headers["authorization"], "Bearer chave-1");
    assert_eq!(calls[1].headers["authorization"], "Bearer chave-2");
    assert_eq!(calls[1].headers["x-edited"], "sim");
    let contents: Vec<&str> = calls[1].body["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|m| m["content"].as_str())
        .collect();
    assert!(
        contents.ends_with(&["primeira", "resposta 0", "segunda"]),
        "{contents:?}"
    );

    // Removed: the next turn fails clearly and nothing is sent.
    h.connections
        .remove("edit", CallOrigin::User)
        .await
        .unwrap();
    let err = h
        .sessions
        .send(&session.id, "terceira".into(), CallOrigin::User)
        .await
        .unwrap_err();
    assert_eq!(err.kind, ProviderErrorKind::Unavailable);
    assert!(
        err.message.contains("no longer registered"),
        "{}",
        err.message
    );
    assert_eq!(api.calls("/v1/chat/completions").len(), 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn complete_answers_once_without_tools_or_history() {
    let api = FakeApi::start(|request, _| {
        if request.path.ends_with("/messages") {
            Reply::sse(vec![
                anthropic_event(
                    "message_start",
                    json!({"type": "message_start", "message": {
                    "model": "claude-served", "usage": {"input_tokens": 40, "output_tokens": 1}}}),
                ),
                anthropic_event(
                    "content_block_start",
                    json!({"type": "content_block_start", "index": 0,
                    "content_block": {"type": "text", "text": ""}}),
                ),
                anthropic_event(
                    "content_block_delta",
                    json!({"type": "content_block_delta", "index": 0,
                    "delta": {"type": "text_delta", "text": "{\"choice\":\"c1\"}"}}),
                ),
                anthropic_event(
                    "content_block_stop",
                    json!({"type": "content_block_stop", "index": 0}),
                ),
                anthropic_event(
                    "message_delta",
                    json!({"type": "message_delta",
                    "delta": {"stop_reason": "end_turn"}, "usage": {"output_tokens": 8}}),
                ),
                anthropic_event("message_stop", json!({"type": "message_stop"})),
            ])
        } else {
            Reply::sse(vec![
                openai_chunk(json!({"role": "assistant", "content": "resposta"}), None),
                openai_chunk(json!({}), Some("stop")),
                openai_usage(30, 2),
            ])
            .sse_done()
        }
    })
    .await;
    let h = Harness::new();
    h.add(
        connection(json!({
            "id": "claude", "name": "Claude", "kind": "anthropic",
            "baseUrl": api.url("/v1"),
            "models": [{"id": "claude-test", "inputPrice": 3.0, "outputPrice": 15.0}]
        })),
        None,
    )
    .await;
    h.add(
        connection(json!({
            "id": "local", "name": "Local", "kind": "openai", "baseUrl": api.url("/v1"),
            "models": [{"id": "small"}, {"id": "large"}]
        })),
        None,
    )
    .await;
    let cancel = tokio_util::sync::CancellationToken::new();

    let claude = h.registry.get(&"claude".into()).unwrap();
    assert!(claude.capabilities().completion);
    let request = CompletionRequest {
        model: None,
        system: Some("Você é o gerenciador.".into()),
        prompt: "Qual candidato?".into(),
    };
    let answer = claude.complete(&request, &cancel).await.unwrap();
    assert_eq!(answer.text, "{\"choice\":\"c1\"}");
    assert_eq!(answer.model.as_deref(), Some("claude-served"));
    assert_eq!(answer.usage.input_tokens, 40);
    assert_eq!(answer.usage.output_tokens, 8);
    let cost = answer.usage.cost_usd.unwrap();
    assert!(
        (cost - (40.0 * 3.0 + 8.0 * 15.0) / 1e6).abs() < 1e-12,
        "{cost}"
    );
    let sent = &api.calls("/v1/messages")[0].body;
    assert_eq!(sent["system"], "Você é o gerenciador.");
    assert_eq!(sent["messages"].as_array().unwrap().len(), 1);
    assert!(sent.get("tools").is_none(), "no tools on a completion");

    // Model override, and nothing is kept between completions.
    let local = h.registry.get(&"local".into()).unwrap();
    for _ in 0..2 {
        let answer = local
            .complete(
                &CompletionRequest {
                    model: Some("large".into()),
                    system: None,
                    prompt: "oi".into(),
                },
                &cancel,
            )
            .await
            .unwrap();
        assert_eq!(answer.text, "resposta");
        assert_eq!(answer.usage.cost_usd, None, "no prices configured");
    }
    let calls = api.calls("/v1/chat/completions");
    assert_eq!(calls.len(), 2);
    for call in &calls {
        assert_eq!(call.body["model"], "large");
        assert_eq!(call.body["messages"].as_array().unwrap().len(), 1);
        assert!(call.body.get("tools").is_none());
    }
    // No session, no turn, no tool call in the history.
    assert!(h.sessions.list().is_empty());
    assert!(h.audits(EventKind::TurnCompleted).is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_conversation_continues_after_a_restart() {
    let api = FakeApi::start(|_, index| {
        Reply::sse(vec![
            openai_chunk(json!({"content": format!("resposta {index}")}), None),
            openai_chunk(json!({}), Some("stop")),
            openai_usage(5, 2),
        ])
        .sse_done()
    })
    .await;
    let conn = json!({
        "id": "local", "name": "Local", "kind": "openai", "baseUrl": api.url("/v1"),
        "credential": {"source": "vault"}, "models": [{"id": "gpt-test"}]
    });
    let store: Arc<dyn SessionStore> = Arc::new(MemorySessionStore::new());

    let first = Harness::with_store(store.clone());
    first.add(connection(conn.clone()), Some("sk-test")).await;
    let session = first.start("local").await;
    first.turn(&session.id, "primeiro").await;
    drop(first);

    // "Restart": new registry, new connection manager, empty memory.
    let second = Harness::with_store(store.clone());
    second.add(connection(conn), Some("sk-test")).await;
    let restored = second.sessions.info(&session.id).unwrap();
    assert_eq!(restored.status, SessionStatus::Closed);
    second
        .sessions
        .resume(&session.id, CallOrigin::User)
        .await
        .unwrap();
    second.turn(&session.id, "segundo").await;
    assert_eq!(second.last_turn(&session.id), (TurnStatus::Completed, None));

    let calls = api.calls("/v1/chat/completions");
    assert_eq!(calls.len(), 2);
    let messages = calls[1].body["messages"].as_array().unwrap();
    let said: Vec<_> = messages
        .iter()
        .map(|m| {
            format!(
                "{}: {}",
                m["role"].as_str().unwrap(),
                m["content"].as_str().unwrap_or_default()
            )
        })
        .collect();
    assert_eq!(said.len(), 4, "{said:?}");
    assert!(said[0].starts_with("system: You are an AI agent"));
    assert_eq!(
        &said[1..],
        ["user: primeiro", "assistant: resposta 0", "user: segundo"]
    );
}

/// Answers every session with the same short context (ADR-0013).
struct FixedContext;

#[async_trait::async_trait]
impl ContextSource for FixedContext {
    async fn build(&self, request: ContextRequest) -> Result<Option<AttachedContext>, String> {
        Ok(Some(AttachedContext {
            text: format!(
                "## TASK\n{}\n\n## GIT STATE\nbranch main\ntools: {}",
                request.task, request.tools
            ),
            summary: ContextSummary::default(),
        }))
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn the_project_context_joins_the_system_instructions_once() {
    let api = FakeApi::start(|_, index| {
        Reply::sse(vec![
            openai_chunk(json!({"content": format!("resposta {index}")}), None),
            openai_chunk(json!({}), Some("stop")),
            openai_usage(5, 2),
        ])
        .sse_done()
    })
    .await;
    let conn = json!({
        "id": "local", "name": "Local", "kind": "openai", "baseUrl": api.url("/v1"),
        "credential": {"source": "vault"}, "models": [{"id": "gpt-test"}]
    });
    let store: Arc<dyn SessionStore> = Arc::new(MemorySessionStore::new());
    let first = Harness::with_store(store.clone());
    first.sessions.set_context_source(Arc::new(FixedContext));
    first.add(connection(conn.clone()), Some("sk-test")).await;
    let session = first.start("local").await;
    first.turn(&session.id, "corrigir o checkout").await;
    first.turn(&session.id, "e os testes?").await;
    drop(first);

    // After a restart the conversation keeps the same instructions.
    let second = Harness::with_store(store.clone());
    second.sessions.set_context_source(Arc::new(FixedContext));
    second.add(connection(conn), Some("sk-test")).await;
    second
        .sessions
        .resume(&session.id, CallOrigin::User)
        .await
        .unwrap();
    second.turn(&session.id, "terminou?").await;

    let calls = api.calls("/v1/chat/completions");
    assert_eq!(calls.len(), 3);
    let system = |i: usize| {
        calls[i].body["messages"][0]["content"]
            .as_str()
            .unwrap()
            .to_owned()
    };
    assert!(system(0).starts_with("You are an AI agent"));
    assert!(system(0)
        .ends_with("## TASK\ncorrigir o checkout\n\n## GIT STATE\nbranch main\ntools: true"));
    assert_eq!(
        system(1),
        system(0),
        "later turns do not rebuild the context"
    );
    assert_eq!(system(2), system(0), "the context survives a restart");
    // The context is not repeated as a user message.
    let users: Vec<_> = calls[2].body["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|m| m["role"] == "user")
        .map(|m| m["content"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(users, ["corrigir o checkout", "e os testes?", "terminou?"]);

    // A connection without tools is built a context that does not point to
    // them.
    let offline = Harness::new();
    offline.sessions.set_context_source(Arc::new(FixedContext));
    offline
        .add(
            connection(json!({
                "id": "sem-ferramentas", "name": "Sem ferramentas", "kind": "openai",
                "baseUrl": api.url("/v1"), "credential": {"source": "vault"},
                "models": [{"id": "gpt-test"}], "toolMode": "none"
            })),
            Some("sk-test"),
        )
        .await;
    let session = offline.start("sem-ferramentas").await;
    offline.turn(&session.id, "oi").await;
    let calls = api.calls("/v1/chat/completions");
    let system = calls[3].body["messages"][0]["content"].as_str().unwrap();
    assert!(system.ends_with("tools: false"), "{system}");
}
