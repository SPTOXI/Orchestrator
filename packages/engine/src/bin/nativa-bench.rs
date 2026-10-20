//! `nativa-bench [--dtype t2|q4|q8|f32] [--threads N] [--tokens N]
//! [--dim D --layers L --hidden H --vocab V]`
//!
//! Builds a model with random weights of the given shape (default: the
//! Semente core, ~50 M parameters), writes it to a temporary file, maps it
//! and measures generation speed and memory. Speed does not depend on the
//! values of the weights, only on their size and layout.

use std::collections::BTreeMap;
use std::io::BufWriter;
use std::sync::atomic::AtomicBool;
use std::time::Instant;

use orchestrator_nativa::format::{self, TensorData};
use orchestrator_nativa::tokenizer::TokenizerSpec;
use orchestrator_nativa::{
    generate, Config, Dtype, GenerateParams, KvPrecision, LoadOptions, Model, SamplingParams,
    SessionOptions,
};

struct Args {
    dtype: Dtype,
    threads: Option<usize>,
    tokens: usize,
    dim: usize,
    layers: usize,
    hidden: usize,
    vocab: usize,
    /// Training steps to time (0 = none).
    train: usize,
    /// Prompt tokens.
    prompt: usize,
}

fn parse() -> Args {
    let mut a = Args {
        dtype: Dtype::T2,
        threads: None,
        tokens: 128,
        dim: 512,
        layers: 12,
        hidden: 1408,
        vocab: 32_000,
        train: 0,
        prompt: 32,
    };
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let mut it = argv.chunks(2);
    while let Some([k, v]) = it.next() {
        let n = || v.parse::<usize>().expect("a number");
        match k.as_str() {
            "--dtype" => {
                a.dtype = match v.as_str() {
                    "t2" => Dtype::T2,
                    "q4" => Dtype::Q4,
                    "q8" => Dtype::Q8,
                    "f32" => Dtype::F32,
                    _ => panic!("dtype: t2, q4, q8 or f32"),
                }
            }
            "--threads" => a.threads = Some(n()),
            "--tokens" => a.tokens = n(),
            "--dim" => a.dim = n(),
            "--layers" => a.layers = n(),
            "--hidden" => a.hidden = n(),
            "--vocab" => a.vocab = n(),
            "--train" => a.train = n(),
            "--prompt" => a.prompt = n(),
            _ => panic!("unknown option {k}"),
        }
    }
    a
}

fn random(n: usize, seed: u64, scale: f32) -> Vec<f32> {
    let mut s = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
    (0..n)
        .map(|_| {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            ((s >> 40) as f32 / (1u64 << 23) as f32 - 1.0) * scale
        })
        .collect()
}

/// Resident memory of this process (Linux), in bytes.
fn rss() -> Option<usize> {
    let s = std::fs::read_to_string("/proc/self/statm").ok()?;
    let pages: usize = s.split_whitespace().nth(1)?.parse().ok()?;
    Some(pages * 4096)
}

fn main() {
    let a = parse();
    let special: Vec<String> = (0..a.vocab - 256).map(|i| format!("<|t{i}|>")).collect();
    let config = Config {
        vocab_size: a.vocab,
        dim: a.dim,
        n_layers: a.layers,
        n_heads: a.dim / 64,
        n_kv_heads: (a.dim / 64 / 2).max(1),
        hidden_dim: a.hidden,
        max_seq_len: 2048,
        rope_theta: 10000.0,
        norm_eps: 1e-5,
    };
    let kv = config.kv_dim();
    let names: Vec<String> = (0..a.layers)
        .flat_map(|l| {
            [
                "attn_norm",
                "wq",
                "wk",
                "wv",
                "wo",
                "ffn_norm",
                "w1",
                "w2",
                "w3",
            ]
            .map(|n| format!("layers.{l}.{n}"))
        })
        .collect();
    let path = std::env::temp_dir().join(format!("nativa-bench-{}.nativa", std::process::id()));
    {
        let mut tensors = Vec::new();
        let mut seed = 1;
        let mut add = |name: &'static str, shape: Vec<usize>, dtype: Dtype| {
            seed += 1;
            let n = shape.iter().product();
            let scale = if shape.len() == 1 { 1.0 } else { 0.05 };
            TensorData::quantized(name, dtype, shape, &random(n, seed, scale))
        };
        let leak = |s: &String| -> &'static str { Box::leak(s.clone().into_boxed_str()) };
        tensors.push(add("tok_embeddings", vec![a.vocab, a.dim], a.dtype));
        for (i, name) in names.iter().enumerate() {
            let (shape, dtype) = match i % 9 {
                0 | 5 => (vec![a.dim], Dtype::F32),
                1 | 4 => (vec![a.dim, a.dim], a.dtype),
                2 | 3 => (vec![kv, a.dim], a.dtype),
                6 | 8 => (vec![a.hidden, a.dim], a.dtype),
                _ => (vec![a.dim, a.hidden], a.dtype),
            };
            tensors.push(add(leak(name), shape, dtype));
        }
        tensors.push(add("norm", vec![a.dim], Dtype::F32));
        tensors.push(add("output", vec![a.vocab, a.dim], a.dtype));
        let tok = TokenizerSpec {
            merges: vec![],
            special,
        };
        let mut out = BufWriter::new(std::fs::File::create(&path).expect("create"));
        format::write(&mut out, &config, &tok, &BTreeMap::new(), &tensors).expect("write");
    }
    let file_mb = std::fs::metadata(&path).unwrap().len() as f64 / 1e6;
    let params = a.vocab * a.dim * 2
        + a.layers * (2 * a.dim * a.dim + 2 * kv * a.dim + 3 * a.dim * a.hidden);

    let before = rss().unwrap_or(0);
    let model = Model::load_with(&path, &LoadOptions { threads: a.threads }).expect("load");
    let opts = SessionOptions {
        context: Some(1024),
        kv: KvPrecision::Q8,
        memory_budget: None,
    };
    let report = model.memory(&opts).unwrap();
    let mut session = model.session_with(&opts).unwrap();
    let prompt: Vec<u32> = (0..a.prompt as u32).map(|i| 300 + i).collect();
    let params_gen = GenerateParams {
        max_tokens: a.tokens,
        sampling: SamplingParams {
            seed: 1,
            ..SamplingParams::default()
        },
        stop: vec![],
    };
    let cancel = AtomicBool::new(false);
    let t0 = Instant::now();
    let mut first = None;
    let out = generate(&mut session, &prompt, &params_gen, &cancel, |_| {
        first.get_or_insert_with(|| t0.elapsed());
        true
    })
    .expect("generate");
    let total = t0.elapsed();
    let prefill = first.unwrap_or(total);
    let gen_secs = (total - prefill).as_secs_f64();
    let tok_s = (out.tokens.len().saturating_sub(1)) as f64 / gen_secs.max(1e-9);
    let after = rss().unwrap_or(0);

    println!(
        "model        {:.1} M parameters, {:?}, file {file_mb:.1} MB",
        params as f64 / 1e6,
        a.dtype
    );
    println!(
        "kernels      {}, {} threads",
        model.kernels(),
        model.threads()
    );
    println!(
        "prompt       {} tokens in {:.1} ms ({:.0} tokens/s)",
        prompt.len(),
        prefill.as_secs_f64() * 1e3,
        prompt.len() as f64 / prefill.as_secs_f64()
    );
    println!(
        "generation   {} tokens, {tok_s:.1} tokens/s",
        out.tokens.len()
    );
    println!(
        "memory plan  weights {:.1} MB mapped + {:.1} MB owned, KV {:.1} MB, scratch {:.2} MB, total {:.1} MB",
        report.weights_mapped as f64 / 1e6,
        report.weights_owned as f64 / 1e6,
        report.kv_cache as f64 / 1e6,
        report.scratch as f64 / 1e6,
        report.total() as f64 / 1e6
    );
    drop(session);
    if a.train > 0 {
        use orchestrator_nativa::lora::{Adapter, LoraConfig};
        use orchestrator_nativa::train::{Example, TrainConfig, Trainer};
        let mut adapter = Adapter::new(&model, LoraConfig::default(), 1).expect("adapter");
        let mut trainer = Trainer::new(&adapter, TrainConfig::default());
        let ex = Example {
            tokens: (0..128).map(|i| 300 + i).collect(),
            loss_from: 64,
        };
        let t = Instant::now();
        for _ in 0..a.train {
            trainer
                .step(&model, &mut adapter, std::slice::from_ref(&ex))
                .expect("train step");
        }
        let per = t.elapsed().as_secs_f64() / a.train as f64;
        let tf = Instant::now();
        for _ in 0..a.train {
            orchestrator_nativa::train::loss(&model, Some(&adapter), &ex).expect("forward");
        }
        let fwd = tf.elapsed().as_secs_f64() / a.train as f64;
        println!(
            "training     forward pass alone: {fwd:.2} s ({:.0}% of a step)",
            fwd / per * 100.0
        );
        println!(
            "training     LoRA rank 8 on all matrices ({:.2} M values), 128-token sequence: {per:.2} s per step, {:.0} tokens/s",
            adapter.len() as f64 / 1e6,
            128.0 / per
        );
    }
    drop(model);
    let _ = std::fs::remove_file(&path);
    if after > 0 {
        println!(
            "resident     {:.1} MB (process grew {:.1} MB while loading and generating)",
            after as f64 / 1e6,
            (after.saturating_sub(before)) as f64 / 1e6
        );
    }
}
