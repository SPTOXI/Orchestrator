//! Phase M1 acceptance: the engine matches the independent reference
//! (`training/reference/nativa_ref.py`, float64 numpy) on a small model with
//! random weights — every logit at every position, and every token id.
//!
//! Regenerate the fixtures with
//! `python3 training/reference/nativa_ref.py packages/engine/tests/fixtures`.

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;

use orchestrator_nativa::{generate, GenerateParams, Model, SamplingParams, StopReason};
use serde::Deserialize;

const TOLERANCE: f32 = 1e-4;

#[derive(Deserialize)]
struct Encoding {
    text: String,
    ids: Vec<u32>,
}

#[derive(Deserialize)]
struct Expected {
    encodings: Vec<Encoding>,
    prompt: Vec<u32>,
    logits: Vec<Vec<f32>>,
}

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn load() -> (Model, Expected) {
    let model = Model::load(&fixtures().join("tiny.nativa")).unwrap();
    let json = std::fs::read_to_string(fixtures().join("tiny.expected.json")).unwrap();
    (model, serde_json::from_str(&json).unwrap())
}

#[test]
fn tokenizer_matches_reference() {
    let (model, expected) = load();
    let tok = model.tokenizer();
    for e in &expected.encodings {
        assert_eq!(tok.encode(&e.text), e.ids, "encoding {:?}", e.text);
        assert_eq!(tok.decode(&e.ids), e.text, "decoding {:?}", e.text);
    }
}

#[test]
fn logits_match_reference_at_every_position() {
    let (model, expected) = load();
    assert_eq!(expected.prompt.len(), expected.logits.len());
    let mut session = model.session();
    let mut worst = 0.0f32;
    for (pos, (&token, want)) in expected.prompt.iter().zip(&expected.logits).enumerate() {
        let got = session.step(token).unwrap();
        assert_eq!(got.len(), want.len());
        for (i, (g, w)) in got.iter().zip(want).enumerate() {
            let diff = (g - w).abs();
            worst = worst.max(diff);
            assert!(
                diff <= TOLERANCE,
                "position {pos}, logit {i}: engine {g}, reference {w}"
            );
        }
    }
    assert_eq!(session.position(), expected.prompt.len());
    eprintln!("largest difference from the reference: {worst:e}");
}

#[test]
fn greedy_generation_follows_the_logits_and_stops() {
    let (model, expected) = load();
    let cancel = AtomicBool::new(false);
    let params = GenerateParams {
        max_tokens: 5,
        sampling: SamplingParams::greedy(),
        stop: vec![],
    };
    let mut seen = Vec::new();
    let mut session = model.session();
    let out = generate(&mut session, &expected.prompt, &params, &cancel, |t| {
        seen.push(t);
        true
    })
    .unwrap();
    assert_eq!(out.stop, StopReason::MaxTokens);
    assert_eq!(out.tokens.len(), 5);
    assert_eq!(seen, out.tokens);

    // The first generated token is the argmax of the reference's last row.
    let last = expected.logits.last().unwrap();
    let argmax = (0..last.len())
        .max_by(|&a, &b| last[a].total_cmp(&last[b]))
        .unwrap();
    assert_eq!(out.tokens[0] as usize, argmax);

    // Same prompt, the first token as a stop token: nothing is returned.
    let stop = GenerateParams {
        stop: vec![out.tokens[0]],
        ..params.clone()
    };
    let mut session = model.session();
    let again = generate(&mut session, &expected.prompt, &stop, &cancel, |_| true).unwrap();
    assert_eq!(again.stop, StopReason::StopToken);
    assert!(again.tokens.is_empty());
}

#[test]
fn cancellation_and_full_context() {
    let (model, expected) = load();
    let params = GenerateParams {
        max_tokens: 1000,
        sampling: SamplingParams {
            seed: 7,
            ..SamplingParams::default()
        },
        stop: vec![],
    };

    let cancelled = AtomicBool::new(true);
    let mut session = model.session();
    let out = generate(&mut session, &expected.prompt, &params, &cancelled, |_| {
        true
    })
    .unwrap();
    assert_eq!(out.stop, StopReason::Cancelled);

    let go = AtomicBool::new(false);
    let mut session = model.session();
    let out = generate(&mut session, &expected.prompt, &params, &go, |_| true).unwrap();
    assert_eq!(out.stop, StopReason::ContextFull);
    assert_eq!(session.position(), model.config().max_seq_len);
    assert!(session.step(0).is_err());

    let mut session = model.session();
    let out = generate(&mut session, &expected.prompt, &params, &go, |_| false).unwrap();
    assert_eq!(out.stop, StopReason::Callback);
    assert_eq!(out.tokens.len(), 1);
}

#[test]
fn rejects_tokens_outside_the_vocabulary() {
    let (model, _) = load();
    let mut session = model.session();
    assert!(session.step(model.config().vocab_size as u32).is_err());
}
