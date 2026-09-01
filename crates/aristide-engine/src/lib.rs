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
mod command;
pub mod enclosure;
pub mod resample;
pub mod reverb;
pub mod routing;
mod tone;
mod voice;
pub mod wind;

use std::sync::Arc;

use bank::SampleBank;
use command::COMMAND_QUEUE_CAPACITY;
use enclosure::{Enclosure, ENCLOSURE_NONE, MAX_ENCLOSURES};
use resample::SincTables;
use routing::{Bus, MAX_BUSES, MAX_CHUNK_FRAMES};
use rtrb::{Consumer, Producer, RingBuffer};
use tone::{ToneStage, ToneVoice, TONE_ATTACK_SECONDS, TONE_GAIN, TONE_RELEASE_SECONDS};
use voice::{
    Brightness, EnclosureState, PlaybackCursor, ReleaseState, SamplePhase, SampledVoice,
    Voice, WindState,
};
use wind::{WindGroup, MAX_WIND_GROUPS};

pub use command::{Command, EngineHandle};

pub const MAX_VOICES: usize = 2048;

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
                            sampled.release.amplitude = 0.0;
                        } else if sampled.phase == SamplePhase::Held
                            && sampled.release.pending.is_none()
                        {
                            let delay = (wind::xorshift_unit(&mut sampled.rng)
                                * max_stagger) as u16;
                            sampled.release.pending = Some((delay, age_ms));
                        } else if let Some(sample) = self.bank.get(sampled.cursor.sample) {
                            sampled.begin_release(sample, age_ms, self.sample_rate);
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
                    sampled.release.amplitude = 1.0;
                    // Gentle ~150 ms fade: under 128 masking tails this
                    // is inaudible; the 15 ms kill ramp is not.
                    sampled.release.fade_scale = 0.1;
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
                let params = self.wind[sampled.wind.group as usize].params();
                let attack_frames = params.attack_ms * 0.001 * self.sample_rate;
                let boost = if (sampled.age_frames as f32) < attack_frames {
                    params.attack_boost * (1.0 - sampled.age_frames as f32 / attack_frames)
                } else {
                    0.0
                };
                demand[sampled.wind.group as usize] += sampled.wind.weight * (1.0 + boost);
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
                    let Some(sample) = bank.get(sampled.cursor.sample) else {
                        *voice = Voice::Idle;
                        free_slots.push(index as u16);
                        continue;
                    };
                    // Wind factors follow the swell-box rule below: a
                    // Held voice re-reads its chest each block; a
                    // released voice keeps the factors frozen from its
                    // last Held block — the pallet is closed, the tail
                    // is room decay, and trem/pressure must not wobble
                    // it.
                    if !lite && sampled.phase == SamplePhase::Held {
                        let chest = &wind[sampled.wind.group as usize];
                        sampled.wind.follow_chest(chest, dt, &mut sampled.rng);
                    }                    let (rate_scale, gain, treble) = if lite {
                        (1.0f64, master, 1.0f32)
                    } else {
                        (
                            sampled.wind.rate as f64,
                            master * sampled.wind.gain,
                            sampled.wind.treble,
                        )
                    };

                    // Bypass the tilt filter while it would do nothing:
                    // keeps untouched-pressure rendering bit-identical.
                    let tilting = sampled.brightness.a > 0.0 && (treble - 1.0).abs() > 1e-4;

                    // Swell box: a Held voice tracks its box each block
                    // (gain ramped per frame — pedal sweeps would zipper
                    // otherwise); any released/fading voice keeps the
                    // factors frozen from its last Held block. Lite mode
                    // keeps the broadband gain (the pedal must still do
                    // something) and only skips the shutter filter.
                    let enclosed = sampled.enclosure.index != ENCLOSURE_NONE;
                    if enclosed && sampled.phase == SamplePhase::Held {
                        let box_state = &enclosures[sampled.enclosure.index as usize];
                        sampled.enclosure.follow_box(box_state);
                    }
                    let held = sampled.phase == SamplePhase::Held;
                    sampled.cursor.step_glide(frames, held, sinc);
                    sampled.age_frames = sampled
                        .age_frames
                        .saturating_add((frames - start_frame) as u32);
                    // The voice can hand over to a separate release
                    // sample or switch loops mid-block; track the refs
                    // and per-block invariants it reads from.
                    let mut current = sample;
                    let mut current_id = sampled.cursor.sample;
                    let mut current_loop_index = sampled.cursor.loop_index;
                    let mut current_external_id = sampled.cursor.external_release;
                    let mut external = current_external_id.and_then(|id| bank.get(id));
                    let mut ctx = sampled.block_context(current, external, rate_scale);
                    ctx.lite = lite;
                    ctx.output_sr = output_sr;
                    let scratch = buses[sampled.bus as usize].mix_target(frames);
                    for frame in start_frame..frames {
                        if sampled.cursor.sample != current_id {
                            match bank.get(sampled.cursor.sample) {
                                Some(switched) => {
                                    current = switched;
                                    current_id = sampled.cursor.sample;
                                    external = None;
                                    current_external_id = None;
                                    current_loop_index = sampled.cursor.loop_index;
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
                        } else if sampled.cursor.loop_index != current_loop_index
                            || sampled.cursor.external_release != current_external_id
                        {
                            current_loop_index = sampled.cursor.loop_index;
                            current_external_id = sampled.cursor.external_release;
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
                                sampled.follow_envelope(left, right, *envelope_step);
                                if tilting {
                                    sampled.brightness.apply(&mut left, &mut right, treble);
                                }
                                if enclosed {
                                    sampled
                                        .enclosure
                                        .apply(&mut left, &mut right, lite, *enc_ramp);
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
                        gain,
                        envelope: 0.0,
                        bus: bus.min(MAX_BUSES as u8 - 1),
                        // Onset delays are bounded only against nonsense
                        // (30 s covers any musical canon trick).
                        onset: delay_frames.min((30.0 * self.sample_rate) as u32),
                        age_frames: 0,
                        rng: (handle as u32).wrapping_mul(0x9E37_79B9) | 1,
                        phase: SamplePhase::Held,
                        cursor: PlaybackCursor {
                            sample,
                            position: start_position,
                            release_position: 0.0,
                            rate: rate as f64,
                            rate_target: rate as f64,
                            glide_frames: 0,
                            kernel: self.sinc.select(rate as f64),
                            loop_index: 0,
                            external_release: None,
                            past_loop: false,
                        },
                        wind: WindState {
                            group,
                            weight: wind_weight.max(0.0),
                            rate: wind_rate,
                            gain: wind_gain,
                            treble: wind_treble,
                            sens: wind_sens,
                            pitch_rate: wind_pitch_rate,
                            gain_rate: wind_gain_rate,
                            wander: wind::Wander::default(),
                        },
                        brightness: Brightness {
                            a: brightness.clamp(0.0, 1.0),
                            lowpass: [0.0; 2],
                        },
                        enclosure: EnclosureState {
                            index: enclosure,
                            gain: box_state.gain(),
                            gain_target: box_state.gain(),
                            hi_gain: box_state.hi_gain(),
                            coeff: box_state.coeff(),
                            lowpass: [0.0; 2],
                        },
                        release: ReleaseState {
                            fade: 0.0,
                            fade_step: 0.0,
                            fade_scale: 1.0,
                            amplitude: 1.0,
                            tail_gain: 1.0,
                            tail_decay: 1.0,
                            charge: 1.0,
                            charge_deficit: 0.0,
                            charge_step: 1.0,
                            bend: 0.0,
                            bend_depth: 0.0,
                            bend_step: 0.0,
                            pending: None,
                            wave_trem: self.wave_trems & (1u32 << u32::from(group).min(31)) != 0,
                        },
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
                        if sampled.wind.group == group && sampled.phase == SamplePhase::Held {
                            sampled.release.wave_trem = engaged;
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
                            sampled.cursor.rate_target = rate as f64;
                            sampled.cursor.glide_frames = glide_frames;
                            if glide_frames == 0 {
                                sampled.cursor.rate = rate as f64;
                                sampled.cursor.kernel = self.sinc.select(sampled.cursor.rate);
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
                            sampled.release.amplitude = 1.0;
                            sampled.release.fade_scale = 1.0;
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
                                sampled.release.amplitude = 1.0;
                                sampled.release.fade_scale = 1.0;
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
