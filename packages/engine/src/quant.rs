//! Quantized weights (phase M2): fewer bits per weight, less memory and
//! less memory bandwidth — on a CPU, generation speed is mostly how fast
//! the weights can be read.
//!
//! Every quantized row is a run of blocks of [`BLOCK`] values. Each block
//! is a little-endian f32 scale followed by the packed values (layouts are
//! the project's own):
//!
//! | dtype | bytes per block | value |
//! | ----- | --------------- | ----- |
//! | `q8`  | 4 + 32 | `i8 × scale`; byte `j` is value `j` |
//! | `q4`  | 4 + 16 | `(n − 8) × scale`; byte `j` holds value `2j` in the low nibble and `2j + 1` in the high one |
//! | `t2`  | 4 + 8  | `{0, +1, −1}[c] × scale`; byte `j` holds values `4j..4j+4`, value `4j + k` in bits `2k..2k+2` |
//!
//! - Symmetric block quantization with a per-block scale (absmax): Dettmers
//!   et al., "LLM.int8(): 8-bit Matrix Multiplication for Transformers at
//!   Scale" (2022).
//! - Ternary weights with the absmean scale: Ma et al., "The Era of 1-bit
//!   LLMs: All Large Language Models are in 1.58 Bits" (2024).
//!
//! The activations a quantized matrix multiplies are themselves quantized
//! to `q8` blocks, so every dot product runs on integers and is scaled once
//! per block. The functions here are the scalar reference; [`crate::simd`]
//! must give the same integers.

use crate::format::Dtype;

/// Values per block.
pub const BLOCK: usize = 32;

/// Activations as `q8` blocks: one scale and 32 values per block.
#[derive(Debug, Clone, Default)]
pub struct QuantizedVec {
    pub scales: Vec<f32>,
    pub values: Vec<i8>,
}

impl QuantizedVec {
    pub fn quantize(&mut self, x: &[f32]) {
        debug_assert!(x.len().is_multiple_of(BLOCK));
        self.scales.clear();
        self.values.clear();
        for block in x.chunks_exact(BLOCK) {
            let amax = block.iter().fold(0.0f32, |m, v| m.max(v.abs()));
            let scale = amax / 127.0;
            let inv = if scale > 0.0 { 1.0 / scale } else { 0.0 };
            self.scales.push(scale);
            self.values.extend(
                block
                    .iter()
                    .map(|v| (v * inv).round().clamp(-127.0, 127.0) as i8),
            );
        }
    }
}

pub fn block_bytes(dtype: Dtype) -> usize {
    match dtype {
        Dtype::F32 => 4,
        Dtype::Q8 => 4 + BLOCK,
        Dtype::Q4 => 4 + BLOCK / 2,
        Dtype::T2 => 4 + BLOCK / 4,
    }
}

/// Bytes of one row of `cols` values.
pub fn row_bytes(dtype: Dtype, cols: usize) -> usize {
    match dtype {
        Dtype::F32 => cols * 4,
        _ => cols / BLOCK * block_bytes(dtype),
    }
}

pub fn scale_of(block: &[u8]) -> f32 {
    f32::from_le_bytes(block[..4].try_into().unwrap())
}

/// Quantizes one row (its length a multiple of [`BLOCK`]).
pub fn quantize_row(dtype: Dtype, row: &[f32], out: &mut Vec<u8>) {
    match dtype {
        Dtype::F32 => out.extend(row.iter().flat_map(|v| v.to_le_bytes())),
        Dtype::Q8 => {
            for block in row.chunks_exact(BLOCK) {
                let amax = block.iter().fold(0.0f32, |m, v| m.max(v.abs()));
                let scale = amax / 127.0;
                let inv = if scale > 0.0 { 1.0 / scale } else { 0.0 };
                out.extend(scale.to_le_bytes());
                out.extend(
                    block
                        .iter()
                        .map(|v| (v * inv).round().clamp(-127.0, 127.0) as i8 as u8),
                );
            }
        }
        Dtype::Q4 => {
            for block in row.chunks_exact(BLOCK) {
                let amax = block.iter().fold(0.0f32, |m, v| m.max(v.abs()));
                let scale = amax / 7.0;
                let inv = if scale > 0.0 { 1.0 / scale } else { 0.0 };
                let q = |v: f32| ((v * inv).round().clamp(-7.0, 7.0) as i32 + 8) as u8;
                out.extend(scale.to_le_bytes());
                out.extend(block.chunks_exact(2).map(|p| q(p[0]) | (q(p[1]) << 4)));
            }
        }
        Dtype::T2 => {
            for block in row.chunks_exact(BLOCK) {
                let scale = block.iter().map(|v| v.abs()).sum::<f32>() / BLOCK as f32;
                let inv = if scale > 0.0 { 1.0 / scale } else { 0.0 };
                let code = |v: f32| match (v * inv).round().clamp(-1.0, 1.0) as i32 {
                    1 => 1u8,
                    -1 => 2u8,
                    _ => 0u8,
                };
                out.extend(scale.to_le_bytes());
                out.extend(block.chunks_exact(4).map(|q| {
                    code(q[0]) | (code(q[1]) << 2) | (code(q[2]) << 4) | (code(q[3]) << 6)
                }));
            }
        }
    }
}

/// The integer values of one block, as i8 (scale not applied).
pub fn unpack_block(dtype: Dtype, block: &[u8], out: &mut [i8; BLOCK]) {
    let packed = &block[4..];
    match dtype {
        Dtype::F32 => unreachable!("f32 has no blocks"),
        Dtype::Q8 => {
            for (o, &b) in out.iter_mut().zip(packed) {
                *o = b as i8;
            }
        }
        Dtype::Q4 => {
            for (j, &b) in packed.iter().enumerate() {
                out[2 * j] = (b & 0x0f) as i8 - 8;
                out[2 * j + 1] = (b >> 4) as i8 - 8;
            }
        }
        Dtype::T2 => {
            const T: [i8; 4] = [0, 1, -1, 0];
            for (j, &b) in packed.iter().enumerate() {
                for k in 0..4 {
                    out[4 * j + k] = T[((b >> (2 * k)) & 3) as usize];
                }
            }
        }
    }
}

/// Back to f32 (for tests, conversion and embeddings).
pub fn dequantize_row(dtype: Dtype, bytes: &[u8], cols: usize, out: &mut [f32]) {
    match dtype {
        Dtype::F32 => {
            for (o, c) in out.iter_mut().zip(bytes.chunks_exact(4)) {
                *o = f32::from_le_bytes(c.try_into().unwrap());
            }
        }
        _ => {
            let bb = block_bytes(dtype);
            let mut vals = [0i8; BLOCK];
            for (b, o) in bytes[..cols / BLOCK * bb]
                .chunks_exact(bb)
                .zip(out.chunks_exact_mut(BLOCK))
            {
                unpack_block(dtype, b, &mut vals);
                let s = scale_of(b);
                for (o, &v) in o.iter_mut().zip(&vals) {
                    *o = v as f32 * s;
                }
            }
        }
    }
}

/// Scalar reference: dot product of one quantized row with `q8`
/// activations.
pub fn dot_row_scalar(dtype: Dtype, row: &[u8], x: &QuantizedVec) -> f32 {
    let bb = block_bytes(dtype);
    let mut vals = [0i8; BLOCK];
    let mut sum = 0.0f32;
    for ((block, &xs), xv) in row
        .chunks_exact(bb)
        .zip(&x.scales)
        .zip(x.values.chunks_exact(BLOCK))
    {
        unpack_block(dtype, block, &mut vals);
        let isum: i32 = vals
            .iter()
            .zip(xv)
            .map(|(&a, &b)| a as i32 * b as i32)
            .sum();
        sum += isum as f32 * scale_of(block) * xs;
    }
    sum
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Deterministic pseudo-random values in [-1, 1).
    pub fn values(n: usize, seed: u64) -> Vec<f32> {
        let mut s = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
        (0..n)
            .map(|_| {
                s ^= s << 13;
                s ^= s >> 7;
                s ^= s << 17;
                (s >> 40) as f32 / (1u64 << 23) as f32 - 1.0
            })
            .collect()
    }

    #[test]
    fn round_trip_error_is_bounded() {
        let row = values(128, 1);
        for (dtype, bound) in [(Dtype::Q8, 0.005), (Dtype::Q4, 0.08)] {
            let mut bytes = Vec::new();
            quantize_row(dtype, &row, &mut bytes);
            assert_eq!(bytes.len(), row_bytes(dtype, row.len()));
            let mut back = vec![0.0; row.len()];
            dequantize_row(dtype, &bytes, row.len(), &mut back);
            let worst = row
                .iter()
                .zip(&back)
                .map(|(a, b)| (a - b).abs())
                .fold(0.0, f32::max);
            assert!(worst <= bound, "{dtype:?}: {worst}");
        }
    }

    #[test]
    fn ternary_keeps_signs_and_zeros() {
        let mut row = vec![0.0f32; 32];
        row[0] = 1.0;
        row[1] = -1.0;
        row[2] = 0.01;
        let mut bytes = Vec::new();
        quantize_row(Dtype::T2, &row, &mut bytes);
        let mut back = vec![0.0; 32];
        dequantize_row(Dtype::T2, &bytes, 32, &mut back);
        let s = 2.01 / 32.0;
        assert_eq!(&back[..3], &[s, -s, 0.0]);
        assert!(back[3..].iter().all(|&v| v == 0.0));
    }

    #[test]
    fn integer_dot_matches_dequantized_dot() {
        let row = values(96, 2);
        let x = values(96, 3);
        let mut qx = QuantizedVec::default();
        qx.quantize(&x);
        let mut xd = vec![0.0; 96];
        for (i, v) in xd.iter_mut().enumerate() {
            *v = qx.values[i] as f32 * qx.scales[i / BLOCK];
        }
        for dtype in [Dtype::Q8, Dtype::Q4, Dtype::T2] {
            let mut bytes = Vec::new();
            quantize_row(dtype, &row, &mut bytes);
            let mut w = vec![0.0; 96];
            dequantize_row(dtype, &bytes, 96, &mut w);
            let want: f32 = w.iter().zip(&xd).map(|(a, b)| a * b).sum();
            let got = dot_row_scalar(dtype, &bytes, &qx);
            assert!((got - want).abs() < 1e-4, "{dtype:?}: {got} vs {want}");
        }
    }
}
