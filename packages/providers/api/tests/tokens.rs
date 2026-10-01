//! Phase 11 (ADR-0018): the prompt cache, the real cost and the repeats of
//! a request the server did not take.

mod support;

use orchestrator_core::{CallOrigin, EventKind, NoticeLevel, SessionEvent, TurnStatus};
use orchestrator_providers::CompactionPolicy;
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

// ---- compaction ------------------------------------------------------

/// Whether an OpenAI request is the one asking for the summary.
fn asks_summary(body: &Value) -> bool {
    body["messages"]
        .as_array()
        .and_then(|m| m.last())
        .and_then(|m| m["content"].as_str())
        .is_some_and(|c| c.contains("compacting this conversation"))
}

/// An OpenAI reply with text and the prompt size the vendor reports.
fn openai_reply(text: &str, prompt_tokens: u64) -> Reply {
    Reply::sse(vec![
        (None, json!({"model": "m", "choices": [{"index": 0,
            "delta": {"role": "assistant", "content": text}, "finish_reason": null}]})),
        (None, json!({"model": "m", "choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}]})),
        (None, json!({"choices": [], "usage": {"prompt_tokens": prompt_tokens, "completion_tokens": 50}})),
    ])
    .sse_done()
}

fn openai_tool_call(prompt_tokens: u64) -> Reply {
    Reply::sse(vec![
        (None, json!({"model": "m", "choices": [{"index": 0, "delta": {"role": "assistant",
            "tool_calls": [{"index": 0, "id": "call_1", "type": "function",
                "function": {"name": "filesystem__read", "arguments": "{\"path\":\"hello.txt\"}"}}]},
            "finish_reason": null}]})),
        (None, json!({"model": "m", "choices": [{"index": 0, "delta": {}, "finish_reason": "tool_calls"}]})),
        (None, json!({"choices": [], "usage": {"prompt_tokens": prompt_tokens, "completion_tokens": 20}})),
    ])
    .sse_done()
}

async fn openai(h: &Harness, api: &FakeApi, context_window: u32) {
    h.add(
        connection(json!({
            "id": "o", "name": "O", "kind": "openai", "baseUrl": api.url("/v1"),
            "models": [{"id": "m", "contextWindow": context_window}]
        })),
        None,
    )
    .await;
    // The smallest limit: 8 000 tokens (or 80% of the window).
    h.sessions.set_compaction(CompactionPolicy {
        threshold_tokens: CompactionPolicy::MIN_TOKENS,
        ..Default::default()
    });
}

fn compactions(h: &Harness, id: &orchestrator_core::SessionId) -> Vec<(bool, u64, u32, String)> {
    h.events(id)
        .into_iter()
        .filter_map(|e| match e {
            SessionEvent::Compacted {
                automatic,
                before_tokens,
                messages,
                summary,
                ..
            } => Some((automatic, before_tokens, messages, summary)),
            _ => None,
        })
        .collect()
}

fn user_texts(body: &Value) -> Vec<String> {
    body["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|m| m["role"] == "user")
        .filter_map(|m| m["content"].as_str().map(str::to_owned))
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_long_conversation_is_compacted_by_the_ai_between_turns() {
    // The first turn reports a 9 000-token prompt: the second one compacts
    // first.
    let api = FakeApi::start(|request, _| {
        if asks_summary(&request.body) {
            openai_reply(
                "<summary>O usuário pediu o plano A; ficou decidido usar Rust.</summary>",
                9_100,
            )
        } else if user_texts(&request.body).len() == 1 && user_texts(&request.body)[0] == "primeira"
        {
            openai_reply("Entendido.", 9_000)
        } else {
            openai_reply("Continuo.", 1_200)
        }
    })
    .await;
    let h = Harness::new();
    openai(&h, &api, 1_000_000).await;
    let session = h.start("o").await;
    h.turn(&session.id, "primeira").await;
    let info = h.turn(&session.id, "segunda").await;
    assert_eq!(h.last_turn(&session.id), (TurnStatus::Completed, None));

    let calls = api.calls("/v1/chat");
    assert_eq!(calls.len(), 3, "turn 1, summary, turn 2");
    let (first, summary, second) = (&calls[0].body, &calls[1].body, &calls[2].body);
    assert!(asks_summary(summary));
    // The summary request reads the same prefix: same tools, same system,
    // the whole conversation first.
    assert_eq!(summary["tools"], first["tools"]);
    let convo = first["messages"].as_array().unwrap();
    assert_eq!(
        summary["messages"].as_array().unwrap()[..convo.len()],
        convo[..]
    );
    // After it, nothing of the old transcript: system + the summary opening
    // the new message.
    let messages = second["messages"].as_array().unwrap();
    assert_eq!(messages.len(), 2, "{messages:?}");
    assert_eq!(messages[0], first["messages"][0], "system untouched");
    let opened = messages[1]["content"].as_str().unwrap();
    assert!(opened.contains("ficou decidido usar Rust") && opened.ends_with("segunda"));
    assert!(!opened.contains("<summary></summary>"));

    let done = compactions(&h, &session.id);
    assert_eq!(done.len(), 1);
    let (automatic, before, messages, text) = &done[0];
    assert!(*automatic);
    assert!(*before >= 9_050, "{before}");
    assert_eq!(*messages, 2);
    assert_eq!(text, "O usuário pediu o plano A; ficou decidido usar Rust.");
    let audit = h.audits(EventKind::ContextCompacted);
    assert_eq!(audit.len(), 1);
    assert_eq!(audit[0].data["automatic"], true);
    assert_eq!(audit[0].data["messages"], 2);
    assert!(audit[0].data.get("summary").is_none(), "numbers only");
    // The summary is paid for like any request of the turn.
    assert_eq!(info.usage.output_tokens, 150);
}

#[tokio::test(flavor = "multi_thread")]
async fn compactar_replaces_the_conversation_on_request() {
    let api = FakeApi::start(|request, _| {
        if asks_summary(&request.body) {
            openai_reply("<summary>resumo pedido</summary>", 500)
        } else {
            openai_reply("ok", 300)
        }
    })
    .await;
    let h = Harness::new();
    openai(&h, &api, 1_000_000).await;
    let session = h.start("o").await;
    let empty = h.sessions.compact(&session.id, CallOrigin::User).await;
    assert!(empty.unwrap_err().message.contains("vazia"));

    h.turn(&session.id, "um").await;
    h.turn(&session.id, "dois").await;
    let result = h
        .sessions
        .compact(&session.id, CallOrigin::User)
        .await
        .unwrap();
    assert_eq!(result.status, TurnStatus::Completed);
    let done = compactions(&h, &session.id);
    assert_eq!(done.len(), 1);
    assert!(!done[0].0, "asked by the user");
    assert_eq!(done[0].2, 4, "two turns = four messages");

    h.turn(&session.id, "três").await;
    let last = api.calls("/v1/chat").last().unwrap().body.clone();
    let users = user_texts(&last);
    assert_eq!(users.len(), 1);
    assert!(users[0].contains("resumo pedido") && users[0].ends_with("três"));

    // Nothing more to compact until the conversation grows again.
    h.sessions
        .compact(&session.id, CallOrigin::User)
        .await
        .unwrap();
    assert_eq!(compactions(&h, &session.id).len(), 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_failed_summary_leaves_the_conversation_as_it_was() {
    let api = FakeApi::start(|request, _| {
        if asks_summary(&request.body) {
            Reply::status(400, json!({"error": {"message": "context too long"}}))
        } else {
            openai_reply("ok", 9_000)
        }
    })
    .await;
    let h = Harness::new();
    openai(&h, &api, 1_000_000).await;
    let session = h.start("o").await;
    h.turn(&session.id, "um").await;
    h.turn(&session.id, "dois").await;
    assert_eq!(h.last_turn(&session.id), (TurnStatus::Completed, None));
    assert!(compactions(&h, &session.id).is_empty());
    let warned = h.events(&session.id).into_iter().any(|e| {
        matches!(e, SessionEvent::Notice { level: NoticeLevel::Warning, message, .. }
            if message.contains("não foi compactada") && message.contains("context too long"))
    });
    assert!(warned);
    let last = api.calls("/v1/chat").last().unwrap().body.clone();
    assert_eq!(
        user_texts(&last),
        vec!["um", "dois"],
        "whole conversation kept"
    );

    // Asked by the user, the failure is the turn's.
    let failed = h
        .sessions
        .compact(&session.id, CallOrigin::User)
        .await
        .unwrap();
    assert_eq!(failed.status, TurnStatus::Failed);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_turn_compacts_between_tool_rounds_and_carries_on() {
    // Window of 10 000 → limit 8 000: the tool round reports 8 500.
    let api = FakeApi::start(|request, _| {
        let body = &request.body;
        if asks_summary(body) {
            openai_reply(
                "<summary>Lido hello.txt: diz olá do projeto.</summary>",
                8_600,
            )
        } else if user_texts(body) == ["leia hello.txt"]
            && body["messages"].as_array().unwrap().len() == 2
        {
            openai_tool_call(8_480)
        } else {
            openai_reply("Terminei.", 900)
        }
    })
    .await;
    let h = Harness::new();
    openai(&h, &api, 10_000).await;
    let session = h.start("o").await;
    h.turn(&session.id, "leia hello.txt").await;
    assert_eq!(h.last_turn(&session.id), (TurnStatus::Completed, None));
    let calls = api.calls("/v1/chat");
    assert_eq!(calls.len(), 3, "tool round, summary, next round");
    let summary = &calls[1].body;
    let messages = summary["messages"].as_array().unwrap();
    // The round was complete: the call and its result, then the request.
    assert_eq!(messages[messages.len() - 2]["role"], "tool");
    let next = calls[2].body["messages"].as_array().unwrap();
    assert_eq!(next.len(), 2);
    let opened = next[1]["content"].as_str().unwrap();
    assert!(opened.contains("diz olá do projeto") && opened.ends_with("summary leaves off."));
    assert_eq!(h.text(&session.id), "Terminei.");
    assert_eq!(compactions(&h, &session.id).len(), 1);
}
