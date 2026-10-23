//! Phase M3 acceptance, learning: a LoRA adapter trained on the CPU learns
//! a small task (answer each routine name with its reply), the trained
//! adapter survives saving and loading, and embeddings behave.

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::time::Instant;

use orchestrator_nativa::lora::{Adapter, LoraConfig};
use orchestrator_nativa::train::{self, Example, TrainConfig, Trainer};
use orchestrator_nativa::{
    generate, similarity, GenerateParams, KvPrecision, Model, SamplingParams, SessionOptions,
    StopReason,
};

fn model() -> Model {
    Model::load(&PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/tiny.nativa"))
        .unwrap()
}

const PAIRS: [(&str, &str); 4] = [
    ("backup", " feito"),
    ("status", " tudo bem"),
    ("the", " de o"),
    ("ação", " ok"),
];

fn data(model: &Model) -> Vec<Example> {
    let tok = model.tokenizer();
    PAIRS
        .iter()
        .map(|(ask, answer)| {
            let prompt = tok.encode(&format!("<|rotina|>{ask}<|ferramenta|>"));
            let mut tokens = prompt.clone();
            tokens.extend(tok.encode(&format!("{answer}<|fim|>")));
            Example {
                loss_from: prompt.len(),
                tokens,
            }
        })
        .collect()
}

fn answers(model: &Model, adapter: Option<&Adapter>) -> Vec<String> {
    let tok = model.tokenizer();
    let fim = tok.special_id("<|fim|>").unwrap();
    let params = GenerateParams {
        max_tokens: 20,
        sampling: SamplingParams::greedy(),
        stop: vec![fim],
    };
    let cancel = AtomicBool::new(false);
    PAIRS
        .iter()
        .map(|(ask, _)| {
            let s = model
                .session_with(&SessionOptions {
                    kv: KvPrecision::Q8,
                    ..Default::default()
                })
                .unwrap();
            let mut s = match adapter {
                Some(a) => s.with_adapter(a).unwrap(),
                None => s,
            };
            let prompt = tok.encode(&format!("<|rotina|>{ask}<|ferramenta|>"));
            let out = generate(&mut s, &prompt, &params, &cancel, |_| true).unwrap();
            let text = tok.decode(&out.tokens);
            if out.stop == StopReason::StopToken {
                text
            } else {
                format!("{text}…")
            }
        })
        .collect()
}

#[test]
fn an_adapter_learns_the_task_on_the_cpu() {
    let model = model();
    let data = data(&model);
    let want: Vec<String> = PAIRS.iter().map(|(_, a)| a.to_string()).collect();
    assert_ne!(
        answers(&model, None),
        want,
        "the base model does not know it"
    );

    let mut adapter = Adapter::new(
        &model,
        LoraConfig {
            rank: 8,
            alpha: 16.0,
            ..Default::default()
        },
        1,
    )
    .unwrap();
    let before: f32 = data
        .iter()
        .map(|e| train::loss(&model, Some(&adapter), e).unwrap())
        .sum::<f32>()
        / 4.0;
    let mut trainer = Trainer::new(
        &adapter,
        TrainConfig {
            learning_rate: 1e-2,
            ..Default::default()
        },
    );
    let cancel = AtomicBool::new(false);
    let t0 = Instant::now();
    let mut learned_at = None;
    for round in 0..40 {
        trainer
            .run(&model, &mut adapter, &data, 4, 10, &cancel, |_, _| true)
            .unwrap();
        if answers(&model, Some(&adapter)) == want {
            learned_at = Some((round + 1) * 10);
            break;
        }
    }
    let after: f32 = data
        .iter()
        .map(|e| train::loss(&model, Some(&adapter), e).unwrap())
        .sum::<f32>()
        / 4.0;
    eprintln!(
        "loss {before:.3} → {after:.3}; learned after {learned_at:?} steps in {:.1} s ({} values trained)",
        t0.elapsed().as_secs_f64(),
        adapter.len()
    );
    assert!(
        learned_at.is_some(),
        "answers: {:?}",
        answers(&model, Some(&adapter))
    );
    assert!(after < before / 4.0);

    // Saved and loaded, the adapter gives the same answers.
    let mut bytes = Vec::new();
    adapter.save(&model, &mut bytes).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("pessoal.nativa");
    std::fs::write(&path, bytes).unwrap();
    let loaded = Adapter::load(&model, &path).unwrap();
    assert_eq!(answers(&model, Some(&loaded)), want);
}

#[test]
fn cancellation_stops_training() {
    let model = model();
    let data = data(&model);
    let mut adapter = Adapter::new(&model, LoraConfig::default(), 2).unwrap();
    let mut trainer = Trainer::new(&adapter, TrainConfig::default());
    let cancel = AtomicBool::new(false);
    trainer
        .run(&model, &mut adapter, &data, 2, 100, &cancel, |step, _| {
            step < 3
        })
        .unwrap();
    assert_eq!(trainer.steps(), 3);
    let cancelled = AtomicBool::new(true);
    trainer
        .run(&model, &mut adapter, &data, 2, 100, &cancelled, |_, _| true)
        .unwrap();
    assert_eq!(trainer.steps(), 3);
}

#[test]
fn embeddings_are_unit_length_and_stable() {
    let model = model();
    let tok = model.tokenizer();
    let a = model.embed(&tok.encode("abrir o projeto")).unwrap();
    let a2 = model.embed(&tok.encode("abrir o projeto")).unwrap();
    let b = model.embed(&tok.encode("こんにちは、世界")).unwrap();
    assert_eq!(a.len(), model.config().dim);
    assert!((similarity(&a, &a) - 1.0).abs() < 1e-5);
    assert_eq!(a, a2);
    assert!(similarity(&a, &b) < 0.999);
    assert!(model.embed(&[]).is_err());
}
