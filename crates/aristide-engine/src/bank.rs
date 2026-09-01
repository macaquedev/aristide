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
pub(crate) const ALIGNMENT_BUCKETS: usize = 64;

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

/// Dequantization scale for 16-bit resident audio.
pub const I16_SCALE: f32 = 1.0 / 32768.0;

/// Decoded audio at the resolution the instrument keeps resident.
/// 16-bit halves RAM against f32 with a −96 dB floor — below organ
/// recordings' own room noise, and effectively what GO and HW play
/// from by default; f32 keeps the decode bit-exact for A/B.
#[derive(Debug, Clone)]
pub enum SampleData {
    F32(Vec<f32>),
    I16(Vec<i16>),
}

impl SampleData {
    #[inline]
    pub fn len(&self) -> usize {
        match self {
            SampleData::F32(data) => data.len(),
            SampleData::I16(data) => data.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// One interleaved value as f32, whatever the resident format.
    #[inline]
    pub fn get(&self, index: usize) -> f32 {
        match self {
            SampleData::F32(data) => data[index],
            SampleData::I16(data) => f32::from(data[index]) * I16_SCALE,
        }
    }

    /// Resident bytes of audio.
    pub fn bytes(&self) -> usize {
        match self {
            SampleData::F32(data) => data.len() * size_of::<f32>(),
            SampleData::I16(data) => data.len() * size_of::<i16>(),
        }
    }
}

/// One decoded audio file: attack, sustain loop, and (optionally) an
/// embedded release tail after the loop.
#[derive(Debug, Clone)]
pub struct Sample {
    /// Interleaved samples, normalized to `[-1.0, 1.0]`.
    data: SampleData,
    channels: u16,
    sample_rate_hz: f32,
    /// Sustain loop as a half-open frame range; `None` = one-shot
    /// (percussive samples such as action noises play to the end).
    sustain_loop: Option<(u64, u64)>,
    /// Additional sustain loops (author-provided alternates). Voices
    /// pick a loop at random per pass, decorrelating repetition between
    /// passes and between unison pipes.
    extra_loops: Vec<(u64, u64)>,
    /// Frame where the embedded release tail starts. Only meaningful if
    /// it lies strictly before `frames()`; loop-less samples ignore it.
    release_start: u64,
    release_alignment: Option<ReleaseAlignment>,
    /// Measured fundamental period, once alignment analysis has run —
    /// shared by the embedded and separate-release phase maps.
    measured_period: Option<f64>,
    /// Separate recorded releases, sorted by `max_hold_ms` (None last).
    releases: Vec<ReleaseOption>,
    /// Mean |sample| over the tail's first stretch — the loudness the
    /// recorded release *starts* at. Voices scale the tail so it
    /// continues at their own current level instead of striking at the
    /// recording's (the "bell" artifact).
    tail_reference_level: f32,
    /// Measured decay rate of the embedded tail, in amplitude dB per
    /// second (positive = decaying). Repitching a sample by rate R also
    /// time-scales its recorded room decay by R — but a room's decay
    /// rate does not transpose (Rucz 2015; Angster and Miklos). Voices
    /// use this to gain-compensate so ring time stays key-invariant.
    tail_decay_db_per_s: f32,
    /// Level of the tail's final stretch relative to its loudest stretch,
    /// in dB (≤ 0). A recording truncated mid-decay — or a mislabeled
    /// release that never decays — is still audible when the data runs
    /// out, and the voice would end in a hard cut. Voices add whatever
    /// extra decay settles the tail to silence by EOF.
    tail_eof_level_db: f32,
    /// Frame where playback starts (ODF `AttackStart`; 0 = the file's
    /// first frame) — lead-in the producer marked as not-the-note.
    attack_start: u64,
    /// Producer-tuned key-off crossfade into the *embedded* tail, in
    /// ms (ODF `ReleaseCrossfadeLength`; 0 = the engine's pitch-scaled
    /// default). Separate releases carry theirs on [`ReleaseOption`].
    release_crossfade_ms: u16,
}

/// A separate recorded release, selectable by how long the note was
/// held (GO `MaxKeyPressTime`; HW multi-release sampling).
#[derive(Debug, Clone)]
pub struct ReleaseOption {
    /// Bank index of the release sample (a one-shot entry).
    pub sample: u32,
    /// Selected when the note was held at most this long; `None` = the
    /// default/longest release.
    pub max_hold_ms: Option<u32>,
    /// Which wave-tremulant state this release was recorded under
    /// (GO `IsTremulant` tri-state): `Some(true)` only while the
    /// chest's wave trem is engaged, `Some(false)` only while it is
    /// not, `None` either way.
    pub wave_trem: Option<bool>,
    /// Producer-tuned key-off crossfade in ms (ODF
    /// `ReleaseCrossfadeLength`; 0 = the engine's pitch-scaled fade).
    pub crossfade_ms: u16,
    /// Phase map from the source sample's cycle into the release's
    /// opening period.
    alignment: Option<ReleaseAlignment>,
    /// Mean |sample| of the release head, for level matching.
    pub level: f32,
}

impl ReleaseOption {
    #[inline]
    pub fn alignment(&self) -> Option<&ReleaseAlignment> {
        self.alignment.as_ref()
    }
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
        if !data.len().is_multiple_of(channels as usize) {
            return Err(format!(
                "data length {} not a multiple of {channels} channels",
                data.len()
            ));
        }
        let frames = (data.len() / channels as usize) as u64;
        if frames == 0 {
            return Err("empty sample".into());
        }
        if let Some((start, end)) = sustain_loop
            && (start >= end || end > frames)
        {
            return Err(format!(
                "loop {start}..{end} out of bounds for {frames} frames"
            ));
        }
        if !(sample_rate_hz.is_finite() && sample_rate_hz > 0.0) {
            return Err(format!("bad sample rate {sample_rate_hz}"));
        }
        let mut sample = Sample {
            data: SampleData::F32(data),
            channels,
            sample_rate_hz,
            sustain_loop,
            extra_loops: Vec::new(),
            release_start: release_start.min(frames),
            release_alignment: None,
            measured_period: None,
            releases: Vec::new(),
            tail_reference_level: 0.0,
            tail_decay_db_per_s: 0.0,
            tail_eof_level_db: -120.0,
            attack_start: 0,
            release_crossfade_ms: 0,
        };
        if let Some(tail) = sample.release_start() {
            // Short window (~12 ms at 44.1 k): high pipes' room decay is
            // fast, and a long average under-reads the tail's starting
            // level, making the level matcher boost it — a ping.
            let window = 512.min(sample.frames() - tail);
            sample.tail_reference_level = sample.mean_abs(tail, window);
            let (decay, eof_db) = sample.measure_tail(tail);
            sample.tail_decay_db_per_s = decay;
            sample.tail_eof_level_db = eof_db;
        }
        Ok(sample)
    }

    /// Fundamental phase (radians) of the waveform at `start`, by
    /// projecting `window` frames onto sin/cos at `period` — immune to
    /// harmonic content, unlike correlation peaks.
    fn quadrature_phase(&self, start: u64, window: u64, period: f64) -> f64 {
        let ch = self.channels as usize;
        let window = window.min(self.frames().saturating_sub(start));
        let mut re = 0.0f64;
        let mut im = 0.0f64;
        for i in 0..window {
            let x = self.data.get((start + i) as usize * ch) as f64;
            let angle = core::f64::consts::TAU * i as f64 / period;
            re += x * angle.cos();
            im += x * angle.sin();
        }
        im.atan2(re)
    }

    /// Measure the true fundamental period from the sustain loop by
    /// normalized cross-correlation at a lag of many nominal periods
    /// (long lag divides the peak-position error), parabolic-refined.
    /// Returns `None` when the material doesn't correlate with itself
    /// (unpitched noises) or the loop is too short to measure.
    /// Normalized cross-correlation between the `window` frames at
    /// `start` and the same length `lag` frames later — 1 when the
    /// waveform repeats exactly at that lag.
    fn correlation(&self, start: u64, window: u64, lag: i64) -> f64 {
        let ch = self.channels as usize;
        let sample_at = |frame: u64| self.data.get(frame as usize * ch);
        let mut dot = 0.0f64;
        let mut energy_a = 0.0f64;
        let mut energy_b = 0.0f64;
        for i in 0..window {
            let a = sample_at(start + i) as f64;
            let b = sample_at((start as i64 + lag) as u64 + i) as f64;
            dot += a * b;
            energy_a += a * a;
            energy_b += b * b;
        }
        dot / (energy_a * energy_b).sqrt().max(1e-12)
    }

    /// The best-correlating lag in `lags` (stepping by `step`) and its
    /// score, parabolically interpolated to sub-frame resolution.
    fn peak_lag(
        &self,
        start: u64,
        window: u64,
        end: u64,
        lags: std::ops::RangeInclusive<i64>,
        step: i64,
    ) -> Option<(f64, f64)> {
        let fits = |lag: i64| lag > 0 && (start as i64 + lag) as u64 + window <= end;
        let mut best: Option<(i64, f64)> = None;
        let mut lag = *lags.start();
        while lag <= *lags.end() {
            if fits(lag) {
                let value = self.correlation(start, window, lag);
                if best.is_none_or(|(_, score)| value > score) {
                    best = Some((lag, value));
                }
            }
            lag += step.max(1);
        }
        let (best_lag, best_score) = best?;
        if !(fits(best_lag - 1) && fits(best_lag + 1)) {
            return Some((best_lag as f64, best_score));
        }
        let (left, right) = (
            self.correlation(start, window, best_lag - 1),
            self.correlation(start, window, best_lag + 1),
        );
        let denominator = left - 2.0 * best_score + right;
        let offset = if denominator.abs() > 1e-12 {
            (0.5 * (left - right) / denominator).clamp(-0.5, 0.5)
        } else {
            0.0
        };
        Some((best_lag as f64 + offset, best_score))
    }

    /// Measure the fundamental period (in this sample's own frames)
    /// from the sustain loop, searching ±600 cents around `expected`
    /// frames. `None` when the sample has no loop long enough or the
    /// material doesn't repeat (unpitched noises).
    ///
    /// Staged: a coarse one-cycle scan over the whole window finds
    /// the right cycle wherever the recording actually sits — a set
    /// recorded at a′ = 415 is 6 % long of its nominal period, and a
    /// long-lag search centred on the nominal would lock onto the
    /// wrong cycle count and return garbage with confidence. Then the
    /// lag is re-measured over as many whole cycles as the loop holds
    /// (24 at most), each stage only searching the frames the previous
    /// one could still be wrong by, for sub-1e-5 relative accuracy:
    /// alignment tracks phase over hundreds of periods, and tuning
    /// wants the pitch to a fraction of a cent.
    pub fn measure_period(&self, expected: f64) -> Option<f64> {
        let (loop_start, loop_end) = self.sustain_loop?;
        let loop_len = (loop_end - loop_start) as f64;
        if expected.is_nan() || expected < 4.0 || loop_len < expected * 2.5 {
            return None;
        }
        let window = (2048.0_f64).min(loop_len / 2.0) as u64;
        let peak = |lags: std::ops::RangeInclusive<i64>, step: i64| {
            self.peak_lag(loop_start, window, loop_end, lags, step)
        };

        let spread = std::f64::consts::SQRT_2;
        let low = (expected / spread).floor().max(1.0) as i64;
        let high = (expected * spread).ceil() as i64;
        let step = (expected / 256.0).floor().max(1.0) as i64;
        let (coarse, _) = peak(low..=high, step)?;
        let (mut period, mut score) = if step > 1 {
            let centre = coarse.round() as i64;
            peak((centre - step)..=(centre + step), 1)?
        } else {
            (coarse, 0.0)
        };

        // Each further stage measures `cycles` periods at once; the
        // interpolated estimate before it is good to a fraction of a
        // frame per cycle it measured, so the search spans only that
        // residue — and never half a period, which would let a
        // neighbouring multiple of a short period (high pipes: a
        // handful of frames) tie with the right one.
        let max_cycles = (((loop_len - window as f64) / period).floor() as u32).clamp(1, 24);
        let mut measured_cycles = 1u32;
        for cycles in [4u32, max_cycles] {
            if cycles <= measured_cycles || cycles > max_cycles {
                continue;
            }
            let centre = (cycles as f64 * period).round() as i64;
            let residue = (cycles as f64 * 0.25 / measured_cycles as f64).ceil() as i64;
            let span = residue.max(2).min(((period / 2.0).floor() as i64 - 1).max(1));
            let Some((lag, value)) = peak((centre - span)..=(centre + span), 1) else {
                break;
            };
            period = lag / cycles as f64;
            score = value;
            measured_cycles = cycles;
        }
        if measured_cycles == 1 {
            score = self.correlation(loop_start, window, period.round() as i64);
        }
        (score >= 0.5).then_some(period)
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
                self.data.get(base).abs()
            } else {
                (self.data.get(base).abs() + self.data.get(base + 1).abs()) * 0.5
            };
            sum += value as f64;
        }
        (sum / window.max(1) as f64) as f32
    }

    #[inline]
    pub fn tail_decay_db_per_s(&self) -> f32 {
        self.tail_decay_db_per_s
    }

    /// Measured fundamental period in source frames, when alignment
    /// analysis has run.
    pub fn measured_period(&self) -> Option<f64> {
        self.measured_period
    }

    /// Tail log-envelope measurement over 50 ms RMS windows, skipping
    /// the first 150 ms — the drive-collapse plateau before the
    /// exponential knee (Rucz 2015, fig. 2.5b). Returns:
    /// - the least-squares decay slope (dB/s, positive = decaying;
    ///   fitted down to the measurement floor), 0 when unfittable;
    /// - the final window's level relative to the tail's loudest window
    ///   (dB, ≤ 0) — how loud the recording still is when it runs out.
    fn measure_tail(&self, tail: u64) -> (f32, f32) {
        let sr = self.sample_rate_hz as f64;
        let window = (0.05 * sr) as u64;
        let skip = (0.15 * sr) as u64;
        let start = tail + skip;
        if window == 0 || self.frames() < start + 3 * window {
            return (0.0, -120.0);
        }
        let count = ((self.frames() - start) / window).saturating_sub(1) as usize;
        if count < 3 {
            return (0.0, -120.0);
        }
        let mut window_rms = Vec::with_capacity(count);
        for k in 0..count {
            let mut acc = 0.0f64;
            for i in 0..window {
                let (l, r) = self.read((start + k as u64 * window + i) as f64);
                let v = ((l + r) * 0.5) as f64;
                acc += v * v;
            }
            window_rms.push((acc / window as f64).sqrt());
        }
        // Fit only down to 45 dB below the tail's own peak: recordings
        // carry a noise floor, and fitting into it flattens the slope
        // (a 2' pipe once "measured" 23 dB/s because the fit ran deep
        // into hiss).
        let peak = window_rms.iter().cloned().fold(0.0f64, f64::max);
        let eof_db = if peak > 1e-7 {
            let last = window_rms.last().copied().unwrap_or(0.0).max(peak * 1e-6);
            ((20.0 * (last / peak).log10()) as f32).clamp(-120.0, 0.0)
        } else {
            -120.0
        };
        let floor = (peak * 10.0f64.powf(-45.0 / 20.0)).max(1e-6);
        let mut xs = 0.0f64;
        let mut ys = 0.0f64;
        let mut xx = 0.0f64;
        let mut xy = 0.0f64;
        let mut n = 0.0f64;
        for (k, &rms) in window_rms.iter().enumerate() {
            if rms < floor {
                break;
            }
            let x = k as f64 * window as f64 / sr;
            let y = 20.0 * rms.log10();
            xs += x;
            ys += y;
            xx += x * x;
            xy += x * y;
            n += 1.0;
        }
        if n < 3.0 {
            return (0.0, eof_db);
        }
        let denominator = n * xx - xs * xs;
        if denominator.abs() < 1e-12 {
            return (0.0, eof_db);
        }
        let slope = (n * xy - xs * ys) / denominator; // dB/s, negative when decaying
        ((-slope as f32).clamp(0.0, 120.0), eof_db)
    }

    pub fn tail_reference_level(&self) -> f32 {
        self.tail_reference_level
    }

    pub fn tail_eof_level_db(&self) -> f32 {
        self.tail_eof_level_db
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
        if fundamental_hz.is_nan() || fundamental_hz <= 0.0 {
            return;
        }
        // The nominal pitch is 12-EDO bookkeeping; real pipes sit cents
        // away, and phase tracked over hundreds of periods from the
        // loop-start anchor scrambles completely with even 0.1 % period
        // error. Refine against the audio itself (long-lag
        // autocorrelation, parabolically interpolated) — sub-1e-5
        // relative accuracy, or give up on alignment when the material
        // doesn't correlate (unpitched noises).
        let nominal = self.sample_rate_hz as f64 / fundamental_hz as f64;
        let Some(period) = self.measure_period(nominal) else {
            return;
        };
        self.measured_period = Some(period);
        // The embedded-tail table additionally needs a tail to map into
        // (samples with only separate releases stop here, period saved).
        let Some(tail) = self.release_start() else {
            return;
        };
        let period_frames = period.round() as u64;
        // Correlation window: a couple of periods, floored at 128
        // frames — a single short period (high pipes: ~30 frames) is
        // far too little signal to lock phase against room noise, and a
        // mis-locked phase0 turns every release into a click that rings
        // in the recorded reverb ("pingy at the top").
        let window = (period_frames * 2).clamp(128, 600);
        let frames = self.frames();
        if period_frames < 4 || tail + period_frames + window >= frames {
            return;
        }

        // Fundamental phase via quadrature projection at the measured
        // period — NOT correlation argmax: on principal pipes with
        // strong 2nd harmonics the argmax could lock a half period off
        // (fundamental cancels, octave reinforces = a missing-
        // fundamental strike, i.e. exactly a bell).
        let theta_loop = self.quadrature_phase(loop_start, window * 4, period);
        let theta_tail = self.quadrature_phase(tail, window * 4, period);
        let offsets = (0..ALIGNMENT_BUCKETS)
            .map(|bucket| {
                let turns = (theta_tail - theta_loop) / core::f64::consts::TAU
                    + bucket as f64 / ALIGNMENT_BUCKETS as f64;
                let delta = (period * turns.rem_euclid(1.0)).round() as u64;
                (tail + delta.min(period_frames.saturating_sub(1))) as u32
            })
            .collect();
        self.release_alignment = Some(ReleaseAlignment { period, offsets });
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
    pub(crate) fn release_alignment(&self) -> Option<&ReleaseAlignment> {
        self.release_alignment.as_ref()
    }

    #[inline]
    pub fn sustain_loop(&self) -> Option<(u64, u64)> {
        self.sustain_loop
    }

    /// Register an alternate sustain loop (validated like the primary).
    pub fn add_loop(&mut self, start: u64, end: u64) -> Result<(), String> {
        if self.sustain_loop.is_none() {
            return Err("no primary loop to alternate with".into());
        }
        if start >= end || end > self.frames() {
            return Err(format!("loop {start}..{end} out of bounds"));
        }
        self.extra_loops.push((start, end));
        Ok(())
    }

    /// Total number of selectable loops (primary + alternates).
    #[inline]
    pub fn loop_count(&self) -> usize {
        self.sustain_loop.is_some() as usize + self.extra_loops.len()
    }

    /// Loop by index: 0 = primary, then alternates. Out of range falls
    /// back to the primary so RT code never needs a bounds branch.
    #[inline]
    pub fn loop_at(&self, index: usize) -> Option<(u64, u64)> {
        if index == 0 || index > self.extra_loops.len() {
            self.sustain_loop
        } else {
            Some(self.extra_loops[index - 1])
        }
    }

    /// Attach a separate recorded release (already pushed to the bank as
    /// `target_index`), selectable when the note was held at most
    /// `max_hold_ms`. Builds the cross-file phase map when this sample's
    /// period has been measured (call [`Sample::align_release`] first).
    pub fn attach_release(
        &mut self,
        target: &Sample,
        target_index: u32,
        max_hold_ms: Option<u32>,
        wave_trem: Option<bool>,
        crossfade_ms: u16,
    ) {
        let level = target.mean_abs(0, 512.min(target.frames()));
        let alignment = match (self.measured_period, self.sustain_loop) {
            (Some(period), Some((loop_start, _))) => {
                let period_frames = period.round().max(4.0) as u64;
                let window = (period_frames * 4).clamp(128, 2048);
                if target.frames() > period_frames + window {
                    // Quadrature phases (harmonic-immune; see
                    // align_release) — cross-file this time.
                    let theta_loop = self.quadrature_phase(loop_start, window, period);
                    let theta_target = target.quadrature_phase(0, window, period);
                    let offsets = (0..ALIGNMENT_BUCKETS)
                        .map(|bucket| {
                            let turns = (theta_target - theta_loop)
                                / core::f64::consts::TAU
                                + bucket as f64 / ALIGNMENT_BUCKETS as f64;
                            let delta = (period * turns.rem_euclid(1.0)).round() as u64;
                            delta.min(period_frames.saturating_sub(1)) as u32
                        })
                        .collect();
                    Some(ReleaseAlignment { period, offsets })
                } else {
                    None
                }
            }
            _ => None,
        };
        let option = ReleaseOption {
            sample: target_index,
            max_hold_ms,
            wave_trem,
            crossfade_ms,
            alignment,
            level,
        };
        // Keep sorted: bounded holds ascending, unbounded last — the RT
        // selection is then "first option whose bound covers the hold".
        let position = self
            .releases
            .iter()
            .position(|existing| match (existing.max_hold_ms, option.max_hold_ms) {
                (None, _) => true,
                (Some(_), None) => false,
                (Some(a), Some(b)) => a > b,
            })
            .unwrap_or(self.releases.len());
        self.releases.insert(position, option);
    }

    /// Separate release options, sorted for hold-time selection.
    #[inline]
    pub fn release_options(&self) -> &[ReleaseOption] {
        &self.releases
    }

    /// The release tail's first frame, if this sample has one to splice to.
    #[inline]
    pub fn release_start(&self) -> Option<u64> {
        (self.sustain_loop.is_some() && self.release_start < self.frames())
            .then_some(self.release_start)
    }

    /// Start playback at `frame` instead of 0 (ODF `AttackStart`).
    /// Clamped so the cursor can always still reach the sustain loop.
    pub fn set_attack_start(&mut self, frame: u64) {
        let ceiling = match self.sustain_loop {
            Some((start, _)) => start,
            None => self.frames().saturating_sub(1),
        };
        self.attack_start = frame.min(ceiling);
    }

    #[inline]
    pub fn attack_start(&self) -> u64 {
        self.attack_start
    }

    /// Declare the producer's key-off crossfade for the embedded tail.
    pub fn set_release_crossfade_ms(&mut self, ms: u16) {
        self.release_crossfade_ms = ms;
    }

    #[inline]
    pub fn release_crossfade_ms(&self) -> u16 {
        self.release_crossfade_ms
    }

    /// Write this sample for the server's load cache: every field the
    /// decode-and-analysis phase produced, so a cache hit skips both.
    /// Attached releases are deliberately NOT written — caching happens
    /// pre-attach (bank indices are an assembly fact, and re-attaching
    /// is cheap). Little-endian, guarded by the cache's own version tag.
    pub fn write_cache(&self, out: &mut impl std::io::Write) -> std::io::Result<()> {
        fn put_opt_u64(out: &mut impl std::io::Write, v: Option<u64>) -> std::io::Result<()> {
            match v {
                Some(v) => {
                    out.write_all(&[1])?;
                    out.write_all(&v.to_le_bytes())
                }
                None => out.write_all(&[0]),
            }
        }
        out.write_all(&self.channels.to_le_bytes())?;
        out.write_all(&self.sample_rate_hz.to_le_bytes())?;
        put_opt_u64(out, self.sustain_loop.map(|(s, _)| s))?;
        put_opt_u64(out, self.sustain_loop.map(|(_, e)| e))?;
        out.write_all(&(self.extra_loops.len() as u64).to_le_bytes())?;
        for &(start, end) in &self.extra_loops {
            out.write_all(&start.to_le_bytes())?;
            out.write_all(&end.to_le_bytes())?;
        }
        out.write_all(&self.release_start.to_le_bytes())?;
        match &self.release_alignment {
            Some(alignment) => {
                out.write_all(&[1])?;
                out.write_all(&alignment.period.to_le_bytes())?;
                out.write_all(&(alignment.offsets.len() as u64).to_le_bytes())?;
                for &offset in &alignment.offsets {
                    out.write_all(&offset.to_le_bytes())?;
                }
            }
            None => out.write_all(&[0])?,
        }
        match self.measured_period {
            Some(period) => {
                out.write_all(&[1])?;
                out.write_all(&period.to_le_bytes())?;
            }
            None => out.write_all(&[0])?,
        }
        out.write_all(&self.tail_reference_level.to_le_bytes())?;
        out.write_all(&self.tail_decay_db_per_s.to_le_bytes())?;
        out.write_all(&self.tail_eof_level_db.to_le_bytes())?;
        out.write_all(&self.attack_start.to_le_bytes())?;
        out.write_all(&self.release_crossfade_ms.to_le_bytes())?;
        match &self.data {
            SampleData::F32(data) => {
                out.write_all(&[0])?;
                out.write_all(&(data.len() as u64).to_le_bytes())?;
                // SAFETY: f32 is plain-old-data; byte view for bulk
                // I/O on the little-endian platforms we build for.
                let bytes = unsafe {
                    std::slice::from_raw_parts(data.as_ptr() as *const u8, data.len() * 4)
                };
                out.write_all(bytes)?;
            }
            SampleData::I16(data) => {
                out.write_all(&[1])?;
                out.write_all(&(data.len() as u64).to_le_bytes())?;
                // SAFETY: as above, i16 POD byte view.
                let bytes = unsafe {
                    std::slice::from_raw_parts(data.as_ptr() as *const u8, data.len() * 2)
                };
                out.write_all(bytes)?;
            }
        }
        Ok(())
    }

    /// Rebuild a sample [`Sample::write_cache`] wrote. Any structural
    /// surprise is an error — the caller treats it as a cache miss.
    pub fn read_cache(input: &mut impl std::io::Read) -> std::io::Result<Sample> {
        use std::io::{Error, ErrorKind, Read};
        fn get<const N: usize>(input: &mut impl Read) -> std::io::Result<[u8; N]> {
            let mut bytes = [0u8; N];
            input.read_exact(&mut bytes)?;
            Ok(bytes)
        }
        fn get_u64(input: &mut impl Read) -> std::io::Result<u64> {
            Ok(u64::from_le_bytes(get::<8>(input)?))
        }
        fn get_len(input: &mut impl Read, cap: u64) -> std::io::Result<usize> {
            let len = get_u64(input)?;
            if len > cap {
                return Err(Error::new(ErrorKind::InvalidData, "cache length absurd"));
            }
            Ok(len as usize)
        }
        fn get_opt_u64(input: &mut impl Read) -> std::io::Result<Option<u64>> {
            Ok(match get::<1>(input)?[0] {
                0 => None,
                _ => Some(get_u64(input)?),
            })
        }
        let channels = u16::from_le_bytes(get::<2>(input)?);
        let sample_rate_hz = f32::from_le_bytes(get::<4>(input)?);
        let sustain_loop = match (get_opt_u64(input)?, get_opt_u64(input)?) {
            (Some(start), Some(end)) => Some((start, end)),
            _ => None,
        };
        let extra_count = get_len(input, 1 << 16)?;
        let mut extra_loops = Vec::with_capacity(extra_count);
        for _ in 0..extra_count {
            extra_loops.push((get_u64(input)?, get_u64(input)?));
        }
        let release_start = get_u64(input)?;
        let release_alignment = match get::<1>(input)?[0] {
            0 => None,
            _ => {
                let period = f64::from_le_bytes(get::<8>(input)?);
                let count = get_len(input, 1 << 16)?;
                let mut offsets = Vec::with_capacity(count);
                for _ in 0..count {
                    offsets.push(u32::from_le_bytes(get::<4>(input)?));
                }
                Some(ReleaseAlignment { period, offsets })
            }
        };
        let measured_period = match get::<1>(input)?[0] {
            0 => None,
            _ => Some(f64::from_le_bytes(get::<8>(input)?)),
        };
        let tail_reference_level = f32::from_le_bytes(get::<4>(input)?);
        let tail_decay_db_per_s = f32::from_le_bytes(get::<4>(input)?);
        let tail_eof_level_db = f32::from_le_bytes(get::<4>(input)?);
        let attack_start = get_u64(input)?;
        let release_crossfade_ms = u16::from_le_bytes(get::<2>(input)?);
        let tag = get::<1>(input)?[0];
        let len = get_len(input, 1 << 33)?;
        let data = match tag {
            0 => {
                let mut data = vec![0f32; len];
                // SAFETY: POD byte view, as in write_cache.
                let bytes = unsafe {
                    std::slice::from_raw_parts_mut(data.as_mut_ptr() as *mut u8, len * 4)
                };
                input.read_exact(bytes)?;
                SampleData::F32(data)
            }
            1 => {
                let mut data = vec![0i16; len];
                // SAFETY: POD byte view, as in write_cache.
                let bytes = unsafe {
                    std::slice::from_raw_parts_mut(data.as_mut_ptr() as *mut u8, len * 2)
                };
                input.read_exact(bytes)?;
                SampleData::I16(data)
            }
            _ => return Err(Error::new(ErrorKind::InvalidData, "unknown data tag")),
        };
        if channels == 0 || data.len() % channels as usize != 0 {
            return Err(Error::new(ErrorKind::InvalidData, "cache shape invalid"));
        }
        Ok(Sample {
            data,
            channels,
            sample_rate_hz,
            sustain_loop,
            extra_loops,
            release_start,
            release_alignment,
            measured_period,
            releases: Vec::new(),
            tail_reference_level,
            tail_decay_db_per_s,
            tail_eof_level_db,
            attack_start,
            release_crossfade_ms,
        })
    }

    /// Resident interleaved data + channel count, for the sinc reader.
    #[inline]
    pub(crate) fn raw(&self) -> (&SampleData, u16) {
        (&self.data, self.channels)
    }

    /// Re-quantize the resident audio to 16 bits (see [`SampleData`]).
    /// Call after analysis (period refinement, tail measurement, phase
    /// maps) so those keep the full decode precision; playback then
    /// dequantizes per read.
    pub fn quantize_i16(&mut self) {
        if let SampleData::F32(data) = &self.data {
            self.data = SampleData::I16(
                data.iter()
                    .map(|&value| (value * 32768.0).round().clamp(-32768.0, 32767.0) as i16)
                    .collect(),
            );
        }
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
        let at = |i: usize| self.data.get(i);
        let left = at(a) + (at(b) - at(a)) * fraction;
        if ch == 1 {
            (left, left)
        } else {
            let right = at(a + 1) + (at(b + 1) - at(a + 1)) * fraction;
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

    /// Touch every page of sample data so first-play page faults never
    /// land inside the audio callback (Linux maps heap lazily). Returns
    /// a checksum so the traversal can't be optimized away.
    pub fn pre_fault(&self) -> f32 {
        let mut checksum = 0.0f32;
        for sample in &self.samples {
            // One touch per 4 KiB page.
            match &sample.data {
                SampleData::F32(data) => {
                    for value in data.iter().step_by(1024) {
                        checksum += *value;
                    }
                }
                SampleData::I16(data) => {
                    for value in data.iter().step_by(2048) {
                        checksum += f32::from(*value) * I16_SCALE;
                    }
                }
            }
        }
        checksum
    }

    /// Total decoded audio held, in bytes.
    pub fn resident_bytes(&self) -> usize {
        self.samples.iter().map(|s| s.data.bytes()).sum()
    }
}

#[cfg(test)]
mod period_tests {
    use super::Sample;

    /// A looped tone at `period` frames (48 kHz), with the harmonics
    /// given as (number, amplitude), plus white noise at `noise`.
    fn tone(period: f64, harmonics: &[(u32, f64)], noise: f64) -> Sample {
        let frames = (period * 200.0).ceil() as usize + 4096;
        let mut seed = 0x2545_F491_4F6C_DD1Du64;
        let mut rand = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed >> 11) as f64 / (1u64 << 53) as f64 * 2.0 - 1.0
        };
        let data: Vec<f32> = (0..frames)
            .map(|n| {
                let phase = std::f64::consts::TAU * n as f64 / period;
                let tone: f64 = harmonics
                    .iter()
                    .map(|&(h, amplitude)| amplitude * (phase * h as f64).sin())
                    .sum();
                (tone + noise * rand()) as f32
            })
            .collect();
        let loop_end = frames as u64 - 1024;
        Sample::new(data, 1, 48000.0, Some((1024, loop_end)), loop_end).expect("sample")
    }

    fn cents(measured: f64, truth: f64) -> f64 {
        1200.0 * (truth / measured).log2()
    }

    /// The measurement finds the recording's own period wherever it
    /// sits within ±600 cents of the expected one — low pipes, high
    /// pipes, strong upper harmonics — to a fraction of a cent.
    #[test]
    fn measures_the_period_wherever_the_recording_sits() {
        let ratio = |cents: f64| (cents / 1200.0).exp2();
        let cases: [(f64, f64, &[(u32, f64)]); 6] = [
            (109.09, -100.0, &[(1, 1.0)]),
            (109.09, 0.0, &[(1, 1.0), (2, 0.9), (3, 0.5)]),
            (1468.0, -100.0, &[(1, 1.0), (2, 0.3), (3, 0.6)]),
            (12.0, 450.0, &[(1, 1.0), (2, 0.4)]),
            (12.0, -550.0, &[(1, 1.0)]),
            (300.0, 590.0, &[(1, 0.5), (2, 1.0), (4, 0.7)]),
        ];
        for (expected, offset, harmonics) in cases {
            let truth = expected * ratio(-offset);
            let sample = tone(truth, harmonics, 0.02);
            let measured = sample
                .measure_period(expected)
                .unwrap_or_else(|| panic!("no period for {expected} @ {offset:+} cents"));
            let error = cents(measured, truth);
            assert!(
                error.abs() < 0.3,
                "{expected} frames @ {offset:+} cents: measured {measured}, off by {error:.3} cents"
            );
        }
    }

    #[test]
    fn noise_has_no_period() {
        let sample = tone(200.0, &[], 1.0);
        assert_eq!(sample.measure_period(200.0), None);
    }

    /// Release alignment now stores the measured period even when the
    /// recording is well off its nominal pitch.
    #[test]
    fn alignment_measures_an_off_pitch_recording() {
        let truth = 115.66; // a′ = 415 at 48 kHz
        let mut sample = tone(truth, &[(1, 1.0), (2, 0.5)], 0.01);
        sample.align_release(48000.0 / 109.09);
        let period = sample.measured_period().expect("measured");
        assert!(cents(period, truth).abs() < 0.3, "{period} vs {truth}");
    }
}
