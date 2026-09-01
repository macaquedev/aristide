//! The control → RT command vocabulary and its queue.

use rtrb::Producer;

use crate::enclosure::EnclosureParams;
use crate::routing;
use crate::wind::{self, WindParams};

pub(crate) const COMMAND_QUEUE_CAPACITY: usize = 8192;

#[derive(Debug, Clone, Copy)]
pub enum Command {
    /// Start a sampled voice. `rate` is source frames per output frame
    /// (sample-rate ratio × pitch adjustments), `gain` is linear.
    /// `group` is the wind group the voice draws from and `wind_weight`
    /// how much it draws (0 = draws nothing, e.g. action noises).
    /// `brightness` is the voice's tilt-filter one-pole coefficient
    /// (control-side from the pipe's pitch; 0 bypasses the filter).
    /// `enclosure` is the swell box the voice sits inside
    /// ([`ENCLOSURE_NONE`](crate::enclosure::ENCLOSURE_NONE) for
    /// unenclosed divisions).
    /// `bus` is the output bus the voice renders onto (0 = the main
    /// pair) and `delay_frames` an onset delay: the voice waits that
    /// many output frames before speaking — per-pipe tracker/speaking
    /// delay, the Orgelpark trick at its smallest. A voice released
    /// before it ever spoke dies silently.
    StartVoice {
        handle: u64,
        sample: u32,
        rate: f32,
        gain: f32,
        group: u8,
        wind_weight: f32,
        brightness: f32,
        enclosure: u8,
        bus: u8,
        delay_frames: u32,
        /// The pipe's sounding frequency in Hz — how big a pipe this
        /// is, which is what decides how fast its amplitude can answer
        /// the chest (speech time ~ tens of periods). 0 = unpitched
        /// (noises): chest factors apply unlagged.
        nominal_hz: f32,
    },
    /// Reconfigure one wind group's supply model.
    SetWind { group: u8, params: WindParams },
    /// Configure one wind group's tremulant.
    SetTremulantParams {
        group: u8,
        params: wind::TremulantParams,
    },
    /// Engage/disengage one wind group's tremulant (ramped).
    SetTremulant { group: u8, engaged: bool },
    /// Flag one wind group's *wave* tremulant: no synthesized
    /// modulation, only which recording variants pipes on the chest
    /// prefer — held voices will pick releases matching this state.
    SetWaveTremulant { group: u8, engaged: bool },
    /// Reconfigure one enclosure's box model.
    SetEnclosure {
        enclosure: u8,
        params: EnclosureParams,
    },
    /// Move one enclosure's pedal (0 = closed, 1 = open); the shutter
    /// inertia model slews toward it.
    SetEnclosurePosition { enclosure: u8, position: f32 },
    /// Glide a sounding voice's playback rate to a new target over
    /// `glide_ms` (0 = snap at the next block). The slew is geometric —
    /// constant cents per frame — so a glide reads as linear pitch
    /// motion. Only Held voices move: a release tail is room decay that
    /// already left the pipe, the same reason shutter moves never touch
    /// it. This is the seam MPE/MIDI 2.0 per-note pitch and live tuning
    /// drift ride on.
    SetVoiceRate {
        handle: u64,
        rate: f32,
        glide_ms: f32,
    },
    /// Release the voice started with `handle`. Loop-less (percussive)
    /// voices ignore this and play to their end.
    StopVoice { handle: u64 },
    /// Silence a voice quickly WITHOUT its release tail (a short fade) —
    /// for retiring control-noise voices silently.
    KillVoice { handle: u64 },
    /// Configure one output bus's delay node (the first public effects
    /// node; `mix: 0` bypasses it).
    SetBusDelay {
        bus: u8,
        params: routing::DelayParams,
    },
    /// Route one bus onto an output channel pair at a level. Channels
    /// the device hasn't got fall back to the main pair at render time.
    SetBusOutput {
        bus: u8,
        left: u8,
        right: u8,
        gain: f32,
    },
    /// Start/stop the built-in additive test tone (no-set mode).
    NoteOn { key: u8, freq_hz: f32 },
    NoteOff { key: u8 },
    /// Fade out every sounding voice.
    AllNotesOff,
    SetMasterGain { linear: f32 },
    /// Convolution reverb wet level (0 bypasses entirely).
    SetReverbWet { wet: f32 },
}

/// Control-plane side of the engine. Not RT-constrained.
pub struct EngineHandle {
    pub(crate) commands: Producer<Command>,
}

impl EngineHandle {
    /// Returns `false` if the queue was full and the command dropped.
    pub fn send(&mut self, command: Command) -> bool {
        self.commands.push(command).is_ok()
    }
}
