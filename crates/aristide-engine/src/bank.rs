//! Immutable sample storage shared with the audio thread.
//!
//! A [`SampleBank`] is built control-side (decode, validation, layout),
//! wrapped in an `Arc`, and handed to the engine at construction. The RT
//! path only ever reads it, so the invariants in the crate root hold.
//!
//! M3 state: every sample is fully decoded in RAM. The `Sample` layout
//! (contiguous interleaved f32 + frame-based markers) is what a future
//! disk streamer will fill ring buffers with, so streaming replaces the
//! storage behind this API rather than the API itself.

/// Phase buckets per waveform period in a [`ReleaseAlignment`] table.
pub const ALIGNMENT_BUCKETS: usize = 64;

/// Phase-aligned release splicing: precomputed control-side, indexed by
/// the RT thread in O(1) on note-off.
///
/// Splicing from the sustain loop to the release tail at a fixed frame
/// lands at a random point in the waveform's cycle — the crossfade then
/// partially cancels (a dip, or a double-strike "click"). This table
/// maps the voice's phase within its fundamental period to the tail
/// frame with the *same* phase, so the splice continues the waveform.
#[derive(Debug, Clone)]
pub struct ReleaseAlignment {
    /// Fundamental period in frames at the file's own sample rate.
    period: f64,
    /// `offsets[b]` = tail frame whose phase matches a voice at phase
    /// `b / ALIGNMENT_BUCKETS` (phase measured from the loop start).
    offsets: Vec<u32>,
}

impl ReleaseAlignment {
    /// The aligned splice target for a voice currently at `position`,
    /// inside a loop starting at `loop_start`. RT-safe: two fmods and a
    /// table index.
    #[inline]
    pub fn target(&self, position: f64, loop_start: u64) -> u64 {
        // rem_euclid-style fract: correct even for releases that arrive
        // before the cursor first reaches the loop.
        let phase = (((position - loop_start as f64) / self.period).fract() + 1.0).fract();
        let bucket = ((phase * ALIGNMENT_BUCKETS as f64) as usize).min(ALIGNMENT_BUCKETS - 1);
        self.offsets[bucket] as u64
    }
}

/// One decoded audio file: attack, sustain loop, and (optionally) an
/// embedded release tail after the loop.
#[derive(Debug, Clone)]
pub struct Sample {
    /// Interleaved samples, normalized to `[-1.0, 1.0]`.
    data: Vec<f32>,
    channels: u16,
    sample_rate_hz: f32,
    /// Sustain loop as a half-open frame range; `None` = one-shot
    /// (percussive samples such as action noises play to the end).
    sustain_loop: Option<(u64, u64)>,
    /// Frame where the embedded release tail starts. Only meaningful if
    /// it lies strictly before `frames()`; loop-less samples ignore it.
    release_start: u64,
    release_alignment: Option<ReleaseAlignment>,
    /// Mean |sample| over the tail's first stretch — the loudness the
    /// recorded release *starts* at. Voices scale the tail so it
    /// continues at their own current level instead of striking at the
    /// recording's (the "bell" artifact).
    tail_reference_level: f32,
}

impl Sample {
    /// Validates frame markers against the data so the RT reader never
    /// has to re-check anything but its own cursors.
    pub fn new(
        data: Vec<f32>,
        channels: u16,
        sample_rate_hz: f32,
        sustain_loop: Option<(u64, u64)>,
        release_start: u64,
    ) -> Result<Sample, String> {
        if channels == 0 {
            return Err("zero channels".into());
        }
        if data.len() % channels as usize != 0 {
            return Err(format!(
                "data length {} not a multiple of {channels} channels",
                data.len()
            ));
        }
        let frames = (data.len() / channels as usize) as u64;
        if frames == 0 {
            return Err("empty sample".into());
        }
        if let Some((start, end)) = sustain_loop {
            if start >= end || end > frames {
                return Err(format!(
                    "loop {start}..{end} out of bounds for {frames} frames"
                ));
            }
        }
        if !(sample_rate_hz.is_finite() && sample_rate_hz > 0.0) {
            return Err(format!("bad sample rate {sample_rate_hz}"));
        }
        let mut sample = Sample {
            data,
            channels,
            sample_rate_hz,
            sustain_loop,
            release_start: release_start.min(frames),
            release_alignment: None,
            tail_reference_level: 0.0,
        };
        if let Some(tail) = sample.release_start() {
            let window = 2048.min(sample.frames() - tail);
            sample.tail_reference_level = sample.mean_abs(tail, window);
        }
        Ok(sample)
    }

    /// Mean absolute value (mono-summed) over `window` frames from
    /// `start` — the level metric shared with the voices' envelope
    /// followers.
    fn mean_abs(&self, start: u64, window: u64) -> f32 {
        let ch = self.channels as usize;
        let mut sum = 0.0f64;
        for frame in start..start + window {
            let base = frame as usize * ch;
            let value = if ch == 1 {
                self.data[base].abs()
            } else {
                (self.data[base].abs() + self.data[base + 1].abs()) * 0.5
            };
            sum += value as f64;
        }
        (sum / window.max(1) as f64) as f32
    }

    #[inline]
    pub fn tail_reference_level(&self) -> f32 {
        self.tail_reference_level
    }

    /// Control-side analysis: build the [`ReleaseAlignment`] table for a
    /// pipe sounding at `fundamental_hz`.
    ///
    /// One cross-correlation search locates the tail frame matching
    /// phase 0 (the loop start's phase); the remaining buckets follow
    /// arithmetically because the tail continues the same periodic
    /// waveform. Skips silently when the sample has no spliceable tail
    /// or the tail is too short to search — the fixed splice remains.
    pub fn align_release(&mut self, fundamental_hz: f32) {
        let Some((loop_start, _)) = self.sustain_loop else {
            return;
        };
        let Some(tail) = self.release_start() else {
            return;
        };
        if !(fundamental_hz > 0.0) {
            return;
        }
        let period = self.sample_rate_hz as f64 / fundamental_hz as f64;
        let period_frames = period.round() as u64;
        // Correlation window: one period, capped to keep analysis cheap.
        let window = period_frames.min(600).max(16);
        let frames = self.frames();
        if period_frames < 4 || tail + period_frames + window >= frames {
            return;
        }

        // Template: the waveform right at the loop start (phase 0).
        let score = |offset: u64| -> f64 {
            let mut dot = 0.0f64;
            let mut energy = 0.0f64;
            let ch = self.channels as usize;
            for i in 0..window {
                let a = self.data[(loop_start + i) as usize * ch] as f64;
                let b = self.data[(offset + i) as usize * ch] as f64;
                dot += a * b;
                energy += b * b;
            }
            dot / energy.max(1e-12).sqrt()
        };
        let phase0 = (tail..tail + period_frames)
            .map(|offset| (offset, score(offset)))
            .max_by(|a, b| a.1.total_cmp(&b.1))
            .expect("non-empty range")
            .0;

        let offsets = (0..ALIGNMENT_BUCKETS)
            .map(|bucket| {
                let advance = (bucket as f64 / ALIGNMENT_BUCKETS as f64 * period).round() as u64;
                let mut offset = phase0 + advance;
                if offset >= tail + period_frames {
                    offset -= period_frames;
                }
                offset as u32
            })
            .collect();
        self.release_alignment = Some(ReleaseAlignment { period, offsets });
    }

    #[inline]
    pub fn release_alignment(&self) -> Option<&ReleaseAlignment> {
        self.release_alignment.as_ref()
    }

    #[inline]
    pub fn frames(&self) -> u64 {
        (self.data.len() / self.channels as usize) as u64
    }

    #[inline]
    pub fn channels(&self) -> u16 {
        self.channels
    }

    #[inline]
    pub fn sample_rate_hz(&self) -> f32 {
        self.sample_rate_hz
    }

    #[inline]
    pub fn sustain_loop(&self) -> Option<(u64, u64)> {
        self.sustain_loop
    }

    /// The release tail's first frame, if this sample has one to splice to.
    #[inline]
    pub fn release_start(&self) -> Option<u64> {
        (self.sustain_loop.is_some() && self.release_start < self.frames())
            .then_some(self.release_start)
    }

    /// Raw interleaved data + channel count, for the sinc reader.
    #[inline]
    pub(crate) fn raw(&self) -> (&[f32], u16) {
        (&self.data, self.channels)
    }

    /// Linearly interpolated stereo read at a fractional frame position.
    /// Mono samples are duplicated to both outputs; positions at or past
    /// the last frame clamp to it.
    #[inline]
    pub fn read(&self, position: f64) -> (f32, f32) {
        let last = self.frames() - 1;
        let index = (position as u64).min(last);
        let next = (index + 1).min(last);
        let fraction = (position - index as f64) as f32;
        let ch = self.channels as usize;
        let a = index as usize * ch;
        let b = next as usize * ch;
        let left = self.data[a] + (self.data[b] - self.data[a]) * fraction;
        if ch == 1 {
            (left, left)
        } else {
            let right = self.data[a + 1] + (self.data[b + 1] - self.data[a + 1]) * fraction;
            (left, right)
        }
    }
}

/// The set of samples a loaded instrument plays from. Index-stable:
/// voices refer to samples by position in `samples`.
#[derive(Debug, Clone, Default)]
pub struct SampleBank {
    samples: Vec<Sample>,
}

impl SampleBank {
    pub fn push(&mut self, sample: Sample) -> u32 {
        self.samples.push(sample);
        (self.samples.len() - 1) as u32
    }

    #[inline]
    pub fn get(&self, index: u32) -> Option<&Sample> {
        self.samples.get(index as usize)
    }

    pub fn len(&self) -> usize {
        self.samples.len()
    }

    pub fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }

    /// Total decoded audio held, in bytes.
    pub fn resident_bytes(&self) -> usize {
        self.samples
            .iter()
            .map(|s| s.data.len() * size_of::<f32>())
            .sum()
    }
}
