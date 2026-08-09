//! The real-time audio core.
//!
//! Invariants for everything in this crate's audio path:
//! - never allocate, lock, or perform I/O on the audio thread
//! - control-plane communication via lock-free queues only
//! - sample data is immutable and pre-decoded ([`bank::SampleBank`]),
//!   shared with the audio thread behind an `Arc` taken at construction
//!
//! The engine is a pure library — buffers in, buffers out. Device
//! ownership lives in `aristide-server`.
//!
//! M3 state: a fixed voice pool playing either sampled pipes (attack →
//! sustain loop → spliced release tail) or the M1 additive test tone.
//! The engine has no notion of keys-as-pitches or of the organ model:
//! the control side resolves keys → pipes → [`Command::StartVoice`]
//! with a sample index, playback rate, and gain. That keeps microtonal
//! mapping, registration, and couplers out of the RT core entirely.

pub mod bank;
pub mod resample;
pub mod wind;

use std::sync::Arc;

use bank::{Sample, SampleBank};
use resample::SincTable;
use rtrb::{Consumer, Producer, RingBuffer};
use wind::{WindGroup, WindParams, MAX_WIND_GROUPS};

pub const MAX_VOICES: usize = 2048;
const COMMAND_QUEUE_CAPACITY: usize = 8192;

const TONE_ATTACK_SECONDS: f32 = 0.006;
const TONE_RELEASE_SECONDS: f32 = 0.09;
const TONE_GAIN: f32 = 0.08;
/// Loop-position → release-tail splice length. Phase-aligned release
/// selection (M4) will replace this fixed equal-power-ish ramp.
const RELEASE_CROSSFADE_SECONDS: f32 = 0.03;
/// Emergency fade for voices stopped without a release tail to go to.
const KILL_FADE_SECONDS: f32 = 0.015;

const DEFAULT_MASTER_GAIN: f32 = 0.35;

/// Principal-chorus-flavoured partials: 8', 4', 2 2/3', 2', 1 1/3'.
const HARMONICS: [(f32, f32); 5] = [
    (1.0, 0.50),
    (2.0, 0.24),
    (3.0, 0.09),
    (4.0, 0.12),
    (6.0, 0.04),
];

#[derive(Debug, Clone, Copy)]
pub enum Command {
    /// Start a sampled voice. `rate` is source frames per output frame
    /// (sample-rate ratio × pitch adjustments), `gain` is linear.
    /// `group` is the wind group the voice draws from and `wind_weight`
    /// how much it draws (0 = draws nothing, e.g. action noises).
    StartVoice {
        handle: u64,
        sample: u32,
        rate: f32,
        gain: f32,
        group: u8,
        wind_weight: f32,
    },
    /// Reconfigure one wind group's supply model.
    SetWind { group: u8, params: WindParams },
    /// Release the voice started with `handle`. Loop-less (percussive)
    /// voices ignore this and play to their end.
    StopVoice { handle: u64 },
    /// Start/stop the built-in additive test tone (no-set mode).
    NoteOn { key: u8, freq_hz: f32 },
    NoteOff { key: u8 },
    /// Fade out every sounding voice.
    AllNotesOff,
    SetMasterGain { linear: f32 },
}

/// Control-plane side of the engine. Not RT-constrained.
pub struct EngineHandle {
    commands: Producer<Command>,
}

impl EngineHandle {
    /// Returns `false` if the queue was full and the command dropped.
    pub fn send(&mut self, command: Command) -> bool {
        self.commands.push(command).is_ok()
    }
}

#[derive(Clone, Copy, Default, PartialEq)]
enum ToneStage {
    #[default]
    Idle,
    Attack,
    Sustain,
    Release,
}

#[derive(Clone, Copy, Default)]
struct ToneVoice {
    key: u8,
    phase: f32,
    phase_increment: f32,
    envelope: f32,
    stage: ToneStage,
}

impl ToneVoice {
    #[inline]
    fn tick(&mut self, attack_step: f32, release_step: f32) -> f32 {
        match self.stage {
            ToneStage::Idle => return 0.0,
            ToneStage::Attack => {
                self.envelope += attack_step;
                if self.envelope >= 1.0 {
                    self.envelope = 1.0;
                    self.stage = ToneStage::Sustain;
                }
            }
            ToneStage::Sustain => {}
            ToneStage::Release => {
                self.envelope -= release_step;
                if self.envelope <= 0.0 {
                    self.envelope = 0.0;
                    self.stage = ToneStage::Idle;
                    return 0.0;
                }
            }
        }
        let mut sample = 0.0;
        for (multiple, amplitude) in HARMONICS {
            sample += amplitude * (core::f32::consts::TAU * self.phase * multiple).sin();
        }
        self.phase += self.phase_increment;
        if self.phase >= 1.0 {
            self.phase -= 1.0;
        }
        sample * self.envelope
    }
}

#[derive(Clone, Copy, PartialEq)]
enum SamplePhase {
    /// Attack and sustain loop, key held.
    Held,
    /// Ramping from the loop position onto the release tail.
    Crossfade,
    /// Playing the release tail out.
    Tail,
    /// Emergency amplitude ramp (no tail to go to / AllNotesOff).
    FadeOut,
}

#[derive(Clone, Copy)]
struct SampledVoice {
    handle: u64,
    sample: u32,
    /// Fractional frame cursor into the sample.
    position: f64,
    /// Second cursor, into the release tail, during [`SamplePhase::Crossfade`].
    release_position: f64,
    /// Source frames advanced per output frame (before wind modulation).
    rate: f64,
    gain: f32,
    /// Crossfade progress 0→1.
    fade: f32,
    /// FadeOut amplitude 1→0.
    amplitude: f32,
    /// Wind group index (pre-clamped to `MAX_WIND_GROUPS`).
    group: u8,
    /// How much wind this voice draws while sounding.
    wind_weight: f32,
    /// Output frames since the voice started — drives the wind model's
    /// pallet-opening attack boost.
    age_frames: u32,
    phase: SamplePhase,
}

impl SampledVoice {
    /// Render one frame and advance. Returns `None` when the voice ends.
    /// End-of-data checks happen on entry so every read frame is emitted.
    #[inline]
    fn tick(
        &mut self,
        sample: &Sample,
        table: &SincTable,
        rate_scale: f64,
        crossfade_step: f32,
        kill_step: f32,
    ) -> Option<(f32, f32)> {
        let rate = self.rate * rate_scale;
        let last = (sample.frames() - 1) as f64;
        let looping = sample.sustain_loop().is_some();
        let ended = match self.phase {
            SamplePhase::Held => !looping && self.position >= last,
            SamplePhase::Crossfade => self.release_position >= last,
            SamplePhase::Tail => self.position >= last,
            SamplePhase::FadeOut => self.amplitude <= 0.0 || (!looping && self.position >= last),
        };
        if ended {
            return None;
        }

        // Cursors still circling the sustain loop wrap their kernel taps
        // across the seam; tail reads clamp at the sample edges.
        let seam = if self.phase == SamplePhase::Tail {
            None
        } else {
            sample.sustain_loop()
        };
        let (mut left, mut right) = table.read(sample, self.position, seam);
        let mut advance_position = true;
        match self.phase {
            SamplePhase::Held | SamplePhase::Tail => {}
            SamplePhase::Crossfade => {
                let (tail_l, tail_r) = table.read(sample, self.release_position, None);
                left += (tail_l - left) * self.fade;
                right += (tail_r - right) * self.fade;
                self.fade += crossfade_step;
                self.release_position += rate;
                if self.fade >= 1.0 {
                    // Hand the (already advanced) tail cursor over.
                    self.position = self.release_position;
                    self.phase = SamplePhase::Tail;
                    advance_position = false;
                }
            }
            SamplePhase::FadeOut => {
                left *= self.amplitude;
                right *= self.amplitude;
                self.amplitude -= kill_step;
            }
        }

        if advance_position {
            self.position += rate;
            // Only cursors still circling the sustain loop wrap; a Tail
            // cursor has left it for the release material.
            if self.phase != SamplePhase::Tail {
                if let Some((start, end)) = sample.sustain_loop() {
                    while self.position >= end as f64 {
                        self.position -= (end - start) as f64;
                    }
                }
            }
        }
        Some((left * self.gain, right * self.gain))
    }

    /// Key released: splice to the release tail if there is one.
    fn release(&mut self, sample: &Sample) {
        match self.phase {
            SamplePhase::Held | SamplePhase::Crossfade => {}
            _ => return,
        }
        match sample.release_start() {
            Some(tail) if self.phase == SamplePhase::Held => {
                self.release_position = match (sample.release_alignment(), sample.sustain_loop()) {
                    (Some(alignment), Some((loop_start, _))) => {
                        alignment.target(self.position, loop_start) as f64
                    }
                    _ => tail as f64,
                };
                self.fade = 0.0;
                self.phase = SamplePhase::Crossfade;
            }
            Some(_) => {} // already crossfading
            None => {
                if sample.sustain_loop().is_some() {
                    self.amplitude = 1.0;
                    self.phase = SamplePhase::FadeOut;
                }
                // Loop-less (percussive) samples play to the end.
            }
        }
    }
}

#[derive(Clone, Copy, Default)]
enum Voice {
    #[default]
    Idle,
    Tone(ToneVoice),
    Sampled(SampledVoice),
}

/// RT side. Owned by the audio callback; every method here upholds the
/// no-alloc/no-lock/no-I/O invariants.
pub struct Engine {
    sample_rate: f32,
    commands: Consumer<Command>,
    bank: Arc<SampleBank>,
    sinc: SincTable,
    voices: Box<[Voice]>,
    wind: [WindGroup; MAX_WIND_GROUPS],
    master_gain: f32,
    tone_attack_step: f32,
    tone_release_step: f32,
    crossfade_step: f32,
    kill_step: f32,
}

impl Engine {
    pub fn new(sample_rate: f32, bank: Arc<SampleBank>) -> (Engine, EngineHandle) {
        let (producer, consumer) = RingBuffer::new(COMMAND_QUEUE_CAPACITY);
        let engine = Engine {
            sample_rate,
            commands: consumer,
            bank,
            sinc: SincTable::new(),
            voices: vec![Voice::Idle; MAX_VOICES].into_boxed_slice(),
            wind: [WindGroup::default(); MAX_WIND_GROUPS],
            master_gain: DEFAULT_MASTER_GAIN,
            tone_attack_step: 1.0 / (TONE_ATTACK_SECONDS * sample_rate),
            tone_release_step: 1.0 / (TONE_RELEASE_SECONDS * sample_rate),
            crossfade_step: 1.0 / (RELEASE_CROSSFADE_SECONDS * sample_rate),
            kill_step: 1.0 / (KILL_FADE_SECONDS * sample_rate),
        };
        (engine, EngineHandle { commands: producer })
    }

    /// Render one interleaved buffer, mixing every active voice.
    /// Stereo material lands in the first two channels (summed to one
    /// on mono outputs); channel routing proper arrives with M6.
    pub fn process(&mut self, buffer: &mut [f32], channels: usize) {
        while let Ok(command) = self.commands.pop() {
            self.apply(command);
        }

        buffer.fill(0.0);
        let channels = channels.max(1);
        let frames = buffer.len() / channels;
        let master = self.master_gain;

        // Wind: one regulator step per block. Demand sums the wind
        // weight of everything sounding on each chest, with young
        // voices boosted (the pallet-opening gulp).
        let mut demand = [0.0f32; MAX_WIND_GROUPS];
        for voice in self.voices.iter() {
            if let Voice::Sampled(sampled) = voice {
                let params = self.wind[sampled.group as usize].params();
                let attack_frames = params.attack_ms * 0.001 * self.sample_rate;
                let boost = if (sampled.age_frames as f32) < attack_frames {
                    params.attack_boost * (1.0 - sampled.age_frames as f32 / attack_frames)
                } else {
                    0.0
                };
                demand[sampled.group as usize] += sampled.wind_weight * (1.0 + boost);
            }
        }
        let dt = frames as f32 / self.sample_rate;
        for (group, wind) in self.wind.iter_mut().enumerate() {
            wind.step(demand[group], dt);
        }

        // Split borrows: voices mutably, bank/read-only params shared.
        let Engine {
            voices,
            bank,
            sinc,
            wind,
            tone_attack_step,
            tone_release_step,
            crossfade_step,
            kill_step,
            ..
        } = self;

        for voice in voices.iter_mut() {
            match voice {
                Voice::Idle => {}
                Voice::Tone(tone) => {
                    for frame in 0..frames {
                        let value = tone.tick(*tone_attack_step, *tone_release_step);
                        if tone.stage == ToneStage::Idle {
                            *voice = Voice::Idle;
                            break;
                        }
                        mix_frame(
                            &mut buffer[frame * channels..],
                            channels,
                            value * TONE_GAIN * master,
                            value * TONE_GAIN * master,
                        );
                    }
                }
                Voice::Sampled(sampled) => {
                    let Some(sample) = bank.get(sampled.sample) else {
                        *voice = Voice::Idle;
                        continue;
                    };
                    let chest = &wind[sampled.group as usize];
                    let rate_scale = chest.rate_factor() as f64;
                    let gain = master * chest.gain_factor();
                    sampled.age_frames = sampled.age_frames.saturating_add(frames as u32);
                    for frame in 0..frames {
                        match sampled.tick(sample, sinc, rate_scale, *crossfade_step, *kill_step) {
                            Some((left, right)) => mix_frame(
                                &mut buffer[frame * channels..],
                                channels,
                                left * gain,
                                right * gain,
                            ),
                            None => {
                                *voice = Voice::Idle;
                                break;
                            }
                        }
                    }
                }
            }
        }
    }

    fn apply(&mut self, command: Command) {
        match command {
            Command::StartVoice {
                handle,
                sample,
                rate,
                gain,
                group,
                wind_weight,
            } => {
                if self.bank.get(sample).is_none() || !(rate > 0.0) {
                    return;
                }
                if let Some(slot) = self.free_slot() {
                    self.voices[slot] = Voice::Sampled(SampledVoice {
                        handle,
                        sample,
                        position: 0.0,
                        release_position: 0.0,
                        rate: rate as f64,
                        gain,
                        fade: 0.0,
                        amplitude: 1.0,
                        group: group.min(MAX_WIND_GROUPS as u8 - 1),
                        wind_weight: wind_weight.max(0.0),
                        age_frames: 0,
                        phase: SamplePhase::Held,
                    });
                }
            }
            Command::SetWind { group, params } => {
                if let Some(wind) = self.wind.get_mut(group as usize) {
                    wind.set_params(params);
                }
            }
            Command::StopVoice { handle } => {
                for voice in self.voices.iter_mut() {
                    if let Voice::Sampled(sampled) = voice {
                        if sampled.handle == handle {
                            if let Some(sample) = self.bank.get(sampled.sample) {
                                sampled.release(sample);
                            }
                        }
                    }
                }
            }
            Command::NoteOn { key, freq_hz } => {
                if let Some(slot) = self.free_slot() {
                    self.voices[slot] = Voice::Tone(ToneVoice {
                        key,
                        phase: 0.0,
                        phase_increment: freq_hz / self.sample_rate,
                        envelope: 0.0,
                        stage: ToneStage::Attack,
                    });
                }
            }
            Command::NoteOff { key } => {
                for voice in self.voices.iter_mut() {
                    if let Voice::Tone(tone) = voice {
                        if tone.key == key
                            && (tone.stage == ToneStage::Attack || tone.stage == ToneStage::Sustain)
                        {
                            tone.stage = ToneStage::Release;
                        }
                    }
                }
            }
            Command::AllNotesOff => {
                for voice in self.voices.iter_mut() {
                    match voice {
                        Voice::Idle => {}
                        Voice::Tone(tone) => {
                            if tone.stage != ToneStage::Idle {
                                tone.stage = ToneStage::Release;
                            }
                        }
                        Voice::Sampled(sampled) => {
                            if sampled.phase != SamplePhase::FadeOut {
                                sampled.amplitude = 1.0;
                                sampled.phase = SamplePhase::FadeOut;
                            }
                        }
                    }
                }
            }
            Command::SetMasterGain { linear } => {
                if linear.is_finite() && (0.0..=4.0).contains(&linear) {
                    self.master_gain = linear;
                }
            }
        }
    }

    /// Current pressure of a wind group (diagnostics and tests).
    pub fn wind_pressure(&self, group: usize) -> f32 {
        self.wind
            .get(group)
            .map(|w| w.pressure())
            .unwrap_or(1.0)
    }

    /// An idle slot, or one already on its way out (tail/fade) to steal.
    fn free_slot(&self) -> Option<usize> {
        let mut dying = None;
        for (index, voice) in self.voices.iter().enumerate() {
            match voice {
                Voice::Idle => return Some(index),
                Voice::Sampled(s)
                    if dying.is_none()
                        && matches!(s.phase, SamplePhase::Tail | SamplePhase::FadeOut) =>
                {
                    dying = Some(index)
                }
                _ => {}
            }
        }
        dying
    }
}

#[inline]
fn mix_frame(frame: &mut [f32], channels: usize, left: f32, right: f32) {
    if channels == 1 {
        frame[0] += (left + right) * 0.5;
    } else {
        frame[0] += left;
        frame[1] += right;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 100-frame mono ramp 0..1, loop frames 20..=59, release tail at 60.
    fn test_bank() -> Arc<SampleBank> {
        let data: Vec<f32> = (0..100).map(|i| i as f32 / 100.0).collect();
        let sample = Sample::new(data, 1, 100.0, Some((20, 60)), 60).expect("valid");
        let mut bank = SampleBank::default();
        bank.push(sample);
        Arc::new(bank)
    }

    fn render(engine: &mut Engine, frames: usize) -> Vec<f32> {
        let mut buffer = vec![0.0f32; frames * 2];
        engine.process(&mut buffer, 2);
        buffer
    }

    #[test]
    fn sampled_voice_plays_and_loops() {
        let (mut engine, mut handle) = Engine::new(100.0, test_bank());
        handle.send(Command::StartVoice {
            handle: 1,
            sample: 0,
            rate: 1.0,
            gain: 1.0,
            group: 0,
            wind_weight: 0.0,
        });
        // 200 frames from a 100-frame sample: only survivable by looping.
        let out = render(&mut engine, 200);
        assert!(out[10] > 0.0, "audio should be flowing");
        let late = &out[300..400];
        assert!(
            late.iter().any(|&v| v != 0.0),
            "loop should keep the voice alive past the sample end"
        );
        // Looping stays within loop bounds: values in (0.2*g, 0.6*g).
        let master = DEFAULT_MASTER_GAIN;
        for (i, &v) in out.iter().enumerate().skip(140) {
            assert!(
                v >= 0.19 * master && v <= 0.61 * master,
                "frame {i}: {v} escaped the sustain loop"
            );
        }
    }

    #[test]
    fn released_voice_splices_to_tail_and_ends() {
        let (mut engine, mut handle) = Engine::new(100.0, test_bank());
        handle.send(Command::StartVoice {
            handle: 7,
            sample: 0,
            rate: 1.0,
            gain: 1.0,
            group: 0,
            wind_weight: 0.0,
        });
        render(&mut engine, 10);
        handle.send(Command::StopVoice { handle: 7 });
        // Tail is 40 frames (60..100) + crossfade 3 frames at sr=100.
        render(&mut engine, 30);
        let out = render(&mut engine, 100);
        let silent_after = &out[60 * 2..];
        assert!(
            silent_after.iter().all(|&v| v == 0.0),
            "voice should have ended after the release tail"
        );
    }

    #[test]
    fn percussive_sample_ignores_stop_and_ends_itself() {
        let data: Vec<f32> = (0..50).map(|_| 0.5).collect();
        let sample = Sample::new(data, 1, 100.0, None, 0).expect("valid");
        let mut bank = SampleBank::default();
        bank.push(sample);
        let (mut engine, mut handle) = Engine::new(100.0, Arc::new(bank));
        handle.send(Command::StartVoice {
            handle: 1,
            sample: 0,
            rate: 1.0,
            gain: 1.0,
            group: 0,
            wind_weight: 0.0,
        });
        handle.send(Command::StopVoice { handle: 1 });
        let out = render(&mut engine, 60);
        assert!(out[20] != 0.0, "percussive sample should keep playing");
        let out = render(&mut engine, 20);
        assert!(out.iter().all(|&v| v == 0.0), "and then end on its own");
    }

    #[test]
    fn tone_voice_still_works() {
        let (mut engine, mut handle) = Engine::new(48000.0, Arc::new(SampleBank::default()));
        handle.send(Command::NoteOn {
            key: 69,
            freq_hz: 440.0,
        });
        let out = render(&mut engine, 512);
        assert!(out.iter().any(|&v| v != 0.0));
        handle.send(Command::NoteOff { key: 69 });
        render(&mut engine, 48000);
        let out = render(&mut engine, 512);
        assert!(out.iter().all(|&v| v == 0.0));
    }

    /// A synthetic pipe: continuous sine through attack, loop, and a
    /// gently decaying release tail, so waveform phase is knowable
    /// everywhere. `aligned` controls whether the alignment table is
    /// built (false = naive fixed splice, the M3 behaviour).
    fn sine_pipe_bank(period: usize, aligned: bool) -> Arc<SampleBank> {
        let omega = std::f64::consts::TAU / period as f64;
        let loop_start = period * 4;
        let loop_end = period * 12;
        let frames = period * 24;
        let data: Vec<f32> = (0..frames)
            .map(|n| {
                let envelope = if n >= loop_end {
                    1.0 - 0.5 * (n - loop_end) as f64 / (frames - loop_end) as f64
                } else {
                    1.0
                };
                (envelope * (omega * n as f64).sin()) as f32
            })
            .collect();
        let mut sample = Sample::new(
            data,
            1,
            48000.0,
            Some((loop_start as u64, loop_end as u64)),
            loop_end as u64,
        )
        .expect("valid");
        if aligned {
            sample.align_release(48000.0 / period as f32);
            assert!(sample.release_alignment().is_some(), "alignment built");
        }
        let mut bank = SampleBank::default();
        bank.push(sample);
        Arc::new(bank)
    }

    /// A misaligned splice doesn't click through a 30 ms crossfade — it
    /// *cancels*: output amplitude dips toward zero mid-fade. Measure
    /// the worst period-length RMS during the crossfade relative to the
    /// held level.
    fn release_dip_ratio(bank: Arc<SampleBank>, stop_after: usize, period: usize) -> f32 {
        let (mut engine, mut handle) = Engine::new(48000.0, bank);
        handle.send(Command::StartVoice {
            handle: 1,
            sample: 0,
            rate: 1.0,
            gain: 1.0,
            group: 0,
            wind_weight: 0.0,
        });
        let mut buffer = vec![0.0f32; stop_after * 2];
        engine.process(&mut buffer, 2);
        let held: Vec<f32> = buffer.chunks(2).map(|f| f[0]).collect();
        let steady = rms(&held[held.len() - period..]);

        handle.send(Command::StopVoice { handle: 1 });
        // 30 ms crossfade = 1440 frames at 48 kHz.
        let mut buffer = vec![0.0f32; 1500 * 2];
        engine.process(&mut buffer, 2);
        let fade: Vec<f32> = buffer.chunks(2).map(|f| f[0]).collect();
        let mut worst = f32::MAX;
        let mut start = 0;
        while start + period <= fade.len() {
            worst = worst.min(rms(&fade[start..start + period]));
            start += period / 8;
        }
        worst / steady
    }

    fn rms(window: &[f32]) -> f32 {
        (window.iter().map(|v| v * v).sum::<f32>() / window.len() as f32).sqrt()
    }

    #[test]
    fn aligned_release_splice_never_cancels() {
        let period = 480; // 100 Hz at 48 kHz
        // Stop moments spread across the waveform cycle, including the
        // adversarial anti-phase one (2640 = 5.5 periods).
        for stop_after in [2400, 2520, 2580, 2640, 2700, 2763, 2885] {
            let aligned = release_dip_ratio(sine_pipe_bank(period, true), stop_after, period);
            assert!(
                aligned > 0.7,
                "stop at {stop_after}: aligned splice dipped to {aligned:.2} of held level"
            );
        }
        // And the naive splice really is the artifact we claim to fix:
        // anti-phase stop cancels hard.
        let naive = release_dip_ratio(sine_pipe_bank(period, false), 2640, period);
        let aligned = release_dip_ratio(sine_pipe_bank(period, true), 2640, period);
        println!("anti-phase stop: aligned holds {aligned:.2} of level, naive dips to {naive:.2}");
        assert!(
            naive < 0.45,
            "naive anti-phase splice should cancel (got {naive:.2}) — is this test still valid?"
        );
    }

    #[test]
    fn alignment_targets_match_waveform_phase() {
        let period = 480usize;
        let bank = sine_pipe_bank(period, true);
        let sample = bank.get(0).expect("sample");
        let alignment = sample.release_alignment().expect("alignment");
        let (loop_start, _) = sample.sustain_loop().expect("loop");
        for probe in 0..32 {
            let position = loop_start as f64 + probe as f64 * 37.3;
            let target = alignment.target(position, loop_start);
            let source_phase = (position / period as f64).fract();
            let target_phase = (target as f64 / period as f64).fract();
            let mut delta = (source_phase - target_phase).abs();
            delta = delta.min(1.0 - delta);
            assert!(
                delta < 1.5 / bank::ALIGNMENT_BUCKETS as f64 + 0.01,
                "position {position}: source phase {source_phase:.3} vs target {target_phase:.3}"
            );
        }
    }

    /// Mean rising-zero-crossing period of channel 0, sub-sample refined.
    fn measured_period(buffer: &[f32]) -> f64 {
        let mono: Vec<f32> = buffer.chunks(2).map(|f| f[0]).collect();
        let mut crossings = Vec::new();
        for i in 1..mono.len() {
            if mono[i - 1] < 0.0 && mono[i] >= 0.0 {
                let t = (i - 1) as f64 + (-mono[i - 1] as f64) / ((mono[i] - mono[i - 1]) as f64);
                crossings.push(t);
            }
        }
        assert!(crossings.len() > 3, "not enough periods to measure");
        (crossings.last().unwrap() - crossings[0]) / (crossings.len() - 1) as f64
    }

    #[test]
    fn wind_pressure_sags_pitch_under_load() {
        let period = 480usize;

        let run = |phantom_voices: usize| -> f64 {
            let (mut engine, mut handle) = Engine::new(48000.0, sine_pipe_bank(period, true));
            // Phantoms draw wind but are silent: gain 0, weight 1.
            for i in 0..phantom_voices {
                handle.send(Command::StartVoice {
                    handle: 100 + i as u64,
                    sample: 0,
                    rate: 1.0,
                    gain: 0.0,
                    group: 3,
                    wind_weight: 1.0,
                });
            }
            handle.send(Command::StartVoice {
                handle: 1,
                sample: 0,
                rate: 1.0,
                gain: 1.0,
                group: 3,
                wind_weight: 0.0,
            });
            // Settle for ~1.5 s (12+ time constants), then measure.
            let mut buffer = vec![0.0f32; 1024 * 2];
            for _ in 0..70 {
                engine.process(&mut buffer, 2);
            }
            if phantom_voices > 0 {
                let pressure = engine.wind_pressure(3);
                assert!(
                    (pressure - 0.995).abs() < 0.002,
                    "steady pressure {pressure} should be ~0.995 at reference demand"
                );
            }
            let mut buffer = vec![0.0f32; 8192 * 2];
            engine.process(&mut buffer, 2);
            measured_period(&buffer)
        };

        let unloaded = run(0);
        // 30 phantoms × weight 1.0 = the default reference demand.
        let loaded = run(30);
        assert!(
            (unloaded - period as f64).abs() < 0.3,
            "unloaded period {unloaded} should be ~{period}"
        );
        // Expected: P=0.995, rate factor 0.995^0.35 ≈ 0.99825 → ~480.8
        // (≈ −3 cents: audible as breathing, not as portamento).
        assert!(
            loaded > 480.4 && loaded < 481.4,
            "loaded period {loaded} should sag to ~480.8 frames"
        );
    }

    #[test]
    fn stereo_sample_reaches_both_channels() {
        // L ramps up, R constant — catches interleave mistakes.
        let mut data = Vec::new();
        for i in 0..100 {
            data.push(i as f32 / 100.0);
            data.push(0.25);
        }
        let sample = Sample::new(data, 2, 100.0, Some((10, 90)), 90).expect("valid");
        let mut bank = SampleBank::default();
        bank.push(sample);
        let (mut engine, mut handle) = Engine::new(100.0, Arc::new(bank));
        handle.send(Command::StartVoice {
            handle: 1,
            sample: 0,
            rate: 1.0,
            gain: 1.0,
            group: 0,
            wind_weight: 0.0,
        });
        let out = render(&mut engine, 50);
        let master = DEFAULT_MASTER_GAIN;
        assert!((out[41] - 0.25 * master).abs() < 1e-6, "right channel");
        assert!((out[40] - out[41]).abs() > 1e-6, "channels differ");
    }
}
