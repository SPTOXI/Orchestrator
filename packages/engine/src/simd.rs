//! SIMD kernels (phase M2): AVX2 + FMA on x86-64 (chosen at run time),
//! NEON on ARM64 (Apple M, ARM notebooks), the scalar reference elsewhere.
//!
//! Quantized dot products work block by block: the 32 weights are
//! unpacked to i8, multiplied with the 32 i8 activations and summed
//! exactly as integers; the blocks' scaled sums are then added in vector
//! lanes, so the result equals [`crate::quant::dot_row_scalar`] up to the
//! rounding of that last float sum. The f32 dot product likewise.

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

/// Four `q8` rows against the same activations: each activation block is
/// loaded once for the four rows. Every row's result is exactly what
/// [`dot_q`] gives for it.
pub fn dot_q8_x4(rows: [&[u8]; 4], x: &QuantizedVec) -> [f32; 4] {
    #[cfg(target_arch = "x86_64")]
    {
        if has_avx2() {
            // SAFETY: AVX2 and FMA were detected on this CPU.
            return unsafe { x86::dot_q8_x4(rows, x) };
        }
    }
    #[cfg(target_arch = "aarch64")]
    {
        // SAFETY: NEON is part of every ARM64 CPU.
        return unsafe { arm::dot_q8_x4(rows, x) };
    }
    #[allow(unreachable_code)]
    rows.map(|r| quant::dot_row_scalar(Dtype::Q8, r, x))
}

/// `y += Σₖ a[k] · x[k]` for eight vectors at once: `y` is read and
/// written once instead of eight times.
pub fn axpy_x8(y: &mut [f32], a: [f32; 8], x: [&[f32]; 8]) {
    #[cfg(target_arch = "x86_64")]
    {
        if has_avx2() {
            // SAFETY: AVX2 and FMA were detected on this CPU.
            return unsafe { x86::axpy_x8(y, a, x) };
        }
    }
    #[cfg(target_arch = "aarch64")]
    {
        // SAFETY: NEON is part of every ARM64 CPU.
        return unsafe { arm::axpy_x8(y, a, x) };
    }
    #[allow(unreachable_code)]
    for (c, v) in y.iter_mut().enumerate() {
        for k in 0..8 {
            *v += a[k] * x[k][c];
        }
    }
}

/// `y += a · x`.
pub fn axpy(y: &mut [f32], a: f32, x: &[f32]) {
    debug_assert_eq!(y.len(), x.len());
    #[cfg(target_arch = "x86_64")]
    {
        if has_avx2() {
            // SAFETY: AVX2 and FMA were detected on this CPU.
            return unsafe { x86::axpy(y, a, x) };
        }
    }
    #[cfg(target_arch = "aarch64")]
    {
        // SAFETY: NEON is part of every ARM64 CPU.
        return unsafe { arm::axpy(y, a, x) };
    }
    #[allow(unreachable_code)]
    for (v, w) in y.iter_mut().zip(x) {
        *v += a * w;
    }
}

#[cfg(target_arch = "x86_64")]
mod x86 {
    use super::*;
    use std::arch::x86_64::*;

    #[target_feature(enable = "avx2,fma")]
    pub unsafe fn axpy_x8(y: &mut [f32], a: [f32; 8], x: [&[f32]; 8]) {
        let n = y.len();
        let py = y.as_mut_ptr();
        let av: [__m256; 8] = std::array::from_fn(|k| _mm256_set1_ps(a[k]));
        let mut i = 0;
        while i + 8 <= n {
            let mut acc = _mm256_loadu_ps(py.add(i));
            for k in 0..8 {
                acc = _mm256_fmadd_ps(av[k], _mm256_loadu_ps(x[k].as_ptr().add(i)), acc);
            }
            _mm256_storeu_ps(py.add(i), acc);
            i += 8;
        }
        while i < n {
            for k in 0..8 {
                *py.add(i) += a[k] * x[k][i];
            }
            i += 1;
        }
    }

    #[target_feature(enable = "avx2,fma")]
    pub unsafe fn axpy(y: &mut [f32], a: f32, x: &[f32]) {
        let n = y.len();
        let (py, px) = (y.as_mut_ptr(), x.as_ptr());
        let av = _mm256_set1_ps(a);
        let mut i = 0;
        while i + 8 <= n {
            let r = _mm256_fmadd_ps(av, _mm256_loadu_ps(px.add(i)), _mm256_loadu_ps(py.add(i)));
            _mm256_storeu_ps(py.add(i), r);
            i += 8;
        }
        while i < n {
            *py.add(i) += a * *px.add(i);
            i += 1;
        }
    }

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

    /// The 32 i8 × i8 products of a block, summed exactly into 8 i32 lanes.
    #[target_feature(enable = "avx2")]
    unsafe fn idot32(w: __m256i, x: __m256i) -> __m256i {
        let aw = _mm256_sign_epi8(w, w);
        let sx = _mm256_sign_epi8(x, w);
        let p16 = _mm256_maddubs_epi16(aw, sx);
        _mm256_madd_epi16(p16, _mm256_set1_epi16(1))
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
    pub unsafe fn dot_q8_x4(rows: [&[u8]; 4], x: &QuantizedVec) -> [f32; 4] {
        let bb = quant::block_bytes(Dtype::Q8);
        let mut acc = [_mm256_setzero_ps(); 4];
        for i in 0..x.scales.len() {
            let xv = _mm256_loadu_si256(x.values.as_ptr().add(i * BLOCK) as *const __m256i);
            for (k, row) in rows.iter().enumerate() {
                let block = row.as_ptr().add(i * bb);
                let w = _mm256_loadu_si256(block.add(4) as *const __m256i);
                let scale = f32::from_le_bytes(*(block as *const [u8; 4])) * x.scales[i];
                acc[k] = _mm256_fmadd_ps(
                    _mm256_cvtepi32_ps(idot32(w, xv)),
                    _mm256_set1_ps(scale),
                    acc[k],
                );
            }
        }
        acc.map(|a| hsum_ps(a))
    }

    #[target_feature(enable = "avx2,fma")]
    pub unsafe fn dot_q(dtype: Dtype, row: &[u8], x: &QuantizedVec) -> f32 {
        let bb = quant::block_bytes(dtype);
        let blocks = x.scales.len();
        debug_assert!(row.len() >= blocks * bb);
        // Each block's integer sums are exact; they are scaled and summed
        // in 8 float lanes, added together once at the end.
        let mut acc = _mm256_setzero_ps();
        for i in 0..blocks {
            let block = row.as_ptr().add(i * bb);
            let w = unpack(dtype, block.add(4));
            let xv = _mm256_loadu_si256(x.values.as_ptr().add(i * BLOCK) as *const __m256i);
            let scale = f32::from_le_bytes(*(block as *const [u8; 4])) * x.scales[i];
            acc = _mm256_fmadd_ps(
                _mm256_cvtepi32_ps(idot32(w, xv)),
                _mm256_set1_ps(scale),
                acc,
            );
        }
        hsum_ps(acc)
    }
}

#[cfg(target_arch = "aarch64")]
mod arm {
    use super::*;
    use std::arch::aarch64::*;

    #[target_feature(enable = "neon")]
    pub unsafe fn axpy_x8(y: &mut [f32], a: [f32; 8], x: [&[f32]; 8]) {
        let n = y.len();
        let py = y.as_mut_ptr();
        let mut i = 0;
        while i + 4 <= n {
            let mut acc = vld1q_f32(py.add(i));
            for k in 0..8 {
                acc = vfmaq_n_f32(acc, vld1q_f32(x[k].as_ptr().add(i)), a[k]);
            }
            vst1q_f32(py.add(i), acc);
            i += 4;
        }
        while i < n {
            for k in 0..8 {
                *py.add(i) += a[k] * x[k][i];
            }
            i += 1;
        }
    }

    #[target_feature(enable = "neon")]
    pub unsafe fn axpy(y: &mut [f32], a: f32, x: &[f32]) {
        let n = y.len();
        let (py, px) = (y.as_mut_ptr(), x.as_ptr());
        let av = vdupq_n_f32(a);
        let mut i = 0;
        while i + 4 <= n {
            vst1q_f32(
                py.add(i),
                vfmaq_f32(vld1q_f32(py.add(i)), av, vld1q_f32(px.add(i))),
            );
            i += 4;
        }
        while i < n {
            *py.add(i) += a * *px.add(i);
            i += 1;
        }
    }

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
    pub unsafe fn dot_q8_x4(rows: [&[u8]; 4], x: &QuantizedVec) -> [f32; 4] {
        let bb = quant::block_bytes(Dtype::Q8);
        let mut sum = [vdupq_n_f32(0.0); 4];
        for i in 0..x.scales.len() {
            let xp = x.values.as_ptr().add(i * BLOCK);
            let (x0, x1) = (vld1q_s8(xp), vld1q_s8(xp.add(16)));
            for (k, row) in rows.iter().enumerate() {
                let block = row.as_ptr().add(i * bb);
                let w0 = vld1q_s8(block.add(4) as *const i8);
                let w1 = vld1q_s8(block.add(20) as *const i8);
                let acc = idot16(idot16(vdupq_n_s32(0), w0, x0), w1, x1);
                let scale = f32::from_le_bytes(*(block as *const [u8; 4])) * x.scales[i];
                sum[k] = vfmaq_n_f32(sum[k], vcvtq_f32_s32(acc), scale);
            }
        }
        sum.map(|s| vaddvq_f32(s))
    }

    #[target_feature(enable = "neon")]
    pub unsafe fn dot_q(dtype: Dtype, row: &[u8], x: &QuantizedVec) -> f32 {
        let bb = quant::block_bytes(dtype);
        let blocks = x.scales.len();
        debug_assert!(row.len() >= blocks * bb);
        // Exact integer sums per block, scaled and summed in 4 float lanes.
        let mut sum = vdupq_n_f32(0.0);
        for i in 0..blocks {
            let block = row.as_ptr().add(i * bb);
            let (w0, w1) = unpack(dtype, block.add(4));
            let xp = x.values.as_ptr().add(i * BLOCK);
            let acc = idot16(vdupq_n_s32(0), w0, vld1q_s8(xp));
            let acc = idot16(acc, w1, vld1q_s8(xp.add(16)));
            let scale = f32::from_le_bytes(*(block as *const [u8; 4])) * x.scales[i];
            sum = vfmaq_n_f32(sum, vcvtq_f32_s32(acc), scale);
        }
        vaddvq_f32(sum)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::quant::tests::values;
    use crate::quant::{dot_row_scalar, quantize_row};

    #[test]
    fn quantized_kernels_equal_the_scalar_reference() {
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
                // Only the float sum of the blocks is ordered differently.
                let bound = 1e-6 * magnitude(dtype, &bytes, &qx);
                assert!(
                    (got - want).abs() <= bound,
                    "{dtype:?}, {cols} cols: {got} vs {want}"
                );
            }
        }
    }

    /// Sum of the blocks' absolute scaled sums: the size the rounding of
    /// their float sum is measured against.
    fn magnitude(dtype: Dtype, row: &[u8], x: &QuantizedVec) -> f32 {
        let bb = crate::quant::block_bytes(dtype);
        let mut vals = [0i8; BLOCK];
        row.chunks_exact(bb)
            .zip(&x.scales)
            .zip(x.values.chunks_exact(BLOCK))
            .map(|((b, &xs), xv)| {
                crate::quant::unpack_block(dtype, b, &mut vals);
                let i: i32 = vals
                    .iter()
                    .zip(xv)
                    .map(|(&a, &c)| a as i32 * c as i32)
                    .sum();
                (i as f32 * crate::quant::scale_of(b) * xs).abs()
            })
            .sum::<f32>()
            + 1e-30
    }

    #[test]
    fn extreme_values_do_not_overflow() {
        // All weights and activations at the limits: 32 × 127 × 127 fits.
        let mut qx = QuantizedVec::default();
        qx.quantize(&[1.0; 64]);
        let mut bytes = Vec::new();
        quantize_row(Dtype::Q8, &[-1.0; 64], &mut bytes);
        // Integers exact: -1 × 1 × 64 at scale (1/127)² × 127², exactly -64.
        assert_eq!(
            dot_q(Dtype::Q8, &bytes, &qx),
            dot_row_scalar(Dtype::Q8, &bytes, &qx)
        );
    }

    #[test]
    fn four_rows_at_once_equal_one_at_a_time() {
        let cols = 512;
        let mut qx = QuantizedVec::default();
        qx.quantize(&values(cols, 21));
        let rows: Vec<Vec<u8>> = (0..4)
            .map(|k| {
                let mut b = Vec::new();
                quantize_row(Dtype::Q8, &values(cols, 30 + k), &mut b);
                b
            })
            .collect();
        let got = dot_q8_x4([&rows[0], &rows[1], &rows[2], &rows[3]], &qx);
        for (g, r) in got.iter().zip(&rows) {
            assert_eq!(g.to_bits(), dot_q(Dtype::Q8, r, &qx).to_bits());
        }
        // A repacked q4 or t2 row gives what the row itself gives.
        for dtype in [Dtype::Q4, Dtype::T2] {
            let mut b = Vec::new();
            quantize_row(dtype, &values(cols, 40), &mut b);
            let mut q8 = Vec::new();
            crate::quant::repack_q8(dtype, &b, &mut q8);
            assert_eq!(
                dot_q(Dtype::Q8, &q8, &qx).to_bits(),
                dot_q(dtype, &b, &qx).to_bits()
            );
        }
    }

    #[test]
    fn axpy_x8_agrees_with_scalar() {
        for n in [1, 9, 64, 100] {
            let xs: Vec<Vec<f32>> = (0..8).map(|k| values(n, 50 + k)).collect();
            let a: [f32; 8] = std::array::from_fn(|k| 0.1 * k as f32 - 0.3);
            let mut y = values(n, 60);
            let mut want = y.clone();
            for (c, v) in want.iter_mut().enumerate() {
                for k in 0..8 {
                    *v += a[k] * xs[k][c];
                }
            }
            axpy_x8(&mut y, a, std::array::from_fn(|k| &xs[k][..]));
            for (g, w) in y.iter().zip(&want) {
                assert!((g - w).abs() < 1e-5, "{g} vs {w}");
            }
        }
    }

    #[test]
    fn axpy_agrees_with_scalar() {
        for n in [1, 7, 8, 9, 33, 100] {
            let x = values(n, 8);
            let mut y = values(n, 9);
            let mut want = y.clone();
            for (v, w) in want.iter_mut().zip(&x) {
                *v += 0.37 * w;
            }
            axpy(&mut y, 0.37, &x);
            for (a, b) in y.iter().zip(&want) {
                assert!((a - b).abs() < 1e-6);
            }
        }
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
