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
pub mod enclosure;
pub mod resample;
pub mod reverb;
pub mod routing;
pub mod wind;

use std::sync::Arc;

use aristide_model::units::cents_to_ratio;
use bank::{Sample, SampleBank};
use enclosure::{Enclosure, EnclosureParams, ENCLOSURE_NONE, MAX_ENCLOSURES};
use resample::SincTables;
use routing::{Bus, MAX_BUSES, MAX_CHUNK_FRAMES};
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

/// −15 dB: the GrandOrgue ecosystem convention — sample sets (e.g.
/// Grabowski's) are normalized so a full-organ registration fits under
/// exactly this default. Our previous −9 dB default clipped tuttis that
/// GO plays cleanly.
const DEFAULT_MASTER_GAIN: f32 = 0.178;

/// Release tails older organs let ring for seconds; fast playing piles
/// hundreds of them up and the CPU, not the ear, pays (each is masked
/// by the fresh attacks anyway). Above this budget the quietest tails
/// get a fast fade — HW's own polyphony strategy ("the most
/// inconspicuous release samples" go first, Technical Datasheet).
const TAIL_VOICE_BUDGET: usize = 128;
/// At most this many tails shed per block, so a burst thins gradually.
const TAIL_SHED_PER_BLOCK: usize = 8;

/// Master-bus limiter ceiling and release. Big registrations with
/// couplers can sum far past full scale; without this the DAC hard-clips
/// ("horrible distortion when playing lots of notes"). Instant attack,
/// ~200 ms release: a sustained tutti settles to a clean constant
/// turn-down rather than crunching.
const LIMITER_CEILING: f32 = 0.97;
const LIMITER_RELEASE_SECONDS: f32 = 0.2;

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
    /// `brightness` is the voice's tilt-filter one-pole coefficient
    /// (control-side from the pipe's pitch; 0 bypasses the filter).
    /// `enclosure` is the swell box the voice sits inside
    /// ([`ENCLOSURE_NONE`] for unenclosed divisions).
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
    /// Where [`Command::SetVoiceRate`] is taking `rate`, and how many
    /// output frames of geometric slew remain. `glide_frames == 0` ⇔
    /// settled (`rate == rate_target`); the slew advances per block, so
    /// within a block pitch is constant — at control rates that is the
    /// same quantization every MIDI-driven sampler has.
    rate_target: f64,
    glide_frames: u32,
    /// Sinc kernel bucket chosen once at [`Command::StartVoice`] from
    /// `rate` ([`SincTables::select`]) and reused for the voice's whole
    /// life: tremulant/release-bend wobble never swings `rate` far
    /// enough to cross a quarter-octave bucket boundary, so re-picking
    /// per block would only add cost, not quality.
    kernel: usize,
    gain: f32,
    /// Crossfade progress 0→1.
    fade: f32,
    /// Release pitch drop: as the pallet closes, blowing pressure
    /// collapses and a flue pipe's pitch sags before the sound dies —
    /// small pipes noticeably so (Viscount US7442869 models this; Aeolus
    /// has per-stop release detune). A constant-pitch high release reads
    /// as a struck bell; bells don't bend. `bend` ramps 0 to 1; the
    /// playback rate is scaled by (1 - depth * bend).
    release_bend: f32,
    release_bend_depth: f32,
    release_bend_step: f32,
    /// Staccato room-charge: the tail's LATE diffuse field never built
    /// up for a short note, but its early reflections and speech-off
    /// did. Output is scaled by (charge + deficit) where deficit decays
    /// from (1 - charge) to 0 over ~150 ms: full level at the splice,
    /// settling to the charge level for the developed-reverb portion.
    tail_charge: f32,
    tail_charge_deficit: f32,
    tail_charge_step: f32,
    /// Per-voice crossfade step, set at release() from the pipe's
    /// fundamental: ~9 periods, clamped 6–184 ms (GO/HW practice: bass
    /// splices need long fades, treble fades must be short or they smear
    /// the speech-off transient into an "artificial" fade). 0 = use the
    /// engine default.
    fade_step: f32,
    /// FadeOut amplitude 1→0.
    amplitude: f32,
    /// Fast envelope follower on the voice's own (pre-gain) output —
    /// what "how loud am I right now" means at release time.
    envelope: f32,
    /// Level-matching scale applied to the release tail so it continues
    /// at the voice's current loudness instead of the recording's.
    tail_gain: f32,
    /// Output bus the voice renders onto (pre-clamped to `MAX_BUSES`).
    bus: u8,
    /// Output frames still to wait before the pipe speaks (per-pipe
    /// onset delay). While pending the voice renders nothing, draws no
    /// wind, and does not age; released before speaking, it dies
    /// silently — the pallet never opened.
    onset: u32,
    /// Wind group index (pre-clamped to `MAX_WIND_GROUPS`).
    group: u8,
    /// How much wind this voice draws while sounding.
    wind_weight: f32,
    /// Chest factors (pressure/tremulant/flow-noise pitch, gain,
    /// brightness) cached per voice under the same rule as the box
    /// factors below: a Held voice re-reads them each block; a
    /// released voice keeps them FROZEN — the pallet is closed, the
    /// tail is room decay, and it must not wobble (GO detaches
    /// releases from the windchest likewise).
    wind_rate: f32,
    wind_gain: f32,
    wind_treble: f32,
    /// This pipe's own answer to the chest, fixed at voice start.
    /// `wind_sens` spreads the modulation depth across the chorus (no
    /// two pipes are voiced identically); the two rates are 1/τ for
    /// the one-pole lags below — pitch follows pressure within a few
    /// speaking periods, amplitude and timbre only over the pipe's
    /// speech time (~tens of periods), so a 16' bass barely flutters
    /// at tremulant rates while a 2' pipe follows the valve, and every
    /// pipe sits at its own phase. Uniform, instant factors are the
    /// single-LFO sound of an electronic vibrato.
    wind_sens: f32,
    wind_pitch_rate: f32,
    wind_gain_rate: f32,
    /// Output frames since the voice started — drives the wind model's
    /// pallet-opening attack boost.
    age_frames: u32,
    /// Tilt-filter coefficient (0 = bypass) and per-channel lowpass
    /// state: out = lp + treble·(x − lp) splits the signal at roughly
    /// the pipe's 2nd harmonic so pressure can breathe the timbre.
    brightness_a: f32,
    lowpass: [f32; 2],
    /// Swell box this voice sits inside ([`ENCLOSURE_NONE`] = none),
    /// with the box factors cached per voice: a Held voice re-reads
    /// them each block; a released voice keeps them FROZEN — the tail
    /// is room decay that already left the box, so later shutter moves
    /// must not touch it (HW's rule; GO bakes the gain in likewise).
    enclosure: u8,
    /// Broadband box gain, de-zippered per frame with a ~5 ms one-pole
    /// toward `enc_gain_target` (block-stepped gain is audible zipper;
    /// a one-pole never overshoots regardless of block size).
    enc_gain: f32,
    enc_gain_target: f32,
    /// Shelf leg: high-frequency gain and one-pole corner coefficient,
    /// same filter form as the brightness tilt but hinged at the box
    /// corner instead of the pipe's 2nd harmonic.
    enc_hi_gain: f32,
    enc_coeff: f32,
    enc_lowpass: [f32; 2],
    /// Per-pipe wind-flow noise (slow, independent per voice).
    wander: wind::Wander,
    rng: u32,
    /// Which of the sample's sustain loops the cursor is circling; a
    /// new one is drawn at random on each pass.
    loop_index: u8,
    /// Separate release sample being crossfaded into, if any.
    external_release: Option<u32>,
    /// Per-frame gain decay during Crossfade/Tail (1.0 = none). GO's
    /// staccato model: a short note hasn't formed the room's reverb
    /// yet, so its recorded (fully-reverberant) release tail is decayed
    /// over seconds to compensate.
    tail_decay: f32,
    /// FadeOut speed multiplier on the kill ramp: 1.0 = 15 ms (silent
    /// noise voices, panic), 0.1 = ~150 ms (polyphony shedding, where
    /// abruptness would be audible).
    fade_scale: f32,
    /// A scheduled key-release: (frames until the pallet closes, hold
    /// age in ms captured at key-up). Real pallets never close in the
    /// same millisecond across a chord, and spreading the release also
    /// spreads the crossfade CPU spike that a mass release causes.
    pending_release: Option<(u16, u32)>,
    /// The chest's wave-tremulant state as this voice last saw it —
    /// which recording variant its release should match. Follows the
    /// live state while Held (GO selects by the state at key-off).
    wave_trem: bool,
    phase: SamplePhase,
    /// The cursor has left the sustain loop for release material (set at
    /// crossfade completion). Loop wrapping and seam-tap reads must never
    /// apply again: a shed/killed tail whose phase is FadeOut is NOT in
    /// the loop, and wrapping it teleports the cursor back into
    /// full-level sustain — the click/ghost-note bug found 2026-08-11.
    past_loop: bool,
}

/// Per-block invariants of a sampled voice's render loop, hoisted out
/// of the per-frame path (`Sample::frames()` alone is a u64 division —
/// two of those per frame per voice was a real cost at high polyphony).
/// Must be recomputed when the voice changes sample or loop mid-block.
#[derive(Clone, Copy)]
struct VoiceBlockContext {
    lite: bool,
    rate: f64,
    last: f64,
    tail_last: f64,
    current_loop: Option<(u64, u64)>,
    looping: bool,
    /// Engine output sample rate; releases need it to convert dB/s
    /// decay compensation into a per-frame factor.
    output_sr: f32,
}

impl SampledVoice {
    #[inline]
    fn block_context(
        &self,
        sample: &Sample,
        external: Option<&Sample>,
        rate_scale: f64,
    ) -> VoiceBlockContext {
        let current_loop = sample.loop_at(self.loop_index as usize);
        VoiceBlockContext {
            lite: false,
            rate: self.rate * rate_scale,
            last: (sample.frames() - 1) as f64,
            tail_last: (external.unwrap_or(sample).frames() - 1) as f64,
            current_loop,
            looping: current_loop.is_some(),
            output_sr: 44_100.0, // overridden by the block loop
        }
    }

    /// Render one frame and advance. Returns `None` when the voice ends.
    /// End-of-data checks happen on entry so every read frame is emitted.
    #[inline]
    fn tick(
        &mut self,
        sample: &Sample,
        external: Option<&Sample>,
        tables: &SincTables,
        ctx: &VoiceBlockContext,
        crossfade_step: f32,
        kill_step: f32,
    ) -> Option<(f32, f32)> {
        let mut rate = ctx.rate;
        if self.release_bend_depth > 0.0
            && matches!(self.phase, SamplePhase::Crossfade | SamplePhase::Tail)
        {
            self.release_bend += (1.0 - self.release_bend) * self.release_bend_step;
            rate *= 1.0 - (self.release_bend_depth * self.release_bend) as f64;
        }
        let last = ctx.last;
        let current_loop = ctx.current_loop;
        let looping = ctx.looping;
        // A scheduled release fires when its pallet-delay runs out.
        if self.phase == SamplePhase::Held {
            if let Some((delay, age_ms)) = self.pending_release {
                if delay == 0 {
                    self.pending_release = None;
                    self.release(sample, age_ms, ctx.output_sr);
                } else {
                    self.pending_release = Some((delay - 1, age_ms));
                }
            }
        }
        // During a crossfade into a separate release sample, the tail
        // cursor lives in that sample's coordinates.
        let tail_sample = external.unwrap_or(sample);
        let tail_last = ctx.tail_last;
        let ended = match self.phase {
            SamplePhase::Held => !looping && self.position >= last,
            SamplePhase::Crossfade => self.release_position >= tail_last,
            SamplePhase::Tail => self.position >= last,
            SamplePhase::FadeOut => {
                self.amplitude <= 0.0 || ((!looping || self.past_loop) && self.position >= last)
            }
        };
        if ended {
            return None;
        }

        // Cursors still circling the sustain loop wrap their kernel taps
        // across the seam; tail reads clamp at the sample edges.
        let seam = if self.phase == SamplePhase::Tail || self.past_loop {
            None
        } else {
            current_loop
        };
        // During a crossfade the OUTGOING leg fades to zero — linear
        // interpolation there is inaudible and halves the double-read
        // cost that made mass releases blow the block budget. The
        // persistent (incoming) leg keeps full sinc quality.
        // The outgoing crossfade leg once dropped to linear interpolation
        // to halve the double-read cost, but the sinc→linear switch at
        // release() put a ~-46 dB kink at the START of every splice — an
        // audible tick on exposed releases. Keep full quality; the pallet
        // stagger already spreads the crossfade CPU spike.
        let (mut left, mut right) = if ctx.lite {
            sample.read(self.position)
        } else {
            tables.read(self.kernel, sample, self.position, seam)
        };
        let mut advance_position = true;
        // This frame's output gain, captured BEFORE the match arms mutate
        // self.gain. The crossfade-completion frame folds tail_gain into
        // the voice gain for FUTURE frames, but its own blend already
        // applied tail_gain — returning with the mutated gain applied it
        // twice, dipping exactly one frame by up to 5x (tail_gain floor
        // 0.2): an audible tick on every splice handover.
        let frame_gain = self.gain;
        match self.phase {
            SamplePhase::Held | SamplePhase::Tail => {}
            SamplePhase::Crossfade => {
                let (tail_l, tail_r) = if ctx.lite {
                    tail_sample.read(self.release_position)
                } else {
                    tables.read(self.kernel, tail_sample, self.release_position, None)
                };
                // Raised-cosine-shaped blend (smoothstep ≈ it, no trig):
                // linear fades dip audibly on the uncorrelated noise
                // floor (Appleton 2019).
                let weight = self.fade * self.fade * (3.0 - 2.0 * self.fade);
                left += (tail_l * self.tail_gain - left) * weight;
                right += (tail_r * self.tail_gain - right) * weight;
                self.fade += if self.fade_step > 0.0 {
                    self.fade_step
                } else {
                    crossfade_step
                };
                self.release_position += rate;
                if self.fade >= 1.0 {
                    // Hand the (already advanced) tail cursor over and
                    // fold the level match into the voice gain. If the
                    // tail is a separate sample, the voice moves there.
                    self.position = self.release_position;
                    self.gain *= self.tail_gain;
                    self.tail_gain = 1.0;
                    if let Some(external_id) = self.external_release.take() {
                        self.sample = external_id;
                        self.loop_index = 0;
                    }
                    self.phase = SamplePhase::Tail;
                    self.past_loop = true;
                    advance_position = false;
                }
            }
            SamplePhase::FadeOut => {
                left *= self.amplitude;
                right *= self.amplitude;
                self.amplitude -= kill_step * self.fade_scale;
            }
        }

        if self.tail_charge_deficit > 0.0
            && !matches!(self.phase, SamplePhase::Held)
        {
            let factor = self.tail_charge + self.tail_charge_deficit;
            left *= factor;
            right *= factor;
            self.tail_charge_deficit *= self.tail_charge_step;
            if self.tail_charge_deficit < 1e-4 {
                // Settled: fold the charge into the gain and stop paying
                // the per-frame cost.
                self.gain *= self.tail_charge;
                self.tail_charge = 1.0;
                self.tail_charge_deficit = 0.0;
            }
        } else if self.tail_charge != 1.0 && !matches!(self.phase, SamplePhase::Held) {
            left *= self.tail_charge;
            right *= self.tail_charge;
        }
        // EOF guard: a tail must reach the end of its material silent.
        // Decay compensation can leave boosted level near EOF (and some
        // sets simply end hot); fade the final ~46 ms instead of cutting.
        if self.past_loop {
            const GUARD_FRAMES: f64 = 2048.0;
            let remaining = last - self.position;
            if remaining < GUARD_FRAMES {
                let scale = (remaining / GUARD_FRAMES).max(0.0) as f32;
                left *= scale;
                right *= scale;
            }
        }
        if matches!(self.phase, SamplePhase::Crossfade | SamplePhase::Tail)
            && self.tail_decay != 1.0
        {
            self.gain *= self.tail_decay;
        }
        if advance_position {
            self.position += rate;
            // Only cursors still circling the sustain loop wrap; a Tail
            // cursor has left it for the release material. On each pass
            // a fresh loop is drawn at random (multi-loop sets), which
            // decorrelates repetition.
            if self.phase != SamplePhase::Tail && !self.past_loop {
                if let Some((start, end)) = current_loop {
                    if self.position >= end as f64 {
                        // Wrap to THIS loop's own start — the only splice
                        // the set's author guaranteed seamless. Loop
                        // variety comes from choosing which loop's end we
                        // run toward next (all loops live in one
                        // continuous recording, so playing from here to
                        // any later end is seamless too). Jumping into a
                        // different loop's start pops audibly.
                        let overshoot = self.position - end as f64;
                        self.position = (start as f64 + overshoot).min(end as f64 - 1.0);
                        let count = sample.loop_count();
                        if count > 1 {
                            let candidate = (wind::xorshift_unit(&mut self.rng) * count as f32)
                                as u8
                                % count as u8;
                            if let Some((_, candidate_end)) =
                                sample.loop_at(candidate as usize)
                            {
                                if (candidate_end as f64) > self.position {
                                    self.loop_index = candidate;
                                }
                            }
                        }
                    }
                }
            }
        }
        Some((left * frame_gain, right * frame_gain))
    }

    /// Key released: splice to a separate release (selected by hold
    /// duration) or the embedded tail, whichever the sample offers.
    fn release(&mut self, sample: &Sample, age_ms: u32, output_rate: f32) {
        fn pitch_scaled_fade_step(
            sample: &Sample,
            rate: f64,
            output_rate: f32,
            age_ms: u32,
        ) -> f32 {
            let Some(period) = sample.measured_period() else {
                return 0.0; // engine default
            };
            let output_period = period / rate.max(1e-6);
            // ~9 fundamental periods (GO: 184 ms bass → 6 ms treble),
            // but never longer than the note has lived: a mid-attack
            // release must not keep swelling through a long fade — the
            // drive collapses when the pallet closes.
            let age_frames = age_ms as f64 * 0.001 * output_rate as f64;
            let frames = (9.0 * output_period)
                .min(age_frames.max(0.006 * output_rate as f64))
                .clamp(0.006 * output_rate as f64, 0.184 * output_rate as f64);
            (1.0 / frames) as f32
        }
        // A producer-tuned crossfade (ODF ReleaseCrossfadeLength)
        // overrides the pitch-scaled default; the note-age cap stays —
        // a mid-attack release must still collapse, not swell.
        fn odf_fade_step(ms: u16, output_rate: f32, age_ms: u32) -> f32 {
            let age_frames = age_ms as f64 * 0.001 * output_rate as f64;
            let frames = (f64::from(ms) * 0.001 * output_rate as f64)
                .min(age_frames.max(0.006 * output_rate as f64))
                .max(1.0);
            (1.0 / frames) as f32
        }
        match self.phase {
            SamplePhase::Held | SamplePhase::Crossfade => {}
            _ => return,
        }
        if self.phase == SamplePhase::Held && sample.sustain_loop().is_some() {
            // Options are sorted (bounded holds ascending, unbounded
            // last): the first whose bound covers the hold wins.
            let chosen = sample
                .release_options()
                .iter()
                .filter(|option| {
                    option
                        .wave_trem
                        .is_none_or(|wants| wants == self.wave_trem)
                })
                .find(|option| option.max_hold_ms.is_none_or(|max| age_ms <= max));
            if let Some(option) = chosen {
                self.external_release = Some(option.sample);
                self.release_position = match (option.alignment(), sample.sustain_loop()) {
                    (Some(alignment), Some((loop_start, _))) => {
                        alignment.target(self.position, loop_start) as f64
                    }
                    _ => 0.0,
                };
                self.tail_gain = if option.level > 1e-5 {
                    (self.envelope / option.level).clamp(0.05, 1.1)
                } else {
                    1.0
                };
                self.fade_step = if option.crossfade_ms > 0 {
                    odf_fade_step(option.crossfade_ms, output_rate, age_ms)
                } else {
                    pitch_scaled_fade_step(sample, self.rate, output_rate, age_ms)
                };
                self.fade = 0.0;
                self.phase = SamplePhase::Crossfade;
                return;
            }
        }
        match sample.release_start() {
            Some(tail) if self.phase == SamplePhase::Held => {
                self.release_position = match (sample.release_alignment(), sample.sustain_loop()) {
                    (Some(alignment), Some((loop_start, _))) => {
                        alignment.target(self.position, loop_start) as f64
                    }
                    _ => tail as f64,
                };
                // Level match: scale the tail to continue at the voice's
                // current loudness (early releases are quieter than the
                // recorded sustain — unscaled tails strike like a bell).
                // Floor at 0.2 like GO: a fully-silent-entry release
                // sounds MORE artificial than a slightly loud one.
                // Exception: a near-silent loop (control-noise samples:
                // thump → silent loop → thump tail) means the tail is
                // MEANT to be louder — play it as recorded.
                let reference = sample.tail_reference_level();
                self.tail_gain = if reference > 1e-5 && self.envelope > 0.02 * reference {
                    (self.envelope / reference).clamp(0.2, 1.1)
                } else {
                    1.0
                };
                // Staccato: a room's decay RATE is fixed by the room —
                // a short note leaves a QUIETER tail, never a faster-
                // decaying one (GO decays the rate instead, which turns
                // fast passages into plucks). Model the room charge as a
                // first-order build-up toward steady state.
                let tail_seconds =
                    (sample.frames().saturating_sub(tail)) as f32 / sample.sample_rate_hz();
                let full_reverb_ms = (60.0 * tail_seconds + 40.0).clamp(100.0, 350.0);
                let mut staccato_extra_db_per_s = 0.0f32;
                if (age_ms as f32) < full_reverb_ms {
                    let charge =
                        (1.0 - (-(age_ms as f32) / (0.5 * full_reverb_ms)).exp()).max(0.1);
                    self.tail_charge = charge;
                    self.tail_charge_deficit = 1.0 - charge;
                    self.tail_charge_step = (-1.0 / (0.15 * output_rate)).exp();
                    // Level scaling alone leaves a conspicuous shimmer
                    // after high staccato (the diffuse field wasn't just
                    // quieter, it never fully formed): also shorten the
                    // late tail in proportion to how undeveloped it was.
                    staccato_extra_db_per_s = (1.0 - charge) * 25.0;
                }
                // Repitching by R also plays the recorded room decay R×
                // too fast (or slow) — ring time must not depend on the
                // key, so compensate the measured tail decay rate with a
                // per-frame gain factor. Down-repitched pipes were the
                // "bell": their tails rang up to 40% too long.
                self.fade_step = if sample.release_crossfade_ms() > 0 {
                    odf_fade_step(sample.release_crossfade_ms(), output_rate, age_ms)
                } else {
                    pitch_scaled_fade_step(sample, self.rate, output_rate, age_ms)
                };
                if let Some(period) = sample.measured_period() {
                    let f0 = sample.sample_rate_hz() as f64 / period;
                    // Depth grows with pipe pitch: ~35 cents at 1 kHz+,
                    // ~15 at 250 Hz, negligible for big pipes.
                    let cents = (4.0 * (f0 / 100.0).sqrt()).clamp(1.0, 12.0);
                    self.release_bend_depth = 1.0 - cents_to_ratio(-cents) as f32;
                    // Pressure collapse: ~12 periods, 15-80 ms.
                    let tau_s = (12.0 * period / sample.sample_rate_hz() as f64)
                        .clamp(0.015, 0.080);
                    self.release_bend_step =
                        1.0 - (-1.0 / (tau_s as f32 * output_rate)).exp();
                    self.release_bend = 0.0;
                }
                let lambda = sample.tail_decay_db_per_s();
                let repitch =
                    (self.rate as f32) * output_rate / sample.sample_rate_hz();
                let comp_db_per_s = if lambda > 0.0 && (repitch - 1.0).abs() > 0.01 {
                    (lambda * (repitch - 1.0)).clamp(-25.0, 25.0)
                } else {
                    0.0
                };
                // A tail still audible when its recording runs out ends
                // in a hard cut — the demo set's mixture has a rank 50 dB
                // hot at EOF that rings bell-like and then vanishes. Add
                // whatever decay settles the tail to ≈ -60 dB by EOF,
                // counting the level the decay compensation adds back.
                let out_tail_seconds = tail_seconds / repitch.max(0.01);
                let settle_db_per_s = if out_tail_seconds > 0.3 {
                    let eof_db =
                        sample.tail_eof_level_db() + comp_db_per_s * out_tail_seconds;
                    ((eof_db + 60.0) / out_tail_seconds).clamp(0.0, 60.0)
                } else {
                    0.0
                };
                let db_per_s = comp_db_per_s - staccato_extra_db_per_s - settle_db_per_s;
                self.tail_decay = if db_per_s.abs() > 0.01 {
                    10.0f32.powf(db_per_s / (20.0 * output_rate))
                } else {
                    1.0
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
    sinc: SincTables,
    voices: Box<[Voice]>,
    /// Output buses (scratch + delay rings preallocated; see routing.rs).
    buses: Vec<Bus>,
    wind: [WindGroup; MAX_WIND_GROUPS],
    /// Wave-tremulant state per wind group (bitmask): sample-variant
    /// selection only, no modulation — see [`Command::SetWaveTremulant`].
    wave_trems: u32,
    enclosures: [Enclosure; MAX_ENCLOSURES],
    reverb: Option<reverb::Reverb>,
    /// Diagnostic "safe mode": linear interpolation, no wind/tremulant/
    /// brightness/flow-noise — per-voice cost at or below GrandOrgue's.
    /// If audio still glitches in this mode, the environment (not the
    /// engine's DSP weight) is the cause.
    lite: bool,
    /// Diagnostic tap: every output sample is offered to this ring
    /// (lock-free push, silently dropped when full) so the control side
    /// can record the engine's EXACT output — the decisive test for
    /// "is the noise in the engine or in the delivery layer".
    tap: Option<Producer<f32>>,
    /// Free voice slots (invariant: index here ⇔ voice is Idle). A mass
    /// chord used to run 200+ O(2048) scans in one block — that spike
    /// alone ate the block budget.
    free_slots: Vec<u16>,
    /// StopVoice handles batched during the command drain and applied
    /// in ONE voice pass (mass releases used to scan per handle).
    stop_batch: Vec<u64>,
    /// Max random pallet-close delay applied to key releases, frames.
    release_stagger_frames: f32,
    /// Limiter envelope: the current tracked bus peak (decaying).
    limiter_envelope: f32,
    limiter_release: f32,
    master_gain: f32,
    tone_attack_step: f32,
    tone_release_step: f32,
    crossfade_step: f32,
    kill_step: f32,
    /// ~10 ms envelope-follower coefficient for release level matching.
    envelope_step: f32,
    /// ~5 ms one-pole coefficient de-zippering per-voice enclosure gain.
    enc_ramp: f32,
}

impl Engine {
    pub fn new(sample_rate: f32, bank: Arc<SampleBank>) -> (Engine, EngineHandle) {
        let (producer, consumer) = RingBuffer::new(COMMAND_QUEUE_CAPACITY);
        let mut engine = Engine {
            sample_rate,
            commands: consumer,
            bank,
            sinc: SincTables::new(),
            voices: vec![Voice::Idle; MAX_VOICES].into_boxed_slice(),
            buses: (0..MAX_BUSES).map(|_| Bus::new(sample_rate)).collect(),
            wind: [WindGroup::default(); MAX_WIND_GROUPS],
            wave_trems: 0,
            enclosures: [Enclosure::default(); MAX_ENCLOSURES],
            reverb: None,
            lite: false,
            tap: None,
            free_slots: (0..MAX_VOICES as u16).rev().collect(),
            stop_batch: Vec::with_capacity(MAX_VOICES),
            release_stagger_frames: 0.008 * sample_rate,
            limiter_envelope: 0.0,
            limiter_release: (-1.0 / (LIMITER_RELEASE_SECONDS * sample_rate)).exp(),
            master_gain: DEFAULT_MASTER_GAIN,
            tone_attack_step: 1.0 / (TONE_ATTACK_SECONDS * sample_rate),
            tone_release_step: 1.0 / (TONE_RELEASE_SECONDS * sample_rate),
            crossfade_step: 1.0 / (RELEASE_CROSSFADE_SECONDS * sample_rate),
            kill_step: 1.0 / (KILL_FADE_SECONDS * sample_rate),
            envelope_step: 1.0 - (-1.0 / (0.01 * sample_rate)).exp(),
            enc_ramp: 1.0 - (-1.0 / (0.005 * sample_rate)).exp(),
        };
        // Sixteen chests must not share one random sequence and one
        // tremulant phase — decorrelated once, here, they drift apart
        // for the rest of their lives.
        for (index, group) in engine.wind.iter_mut().enumerate() {
            group.decorrelate(index);
        }
        (engine, EngineHandle { commands: producer })
    }

    /// Render one interleaved buffer, mixing every active voice onto
    /// its bus and every bus onto its output channels. By default
    /// everything sits on bus 0 → channels 0/1 (summed to one on mono
    /// outputs), which renders bit-identically to the pre-bus engine.
    pub fn process(&mut self, buffer: &mut [f32], channels: usize) {
        while let Ok(command) = self.commands.pop() {
            self.apply(command);
        }
        // Apply the block's key releases in ONE pass over the pool
        // (per-handle scans made mass releases O(handles × voices) —
        // the measured 5 ms spike behind the release pops).
        if !self.stop_batch.is_empty() {
            self.stop_batch.sort_unstable();
            let per_ms = self.sample_rate / 1000.0;
            // Big releases spread wider (real tuttis do too): scale the
            // pallet stagger with the batch so crossfades don't all
            // land in the same two blocks.
            let batch_scale = 1.0 + self.stop_batch.len() as f32 / 64.0;
            let max_stagger =
                (self.release_stagger_frames * batch_scale).min(0.025 * self.sample_rate);
            for voice in self.voices.iter_mut() {
                if let Voice::Sampled(sampled) = voice {
                    if self.stop_batch.binary_search(&sampled.handle).is_ok() {
                        let age_ms = (sampled.age_frames as f32 / per_ms) as u32;
                        if sampled.onset > 0 {
                            // Released before the onset delay elapsed:
                            // the pallet never opened, so nothing ever
                            // sounded and nothing should.
                            sampled.phase = SamplePhase::FadeOut;
                            sampled.amplitude = 0.0;
                        } else if sampled.phase == SamplePhase::Held
                            && sampled.pending_release.is_none()
                        {
                            let delay = (wind::xorshift_unit(&mut sampled.rng)
                                * max_stagger) as u16;
                            sampled.pending_release = Some((delay, age_ms));
                        } else if let Some(sample) = self.bank.get(sampled.sample) {
                            sampled.release(sample, age_ms, self.sample_rate);
                        }
                    }
                }
            }
            self.stop_batch.clear();
        }

        buffer.fill(0.0);
        let channels = channels.max(1);
        let total = buffer.len() / channels;
        // Render in bounded slices so bus scratch can be sized once at
        // construction; callbacks bigger than a slice are rare but
        // legal, and everything inside is stateful and continuous
        // across the seam.
        let mut offset = 0;
        while offset < total {
            let frames = (total - offset).min(MAX_CHUNK_FRAMES);
            let range = offset * channels..(offset + frames) * channels;
            self.render_chunk(&mut buffer[range], channels, frames);
            offset += frames;
        }

        // Diagnostic recording tap (drops samples when the ring is
        // full rather than ever blocking).
        if let Some(tap) = &mut self.tap {
            for &value in buffer.iter() {
                let _ = tap.push(value);
            }
        }
    }

    fn render_chunk(&mut self, buffer: &mut [f32], channels: usize, frames: usize) {
        let master = self.master_gain;

        // Polyphony guard: bound the release-tail pileup (fast playing
        // stacks seconds-long tails; the quietest are inaudible under
        // the fresh attacks but still cost full render time).
        let mut tail_count = 0usize;
        for voice in self.voices.iter() {
            if let Voice::Sampled(sampled) = voice {
                if matches!(sampled.phase, SamplePhase::Tail | SamplePhase::Crossfade) {
                    tail_count += 1;
                }
            }
        }
        if tail_count > TAIL_VOICE_BUDGET {
            let to_shed = (tail_count - TAIL_VOICE_BUDGET).min(TAIL_SHED_PER_BLOCK);
            for _ in 0..to_shed {
                // Rank by audible contribution — envelope × voice gain —
                // not raw sample level (a quiet recording with high gain
                // outranks a hot recording turned down).
                let mut quietest: Option<(usize, f32)> = None;
                for (index, voice) in self.voices.iter().enumerate() {
                    if let Voice::Sampled(sampled) = voice {
                        let contribution = sampled.envelope * sampled.gain;
                        if sampled.phase == SamplePhase::Tail
                            && quietest.is_none_or(|(_, level)| contribution < level)
                        {
                            quietest = Some((index, contribution));
                        }
                    }
                }
                let Some((index, _)) = quietest else { break };
                if let Voice::Sampled(sampled) = &mut self.voices[index] {
                    sampled.amplitude = 1.0;
                    // Gentle ~150 ms fade: under 128 masking tails this
                    // is inaudible; the 15 ms kill ramp is not.
                    sampled.fade_scale = 0.1;
                    sampled.phase = SamplePhase::FadeOut;
                }
            }
        }

        // Wind: one regulator step per block. Demand sums the wind
        // weight of everything sounding on each chest, with young
        // voices boosted (the pallet-opening gulp).
        let mut demand = [0.0f32; MAX_WIND_GROUPS];
        for voice in self.voices.iter() {
            if let Voice::Sampled(sampled) = voice {
                if sampled.onset > 0 {
                    // A pipe waiting on its onset delay draws no wind:
                    // the pallet hasn't opened yet.
                    continue;
                }
                if sampled.phase != SamplePhase::Held {
                    // A released or fading pipe draws none either: the
                    // pallet has closed, only the tail is sounding —
                    // pressure recovers while tails ring out.
                    continue;
                }
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
        if !self.lite {
            for (group, wind) in self.wind.iter_mut().enumerate() {
                wind.step(demand[group], dt);
            }
            for box_state in self.enclosures.iter_mut() {
                box_state.step(dt, self.sample_rate);
            }
        }

        let lite = self.lite;
        // Split borrows: voices mutably, bank/read-only params shared.
        let output_sr = self.sample_rate;
        let Engine {
            voices,
            buses,
            bank,
            sinc,
            wind,
            enclosures,
            free_slots,
            tone_attack_step,
            tone_release_step,
            crossfade_step,
            kill_step,
            envelope_step,
            enc_ramp,
            ..
        } = self;

        for bus in buses.iter_mut() {
            bus.begin_chunk(frames);
        }

        for index in 0..voices.len() {
            let voice = &mut voices[index];
            match voice {
                Voice::Idle => {}
                Voice::Tone(tone) => {
                    let scratch = buses[0].mix_target(frames);
                    for frame in 0..frames {
                        let value = tone.tick(*tone_attack_step, *tone_release_step);
                        if tone.stage == ToneStage::Idle {
                            *voice = Voice::Idle;
                            free_slots.push(index as u16);
                            break;
                        }
                        scratch[frame * 2] += value * TONE_GAIN * master;
                        scratch[frame * 2 + 1] += value * TONE_GAIN * master;
                    }
                }
                Voice::Sampled(sampled) => {
                    // Onset delay: silent, un-aged, until it elapses —
                    // then the voice speaks partway into this chunk. A
                    // voice killed while still waiting never speaks.
                    let start_frame = if sampled.onset > 0 {
                        if sampled.phase != SamplePhase::Held {
                            *voice = Voice::Idle;
                            free_slots.push(index as u16);
                            continue;
                        }
                        if sampled.onset as usize >= frames {
                            sampled.onset -= frames as u32;
                            continue;
                        }
                        let start = sampled.onset as usize;
                        sampled.onset = 0;
                        start
                    } else {
                        0
                    };
                    let Some(sample) = bank.get(sampled.sample) else {
                        *voice = Voice::Idle;
                        free_slots.push(index as u16);
                        continue;
                    };
                    let chest = &wind[sampled.group as usize];
                    let params = chest.params();

                    // Wind factors follow the swell-box rule below: a
                    // Held voice re-reads its chest each block; a
                    // released voice keeps the factors frozen from its
                    // last Held block — the pallet is closed, the tail
                    // is room decay, and trem/pressure must not wobble
                    // it.
                    if !lite && sampled.phase == SamplePhase::Held {
                        // Per-voice flow noise, linearized around the
                        // chest factors (a powf per voice per block
                        // would also be fine, but ±2 % deviations are
                        // firmly linear).
                        let mut deviation = 0.0;
                        if params.flow_noise > 0.0 && sampled.wind_weight > 0.0 {
                            sampled
                                .wander
                                .step(dt, params.flow_noise, &mut sampled.rng);
                            deviation = sampled.wander.deviation();
                        }
                        // The chest says where pressure is; the PIPE
                        // decides how it answers: its own sensitivity
                        // spreads the depth across the chorus, and the
                        // one-pole lags below give each pipe its speech
                        // dynamics — pitch follows fast, amplitude and
                        // timbre over ~tens of periods, so basses
                        // barely flutter at tremulant rates and every
                        // pipe sits at its own phase. All identity when
                        // the chest is quiet (factors 1, no noise).
                        let sens = sampled.wind_sens;
                        let target_rate = 1.0
                            + (chest.rate_factor() - 1.0) * sens
                            + params.pitch_exponent * deviation;
                        let target_gain = 1.0
                            + (chest.gain_factor() - 1.0) * sens
                            + params.gain_exponent * deviation;
                        // The brightness exponent is calibrated on the
                        // regulator's few-percent sags; at tremulant
                        // pressure swings a pipe's spectrum saturates
                        // long before P^3 says ±6 dB, so the swing is
                        // capped at ≈ ±2.5 dB.
                        let target_treble = (1.0
                            + (chest.brightness_factor() - 1.0) * sens
                            + params.brightness_exponent * deviation)
                            .clamp(0.75, 1.33);
                        let pitch_alpha = (dt * sampled.wind_pitch_rate).min(1.0);
                        let slow_alpha = (dt * sampled.wind_gain_rate).min(1.0);
                        sampled.wind_rate += (target_rate - sampled.wind_rate) * pitch_alpha;
                        sampled.wind_gain += (target_gain - sampled.wind_gain) * slow_alpha;
                        sampled.wind_treble +=
                            (target_treble - sampled.wind_treble) * slow_alpha;
                    }
                    let (rate_scale, gain, treble) = if lite {
                        (1.0f64, master, 1.0f32)
                    } else {
                        (
                            sampled.wind_rate as f64,
                            master * sampled.wind_gain,
                            sampled.wind_treble,
                        )
                    };

                    let tilt_a = sampled.brightness_a;
                    // Bypass the filter while it would do nothing: keeps
                    // untouched-pressure rendering bit-identical.
                    let tilting = tilt_a > 0.0 && (treble - 1.0).abs() > 1e-4;

                    // Swell box: a Held voice tracks its box each block
                    // (gain ramped per frame — pedal sweeps would zipper
                    // otherwise); any released/fading voice keeps the
                    // factors frozen from its last Held block. Lite mode
                    // keeps the broadband gain (the pedal must still do
                    // something) and only skips the shutter filter.
                    let enclosed = sampled.enclosure != ENCLOSURE_NONE;
                    if enclosed && sampled.phase == SamplePhase::Held {
                        let box_state = &enclosures[sampled.enclosure as usize];
                        sampled.enc_gain_target = box_state.gain();
                        sampled.enc_hi_gain = box_state.hi_gain();
                        sampled.enc_coeff = box_state.coeff();
                    }
                    // A pending rate glide takes one geometric step per
                    // block (the powf is paid only while gliding). A
                    // bend can cross a quarter-octave sinc bucket, so
                    // the kernel is re-picked while in motion — the
                    // one case the pick-once rule at StartVoice excludes.
                    if sampled.glide_frames > 0 {
                        if sampled.phase == SamplePhase::Held {
                            if sampled.glide_frames as usize <= frames {
                                sampled.rate = sampled.rate_target;
                                sampled.glide_frames = 0;
                            } else {
                                let fraction = frames as f64 / sampled.glide_frames as f64;
                                sampled.rate *=
                                    (sampled.rate_target / sampled.rate).powf(fraction);
                                sampled.glide_frames -= frames as u32;
                            }
                            sampled.kernel = sinc.select(sampled.rate);
                        } else {
                            // Released mid-glide: the tail keeps the
                            // pitch it reached, as it keeps its box.
                            sampled.glide_frames = 0;
                            sampled.rate_target = sampled.rate;
                        }
                    }
                    sampled.age_frames = sampled
                        .age_frames
                        .saturating_add((frames - start_frame) as u32);
                    // The voice can hand over to a separate release
                    // sample or switch loops mid-block; track the refs
                    // and per-block invariants it reads from.
                    let mut current = sample;
                    let mut current_id = sampled.sample;
                    let mut current_loop_index = sampled.loop_index;
                    let mut current_external_id = sampled.external_release;
                    let mut external = current_external_id.and_then(|id| bank.get(id));
                    let mut ctx = sampled.block_context(current, external, rate_scale);
                    ctx.lite = lite;
                    ctx.output_sr = output_sr;
                    let scratch = buses[sampled.bus as usize].mix_target(frames);
                    for frame in start_frame..frames {
                        if sampled.sample != current_id {
                            match bank.get(sampled.sample) {
                                Some(switched) => {
                                    current = switched;
                                    current_id = sampled.sample;
                                    external = None;
                                    current_external_id = None;
                                    current_loop_index = sampled.loop_index;
                                    ctx = sampled.block_context(current, external, rate_scale);
                                    ctx.lite = lite;
                    ctx.output_sr = output_sr;
                                }
                                None => {
                                    *voice = Voice::Idle;
                                    free_slots.push(index as u16);
                                    break;
                                }
                            }
                        } else if sampled.loop_index != current_loop_index
                            || sampled.external_release != current_external_id
                        {
                            current_loop_index = sampled.loop_index;
                            current_external_id = sampled.external_release;
                            external = current_external_id.and_then(|id| bank.get(id));
                            ctx = sampled.block_context(current, external, rate_scale);
                            ctx.lite = lite;
                    ctx.output_sr = output_sr;
                        }
                        match sampled.tick(
                            current,
                            external,
                            sinc,
                            &ctx,
                            *crossfade_step,
                            *kill_step,
                        ) {
                            Some((mut left, mut right)) => {
                                // Track the voice's own loudness (pre-
                                // gain) for release level matching.
                                sampled.envelope += *envelope_step
                                    * ((left.abs() + right.abs()) * 0.5 - sampled.envelope);
                                if tilting {
                                    let lp = &mut sampled.lowpass;
                                    lp[0] += tilt_a * (left - lp[0]);
                                    lp[1] += tilt_a * (right - lp[1]);
                                    left = lp[0] + treble * (left - lp[0]);
                                    right = lp[1] + treble * (right - lp[1]);
                                }
                                if enclosed {
                                    if !lite {
                                        let lp = &mut sampled.enc_lowpass;
                                        lp[0] += sampled.enc_coeff * (left - lp[0]);
                                        lp[1] += sampled.enc_coeff * (right - lp[1]);
                                        left = lp[0] + sampled.enc_hi_gain * (left - lp[0]);
                                        right = lp[1] + sampled.enc_hi_gain * (right - lp[1]);
                                    }
                                    sampled.enc_gain += *enc_ramp
                                        * (sampled.enc_gain_target - sampled.enc_gain);
                                    left *= sampled.enc_gain;
                                    right *= sampled.enc_gain;
                                }
                                scratch[frame * 2] += left * gain;
                                scratch[frame * 2 + 1] += right * gain;
                            }
                            None => {
                                *voice = Voice::Idle;
                                free_slots.push(index as u16);
                                break;
                            }
                        }
                    }
                }
            }
        }

        // Buses: insert effects, then land each on its output pair.
        for bus in buses.iter_mut() {
            bus.finish_chunk(frames, buffer, channels);
        }

        // Room: convolution reverb over the summed mix (wet trails dry
        // by one internal block; see reverb.rs).
        if let Some(reverb) = &mut self.reverb {
            reverb.process(buffer, channels);
        }

        // Master limiter: instant attack, exponential release. Bit-exact
        // passthrough while the bus stays under the ceiling.
        for frame in buffer.chunks_mut(channels) {
            let mut peak = 0.0f32;
            for value in frame.iter() {
                peak = peak.max(value.abs());
            }
            self.limiter_envelope =
                peak.max(self.limiter_envelope * self.limiter_release);
            if self.limiter_envelope > LIMITER_CEILING {
                let gain = LIMITER_CEILING / self.limiter_envelope;
                for value in frame.iter_mut() {
                    *value *= gain;
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
                brightness,
                enclosure,
                bus,
                delay_frames,
                nominal_hz,
            } => {
                let Some(start_position) = self
                    .bank
                    .get(sample)
                    .map(|s| s.attack_start() as f64)
                    .filter(|_| rate > 0.0)
                else {
                    return;
                };
                let enclosure = if (enclosure as usize) < MAX_ENCLOSURES {
                    enclosure
                } else {
                    ENCLOSURE_NONE
                };
                // Voices born inside a box start at the box's CURRENT
                // factors (starting at 1.0 would ramp every attack).
                let box_state = self
                    .enclosures
                    .get(enclosure as usize)
                    .copied()
                    .unwrap_or_default();
                // The pipe's own response to its chest. Sensitivity is
                // a fixed per-voice spread (±25 %, hashed from the
                // handle — no two pipes are voiced identically); the
                // lag time constants scale with the pipe's period:
                // pitch answers within a few periods, amplitude and
                // timbre only over the speech time. Unpitched voices
                // (nominal_hz = 0) take the chest unlagged.
                let (wind_sens, wind_pitch_rate, wind_gain_rate) = if nominal_hz > 0.0 {
                    let mut seed = (handle as u32).wrapping_mul(0x6C07_8965) | 1;
                    let hz = nominal_hz.clamp(16.0, 8000.0);
                    (
                        0.75 + 0.5 * wind::xorshift_unit(&mut seed),
                        1.0 / (4.0 / hz).clamp(0.004, 0.12),
                        1.0 / (25.0 / hz).clamp(0.02, 0.6),
                    )
                } else {
                    (1.0, f32::MAX, f32::MAX)
                };
                // Likewise a voice starts at its chest's current
                // factors, in case it is released before its first
                // Held block renders (frozen values must be real).
                let group = group.min(MAX_WIND_GROUPS as u8 - 1);
                let chest = &self.wind[group as usize];
                let (wind_rate, wind_gain, wind_treble) = (
                    1.0 + (chest.rate_factor() - 1.0) * wind_sens,
                    1.0 + (chest.gain_factor() - 1.0) * wind_sens,
                    (1.0 + (chest.brightness_factor() - 1.0) * wind_sens).clamp(0.75, 1.33),
                );
                if let Some(slot) = self.allocate_slot() {
                    self.voices[slot] = Voice::Sampled(SampledVoice {
                        handle,
                        sample,
                        position: start_position,
                        release_position: 0.0,
                        rate: rate as f64,
                        rate_target: rate as f64,
                        glide_frames: 0,
                        kernel: self.sinc.select(rate as f64),
                        gain,
                        fade: 0.0,
                        fade_step: 0.0,
                        tail_charge: 1.0,
                        tail_charge_deficit: 0.0,
                        tail_charge_step: 1.0,
                        release_bend: 0.0,
                        release_bend_depth: 0.0,
                        release_bend_step: 0.0,
                        amplitude: 1.0,
                        envelope: 0.0,
                        tail_gain: 1.0,
                        bus: bus.min(MAX_BUSES as u8 - 1),
                        // Onset delays are bounded only against nonsense
                        // (30 s covers any musical canon trick).
                        onset: delay_frames.min((30.0 * self.sample_rate) as u32),
                        group,
                        wind_weight: wind_weight.max(0.0),
                        wind_rate,
                        wind_gain,
                        wind_treble,
                        wind_sens,
                        wind_pitch_rate,
                        wind_gain_rate,
                        age_frames: 0,
                        tail_decay: 1.0,
                        fade_scale: 1.0,
                        pending_release: None,
                        wave_trem: self.wave_trems & (1u32 << u32::from(group).min(31)) != 0,
                        brightness_a: brightness.clamp(0.0, 1.0),
                        lowpass: [0.0; 2],
                        enclosure,
                        enc_gain: box_state.gain(),
                        enc_gain_target: box_state.gain(),
                        enc_hi_gain: box_state.hi_gain(),
                        enc_coeff: box_state.coeff(),
                        enc_lowpass: [0.0; 2],
                        wander: wind::Wander::default(),
                        rng: (handle as u32).wrapping_mul(0x9E37_79B9) | 1,
                        loop_index: 0,
                        external_release: None,
                        phase: SamplePhase::Held,
                        past_loop: false,
                    });
                }
            }
            Command::SetBusDelay { bus, params } => {
                if let Some(bus) = self.buses.get_mut(bus as usize) {
                    bus.set_delay(params);
                }
            }
            Command::SetBusOutput {
                bus,
                left,
                right,
                gain,
            } => {
                if let Some(bus) = self.buses.get_mut(bus as usize) {
                    bus.set_output(left, right, gain);
                }
            }
            Command::SetWind { group, params } => {
                if let Some(wind) = self.wind.get_mut(group as usize) {
                    wind.set_params(params);
                }
            }
            Command::SetTremulantParams { group, params } => {
                if let Some(wind) = self.wind.get_mut(group as usize) {
                    wind.set_tremulant_params(params);
                }
            }
            Command::SetTremulant { group, engaged } => {
                if let Some(wind) = self.wind.get_mut(group as usize) {
                    wind.set_tremulant(engaged);
                }
            }
            Command::SetWaveTremulant { group, engaged } => {
                let bit = 1u32 << u32::from(group).min(31);
                if engaged {
                    self.wave_trems |= bit;
                } else {
                    self.wave_trems &= !bit;
                }
                // Held voices follow the switch so their eventual
                // release matches the state at key-off; tails keep the
                // state they released under.
                for voice in self.voices.iter_mut() {
                    if let Voice::Sampled(sampled) = voice {
                        if sampled.group == group && sampled.phase == SamplePhase::Held {
                            sampled.wave_trem = engaged;
                        }
                    }
                }
            }
            Command::SetEnclosure { enclosure, params } => {
                if let Some(box_state) = self.enclosures.get_mut(enclosure as usize) {
                    box_state.set_params(params);
                }
            }
            Command::SetEnclosurePosition {
                enclosure,
                position,
            } => {
                if let Some(box_state) = self.enclosures.get_mut(enclosure as usize) {
                    box_state.set_target(position);
                }
            }
            Command::SetVoiceRate {
                handle,
                rate,
                glide_ms,
            } => {
                if !(rate > 0.0) || !glide_ms.is_finite() {
                    return;
                }
                let glide_frames = (glide_ms.max(0.0) * 0.001 * self.sample_rate) as u32;
                for voice in self.voices.iter_mut() {
                    if let Voice::Sampled(sampled) = voice {
                        if sampled.handle == handle {
                            sampled.rate_target = rate as f64;
                            sampled.glide_frames = glide_frames;
                            if glide_frames == 0 {
                                sampled.rate = rate as f64;
                                sampled.kernel = self.sinc.select(sampled.rate);
                            }
                        }
                    }
                }
            }
            Command::StopVoice { handle } => {
                // Batched: applied in one pool pass at the top of the
                // next process() (same block — commands drain first).
                if self.stop_batch.len() < self.stop_batch.capacity() {
                    self.stop_batch.push(handle);
                }
            }
            Command::KillVoice { handle } => {
                for voice in self.voices.iter_mut() {
                    if let Voice::Sampled(sampled) = voice {
                        if sampled.handle == handle && sampled.phase != SamplePhase::FadeOut {
                            sampled.amplitude = 1.0;
                            sampled.fade_scale = 1.0;
                            sampled.phase = SamplePhase::FadeOut;
                        }
                    }
                }
            }
            Command::NoteOn { key, freq_hz } => {
                if let Some(slot) = self.allocate_slot() {
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
                                sampled.fade_scale = 1.0;
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
            Command::SetReverbWet { wet } => {
                if let Some(reverb) = &mut self.reverb {
                    reverb.set_wet(wet);
                }
            }
        }
    }

    /// Maximum random pallet-close delay on key release (default 8 ms;
    /// 0 = releases fire on the exact command frame, used by tests).
    pub fn set_release_stagger(&mut self, seconds: f32) {
        self.release_stagger_frames = (seconds.clamp(0.0, 0.05)) * self.sample_rate;
    }

    /// Enable safe mode (see the `lite` field). Control-side only.
    pub fn set_lite(&mut self, on: bool) {
        self.lite = on;
    }

    /// Install the diagnostic output tap. Control-side only — call
    /// before the engine moves into the audio callback.
    pub fn set_tap(&mut self, tap: Producer<f32>) {
        self.tap = Some(tap);
    }

    /// Install a convolution reverb. Control-side only — call before
    /// the engine moves into the audio callback.
    pub fn set_reverb(&mut self, ir: Option<Arc<reverb::PreparedIr>>, wet: f32) {
        self.reverb = ir.map(|ir| reverb::Reverb::new(ir, wet));
    }

    /// Diagnostics: validate the free-slot invariant (every listed slot
    /// is Idle, no duplicates) — slot corruption resurrects old voices.
    /// Not for the audio thread; used by tests and debug tooling.
    /// Current limiter gain in dB (0 = passthrough, negative = actively
    /// reducing). Diagnostic + meter feed; safe to read between blocks.
    pub fn limiter_gain_db(&self) -> f32 {
        if self.limiter_envelope > LIMITER_CEILING {
            20.0 * (LIMITER_CEILING / self.limiter_envelope).log10()
        } else {
            0.0
        }
    }

    pub fn assert_slot_invariants(&self) {
        let mut seen = std::collections::HashSet::new();
        for &slot in &self.free_slots {
            assert!(seen.insert(slot), "slot {slot} in free list twice");
            assert!(
                matches!(self.voices[slot as usize], Voice::Idle),
                "slot {slot} in free list but voice not idle"
            );
        }
        // And the converse: every idle voice is findable.
        let idle = self
            .voices
            .iter()
            .filter(|v| matches!(v, Voice::Idle))
            .count();
        assert_eq!(idle, self.free_slots.len(), "idle voices lost from free list");
    }

    /// Current shutter position of an enclosure (diagnostics and tests).
    pub fn enclosure_position(&self, index: usize) -> f32 {
        self.enclosures
            .get(index)
            .map(|e| e.position())
            .unwrap_or(1.0)
    }

    /// Current pressure of a wind group (diagnostics and tests).
    pub fn wind_pressure(&self, group: usize) -> f32 {
        self.wind
            .get(group)
            .map(|w| w.pressure())
            .unwrap_or(1.0)
    }

    /// A free slot in O(1); with the pool exhausted, steal a voice
    /// already on its way out (rare — the tail budget keeps headroom).
    fn allocate_slot(&mut self) -> Option<usize> {
        if let Some(index) = self.free_slots.pop() {
            return Some(index as usize);
        }
        self.voices.iter().position(|voice| {
            matches!(
                voice,
                Voice::Sampled(s) if matches!(s.phase, SamplePhase::Tail | SamplePhase::FadeOut)
            )
        })
    }
}

#[cfg(test)]
mod tests;
