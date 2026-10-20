//! Low-rank adapters (phase M3): the Native AI's personal learning. The
//! model's own weights stay frozen (and quantized); a matrix `W` becomes
//! `W + (alpha / rank) · B·A`, with `A [rank, cols]` and `B [rows, rank]`
//! small enough to train on a notebook's CPU and to swap or throw away.
//!
//! Hu et al., "LoRA: Low-Rank Adaptation of Large Language Models" (2021).
//!
//! An adapter is saved as a `.nativa` file of f32 tensors
//! `lora.{layer}.{target}.a` and `.b`, with the base model's config (it is
//! refused on any other model) and `kind = lora`, `rank`, `alpha` in the
//! metadata.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::format::{self, Config, NativaFile, TensorData};
use crate::model::{Layer, Matrix, Model};

/// The matrices of a layer an adapter can change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Target {
    Wq,
    Wk,
    Wv,
    Wo,
    W1,
    W2,
    W3,
}

impl Target {
    pub const ALL: [Target; 7] = [
        Target::Wq,
        Target::Wk,
        Target::Wv,
        Target::Wo,
        Target::W1,
        Target::W2,
        Target::W3,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Target::Wq => "wq",
            Target::Wk => "wk",
            Target::Wv => "wv",
            Target::Wo => "wo",
            Target::W1 => "w1",
            Target::W2 => "w2",
            Target::W3 => "w3",
        }
    }

    pub(crate) fn matrix(self, layer: &Layer) -> &Matrix {
        match self {
            Target::Wq => &layer.wq,
            Target::Wk => &layer.wk,
            Target::Wv => &layer.wv,
            Target::Wo => &layer.wo,
            Target::W1 => &layer.w1,
            Target::W2 => &layer.w2,
            Target::W3 => &layer.w3,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LoraConfig {
    pub rank: usize,
    pub alpha: f32,
    pub targets: Vec<Target>,
}

impl Default for LoraConfig {
    fn default() -> Self {
        Self {
            rank: 8,
            alpha: 16.0,
            targets: Target::ALL.to_vec(),
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct Slot {
    pub layer: usize,
    pub target: Target,
    pub rows: usize,
    pub cols: usize,
    /// Offsets of `A` and `B` in `params`.
    pub a: usize,
    pub b: usize,
}

/// The trainable weights, one flat vector (what the optimizer walks).
#[derive(Debug, Clone)]
pub struct Adapter {
    pub config: LoraConfig,
    base: Config,
    pub(crate) params: Vec<f32>,
    pub(crate) slots: Vec<Slot>,
    /// `index[layer * 7 + target]` → slot.
    index: Vec<Option<usize>>,
}

impl Adapter {
    /// A fresh adapter for `model`: `A` small and random, `B` zero, so it
    /// starts as "no change".
    pub fn new(model: &Model, config: LoraConfig, seed: u64) -> Result<Self> {
        if config.rank == 0 || config.targets.is_empty() {
            return Err(Error::BadConfig(
                "an adapter needs a rank and targets".into(),
            ));
        }
        let mut slots = Vec::new();
        let mut index = vec![None; model.layers.len() * 7];
        let mut len = 0;
        for (l, layer) in model.layers.iter().enumerate() {
            for &target in &config.targets {
                let m = target.matrix(layer);
                let a = len;
                let b = a + config.rank * m.cols;
                len = b + m.rows * config.rank;
                index[l * 7 + target as usize] = Some(slots.len());
                slots.push(Slot {
                    layer: l,
                    target,
                    rows: m.rows,
                    cols: m.cols,
                    a,
                    b,
                });
            }
        }
        let mut params = vec![0.0; len];
        let mut rng = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
        for s in &slots {
            let bound = 1.0 / (s.cols as f32).sqrt();
            for p in &mut params[s.a..s.b] {
                rng ^= rng << 13;
                rng ^= rng >> 7;
                rng ^= rng << 17;
                *p = ((rng >> 40) as f32 / (1u64 << 23) as f32 - 1.0) * bound;
            }
        }
        Ok(Self {
            config,
            base: model.config().clone(),
            params,
            slots,
            index,
        })
    }

    pub fn scale(&self) -> f32 {
        self.config.alpha / self.config.rank as f32
    }

    /// Trainable values.
    pub fn len(&self) -> usize {
        self.params.len()
    }

    pub fn is_empty(&self) -> bool {
        self.params.is_empty()
    }

    pub(crate) fn slot(&self, layer: usize, target: Target) -> Option<&Slot> {
        self.index
            .get(layer * 7 + target as usize)
            .copied()
            .flatten()
            .map(|i| &self.slots[i])
    }

    pub(crate) fn check(&self, model: &Model) -> Result<()> {
        if &self.base != model.config() {
            return Err(Error::BadConfig(
                "the adapter was made for a different model".into(),
            ));
        }
        Ok(())
    }

    /// `out += scale · B·(A·x)`, with `u` as scratch of `rank` values (left
    /// holding `A·x`).
    pub(crate) fn apply(&self, slot: &Slot, x: &[f32], u: &mut [f32], out: &mut [f32]) {
        let r = self.config.rank;
        let a = &self.params[slot.a..slot.b];
        let b = &self.params[slot.b..slot.b + slot.rows * r];
        for (j, uj) in u.iter_mut().enumerate().take(r) {
            *uj = crate::simd::dot_f32(&a[j * slot.cols..(j + 1) * slot.cols], x);
        }
        let s = self.scale();
        // Rows of B are only `rank` long: a plain loop beats a kernel call.
        for (o, brow) in out.iter_mut().zip(b.chunks_exact(r)) {
            *o += s * brow.iter().zip(&u[..r]).map(|(x, y)| x * y).sum::<f32>();
        }
    }

    pub fn save(&self, model: &Model, out: &mut impl Write) -> Result<()> {
        self.check(model)?;
        let names: Vec<(String, String)> = self
            .slots
            .iter()
            .map(|s| {
                let p = format!("lora.{}.{}", s.layer, s.target.name());
                (format!("{p}.a"), format!("{p}.b"))
            })
            .collect();
        let r = self.config.rank;
        let mut tensors = Vec::new();
        for (s, (a, b)) in self.slots.iter().zip(&names) {
            tensors.push(TensorData::f32(a, vec![r, s.cols], &self.params[s.a..s.b]));
            tensors.push(TensorData::f32(
                b,
                vec![s.rows, r],
                &self.params[s.b..s.b + s.rows * r],
            ));
        }
        let mut metadata = BTreeMap::new();
        metadata.insert("kind".into(), "lora".into());
        metadata.insert("rank".into(), r.to_string());
        metadata.insert("alpha".into(), self.config.alpha.to_string());
        metadata.insert(
            "targets".into(),
            serde_json::to_string(&self.config.targets).unwrap(),
        );
        format::write(
            out,
            &self.base,
            &model.tokenizer().spec().clone(),
            &metadata,
            &tensors,
        )
        .map_err(|source| Error::Io {
            path: "<adapter>".into(),
            source,
        })
    }

    pub fn load(model: &Model, path: &Path) -> Result<Self> {
        let file = NativaFile::read(path)?;
        let meta = &file.header.metadata;
        let bad = |m: &str| Error::BadHeader(format!("adapter: {m}"));
        if meta.get("kind").map(String::as_str) != Some("lora") {
            return Err(bad("not a LoRA adapter"));
        }
        let rank = meta
            .get("rank")
            .and_then(|v| v.parse().ok())
            .ok_or_else(|| bad("rank"))?;
        let alpha = meta
            .get("alpha")
            .and_then(|v| v.parse().ok())
            .ok_or_else(|| bad("alpha"))?;
        let targets = meta
            .get("targets")
            .and_then(|v| serde_json::from_str(v).ok())
            .ok_or_else(|| bad("targets"))?;
        let mut a = Self::new(
            model,
            LoraConfig {
                rank,
                alpha,
                targets,
            },
            0,
        )?;
        if file.header.config != *model.config() {
            return Err(Error::BadConfig(
                "the adapter was made for a different model".into(),
            ));
        }
        for s in a.slots.clone() {
            let p = format!("lora.{}.{}", s.layer, s.target.name());
            let av = file.tensor_f32(&format!("{p}.a"), &[rank, s.cols])?;
            let bv = file.tensor_f32(&format!("{p}.b"), &[s.rows, rank])?;
            a.params[s.a..s.b].copy_from_slice(&av);
            a.params[s.b..s.b + s.rows * rank].copy_from_slice(&bv);
        }
        Ok(a)
    }
}
