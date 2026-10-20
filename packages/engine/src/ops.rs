//! The math of the model, scalar f32 (phase M1).
//!
//! This is the reference path: simple loops that later SIMD kernels (M2)
//! are tested against, value for value.

/// `out = W · x`, with `W` row-major `[out.len(), x.len()]`.
pub fn matvec(out: &mut [f32], w: &[f32], x: &[f32]) {
    let cols = x.len();
    debug_assert_eq!(w.len(), out.len() * cols);
    for (o, row) in out.iter_mut().zip(w.chunks_exact(cols)) {
        *o = dot(row, x);
    }
}

pub fn dot(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

/// RMSNorm: Zhang and Sennrich, "Root Mean Square Layer Normalization"
/// (2019).
pub fn rmsnorm(out: &mut [f32], x: &[f32], weight: &[f32], eps: f32) {
    let mean_sq = x.iter().map(|v| v * v).sum::<f32>() / x.len() as f32;
    let scale = 1.0 / (mean_sq + eps).sqrt();
    for ((o, v), w) in out.iter_mut().zip(x).zip(weight) {
        *o = v * scale * w;
    }
}

/// In place, numerically stable (the maximum is subtracted first).
pub fn softmax(x: &mut [f32]) {
    let max = x.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    let mut sum = 0.0;
    for v in x.iter_mut() {
        *v = (*v - max).exp();
        sum += *v;
    }
    for v in x.iter_mut() {
        *v /= sum;
    }
}

/// SiLU (swish): Elfwing, Uchibe and Doya (2017); Ramachandran, Zoph and
/// Le (2017).
pub fn silu(x: f32) -> f32 {
    x / (1.0 + (-x).exp())
}

/// Rotary position embedding on every head of `v` (length `n * head_dim`),
/// rotating the pairs `(2i, 2i + 1)` of each head by `pos * theta^(-2i/d)`.
///
/// Su et al., "RoFormer: Enhanced Transformer with Rotary Position
/// Embedding" (2021).
pub fn rope(v: &mut [f32], pos: usize, head_dim: usize, theta: f32) {
    for head in v.chunks_exact_mut(head_dim) {
        for i in 0..head_dim / 2 {
            let freq = (theta as f64).powf(-((2 * i) as f64) / head_dim as f64);
            let angle = pos as f64 * freq;
            let (sin, cos) = (angle.sin() as f32, angle.cos() as f32);
            let (a, b) = (head[2 * i], head[2 * i + 1]);
            head[2 * i] = a * cos - b * sin;
            head[2 * i + 1] = a * sin + b * cos;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matvec_rows_by_columns() {
        let w = [1.0, 2.0, 3.0, 4.0, 5.0, 6.0];
        let mut out = [0.0; 2];
        matvec(&mut out, &w, &[1.0, 0.0, -1.0]);
        assert_eq!(out, [-2.0, -2.0]);
    }

    #[test]
    fn softmax_sums_to_one_and_survives_large_values() {
        let mut x = [1000.0, 1001.0, 1002.0];
        softmax(&mut x);
        assert!((x.iter().sum::<f32>() - 1.0).abs() < 1e-6);
        assert!(x[2] > x[1] && x[1] > x[0]);
    }

    #[test]
    fn rope_keeps_length_and_is_identity_at_zero() {
        let orig = [0.3, -1.2, 0.7, 2.0];
        let mut v = orig;
        rope(&mut v, 0, 4, 10000.0);
        assert_eq!(v, orig);
        rope(&mut v, 7, 4, 10000.0);
        let norm = |x: &[f32]| x.iter().map(|a| a * a).sum::<f32>();
        assert!((norm(&v) - norm(&orig)).abs() < 1e-5);
        assert_ne!(v, orig);
    }

    #[test]
    fn rmsnorm_unit_scale() {
        let mut out = [0.0; 2];
        rmsnorm(&mut out, &[3.0, 4.0], &[1.0, 1.0], 0.0);
        let rms = (12.5f32).sqrt();
        assert!((out[0] - 3.0 / rms).abs() < 1e-6);
    }
}
