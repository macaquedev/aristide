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
        Ok(Sample {
            data,
            channels,
            sample_rate_hz,
            sustain_loop,
            release_start: release_start.min(frames),
        })
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
