//! The Native AI's own engine (ADR-0027).
//!
//! Written by the project, with no inference or training library: only
//! published ideas, each cited in `docs/native-ai.md`.
//!
//! - [`format`]: the `.nativa` file — a versioned header and the tensors.
//! - [`tokenizer`]: byte-level BPE; any language and script fits.
//! - [`model`]: the decoder-only transformer, scalar f32 path (phase M1).
//! - [`sampler`]: greedy, temperature, top-p and min-p, with its own RNG.
//! - [`generate`]: the generation loop, with stop tokens and cancellation.

pub mod error;
pub mod format;
pub mod generate;
pub mod model;
pub mod ops;
pub mod sampler;
pub mod tokenizer;

pub use error::{Error, Result};
pub use format::{Config, Dtype, Header, TensorInfo, FORMAT_VERSION, MAGIC};
pub use generate::{generate, GenerateParams, Generation, StopReason};
pub use model::{Model, Session};
pub use sampler::{Sampler, SamplingParams};
pub use tokenizer::Tokenizer;
