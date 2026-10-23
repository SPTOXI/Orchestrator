//! Phase M2: quantized weights, 8-bit KV cache, threads, memory mapping
//! and the memory budget — each checked against the M1 reference.

use std::path::PathBuf;

use orchestrator_nativa::convert;
use orchestrator_nativa::{
    Dtype, Error, KvPrecision, LoadOptions, Model, NativaFile, SessionOptions,
};
use serde::Deserialize;

#[derive(Deserialize)]
struct Expected {
    prompt: Vec<u32>,
    logits: Vec<Vec<f32>>,
}

fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/tiny.nativa")
}

fn expected() -> Expected {
    let path = fixture().with_extension("expected.json");
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn run(model: &Model, options: &SessionOptions, prompt: &[u32]) -> Vec<Vec<f32>> {
    let mut s = model.session_with(options).unwrap();
    prompt
        .iter()
        .map(|&t| s.step(t).unwrap().to_vec())
        .collect()
}

fn f32_kv() -> SessionOptions {
    SessionOptions {
        kv: KvPrecision::F32,
        ..Default::default()
    }
}

fn max_diff(a: &[Vec<f32>], b: &[Vec<f32>]) -> f32 {
    a.iter()
        .flatten()
        .zip(b.iter().flatten())
        .map(|(x, y)| (x - y).abs())
        .fold(0.0, f32::max)
}

fn argmax(v: &[f32]) -> usize {
    (0..v.len()).max_by(|&a, &b| v[a].total_cmp(&v[b])).unwrap()
}

fn quantized(dtype: Dtype) -> Model {
    let src = NativaFile::read(&fixture()).unwrap();
    let mut out = Vec::new();
    let n = convert::quantize(&src, dtype, &mut out).unwrap();
    assert_eq!(n, 2 * 7 + 2, "every matrix is quantized");
    let file = NativaFile::parse(out).unwrap();
    assert_eq!(file.version, 2);
    Model::from_file(file, &LoadOptions::default()).unwrap()
}

#[test]
fn version_1_files_still_load_mapped() {
    let file = NativaFile::open(&fixture()).unwrap();
    assert_eq!(file.version, 1);
    assert!(file.is_mapped());
}

#[test]
fn mapped_and_owned_files_give_the_same_logits() {
    let e = expected();
    let mapped = Model::load(&fixture()).unwrap();
    let owned = Model::from_file(
        NativaFile::read(&fixture()).unwrap(),
        &LoadOptions::default(),
    )
    .unwrap();
    assert_eq!(
        run(&mapped, &f32_kv(), &e.prompt),
        run(&owned, &f32_kv(), &e.prompt)
    );
}

#[test]
fn threads_do_not_change_the_result() {
    let e = expected();
    let one = Model::load_with(
        &fixture(),
        &LoadOptions {
            threads: Some(1),
            ..Default::default()
        },
    )
    .unwrap();
    let four = Model::load_with(
        &fixture(),
        &LoadOptions {
            threads: Some(4),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(four.threads(), 4);
    assert_eq!(
        run(&one, &f32_kv(), &e.prompt),
        run(&four, &f32_kv(), &e.prompt)
    );
    let q1 = quantized(Dtype::Q4);
    let q4 = Model::from_file(
        {
            let mut out = Vec::new();
            convert::quantize(&NativaFile::read(&fixture()).unwrap(), Dtype::Q4, &mut out).unwrap();
            NativaFile::parse(out).unwrap()
        },
        &LoadOptions {
            threads: Some(1),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(
        run(&q1, &f32_kv(), &e.prompt),
        run(&q4, &f32_kv(), &e.prompt)
    );
}

#[test]
fn quantized_models_stay_close_to_the_reference() {
    let e = expected();
    // Random weights are the hardest case for quantization (no structure to
    // keep): the float64 reference itself, with only its weights rounded to
    // q8, moves the logits (std 1.15) by up to 0.095. The engine adds the
    // q8 activations on top. q8 must be the closest.
    let mut last = 0.0;
    for (dtype, bound) in [(Dtype::Q8, 0.2), (Dtype::Q4, 2.0)] {
        let got = run(&quantized(dtype), &f32_kv(), &e.prompt);
        let diff = max_diff(&got, &e.logits);
        eprintln!("{dtype:?}: largest difference {diff}");
        assert!(diff <= bound, "{dtype:?}: {diff}");
        assert!(diff >= last);
        last = diff;
    }
    // q8 keeps the most likely next token at every position.
    let got = run(&quantized(Dtype::Q8), &f32_kv(), &e.prompt);
    for (g, w) in got.iter().zip(&e.logits) {
        assert_eq!(argmax(g), argmax(w));
    }
    // Ternary weights only make sense for models trained ternary (T1); here
    // the check is that it runs and produces finite logits.
    let t = run(&quantized(Dtype::T2), &f32_kv(), &e.prompt);
    assert!(t.iter().flatten().all(|v| v.is_finite()));
}

#[test]
fn eight_bit_kv_cache_stays_close() {
    let e = expected();
    let model = Model::load(&fixture()).unwrap();
    let q8 = SessionOptions {
        kv: KvPrecision::Q8,
        ..Default::default()
    };
    let got = run(&model, &q8, &e.prompt);
    let diff = max_diff(&got, &e.logits);
    eprintln!("q8 KV cache: largest difference {diff}");
    assert!(diff <= 0.05, "{diff}");
    for (g, w) in got.iter().zip(&e.logits) {
        assert_eq!(argmax(g), argmax(w));
    }
}

#[test]
fn memory_budget_and_context() {
    let model = Model::load(&fixture()).unwrap();
    let full = model.memory(&SessionOptions::default()).unwrap();
    let f32_report = model.memory(&f32_kv()).unwrap();
    assert!(full.weights_mapped > 0);
    // head_dim is 8 here, so the per-head scale weighs more than in real
    // models: 1.5 bytes per value against 4.
    assert!(
        full.kv_cache * 2 < f32_report.kv_cache,
        "q8 cache is much smaller"
    );

    let tight = SessionOptions {
        memory_budget: Some(1000),
        ..Default::default()
    };
    assert!(matches!(
        model.session_with(&tight),
        Err(Error::OverBudget { .. })
    ));

    let budget = full.total() - full.kv_cache / 2;
    let ctx = model.fit_context(budget, KvPrecision::Q8).unwrap();
    assert!(ctx > 0 && ctx < model.config().max_seq_len);
    let fits = SessionOptions {
        context: Some(ctx),
        memory_budget: Some(budget),
        ..Default::default()
    };
    let mut s = model.session_with(&fits).unwrap();
    assert_eq!(s.context(), ctx);
    for _ in 0..ctx {
        s.step(1).unwrap();
    }
    assert!(s.is_full());
    assert!(model.fit_context(10, KvPrecision::Q8).is_none());

    let too_long = SessionOptions {
        context: Some(model.config().max_seq_len + 1),
        ..Default::default()
    };
    assert!(model.session_with(&too_long).is_err());
}
