//! Choosing the next token from the logits.
//!
//! - Top-p (nucleus): Holtzman et al., "The Curious Case of Neural Text
//!   Degeneration" (2019).
//! - Min-p: Nguyen et al., "Turning Up the Heat: Min-p Sampling for
//!   Creative and Coherent LLM Outputs" (2024).
//! - RNG: SplitMix64 (Steele, Lea and Flood, 2014), so a seed gives the
//!   same text on every computer.

use serde::{Deserialize, Serialize};

use crate::ops::softmax;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct SamplingParams {
    /// 0 = always the most likely token.
    pub temperature: f32,
    /// Keep the smallest set of tokens whose probability adds up to this
    /// (1 = off).
    pub top_p: f32,
    /// Drop tokens less likely than `min_p` × the most likely one (0 = off).
    pub min_p: f32,
    pub seed: u64,
}

impl Default for SamplingParams {
    fn default() -> Self {
        Self {
            temperature: 0.7,
            top_p: 0.95,
            min_p: 0.05,
            seed: 0,
        }
    }
}

impl SamplingParams {
    pub fn greedy() -> Self {
        Self {
            temperature: 0.0,
            ..Self::default()
        }
    }
}

pub struct Sampler {
    params: SamplingParams,
    state: u64,
    probs: Vec<(u32, f32)>,
}

impl Sampler {
    pub fn new(params: SamplingParams) -> Self {
        Self {
            params,
            state: params.seed,
            probs: Vec::new(),
        }
    }

    pub fn sample(&mut self, logits: &[f32]) -> u32 {
        let p = self.params;
        if p.temperature <= 0.0 {
            return argmax(logits);
        }
        let mut scaled: Vec<f32> = logits.iter().map(|l| l / p.temperature).collect();
        softmax(&mut scaled);
        self.probs.clear();
        self.probs
            .extend(scaled.iter().enumerate().map(|(i, &q)| (i as u32, q)));
        self.probs
            .sort_unstable_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));

        let top = self.probs[0].1;
        let mut keep = self.probs.len();
        if p.min_p > 0.0 {
            keep = self
                .probs
                .iter()
                .take_while(|(_, q)| *q >= p.min_p * top)
                .count();
        }
        if p.top_p < 1.0 {
            let mut sum = 0.0;
            let nucleus = self.probs[..keep]
                .iter()
                .take_while(|(_, q)| {
                    let inside = sum < p.top_p;
                    sum += q;
                    inside
                })
                .count();
            keep = keep.min(nucleus);
        }
        let u = self.next_f32();
        // Tokens with no probability (masked by a grammar) are never kept.
        let nonzero = self.probs.iter().take_while(|(_, q)| *q > 0.0).count();
        let kept = &self.probs[..keep.min(nonzero).max(1)];
        let total: f32 = kept.iter().map(|(_, q)| q).sum();
        let mut r = u * total;
        for &(id, q) in kept {
            r -= q;
            if r <= 0.0 {
                return id;
            }
        }
        kept[kept.len() - 1].0
    }

    fn next_f32(&mut self) -> f32 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^= z >> 31;
        (z >> 40) as f32 / (1u64 << 24) as f32
    }
}

pub fn argmax(x: &[f32]) -> u32 {
    let mut best = 0;
    for (i, v) in x.iter().enumerate() {
        if *v > x[best] {
            best = i;
        }
    }
    best as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn greedy_takes_the_largest() {
        let mut s = Sampler::new(SamplingParams::greedy());
        assert_eq!(s.sample(&[0.1, 3.0, -2.0, 2.9]), 1);
    }

    #[test]
    fn same_seed_same_choices() {
        let logits: Vec<f32> = (0..50).map(|i| ((i * 7) % 11) as f32 * 0.3).collect();
        let run = |seed| {
            let mut s = Sampler::new(SamplingParams {
                seed,
                ..Default::default()
            });
            (0..20).map(|_| s.sample(&logits)).collect::<Vec<_>>()
        };
        assert_eq!(run(42), run(42));
        assert_ne!(run(42), run(43));
    }

    #[test]
    fn min_p_and_top_p_cut_the_tail() {
        // One token far more likely than the rest: both filters keep only it.
        let mut logits = vec![0.0f32; 100];
        logits[7] = 20.0;
        for params in [
            SamplingParams {
                temperature: 1.0,
                top_p: 1.0,
                min_p: 0.1,
                seed: 1,
            },
            SamplingParams {
                temperature: 1.0,
                top_p: 0.5,
                min_p: 0.0,
                seed: 1,
            },
        ] {
            let mut s = Sampler::new(params);
            assert!((0..100).all(|_| s.sample(&logits) == 7));
        }
    }
}
