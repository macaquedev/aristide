//! Convolution reverb: uniformly partitioned overlap-save (UPOLS).
//!
//! The standard real-time scheme (Wefers, *Partitioned convolution
//! algorithms for real-time auralization*, Logos 2015; Gardner 1995 for
//! the zero-latency non-uniform refinement we defer to v2): the impulse
//! response is chopped into `BLOCK`-frame partitions, each stored as a
//! spectrum; per input block one forward FFT feeds a frequency-domain
//! delay line, one complex multiply-accumulate per partition and one
//! inverse FFT produce the wet block. Cost is independent of IR length
//! per sample beyond the MACs — a 4 s IR is ~750 partitions × 129
//! complex MACs per 256-frame block per channel: trivial.
//!
//! RT invariants: every buffer and FFT scratch is allocated in
//! [`PreparedIr::prepare`]/[`Reverb::new`] (control side); the audio
//! thread only indexes, multiplies, and runs preplanned FFTs with
//! preallocated scratch. Wet output trails the dry by one block
//! (5.3 ms at 48 kHz) — heard as a tiny pre-delay, inaudible inside a
//! reverb tail. GO routes its dry path through the convolver with a
//! Dirac spike; we simply don't delay the dry signal at all.

use std::sync::Arc;

use realfft::num_complex::Complex;
use realfft::{ComplexToReal, RealFftPlanner, RealToComplex};

use crate::bank::Sample;
use crate::resample::SincTable;

/// Internal processing block (frames). 256 keeps latency at ~5 ms and
/// the FFT in L1.
pub const BLOCK: usize = 256;
const FFT_SIZE: usize = 2 * BLOCK;
const SPECTRUM: usize = FFT_SIZE / 2 + 1;
/// Cap the IR so a runaway file can't eat the CPU (6 s at 48 kHz).
const MAX_IR_FRAMES: usize = 6 * 48_000;

/// A frequency-domain impulse response, prepared control-side.
pub struct PreparedIr {
    /// Per output channel (stereo), the partition spectra.
    partitions: [Vec<Vec<Complex<f32>>>; 2],
    forward: Arc<dyn RealToComplex<f32>>,
    inverse: Arc<dyn ComplexToReal<f32>>,
}

impl PreparedIr {
    /// Build from interleaved IR data at `ir_rate`, resampled to
    /// `device_rate` with the engine's own sinc reader.
    pub fn prepare(
        data: &[f32],
        channels: u16,
        ir_rate: f32,
        device_rate: f32,
    ) -> Result<PreparedIr, String> {
        if channels == 0 || data.is_empty() {
            return Err("empty impulse response".into());
        }
        let sample = Sample::new(data.to_vec(), channels, ir_rate, None, u64::MAX)?;
        let ratio = ir_rate as f64 / device_rate as f64;
        let out_frames = ((sample.frames() as f64 / ratio) as usize).min(MAX_IR_FRAMES);
        if out_frames == 0 {
            return Err("impulse response too short".into());
        }

        // Split into stereo channel buffers (mono IRs duplicate). Only
        // resample when the rates actually differ: the sinc kernel is a
        // 0.9-Nyquist lowpass, which would needlessly soften a same-rate
        // IR's taps.
        let mut resampled: [Vec<f32>; 2] = [
            Vec::with_capacity(out_frames),
            Vec::with_capacity(out_frames),
        ];
        if (ratio - 1.0).abs() < 1e-9 {
            let ch = channels as usize;
            for frame in 0..out_frames {
                let left = data[frame * ch];
                let right = if ch > 1 { data[frame * ch + 1] } else { left };
                resampled[0].push(left);
                resampled[1].push(right);
            }
        } else {
            let table = SincTable::new();
            for frame in 0..out_frames {
                let (left, right) = table.read(&sample, frame as f64 * ratio, None);
                resampled[0].push(left);
                resampled[1].push(right);
            }
        }

        // Normalize: unity total energy per channel pair, so the wet
        // level is comparable across IRs and wet=1 doesn't clip badly.
        let energy: f32 = resampled
            .iter()
            .flat_map(|c| c.iter())
            .map(|v| v * v)
            .sum();
        let scale = if energy > 1e-12 {
            (2.0 / energy).sqrt()
        } else {
            return Err("silent impulse response".into());
        };

        let mut planner = RealFftPlanner::<f32>::new();
        let forward = planner.plan_fft_forward(FFT_SIZE);
        let inverse = planner.plan_fft_inverse(FFT_SIZE);

        let mut partitions: [Vec<Vec<Complex<f32>>>; 2] = [Vec::new(), Vec::new()];
        let mut time_buffer = forward.make_input_vec();
        let mut scratch = forward.make_scratch_vec();
        for channel in 0..2 {
            for chunk in resampled[channel].chunks(BLOCK) {
                time_buffer.fill(0.0);
                for (out, &v) in time_buffer.iter_mut().zip(chunk.iter()) {
                    *out = v * scale;
                }
                let mut spectrum = forward.make_output_vec();
                forward
                    .process_with_scratch(&mut time_buffer, &mut spectrum, &mut scratch)
                    .map_err(|e| e.to_string())?;
                partitions[channel].push(spectrum);
            }
        }
        Ok(PreparedIr {
            partitions,
            forward,
            inverse,
        })
    }

    pub fn partition_count(&self) -> usize {
        self.partitions[0].len()
    }
}

/// Per-channel UPOLS state.
struct ChannelState {
    /// Frequency-domain delay line: newest spectrum at `head`.
    fdl: Vec<Vec<Complex<f32>>>,
    /// Last input block (the overlap of overlap-save).
    previous: Vec<f32>,
    /// Wet output waiting to be mixed (one block behind the dry).
    pending: Vec<f32>,
}

/// The RT-side reverb. All hot-path work uses preallocated storage.
pub struct Reverb {
    ir: Arc<PreparedIr>,
    channels: [ChannelState; 2],
    head: usize,
    /// Frames accumulated toward the next block boundary.
    fill: usize,
    input_accumulator: [Vec<f32>; 2],
    time_scratch: Vec<f32>,
    spectrum_scratch: Vec<Complex<f32>>,
    accumulator: Vec<Complex<f32>>,
    fft_scratch: Vec<Complex<f32>>,
    ifft_scratch: Vec<Complex<f32>>,
    wet: f32,
}

impl Reverb {
    pub fn new(ir: Arc<PreparedIr>, wet: f32) -> Reverb {
        let partitions = ir.partition_count().max(1);
        let channel = || ChannelState {
            fdl: vec![vec![Complex::default(); SPECTRUM]; partitions],
            previous: vec![0.0; BLOCK],
            pending: vec![0.0; BLOCK],
        };
        Reverb {
            channels: [channel(), channel()],
            head: 0,
            fill: 0,
            input_accumulator: [vec![0.0; BLOCK], vec![0.0; BLOCK]],
            time_scratch: ir.forward.make_input_vec(),
            spectrum_scratch: ir.forward.make_output_vec(),
            accumulator: vec![Complex::default(); SPECTRUM],
            fft_scratch: ir.forward.make_scratch_vec(),
            ifft_scratch: ir.inverse.make_scratch_vec(),
            wet: wet.clamp(0.0, 2.0),
            ir,
        }
    }

    pub fn set_wet(&mut self, wet: f32) {
        self.wet = wet.clamp(0.0, 2.0);
    }

    pub fn wet(&self) -> f32 {
        self.wet
    }

    /// Mix the wet signal into an interleaved buffer in place.
    pub fn process(&mut self, buffer: &mut [f32], out_channels: usize) {
        if self.wet <= 0.0 {
            return;
        }
        let out_channels = out_channels.max(1);
        let frames = buffer.len() / out_channels;
        for frame in 0..frames {
            let base = frame * out_channels;
            // Feed the dry mix into the accumulator (mono folds to both).
            let left = buffer[base];
            let right = if out_channels > 1 {
                buffer[base + 1]
            } else {
                left
            };
            self.input_accumulator[0][self.fill] = left;
            self.input_accumulator[1][self.fill] = right;

            // Mix out the wet block computed one block ago.
            let wet_left = self.channels[0].pending[self.fill] * self.wet;
            let wet_right = self.channels[1].pending[self.fill] * self.wet;
            buffer[base] += wet_left;
            if out_channels > 1 {
                buffer[base + 1] += wet_right;
            } else {
                buffer[base] += 0.0; // mono: left already carries the fold
            }

            self.fill += 1;
            if self.fill == BLOCK {
                self.fill = 0;
                self.advance_block();
            }
        }
    }

    /// One UPOLS step for both channels.
    fn advance_block(&mut self) {
        let partitions = self.ir.partition_count();
        if partitions == 0 {
            return;
        }
        self.head = (self.head + partitions - 1) % partitions;
        for channel in 0..2 {
            let state = &mut self.channels[channel];
            // Overlap-save input: [previous | current].
            self.time_scratch[..BLOCK].copy_from_slice(&state.previous);
            self.time_scratch[BLOCK..].copy_from_slice(&self.input_accumulator[channel]);
            state
                .previous
                .copy_from_slice(&self.input_accumulator[channel]);

            let _ = self.ir.forward.process_with_scratch(
                &mut self.time_scratch,
                &mut state.fdl[self.head],
                &mut self.fft_scratch,
            );

            // Multiply-accumulate over the delay line.
            self.accumulator.fill(Complex::default());
            for p in 0..partitions {
                let spectrum = &state.fdl[(self.head + p) % partitions];
                let ir_part = &self.ir.partitions[channel][p];
                for ((acc, x), h) in self
                    .accumulator
                    .iter_mut()
                    .zip(spectrum.iter())
                    .zip(ir_part.iter())
                {
                    *acc += x * h;
                }
            }

            self.spectrum_scratch.copy_from_slice(&self.accumulator);
            let _ = self.ir.inverse.process_with_scratch(
                &mut self.spectrum_scratch,
                &mut self.time_scratch,
                &mut self.ifft_scratch,
            );
            // Overlap-save keeps the SECOND half; realfft's inverse is
            // unnormalized, so scale by 1/N.
            let scale = 1.0 / FFT_SIZE as f32;
            for (out, &v) in state
                .pending
                .iter_mut()
                .zip(self.time_scratch[BLOCK..].iter())
            {
                *out = v * scale;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prepare(ir: Vec<f32>) -> Arc<PreparedIr> {
        Arc::new(PreparedIr::prepare(&ir, 1, 48000.0, 48000.0).expect("prepare"))
    }

    #[test]
    fn impulse_reproduces_the_ir() {
        // IR: a few distinct taps across two partitions.
        let mut ir = vec![0.0f32; 400];
        ir[0] = 1.0;
        ir[100] = 0.5;
        ir[300] = -0.25;
        let prepared = prepare(ir.clone());
        let mut reverb = Reverb::new(prepared, 1.0);

        // Unit impulse at frame 0, stereo interleaved, several blocks.
        let frames = 4 * BLOCK;
        let mut buffer = vec![0.0f32; frames * 2];
        buffer[0] = 1.0;
        buffer[1] = 1.0;
        let dry = buffer.clone();
        reverb.process(&mut buffer, 2);

        // Energy was normalized in prepare; recover the scale from the
        // biggest tap and check tap ratios + positions (wet is delayed
        // by exactly one BLOCK).
        let wet: Vec<f32> = buffer
            .chunks(2)
            .zip(dry.chunks(2))
            .map(|(y, x)| y[0] - x[0])
            .collect();
        let scale = wet[BLOCK];
        assert!(scale > 0.1, "first tap missing: {scale}");
        let at = |ir_index: usize| wet[BLOCK + ir_index] / scale;
        assert!((at(100) - 0.5).abs() < 1e-3, "tap at 100: {}", at(100));
        assert!((at(300) + 0.25).abs() < 1e-3, "tap at 300: {}", at(300));
        // Silence where the IR has no taps.
        assert!(wet[BLOCK + 50].abs() / scale < 1e-3);
    }

    #[test]
    fn zero_wet_is_bit_exact_passthrough() {
        let prepared = prepare(vec![1.0, 0.7, 0.3]);
        let mut reverb = Reverb::new(prepared, 0.0);
        let mut buffer: Vec<f32> = (0..2048).map(|i| (i as f32 * 0.37).sin()).collect();
        let original = buffer.clone();
        reverb.process(&mut buffer, 2);
        assert_eq!(buffer, original);
    }

    #[test]
    fn long_ir_rings_after_input_stops() {
        // 2000-frame exponential decay: ~8 partitions.
        let ir: Vec<f32> = (0..2000)
            .map(|i| (-(i as f32) / 400.0).exp() * if i % 7 == 0 { 1.0 } else { 0.3 })
            .collect();
        let prepared = prepare(ir);
        assert!(prepared.partition_count() >= 8);
        let mut reverb = Reverb::new(prepared, 1.0);

        // One noisy block, then silence.
        let mut noise_state = 0x2468_ACE0u32;
        let mut noise = move || {
            noise_state ^= noise_state << 13;
            noise_state ^= noise_state >> 17;
            noise_state ^= noise_state << 5;
            (noise_state >> 8) as f32 / (1u32 << 24) as f32 - 0.5
        };
        let mut buffer = vec![0.0f32; BLOCK * 2];
        for v in buffer.iter_mut() {
            *v = noise();
        }
        reverb.process(&mut buffer, 2);

        let mut tail_energy = 0.0f32;
        for _ in 0..8 {
            let mut silence = vec![0.0f32; BLOCK * 2];
            reverb.process(&mut silence, 2);
            tail_energy += silence.iter().map(|v| v * v).sum::<f32>();
        }
        assert!(
            tail_energy > 1e-4,
            "reverb tail should ring after input stops: {tail_energy}"
        );
    }
}
