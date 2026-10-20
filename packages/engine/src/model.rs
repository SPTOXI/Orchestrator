//! The decoder-only transformer, scalar f32 (phase M1).
//!
//! Per layer: RMSNorm → attention with grouped KV heads and rotary
//! positions → residual → RMSNorm → SwiGLU feed-forward → residual.
//!
//! - Attention: Vaswani et al., "Attention Is All You Need" (2017).
//! - Grouped KV heads: Ainslie et al., "GQA: Training Generalized
//!   Multi-Query Transformer Models from Multi-Head Checkpoints" (2023).
//! - SwiGLU: Shazeer, "GLU Variants Improve Transformer" (2020).
//!
//! Tensor names in the file: `tok_embeddings [vocab, dim]`;
//! `layers.{i}.attn_norm [dim]`, `.wq [dim, dim]`, `.wk [kv_dim, dim]`,
//! `.wv [kv_dim, dim]`, `.wo [dim, dim]`, `.ffn_norm [dim]`,
//! `.w1 [hidden, dim]` (gate), `.w3 [hidden, dim]` (up),
//! `.w2 [dim, hidden]` (down); `norm [dim]`; `output [vocab, dim]`
//! (optional: without it the embeddings are reused).

use std::path::Path;

use crate::error::{Error, Result};
use crate::format::{Config, NativaFile};
use crate::ops::{dot, matvec, rmsnorm, rope, silu, softmax};
use crate::tokenizer::Tokenizer;

struct Layer {
    attn_norm: Vec<f32>,
    wq: Vec<f32>,
    wk: Vec<f32>,
    wv: Vec<f32>,
    wo: Vec<f32>,
    ffn_norm: Vec<f32>,
    w1: Vec<f32>,
    w2: Vec<f32>,
    w3: Vec<f32>,
}

pub struct Model {
    config: Config,
    tokenizer: Tokenizer,
    tok_embeddings: Vec<f32>,
    layers: Vec<Layer>,
    norm: Vec<f32>,
    output: Option<Vec<f32>>,
}

impl Model {
    pub fn load(path: &Path) -> Result<Self> {
        Self::from_file(&NativaFile::read(path)?)
    }

    pub fn from_file(file: &NativaFile) -> Result<Self> {
        let c = file.header.config.clone();
        c.validate()?;
        let tokenizer = Tokenizer::new(file.header.tokenizer.clone())?;
        if tokenizer.vocab_size() != c.vocab_size {
            return Err(Error::BadConfig(format!(
                "vocab_size is {} but the tokenizer has {} ids",
                c.vocab_size,
                tokenizer.vocab_size()
            )));
        }
        let (dim, kv, hidden) = (c.dim, c.kv_dim(), c.hidden_dim);
        let layers = (0..c.n_layers)
            .map(|i| {
                let t =
                    |n: &str, shape: &[usize]| file.tensor_f32(&format!("layers.{i}.{n}"), shape);
                Ok(Layer {
                    attn_norm: t("attn_norm", &[dim])?,
                    wq: t("wq", &[dim, dim])?,
                    wk: t("wk", &[kv, dim])?,
                    wv: t("wv", &[kv, dim])?,
                    wo: t("wo", &[dim, dim])?,
                    ffn_norm: t("ffn_norm", &[dim])?,
                    w1: t("w1", &[hidden, dim])?,
                    w2: t("w2", &[dim, hidden])?,
                    w3: t("w3", &[hidden, dim])?,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let output = match file.tensor_info("output") {
            Some(_) => Some(file.tensor_f32("output", &[c.vocab_size, dim])?),
            None => None,
        };
        Ok(Self {
            tok_embeddings: file.tensor_f32("tok_embeddings", &[c.vocab_size, dim])?,
            norm: file.tensor_f32("norm", &[dim])?,
            layers,
            output,
            tokenizer,
            config: c,
        })
    }

    pub fn config(&self) -> &Config {
        &self.config
    }

    pub fn tokenizer(&self) -> &Tokenizer {
        &self.tokenizer
    }

    /// A fresh conversation state (KV cache) with room for `max_seq_len`.
    pub fn session(&self) -> Session<'_> {
        let c = &self.config;
        let cache = c.n_layers * c.max_seq_len * c.kv_dim();
        Session {
            model: self,
            pos: 0,
            key_cache: vec![0.0; cache],
            value_cache: vec![0.0; cache],
            x: vec![0.0; c.dim],
            xb: vec![0.0; c.dim],
            xb2: vec![0.0; c.dim],
            q: vec![0.0; c.dim],
            k: vec![0.0; c.kv_dim()],
            v: vec![0.0; c.kv_dim()],
            att: vec![0.0; c.max_seq_len],
            hb: vec![0.0; c.hidden_dim],
            hb2: vec![0.0; c.hidden_dim],
            logits: vec![0.0; c.vocab_size],
        }
    }
}

/// One conversation: the KV cache and the scratch buffers. Feeding tokens
/// one at a time with [`Session::step`] is the whole API; the prompt and
/// the generated tokens go through the same path.
pub struct Session<'m> {
    model: &'m Model,
    pos: usize,
    key_cache: Vec<f32>,
    value_cache: Vec<f32>,
    x: Vec<f32>,
    xb: Vec<f32>,
    xb2: Vec<f32>,
    q: Vec<f32>,
    k: Vec<f32>,
    v: Vec<f32>,
    att: Vec<f32>,
    hb: Vec<f32>,
    hb2: Vec<f32>,
    logits: Vec<f32>,
}

impl Session<'_> {
    /// Tokens already in the cache.
    pub fn position(&self) -> usize {
        self.pos
    }

    pub fn is_full(&self) -> bool {
        self.pos >= self.model.config.max_seq_len
    }

    /// Feeds one token and returns the logits for the next one.
    pub fn step(&mut self, token: u32) -> Result<&[f32]> {
        let m = self.model;
        let c = &m.config;
        if token as usize >= c.vocab_size {
            return Err(Error::BadToken {
                token,
                vocab_size: c.vocab_size,
            });
        }
        if self.is_full() {
            return Err(Error::BadConfig(format!(
                "context is full ({} tokens)",
                c.max_seq_len
            )));
        }
        let (dim, kv_dim, hd, pos) = (c.dim, c.kv_dim(), c.head_dim(), self.pos);
        let group = c.n_heads / c.n_kv_heads;
        let scale = 1.0 / (hd as f32).sqrt();
        let t = token as usize;
        self.x
            .copy_from_slice(&m.tok_embeddings[t * dim..(t + 1) * dim]);

        for (l, layer) in m.layers.iter().enumerate() {
            rmsnorm(&mut self.xb, &self.x, &layer.attn_norm, c.norm_eps);
            matvec(&mut self.q, &layer.wq, &self.xb);
            matvec(&mut self.k, &layer.wk, &self.xb);
            matvec(&mut self.v, &layer.wv, &self.xb);
            rope(&mut self.q, pos, hd, c.rope_theta);
            rope(&mut self.k, pos, hd, c.rope_theta);

            let layer_base = l * c.max_seq_len * kv_dim;
            let at = layer_base + pos * kv_dim;
            self.key_cache[at..at + kv_dim].copy_from_slice(&self.k);
            self.value_cache[at..at + kv_dim].copy_from_slice(&self.v);

            for h in 0..c.n_heads {
                let q = &self.q[h * hd..(h + 1) * hd];
                let kv_off = (h / group) * hd;
                let att = &mut self.att[..=pos];
                for (s, a) in att.iter_mut().enumerate() {
                    let k = layer_base + s * kv_dim + kv_off;
                    *a = dot(q, &self.key_cache[k..k + hd]) * scale;
                }
                softmax(att);
                let out = &mut self.xb2[h * hd..(h + 1) * hd];
                out.fill(0.0);
                for (s, &a) in att.iter().enumerate() {
                    let v = layer_base + s * kv_dim + kv_off;
                    for (o, val) in out.iter_mut().zip(&self.value_cache[v..v + hd]) {
                        *o += a * val;
                    }
                }
            }
            matvec(&mut self.xb, &layer.wo, &self.xb2);
            add(&mut self.x, &self.xb);

            rmsnorm(&mut self.xb, &self.x, &layer.ffn_norm, c.norm_eps);
            matvec(&mut self.hb, &layer.w1, &self.xb);
            matvec(&mut self.hb2, &layer.w3, &self.xb);
            for (g, u) in self.hb.iter_mut().zip(&self.hb2) {
                *g = silu(*g) * u;
            }
            matvec(&mut self.xb, &layer.w2, &self.hb);
            add(&mut self.x, &self.xb);
        }

        rmsnorm(&mut self.xb, &self.x, &m.norm, c.norm_eps);
        let out = m.output.as_deref().unwrap_or(&m.tok_embeddings);
        matvec(&mut self.logits, out, &self.xb);
        self.pos += 1;
        Ok(&self.logits)
    }
}

fn add(x: &mut [f32], y: &[f32]) {
    for (a, b) in x.iter_mut().zip(y) {
        *a += b;
    }
}
