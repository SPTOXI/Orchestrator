//! The Native AI's own engine (ADR-0027).
//!
//! Written by the project, with no inference or training library: only
//! published ideas, each cited in `docs/native-ai.md`.
//!
//! - [`format`]: the `.nativa` file — a versioned header and the tensors.
//! - [`tokenizer`]: byte-level BPE; any language and script fits.
//! - [`quant`]: quantized weights (`q8`, `q4`, ternary `t2`) and their
//!   scalar reference; [`simd`]: the AVX2 and NEON kernels (phase M2).
//! - [`model`]: the decoder-only transformer — weights mapped from the file
//!   ([`mmap`]), products across a thread pool ([`pool`]), KV cache in f32
//!   or 8 bits, memory budget.
//! - [`convert`]: quantizing a model file.
//! - [`sampler`]: greedy, temperature, top-p and min-p, with its own RNG.
//! - [`generate`]: the generation loop, with stop tokens and cancellation.

pub mod convert;
pub mod error;
pub mod format;
pub mod generate;
pub mod mmap;
pub mod model;
pub mod ops;
pub mod pool;
pub mod quant;
pub mod sampler;
pub mod simd;
pub mod tokenizer;

pub use error::{Error, Result};
pub use format::{Config, Dtype, Header, NativaFile, TensorInfo, FORMAT_VERSION, MAGIC};
pub use generate::{generate, GenerateParams, Generation, StopReason};
pub use model::{KvPrecision, LoadOptions, MemoryReport, Model, Session, SessionOptions};
pub use sampler::{Sampler, SamplingParams};
pub use tokenizer::Tokenizer;
