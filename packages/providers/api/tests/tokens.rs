//! Phase 11 (ADR-0018): the prompt cache, the real cost and the repeats of
//! a request the server did not take.

mod support;

use orchestrator_core::{NoticeLevel, SessionEvent, TurnStatus};
use serde_json::{json, Value};
use support::{connection, FakeApi, Harness, Reply};

fn event(kind: &'static str, data: Value) -> (Option<&'static str>, Value) {
    (Some(kind), data)
}

/// An Anthropic reply: text, or one `filesystem.read` call, with the usage
/// the cache would report.
fn anthropic_reply(tool: bool, read: u64, written: u64) -> Reply {
    let mut events = vec![event(
        "message_start",
        json!({"type": "message_start", "message": {"model": "claude-test", "usage": {
            "input_tokens": 100, "cache_read_input_tokens": read,
            "cache_creation_input_tokens": written, "output_tokens": 1}}}),
    )];
    if tool {
        events.extend([
            event("content_block_start", json!({"type": "content_block_start", "index": 0,
                "content_block": {"type": "tool_use", "id": "toolu_1", "name": "filesystem__read", "input": {}}})),
            event("content_block_delta", json!({"type": "content_block_delta", "index": 0,
                "delta": {"type": "input_json_delta", "partial_json": "{\"path\":\"hello.txt\"}"}})),
            event("content_block_stop", json!({"type": "content_block_stop", "index": 0})),
            event("message_delta", json!({"type": "message_delta",
                "delta": {"stop_reason": "tool_use"}, "usage": {"output_tokens": 20}})),
        ]);
    } else {
        events.extend([
            event(
                "content_block_start",
                json!({"type": "content_block_start", "index": 0,
                "content_block": {"type": "text", "text": ""}}),
            ),
            event(
                "content_block_delta",
                json!({"type": "content_block_delta", "index": 0,
                "delta": {"type": "text_delta", "text": "Pronto."}}),
            ),
            event(
                "message_delta",
                json!({"type": "message_delta",
                "delta": {"stop_reason": "end_turn"}, "usage": {"output_tokens": 10}}),
            ),
        ]);
    }
    events.push(event("message_stop", json!({"type": "message_stop"})));
    Reply::sse(events)
}

/// `value` without any `cache_control` marker: the marker moves forward
/// with the conversation and is not part of the cached bytes.
fn unmarked(value: &Value) -> Value {
    match value {
        Value::Object(map) => Value::Object(
            map.iter()
                .filter(|(k, _)| k.as_str() != "cache_control")
                .map(|(k, v)| (k.clone(), unmarked(v)))
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.iter().map(unmarked).collect()),
        other => other.clone(),
    }
}

fn marked(value: &Value) -> bool {
    value.get("cache_control") == Some(&json!({"type": "ephemeral"}))
}

async fn claude(h: &Harness, api: &FakeApi, options: Value) {
    h.add(
        connection(json!({
            "id": "claude", "name": "Claude", "kind": "anthropic",
            "baseUrl": api.url("/v1"), "credential": {"source": "vault"},
            "options": options,
            "models": [{"id": "claude-test", "inputPrice": 4.0, "outputPrice": 20.0,
                        "cachedInputPrice": 0.2}]
        })),
        Some("sk-ant"),
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn anthropic_requests_keep_a_stable_prefix_marked_for_the_cache() {
    // Turn 1: a tool round, then text. Turn 2: text.
    let api = FakeApi::start(|_, index| match index {
        0 => anthropic_reply(true, 0, 5_000),
        1 => anthropic_reply(false, 5_000, 300),
        _ => anthropic_reply(false, 5_300, 200),
    })
    .await;
    let h = Harness::new();
    claude(&h, &api, json!({})).await;
    let session = h.start("claude").await;
    h.turn(&session.id, "leia o arquivo").await;
    let info = h.turn(&session.id, "e agora?").await;
    assert_eq!(h.last_turn(&session.id), (TurnStatus::Completed, None));

    let calls = api.calls("/v1/messages");
    assert_eq!(calls.len(), 3);
    for call in &calls {
        let body = &call.body;
        // Tools (shared by every session of the model), system, conversation.
        let tools = body["tools"].as_array().unwrap();
        assert!(marked(tools.last().unwrap()), "last tool marked");
        assert!(tools[..tools.len() - 1].iter().all(|t| !marked(t)));
        assert!(marked(&body["system"][0]), "system marked");
        let messages = body["messages"].as_array().unwrap();
        let last = messages.last().unwrap()["content"].as_array().unwrap();
        assert!(marked(last.last().unwrap()), "conversation marked");
    }
    // Byte for byte, each request starts with the previous one.
    for pair in calls.windows(2) {
        let (a, b) = (unmarked(&pair[0].body), unmarked(&pair[1].body));
        assert_eq!(a["tools"], b["tools"]);
        assert_eq!(a["system"], b["system"]);
        let (a, b) = (
            a["messages"].as_array().unwrap(),
            b["messages"].as_array().unwrap(),
        );
        assert!(b.len() > a.len());
        assert_eq!(a[..], b[..a.len()], "append-only conversation");
    }

    // The cost is the vendor's: reads at the cache price, writes at 1.25×.
    let usage = info.usage;
    assert_eq!(usage.cached_input_tokens, 10_300);
    assert_eq!(usage.cache_write_tokens, 5_500);
    assert_eq!(usage.input_tokens, 300 + 10_300 + 5_500);
    let per = |tokens: f64, price: f64| tokens * price / 1e6;
    let cost = per(300.0, 4.0) + per(10_300.0, 0.2) + per(5_500.0, 5.0) + per(40.0, 20.0);
    assert!((usage.cost_usd.unwrap() - cost).abs() < 1e-9, "{usage:?}");
    let saved = per(15_800.0, 4.0) - per(10_300.0, 0.2) - per(5_500.0, 5.0);
    assert!((usage.cache_saved_usd.unwrap() - saved).abs() < 1e-9);
}

#[tokio::test(flavor = "multi_thread")]
async fn the_cache_can_be_turned_off_or_kept_for_an_hour() {
    let api = FakeApi::start(|_, _| anthropic_reply(false, 0, 0)).await;
    let h = Harness::new();
    claude(&h, &api, json!({"promptCache": false})).await;
    let session = h.start("claude").await;
    h.turn(&session.id, "oi").await;
    let body = &api.calls("/v1/messages")[0].body;
    assert!(body["system"].is_string(), "plain system prompt");
    assert_eq!(unmarked(body), *body, "no marker anywhere");

    let api = FakeApi::start(|_, _| anthropic_reply(false, 0, 0)).await;
    let h = Harness::new();
    claude(&h, &api, json!({"cacheTtl": "1h"})).await;
    let session = h.start("claude").await;
    h.turn(&session.id, "oi").await;
    let body = &api.calls("/v1/messages")[0].body;
    assert_eq!(
        body["system"][0]["cache_control"],
        json!({"type": "ephemeral", "ttl": "1h"})
    );
}

fn openai_text(text: &str) -> Reply {
    Reply::sse(vec![
        (
            None,
            json!({"model": "m", "choices": [{"index": 0,
            "delta": {"role": "assistant", "content": text}, "finish_reason": null}]}),
        ),
        (
            None,
            json!({"model": "m", "choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}]}),
        ),
        (
            None,
            json!({"choices": [], "usage": {"prompt_tokens": 10, "completion_tokens": 2}}),
        ),
    ])
    .sse_done()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_request_the_server_did_not_take_is_repeated() {
    // 429 (asks to wait 0 s), then 529 overload (asks 10 ms), then success.
    let api = FakeApi::start(|_, index| match index {
        0 => Reply::status(429, json!({"error": {"message": "Rate limit reached"}}))
            .header("retry-after", "0"),
        1 => Reply::status(529, json!({"error": {"message": "Overloaded"}}))
            .header("retry-after-ms", "10"),
        _ => openai_text("ok"),
    })
    .await;
    let h = Harness::new();
    h.add(
        connection(json!({
            "id": "o", "name": "O", "kind": "openai", "baseUrl": api.url("/v1"),
            "toolMode": "none", "models": [{"id": "m"}]
        })),
        None,
    )
    .await;
    let session = h.start("o").await;
    h.turn(&session.id, "oi").await;
    assert_eq!(h.last_turn(&session.id), (TurnStatus::Completed, None));
    assert_eq!(api.calls("/v1/chat").len(), 3);
    let notices: Vec<String> = h
        .events(&session.id)
        .into_iter()
        .filter_map(|e| match e {
            SessionEvent::Notice {
                level: NoticeLevel::Warning,
                message,
                ..
            } => Some(message),
            _ => None,
        })
        .collect();
    assert_eq!(notices.len(), 2, "{notices:?}");
    assert!(notices[0].contains("HTTP 429") && notices[0].contains("1 de 2"));
    assert!(notices[1].contains("HTTP 529") && notices[1].contains("2 de 2"));

    // A rejected request is not repeated.
    let api =
        FakeApi::start(|_, _| Reply::status(400, json!({"error": {"message": "bad field"}}))).await;
    let h = Harness::new();
    h.add(
        connection(json!({
            "id": "o", "name": "O", "kind": "openai", "baseUrl": api.url("/v1"),
            "models": [{"id": "m"}]
        })),
        None,
    )
    .await;
    let session = h.start("o").await;
    h.turn(&session.id, "oi").await;
    assert_eq!(h.last_turn(&session.id).0, TurnStatus::Failed);
    assert_eq!(api.calls("/v1/chat").len(), 1);
}
