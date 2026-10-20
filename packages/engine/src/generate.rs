//! The generation loop: prompt, then one token at a time until a stop
//! token, the token limit, a full context or cancellation (the Stop
//! button reaches the engine between any two tokens).

use std::sync::atomic::{AtomicBool, Ordering};

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::grammar::{Matcher, Schema, TokenTrie};
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
    /// The output is a complete value of the schema ([`generate_json`]).
    Complete,
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

/// Within this many tokens of the limit, every candidate is also checked
/// for still being completable in the tokens left.
const FINISH_MARGIN: usize = 128;

/// Like [`generate`], but the output is always a valid value of `schema`
/// (compact JSON). Near the token limit the shortest completion is forced,
/// so the result is complete whenever `max_tokens` allows the schema's
/// smallest value; it is only cut short by cancellation, a full context or
/// the callback.
pub fn generate_json(
    session: &mut Session<'_>,
    prompt: &[u32],
    params: &GenerateParams,
    schema: &Schema,
    trie: &TokenTrie,
    cancel: &AtomicBool,
    mut on_token: impl FnMut(u32) -> bool,
) -> Result<Generation> {
    let minimal = schema.minimal().len();
    if minimal > params.max_tokens {
        return Err(Error::BadConfig(format!(
            "max_tokens {} is below the {minimal} the schema's smallest value needs",
            params.max_tokens
        )));
    }
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
    let mut matcher = Matcher::new(schema);
    let mut masked = Vec::new();
    loop {
        if matcher.is_complete() {
            return finish(tokens, StopReason::Complete);
        }
        if cancel.load(Ordering::Relaxed) {
            return finish(tokens, StopReason::Cancelled);
        }
        let left = params.max_tokens - tokens.len();
        let rest = matcher.finish();
        let next = if rest.len() >= left || logits.is_empty() {
            // Forced: the shortest completion, one byte token at a time.
            let b = rest[0];
            matcher.advance(b);
            u32::from(b)
        } else {
            let near_end = left <= rest.len() + FINISH_MARGIN;
            masked.clear();
            masked.resize(logits.len(), f32::NEG_INFINITY);
            let mut states = Vec::new();
            trie.allowed(&matcher, |t, after| {
                if near_end && after.finish().len() > left - 1 {
                    return;
                }
                masked[t as usize] = logits[t as usize];
                states.push((t, after.clone()));
            });
            if states.is_empty() {
                // Cannot happen with a byte-level vocabulary: the byte the
                // completion needs is always a token. Kept as a clear error.
                return Err(Error::BadConfig("no token fits the schema here".into()));
            }
            let t = sampler.sample(&masked);
            matcher = states
                .into_iter()
                .find(|(id, _)| *id == t)
                .map(|(_, m)| m)
                .unwrap();
            t
        };
        tokens.push(next);
        if !on_token(next) {
            return finish(tokens, StopReason::Callback);
        }
        if matcher.is_complete() {
            return finish(tokens, StopReason::Complete);
        }
        if session.is_full() {
            return finish(tokens, StopReason::ContextFull);
        }
        logits.clear();
        logits.extend_from_slice(session.step(next)?);
    }
}
