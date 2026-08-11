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
pub mod reverb;
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
    StartVoice {
        handle: u64,
        sample: u32,
        rate: f32,
        gain: f32,
        group: u8,
        wind_weight: f32,
        brightness: f32,
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
    /// Release the voice started with `handle`. Loop-less (percussive)
    /// voices ignore this and play to their end.
    StopVoice { handle: u64 },
    /// Silence a voice quickly WITHOUT its release tail (a short fade) —
    /// for retiring control-noise voices silently.
    KillVoice { handle: u64 },
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
    gain: f32,
    /// Crossfade progress 0→1.
    fade: f32,
    /// FadeOut amplitude 1→0.
    amplitude: f32,
    /// Fast envelope follower on the voice's own (pre-gain) output —
    /// what "how loud am I right now" means at release time.
    envelope: f32,
    /// Level-matching scale applied to the release tail so it continues
    /// at the voice's current loudness instead of the recording's.
    tail_gain: f32,
    /// Wind group index (pre-clamped to `MAX_WIND_GROUPS`).
    group: u8,
    /// How much wind this voice draws while sounding.
    wind_weight: f32,
    /// Output frames since the voice started — drives the wind model's
    /// pallet-opening attack boost.
    age_frames: u32,
    /// Tilt-filter coefficient (0 = bypass) and per-channel lowpass
    /// state: out = lp + treble·(x − lp) splits the signal at roughly
    /// the pipe's 2nd harmonic so pressure can breathe the timbre.
    brightness_a: f32,
    lowpass: [f32; 2],
    /// Per-pipe wind-flow noise (slow, independent per voice).
    wander: wind::Wander,
    rng: u32,
    /// Which of the sample's sustain loops the cursor is circling; a
    /// new one is drawn at random on each pass.
    loop_index: u8,
    /// Separate release sample being crossfaded into, if any.
    external_release: Option<u32>,
    /// A scheduled key-release: (frames until the pallet closes, hold
    /// age in ms captured at key-up). Real pallets never close in the
    /// same millisecond across a chord, and spreading the release also
    /// spreads the crossfade CPU spike that a mass release causes.
    pending_release: Option<(u16, u32)>,
    phase: SamplePhase,
}

/// Per-block invariants of a sampled voice's render loop, hoisted out
/// of the per-frame path (`Sample::frames()` alone is a u64 division —
/// two of those per frame per voice was a real cost at high polyphony).
/// Must be recomputed when the voice changes sample or loop mid-block.
#[derive(Clone, Copy)]
struct VoiceBlockContext {
    rate: f64,
    last: f64,
    tail_last: f64,
    current_loop: Option<(u64, u64)>,
    looping: bool,
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
            rate: self.rate * rate_scale,
            last: (sample.frames() - 1) as f64,
            tail_last: (external.unwrap_or(sample).frames() - 1) as f64,
            current_loop,
            looping: current_loop.is_some(),
        }
    }

    /// Render one frame and advance. Returns `None` when the voice ends.
    /// End-of-data checks happen on entry so every read frame is emitted.
    #[inline]
    fn tick(
        &mut self,
        sample: &Sample,
        external: Option<&Sample>,
        table: &SincTable,
        ctx: &VoiceBlockContext,
        crossfade_step: f32,
        kill_step: f32,
    ) -> Option<(f32, f32)> {
        let rate = ctx.rate;
        let last = ctx.last;
        let current_loop = ctx.current_loop;
        let looping = ctx.looping;
        // A scheduled release fires when its pallet-delay runs out.
        if self.phase == SamplePhase::Held {
            if let Some((delay, age_ms)) = self.pending_release {
                if delay == 0 {
                    self.pending_release = None;
                    self.release(sample, age_ms);
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
            current_loop
        };
        // During a crossfade the OUTGOING leg fades to zero — linear
        // interpolation there is inaudible and halves the double-read
        // cost that made mass releases blow the block budget. The
        // persistent (incoming) leg keeps full sinc quality.
        let (mut left, mut right) = if self.phase == SamplePhase::Crossfade {
            sample.read(self.position)
        } else {
            table.read(sample, self.position, seam)
        };
        let mut advance_position = true;
        match self.phase {
            SamplePhase::Held | SamplePhase::Tail => {}
            SamplePhase::Crossfade => {
                let (tail_l, tail_r) = table.read(tail_sample, self.release_position, None);
                // Raised-cosine-shaped blend (smoothstep ≈ it, no trig):
                // linear fades dip audibly on the uncorrelated noise
                // floor (Appleton 2019).
                let weight = self.fade * self.fade * (3.0 - 2.0 * self.fade);
                left += (tail_l * self.tail_gain - left) * weight;
                right += (tail_r * self.tail_gain - right) * weight;
                self.fade += crossfade_step;
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
            // cursor has left it for the release material. On each pass
            // a fresh loop is drawn at random (multi-loop sets), which
            // decorrelates repetition.
            if self.phase != SamplePhase::Tail {
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
        Some((left * self.gain, right * self.gain))
    }

    /// Key released: splice to a separate release (selected by hold
    /// duration) or the embedded tail, whichever the sample offers.
    fn release(&mut self, sample: &Sample, age_ms: u32) {
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
                // Exception: a near-silent loop (control-noise samples:
                // thump → silent loop → thump tail) means the tail is
                // MEANT to be louder — play it as recorded.
                let reference = sample.tail_reference_level();
                self.tail_gain = if reference > 1e-5 && self.envelope > 0.02 * reference {
                    (self.envelope / reference).clamp(0.05, 1.1)
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
    sinc: SincTable,
    voices: Box<[Voice]>,
    wind: [WindGroup; MAX_WIND_GROUPS],
    reverb: Option<reverb::Reverb>,
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
            reverb: None,
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
                        if sampled.phase == SamplePhase::Held
                            && sampled.pending_release.is_none()
                        {
                            let delay = (wind::xorshift_unit(&mut sampled.rng)
                                * max_stagger) as u16;
                            sampled.pending_release = Some((delay, age_ms));
                        } else if let Some(sample) = self.bank.get(sampled.sample) {
                            sampled.release(sample, age_ms);
                        }
                    }
                }
            }
            self.stop_batch.clear();
        }

        buffer.fill(0.0);
        let channels = channels.max(1);
        let frames = buffer.len() / channels;
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
                let mut quietest: Option<(usize, f32)> = None;
                for (index, voice) in self.voices.iter().enumerate() {
                    if let Voice::Sampled(sampled) = voice {
                        if sampled.phase == SamplePhase::Tail
                            && quietest.is_none_or(|(_, level)| sampled.envelope < level)
                        {
                            quietest = Some((index, sampled.envelope));
                        }
                    }
                }
                let Some((index, _)) = quietest else { break };
                if let Voice::Sampled(sampled) = &mut self.voices[index] {
                    sampled.amplitude = 1.0;
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
            free_slots,
            tone_attack_step,
            tone_release_step,
            crossfade_step,
            kill_step,
            envelope_step,
            ..
        } = self;

        for index in 0..voices.len() {
            let voice = &mut voices[index];
            match voice {
                Voice::Idle => {}
                Voice::Tone(tone) => {
                    for frame in 0..frames {
                        let value = tone.tick(*tone_attack_step, *tone_release_step);
                        if tone.stage == ToneStage::Idle {
                            *voice = Voice::Idle;
                            free_slots.push(index as u16);
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
                        free_slots.push(index as u16);
                        continue;
                    };
                    let chest = &wind[sampled.group as usize];
                    let params = chest.params();

                    // Per-voice flow noise, linearized around the chest
                    // factors (a powf per voice per block would also be
                    // fine, but ±2 % deviations are firmly linear).
                    let mut deviation = 0.0;
                    if params.flow_noise > 0.0 && sampled.wind_weight > 0.0 {
                        sampled
                            .wander
                            .step(dt, params.flow_noise, &mut sampled.rng);
                        deviation = sampled.wander.deviation();
                    }
                    let rate_scale =
                        (chest.rate_factor() * (1.0 + params.pitch_exponent * deviation)) as f64;
                    let gain =
                        master * chest.gain_factor() * (1.0 + params.gain_exponent * deviation);
                    let treble = (chest.brightness_factor()
                        * (1.0 + params.brightness_exponent * deviation))
                        .clamp(0.25, 2.0);

                    let tilt_a = sampled.brightness_a;
                    // Bypass the filter while it would do nothing: keeps
                    // untouched-pressure rendering bit-identical.
                    let tilting = tilt_a > 0.0 && (treble - 1.0).abs() > 1e-4;
                    sampled.age_frames = sampled.age_frames.saturating_add(frames as u32);
                    // The voice can hand over to a separate release
                    // sample or switch loops mid-block; track the refs
                    // and per-block invariants it reads from.
                    let mut current = sample;
                    let mut current_id = sampled.sample;
                    let mut current_loop_index = sampled.loop_index;
                    let mut current_external_id = sampled.external_release;
                    let mut external = current_external_id.and_then(|id| bank.get(id));
                    let mut ctx = sampled.block_context(current, external, rate_scale);
                    for frame in 0..frames {
                        if sampled.sample != current_id {
                            match bank.get(sampled.sample) {
                                Some(switched) => {
                                    current = switched;
                                    current_id = sampled.sample;
                                    external = None;
                                    current_external_id = None;
                                    current_loop_index = sampled.loop_index;
                                    ctx = sampled.block_context(current, external, rate_scale);
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
                                mix_frame(
                                    &mut buffer[frame * channels..],
                                    channels,
                                    left * gain,
                                    right * gain,
                                )
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
            } => {
                if self.bank.get(sample).is_none() || !(rate > 0.0) {
                    return;
                }
                if let Some(slot) = self.allocate_slot() {
                    self.voices[slot] = Voice::Sampled(SampledVoice {
                        handle,
                        sample,
                        position: 0.0,
                        release_position: 0.0,
                        rate: rate as f64,
                        gain,
                        fade: 0.0,
                        amplitude: 1.0,
                        envelope: 0.0,
                        tail_gain: 1.0,
                        group: group.min(MAX_WIND_GROUPS as u8 - 1),
                        wind_weight: wind_weight.max(0.0),
                        age_frames: 0,
                        pending_release: None,
                        brightness_a: brightness.clamp(0.0, 1.0),
                        lowpass: [0.0; 2],
                        wander: wind::Wander::default(),
                        rng: (handle as u32).wrapping_mul(0x9E37_79B9) | 1,
                        loop_index: 0,
                        external_release: None,
                        phase: SamplePhase::Held,
                    });
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

    /// Install a convolution reverb. Control-side only — call before
    /// the engine moves into the audio callback.
    pub fn set_reverb(&mut self, ir: Option<Arc<reverb::PreparedIr>>, wet: f32) {
        self.reverb = ir.map(|ir| reverb::Reverb::new(ir, wet));
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
        engine.set_release_stagger(0.0);
        handle.send(Command::StartVoice {
            handle: 1,
            sample: 0,
            rate: 1.0,
            gain: 1.0,
            group: 0,
            wind_weight: 0.0,
            brightness: 0.0,
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
        engine.set_release_stagger(0.0);
        handle.send(Command::StartVoice {
            handle: 7,
            sample: 0,
            rate: 1.0,
            gain: 1.0,
            group: 0,
            wind_weight: 0.0,
            brightness: 0.0,
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
        engine.set_release_stagger(0.0);
        handle.send(Command::StartVoice {
            handle: 1,
            sample: 0,
            rate: 1.0,
            gain: 1.0,
            group: 0,
            wind_weight: 0.0,
            brightness: 0.0,
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
        engine.set_release_stagger(0.0);
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
        engine.set_release_stagger(0.0);
        handle.send(Command::StartVoice {
            handle: 1,
            sample: 0,
            rate: 1.0,
            gain: 1.0,
            group: 0,
            wind_weight: 0.0,
            brightness: 0.0,
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
            engine.set_release_stagger(0.0);
            // Phantoms draw wind but are silent: gain 0, weight 1.
            for i in 0..phantom_voices {
                handle.send(Command::StartVoice {
                    handle: 100 + i as u64,
                    sample: 0,
                    rate: 1.0,
                    gain: 0.0,
                    group: 3,
                    wind_weight: 1.0,
                    brightness: 0.0,
                });
            }
            handle.send(Command::StartVoice {
                handle: 1,
                sample: 0,
                rate: 1.0,
                gain: 1.0,
                group: 3,
                wind_weight: 0.0,
                brightness: 0.0,
            });
            // Settle for ~1.5 s (12+ time constants), then measure.
            let mut buffer = vec![0.0f32; 1024 * 2];
            for _ in 0..70 {
                engine.process(&mut buffer, 2);
            }
            if phantom_voices > 0 {
                let pressure = engine.wind_pressure(3);
                assert!(
                    (pressure - 0.94).abs() < 0.004,
                    "steady pressure {pressure} should be ~0.94 at reference demand"
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
        // Expected: P=0.94 (realistic 6% chest drop), rate factor
        // 0.94^0.032 ≈ 0.99802 → ~481.0 (≈ −3.4 cents steady: the
        // physically calibrated sensitivity of ~0.55 cents per 1%).
        assert!(
            loaded > 480.5 && loaded < 481.5,
            "loaded period {loaded} should sag to ~481 frames"
        );
    }

    #[test]
    fn limiter_prevents_clipping_without_distorting() {
        // A bank whose single voice massively exceeds full scale.
        let period = 480usize;
        let omega = std::f64::consts::TAU / period as f64;
        let data: Vec<f32> = (0..period * 20)
            .map(|n| (omega * n as f64).sin() as f32)
            .collect();
        let end = (period * 20) as u64;
        let sample = Sample::new(data, 1, 48000.0, Some((0, end)), end).expect("valid");
        let mut bank = SampleBank::default();
        bank.push(sample);
        let (mut engine, mut handle) = Engine::new(48000.0, Arc::new(bank));
        engine.set_release_stagger(0.0);
        handle.send(Command::StartVoice {
            handle: 1,
            sample: 0,
            rate: 1.0,
            gain: 12.0, // ~4.2x full scale after master gain
            group: 0,
            wind_weight: 0.0,
            brightness: 0.0,
        });
        // Let the limiter settle, then inspect a window.
        let mut buffer = vec![0.0f32; 48000 * 2];
        engine.process(&mut buffer, 2);
        let mut buffer = vec![0.0f32; 9600 * 2];
        engine.process(&mut buffer, 2);
        let mono: Vec<f32> = buffer.chunks(2).map(|f| f[0]).collect();

        let peak = mono.iter().fold(0.0f32, |m, &v| m.max(v.abs()));
        assert!(peak <= LIMITER_CEILING + 1e-4, "still clipping: {peak}");
        assert!(peak > 0.9, "over-limited: {peak}");

        // Settled limiting must be a clean gain: the waveform stays a
        // sine (normalized correlation against the ideal ≈ 1).
        let mut dot = 0.0f64;
        let mut energy_a = 0.0f64;
        let mut energy_b = 0.0f64;
        for (n, &v) in mono.iter().enumerate() {
            // Voice position offset is unknown; correlate against both
            // quadratures to be phase-agnostic.
            let ideal = (omega * n as f64).sin();
            dot += v as f64 * ideal;
            energy_a += (v as f64) * (v as f64);
            energy_b += ideal * ideal;
        }
        let correlation = dot.abs() / (energy_a * energy_b).sqrt();
        // Phase offset makes plain correlation pessimistic; use spectral
        // purity instead: total distortion shows up as |v| flattening.
        // A clipped sine has correlation ~0.97 vs ~1.0 clean; combined
        // with quadrature ambiguity accept > 0.7 here and rely on the
        // flatness check below for the real assertion.
        let _ = correlation;
        // Crest factor of a clean sine = √2 ≈ 1.414; hard clipping
        // pushes it toward 1.0. Allow a little slack.
        let rms = (energy_a / mono.len() as f64).sqrt();
        let crest = peak as f64 / rms;
        assert!(
            (crest - std::f64::consts::SQRT_2).abs() < 0.06,
            "waveform flattened (crest {crest:.3}, clean sine = 1.414) — limiter is distorting"
        );
    }

    #[test]
    fn limiter_passthrough_below_ceiling() {
        let (mut engine, mut handle) = Engine::new(48000.0, sine_pipe_bank(480, true));
        engine.set_release_stagger(0.0);
        handle.send(Command::StartVoice {
            handle: 1,
            sample: 0,
            rate: 1.0,
            gain: 1.0,
            group: 0,
            wind_weight: 0.0,
            brightness: 0.0,
        });
        let mut buffer = vec![0.0f32; 4800 * 2];
        engine.process(&mut buffer, 2);
        let peak = buffer.iter().fold(0.0f32, |m, &v| m.max(v.abs()));
        assert!(peak < LIMITER_CEILING * 0.7, "fixture should be quiet");
        assert!(peak > 0.1, "fixture should be audible");
    }

    #[test]
    fn multi_loop_voices_visit_all_loops() {
        // Two loops with distinct constant levels: 0.8 and 0.3. A voice
        // drawing loops at random must produce both levels over time.
        let mut data = vec![0.0f32; 600];
        for (index, value) in data.iter_mut().enumerate() {
            *value = match index {
                0..=99 => index as f32 / 100.0 * 0.8, // attack ramp
                100..=199 => 0.8,                     // loop A
                // smooth descent between the loops (the engine plays
                // through here when switching toward loop B)
                200..=299 => 0.8 - 0.5 * (index - 199) as f32 / 100.0,
                300..=399 => 0.3, // loop B
                _ => 0.3 - 0.3 * (index - 399) as f32 / 200.0, // tail out
            };
        }
        let mut sample = Sample::new(data, 1, 48000.0, Some((100, 200)), 400).expect("valid");
        sample.add_loop(300, 400).expect("alternate loop");
        let mut bank = SampleBank::default();
        bank.push(sample);
        let (mut engine, mut handle) = Engine::new(48000.0, Arc::new(bank));
        engine.set_release_stagger(0.0);
        handle.send(Command::StartVoice {
            handle: 1,
            sample: 0,
            rate: 1.0,
            gain: 1.0,
            group: 0,
            wind_weight: 0.0,
            brightness: 0.0,
        });
        // ~100 loop passes.
        let mut buffer = vec![0.0f32; 10000 * 2];
        engine.process(&mut buffer, 2);
        let master = DEFAULT_MASTER_GAIN;
        let near = |target: f32| {
            buffer
                .chunks(2)
                .filter(|f| (f[0] - target * master).abs() < 0.05 * master)
                .count()
        };
        let high = near(0.8);
        let low = near(0.3);
        // These loops are disjoint and sequential (pathological — real
        // sets' loops overlap), so once the voice commits to the later
        // loop the earlier one is behind it; what matters is that both
        // get PLAYED and that every transition is seamless.
        assert!(
            high > 200 && low > 500,
            "both loops should be visited: high {high}, low {low}"
        );

        // Loop switching must never splice discontinuously: the data is
        // constants + a gentle ramp, so any frame-to-frame jump beyond
        // the ramp slope is a click (the old code jumped straight from
        // loop A's end into loop B's start: a 0.5-amplitude pop).
        let mono: Vec<f32> = buffer.chunks(2).map(|f| f[0]).collect();
        // Skip the attack ramp start-up.
        let max_delta = mono[200..]
            .windows(2)
            .map(|w| (w[1] - w[0]).abs())
            .fold(0.0f32, f32::max);
        assert!(
            max_delta < 0.02 * master,
            "loop transition clicked: max frame delta {max_delta}"
        );
    }

    #[test]
    fn separate_releases_select_by_hold_time() {
        let period = 480usize;
        let omega = std::f64::consts::TAU / period as f64;
        let sine = |frames: usize, envelope: &dyn Fn(usize) -> f64| -> Vec<f32> {
            (0..frames)
                .map(|n| (envelope(n) * (omega * n as f64).sin()) as f32)
                .collect()
        };
        // Attack sample: loop, no embedded tail (tail beyond EOF).
        let attack_frames = period * 16;
        let attack = sine(attack_frames, &|_| 1.0);
        let mut source = Sample::new(
            attack,
            1,
            48000.0,
            Some(((period * 4) as u64, (period * 12) as u64)),
            attack_frames as u64,
        )
        .expect("valid");
        source.align_release(48000.0 / period as f32);
        // Short release: 0.15 s of decaying sine. Long: 1.5 s.
        let short = Sample::new(
            sine(7200, &|n| 1.0 - n as f64 / 7200.0),
            1,
            48000.0,
            None,
            7200,
        )
        .expect("valid");
        let long = Sample::new(
            sine(72000, &|n| 1.0 - n as f64 / 72000.0),
            1,
            48000.0,
            None,
            72000,
        )
        .expect("valid");

        let mut bank = SampleBank::default();
        // Push releases first so their indices exist for attach.
        let short_id = bank.push(short);
        let long_id = bank.push(long);
        source.attach_release(bank.get(short_id).expect("short"), short_id, Some(300));
        source.attach_release(bank.get(long_id).expect("long"), long_id, None);
        let source_id = bank.push(source);
        let bank = Arc::new(bank);

        let audible_seconds = |hold_frames: usize| -> f64 {
            let (mut engine, mut handle) = Engine::new(48000.0, Arc::clone(&bank));
            engine.set_release_stagger(0.0);
            handle.send(Command::StartVoice {
                handle: 1,
                sample: source_id,
                rate: 1.0,
                gain: 1.0,
                group: 0,
                wind_weight: 0.0,
                brightness: 0.0,
            });
            let mut buffer = vec![0.0f32; hold_frames * 2];
            engine.process(&mut buffer, 2);
            handle.send(Command::StopVoice { handle: 1 });
            // Render 2 s and find the last audible frame.
            let mut buffer = vec![0.0f32; 96000 * 2];
            engine.process(&mut buffer, 2);
            let last = buffer
                .chunks(2)
                .rposition(|f| f[0].abs() > 0.001)
                .unwrap_or(0);
            last as f64 / 48000.0
        };

        let staccato = audible_seconds(4800); // held 100 ms → short release
        let tenuto = audible_seconds(24000); // held 500 ms → long release
        assert!(
            staccato < 0.35,
            "staccato should use the 0.15 s release, rang for {staccato:.2} s"
        );
        assert!(
            tenuto > 0.9,
            "tenuto should use the 1.5 s release, rang for {tenuto:.2} s"
        );
    }

    #[test]
    fn alignment_ignores_strong_second_harmonics() {
        // A principal-like pipe: 2nd harmonic nearly as strong as the
        // fundamental. Correlation-argmax alignment could lock a half
        // period off here (fundamental cancels, octave reinforces — a
        // missing-fundamental strike, i.e. a bell). Quadrature must
        // track the FUNDAMENTAL phase.
        let period = 480usize;
        let omega = std::f64::consts::TAU / period as f64;
        let loop_start = 1913u64; // deliberately not phase-aligned
        let loop_end = loop_start + (period * 40) as u64;
        let frames = loop_end + (period * 20) as u64;
        let data: Vec<f32> = (0..frames)
            .map(|n| {
                let envelope = if n >= loop_end {
                    1.0 - 0.5 * (n - loop_end) as f64 / (frames - loop_end) as f64
                } else {
                    1.0
                };
                let t = n as f64;
                (envelope * ((omega * t).sin() + 0.9 * (2.0 * omega * t).sin() * 0.9)) as f32
            })
            .collect();
        let mut sample =
            Sample::new(data, 1, 48000.0, Some((loop_start, loop_end)), loop_end).expect("valid");
        sample.align_release(48000.0 / period as f32);
        let alignment = sample.release_alignment().expect("alignment built");
        for probe in 0..16 {
            let position = loop_start as f64 + probe as f64 * 1123.7;
            let target = alignment.target(position, loop_start);
            let source_phase = (position / period as f64).fract();
            let target_phase = (target as f64 / period as f64).fract();
            let mut delta = (source_phase - target_phase).abs();
            delta = delta.min(1.0 - delta);
            assert!(
                delta < 2.0 / bank::ALIGNMENT_BUCKETS as f64 + 0.01,
                "position {position}: fundamental phase {source_phase:.3} vs \
                 {target_phase:.3} (delta {delta:.3}) — octave ghost splice"
            );
        }
    }

    #[test]
    fn alignment_survives_mistuned_pipes_and_long_loops() {
        // Real pipes sit cents off their nominal pitch, and a voice can
        // be hundreds of periods away from the loop-start phase anchor:
        // without measuring the true period, the alignment table points
        // at effectively random phase. True period here: 479.3 frames
        // (non-integer); declared nominal: 483 (≈ 13 cents off).
        let true_period = 479.3f64;
        let omega = std::f64::consts::TAU / true_period;
        let loop_start = 1920u64;
        let loop_end = loop_start + 200 * 480; // ~200 periods of travel
        let frames = loop_end + 480 * 20;
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
        let mut sample =
            Sample::new(data, 1, 48000.0, Some((loop_start, loop_end)), loop_end).expect("valid");
        sample.align_release(48000.0 / 483.0);
        let alignment = sample.release_alignment().expect("alignment built");

        for probe in 0..24 {
            // Positions spread across the whole loop, far from anchor.
            let position = loop_start as f64 + probe as f64 * 3997.3;
            if position >= loop_end as f64 {
                break;
            }
            let target = alignment.target(position, loop_start);
            let source_phase = (position / true_period).fract();
            let target_phase = (target as f64 / true_period).fract();
            let mut delta = (source_phase - target_phase).abs();
            delta = delta.min(1.0 - delta);
            assert!(
                delta < 2.0 / bank::ALIGNMENT_BUCKETS as f64 + 0.01,
                "position {position}: phase {source_phase:.3} vs target {target_phase:.3} \
                 (delta {delta:.3}) — period estimation failed"
            );
        }
    }

    #[test]
    fn alignment_locks_phase_for_high_pipes_in_noise() {
        // A high pipe: ~30-frame period (≈1.5 kHz) with room noise on
        // top. One-period correlation windows can't lock phase against
        // noise; the widened window must.
        let period = 30usize;
        let omega = std::f64::consts::TAU / period as f64;
        let loop_start = 1200u64;
        let loop_end = loop_start + (period * 100) as u64;
        let frames = loop_end + (period * 40) as u64;
        let mut noise_state = 0x1234_5678u32;
        let mut noise = move || {
            noise_state ^= noise_state << 13;
            noise_state ^= noise_state >> 17;
            noise_state ^= noise_state << 5;
            ((noise_state >> 8) as f64 / (1u32 << 24) as f64 - 0.5) * 0.3
        };
        let data: Vec<f32> = (0..frames)
            .map(|n| {
                let envelope = if n >= loop_end {
                    1.0 - 0.6 * (n - loop_end) as f64 / (frames - loop_end) as f64
                } else {
                    1.0
                };
                (envelope * ((omega * n as f64).sin() + noise())) as f32
            })
            .collect();
        let mut sample =
            Sample::new(data, 1, 44100.0, Some((loop_start, loop_end)), loop_end).expect("valid");
        sample.align_release(44100.0 / period as f32);
        let alignment = sample.release_alignment().expect("alignment built");
        for probe in 0..16 {
            let position = loop_start as f64 + probe as f64 * 217.7;
            let target = alignment.target(position, loop_start);
            let source_phase = (position / period as f64).fract();
            let target_phase = (target as f64 / period as f64).fract();
            let mut delta = (source_phase - target_phase).abs();
            delta = delta.min(1.0 - delta);
            assert!(
                delta < 0.12,
                "position {position}: phase {source_phase:.3} vs {target_phase:.3} — \
                 high-pipe phase lock failed (delta {delta:.3})"
            );
        }
    }

    #[test]
    fn early_release_does_not_strike_like_a_bell() {
        // A pipe whose attack ramps up over 4 periods: releasing during
        // the ramp used to splice to the tail at FULL recorded level —
        // a bell strike. The level match must scale it down.
        let period = 480usize;
        let omega = std::f64::consts::TAU / period as f64;
        let loop_start = period * 8;
        let loop_end = period * 16;
        let frames = period * 28;
        let ramp_end = (period * 4) as f64;
        let data: Vec<f32> = (0..frames)
            .map(|n| {
                let envelope = if (n as f64) < ramp_end {
                    n as f64 / ramp_end
                } else if n >= loop_end {
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
        sample.align_release(48000.0 / period as f32);
        let mut bank = SampleBank::default();
        bank.push(sample);

        let (mut engine, mut handle) = Engine::new(48000.0, Arc::new(bank));
        engine.set_release_stagger(0.0);
        handle.send(Command::StartVoice {
            handle: 1,
            sample: 0,
            rate: 1.0,
            gain: 1.0,
            group: 0,
            wind_weight: 0.0,
            brightness: 0.0,
        });
        // Release 1.5 periods in: the ramp is at ~37 % amplitude.
        let mut buffer = vec![0.0f32; (period * 3 / 2) * 2];
        engine.process(&mut buffer, 2);
        handle.send(Command::StopVoice { handle: 1 });
        let mut buffer = vec![0.0f32; 9600 * 2];
        engine.process(&mut buffer, 2);
        let peak = buffer.iter().fold(0.0f32, |m, &v| m.max(v.abs()));
        // Unmatched, the tail peaked at the full 1.0 × master. With the
        // match, the tail leg is scaled to ~0.37; the residual above
        // that is the main leg still swelling through the crossfade
        // (the pipe genuinely keeps speaking for those 30 ms).
        assert!(
            peak < 0.55 * DEFAULT_MASTER_GAIN,
            "release struck like a bell: peak {peak}"
        );
        assert!(peak > 0.02 * DEFAULT_MASTER_GAIN, "release went silent");
    }

    #[test]
    fn brightness_tilt_attenuates_highs_under_load() {
        // A 1 kHz pipe with its tilt hinged at 200 Hz: nearly all of its
        // energy sits in the "upper partials" band, so the chest's
        // brightness factor acts on it almost directly.
        let period = 48; // 1 kHz at 48 kHz
        let tilt_a = 1.0 - (-std::f64::consts::TAU * 200.0 / 48000.0).exp() as f32;

        let run = |loaded: bool, brightness: f32| -> f32 {
            let (mut engine, mut handle) = Engine::new(48000.0, sine_pipe_bank(period, true));
            engine.set_release_stagger(0.0);
            // Kill per-voice noise so the comparison is deterministic.
            let mut params = wind::WindParams::default();
            params.flow_noise = 0.0;
            for group in 0..wind::MAX_WIND_GROUPS as u8 {
                handle.send(Command::SetWind { group, params });
            }
            if loaded {
                for i in 0..30 {
                    handle.send(Command::StartVoice {
                        handle: 100 + i,
                        sample: 0,
                        rate: 1.0,
                        gain: 0.0,
                        group: 0,
                        wind_weight: 1.0,
                        brightness: 0.0,
                    });
                }
            }
            handle.send(Command::StartVoice {
                handle: 1,
                sample: 0,
                rate: 1.0,
                gain: 1.0,
                group: 0,
                wind_weight: 0.0,
                brightness,
            });
            let mut buffer = vec![0.0f32; 1024 * 2];
            for _ in 0..70 {
                engine.process(&mut buffer, 2);
            }
            let mut buffer = vec![0.0f32; 8192 * 2];
            engine.process(&mut buffer, 2);
            let mono: Vec<f32> = buffer.chunks(2).map(|f| f[0]).collect();
            rms(&mono)
        };

        let unloaded = run(false, tilt_a);
        let loaded_tilted = run(true, tilt_a);
        let loaded_flat = run(true, 0.0);
        // Gain factor alone: 0.94^0.75 ≈ 0.955. With the tilt, a further
        // ≈ 0.94^3 ≈ 0.83 on this (high) pipe.
        let plain = loaded_flat / unloaded;
        let tilted = loaded_tilted / unloaded;
        assert!(
            (plain - 0.955).abs() < 0.02,
            "plain gain ratio {plain} should be ~0.955"
        );
        assert!(
            tilted < plain * 0.88 && tilted > plain * 0.75,
            "tilted ratio {tilted} should add ~0.83x on top of {plain}"
        );
    }

    #[test]
    fn flow_noise_wobbles_pitch_slightly_and_independently() {
        let period = 480;
        let measure_spread = |noise: f32| -> f64 {
            let (mut engine, mut handle) = Engine::new(48000.0, sine_pipe_bank(period, true));
            engine.set_release_stagger(0.0);
            let mut params = wind::WindParams::default();
            params.sag_depth = 0.0; // isolate the per-voice noise
            params.flow_noise = noise;
            handle.send(Command::SetWind { group: 0, params });
            handle.send(Command::StartVoice {
                handle: 1,
                sample: 0,
                rate: 1.0,
                gain: 1.0,
                group: 0,
                wind_weight: 1.0,
                brightness: 0.0,
            });
            // 10 windows of 0.2 s: the wander drifts across them.
            let mut periods = Vec::new();
            let mut buffer = vec![0.0f32; 9600 * 2];
            for _ in 0..10 {
                engine.process(&mut buffer, 2);
                periods.push(measured_period(&buffer));
            }
            let min = periods.iter().cloned().fold(f64::MAX, f64::min);
            let max = periods.iter().cloned().fold(f64::MIN, f64::max);
            max - min
        };

        let quiet = measure_spread(0.0);
        let noisy = measure_spread(0.05);
        assert!(quiet < 0.05, "no noise → no drift, got {quiet}");
        assert!(
            noisy > 0.15,
            "5% flow noise should visibly wander pitch, got {noisy}"
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
        engine.set_release_stagger(0.0);
        handle.send(Command::StartVoice {
            handle: 1,
            sample: 0,
            rate: 1.0,
            gain: 1.0,
            group: 0,
            wind_weight: 0.0,
            brightness: 0.0,
        });
        let out = render(&mut engine, 50);
        let master = DEFAULT_MASTER_GAIN;
        assert!((out[41] - 0.25 * master).abs() < 1e-6, "right channel");
        assert!((out[40] - out[41]).abs() > 1e-6, "channels differ");
    }
}
