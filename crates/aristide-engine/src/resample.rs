//! Polyphase windowed-sinc interpolation — the RT sample reader.
//!
//! Linear interpolation (Sample::read) aliases audibly once material is
//! pitch-shifted (temperaments, random detune, 44.1→48 kHz). This table
//! trades 16 multiply-adds per output sample for ~50 dB better error
//! floors. The table is built control-side at engine construction; the
//! audio thread only indexes into it, upholding the crate invariants.
//!
//! Design: `PHASES` Kaiser-windowed sinc kernels of `TAPS` coefficients,
//! one per fractional offset step, linearly interpolated between
//! neighbouring kernels at read time. Each kernel row is normalized to
//! unity DC gain, so amplitude ripple between phases cancels exactly for
//! constant signals.

use crate::bank::Sample;

/// FIR length per phase. 16 taps × Kaiser β=9 gives ≈ −90 dB stopband —
/// below any organ sample's noise floor — at a cost small enough for
/// thousands of voices.
pub const TAPS: usize = 16;
/// Fractional-position resolution. With inter-row interpolation the
/// phase-quantization error sits far below the kernel's own floor.
pub const PHASES: usize = 512;

/// Cutoff as a fraction of Nyquist: headroom for mild upward
/// transposition (temperament stretch, detune) before aliasing.
const CUTOFF: f64 = 0.9;
const KAISER_BETA: f64 = 9.0;

/// Reads start this many frames before the integer position.
const LEFT_TAPS: usize = TAPS / 2 - 1;

pub struct SincTable {
    /// `(PHASES + 1) × TAPS`, row-major; row `p` is the kernel for
    /// fractional offset `p / PHASES`. The extra row lets reads
    /// interpolate `p → p+1` without wrapping.
    coefficients: Box<[f32]>,
}

impl SincTable {
    pub fn new() -> SincTable {
        let mut coefficients = vec![0.0f32; (PHASES + 1) * TAPS].into_boxed_slice();
        let half_width = (TAPS / 2) as f64;
        for phase in 0..=PHASES {
            let fraction = phase as f64 / PHASES as f64;
            let row = &mut coefficients[phase * TAPS..(phase + 1) * TAPS];
            let mut sum = 0.0f64;
            for (tap, coefficient) in row.iter_mut().enumerate() {
                let x = tap as f64 - LEFT_TAPS as f64 - fraction;
                let window_arg = x / half_width;
                let value = if window_arg.abs() >= 1.0 {
                    0.0
                } else {
                    CUTOFF * sinc(CUTOFF * x) * kaiser(window_arg)
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
        SincTable { coefficients }
    }

    /// Interpolated stereo read at a fractional frame position.
    ///
    /// `loop_wrap`: while a voice circles a sustain loop, kernel taps
    /// that fall at/after the loop end must wrap back by the loop
    /// length, or every pass across the seam would click. Tail reads
    /// pass `None` and clamp at the sample edges instead.
    #[inline]
    pub fn read(
        &self,
        sample: &Sample,
        position: f64,
        loop_wrap: Option<(u64, u64)>,
    ) -> (f32, f32) {
        let (data, channels) = sample.raw();
        let channels = channels as usize;
        let frames = (data.len() / channels) as i64;

        let base = position.floor();
        let fraction = position - base;
        let scaled = fraction * PHASES as f64;
        let row_index = scaled as usize; // 0..=PHASES-1 since fraction < 1
        let row_mix = (scaled - row_index as f64) as f32;
        let row0 = &self.coefficients[row_index * TAPS..row_index * TAPS + TAPS];
        let row1 = &self.coefficients[(row_index + 1) * TAPS..(row_index + 1) * TAPS + TAPS];

        let first_tap_frame = base as i64 - LEFT_TAPS as i64;

        // Fast path: the whole window is in-bounds and doesn't straddle
        // the loop seam — one contiguous dot product.
        let seam_safe = match loop_wrap {
            Some((_, end)) => first_tap_frame + TAPS as i64 <= end as i64,
            None => true,
        };
        if seam_safe && first_tap_frame >= 0 && first_tap_frame + (TAPS as i64) <= frames {
            let start = first_tap_frame as usize * channels;
            let window = &data[start..start + TAPS * channels];
            let mut left = 0.0f32;
            let mut right = 0.0f32;
            if channels == 1 {
                for tap in 0..TAPS {
                    let coefficient = row0[tap] + (row1[tap] - row0[tap]) * row_mix;
                    left += coefficient * window[tap];
                }
                return (left, left);
            }
            for tap in 0..TAPS {
                let coefficient = row0[tap] + (row1[tap] - row0[tap]) * row_mix;
                left += coefficient * window[tap * channels];
                right += coefficient * window[tap * channels + 1];
            }
            return (left, right);
        }

        // Slow path (window touches an edge or the loop seam): map each
        // tap's frame index individually.
        let mut left = 0.0f32;
        let mut right = 0.0f32;
        for tap in 0..TAPS {
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
            left += coefficient * data[frame * channels];
            if channels > 1 {
                right += coefficient * data[frame * channels + 1];
            }
        }
        if channels == 1 {
            (left, left)
        } else {
            (left, right)
        }
    }
}

impl Default for SincTable {
    fn default() -> Self {
        SincTable::new()
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

    #[test]
    fn dc_is_preserved_exactly() {
        let table = SincTable::new();
        let sample = mono_sample(vec![1.0; 4096]);
        for step in 0..100 {
            let position = 100.0 + step as f64 * 0.937;
            let (value, _) = table.read(&sample, position, None);
            assert!(
                (value - 1.0).abs() < 1e-4,
                "position {position}: {value} drifted from DC"
            );
        }
    }

    #[test]
    fn sine_resampling_beats_linear_by_a_wide_margin() {
        let table = SincTable::new();
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
            let (sinc_value, _) = table.read(&sample, position, None);
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
        let table = SincTable::new();

        let wrap = Some((loop_start as u64, loop_end as u64));
        // Positions marching right up to the seam.
        for step in 0..200 {
            let position = (loop_end - 10) as f64 + step as f64 * 0.045;
            if position >= loop_end as f64 {
                break;
            }
            let (value, _) = table.read(&sample, position, wrap);
            let ideal = (omega * position).sin();
            assert!(
                (value as f64 - ideal).abs() < 1e-3,
                "position {position}: {value} vs {ideal} at the loop seam"
            );
        }
    }

    #[test]
    fn edges_do_not_panic() {
        let table = SincTable::new();
        let sample = mono_sample((0..64).map(|n| n as f32 / 64.0).collect());
        for position in [0.0, 0.5, 1.0, 62.9, 63.0] {
            let _ = table.read(&sample, position, None);
        }
        let _ = table.read(&sample, 30.0, Some((8, 32)));
    }
}
