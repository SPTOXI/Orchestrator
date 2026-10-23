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
use crate::lora::{Adapter, Target};
use crate::ops::{rmsnorm, rope, silu, softmax};
use crate::pool::{Pool, SendPtr};
use crate::quant::{self, QuantizedVec};
use crate::simd;
use crate::tokenizer::Tokenizer;

/// Below this many weights a product runs on one thread: waking the pool
/// would cost more than it saves.
const PARALLEL_MIN: usize = 1 << 15;

/// Below this many positions a batched product stays on the CPU: the trip
/// to the GPU and back would cost more than it saves.
#[cfg(feature = "gpu")]
const GPU_MIN_POSITIONS: usize = 8;

pub(crate) enum MatData {
    /// In the file, at this offset of the data section.
    File(usize),
    /// f32 copied out of a file that could not be used in place (an
    /// unaligned buffer or a big-endian CPU).
    Owned(Vec<f32>),
}

pub(crate) struct Matrix {
    pub(crate) dtype: Dtype,
    pub(crate) rows: usize,
    pub(crate) cols: usize,
    pub(crate) data: MatData,
}

pub(crate) struct Layer {
    pub(crate) attn_norm: Vec<f32>,
    pub(crate) wq: Matrix,
    pub(crate) wk: Matrix,
    pub(crate) wv: Matrix,
    pub(crate) wo: Matrix,
    pub(crate) ffn_norm: Vec<f32>,
    pub(crate) w1: Matrix,
    pub(crate) w2: Matrix,
    pub(crate) w3: Matrix,
}

#[derive(Debug, Clone, Default)]
pub struct LoadOptions {
    /// Threads for matrix products (the caller included). Default: the
    /// processors the system reports.
    pub threads: Option<usize>,
    /// Runs the batched products (prompt and training) on the GPU; needs
    /// the `gpu` feature. Generation one token at a time stays on the CPU.
    pub gpu: Option<GpuOptions>,
}

/// What to give the GPU.
#[derive(Debug, Clone)]
pub struct GpuOptions {
    /// Video memory the weights may take, in bytes. Default: 768 MiB, which
    /// leaves room for the activations and the desktop on a 1 GB card.
    pub memory: u64,
    /// Always use the GPU where it can run, instead of measuring it against
    /// the CPU first (tests, benchmarks).
    pub always: bool,
    /// Where to remember which side won each kind of product, so the
    /// measurement happens once per machine instead of once per run. It is
    /// thrown away when the adapter or the kernels change.
    pub choices_file: Option<std::path::PathBuf>,
}

impl Default for GpuOptions {
    fn default() -> Self {
        Self {
            memory: 768 << 20,
            always: false,
            choices_file: None,
        }
    }
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
    #[cfg(feature = "gpu")]
    gpu: Option<crate::gpu::Gpu>,
    pub(crate) tok_embeddings: Matrix,
    pub(crate) layers: Vec<Layer>,
    pub(crate) norm: Vec<f32>,
    pub(crate) output: Option<Matrix>,
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
        #[cfg(feature = "gpu")]
        let gpu = options.gpu.as_ref().map(crate::gpu::Gpu::new).transpose()?;
        #[cfg(not(feature = "gpu"))]
        if options.gpu.is_some() {
            return Err(Error::Gpu(
                "this engine was built without the gpu feature".into(),
            ));
        }
        Ok(Self {
            #[cfg(feature = "gpu")]
            gpu,
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

    /// Where the batched products run: "cpu", or the GPU adapter's name.
    pub fn device(&self) -> String {
        #[cfg(feature = "gpu")]
        if let Some(g) = &self.gpu {
            return g.name().to_string();
        }
        "cpu".into()
    }

    /// Weight bytes in video memory (0 on the CPU).
    pub fn gpu_resident_bytes(&self) -> u64 {
        #[cfg(feature = "gpu")]
        if let Some(g) = &self.gpu {
            return g.resident_bytes();
        }
        0
    }

    /// Batched products run so far on the GPU and on the CPU (with a GPU
    /// present; products too small for it are not counted).
    pub fn gpu_share(&self) -> (u64, u64) {
        #[cfg(feature = "gpu")]
        if let Some(g) = &self.gpu {
            return g.share();
        }
        (0, 0)
    }

    /// The matrix as the GPU takes it, if it should go there: a GPU, and
    /// enough positions to be worth the trip.
    #[cfg(feature = "gpu")]
    fn on_gpu<'a>(
        &'a self,
        m: &'a Matrix,
        t_len: usize,
    ) -> Option<(&'a crate::gpu::Gpu, crate::gpu::Weights<'a>)> {
        let g = self.gpu.as_ref()?;
        if t_len < GPU_MIN_POSITIONS {
            return None;
        }
        let (key, bytes) = match &m.data {
            MatData::File(at) => (*at, self.bytes(m)),
            MatData::Owned(v) => (
                // Heap addresses never collide with file offsets' low range
                // in practice; the top bit keeps the two apart for sure.
                v.as_ptr() as usize | 1 << (usize::BITS - 1),
                // SAFETY: f32 has no padding; any byte is a valid u8.
                unsafe { std::slice::from_raw_parts(v.as_ptr() as *const u8, v.len() * 4) },
            ),
        };
        let w = crate::gpu::Weights {
            key,
            dtype: m.dtype,
            rows: m.rows,
            cols: m.cols,
            bytes,
        };
        g.fits(&w).then_some((g, w))
    }

    /// Runs `f(task)` for `tasks` tasks across the engine's threads.
    pub(crate) fn parallel(&self, tasks: usize, f: &(dyn Fn(usize) + Sync)) {
        self.pool.run(tasks, f);
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

    /// A text embedding: the mean of the last layer's normalized hidden
    /// state over the tokens, scaled to unit length, so the dot product of
    /// two embeddings is their cosine similarity. The same model that
    /// writes also finds things in the memory and the Library (ADR-0027
    /// §1). Texts longer than the context are cut at its end.
    pub fn embed(&self, tokens: &[u32]) -> Result<Vec<f32>> {
        let n = tokens.len().min(self.config.max_seq_len);
        if n == 0 {
            return Err(Error::BadConfig("nothing to embed".into()));
        }
        let mut s = self.session_with(&SessionOptions {
            context: Some(n),
            kv: KvPrecision::F32,
            memory_budget: None,
        })?;
        let mut mean = vec![0.0f32; self.config.dim];
        for &t in &tokens[..n] {
            s.step(t)?;
            for (m, h) in mean.iter_mut().zip(s.hidden()) {
                *m += h;
            }
        }
        let norm = mean.iter().map(|v| v * v).sum::<f32>().sqrt();
        if norm > 0.0 {
            mean.iter_mut().for_each(|v| *v /= norm);
        }
        Ok(mean)
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
            adapter: None,
            lora_u: Vec::new(),
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

    pub(crate) fn bytes(&self, m: &Matrix) -> &[u8] {
        match m.data {
            MatData::File(at) => {
                &self.file.data()[at..at + m.rows * quant::row_bytes(m.dtype, m.cols)]
            }
            MatData::Owned(_) => &[],
        }
    }

    pub(crate) fn f32_values<'a>(&'a self, m: &'a Matrix) -> &'a [f32] {
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
    pub(crate) fn embed_row(&self, token: usize, out: &mut [f32]) {
        let m = &self.tok_embeddings;
        if m.dtype == Dtype::F32 {
            out.copy_from_slice(&self.f32_values(m)[token * m.cols..(token + 1) * m.cols]);
        } else {
            let rb = quant::row_bytes(m.dtype, m.cols);
            let row = &self.bytes(m)[token * rb..(token + 1) * rb];
            quant::dequantize_row(m.dtype, row, m.cols, out);
        }
    }

    /// `out[t] = M · x[t]` for every row `t` of `x` (`T × cols`), reading
    /// each weight row once for all positions — the prompt and training
    /// paths. Every `(row, t)` uses the same kernel as [`Model::matvec`], so
    /// the numbers are the same as one position at a time.
    pub(crate) fn matmul(
        &self,
        out: &mut [f32],
        m: &Matrix,
        x: &[f32],
        qx: &mut Vec<QuantizedVec>,
    ) {
        let t_len = x.len() / m.cols;
        debug_assert_eq!(out.len(), t_len * m.rows);
        #[cfg(feature = "gpu")]
        if let Some((g, w)) = self.on_gpu(m, t_len) {
            let mut cpu = |o: &mut [f32]| self.matmul_cpu(o, m, x, qx);
            if g.choose(&w, false, t_len, out, &mut |o| g.matmul(&w, x, o), &mut cpu) {
                return;
            }
        }
        self.matmul_cpu(out, m, x, qx);
    }

    fn matmul_cpu(&self, out: &mut [f32], m: &Matrix, x: &[f32], qx: &mut Vec<QuantizedVec>) {
        let t_len = x.len() / m.cols;
        let quantized = m.dtype.is_quantized();
        if quantized {
            qx.resize_with(t_len.max(qx.len()), QuantizedVec::default);
            for (q, row) in qx.iter_mut().zip(x.chunks_exact(m.cols)) {
                q.quantize(row);
            }
        }
        let qx: &[QuantizedVec] = qx;
        let (f32s, bytes) = if quantized {
            (&[][..], self.bytes(m))
        } else {
            (self.f32_values(m), &[][..])
        };
        let rb = quant::row_bytes(m.dtype, m.cols);
        let ptr = SendPtr(out.as_mut_ptr());
        // 4- and 2-bit rows are unpacked once per row into q8 blocks (same
        // integers, same scale), then reused for every position.
        let repack = quantized && m.dtype != Dtype::Q8;
        let rows_dot = |rows: std::ops::Range<usize>| {
            let p = ptr;
            let mut q8 = Vec::new();
            let mut r = rows.start;
            // Quantized rows four at a time: one activation load per block
            // serves four weight rows.
            if quantized {
                let q8b = quant::row_bytes(Dtype::Q8, m.cols);
                while r + 4 <= rows.end {
                    let group: [&[u8]; 4] = if repack {
                        q8.clear();
                        for k in 0..4 {
                            let row = &bytes[(r + k) * rb..(r + k + 1) * rb];
                            quant::repack_q8(m.dtype, row, &mut q8);
                        }
                        std::array::from_fn(|k| &q8[k * q8b..(k + 1) * q8b])
                    } else {
                        std::array::from_fn(|k| &bytes[(r + k) * rb..(r + k + 1) * rb])
                    };
                    for (t, x) in qx.iter().enumerate().take(t_len) {
                        let v = simd::dot_q8_x4(group, x);
                        for (k, val) in v.into_iter().enumerate() {
                            // SAFETY: every (t, r) is written by exactly one task.
                            unsafe { *p.0.add(t * m.rows + r + k) = val };
                        }
                    }
                    r += 4;
                }
            }
            for r in r..rows.end {
                let row = if quantized {
                    &bytes[r * rb..(r + 1) * rb]
                } else {
                    &[][..]
                };
                let (kind, row) = if repack {
                    q8.clear();
                    quant::repack_q8(m.dtype, row, &mut q8);
                    (Dtype::Q8, &q8[..])
                } else {
                    (m.dtype, row)
                };
                for t in 0..t_len {
                    let v = if quantized {
                        simd::dot_q(kind, row, &qx[t])
                    } else {
                        simd::dot_f32(
                            &f32s[r * m.cols..(r + 1) * m.cols],
                            &x[t * m.cols..(t + 1) * m.cols],
                        )
                    };
                    // SAFETY: every (t, r) is written by exactly one task.
                    unsafe { *p.0.add(t * m.rows + r) = v };
                }
            }
        };
        let threads = self.pool.threads();
        if threads == 1 || t_len * m.rows * m.cols < PARALLEL_MIN {
            rows_dot(0..m.rows);
            return;
        }
        let chunk = m.rows.div_ceil(threads);
        self.pool.run(threads, &|i| {
            rows_dot((i * chunk).min(m.rows)..((i + 1) * chunk).min(m.rows));
        });
    }

    /// `dx[t] += Mᵀ · dy[t]` for every position, each weight row expanded
    /// once for all of them.
    pub(crate) fn matmul_t_add(&self, dx: &mut [f32], m: &Matrix, dy: &[f32]) {
        let t_len = dy.len() / m.rows;
        debug_assert_eq!(dx.len(), t_len * m.cols);
        #[cfg(feature = "gpu")]
        if let Some((g, w)) = self.on_gpu(m, t_len) {
            let mut part = vec![0.0; dx.len()];
            let mut cpu = |o: &mut [f32]| {
                o.fill(0.0);
                self.matmul_t_add_cpu(o, m, dy)
            };
            if g.choose(
                &w,
                true,
                t_len,
                &mut part,
                &mut |o| g.matmul_t(&w, dy, o),
                &mut cpu,
            ) {
                simd::axpy(dx, 1.0, &part);
                return;
            }
        }
        self.matmul_t_add_cpu(dx, m, dy);
    }

    fn matmul_t_add_cpu(&self, dx: &mut [f32], m: &Matrix, dy: &[f32]) {
        let t_len = dy.len() / m.rows;
        let rb = quant::row_bytes(m.dtype, m.cols);
        let quantized = m.dtype.is_quantized();
        let f32s = if quantized {
            &[][..]
        } else {
            self.f32_values(m)
        };
        let bytes = if quantized { self.bytes(m) } else { &[][..] };
        // Rows in tiles: a tile is expanded once, then each position's
        // gradient row stays in cache while the tile's rows are added to it.
        const TILE: usize = 8;
        let rows_into = |rows: std::ops::Range<usize>, acc: &mut [f32]| {
            let mut tile = vec![0.0; if quantized { TILE * m.cols } else { 0 }];
            let mut start = rows.start;
            while start < rows.end {
                let end = (start + TILE).min(rows.end);
                let w: &[f32] = if quantized {
                    for (i, r) in (start..end).enumerate() {
                        let dst = &mut tile[i * m.cols..(i + 1) * m.cols];
                        quant::dequantize_row(m.dtype, &bytes[r * rb..(r + 1) * rb], m.cols, dst);
                    }
                    &tile[..(end - start) * m.cols]
                } else {
                    &f32s[start * m.cols..end * m.cols]
                };
                for t in 0..t_len {
                    let a = &mut acc[t * m.cols..(t + 1) * m.cols];
                    if end - start == TILE {
                        let g: [f32; TILE] = std::array::from_fn(|i| dy[t * m.rows + start + i]);
                        let rows: [&[f32]; TILE] =
                            std::array::from_fn(|i| &w[i * m.cols..(i + 1) * m.cols]);
                        simd::axpy_x8(a, g, rows);
                    } else {
                        for (i, r) in (start..end).enumerate() {
                            let g = dy[t * m.rows + r];
                            if g != 0.0 {
                                simd::axpy(a, g, &w[i * m.cols..(i + 1) * m.cols]);
                            }
                        }
                    }
                }
                start = end;
            }
        };
        let threads = self.pool.threads();
        if threads == 1 || t_len * m.rows * m.cols < PARALLEL_MIN {
            rows_into(0..m.rows, dx);
            return;
        }
        let chunk = m.rows.div_ceil(threads);
        let partial: Vec<std::sync::Mutex<Vec<f32>>> = (0..threads)
            .map(|_| std::sync::Mutex::new(Vec::new()))
            .collect();
        self.pool.run(threads, &|i| {
            let mut acc = vec![0.0; t_len * m.cols];
            rows_into(
                (i * chunk).min(m.rows)..((i + 1) * chunk).min(m.rows),
                &mut acc,
            );
            *partial[i].lock().unwrap() = acc;
        });
        // Summed in a fixed order: the result does not depend on timing.
        for p in partial {
            let p = p.into_inner().unwrap();
            if !p.is_empty() {
                simd::axpy(dx, 1.0, &p);
            }
        }
    }

    /// `out = M · x` on the SIMD kernels, rows split across the pool.
    pub(crate) fn matvec(&self, out: &mut [f32], m: &Matrix, x: &[f32], qx: &mut QuantizedVec) {
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

impl KvCache {
    /// Keys and values of position `pos` in layer `l` (`ctx` positions per
    /// layer, `n_kv` heads of `hd` values).
    #[allow(clippy::too_many_arguments)]
    fn store(
        &mut self,
        ctx: usize,
        n_kv: usize,
        hd: usize,
        l: usize,
        pos: usize,
        k: &[f32],
        v: &[f32],
    ) {
        let slot = |s: usize, h: usize| (l * ctx + s) * n_kv + h;
        match self {
            KvCache::F32 { k: kc, v: vc } => {
                let at = slot(pos, 0) * hd;
                kc[at..at + n_kv * hd].copy_from_slice(k);
                vc[at..at + n_kv * hd].copy_from_slice(v);
            }
            KvCache::Q8 {
                k: kc,
                v: vc,
                ks,
                vs,
            } => {
                for h in 0..n_kv {
                    let i = slot(pos, h);
                    let src = h * hd..(h + 1) * hd;
                    ks[i] = quantize_head(&k[src.clone()], &mut kc[i * hd..(i + 1) * hd]);
                    vs[i] = quantize_head(&v[src], &mut vc[i * hd..(i + 1) * hd]);
                }
            }
        }
    }

    /// One query head at position `pos` attending over positions `0..=pos`
    /// of KV head `kvh`; the result goes to `out`.
    #[allow(clippy::too_many_arguments)]
    fn attend(
        &self,
        ctx: usize,
        n_kv: usize,
        hd: usize,
        l: usize,
        pos: usize,
        kvh: usize,
        q: &[f32],
        scale: f32,
        att: &mut [f32],
        out: &mut [f32],
    ) {
        debug_assert_eq!(att.len(), pos + 1);
        let slot = |s: usize| (l * ctx + s) * n_kv + kvh;
        match self {
            KvCache::F32 { k, v } => {
                for (s, a) in att.iter_mut().enumerate() {
                    let i = slot(s) * hd;
                    *a = simd::dot_f32(q, &k[i..i + hd]) * scale;
                }
                softmax(att);
                out.fill(0.0);
                for (s, &a) in att.iter().enumerate() {
                    let i = slot(s) * hd;
                    for (o, val) in out.iter_mut().zip(&v[i..i + hd]) {
                        *o += a * val;
                    }
                }
            }
            KvCache::Q8 { k, v, ks, vs } => {
                for (s, a) in att.iter_mut().enumerate() {
                    let i = slot(s);
                    let kh = &k[i * hd..(i + 1) * hd];
                    let d: f32 = q.iter().zip(kh).map(|(x, &y)| x * y as f32).sum();
                    *a = d * ks[i] * scale;
                }
                softmax(att);
                out.fill(0.0);
                for (s, &a) in att.iter().enumerate() {
                    let i = slot(s);
                    let w = a * vs[i];
                    for (o, &val) in out.iter_mut().zip(&v[i * hd..(i + 1) * hd]) {
                        *o += w * val as f32;
                    }
                }
            }
        }
    }
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
    adapter: Option<&'m Adapter>,
    lora_u: Vec<f32>,
}

impl<'m> Session<'m> {
    /// Tokens already in the cache.
    pub fn position(&self) -> usize {
        self.pos
    }

    /// Tokens this session can hold.
    pub fn context(&self) -> usize {
        self.context
    }

    /// Runs with `adapter` added to the model's weights. Only before the
    /// first token.
    pub fn with_adapter(mut self, adapter: &'m Adapter) -> Result<Self> {
        adapter.check(self.model)?;
        if self.pos > 0 {
            return Err(Error::BadConfig(
                "set the adapter before the first token".into(),
            ));
        }
        self.lora_u = vec![0.0; adapter.config.rank];
        self.adapter = Some(adapter);
        Ok(self)
    }

    /// The last layer's normalized hidden state for the last token fed
    /// (what the output matrix turns into logits).
    pub fn hidden(&self) -> &[f32] {
        &self.xb
    }

    pub fn is_full(&self) -> bool {
        self.pos >= self.context
    }

    /// Feeds several tokens at once (a prompt) and returns the logits for
    /// the next one. Each weight row is read once for all of them, and
    /// every number is the same as feeding them one at a time with
    /// [`Session::step`].
    pub fn prefill(&mut self, tokens: &[u32]) -> Result<&[f32]> {
        let m = self.model;
        let c = &m.config;
        match tokens.len() {
            0 => return Err(Error::BadConfig("nothing to feed".into())),
            1 => return self.step(tokens[0]),
            _ => {}
        }
        if let Some(&token) = tokens.iter().find(|&&t| t as usize >= c.vocab_size) {
            return Err(Error::BadToken {
                token,
                vocab_size: c.vocab_size,
            });
        }
        if self.pos + tokens.len() > self.context {
            return Err(Error::BadConfig(format!(
                "{} tokens do not fit: {} of {} already used",
                tokens.len(),
                self.pos,
                self.context
            )));
        }
        let n = tokens.len();
        let (dim, kv_dim, hd, n_kv) = (c.dim, c.kv_dim(), c.head_dim(), c.n_kv_heads);
        let group = c.n_heads / n_kv;
        let scale = 1.0 / (hd as f32).sqrt();
        let pos0 = self.pos;
        let mut qx = Vec::new();
        let mut x = vec![0.0; n * dim];
        for (t, &tok) in tokens.iter().enumerate() {
            m.embed_row(tok as usize, &mut x[t * dim..(t + 1) * dim]);
        }
        let rows = |v: &[f32], w: &[f32], out: &mut [f32]| {
            for (o, r) in out.chunks_exact_mut(w.len()).zip(v.chunks_exact(w.len())) {
                rmsnorm(o, r, w, c.norm_eps);
            }
        };
        let mut xn = vec![0.0; n * dim];
        let mut ao = vec![0.0; n * dim];
        let mut proj = vec![0.0; n * dim];
        for (l, layer) in m.layers.iter().enumerate() {
            // `W·x` for every position, plus the adapter's share.
            let linear = |w: &Matrix,
                          target: Target,
                          input: &[f32],
                          out: &mut Vec<f32>,
                          qx: &mut Vec<QuantizedVec>,
                          u: &mut Vec<f32>| {
                out.resize(n * w.rows, 0.0);
                m.matmul(out, w, input, qx);
                if let Some(a) = self.adapter {
                    if let Some(slot) = a.slot(l, target) {
                        u.resize(a.config.rank, 0.0);
                        for t in 0..n {
                            a.apply(
                                slot,
                                &input[t * w.cols..(t + 1) * w.cols],
                                u,
                                &mut out[t * w.rows..(t + 1) * w.rows],
                            );
                        }
                    }
                }
            };
            let mut u = std::mem::take(&mut self.lora_u);
            rows(&x, &layer.attn_norm, &mut xn);
            let (mut q, mut k, mut v) = (Vec::new(), Vec::new(), Vec::new());
            linear(&layer.wq, Target::Wq, &xn, &mut q, &mut qx, &mut u);
            linear(&layer.wk, Target::Wk, &xn, &mut k, &mut qx, &mut u);
            linear(&layer.wv, Target::Wv, &xn, &mut v, &mut qx, &mut u);
            for t in 0..n {
                let pos = pos0 + t;
                rope(&mut q[t * dim..(t + 1) * dim], pos, hd, c.rope_theta);
                rope(&mut k[t * kv_dim..(t + 1) * kv_dim], pos, hd, c.rope_theta);
                self.kv.store(
                    self.context,
                    n_kv,
                    hd,
                    l,
                    pos,
                    &k[t * kv_dim..(t + 1) * kv_dim],
                    &v[t * kv_dim..(t + 1) * kv_dim],
                );
            }
            for t in 0..n {
                let pos = pos0 + t;
                for h in 0..c.n_heads {
                    self.kv.attend(
                        self.context,
                        n_kv,
                        hd,
                        l,
                        pos,
                        h / group,
                        &q[t * dim + h * hd..t * dim + (h + 1) * hd],
                        scale,
                        &mut self.att[..=pos],
                        &mut ao[t * dim + h * hd..t * dim + (h + 1) * hd],
                    );
                }
            }
            linear(&layer.wo, Target::Wo, &ao, &mut proj, &mut qx, &mut u);
            add(&mut x, &proj);
            rows(&x, &layer.ffn_norm, &mut xn);
            let (mut h1, mut h3) = (Vec::new(), Vec::new());
            linear(&layer.w1, Target::W1, &xn, &mut h1, &mut qx, &mut u);
            linear(&layer.w3, Target::W3, &xn, &mut h3, &mut qx, &mut u);
            for (g, up) in h1.iter_mut().zip(&h3) {
                *g = silu(*g) * up;
            }
            linear(&layer.w2, Target::W2, &h1, &mut proj, &mut qx, &mut u);
            add(&mut x, &proj);
            self.lora_u = u;
        }
        self.x.copy_from_slice(&x[(n - 1) * dim..]);
        rmsnorm(&mut self.xb, &self.x, &m.norm, c.norm_eps);
        let out = m.output.as_ref().unwrap_or(&m.tok_embeddings);
        m.matvec(&mut self.logits, out, &self.xb, &mut self.qx);
        self.pos += n;
        Ok(&self.logits)
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
        let (hd, pos) = (c.head_dim(), self.pos);
        let group = c.n_heads / c.n_kv_heads;
        let scale = 1.0 / (hd as f32).sqrt();
        m.embed_row(token as usize, &mut self.x);

        for (l, layer) in m.layers.iter().enumerate() {
            rmsnorm(&mut self.xb, &self.x, &layer.attn_norm, c.norm_eps);
            // `W·x`, plus the adapter's `scale·B·A·x` when it has this matrix.
            macro_rules! linear {
                ($out:ident, $w:ident, $x:ident, $target:expr) => {
                    m.matvec(&mut self.$out, &layer.$w, &self.$x, &mut self.qx);
                    if let Some(slot) = self.adapter.and_then(|a| a.slot(l, $target)) {
                        let a = self.adapter.unwrap();
                        a.apply(slot, &self.$x, &mut self.lora_u, &mut self.$out);
                    }
                };
            }
            linear!(q, wq, xb, Target::Wq);
            linear!(k, wk, xb, Target::Wk);
            linear!(v, wv, xb, Target::Wv);
            rope(&mut self.q, pos, hd, c.rope_theta);
            rope(&mut self.k, pos, hd, c.rope_theta);

            let n_kv = c.n_kv_heads;
            self.kv
                .store(self.context, n_kv, hd, l, pos, &self.k, &self.v);
            for h in 0..c.n_heads {
                self.kv.attend(
                    self.context,
                    n_kv,
                    hd,
                    l,
                    pos,
                    h / group,
                    &self.q[h * hd..(h + 1) * hd],
                    scale,
                    &mut self.att[..=pos],
                    &mut self.xb2[h * hd..(h + 1) * hd],
                );
            }
            linear!(xb, wo, xb2, Target::Wo);
            add(&mut self.x, &self.xb);

            rmsnorm(&mut self.xb, &self.x, &layer.ffn_norm, c.norm_eps);
            linear!(hb, w1, xb, Target::W1);
            linear!(hb2, w3, xb, Target::W3);
            for (g, u) in self.hb.iter_mut().zip(&self.hb2) {
                *g = silu(*g) * u;
            }
            linear!(xb, w2, hb, Target::W2);
            add(&mut self.x, &self.xb);
        }

        rmsnorm(&mut self.xb, &self.x, &m.norm, c.norm_eps);
        let out = m.output.as_ref().unwrap_or(&m.tok_embeddings);
        m.matvec(&mut self.logits, out, &self.xb, &mut self.qx);
        self.pos += 1;
        Ok(&self.logits)
    }
}

/// Cosine similarity of two embeddings from [`Model::embed`] (already unit
/// length, so the dot product).
pub fn similarity(a: &[f32], b: &[f32]) -> f32 {
    simd::dot_f32(a, b)
}

fn add(x: &mut [f32], y: &[f32]) {
    for (a, b) in x.iter_mut().zip(y) {
        *a += b;
    }
}
