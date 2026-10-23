//! Phase M3 acceptance, grammar: 1,000 generations held to a tool-call
//! schema, every one valid. The model has random weights, so it "wants"
//! garbage — the worst case for the constraint, not the best.

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;

use orchestrator_nativa::{
    generate_json, GenerateParams, KvPrecision, Model, SamplingParams, Schema, SessionOptions,
    StopReason, TokenTrie,
};
use serde_json::{json, Value};

fn model() -> Model {
    Model::load(&PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/tiny.nativa"))
        .unwrap()
}

fn tool_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "tool": {"enum": ["system.info", "system.kill", "files.read", "orchestrator.task"]},
            "pid": {"type": "integer"},
            "force": {"type": "boolean"},
            "ratio": {"type": "number"},
            "paths": {"type": "array", "items": {"type": "string", "maxLength": 6}, "maxItems": 2},
            "extra": {"type": "null"}
        },
        "required": ["tool", "paths"]
    })
}

/// An independent check of a value against the schema subset (written
/// separately from the engine's matcher).
fn valid(v: &Value, s: &Value) -> bool {
    if let Some(choices) = s.get("enum") {
        return choices.as_array().unwrap().contains(v);
    }
    match s["type"].as_str().unwrap() {
        "object" => {
            let Some(obj) = v.as_object() else {
                return false;
            };
            let props = s["properties"].as_object().unwrap();
            let required = s["required"].as_array().unwrap();
            required
                .iter()
                .all(|r| obj.contains_key(r.as_str().unwrap()))
                && obj
                    .iter()
                    .all(|(k, val)| props.get(k).is_some_and(|ps| valid(val, ps)))
        }
        "array" => {
            let Some(items) = v.as_array() else {
                return false;
            };
            let max = s
                .get("maxItems")
                .and_then(Value::as_u64)
                .unwrap_or(u64::MAX);
            items.len() as u64 <= max && items.iter().all(|i| valid(i, &s["items"]))
        }
        "string" => {
            let max = s.get("maxLength").and_then(Value::as_u64).unwrap_or(512);
            v.as_str().is_some_and(|t| t.chars().count() as u64 <= max)
        }
        // serde_json reads "-0" as a float; it is still an integer.
        "integer" => v.is_i64() || v.is_u64() || v.as_f64().is_some_and(|f| f.fract() == 0.0),
        "number" => v.is_number(),
        "boolean" => v.is_boolean(),
        "null" => v.is_null(),
        _ => false,
    }
}

#[test]
fn a_thousand_generations_are_all_valid() {
    let model = model();
    let tok = model.tokenizer();
    let trie = TokenTrie::new(tok);
    let raw = tool_schema();
    let schema = Schema::from_json(&raw).unwrap();
    let prompt = tok.encode("<|ferramenta|>");
    let cancel = AtomicBool::new(false);
    let opts = SessionOptions {
        kv: KvPrecision::Q8,
        ..Default::default()
    };
    let mut complete = 0;
    let mut tools = std::collections::BTreeSet::new();
    let mut with_optional = 0;
    for seed in 0..1000u64 {
        let params = GenerateParams {
            // Short limits (34..63 tokens) force the completion often.
            max_tokens: 34 + (seed % 30) as usize,
            sampling: SamplingParams {
                temperature: 1.2,
                top_p: 1.0,
                min_p: 0.0,
                seed,
            },
            stop: vec![],
        };
        let mut session = model.session_with(&opts).unwrap();
        let out = generate_json(
            &mut session,
            &prompt,
            &params,
            &schema,
            &trie,
            &cancel,
            |_| true,
        )
        .unwrap();
        assert_eq!(out.stop, StopReason::Complete, "seed {seed}");
        assert!(out.tokens.len() <= params.max_tokens);
        let text = tok.decode(&out.tokens);
        let value: Value = serde_json::from_str(&text)
            .unwrap_or_else(|e| panic!("seed {seed}: not JSON ({e}): {text}"));
        assert!(valid(&value, &raw), "seed {seed}: {text}");
        complete += 1;
        tools.insert(value["tool"].as_str().unwrap().to_string());
        let obj = value.as_object().unwrap();
        if obj.len() > 2 {
            with_optional += 1;
        }
        if seed < 3 {
            eprintln!("seed {seed}: {text}");
        }
    }
    assert_eq!(complete, 1000);
    // The constraint must not collapse everything to the smallest value.
    eprintln!("tools used: {tools:?}; with optional properties: {with_optional}");
    assert_eq!(tools.len(), 4);
    assert!(with_optional >= 100, "{with_optional}");
}

#[test]
fn too_small_a_limit_is_refused_up_front() {
    let model = model();
    let trie = TokenTrie::new(model.tokenizer());
    let schema = Schema::from_json(&tool_schema()).unwrap();
    let params = GenerateParams {
        max_tokens: 10,
        sampling: SamplingParams::greedy(),
        stop: vec![],
    };
    let mut session = model.session();
    let cancel = AtomicBool::new(false);
    assert!(
        generate_json(&mut session, &[], &params, &schema, &trie, &cancel, |_| {
            true
        })
        .is_err()
    );
}
