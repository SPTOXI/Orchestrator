//! The decoder-only transformer.
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
//! (optional: without it the embeddings are reused). Matrices may be f32
//! or quantized ([`crate::quant`]); norms are f32.
//!
//! Phase M2: the weights stay in the memory-mapped file, matrix products
//! run on the SIMD kernels across a thread pool, and the KV cache can be
//! kept in 8 bits.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::format::{Config, Dtype, NativaFile};
use crate::ops::{rmsnorm, rope, silu, softmax};
use crate::pool::{Pool, SendPtr};
use crate::quant::{self, QuantizedVec};
use crate::simd;
use crate::tokenizer::Tokenizer;

/// Below this many weights a product runs on one thread: waking the pool
/// would cost more than it saves.
const PARALLEL_MIN: usize = 1 << 15;

enum MatData {
    /// In the file, at this offset of the data section.
    File(usize),
    /// f32 copied out of a file that could not be used in place (an
    /// unaligned buffer or a big-endian CPU).
    Owned(Vec<f32>),
}

struct Matrix {
    dtype: Dtype,
    rows: usize,
    cols: usize,
    data: MatData,
}

struct Layer {
    attn_norm: Vec<f32>,
    wq: Matrix,
    wk: Matrix,
    wv: Matrix,
    wo: Matrix,
    ffn_norm: Vec<f32>,
    w1: Matrix,
    w2: Matrix,
    w3: Matrix,
}

#[derive(Debug, Clone, Default)]
pub struct LoadOptions {
    /// Threads for matrix products (the caller included). Default: the
    /// processors the system reports.
    pub threads: Option<usize>,
}

/// How the KV cache stores keys and values.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum KvPrecision {
    F32,
    /// 8 bits with one scale per head and position: a quarter of the
    /// memory.
    #[default]
    Q8,
}

#[derive(Debug, Clone, Default)]
pub struct SessionOptions {
    /// Tokens the session can hold (default and maximum: the model's
    /// `max_seq_len`).
    pub context: Option<usize>,
    pub kv: KvPrecision,
    /// Refuse to start when the model and this session need more bytes.
    pub memory_budget: Option<usize>,
}

/// What a model and one session use, in bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryReport {
    /// Weights read from the mapped file (the system pages them in as they
    /// are used, and can drop them again).
    pub weights_mapped: usize,
    /// Weights and norms held in the process's own memory.
    pub weights_owned: usize,
    pub kv_cache: usize,
    pub scratch: usize,
}

impl MemoryReport {
    pub fn total(&self) -> usize {
        self.weights_mapped + self.weights_owned + self.kv_cache + self.scratch
    }
}

pub struct Model {
    config: Config,
    tokenizer: Tokenizer,
    file: NativaFile,
    pool: Pool,
    tok_embeddings: Matrix,
    layers: Vec<Layer>,
    norm: Vec<f32>,
    output: Option<Matrix>,
}

impl Model {
    /// Maps the file and loads the model with the default options.
    pub fn load(path: &Path) -> Result<Self> {
        Self::load_with(path, &LoadOptions::default())
    }

    pub fn load_with(path: &Path, options: &LoadOptions) -> Result<Self> {
        Self::from_file(NativaFile::open(path)?, options)
    }

    pub fn from_file(file: NativaFile, options: &LoadOptions) -> Result<Self> {
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
                let m = |n: &str, rows, cols| matrix(&file, &format!("layers.{i}.{n}"), rows, cols);
                let v = |n: &str| file.tensor_f32(&format!("layers.{i}.{n}"), &[dim]);
                Ok(Layer {
                    attn_norm: v("attn_norm")?,
                    wq: m("wq", dim, dim)?,
                    wk: m("wk", kv, dim)?,
                    wv: m("wv", kv, dim)?,
                    wo: m("wo", dim, dim)?,
                    ffn_norm: v("ffn_norm")?,
                    w1: m("w1", hidden, dim)?,
                    w2: m("w2", dim, hidden)?,
                    w3: m("w3", hidden, dim)?,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let output = match file.tensor_info("output") {
            Some(_) => Some(matrix(&file, "output", c.vocab_size, dim)?),
            None => None,
        };
        let threads = options
            .threads
            .unwrap_or_else(|| std::thread::available_parallelism().map_or(1, |n| n.get()));
        Ok(Self {
            tok_embeddings: matrix(&file, "tok_embeddings", c.vocab_size, dim)?,
            norm: file.tensor_f32("norm", &[dim])?,
            layers,
            output,
            tokenizer,
            pool: Pool::new(threads),
            file,
            config: c,
        })
    }

    pub fn config(&self) -> &Config {
        &self.config
    }

    pub fn tokenizer(&self) -> &Tokenizer {
        &self.tokenizer
    }

    pub fn threads(&self) -> usize {
        self.pool.threads()
    }

    /// The SIMD kernels in use ("avx2", "neon" or "scalar").
    pub fn kernels(&self) -> &'static str {
        simd::kernel_name()
    }

    /// Memory for the model and one session with `options`.
    pub fn memory(&self, options: &SessionOptions) -> Result<MemoryReport> {
        let ctx = self.context(options)?;
        let c = &self.config;
        let mut owned = (self.norm.len() + c.n_layers * 2 * c.dim) * 4;
        for m in self.matrices() {
            if let MatData::Owned(v) = &m.data {
                owned += v.len() * 4;
            }
        }
        let mapped = self.file.data_len();
        let mapped = if self.file.is_mapped() { mapped } else { 0 };
        let per_value = match options.kv {
            KvPrecision::F32 => 4.0,
            KvPrecision::Q8 => 1.0 + 4.0 / c.head_dim() as f64,
        };
        let kv_cache = (2 * c.n_layers * ctx * c.kv_dim()) as f64 * per_value;
        let scratch = (4 * c.dim + 2 * c.kv_dim() + 2 * c.hidden_dim + ctx + c.vocab_size) * 4
            + 2 * (c.dim.max(c.hidden_dim) + c.dim.max(c.hidden_dim) / quant::BLOCK * 4);
        Ok(MemoryReport {
            weights_mapped: mapped,
            weights_owned: owned
                + if self.file.is_mapped() {
                    0
                } else {
                    self.file.data_len()
                },
            kv_cache: kv_cache.ceil() as usize,
            scratch,
        })
    }

    /// The largest context (tokens) whose session fits in `budget` bytes,
    /// or `None` when not even one token does.
    pub fn fit_context(&self, budget: usize, kv: KvPrecision) -> Option<usize> {
        let fits = |ctx: usize| {
            let o = SessionOptions {
                context: Some(ctx),
                kv,
                memory_budget: None,
            };
            self.memory(&o).is_ok_and(|m| m.total() <= budget)
        };
        let (mut lo, mut hi) = (0, self.config.max_seq_len);
        if !fits(1) {
            return None;
        }
        while lo < hi {
            let mid = (lo + hi).div_ceil(2);
            if fits(mid) {
                lo = mid;
            } else {
                hi = mid - 1;
            }
        }
        Some(lo)
    }

    /// A session with the model's full context, f32 KV cache.
    pub fn session(&self) -> Session<'_> {
        self.session_with(&SessionOptions {
            kv: KvPrecision::F32,
            ..Default::default()
        })
        .expect("the default session always fits")
    }

    pub fn session_with(&self, options: &SessionOptions) -> Result<Session<'_>> {
        let ctx = self.context(options)?;
        if let Some(budget) = options.memory_budget {
            let needed = self.memory(options)?.total();
            if needed > budget {
                return Err(Error::OverBudget { needed, budget });
            }
        }
        let c = &self.config;
        let cache = c.n_layers * ctx * c.kv_dim();
        let scales = c.n_layers * ctx * c.n_kv_heads;
        let kv = match options.kv {
            KvPrecision::F32 => KvCache::F32 {
                k: vec![0.0; cache],
                v: vec![0.0; cache],
            },
            KvPrecision::Q8 => KvCache::Q8 {
                k: vec![0; cache],
                v: vec![0; cache],
                ks: vec![0.0; scales],
                vs: vec![0.0; scales],
            },
        };
        Ok(Session {
            model: self,
            pos: 0,
            context: ctx,
            kv,
            x: vec![0.0; c.dim],
            xb: vec![0.0; c.dim],
            xb2: vec![0.0; c.dim],
            q: vec![0.0; c.dim],
            k: vec![0.0; c.kv_dim()],
            v: vec![0.0; c.kv_dim()],
            att: vec![0.0; ctx],
            hb: vec![0.0; c.hidden_dim],
            hb2: vec![0.0; c.hidden_dim],
            logits: vec![0.0; c.vocab_size],
            qx: QuantizedVec::default(),
        })
    }

    fn context(&self, options: &SessionOptions) -> Result<usize> {
        let ctx = options.context.unwrap_or(self.config.max_seq_len);
        if ctx == 0 || ctx > self.config.max_seq_len {
            return Err(Error::BadConfig(format!(
                "context must be between 1 and {} tokens",
                self.config.max_seq_len
            )));
        }
        Ok(ctx)
    }

    fn matrices(&self) -> impl Iterator<Item = &Matrix> {
        self.layers
            .iter()
            .flat_map(|l| [&l.wq, &l.wk, &l.wv, &l.wo, &l.w1, &l.w2, &l.w3])
            .chain(std::iter::once(&self.tok_embeddings))
            .chain(self.output.as_ref())
    }

    fn bytes(&self, m: &Matrix) -> &[u8] {
        match m.data {
            MatData::File(at) => {
                &self.file.data()[at..at + m.rows * quant::row_bytes(m.dtype, m.cols)]
            }
            MatData::Owned(_) => &[],
        }
    }

    fn f32_values<'a>(&'a self, m: &'a Matrix) -> &'a [f32] {
        match &m.data {
            MatData::Owned(v) => v,
            MatData::File(_) => {
                let b = self.bytes(m);
                // SAFETY: `matrix` keeps f32 data in the file only when it is
                // 4-byte aligned and the CPU is little-endian.
                unsafe { std::slice::from_raw_parts(b.as_ptr() as *const f32, b.len() / 4) }
            }
        }
    }

    /// One row of the embeddings as f32.
    fn embed(&self, token: usize, out: &mut [f32]) {
        let m = &self.tok_embeddings;
        if m.dtype == Dtype::F32 {
            out.copy_from_slice(&self.f32_values(m)[token * m.cols..(token + 1) * m.cols]);
        } else {
            let rb = quant::row_bytes(m.dtype, m.cols);
            let row = &self.bytes(m)[token * rb..(token + 1) * rb];
            quant::dequantize_row(m.dtype, row, m.cols, out);
        }
    }

    /// `out = M · x` on the SIMD kernels, rows split across the pool.
    fn matvec(&self, out: &mut [f32], m: &Matrix, x: &[f32], qx: &mut QuantizedVec) {
        debug_assert_eq!(out.len(), m.rows);
        debug_assert_eq!(x.len(), m.cols);
        let quantized = m.dtype.is_quantized();
        if quantized {
            qx.quantize(x);
        }
        let qx: &QuantizedVec = qx;
        let (f32s, bytes) = if quantized {
            (&[][..], self.bytes(m))
        } else {
            (self.f32_values(m), &[][..])
        };
        let rb = quant::row_bytes(m.dtype, m.cols);
        let rows_dot = |rows: std::ops::Range<usize>, out: &mut [f32]| {
            for (o, r) in out.iter_mut().zip(rows) {
                *o = if quantized {
                    simd::dot_q(m.dtype, &bytes[r * rb..(r + 1) * rb], qx)
                } else {
                    simd::dot_f32(&f32s[r * m.cols..(r + 1) * m.cols], x)
                };
            }
        };
        let threads = self.pool.threads();
        if threads == 1 || m.rows * m.cols < PARALLEL_MIN {
            rows_dot(0..m.rows, out);
            return;
        }
        let chunk = m.rows.div_ceil(threads);
        let ptr = SendPtr(out.as_mut_ptr());
        self.pool.run(threads, &|t| {
            let start = (t * chunk).min(m.rows);
            let end = ((t + 1) * chunk).min(m.rows);
            let p = ptr;
            // SAFETY: each task writes its own disjoint range of `out`, which
            // outlives `run`.
            let part = unsafe { std::slice::from_raw_parts_mut(p.0.add(start), end - start) };
            rows_dot(start..end, part);
        });
    }
}

fn matrix(file: &NativaFile, name: &str, rows: usize, cols: usize) -> Result<Matrix> {
    let (info, raw) = file.tensor_bytes(name, &[rows, cols])?;
    let dtype = info.dtype;
    let in_place = dtype.is_quantized()
        || (cfg!(target_endian = "little") && (raw.as_ptr() as usize).is_multiple_of(4));
    let data = if in_place {
        MatData::File(info.offset)
    } else {
        MatData::Owned(file.tensor_f32(name, &[rows, cols])?)
    };
    Ok(Matrix {
        dtype,
        rows,
        cols,
        data,
    })
}

enum KvCache {
    F32 {
        k: Vec<f32>,
        v: Vec<f32>,
    },
    Q8 {
        k: Vec<i8>,
        v: Vec<i8>,
        ks: Vec<f32>,
        vs: Vec<f32>,
    },
}

fn quantize_head(src: &[f32], dst: &mut [i8]) -> f32 {
    let amax = src.iter().fold(0.0f32, |m, v| m.max(v.abs()));
    let scale = amax / 127.0;
    let inv = if scale > 0.0 { 1.0 / scale } else { 0.0 };
    for (d, s) in dst.iter_mut().zip(src) {
        *d = (s * inv).round().clamp(-127.0, 127.0) as i8;
    }
    scale
}

/// One conversation: the KV cache and the scratch buffers. Feeding tokens
/// one at a time with [`Session::step`] is the whole API; the prompt and
/// the generated tokens go through the same path.
pub struct Session<'m> {
    model: &'m Model,
    pos: usize,
    context: usize,
    kv: KvCache,
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
    qx: QuantizedVec,
}

impl Session<'_> {
    /// Tokens already in the cache.
    pub fn position(&self) -> usize {
        self.pos
    }

    /// Tokens this session can hold.
    pub fn context(&self) -> usize {
        self.context
    }

    pub fn is_full(&self) -> bool {
        self.pos >= self.context
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
                self.context
            )));
        }
        let (kv_dim, hd, pos) = (c.kv_dim(), c.head_dim(), self.pos);
        let group = c.n_heads / c.n_kv_heads;
        let scale = 1.0 / (hd as f32).sqrt();
        m.embed(token as usize, &mut self.x);

        for (l, layer) in m.layers.iter().enumerate() {
            rmsnorm(&mut self.xb, &self.x, &layer.attn_norm, c.norm_eps);
            m.matvec(&mut self.q, &layer.wq, &self.xb, &mut self.qx);
            m.matvec(&mut self.k, &layer.wk, &self.xb, &mut self.qx);
            m.matvec(&mut self.v, &layer.wv, &self.xb, &mut self.qx);
            rope(&mut self.q, pos, hd, c.rope_theta);
            rope(&mut self.k, pos, hd, c.rope_theta);

            // Position `s` of layer `l`, KV head `h`: values and scale index.
            let slot = |s: usize, h: usize| (l * self.context + s) * c.n_kv_heads + h;
            match &mut self.kv {
                KvCache::F32 { k, v } => {
                    let at = slot(pos, 0) * hd;
                    k[at..at + kv_dim].copy_from_slice(&self.k);
                    v[at..at + kv_dim].copy_from_slice(&self.v);
                }
                KvCache::Q8 { k, v, ks, vs } => {
                    for h in 0..c.n_kv_heads {
                        let i = slot(pos, h);
                        let src = h * hd..(h + 1) * hd;
                        ks[i] = quantize_head(&self.k[src.clone()], &mut k[i * hd..(i + 1) * hd]);
                        vs[i] = quantize_head(&self.v[src], &mut v[i * hd..(i + 1) * hd]);
                    }
                }
            }

            for h in 0..c.n_heads {
                let kvh = h / group;
                let q = &self.q[h * hd..(h + 1) * hd];
                let att = &mut self.att[..=pos];
                let out = &mut self.xb2[h * hd..(h + 1) * hd];
                match &self.kv {
                    KvCache::F32 { k, v } => {
                        for (s, a) in att.iter_mut().enumerate() {
                            let i = slot(s, kvh) * hd;
                            *a = simd::dot_f32(q, &k[i..i + hd]) * scale;
                        }
                        softmax(att);
                        out.fill(0.0);
                        for (s, &a) in att.iter().enumerate() {
                            let i = slot(s, kvh) * hd;
                            for (o, val) in out.iter_mut().zip(&v[i..i + hd]) {
                                *o += a * val;
                            }
                        }
                    }
                    KvCache::Q8 { k, v, ks, vs } => {
                        for (s, a) in att.iter_mut().enumerate() {
                            let i = slot(s, kvh);
                            let kh = &k[i * hd..(i + 1) * hd];
                            let d: f32 = q.iter().zip(kh).map(|(x, &y)| x * y as f32).sum();
                            *a = d * ks[i] * scale;
                        }
                        softmax(att);
                        out.fill(0.0);
                        for (s, &a) in att.iter().enumerate() {
                            let i = slot(s, kvh);
                            let w = a * vs[i];
                            for (o, &val) in out.iter_mut().zip(&v[i * hd..(i + 1) * hd]) {
                                *o += w * val as f32;
                            }
                        }
                    }
                }
            }
            m.matvec(&mut self.xb, &layer.wo, &self.xb2, &mut self.qx);
            add(&mut self.x, &self.xb);

            rmsnorm(&mut self.xb, &self.x, &layer.ffn_norm, c.norm_eps);
            m.matvec(&mut self.hb, &layer.w1, &self.xb, &mut self.qx);
            m.matvec(&mut self.hb2, &layer.w3, &self.xb, &mut self.qx);
            for (g, u) in self.hb.iter_mut().zip(&self.hb2) {
                *g = silu(*g) * u;
            }
            m.matvec(&mut self.xb, &layer.w2, &self.hb, &mut self.qx);
            add(&mut self.x, &self.xb);
        }

        rmsnorm(&mut self.xb, &self.x, &m.norm, c.norm_eps);
        let out = m.output.as_ref().unwrap_or(&m.tok_embeddings);
        m.matvec(&mut self.logits, out, &self.xb, &mut self.qx);
        self.pos += 1;
        Ok(&self.logits)
    }
}

fn add(x: &mut [f32], y: &[f32]) {
    for (a, b) in x.iter_mut().zip(y) {
        *a += b;
    }
}
