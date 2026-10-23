//! Batched processing (before phase T1): feeding a prompt at once gives the
//! same numbers as feeding it one token at a time — with f32 or quantized
//! weights, either KV cache, and an adapter.

use std::path::PathBuf;

use orchestrator_nativa::convert;
use orchestrator_nativa::lora::{Adapter, LoraConfig};
use orchestrator_nativa::{Dtype, KvPrecision, LoadOptions, Model, NativaFile, SessionOptions};

fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/tiny.nativa")
}

fn models() -> Vec<(String, Model)> {
    let mut out = vec![("f32".to_string(), Model::load(&fixture()).unwrap())];
    for dtype in [Dtype::Q8, Dtype::Q4, Dtype::T2] {
        let mut bytes = Vec::new();
        convert::quantize(&NativaFile::read(&fixture()).unwrap(), dtype, &mut bytes).unwrap();
        let file = NativaFile::parse(bytes).unwrap();
        out.push((
            format!("{dtype:?}"),
            Model::from_file(file, &LoadOptions::default()).unwrap(),
        ));
    }
    out
}

fn tokens() -> Vec<u32> {
    (0..40).map(|i| (i * 37 + 5) % 270).collect()
}

#[test]
fn prefill_equals_one_token_at_a_time() {
    for (name, model) in models() {
        let adapter = Adapter::new(
            &model,
            LoraConfig {
                rank: 4,
                ..Default::default()
            },
            3,
        )
        .unwrap();
        for kv in [KvPrecision::F32, KvPrecision::Q8] {
            for with_adapter in [false, true] {
                let opts = SessionOptions {
                    kv,
                    ..Default::default()
                };
                let open = || {
                    let s = model.session_with(&opts).unwrap();
                    if with_adapter {
                        s.with_adapter(&adapter).unwrap()
                    } else {
                        s
                    }
                };
                let toks = tokens();
                let mut one = open();
                let mut want = Vec::new();
                for &t in &toks {
                    want = one.step(t).unwrap().to_vec();
                }
                // In two batches, then one more token through both.
                let mut batched = open();
                batched.prefill(&toks[..25]).unwrap();
                let got = batched.prefill(&toks[25..]).unwrap().to_vec();
                let label = format!("{name}, {kv:?}, adapter {with_adapter}");
                assert_eq!(got, want, "{label}");
                assert_eq!(batched.position(), one.position());
                let a = one.step(9).unwrap().to_vec();
                let b = batched.step(9).unwrap().to_vec();
                assert_eq!(a, b, "{label}: the KV caches agree");
            }
        }
    }
}

#[test]
fn prefill_checks_room_and_tokens() {
    let model = Model::load(&fixture()).unwrap();
    let mut s = model
        .session_with(&SessionOptions {
            context: Some(10),
            ..Default::default()
        })
        .unwrap();
    assert!(s.prefill(&[]).is_err());
    assert!(s.prefill(&[1; 11]).is_err());
    assert!(s.prefill(&[1, 9999]).is_err());
    assert_eq!(s.position(), 0, "a refused batch changes nothing");
    s.prefill(&[1; 10]).unwrap();
    assert!(s.is_full());
}
