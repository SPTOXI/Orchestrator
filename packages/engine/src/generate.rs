//! The generation loop: prompt, then one token at a time until a stop
//! token, the token limit, a full context or cancellation (the Stop
//! button reaches the engine between any two tokens).

use std::sync::atomic::{AtomicBool, Ordering};

use serde::{Deserialize, Serialize};

use crate::error::Result;
use crate::model::Session;
use crate::sampler::{Sampler, SamplingParams};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GenerateParams {
    pub max_tokens: usize,
    pub sampling: SamplingParams,
    /// Generation ends when one of these is produced (it is not returned).
    pub stop: Vec<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    StopToken,
    MaxTokens,
    ContextFull,
    Cancelled,
    /// The callback asked to stop.
    Callback,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Generation {
    pub tokens: Vec<u32>,
    pub stop: StopReason,
}

/// Feeds `prompt` into `session` and generates. `on_token` sees every
/// generated token as it comes and returns `false` to stop.
pub fn generate(
    session: &mut Session<'_>,
    prompt: &[u32],
    params: &GenerateParams,
    cancel: &AtomicBool,
    mut on_token: impl FnMut(u32) -> bool,
) -> Result<Generation> {
    let finish = |tokens, stop| Ok(Generation { tokens, stop });
    let mut tokens = Vec::new();
    let mut logits = Vec::new();
    for &t in prompt {
        if cancel.load(Ordering::Relaxed) {
            return finish(tokens, StopReason::Cancelled);
        }
        if session.is_full() {
            return finish(tokens, StopReason::ContextFull);
        }
        logits.clear();
        logits.extend_from_slice(session.step(t)?);
    }
    let mut sampler = Sampler::new(params.sampling);
    loop {
        if tokens.len() >= params.max_tokens {
            return finish(tokens, StopReason::MaxTokens);
        }
        if cancel.load(Ordering::Relaxed) {
            return finish(tokens, StopReason::Cancelled);
        }
        if logits.is_empty() {
            // Empty prompt and empty session: nothing to continue from.
            return finish(tokens, StopReason::MaxTokens);
        }
        let next = sampler.sample(&logits);
        if params.stop.contains(&next) {
            return finish(tokens, StopReason::StopToken);
        }
        tokens.push(next);
        if !on_token(next) {
            return finish(tokens, StopReason::Callback);
        }
        if session.is_full() {
            return finish(tokens, StopReason::ContextFull);
        }
        logits.clear();
        logits.extend_from_slice(session.step(next)?);
    }
}
