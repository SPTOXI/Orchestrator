//! SIMD kernels (phase M2): AVX2 + FMA on x86-64 (chosen at run time),
//! NEON on ARM64 (Apple M, ARM notebooks), the scalar reference elsewhere.
//!
//! Quantized dot products work block by block: the 32 weights are
//! unpacked to i8, multiplied with the 32 i8 activations, summed exactly
//! as integers and scaled — in the same order as
//! [`crate::quant::dot_row_scalar`], so the result is bit for bit the same.
//! The f32 dot product sums in a different order than the scalar one and
//! agrees up to rounding.

use crate::format::Dtype;
use crate::quant::{self, QuantizedVec, BLOCK};

/// Which kernels this computer uses.
pub fn kernel_name() -> &'static str {
    #[cfg(target_arch = "x86_64")]
    {
        if has_avx2() {
            return "avx2";
        }
    }
    #[cfg(target_arch = "aarch64")]
    {
        return "neon";
    }
    #[allow(unreachable_code)]
    "scalar"
}

#[cfg(target_arch = "x86_64")]
fn has_avx2() -> bool {
    is_x86_feature_detected!("avx2") && is_x86_feature_detected!("fma")
}

pub fn dot_f32(a: &[f32], b: &[f32]) -> f32 {
    debug_assert_eq!(a.len(), b.len());
    #[cfg(target_arch = "x86_64")]
    {
        if has_avx2() {
            // SAFETY: AVX2 and FMA were detected on this CPU.
            return unsafe { x86::dot_f32(a, b) };
        }
    }
    #[cfg(target_arch = "aarch64")]
    {
        // SAFETY: NEON is part of every ARM64 CPU.
        return unsafe { arm::dot_f32(a, b) };
    }
    #[allow(unreachable_code)]
    crate::ops::dot(a, b)
}

/// Dot product of one quantized row with `q8` activations.
pub fn dot_q(dtype: Dtype, row: &[u8], x: &QuantizedVec) -> f32 {
    #[cfg(target_arch = "x86_64")]
    {
        if has_avx2() {
            // SAFETY: AVX2 and FMA were detected on this CPU.
            return unsafe { x86::dot_q(dtype, row, x) };
        }
    }
    #[cfg(target_arch = "aarch64")]
    {
        // SAFETY: NEON is part of every ARM64 CPU.
        return unsafe { arm::dot_q(dtype, row, x) };
    }
    #[allow(unreachable_code)]
    quant::dot_row_scalar(dtype, row, x)
}

#[cfg(target_arch = "x86_64")]
mod x86 {
    use super::*;
    use std::arch::x86_64::*;

    #[target_feature(enable = "avx2,fma")]
    pub unsafe fn dot_f32(a: &[f32], b: &[f32]) -> f32 {
        let n = a.len();
        let (pa, pb) = (a.as_ptr(), b.as_ptr());
        let mut acc = [_mm256_setzero_ps(); 4];
        let mut i = 0;
        while i + 32 <= n {
            for (k, acc) in acc.iter_mut().enumerate() {
                let off = i + 8 * k;
                *acc = _mm256_fmadd_ps(
                    _mm256_loadu_ps(pa.add(off)),
                    _mm256_loadu_ps(pb.add(off)),
                    *acc,
                );
            }
            i += 32;
        }
        while i + 8 <= n {
            acc[0] = _mm256_fmadd_ps(
                _mm256_loadu_ps(pa.add(i)),
                _mm256_loadu_ps(pb.add(i)),
                acc[0],
            );
            i += 8;
        }
        let s = _mm256_add_ps(_mm256_add_ps(acc[0], acc[1]), _mm256_add_ps(acc[2], acc[3]));
        let mut sum = hsum_ps(s);
        while i < n {
            sum += a[i] * b[i];
            i += 1;
        }
        sum
    }

    #[target_feature(enable = "avx2")]
    unsafe fn hsum_ps(v: __m256) -> f32 {
        let lo = _mm256_castps256_ps128(v);
        let hi = _mm256_extractf128_ps(v, 1);
        let s = _mm_add_ps(lo, hi);
        let s = _mm_add_ps(s, _mm_movehl_ps(s, s));
        let s = _mm_add_ss(s, _mm_shuffle_ps(s, s, 1));
        _mm_cvtss_f32(s)
    }

    /// Exact sum of 32 i8 × i8 products.
    #[target_feature(enable = "avx2")]
    unsafe fn idot32(w: __m256i, x: __m256i) -> i32 {
        let aw = _mm256_sign_epi8(w, w);
        let sx = _mm256_sign_epi8(x, w);
        let p16 = _mm256_maddubs_epi16(aw, sx);
        let p32 = _mm256_madd_epi16(p16, _mm256_set1_epi16(1));
        let s = _mm_add_epi32(
            _mm256_castsi256_si128(p32),
            _mm256_extracti128_si256(p32, 1),
        );
        let s = _mm_add_epi32(s, _mm_shuffle_epi32(s, 0b01_00_11_10));
        let s = _mm_add_epi32(s, _mm_shuffle_epi32(s, 0b10_11_00_01));
        _mm_cvtsi128_si32(s)
    }

    #[target_feature(enable = "avx2")]
    unsafe fn unpack(dtype: Dtype, packed: *const u8) -> __m256i {
        match dtype {
            Dtype::Q8 => _mm256_loadu_si256(packed as *const __m256i),
            Dtype::Q4 => {
                let b = _mm_loadu_si128(packed as *const __m128i);
                let mask = _mm_set1_epi8(0x0f);
                let lo = _mm_and_si128(b, mask);
                let hi = _mm_and_si128(_mm_srli_epi16(b, 4), mask);
                let v = _mm256_set_m128i(_mm_unpackhi_epi8(lo, hi), _mm_unpacklo_epi8(lo, hi));
                _mm256_sub_epi8(v, _mm256_set1_epi8(8))
            }
            Dtype::T2 => {
                let b = _mm_loadl_epi64(packed as *const __m128i);
                let mask = _mm_set1_epi8(3);
                let table = _mm_setr_epi8(0, 1, -1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0);
                let code = |shift: i32| {
                    let c = _mm_and_si128(_mm_srl_epi16(b, _mm_cvtsi32_si128(shift)), mask);
                    _mm_shuffle_epi8(table, c)
                };
                let (c0, c1, c2, c3) = (code(0), code(2), code(4), code(6));
                let z01 = _mm_unpacklo_epi8(c0, c1);
                let z23 = _mm_unpacklo_epi8(c2, c3);
                _mm256_set_m128i(_mm_unpackhi_epi16(z01, z23), _mm_unpacklo_epi16(z01, z23))
            }
            Dtype::F32 => unreachable!("f32 has no blocks"),
        }
    }

    #[target_feature(enable = "avx2,fma")]
    pub unsafe fn dot_q(dtype: Dtype, row: &[u8], x: &QuantizedVec) -> f32 {
        let bb = quant::block_bytes(dtype);
        let blocks = x.scales.len();
        debug_assert!(row.len() >= blocks * bb);
        let mut sum = 0.0f32;
        for i in 0..blocks {
            let block = row.as_ptr().add(i * bb);
            let w = unpack(dtype, block.add(4));
            let xv = _mm256_loadu_si256(x.values.as_ptr().add(i * BLOCK) as *const __m256i);
            let scale = f32::from_le_bytes(*(block as *const [u8; 4]));
            sum += idot32(w, xv) as f32 * scale * x.scales[i];
        }
        sum
    }
}

#[cfg(target_arch = "aarch64")]
mod arm {
    use super::*;
    use std::arch::aarch64::*;

    #[target_feature(enable = "neon")]
    pub unsafe fn dot_f32(a: &[f32], b: &[f32]) -> f32 {
        let n = a.len();
        let (pa, pb) = (a.as_ptr(), b.as_ptr());
        let mut acc = [vdupq_n_f32(0.0); 4];
        let mut i = 0;
        while i + 16 <= n {
            for (k, acc) in acc.iter_mut().enumerate() {
                let off = i + 4 * k;
                *acc = vfmaq_f32(*acc, vld1q_f32(pa.add(off)), vld1q_f32(pb.add(off)));
            }
            i += 16;
        }
        while i + 4 <= n {
            acc[0] = vfmaq_f32(acc[0], vld1q_f32(pa.add(i)), vld1q_f32(pb.add(i)));
            i += 4;
        }
        let mut sum = vaddvq_f32(vaddq_f32(
            vaddq_f32(acc[0], acc[1]),
            vaddq_f32(acc[2], acc[3]),
        ));
        while i < n {
            sum += a[i] * b[i];
            i += 1;
        }
        sum
    }

    /// Exact sum of 16 i8 × i8 products, added to `acc`.
    #[target_feature(enable = "neon")]
    unsafe fn idot16(acc: int32x4_t, w: int8x16_t, x: int8x16_t) -> int32x4_t {
        let lo = vmull_s8(vget_low_s8(w), vget_low_s8(x));
        let hi = vmull_s8(vget_high_s8(w), vget_high_s8(x));
        vpadalq_s16(vpadalq_s16(acc, lo), hi)
    }

    #[target_feature(enable = "neon")]
    unsafe fn unpack(dtype: Dtype, packed: *const u8) -> (int8x16_t, int8x16_t) {
        match dtype {
            Dtype::Q8 => (
                vld1q_s8(packed as *const i8),
                vld1q_s8(packed.add(16) as *const i8),
            ),
            Dtype::Q4 => {
                let b = vld1q_u8(packed);
                let lo = vandq_u8(b, vdupq_n_u8(0x0f));
                let hi = vshrq_n_u8::<4>(b);
                let eight = vdupq_n_s8(8);
                (
                    vsubq_s8(vreinterpretq_s8_u8(vzip1q_u8(lo, hi)), eight),
                    vsubq_s8(vreinterpretq_s8_u8(vzip2q_u8(lo, hi)), eight),
                )
            }
            Dtype::T2 => {
                let b = vld1_u8(packed);
                let mask = vdup_n_u8(3);
                let table = vcreate_s8(u64::from_le_bytes([0, 1, 0xff, 0, 0, 0, 0, 0]));
                let c0 = vtbl1_s8(table, vreinterpret_s8_u8(vand_u8(b, mask)));
                let c1 = vtbl1_s8(table, vreinterpret_s8_u8(vand_u8(vshr_n_u8::<2>(b), mask)));
                let c2 = vtbl1_s8(table, vreinterpret_s8_u8(vand_u8(vshr_n_u8::<4>(b), mask)));
                let c3 = vtbl1_s8(table, vreinterpret_s8_u8(vshr_n_u8::<6>(b)));
                let z01 = vzip_s8(c0, c1);
                let z23 = vzip_s8(c2, c3);
                let a = vzip_s16(vreinterpret_s16_s8(z01.0), vreinterpret_s16_s8(z23.0));
                let b2 = vzip_s16(vreinterpret_s16_s8(z01.1), vreinterpret_s16_s8(z23.1));
                (
                    vcombine_s8(vreinterpret_s8_s16(a.0), vreinterpret_s8_s16(a.1)),
                    vcombine_s8(vreinterpret_s8_s16(b2.0), vreinterpret_s8_s16(b2.1)),
                )
            }
            Dtype::F32 => unreachable!("f32 has no blocks"),
        }
    }

    #[target_feature(enable = "neon")]
    pub unsafe fn dot_q(dtype: Dtype, row: &[u8], x: &QuantizedVec) -> f32 {
        let bb = quant::block_bytes(dtype);
        let blocks = x.scales.len();
        debug_assert!(row.len() >= blocks * bb);
        let mut sum = 0.0f32;
        for i in 0..blocks {
            let block = row.as_ptr().add(i * bb);
            let (w0, w1) = unpack(dtype, block.add(4));
            let xp = x.values.as_ptr().add(i * BLOCK);
            let acc = idot16(vdupq_n_s32(0), w0, vld1q_s8(xp));
            let acc = idot16(acc, w1, vld1q_s8(xp.add(16)));
            let scale = f32::from_le_bytes(*(block as *const [u8; 4]));
            sum += vaddvq_s32(acc) as f32 * scale * x.scales[i];
        }
        sum
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::quant::tests::values;
    use crate::quant::{dot_row_scalar, quantize_row};

    #[test]
    fn quantized_kernels_equal_the_scalar_reference_bit_for_bit() {
        eprintln!("kernels: {}", kernel_name());
        for cols in [32, 64, 96, 256, 1024] {
            let x = values(cols, cols as u64);
            let mut qx = QuantizedVec::default();
            qx.quantize(&x);
            for (seed, dtype) in [(11, Dtype::Q8), (12, Dtype::Q4), (13, Dtype::T2)] {
                let row = values(cols, seed + cols as u64);
                let mut bytes = Vec::new();
                quantize_row(dtype, &row, &mut bytes);
                let want = dot_row_scalar(dtype, &bytes, &qx);
                let got = dot_q(dtype, &bytes, &qx);
                assert_eq!(got.to_bits(), want.to_bits(), "{dtype:?}, {cols} cols");
            }
        }
    }

    #[test]
    fn extreme_values_do_not_overflow() {
        // All weights and activations at the limits: 32 × 127 × 127 fits.
        let mut qx = QuantizedVec::default();
        qx.quantize(&[1.0; 64]);
        let mut bytes = Vec::new();
        quantize_row(Dtype::Q8, &[-1.0; 64], &mut bytes);
        assert_eq!(
            dot_q(Dtype::Q8, &bytes, &qx).to_bits(),
            dot_row_scalar(Dtype::Q8, &bytes, &qx).to_bits()
        );
    }

    #[test]
    fn f32_kernel_agrees_with_scalar() {
        for n in [1, 7, 8, 31, 32, 33, 100, 1000] {
            let a = values(n, 5);
            let b = values(n, 6);
            let want = crate::ops::dot(&a, &b);
            let got = dot_f32(&a, &b);
            assert!(
                (got - want).abs() <= 1e-4 * (1.0 + want.abs()),
                "{n}: {got} vs {want}"
            );
        }
    }
}
