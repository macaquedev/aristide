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

use std::sync::Arc;

use bank::{Sample, SampleBank};
use rtrb::{Consumer, Producer, RingBuffer};

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
    StartVoice {
        handle: u64,
        sample: u32,
        rate: f32,
        gain: f32,
    },
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
    /// Source frames advanced per output frame.
    rate: f64,
    gain: f32,
    /// Crossfade progress 0→1.
    fade: f32,
    /// FadeOut amplitude 1→0.
    amplitude: f32,
    phase: SamplePhase,
}

impl SampledVoice {
    /// Render one frame and advance. Returns `None` when the voice ends.
    /// End-of-data checks happen on entry so every read frame is emitted.
    #[inline]
    fn tick(&mut self, sample: &Sample, crossfade_step: f32, kill_step: f32) -> Option<(f32, f32)> {
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

        let (mut left, mut right) = sample.read(self.position);
        let mut advance_position = true;
        match self.phase {
            SamplePhase::Held | SamplePhase::Tail => {}
            SamplePhase::Crossfade => {
                let (tail_l, tail_r) = sample.read(self.release_position);
                left += (tail_l - left) * self.fade;
                right += (tail_r - right) * self.fade;
                self.fade += crossfade_step;
                self.release_position += self.rate;
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
            self.position += self.rate;
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
                self.release_position = tail as f64;
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
    voices: Box<[Voice]>,
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
            voices: vec![Voice::Idle; MAX_VOICES].into_boxed_slice(),
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

        // Split borrows: voices mutably, bank/read-only params shared.
        let Engine {
            voices,
            bank,
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
                    for frame in 0..frames {
                        match sampled.tick(sample, *crossfade_step, *kill_step) {
                            Some((left, right)) => mix_frame(
                                &mut buffer[frame * channels..],
                                channels,
                                left * master,
                                right * master,
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
                        phase: SamplePhase::Held,
                    });
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
        });
        let out = render(&mut engine, 50);
        let master = DEFAULT_MASTER_GAIN;
        assert!((out[41] - 0.25 * master).abs() < 1e-6, "right channel");
        assert!((out[40] - out[41]).abs() > 1e-6, "channels differ");
    }
}
