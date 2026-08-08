//! The real-time audio core.
//!
//! Invariants for everything in this crate's audio path:
//! - never allocate, lock, or perform I/O on the audio thread
//! - control-plane communication via lock-free queues only
//! - disk streaming on dedicated threads filling ring buffers;
//!   sample attacks are pre-cached in RAM
//!
//! The engine is a pure library — buffers in, buffers out. Device
//! ownership lives in `aristide-server`.
//!
//! M1 state: a fixed voice pool playing an additive organ-ish test tone.
//! Pitch is decided control-side (`freq_hz` travels in the command), so
//! the RT core already has no notion of keys-as-pitches — the microtonal
//! mapping layer will slot in without touching this crate.

use rtrb::{Consumer, Producer, RingBuffer};

pub const MAX_VOICES: usize = 256;
const COMMAND_QUEUE_CAPACITY: usize = 2048;

const ATTACK_SECONDS: f32 = 0.006;
const RELEASE_SECONDS: f32 = 0.09;
const MASTER_GAIN: f32 = 0.08;

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
    NoteOn { key: u8, freq_hz: f32 },
    NoteOff { key: u8 },
    AllNotesOff,
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
enum Stage {
    #[default]
    Idle,
    Attack,
    Sustain,
    Release,
}

#[derive(Clone, Copy, Default)]
struct Voice {
    key: u8,
    phase: f32,
    phase_increment: f32,
    envelope: f32,
    stage: Stage,
}

impl Voice {
    fn start(&mut self, key: u8, freq_hz: f32, sample_rate: f32) {
        self.key = key;
        self.phase = 0.0;
        self.phase_increment = freq_hz / sample_rate;
        self.envelope = 0.0;
        self.stage = Stage::Attack;
    }

    #[inline]
    fn tick(&mut self, attack_step: f32, release_step: f32) -> f32 {
        match self.stage {
            Stage::Idle => return 0.0,
            Stage::Attack => {
                self.envelope += attack_step;
                if self.envelope >= 1.0 {
                    self.envelope = 1.0;
                    self.stage = Stage::Sustain;
                }
            }
            Stage::Sustain => {}
            Stage::Release => {
                self.envelope -= release_step;
                if self.envelope <= 0.0 {
                    self.envelope = 0.0;
                    self.stage = Stage::Idle;
                    return 0.0;
                }
            }
        }

        let two_pi = core::f32::consts::TAU;
        let mut sample = 0.0;
        for (multiple, amplitude) in HARMONICS {
            sample += amplitude * (two_pi * self.phase * multiple).sin();
        }
        self.phase += self.phase_increment;
        if self.phase >= 1.0 {
            self.phase -= 1.0;
        }
        sample * self.envelope
    }
}

/// RT side. Owned by the audio callback; every method here upholds the
/// no-alloc/no-lock/no-I/O invariants.
pub struct Engine {
    sample_rate: f32,
    commands: Consumer<Command>,
    voices: [Voice; MAX_VOICES],
    attack_step: f32,
    release_step: f32,
}

impl Engine {
    pub fn new(sample_rate: f32) -> (Engine, EngineHandle) {
        let (producer, consumer) = RingBuffer::new(COMMAND_QUEUE_CAPACITY);
        let engine = Engine {
            sample_rate,
            commands: consumer,
            voices: [Voice::default(); MAX_VOICES],
            attack_step: 1.0 / (ATTACK_SECONDS * sample_rate),
            release_step: 1.0 / (RELEASE_SECONDS * sample_rate),
        };
        (engine, EngineHandle { commands: producer })
    }

    /// Render one interleaved buffer. The same mono signal goes to every
    /// channel until routing exists (M6).
    pub fn process(&mut self, buffer: &mut [f32], channels: usize) {
        while let Ok(command) = self.commands.pop() {
            self.apply(command);
        }

        for frame in buffer.chunks_mut(channels.max(1)) {
            let mut sample = 0.0;
            for voice in &mut self.voices {
                if voice.stage != Stage::Idle {
                    sample += voice.tick(self.attack_step, self.release_step);
                }
            }
            sample *= MASTER_GAIN;
            for out in frame {
                *out = sample;
            }
        }
    }

    fn apply(&mut self, command: Command) {
        match command {
            Command::NoteOn { key, freq_hz } => {
                let slot = self
                    .voices
                    .iter()
                    .position(|v| v.stage == Stage::Idle)
                    .or_else(|| {
                        self.voices
                            .iter()
                            .enumerate()
                            .min_by(|(_, a), (_, b)| a.envelope.total_cmp(&b.envelope))
                            .map(|(i, _)| i)
                    });
                if let Some(index) = slot {
                    self.voices[index].start(key, freq_hz, self.sample_rate);
                }
            }
            Command::NoteOff { key } => {
                for voice in &mut self.voices {
                    if voice.key == key
                        && (voice.stage == Stage::Attack || voice.stage == Stage::Sustain)
                    {
                        voice.stage = Stage::Release;
                    }
                }
            }
            Command::AllNotesOff => {
                for voice in &mut self.voices {
                    if voice.stage != Stage::Idle {
                        voice.stage = Stage::Release;
                    }
                }
            }
        }
    }
}
