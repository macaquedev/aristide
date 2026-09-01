//! Polyphase windowed-sinc interpolation — the RT sample reader.
//!
//! Linear interpolation (Sample::read) aliases audibly once material is
//! pitch-shifted (temperaments, random detune, 44.1→48 kHz). This table
//! trades a handful of multiply-adds per output sample for ~50 dB better
//! error floors. Every kernel is built control-side at engine
//! construction; the audio thread only indexes into them, upholding the
//! crate invariants.
//!
//! Design: for each kernel, `PHASES` Kaiser-windowed sinc rows of that
//! kernel's own tap count, one per fractional offset step, linearly
//! interpolated between neighbouring rows at read time. Each row is
//! normalized to unity DC gain, so amplitude ripple between phases
//! cancels exactly for constant signals.
//!
//! One kernel is not enough. Reading a sample faster than 1:1 (`rate`
//! = source frames per output frame, > 1) is decimation: the output
//! Nyquist, expressed in source-frame terms, falls as 1/rate, so a
//! kernel voiced for rate ≈ 1 lets everything above it fold back into
//! the band as aliasing. The fix is a family of kernels, each cut off
//! and widened for the rate range it serves: [`SincTables`] builds one
//! base kernel for rate ≤ 1 (upward interpolation needs no extra band
//! limiting — pitch only drops, so nothing new crosses the output
//! Nyquist) plus a ladder of quarter-octave buckets for rate > 1.

use crate::bank::Sample;

/// FIR length of the base (rate ≤ 1) kernel. 16 taps × Kaiser β=9 gives
/// ≈ −90 dB stopband — below any organ sample's noise floor — at a cost
/// small enough for thousands of voices. Bucket kernels (rate > 1) scale
/// this up; see [`SincTables::new`].
const BASE_TAPS: usize = 16;
/// Fractional-position resolution. With inter-row interpolation the
/// phase-quantization error sits far below any kernel's own floor.
pub(crate) const PHASES: usize = 512;

/// Stack window for dequantizing 16-bit residents on the fast path:
/// the widest kernel (4× bucket, 64 taps) at stereo. Only the scalar
/// fallback needs it; x86_64 dequantizes in the SIMD path.
#[cfg(not(target_arch = "x86_64"))]
const MAX_WINDOW: usize = 64 * 2;

/// Base cutoff as a fraction of Nyquist: headroom for mild upward
/// transposition (temperament stretch, detune) before aliasing. Bucket
/// kernels shrink this further, in proportion to their rate.
const CUTOFF: f64 = 0.9;
const KAISER_BETA: f64 = 9.0;

/// Quarter-octave rate buckets covering rate > 1 up to two octaves
/// (4×). A voice above 4× still gets the 4× kernel — the tightest band
/// limiting we build — so aliasing rejection stops improving past that
/// point rather than being absent; two octaves covers everything short
/// of filling in missing ranks with wildly transposed pipes.
const BUCKET_COUNT: usize = 8;

/// One Kaiser-windowed polyphase sinc kernel: `taps` coefficients per
/// phase, `PHASES + 1` phases (the extra row lets reads interpolate
/// `p → p+1` without wrapping).
struct Kernel {
    taps: usize,
    /// Reads start this many frames before the integer position.
    left_taps: usize,
    /// `(PHASES + 1) × taps`, row-major; row `p` is the kernel for
    /// fractional offset `p / PHASES`.
    coefficients: Box<[f32]>,
}

impl Kernel {
    /// Build a kernel of `taps` coefficients per phase, low-pass cutoff
    /// `cutoff` (fraction of the *source* Nyquist). The Kaiser window
    /// spans the whole kernel width regardless of tap count, so a wider
    /// (more taps) kernel gets a proportionally narrower transition band
    /// — exactly what a lower cutoff needs to stay well-behaved.
    fn new(taps: usize, cutoff: f64) -> Kernel {
        let mut coefficients = vec![0.0f32; (PHASES + 1) * taps].into_boxed_slice();
        let half_width = (taps / 2) as f64;
        let left_taps = taps / 2 - 1;
        for phase in 0..=PHASES {
            let fraction = phase as f64 / PHASES as f64;
            let row = &mut coefficients[phase * taps..(phase + 1) * taps];
            let mut sum = 0.0f64;
            for (tap, coefficient) in row.iter_mut().enumerate() {
                let x = tap as f64 - left_taps as f64 - fraction;
                let window_arg = x / half_width;
                let value = if window_arg.abs() >= 1.0 {
                    0.0
                } else {
                    cutoff * sinc(cutoff * x) * kaiser(window_arg)
                };
                *coefficient = value as f32;
                sum += value;
            }
            // Unity DC gain per phase.
            if sum != 0.0 {
                for coefficient in row.iter_mut() {
                    *coefficient = (*coefficient as f64 / sum) as f32;
                }
            }
        }
        Kernel {
            taps,
            left_taps,
            coefficients,
        }
    }
}

/// The family of sinc kernels a voice reads through, indexed by rate
/// bucket. `kernels[0]` is the base (rate ≤ 1) kernel; `kernels[1..]`
/// are the quarter-octave rate > 1 buckets, ascending.
pub struct SincTables {
    kernels: Vec<Kernel>,
    /// Upper rate bound of each bucket in `kernels[1..]`, ascending:
    /// `bucket_ratios[i]` is the rate `kernels[i + 1]` was voiced for.
    bucket_ratios: [f64; BUCKET_COUNT],
    /// Detected once at build (control side): AVX2+FMA kernels do the
    /// dot product in 8-wide FMA blocks instead of four SSE blocks.
    #[cfg(target_arch = "x86_64")]
    use_avx2: bool,
}

impl SincTables {
    pub fn new() -> SincTables {
        let mut kernels = Vec::with_capacity(1 + BUCKET_COUNT);
        kernels.push(Kernel::new(BASE_TAPS, CUTOFF));

        let mut bucket_ratios = [0.0f64; BUCKET_COUNT];
        for (i, ratio_slot) in bucket_ratios.iter_mut().enumerate() {
            // Quarter-octave steps: 2^(1/4) .. 2^(8/4) = 4.0.
            let ratio = 2f64.powf((i + 1) as f64 / 4.0);
            *ratio_slot = ratio;
            // Decimating by `ratio` moves the output Nyquist down to
            // 1/ratio of the source Nyquist; cut off there (with the
            // same 0.9 headroom) so the transition band scales with it.
            let cutoff = CUTOFF / ratio;
            // Widen the kernel by the same ratio to keep the transition
            // band (in source frames) constant as the cutoff falls,
            // rounded up to a multiple of 8 so the SIMD paths — which
            // walk in 4- or 8-wide chunks — stay aligned.
            let raw_taps = BASE_TAPS as f64 * ratio;
            let taps = ((raw_taps / 8.0).ceil() as usize) * 8;
            kernels.push(Kernel::new(taps, cutoff));
        }

        SincTables {
            kernels,
            bucket_ratios,
            #[cfg(target_arch = "x86_64")]
            use_avx2: std::arch::is_x86_feature_detected!("avx2")
                && std::arch::is_x86_feature_detected!("fma"),
        }
    }

    /// Pick the kernel index for a voice's playback `rate` (source
    /// frames per output frame). Called once at voice start, control
    /// side — a voice keeps the same kernel for its whole life. Rate
    /// does wobble afterwards (tremulant modulation, release pitch
    /// bend), but never by enough to cross a quarter-octave bucket
    /// boundary, so re-selecting per block would only add cost for no
    /// audible gain.
    pub fn select(&self, rate: f64) -> usize {
        if rate <= 1.0 {
            return 0;
        }
        for (i, &ratio) in self.bucket_ratios.iter().enumerate() {
            if ratio >= rate {
                return i + 1;
            }
        }
        // Beyond the top bucket (> 4×, two octaves up) band limiting
        // stops tracking rate: every voice past this point reads
        // through the same tightest kernel we build.
        self.kernels.len() - 1
    }

    /// Interpolated stereo read at a fractional frame position, through
    /// the kernel at `kernel_index` (from [`SincTables::select`]).
    ///
    /// `loop_wrap`: while a voice circles a sustain loop, kernel taps
    /// that fall at/after the loop end must wrap back by the loop
    /// length, or every pass across the seam would click. Tail reads
    /// pass `None` and clamp at the sample edges instead.
    #[inline]
    pub fn read(
        &self,
        kernel_index: usize,
        sample: &Sample,
        position: f64,
        loop_wrap: Option<(u64, u64)>,
    ) -> (f32, f32) {
        let kernel = &self.kernels[kernel_index];
        let taps = kernel.taps;
        let left_taps = kernel.left_taps;
        let (data, channels) = sample.raw();
        let channels = channels as usize;
        let frames = (data.len() / channels) as i64;

        let base = position.floor();
        let fraction = position - base;
        let scaled = fraction * PHASES as f64;
        let row_index = scaled as usize; // 0..=PHASES-1 since fraction < 1
        let row_mix = (scaled - row_index as f64) as f32;
        let row0 = &kernel.coefficients[row_index * taps..row_index * taps + taps];
        let row1 = &kernel.coefficients[(row_index + 1) * taps..(row_index + 1) * taps + taps];

        let first_tap_frame = base as i64 - left_taps as i64;

        // Fast path: the whole window is in-bounds and doesn't straddle
        // the loop seam — one contiguous dot product. 16-bit residents
        // dequantize the window into a stack buffer first (one branch
        // per output frame, and half the memory traffic pays for the
        // conversion), then run the identical SIMD/scalar dot.
        let seam_safe = match loop_wrap {
            Some((_, end)) => first_tap_frame + taps as i64 <= end as i64,
            None => true,
        };
        if seam_safe && first_tap_frame >= 0 && first_tap_frame + (taps as i64) <= frames {
            let start = first_tap_frame as usize * channels;
            let count = taps * channels;
            return match data {
                crate::bank::SampleData::F32(all) => self.dot_window(
                    row0,
                    row1,
                    row_mix,
                    channels,
                    &all[start..start + count],
                ),
                crate::bank::SampleData::I16(all) => {
                    let window = &all[start..start + count];
                    #[cfg(target_arch = "x86_64")]
                    unsafe {
                        if self.use_avx2 && channels == 1 {
                            let value = dot_blended_mono_avx2_i16(row0, row1, row_mix, window);
                            return (value, value);
                        }
                        if channels == 1 {
                            let value = dot_blended_mono_sse2_i16(row0, row1, row_mix, window);
                            (value, value)
                        } else {
                            dot_blended_stereo_sse2_i16(row0, row1, row_mix, window)
                        }
                    }
                    #[cfg(not(target_arch = "x86_64"))]
                    {
                        debug_assert!(count <= MAX_WINDOW);
                        let mut converted = [0.0f32; MAX_WINDOW];
                        for (out, value) in converted[..count].iter_mut().zip(window) {
                            *out = f32::from(*value) * crate::bank::I16_SCALE;
                        }
                        self.dot_window(row0, row1, row_mix, channels, &converted[..count])
                    }
                }
            };
        }

        // Slow path (window touches an edge or the loop seam): map each
        // tap's frame index individually.
        let mut left = 0.0f32;
        let mut right = 0.0f32;
        for tap in 0..taps {
            let mut frame = first_tap_frame + tap as i64;
            if let Some((start, end)) = loop_wrap {
                let (start, end) = (start as i64, end as i64);
                let length = end - start;
                while frame >= end {
                    frame -= length;
                }
            }
            let frame = frame.clamp(0, frames - 1) as usize;
            let coefficient = row0[tap] + (row1[tap] - row0[tap]) * row_mix;
            left += coefficient * data.get(frame * channels);
            if channels > 1 {
                right += coefficient * data.get(frame * channels + 1);
            }
        }
        if channels == 1 {
            (left, left)
        } else {
            (left, right)
        }
    }

    /// The blended-row dot product over one contiguous window — the
    /// dominant per-sample cost of the whole engine: SIMD on x86_64
    /// (SSE2 is baseline), scalar elsewhere.
    #[inline]
    fn dot_window(
        &self,
        row0: &[f32],
        row1: &[f32],
        row_mix: f32,
        channels: usize,
        window: &[f32],
    ) -> (f32, f32) {
        #[cfg(target_arch = "x86_64")]
        unsafe {
            // Mono only: measured on the stress suite, the 256-bit
            // stereo path was ~10% SLOWER than SSE2 here (shuffle
            // overhead + downclocking); mono's clean 8-wide FMAs do
            // win.
            if self.use_avx2 && channels == 1 {
                let value = dot_blended_mono_avx2(row0, row1, row_mix, window);
                return (value, value);
            }
            if channels == 1 {
                let value = dot_blended_mono_sse2(row0, row1, row_mix, window);
                (value, value)
            } else {
                dot_blended_stereo_sse2(row0, row1, row_mix, window)
            }
        }
        #[cfg(not(target_arch = "x86_64"))]
        {
            let taps = row0.len();
            let mut left = 0.0f32;
            let mut right = 0.0f32;
            if channels == 1 {
                for tap in 0..taps {
                    let coefficient = row0[tap] + (row1[tap] - row0[tap]) * row_mix;
                    left += coefficient * window[tap];
                }
                return (left, left);
            }
            for tap in 0..taps {
                let coefficient = row0[tap] + (row1[tap] - row0[tap]) * row_mix;
                left += coefficient * window[tap * channels];
                right += coefficient * window[tap * channels + 1];
            }
            (left, right)
        }
    }
}

impl Default for SincTables {
    fn default() -> Self {
        SincTables::new()
    }
}

/// Blend the two coefficient rows and dot against a mono window, 4 taps
/// per SSE2 vector. Safety: caller guarantees `row0`, `row1`, `window`
/// each hold at least `row0.len()` values and `row0.len()` is a
/// multiple of 4.
#[cfg(target_arch = "x86_64")]
#[inline]
unsafe fn dot_blended_mono_sse2(row0: &[f32], row1: &[f32], mix: f32, window: &[f32]) -> f32 {
    use core::arch::x86_64::*;
    unsafe {
        let mix4 = _mm_set1_ps(mix);
        let mut acc = _mm_setzero_ps();
        for chunk in 0..row0.len() / 4 {
            let offset = chunk * 4;
            let c0 = _mm_loadu_ps(row0.as_ptr().add(offset));
            let c1 = _mm_loadu_ps(row1.as_ptr().add(offset));
            let c = _mm_add_ps(c0, _mm_mul_ps(_mm_sub_ps(c1, c0), mix4));
            let w = _mm_loadu_ps(window.as_ptr().add(offset));
            acc = _mm_add_ps(acc, _mm_mul_ps(c, w));
        }
        horizontal_sum(acc)
    }
}

/// Stereo variant: the window is interleaved LRLR; shuffle pairs of
/// loads into an L vector and an R vector per 4 taps.
/// Safety: as [`dot_blended_mono_sse2`].
#[cfg(target_arch = "x86_64")]
#[inline]
unsafe fn dot_blended_stereo_sse2(
    row0: &[f32],
    row1: &[f32],
    mix: f32,
    window: &[f32],
) -> (f32, f32) {
    use core::arch::x86_64::*;
    unsafe {
        let mix4 = _mm_set1_ps(mix);
        let mut acc_left = _mm_setzero_ps();
        let mut acc_right = _mm_setzero_ps();
        for chunk in 0..row0.len() / 4 {
            let offset = chunk * 4;
            let c0 = _mm_loadu_ps(row0.as_ptr().add(offset));
            let c1 = _mm_loadu_ps(row1.as_ptr().add(offset));
            let c = _mm_add_ps(c0, _mm_mul_ps(_mm_sub_ps(c1, c0), mix4));
            // L0 R0 L1 R1 | L2 R2 L3 R3 → L0 L1 L2 L3 / R0 R1 R2 R3
            let w01 = _mm_loadu_ps(window.as_ptr().add(offset * 2));
            let w23 = _mm_loadu_ps(window.as_ptr().add(offset * 2 + 4));
            let left = _mm_shuffle_ps(w01, w23, 0b10_00_10_00);
            let right = _mm_shuffle_ps(w01, w23, 0b11_01_11_01);
            acc_left = _mm_add_ps(acc_left, _mm_mul_ps(c, left));
            acc_right = _mm_add_ps(acc_right, _mm_mul_ps(c, right));
        }
        (horizontal_sum(acc_left), horizontal_sum(acc_right))
    }
}

/// Mono i16 SSE2: 8 taps per iteration — one 128-bit load holds eight
/// samples, sign-extended in-register (self-unpack + arithmetic shift)
/// and converted to f32; the dequant scale folds into the final sum,
/// so 16-bit residency costs one extra convert per 4 taps instead of a
/// scalar pre-pass.
/// Safety: caller guarantees `window.len() >= row0.len()` and that
/// `row0.len()` is a multiple of 8 (every kernel is).
#[cfg(target_arch = "x86_64")]
#[inline]
unsafe fn dot_blended_mono_sse2_i16(row0: &[f32], row1: &[f32], mix: f32, window: &[i16]) -> f32 {
    use core::arch::x86_64::*;
    unsafe {
        let mix4 = _mm_set1_ps(mix);
        let mut acc = _mm_setzero_ps();
        for chunk in 0..row0.len() / 8 {
            let offset = chunk * 8;
            let q = _mm_loadu_si128(window.as_ptr().add(offset) as *const __m128i);
            let lo = _mm_cvtepi32_ps(_mm_srai_epi32(_mm_unpacklo_epi16(q, q), 16));
            let hi = _mm_cvtepi32_ps(_mm_srai_epi32(_mm_unpackhi_epi16(q, q), 16));
            let c0a = _mm_loadu_ps(row0.as_ptr().add(offset));
            let c1a = _mm_loadu_ps(row1.as_ptr().add(offset));
            let ca = _mm_add_ps(c0a, _mm_mul_ps(_mm_sub_ps(c1a, c0a), mix4));
            let c0b = _mm_loadu_ps(row0.as_ptr().add(offset + 4));
            let c1b = _mm_loadu_ps(row1.as_ptr().add(offset + 4));
            let cb = _mm_add_ps(c0b, _mm_mul_ps(_mm_sub_ps(c1b, c0b), mix4));
            acc = _mm_add_ps(acc, _mm_mul_ps(ca, lo));
            acc = _mm_add_ps(acc, _mm_mul_ps(cb, hi));
        }
        horizontal_sum(acc) * crate::bank::I16_SCALE
    }
}

/// Stereo i16 SSE2: 4 taps (8 interleaved samples) per iteration, the
/// same deinterleaving shuffles as the f32 kernel after an in-register
/// convert.
/// Safety: caller guarantees `window.len() >= row0.len() * 2` and that
/// `row0.len()` is a multiple of 4.
#[cfg(target_arch = "x86_64")]
#[inline]
unsafe fn dot_blended_stereo_sse2_i16(
    row0: &[f32],
    row1: &[f32],
    mix: f32,
    window: &[i16],
) -> (f32, f32) {
    use core::arch::x86_64::*;
    unsafe {
        let mix4 = _mm_set1_ps(mix);
        let mut acc_left = _mm_setzero_ps();
        let mut acc_right = _mm_setzero_ps();
        for chunk in 0..row0.len() / 4 {
            let offset = chunk * 4;
            let c0 = _mm_loadu_ps(row0.as_ptr().add(offset));
            let c1 = _mm_loadu_ps(row1.as_ptr().add(offset));
            let c = _mm_add_ps(c0, _mm_mul_ps(_mm_sub_ps(c1, c0), mix4));
            // L0 R0 L1 R1 L2 R2 L3 R3 as i16 → two f32 quads → L/R.
            let q = _mm_loadu_si128(window.as_ptr().add(offset * 2) as *const __m128i);
            let w01 = _mm_cvtepi32_ps(_mm_srai_epi32(_mm_unpacklo_epi16(q, q), 16));
            let w23 = _mm_cvtepi32_ps(_mm_srai_epi32(_mm_unpackhi_epi16(q, q), 16));
            let left = _mm_shuffle_ps(w01, w23, 0b10_00_10_00);
            let right = _mm_shuffle_ps(w01, w23, 0b11_01_11_01);
            acc_left = _mm_add_ps(acc_left, _mm_mul_ps(c, left));
            acc_right = _mm_add_ps(acc_right, _mm_mul_ps(c, right));
        }
        (
            horizontal_sum(acc_left) * crate::bank::I16_SCALE,
            horizontal_sum(acc_right) * crate::bank::I16_SCALE,
        )
    }
}

/// Mono i16 AVX2+FMA: 8 taps per fused block via `vpmovsxwd` + convert.
/// Safety: as [`dot_blended_mono_avx2`], window in i16.
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2,fma")]
unsafe fn dot_blended_mono_avx2_i16(row0: &[f32], row1: &[f32], mix: f32, window: &[i16]) -> f32 {
    use core::arch::x86_64::*;
    unsafe {
        let mix8 = _mm256_set1_ps(mix);
        let mut acc = _mm256_setzero_ps();
        for chunk in 0..row0.len() / 8 {
            let offset = chunk * 8;
            let c0 = _mm256_loadu_ps(row0.as_ptr().add(offset));
            let c1 = _mm256_loadu_ps(row1.as_ptr().add(offset));
            let c = _mm256_fmadd_ps(_mm256_sub_ps(c1, c0), mix8, c0);
            let q = _mm_loadu_si128(window.as_ptr().add(offset) as *const __m128i);
            let w = _mm256_cvtepi32_ps(_mm256_cvtepi16_epi32(q));
            acc = _mm256_fmadd_ps(c, w, acc);
        }
        let low = _mm256_castps256_ps128(acc);
        let high = _mm256_extractf128_ps(acc, 1);
        horizontal_sum(_mm_add_ps(low, high)) * crate::bank::I16_SCALE
    }
}

#[cfg(target_arch = "x86_64")]
#[inline]
unsafe fn horizontal_sum(v: core::arch::x86_64::__m128) -> f32 {
    use core::arch::x86_64::*;
    unsafe {
        let hi = _mm_movehl_ps(v, v);
        let sum2 = _mm_add_ps(v, hi);
        let hi1 = _mm_shuffle_ps(sum2, sum2, 0b01);
        _mm_cvtss_f32(_mm_add_ss(sum2, hi1))
    }
}

/// AVX2+FMA: the dot product in 8-wide fused blocks.
/// Safety: caller guarantees slice lengths >= `row0.len()`, that
/// `row0.len()` is a multiple of 8, and that the CPU supports avx2+fma
/// (checked at table construction).
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2,fma")]
unsafe fn dot_blended_mono_avx2(row0: &[f32], row1: &[f32], mix: f32, window: &[f32]) -> f32 {
    use core::arch::x86_64::*;
    unsafe {
        let mix8 = _mm256_set1_ps(mix);
        let mut acc = _mm256_setzero_ps();
        for chunk in 0..row0.len() / 8 {
            let offset = chunk * 8;
            let c0 = _mm256_loadu_ps(row0.as_ptr().add(offset));
            let c1 = _mm256_loadu_ps(row1.as_ptr().add(offset));
            let c = _mm256_fmadd_ps(_mm256_sub_ps(c1, c0), mix8, c0);
            let w = _mm256_loadu_ps(window.as_ptr().add(offset));
            acc = _mm256_fmadd_ps(c, w, acc);
        }
        let low = _mm256_castps256_ps128(acc);
        let high = _mm256_extractf128_ps(acc, 1);
        horizontal_sum(_mm_add_ps(low, high))
    }
}

/// Stereo AVX2: blended coefficients duplicated pair-wise so the
/// interleaved LRLR window multiplies directly — even lanes accumulate
/// left, odd lanes right.
/// Safety: caller guarantees slice lengths >= `row0.len()`, that
/// `row0.len()` is a multiple of 4, and that the CPU supports avx2+fma.
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2,fma")]
#[allow(dead_code)]
unsafe fn dot_blended_stereo_avx2(
    row0: &[f32],
    row1: &[f32],
    mix: f32,
    window: &[f32],
) -> (f32, f32) {
    use core::arch::x86_64::*;
    unsafe {
        let mix4 = _mm_set1_ps(mix);
        let mut acc = _mm256_setzero_ps();
        for chunk in 0..row0.len() / 4 {
            let offset = chunk * 4;
            let c0 = _mm_loadu_ps(row0.as_ptr().add(offset));
            let c1 = _mm_loadu_ps(row1.as_ptr().add(offset));
            let c = _mm_add_ps(c0, _mm_mul_ps(_mm_sub_ps(c1, c0), mix4));
            // [c0 c1 c2 c3] -> [c0 c0 c1 c1 | c2 c2 c3 c3]
            let dup = _mm256_set_m128(_mm_unpackhi_ps(c, c), _mm_unpacklo_ps(c, c));
            let w = _mm256_loadu_ps(window.as_ptr().add(offset * 2));
            acc = _mm256_fmadd_ps(dup, w, acc);
        }
        // Even lanes hold L terms, odd lanes R. Fold 256 -> 128 -> pair.
        let low = _mm256_castps256_ps128(acc);
        let high = _mm256_extractf128_ps(acc, 1);
        let s4 = _mm_add_ps(low, high); // [L R L R]
        let folded = _mm_add_ps(s4, _mm_movehl_ps(s4, s4)); // [L R . .]
        (
            _mm_cvtss_f32(folded),
            _mm_cvtss_f32(_mm_shuffle_ps(folded, folded, 0b01)),
        )
    }
}

fn sinc(x: f64) -> f64 {
    if x.abs() < 1e-12 {
        1.0
    } else {
        let pi_x = std::f64::consts::PI * x;
        pi_x.sin() / pi_x
    }
}

/// Kaiser window on `u ∈ [-1, 1]`: `I0(β√(1−u²)) / I0(β)`.
fn kaiser(u: f64) -> f64 {
    bessel_i0(KAISER_BETA * (1.0 - u * u).max(0.0).sqrt()) / bessel_i0(KAISER_BETA)
}

/// Modified Bessel function of the first kind, order zero (power series;
/// converges fast for the argument range Kaiser uses).
fn bessel_i0(x: f64) -> f64 {
    let mut sum = 1.0;
    let mut term = 1.0;
    let half_x = x / 2.0;
    for k in 1..32 {
        term *= (half_x / k as f64) * (half_x / k as f64);
        sum += term;
        if term < sum * 1e-16 {
            break;
        }
    }
    sum
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bank::Sample;

    fn mono_sample(data: Vec<f32>) -> Sample {
        Sample::new(data, 1, 48000.0, None, 0).expect("valid")
    }

    fn snr_db(signal_power: f64, error_power: f64) -> f64 {
        10.0 * (signal_power / error_power.max(1e-30)).log10()
    }

    /// 16-bit residents read through the same sinc path within
    /// quantization noise of the f32 original — fast path, seam-wrap
    /// slow path, and edges alike.
    #[test]
    fn i16_residents_match_f32_within_quantization_noise() {
        let tables = SincTables::new();
        let omega = std::f64::consts::TAU * 0.13;
        let frames = 4096usize;
        let data: Vec<f32> = (0..frames)
            .map(|n| (0.8 * (omega * n as f64).sin()) as f32)
            .collect();
        let f32_sample = Sample::new(data.clone(), 1, 48000.0, Some((256, 3840)), 0)
            .expect("valid");
        let mut i16_sample = Sample::new(data, 1, 48000.0, Some((256, 3840)), 0)
            .expect("valid");
        i16_sample.quantize_i16();
        for kernel in [0usize, tables.kernels.len() - 1] {
            for step in 0..400 {
                // Sweep across the loop seam and the sample edges so
                // both read paths run.
                let position = step as f64 * 9.7 + 3.3;
                for wrap in [None, Some((256u64, 3840u64))] {
                    let (a, _) = tables.read(kernel, &f32_sample, position, wrap);
                    let (b, _) = tables.read(kernel, &i16_sample, position, wrap);
                    assert!(
                        (a - b).abs() < 2e-3,
                        "kernel {kernel} position {position}: f32 {a} vs i16 {b}"
                    );
                }
            }
        }
    }

    #[test]
    fn dc_is_preserved_exactly() {
        let tables = SincTables::new();
        let sample = mono_sample(vec![1.0; 4096]);
        for step in 0..100 {
            let position = 100.0 + step as f64 * 0.937;
            let (value, _) = tables.read(0, &sample, position, None);
            assert!(
                (value - 1.0).abs() < 1e-4,
                "position {position}: {value} drifted from DC"
            );
        }
    }

    #[test]
    fn sine_resampling_beats_linear_by_a_wide_margin() {
        let tables = SincTables::new();
        // 0.2 cycles/frame = 40% of Nyquist, where linear interp hurts.
        let omega = std::f64::consts::TAU * 0.2;
        let frames = 8192usize;
        let data: Vec<f32> = (0..frames).map(|n| (omega * n as f64).sin() as f32).collect();
        let sample = mono_sample(data);

        let rate = 44100.0 / 48000.0; // the everyday case
        let mut sinc_error = 0.0f64;
        let mut linear_error = 0.0f64;
        let mut signal = 0.0f64;
        let mut position = 64.0f64;
        while position < (frames - 64) as f64 {
            let ideal = (omega * position).sin();
            let (sinc_value, _) = tables.read(0, &sample, position, None);
            let (linear_value, _) = sample.read(position);
            signal += ideal * ideal;
            sinc_error += (sinc_value as f64 - ideal).powi(2);
            linear_error += (linear_value as f64 - ideal).powi(2);
            position += rate;
        }
        let sinc_snr = snr_db(signal, sinc_error);
        let linear_snr = snr_db(signal, linear_error);
        println!("sinc {sinc_snr:.1} dB vs linear {linear_snr:.1} dB at 40% Nyquist");
        assert!(
            sinc_snr > 70.0,
            "sinc SNR {sinc_snr:.1} dB below expectation"
        );
        assert!(
            sinc_snr > linear_snr + 25.0,
            "sinc {sinc_snr:.1} dB should trounce linear {linear_snr:.1} dB"
        );
    }

    /// Non-regression: the rate ≤ 1 (base kernel) path must match the
    /// pre-multi-kernel implementation exactly. Reimplements that
    /// implementation's formula independently with the LITERAL locked
    /// parameters (16 taps, cutoff 0.9) rather than referencing
    /// `BASE_TAPS`/`CUTOFF`, so the test still catches it if those ever
    /// drift.
    #[test]
    fn base_kernel_matches_locked_reference_formula() {
        fn reference_row(phase: usize) -> [f64; 16] {
            const TAPS: usize = 16;
            const CUTOFF: f64 = 0.9;
            let half_width = (TAPS / 2) as f64;
            let left_taps = TAPS / 2 - 1;
            let fraction = phase as f64 / PHASES as f64;
            let mut row = [0.0f64; TAPS];
            let mut sum = 0.0;
            for (tap, coefficient) in row.iter_mut().enumerate() {
                let x = tap as f64 - left_taps as f64 - fraction;
                let window_arg = x / half_width;
                *coefficient = if window_arg.abs() >= 1.0 {
                    0.0
                } else {
                    CUTOFF * sinc(CUTOFF * x) * kaiser(window_arg)
                };
                sum += *coefficient;
            }
            if sum != 0.0 {
                for c in row.iter_mut() {
                    *c /= sum;
                }
            }
            row
        }

        let tables = SincTables::new();
        let data: Vec<f32> = (0..256).map(|n| (n as f32 * 0.037).sin()).collect();
        let sample = mono_sample(data.clone());
        for &position in &[10.0, 10.25, 10.5, 10.75, 50.3, 50.6, 50.9, 120.123] {
            let (value, _) = tables.read(0, &sample, position, None);

            let base = position.floor();
            let fraction = position - base;
            let scaled = fraction * PHASES as f64;
            let row_index = scaled as usize;
            let row_mix = scaled - row_index as f64;
            let row0 = reference_row(row_index);
            let row1 = reference_row(row_index + 1);
            let left_taps = 16 / 2 - 1;
            let first_tap_frame = base as i64 - left_taps as i64;
            let mut expected = 0.0f64;
            for tap in 0..16 {
                let idx = (first_tap_frame + tap as i64).clamp(0, data.len() as i64 - 1) as usize;
                let coefficient = row0[tap] + (row1[tap] - row0[tap]) * row_mix;
                expected += coefficient * data[idx] as f64;
            }
            assert!(
                (value as f64 - expected).abs() < 1e-5,
                "position {position}: {value} != independently computed reference {expected}"
            );
        }
    }

    #[test]
    fn loop_seam_reads_wrap_cleanly() {
        // A sine whose period divides the loop exactly: wrapping across
        // the seam must look like the sine simply continuing.
        let period = 32usize;
        let omega = std::f64::consts::TAU / period as f64;
        let loop_start = 128usize;
        let loop_end = loop_start + period * 8; // 384
        let frames = 512usize;
        let data: Vec<f32> = (0..frames).map(|n| (omega * n as f64).sin() as f32).collect();
        let sample = mono_sample(data);
        let tables = SincTables::new();

        let wrap = Some((loop_start as u64, loop_end as u64));
        // Positions marching right up to the seam.
        for step in 0..200 {
            let position = (loop_end - 10) as f64 + step as f64 * 0.045;
            if position >= loop_end as f64 {
                break;
            }
            let (value, _) = tables.read(0, &sample, position, wrap);
            let ideal = (omega * position).sin();
            assert!(
                (value as f64 - ideal).abs() < 1e-3,
                "position {position}: {value} vs {ideal} at the loop seam"
            );
        }
    }

    #[test]
    fn simd_matches_scalar_reference() {
        let tables = SincTables::new();
        let mut rng = 0xDEAD_BEEFu32;
        let mut noise = move || {
            rng ^= rng << 13;
            rng ^= rng >> 17;
            rng ^= rng << 5;
            (rng >> 8) as f32 / (1u32 << 24) as f32 - 0.5
        };
        for &channels in &[1u16, 2] {
            let frames = 2000usize;
            let data: Vec<f32> = (0..frames * channels as usize).map(|_| noise()).collect();
            let sample = Sample::new(data.clone(), channels, 48000.0, None, 0).expect("valid");
            // Exercise every kernel, not just the base one: the SIMD
            // helpers must stay correct at every tap count they see.
            for kernel_index in 0..tables.kernels.len() {
                let taps = tables.kernels[kernel_index].taps;
                let left_taps = tables.kernels[kernel_index].left_taps;
                for probe in 0..50 {
                    let position = 50.0 + probe as f64 * 9.137;
                    let (left, right) = tables.read(kernel_index, &sample, position, None);
                    // Scalar reference, straight from the definition.
                    let base = position.floor();
                    let fraction = position - base;
                    let scaled = fraction * PHASES as f64;
                    let row_index = scaled as usize;
                    let row_mix = (scaled - row_index as f64) as f32;
                    let kernel = &tables.kernels[kernel_index];
                    let row0 = &kernel.coefficients[row_index * taps..row_index * taps + taps];
                    let row1 = &kernel.coefficients
                        [(row_index + 1) * taps..(row_index + 1) * taps + taps];
                    let first = base as usize - left_taps;
                    let ch = channels as usize;
                    let mut expect_left = 0.0f32;
                    let mut expect_right = 0.0f32;
                    for tap in 0..taps {
                        let c = row0[tap] + (row1[tap] - row0[tap]) * row_mix;
                        expect_left += c * data[(first + tap) * ch];
                        expect_right += c * data[(first + tap) * ch + (ch - 1)];
                    }
                    assert!(
                        (left - expect_left).abs() < 1e-5 && (right - expect_right).abs() < 1e-5,
                        "SIMD mismatch at {position} ({channels}ch, kernel {kernel_index}, \
                         taps {taps}): {left}/{right} vs {expect_left}/{expect_right}"
                    );
                }
            }
        }
    }

    #[test]
    fn edges_do_not_panic() {
        let tables = SincTables::new();
        let sample = mono_sample((0..64).map(|n| n as f32 / 64.0).collect());
        for position in [0.0, 0.5, 1.0, 62.9, 63.0] {
            let _ = tables.read(0, &sample, position, None);
        }
        let _ = tables.read(0, &sample, 30.0, Some((8, 32)));
    }

    #[test]
    fn every_kernel_row_has_unity_dc_gain() {
        let tables = SincTables::new();
        for (index, kernel) in tables.kernels.iter().enumerate() {
            for phase in 0..=PHASES {
                let row = &kernel.coefficients[phase * kernel.taps..(phase + 1) * kernel.taps];
                let sum: f64 = row.iter().map(|&c| c as f64).sum();
                assert!(
                    (sum - 1.0).abs() < 1e-6,
                    "kernel {index} phase {phase}: row sums to {sum}, not unity"
                );
            }
        }
    }

    #[test]
    fn bucket_selection_matches_rate() {
        let tables = SincTables::new();
        assert_eq!(tables.select(0.5), 0);
        assert_eq!(tables.select(1.0), 0);

        // Any rate > 1 must select a bucket whose ratio is >= rate.
        for &rate in &[1.05, 2.0] {
            let index = tables.select(rate);
            assert!(index >= 1, "rate {rate} should select a bucket, not base");
            let ratio = tables.bucket_ratios[index - 1];
            assert!(
                ratio >= rate,
                "bucket ratio {ratio} for rate {rate} is below rate"
            );
            // And it must be the FIRST such bucket (tightest fit).
            if index > 1 {
                assert!(
                    tables.bucket_ratios[index - 2] < rate,
                    "bucket {index} for rate {rate} isn't the tightest fit"
                );
            }
        }

        // Beyond the top bucket (4x), clamp to the last kernel.
        assert_eq!(tables.select(9.0), tables.kernels.len() - 1);
    }

    /// The whole point: a rate-2.0 read must reject the image that
    /// aliases into the output band, and the OLD (base) kernel must
    /// fail that same bar — proving the new kernels are doing real work.
    ///
    /// Arithmetic: reading a `Sample` at `rate` is equivalent to
    /// resampling the (band-limited-to-source-Nyquist) reconstructed
    /// signal onto a coarser grid spaced `rate` source-frames apart —
    /// i.e. decimating to an effective rate of `sample_rate / rate`.
    /// Analyzed against THAT rate, a source-domain frequency `f` below
    /// its Nyquist (`sample_rate / rate / 2`) survives unfolded at `f`;
    /// a source-domain frequency above it aliases to
    /// `|f - round(f / (sample_rate / rate)) * (sample_rate / rate)|`.
    /// A 9 kHz fundamental plus its 18 kHz 2nd harmonic (organ-pipe-like
    /// spectra are exactly this: strong low partials, energy well past
    /// what a naive kernel band-limits) puts the fundamental safely
    /// under the 12 kHz effective Nyquist and the harmonic safely over
    /// it, so only the harmonic should fold — down to 6 kHz.
    #[test]
    fn rate_two_rejects_aliasing_the_base_kernel_lets_through() {
        let sample_rate = 48_000.0f64;
        let source_freq = 9_000.0f64;
        let harmonic_freq = 18_000.0f64;
        let rate = 2.0f64;
        let output_rate = sample_rate / rate; // 24 kHz: effective decimated rate

        let frames = 65536usize;
        let data: Vec<f32> = (0..frames)
            .map(|n| {
                let t = n as f64 / sample_rate;
                let fundamental = (std::f64::consts::TAU * source_freq * t).sin();
                let harmonic = 0.5 * (std::f64::consts::TAU * harmonic_freq * t).sin();
                (fundamental + harmonic) as f32
            })
            .collect();
        let sample = mono_sample(data);

        let expect_through = source_freq; // 9 kHz: under output_rate/2, unfolded
        let n_alias = (harmonic_freq / output_rate).round();
        let expect_alias = (harmonic_freq - n_alias * output_rate).abs(); // 6 kHz

        let tables = SincTables::new();
        let output_frames = 4096usize;
        let rejection_db = |kernel_index: usize| -> f64 {
            let mut output = Vec::with_capacity(output_frames);
            let mut position = 1000.0f64; // clear of the start edge
            for _ in 0..output_frames {
                let (value, _) = tables.read(kernel_index, &sample, position, None);
                output.push(value as f64);
                position += rate;
            }
            let through_power = goertzel_power(&output, output_rate, expect_through);
            let alias_power = goertzel_power(&output, output_rate, expect_alias);
            10.0 * (through_power / alias_power.max(1e-30)).log10()
        };

        let bucket_index = tables.select(rate);
        let base_rejection_db = rejection_db(0);
        let kernel_rejection_db = rejection_db(bucket_index);
        println!(
            "base kernel rejection {base_rejection_db:.1} dB, bucket kernel {kernel_rejection_db:.1} dB \
             (through {expect_through:.0} Hz, alias {expect_alias:.0} Hz, bucket {bucket_index})"
        );
        assert!(
            kernel_rejection_db >= 70.0,
            "bucket kernel alias rejection {kernel_rejection_db:.1} dB below the 70 dB bar"
        );
        assert!(
            base_rejection_db < 70.0,
            "base kernel unexpectedly passes the 70 dB bar ({base_rejection_db:.1} dB) — \
             the test no longer proves the new kernels are needed"
        );
    }

    /// Goertzel single-bin DFT power at `freq_hz` over `signal` sampled
    /// at `rate_hz` — cheaper than a full FFT and all this test needs.
    fn goertzel_power(signal: &[f64], rate_hz: f64, freq_hz: f64) -> f64 {
        let n = signal.len() as f64;
        let k = (freq_hz * n / rate_hz).round();
        let omega = std::f64::consts::TAU * k / n;
        let coeff = 2.0 * omega.cos();
        let (mut s1, mut s2) = (0.0f64, 0.0f64);
        for &x in signal {
            let s0 = x + coeff * s1 - s2;
            s2 = s1;
            s1 = s0;
        }
        let real = s1 - s2 * omega.cos();
        let imag = s2 * omega.sin();
        (real * real + imag * imag) / (n * n)
    }
}

#[cfg(test)]
mod bench {
    use super::*;
    use crate::bank::Sample;

    /// Not a test: `cargo test --release -p aristide-engine bench_read -- --ignored --nocapture`
    #[test]
    #[ignore = "manual micro-benchmark"]
    fn bench_read_f32_vs_i16() {
        let tables = SincTables::new();
        let frames = 1 << 20;
        let data: Vec<f32> = (0..frames)
            .map(|n| (0.5 * (n as f64 * 0.13).sin()) as f32)
            .collect();
        for channels in [1u16, 2u16] {
            let raw: Vec<f32> = if channels == 2 {
                data.iter().flat_map(|&v| [v, v]).collect()
            } else {
                data.clone()
            };
            let f32_sample = Sample::new(raw.clone(), channels, 48000.0, None, 0).unwrap();
            let mut i16_sample = Sample::new(raw, channels, 48000.0, None, 0).unwrap();
            i16_sample.quantize_i16();
            for (label, sample) in [("f32", &f32_sample), ("i16", &i16_sample)] {
                let started = std::time::Instant::now();
                let mut acc = 0.0f32;
                let mut position = 100.0f64;
                for _ in 0..3_000_000u32 {
                    let (l, _) = tables.read(0, sample, position, None);
                    acc += l;
                    position += 0.918_762_5;
                    if position > (frames - 64) as f64 {
                        position = 100.0;
                    }
                }
                println!(
                    "{label} ch{channels}: {:?} for 3M reads (acc {acc})",
                    started.elapsed()
                );
            }
        }
    }
}
