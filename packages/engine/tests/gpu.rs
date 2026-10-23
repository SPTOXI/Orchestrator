//! Phase G1: the batched products on the GPU. The logits match the
//! independent reference, the training gradients match the CPU's, and a
//! memory budget smaller than the model still works (weights go to the GPU
//! by blocks).
//!
//! Needs the `gpu` feature and a GPU adapter (a software one, such as
//! Mesa's lavapipe, is enough); without an adapter the tests say so and
//! pass.
#![cfg(feature = "gpu")]

use std::path::PathBuf;

use orchestrator_nativa::convert;
use orchestrator_nativa::lora::{Adapter, LoraConfig};
use orchestrator_nativa::train::{self, Example};
use orchestrator_nativa::{Dtype, GpuOptions, LoadOptions, Model, NativaFile};
use serde::Deserialize;

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn file(dtype: Dtype) -> NativaFile {
    let src = NativaFile::read(&fixtures().join("tiny.nativa")).unwrap();
    if dtype == Dtype::F32 {
        return src;
    }
    let mut bytes = Vec::new();
    convert::quantize(&src, dtype, &mut bytes).unwrap();
    NativaFile::parse(bytes).unwrap()
}

/// The CPU and the GPU model, or None (and a note) without an adapter.
fn pair(dtype: Dtype, memory: u64) -> Option<(Model, Model)> {
    let cpu = Model::from_file(file(dtype), &LoadOptions::default()).unwrap();
    let gpu = Model::from_file(
        file(dtype),
        &LoadOptions {
            gpu: Some(GpuOptions {
                memory,
                always: true,
                ..Default::default()
            }),
            ..Default::default()
        },
    );
    match gpu {
        Ok(g) => Some((cpu, g)),
        Err(e) => {
            eprintln!("skipped: {e}");
            None
        }
    }
}

fn worst(a: &[f32], b: &[f32]) -> f32 {
    assert_eq!(a.len(), b.len());
    a.iter()
        .zip(b)
        .map(|(x, y)| (x - y).abs())
        .fold(0.0, f32::max)
}

#[derive(Deserialize)]
struct Expected {
    prompt: Vec<u32>,
    logits: Vec<Vec<f32>>,
}

#[test]
fn logits_match_the_reference() {
    let Some((_, gpu)) = pair(Dtype::F32, 768 << 20) else {
        return;
    };
    eprintln!("device: {}", gpu.device());
    let json = std::fs::read_to_string(fixtures().join("tiny.expected.json")).unwrap();
    let e: Expected = serde_json::from_str(&json).unwrap();
    assert!(e.prompt.len() >= 8, "long enough to go to the GPU");
    let got = train::logits(&gpu, None, &e.prompt).unwrap();
    let want: Vec<f32> = e.logits.concat();
    let w = worst(&got, &want);
    eprintln!("largest difference from the reference: {w:e}");
    assert!(w <= 1e-4, "{w}");
    assert!(gpu.gpu_resident_bytes() > 0, "the weights went to the GPU");
}

#[test]
fn quantized_weights_agree_with_the_cpu() {
    // The CPU also rounds the activations to 8 bits; the GPU does not. The
    // bounds are the ones the CPU itself is held to against the reference.
    for (dtype, bound) in [(Dtype::Q8, 0.2), (Dtype::Q4, 2.0), (Dtype::T2, 2.0)] {
        let Some((cpu, gpu)) = pair(dtype, 768 << 20) else {
            return;
        };
        let tokens: Vec<u32> = (0..40).map(|i| (i * 37 + 5) % 270).collect();
        let a = train::logits(&cpu, None, &tokens).unwrap();
        let b = train::logits(&gpu, None, &tokens).unwrap();
        let w = worst(&a, &b);
        eprintln!("{dtype:?}: largest CPU/GPU difference {w:e}");
        assert!(w <= bound, "{dtype:?}: {w}");
    }
}

#[test]
fn training_gradients_agree_with_the_cpu() {
    for dtype in [Dtype::F32, Dtype::T2] {
        let Some((cpu, gpu)) = pair(dtype, 768 << 20) else {
            return;
        };
        let adapter = Adapter::new(
            &cpu,
            LoraConfig {
                rank: 4,
                ..Default::default()
            },
            7,
        )
        .unwrap();
        // B starts at zero; one step gives it values, so every gradient
        // path is used.
        let mut adapter = adapter;
        let ex = Example {
            tokens: (0..33).map(|i| (i * 11 + 3) % 270).collect(),
            loss_from: 4,
        };
        let mut trainer = train::Trainer::new(&adapter, Default::default());
        trainer
            .step(&cpu, &mut adapter, std::slice::from_ref(&ex))
            .unwrap();

        let mut g_cpu = vec![0.0; adapter.len()];
        let mut g_gpu = vec![0.0; adapter.len()];
        let l_cpu = train::loss_and_grad(&cpu, &adapter, &ex, 1.0, &mut g_cpu).unwrap();
        let l_gpu = train::loss_and_grad(&gpu, &adapter, &ex, 1.0, &mut g_gpu).unwrap();
        let norm = g_cpu.iter().map(|g| g * g).sum::<f32>().sqrt();
        let diff = g_cpu
            .iter()
            .zip(&g_gpu)
            .map(|(a, b)| (a - b) * (a - b))
            .sum::<f32>()
            .sqrt();
        eprintln!(
            "{dtype:?}: loss {l_cpu} / {l_gpu}, gradient relative difference {:e}",
            diff / norm
        );
        let tol = if dtype == Dtype::F32 { 1e-3 } else { 0.1 };
        assert!((l_cpu - l_gpu).abs() <= tol * l_cpu.abs().max(1.0));
        assert!(diff / norm <= tol, "{dtype:?}: {}", diff / norm);
    }
}

#[test]
fn a_budget_smaller_than_the_model_works_by_blocks() {
    // tiny's matrices are 4 KiB each (32 × 32 f32) and the output 34 KiB:
    // 40 KiB holds the output and a few layer matrices at a time.
    let Some((_, small)) = pair(Dtype::F32, 40 << 10) else {
        return;
    };
    let Some((_, big)) = pair(Dtype::F32, 768 << 20) else {
        return;
    };
    let tokens: Vec<u32> = (0..24).map(|i| (i * 13 + 1) % 270).collect();
    let a = train::logits(&small, None, &tokens).unwrap();
    let b = train::logits(&big, None, &tokens).unwrap();
    assert_eq!(a, b, "the same kernels, whatever is resident");
    assert!(small.gpu_resident_bytes() <= 40 << 10);
    assert!(big.gpu_resident_bytes() > 40 << 10);
}

#[test]
fn the_automatic_choice_never_changes_the_answer_much() {
    // Without `always`, each kind of product is measured on both sides the
    // first time and the faster one kept. Whichever wins, the logits stay
    // those of the CPU or of the GPU, both held to the reference.
    let cpu = Model::from_file(file(Dtype::F32), &LoadOptions::default()).unwrap();
    let auto = match Model::from_file(
        file(Dtype::F32),
        &LoadOptions {
            gpu: Some(GpuOptions::default()),
            ..Default::default()
        },
    ) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("skipped: {e}");
            return;
        }
    };
    let tokens: Vec<u32> = (0..24).map(|i| (i * 13 + 1) % 270).collect();
    let a = train::logits(&cpu, None, &tokens).unwrap();
    for _ in 0..2 {
        let b = train::logits(&auto, None, &tokens).unwrap();
        assert!(worst(&a, &b) <= 1e-4);
    }
    let (g, c) = auto.gpu_share();
    eprintln!("automatic choice: {g} products on the GPU, {c} on the CPU");
    assert!(g + c > 0, "every batched product was decided");
}

#[test]
fn the_choice_is_remembered_on_disk() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("choices.json");
    let open = || {
        Model::from_file(
            file(Dtype::F32),
            &LoadOptions {
                gpu: Some(GpuOptions {
                    choices_file: Some(path.clone()),
                    ..Default::default()
                }),
                ..Default::default()
            },
        )
    };
    let first = match open() {
        Ok(m) => m,
        Err(e) => {
            eprintln!("skipped: {e}");
            return;
        }
    };
    let tokens: Vec<u32> = (0..24).map(|i| (i * 13 + 1) % 270).collect();
    let a = train::logits(&first, None, &tokens).unwrap();
    let saved = std::fs::read_to_string(&path).expect("the choices were saved");
    assert!(saved.contains("choices"));
    // A second model reads them back and decides nothing anew: its file
    // stays byte for byte the same.
    let second = open().unwrap();
    let b = train::logits(&second, None, &tokens).unwrap();
    assert!(worst(&a, &b) <= 1e-4);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), saved);
    let (g, c) = second.gpu_share();
    assert!(g + c > 0);
}

#[test]
#[ignore = "timing probe: cargo test --release --features gpu --test gpu -- --ignored --nocapture"]
fn round_trip_cost() {
    let Some((_, gpu)) = pair(Dtype::F32, 768 << 20) else {
        return;
    };
    let tokens: Vec<u32> = (0..8).map(|i| i * 7 % 270).collect();
    train::logits(&gpu, None, &tokens).unwrap();
    let (before, _) = gpu.gpu_share();
    let t = std::time::Instant::now();
    for _ in 0..50 {
        train::logits(&gpu, None, &tokens).unwrap();
    }
    let n = gpu.gpu_share().0 - before;
    eprintln!(
        "{n} tiny products, {:.3} ms each (almost all of it the round trip)",
        t.elapsed().as_secs_f64() * 1e3 / n as f64
    );
}
