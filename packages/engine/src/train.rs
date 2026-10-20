//! Training a LoRA adapter on the CPU (phase M3).
//!
//! The forward pass runs over a whole sequence and keeps what the backward
//! pass needs; the backward pass is written out by hand for every
//! operation of the model, and only the adapter receives gradients (the
//! model's weights are frozen, so their gradients are never formed —
//! only `Wᵀ·dy` to pass the gradient on). Checked against finite
//! differences in the tests.
//!
//! - Backpropagation: Rumelhart, Hinton and Williams, "Learning
//!   representations by back-propagating errors" (1986).
//! - Adam: Kingma and Ba, "Adam: A Method for Stochastic Optimization"
//!   (2015); decoupled weight decay (AdamW): Loshchilov and Hutter,
//!   "Decoupled Weight Decay Regularization" (2019).

use std::sync::atomic::{AtomicBool, Ordering};

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::lora::{Adapter, Slot, Target};
use crate::model::{Matrix, Model};
use crate::ops::{rope, silu, softmax};
use crate::quant::QuantizedVec;
use crate::simd::dot_f32;

/// One training sequence. The loss covers predicting `tokens[i]` for every
/// `i >= loss_from` (at least 1): the prompt part is context, the rest is
/// what the model should learn to write.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Example {
    pub tokens: Vec<u32>,
    pub loss_from: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct TrainConfig {
    pub learning_rate: f32,
    pub beta1: f32,
    pub beta2: f32,
    pub eps: f32,
    pub weight_decay: f32,
    /// Gradients are scaled down when their global norm is above this.
    pub max_grad_norm: f32,
}

impl Default for TrainConfig {
    fn default() -> Self {
        Self {
            learning_rate: 1e-3,
            beta1: 0.9,
            beta2: 0.999,
            eps: 1e-8,
            weight_decay: 0.0,
            max_grad_norm: 1.0,
        }
    }
}

/// What the forward pass keeps for one layer, over all positions.
#[derive(Default)]
struct LayerCache {
    xin: Vec<f32>,
    inv1: Vec<f32>,
    xn1: Vec<f32>,
    q: Vec<f32>,
    k: Vec<f32>,
    v: Vec<f32>,
    uq: Vec<f32>,
    uk: Vec<f32>,
    uv: Vec<f32>,
    /// `[t][head][s]`, `s <= t`.
    probs: Vec<f32>,
    ao: Vec<f32>,
    uo: Vec<f32>,
    xmid: Vec<f32>,
    inv2: Vec<f32>,
    xn2: Vec<f32>,
    h1: Vec<f32>,
    h3: Vec<f32>,
    u1: Vec<f32>,
    u3: Vec<f32>,
    g: Vec<f32>,
    u2: Vec<f32>,
}

struct Forward {
    layers: Vec<LayerCache>,
    xf: Vec<f32>,
    invf: Vec<f32>,
    xnf: Vec<f32>,
    logits: Vec<f32>,
}

fn rmsnorm_rows(x: &[f32], w: &[f32], eps: f32, out: &mut [f32], inv: &mut [f32]) {
    let n = w.len();
    for ((xr, or), iv) in x.chunks_exact(n).zip(out.chunks_exact_mut(n)).zip(inv) {
        let ms = xr.iter().map(|v| v * v).sum::<f32>() / n as f32;
        *iv = 1.0 / (ms + eps).sqrt();
        for ((o, xv), wv) in or.iter_mut().zip(xr).zip(w) {
            *o = xv * *iv * wv;
        }
    }
}

/// `dx += ∂rmsnorm/∂x · dy` for every row (the norm weights are frozen).
fn rmsnorm_back_rows(x: &[f32], w: &[f32], inv: &[f32], dy: &[f32], dx: &mut [f32]) {
    let n = w.len();
    for (((xr, dyr), dxr), &iv) in x
        .chunks_exact(n)
        .zip(dy.chunks_exact(n))
        .zip(dx.chunks_exact_mut(n))
        .zip(inv)
    {
        let dot: f32 = xr.iter().zip(dyr).zip(w).map(|((a, b), c)| a * b * c).sum();
        let k = iv * iv * iv * dot / n as f32;
        for (((d, xv), g), wv) in dxr.iter_mut().zip(xr).zip(dyr).zip(w) {
            *d += iv * wv * g - xv * k;
        }
    }
}

/// The inverse rotation: what RoPE's backward pass applies to a gradient.
fn unrope(v: &mut [f32], pos: usize, head_dim: usize, theta: f32) {
    for head in v.chunks_exact_mut(head_dim) {
        for i in 0..head_dim / 2 {
            let freq = (theta as f64).powf(-((2 * i) as f64) / head_dim as f64);
            let angle = pos as f64 * freq;
            let (sin, cos) = (angle.sin() as f32, angle.cos() as f32);
            let (a, b) = (head[2 * i], head[2 * i + 1]);
            head[2 * i] = a * cos + b * sin;
            head[2 * i + 1] = -a * sin + b * cos;
        }
    }
}

struct Ctx<'a> {
    model: &'a Model,
    adapter: Option<&'a Adapter>,
}

impl Ctx<'_> {
    fn slot(&self, layer: usize, target: Target) -> Option<&Slot> {
        self.adapter.and_then(|a| a.slot(layer, target))
    }

    /// `y[t] = W·x[t] (+ adapter)` for every position; returns `(y, u)`.
    fn linear(
        &self,
        m: &Matrix,
        slot: Option<&Slot>,
        x: &[f32],
        qx: &mut QuantizedVec,
    ) -> (Vec<f32>, Vec<f32>) {
        let t_len = x.len() / m.cols;
        let mut y = vec![0.0; t_len * m.rows];
        let r = self.adapter.map_or(0, |a| a.config.rank);
        let mut u = vec![0.0; if slot.is_some() { t_len * r } else { 0 }];
        for t in 0..t_len {
            let xt = &x[t * m.cols..(t + 1) * m.cols];
            let yt = &mut y[t * m.rows..(t + 1) * m.rows];
            self.model.matvec(yt, m, xt, qx);
            if let (Some(s), Some(a)) = (slot, self.adapter) {
                a.apply(s, xt, &mut u[t * r..(t + 1) * r], yt);
            }
        }
        (y, u)
    }

    /// Backward of [`Ctx::linear`]: `dx += Wᵀ·dy (+ adapter)`, and the
    /// adapter's gradients into `grads`.
    #[allow(clippy::too_many_arguments)]
    fn linear_back(
        &self,
        m: &Matrix,
        slot: Option<&Slot>,
        x: &[f32],
        u: &[f32],
        dy: &[f32],
        dx: &mut [f32],
        grads: &mut [f32],
    ) {
        let t_len = x.len() / m.cols;
        for t in 0..t_len {
            let dyt = &dy[t * m.rows..(t + 1) * m.rows];
            let xt = &x[t * m.cols..(t + 1) * m.cols];
            let dxt = &mut dx[t * m.cols..(t + 1) * m.cols];
            self.model.matvec_t_add(dxt, m, dyt);
            let (Some(s), Some(a)) = (slot, self.adapter) else {
                continue;
            };
            let r = a.config.rank;
            let sc = a.scale();
            let ut = &u[t * r..(t + 1) * r];
            let av = &a.params[s.a..s.b];
            let bv = &a.params[s.b..s.b + s.rows * r];
            let mut e = vec![0.0f32; r];
            for (i, &g) in dyt.iter().enumerate() {
                if g == 0.0 {
                    continue;
                }
                let brow = &bv[i * r..(i + 1) * r];
                let gb = &mut grads[s.b + i * r..s.b + (i + 1) * r];
                for j in 0..r {
                    e[j] += brow[j] * g;
                    gb[j] += sc * g * ut[j];
                }
            }
            for (j, &ej) in e.iter().enumerate() {
                if ej == 0.0 {
                    continue;
                }
                let arow = &av[j * s.cols..(j + 1) * s.cols];
                let ga = &mut grads[s.a + j * s.cols..s.a + (j + 1) * s.cols];
                for c in 0..s.cols {
                    ga[c] += sc * ej * xt[c];
                    dxt[c] += sc * arow[c] * ej;
                }
            }
        }
    }

    fn forward(&self, tokens: &[u32]) -> Result<Forward> {
        let model = self.model;
        let c = model.config();
        let t_len = tokens.len();
        if t_len == 0 || t_len > c.max_seq_len {
            return Err(Error::BadConfig(format!(
                "a training sequence needs 1 to {} tokens",
                c.max_seq_len
            )));
        }
        let (dim, kv_dim, hd, nh) = (c.dim, c.kv_dim(), c.head_dim(), c.n_heads);
        let group = nh / c.n_kv_heads;
        let scale = 1.0 / (hd as f32).sqrt();
        let mut qx = QuantizedVec::default();
        let mut x = vec![0.0; t_len * dim];
        for (t, &tok) in tokens.iter().enumerate() {
            if tok as usize >= c.vocab_size {
                return Err(Error::BadToken {
                    token: tok,
                    vocab_size: c.vocab_size,
                });
            }
            model.embed_row(tok as usize, &mut x[t * dim..(t + 1) * dim]);
        }
        let mut layers = Vec::with_capacity(model.layers.len());
        for (l, layer) in model.layers.iter().enumerate() {
            let mut lc = LayerCache {
                xin: x.clone(),
                inv1: vec![0.0; t_len],
                xn1: vec![0.0; t_len * dim],
                ..Default::default()
            };
            rmsnorm_rows(
                &lc.xin,
                &layer.attn_norm,
                c.norm_eps,
                &mut lc.xn1,
                &mut lc.inv1,
            );
            (lc.q, lc.uq) = self.linear(&layer.wq, self.slot(l, Target::Wq), &lc.xn1, &mut qx);
            (lc.k, lc.uk) = self.linear(&layer.wk, self.slot(l, Target::Wk), &lc.xn1, &mut qx);
            (lc.v, lc.uv) = self.linear(&layer.wv, self.slot(l, Target::Wv), &lc.xn1, &mut qx);
            for t in 0..t_len {
                rope(&mut lc.q[t * dim..(t + 1) * dim], t, hd, c.rope_theta);
                rope(&mut lc.k[t * kv_dim..(t + 1) * kv_dim], t, hd, c.rope_theta);
            }
            lc.probs = vec![0.0; t_len * nh * t_len];
            lc.ao = vec![0.0; t_len * dim];
            for t in 0..t_len {
                for h in 0..nh {
                    let kvh = h / group;
                    let q = &lc.q[t * dim + h * hd..t * dim + (h + 1) * hd];
                    let p = &mut lc.probs[(t * nh + h) * t_len..(t * nh + h) * t_len + t + 1];
                    for (s, ps) in p.iter_mut().enumerate() {
                        let k = &lc.k[s * kv_dim + kvh * hd..s * kv_dim + (kvh + 1) * hd];
                        *ps = dot_f32(q, k) * scale;
                    }
                    softmax(p);
                    let out = &mut lc.ao[t * dim + h * hd..t * dim + (h + 1) * hd];
                    for (s, &ps) in p.iter().enumerate() {
                        let v = &lc.v[s * kv_dim + kvh * hd..s * kv_dim + (kvh + 1) * hd];
                        for (o, vv) in out.iter_mut().zip(v) {
                            *o += ps * vv;
                        }
                    }
                }
            }
            let (attn, uo) = self.linear(&layer.wo, self.slot(l, Target::Wo), &lc.ao, &mut qx);
            lc.uo = uo;
            lc.xmid = lc.xin.iter().zip(&attn).map(|(a, b)| a + b).collect();
            lc.inv2 = vec![0.0; t_len];
            lc.xn2 = vec![0.0; t_len * dim];
            rmsnorm_rows(
                &lc.xmid,
                &layer.ffn_norm,
                c.norm_eps,
                &mut lc.xn2,
                &mut lc.inv2,
            );
            (lc.h1, lc.u1) = self.linear(&layer.w1, self.slot(l, Target::W1), &lc.xn2, &mut qx);
            (lc.h3, lc.u3) = self.linear(&layer.w3, self.slot(l, Target::W3), &lc.xn2, &mut qx);
            lc.g = lc
                .h1
                .iter()
                .zip(&lc.h3)
                .map(|(a, b)| silu(*a) * b)
                .collect();
            let (ffn, u2) = self.linear(&layer.w2, self.slot(l, Target::W2), &lc.g, &mut qx);
            lc.u2 = u2;
            x = lc.xmid.iter().zip(&ffn).map(|(a, b)| a + b).collect();
            layers.push(lc);
        }
        let mut invf = vec![0.0; t_len];
        let mut xnf = vec![0.0; t_len * dim];
        rmsnorm_rows(&x, &model.norm, c.norm_eps, &mut xnf, &mut invf);
        let out = model.output.as_ref().unwrap_or(&model.tok_embeddings);
        let (logits, _) = Ctx {
            model,
            adapter: None,
        }
        .linear(out, None, &xnf, &mut qx);
        Ok(Forward {
            layers,
            xf: x,
            invf,
            xnf,
            logits,
        })
    }
}

fn targets(ex: &Example) -> impl Iterator<Item = (usize, u32)> + '_ {
    (ex.loss_from.max(1)..ex.tokens.len()).map(|i| (i - 1, ex.tokens[i]))
}

/// Mean cross-entropy over the example's target tokens, forward only.
pub fn loss(model: &Model, adapter: Option<&Adapter>, ex: &Example) -> Result<f32> {
    let f = Ctx { model, adapter }.forward(&ex.tokens)?;
    let v = model.config().vocab_size;
    let mut sum = 0.0;
    let mut n = 0;
    for (t, target) in targets(ex) {
        let mut p = f.logits[t * v..(t + 1) * v].to_vec();
        softmax(&mut p);
        sum -= p[target as usize].max(1e-30).ln();
        n += 1;
    }
    Ok(if n == 0 { 0.0 } else { sum / n as f32 })
}

/// The logits at every position (tests: they must match [`crate::Session`]).
pub fn logits(model: &Model, adapter: Option<&Adapter>, tokens: &[u32]) -> Result<Vec<f32>> {
    Ok(Ctx { model, adapter }.forward(tokens)?.logits)
}

/// Adds `weight ×` the gradient of the example's loss to `grads` (same
/// layout as the adapter) and returns the loss.
pub fn loss_and_grad(
    model: &Model,
    adapter: &Adapter,
    ex: &Example,
    weight: f32,
    grads: &mut [f32],
) -> Result<f32> {
    adapter.check(model)?;
    assert_eq!(grads.len(), adapter.len());
    let ctx = Ctx {
        model,
        adapter: Some(adapter),
    };
    let f = ctx.forward(&ex.tokens)?;
    let c = model.config();
    let (dim, kv_dim, hd, nh, v) = (c.dim, c.kv_dim(), c.head_dim(), c.n_heads, c.vocab_size);
    let group = nh / c.n_kv_heads;
    let scale = 1.0 / (hd as f32).sqrt();
    let t_len = ex.tokens.len();

    let n = targets(ex).count();
    if n == 0 {
        return Ok(0.0);
    }
    let mut loss = 0.0;
    let mut dlogits = vec![0.0; t_len * v];
    for (t, target) in targets(ex) {
        let d = &mut dlogits[t * v..(t + 1) * v];
        d.copy_from_slice(&f.logits[t * v..(t + 1) * v]);
        softmax(d);
        loss -= d[target as usize].max(1e-30).ln();
        d[target as usize] -= 1.0;
        d.iter_mut().for_each(|g| *g *= weight / n as f32);
    }
    let out = model.output.as_ref().unwrap_or(&model.tok_embeddings);
    let mut dxnf = vec![0.0; t_len * dim];
    for t in 0..t_len {
        model.matvec_t_add(
            &mut dxnf[t * dim..(t + 1) * dim],
            out,
            &dlogits[t * v..(t + 1) * v],
        );
    }
    let mut dx = vec![0.0; t_len * dim];
    rmsnorm_back_rows(&f.xf, &model.norm, &f.invf, &dxnf, &mut dx);

    for (l, (layer, lc)) in model.layers.iter().zip(&f.layers).enumerate().rev() {
        // Feed-forward: out = xmid + W2·g, g = silu(h1)·h3, h = W·xn2.
        let mut dg = vec![0.0; t_len * c.hidden_dim];
        ctx.linear_back(
            &layer.w2,
            ctx.slot(l, Target::W2),
            &lc.g,
            &lc.u2,
            &dx,
            &mut dg,
            grads,
        );
        let mut dh1 = vec![0.0; dg.len()];
        let mut dh3 = vec![0.0; dg.len()];
        for i in 0..dg.len() {
            let h = lc.h1[i];
            let sig = 1.0 / (1.0 + (-h).exp());
            dh3[i] = dg[i] * h * sig;
            dh1[i] = dg[i] * lc.h3[i] * sig * (1.0 + h * (1.0 - sig));
        }
        let mut dxn2 = vec![0.0; t_len * dim];
        ctx.linear_back(
            &layer.w1,
            ctx.slot(l, Target::W1),
            &lc.xn2,
            &lc.u1,
            &dh1,
            &mut dxn2,
            grads,
        );
        ctx.linear_back(
            &layer.w3,
            ctx.slot(l, Target::W3),
            &lc.xn2,
            &lc.u3,
            &dh3,
            &mut dxn2,
            grads,
        );
        let mut dxmid = dx;
        rmsnorm_back_rows(&lc.xmid, &layer.ffn_norm, &lc.inv2, &dxn2, &mut dxmid);

        // Attention: xmid = xin + Wo·ao.
        let mut dao = vec![0.0; t_len * dim];
        ctx.linear_back(
            &layer.wo,
            ctx.slot(l, Target::Wo),
            &lc.ao,
            &lc.uo,
            &dxmid,
            &mut dao,
            grads,
        );
        let mut dq = vec![0.0; t_len * dim];
        let mut dk = vec![0.0; t_len * kv_dim];
        let mut dv = vec![0.0; t_len * kv_dim];
        let mut dp = vec![0.0; t_len];
        for t in 0..t_len {
            for h in 0..nh {
                let kvh = h / group;
                let p = &lc.probs[(t * nh + h) * t_len..(t * nh + h) * t_len + t + 1];
                let dout = &dao[t * dim + h * hd..t * dim + (h + 1) * hd];
                let q = &lc.q[t * dim + h * hd..t * dim + (h + 1) * hd];
                let mut sum = 0.0;
                for (s, &ps) in p.iter().enumerate() {
                    let off = s * kv_dim + kvh * hd;
                    dp[s] = dot_f32(dout, &lc.v[off..off + hd]);
                    sum += ps * dp[s];
                    for (dvv, g) in dv[off..off + hd].iter_mut().zip(dout) {
                        *dvv += ps * g;
                    }
                }
                for (s, &ps) in p.iter().enumerate() {
                    let ds = ps * (dp[s] - sum) * scale;
                    if ds == 0.0 {
                        continue;
                    }
                    let off = s * kv_dim + kvh * hd;
                    let dqh = &mut dq[t * dim + h * hd..t * dim + (h + 1) * hd];
                    for (d, kk) in dqh.iter_mut().zip(&lc.k[off..off + hd]) {
                        *d += ds * kk;
                    }
                    for (d, qq) in dk[off..off + hd].iter_mut().zip(q) {
                        *d += ds * qq;
                    }
                }
            }
        }
        for t in 0..t_len {
            unrope(&mut dq[t * dim..(t + 1) * dim], t, hd, c.rope_theta);
            unrope(&mut dk[t * kv_dim..(t + 1) * kv_dim], t, hd, c.rope_theta);
        }
        let mut dxn1 = vec![0.0; t_len * dim];
        ctx.linear_back(
            &layer.wq,
            ctx.slot(l, Target::Wq),
            &lc.xn1,
            &lc.uq,
            &dq,
            &mut dxn1,
            grads,
        );
        ctx.linear_back(
            &layer.wk,
            ctx.slot(l, Target::Wk),
            &lc.xn1,
            &lc.uk,
            &dk,
            &mut dxn1,
            grads,
        );
        ctx.linear_back(
            &layer.wv,
            ctx.slot(l, Target::Wv),
            &lc.xn1,
            &lc.uv,
            &dv,
            &mut dxn1,
            grads,
        );
        dx = dxmid;
        rmsnorm_back_rows(&lc.xin, &layer.attn_norm, &lc.inv1, &dxn1, &mut dx);
    }
    let _ = &f.xnf;
    Ok(loss / n as f32)
}

/// AdamW over an adapter's values.
pub struct Trainer {
    pub config: TrainConfig,
    m: Vec<f32>,
    v: Vec<f32>,
    steps: u64,
}

impl Trainer {
    pub fn new(adapter: &Adapter, config: TrainConfig) -> Self {
        Self {
            config,
            m: vec![0.0; adapter.len()],
            v: vec![0.0; adapter.len()],
            steps: 0,
        }
    }

    pub fn steps(&self) -> u64 {
        self.steps
    }

    /// One optimizer step on a batch; returns the batch's mean loss.
    pub fn step(&mut self, model: &Model, adapter: &mut Adapter, batch: &[Example]) -> Result<f32> {
        if batch.is_empty() {
            return Err(Error::BadConfig("empty batch".into()));
        }
        let mut grads = vec![0.0; adapter.len()];
        let w = 1.0 / batch.len() as f32;
        let mut loss = 0.0;
        for ex in batch {
            loss += w * loss_and_grad(model, adapter, ex, w, &mut grads)?;
        }
        let c = self.config;
        let norm = grads.iter().map(|g| g * g).sum::<f32>().sqrt();
        let clip = if norm > c.max_grad_norm {
            c.max_grad_norm / norm
        } else {
            1.0
        };
        self.steps += 1;
        let t = self.steps as i32;
        let (b1t, b2t) = (1.0 - c.beta1.powi(t), 1.0 - c.beta2.powi(t));
        for (((p, g), m), v) in adapter
            .params
            .iter_mut()
            .zip(&grads)
            .zip(&mut self.m)
            .zip(&mut self.v)
        {
            let g = g * clip;
            *m = c.beta1 * *m + (1.0 - c.beta1) * g;
            *v = c.beta2 * *v + (1.0 - c.beta2) * g * g;
            let update = (*m / b1t) / ((*v / b2t).sqrt() + c.eps);
            *p -= c.learning_rate * (update + c.weight_decay * *p);
        }
        Ok(loss)
    }

    /// Steps over `data` (in order, `batch` examples at a time, wrapping
    /// around) until `steps` are done, `cancel` is set or `on_step` returns
    /// `false`. Returns the last loss.
    #[allow(clippy::too_many_arguments)]
    pub fn run(
        &mut self,
        model: &Model,
        adapter: &mut Adapter,
        data: &[Example],
        batch: usize,
        steps: usize,
        cancel: &AtomicBool,
        mut on_step: impl FnMut(u64, f32) -> bool,
    ) -> Result<f32> {
        let mut last = f32::NAN;
        let batch = batch.clamp(1, data.len().max(1));
        for i in 0..steps {
            if cancel.load(Ordering::Relaxed) {
                break;
            }
            let start = (i * batch) % data.len();
            let chunk: Vec<Example> = (0..batch)
                .map(|j| data[(start + j) % data.len()].clone())
                .collect();
            last = self.step(model, adapter, &chunk)?;
            if !on_step(self.steps, last) {
                break;
            }
        }
        Ok(last)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lora::LoraConfig;
    use crate::model::{KvPrecision, SessionOptions};
    use std::path::PathBuf;

    fn model() -> Model {
        Model::load(&PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/tiny.nativa"))
            .unwrap()
    }

    /// An adapter with every value random, so every path carries gradient.
    fn busy_adapter(model: &Model, rank: usize) -> Adapter {
        let mut a = Adapter::new(
            model,
            LoraConfig {
                rank,
                alpha: 2.0 * rank as f32,
                ..Default::default()
            },
            5,
        )
        .unwrap();
        let mut s = 99u64;
        for p in &mut a.params {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            *p = ((s >> 40) as f32 / (1u64 << 23) as f32 - 1.0) * 0.1;
        }
        a
    }

    #[test]
    fn training_forward_matches_the_session() {
        let model = model();
        let a = busy_adapter(&model, 4);
        let tokens: Vec<u32> = vec![3, 100, 260, 7, 7, 42, 255, 1, 268];
        let v = model.config().vocab_size;
        for adapter in [None, Some(&a)] {
            let want = logits(&model, adapter, &tokens).unwrap();
            let s = model
                .session_with(&SessionOptions {
                    kv: KvPrecision::F32,
                    ..Default::default()
                })
                .unwrap();
            let mut s = match adapter {
                Some(a) => s.with_adapter(a).unwrap(),
                None => s,
            };
            for (t, &tok) in tokens.iter().enumerate() {
                let got = s.step(tok).unwrap();
                for (g, w) in got.iter().zip(&want[t * v..(t + 1) * v]) {
                    assert!(
                        (g - w).abs() < 1e-4,
                        "adapter {}: {g} vs {w}",
                        adapter.is_some()
                    );
                }
            }
        }
    }

    #[test]
    fn gradients_match_finite_differences() {
        let model = model();
        let mut a = busy_adapter(&model, 2);
        let ex = Example {
            tokens: vec![5, 260, 33, 120, 7, 9, 266],
            loss_from: 2,
        };
        let mut grads = vec![0.0; a.len()];
        loss_and_grad(&model, &a, &ex, 1.0, &mut grads).unwrap();
        // A sample of values from every slot, both A and B.
        let mut checked = 0;
        let slots = a.slots.clone();
        for s in &slots {
            for idx in [s.a, s.a + (s.b - s.a) / 2, s.b, s.b + s.rows * 2 - 1] {
                let orig = a.params[idx];
                let eps = 1e-2;
                a.params[idx] = orig + eps;
                let up = loss(&model, Some(&a), &ex).unwrap();
                a.params[idx] = orig - eps;
                let down = loss(&model, Some(&a), &ex).unwrap();
                a.params[idx] = orig;
                let numeric = (up - down) / (2.0 * eps);
                let analytic = grads[idx];
                let tol = 2e-3 + 0.05 * numeric.abs().max(analytic.abs());
                assert!(
                    (numeric - analytic).abs() <= tol,
                    "layer {} {:?} [{idx}]: numeric {numeric}, analytic {analytic}",
                    s.layer,
                    s.target
                );
                checked += 1;
            }
        }
        assert_eq!(checked, slots.len() * 4);
    }
}
