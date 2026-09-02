//! One sounding pipe: the cursor reading its sample, the chest and box
//! it answers to, and everything the key-up sets in motion.
//!
//! The state is split into `Copy` PODs by lifetime rather than by
//! topic — a stage of the render loop touches one of them and nothing
//! else, which is what keeps the per-frame path readable without
//! costing a borrow.

use aristide_model::units::cents_to_ratio;

use crate::bank::Sample;
use crate::enclosure::{Enclosure, ENCLOSURE_NONE, MAX_ENCLOSURES, MAX_VOICE_ENCLOSURES};
use crate::resample::SincTables;
use crate::tone::ToneVoice;
use crate::wind::{self, WindGroup};

#[derive(Clone, Copy, PartialEq)]
pub(crate) enum SamplePhase {
    /// Attack and sustain loop, key held.
    Held,
    /// Ramping from the loop position onto the release tail.
    Crossfade,
    /// Playing the release tail out.
    Tail,
    /// Emergency amplitude ramp (no tail to go to / AllNotesOff).
    FadeOut,
}

/// Where the voice is reading from and how fast the cursor moves.
#[derive(Clone, Copy)]
pub(crate) struct PlaybackCursor {
    pub(crate) sample: u32,
    /// Fractional frame cursor into the sample.
    pub(crate) position: f64,
    /// Second cursor, into the release tail, during [`SamplePhase::Crossfade`].
    pub(crate) release_position: f64,
    /// Source frames advanced per output frame (before wind modulation).
    pub(crate) rate: f64,
    /// Where [`SetVoiceRate`](crate::Command::SetVoiceRate) is taking
    /// `rate`, and how many output frames of geometric slew remain.
    /// `glide_frames == 0` ⇔ settled (`rate == rate_target`); the slew
    /// advances per block, so within a block pitch is constant — at
    /// control rates that is the same quantization every MIDI-driven
    /// sampler has.
    pub(crate) rate_target: f64,
    pub(crate) glide_frames: u32,
    /// Sinc kernel bucket chosen once at
    /// [`StartVoice`](crate::Command::StartVoice) from `rate`
    /// ([`SincTables::select`]) and reused for the voice's whole life:
    /// tremulant/release-bend wobble never swings `rate` far enough to
    /// cross a quarter-octave bucket boundary, so re-picking per block
    /// would only add cost, not quality.
    pub(crate) kernel: usize,
    /// Which of the sample's sustain loops the cursor is circling; a
    /// new one is drawn at random on each pass.
    pub(crate) loop_index: u8,
    /// Separate release sample being crossfaded into, if any.
    pub(crate) external_release: Option<u32>,
    /// The cursor has left the sustain loop for release material (set at
    /// crossfade completion). Loop wrapping and seam-tap reads must never
    /// apply again: a shed/killed tail whose phase is FadeOut is NOT in
    /// the loop, and wrapping it teleports the cursor back into
    /// full-level sustain — the click/ghost-note bug found 2026-08-11.
    pub(crate) past_loop: bool,
}

/// The voice's side of the wind model: what it draws from its chest,
/// how fast it answers, and the factors it last read.
#[derive(Clone, Copy)]
pub(crate) struct WindState {
    /// Wind group index (pre-clamped to `MAX_WIND_GROUPS`).
    pub(crate) group: u8,
    /// How much wind this voice draws while sounding.
    pub(crate) weight: f32,
    /// Chest factors (pressure/tremulant/flow-noise pitch, gain,
    /// brightness) cached per voice under the same rule as the box
    /// factors in [`EnclosureState`]: a Held voice re-reads them each
    /// block; a released voice keeps them FROZEN — the pallet is
    /// closed, the tail is room decay, and it must not wobble (GO
    /// detaches releases from the windchest likewise).
    pub(crate) rate: f32,
    pub(crate) gain: f32,
    pub(crate) treble: f32,
    /// This pipe's own answer to the chest, fixed at voice start.
    /// `sens` spreads the modulation depth across the chorus (no two
    /// pipes are voiced identically); the two rates are 1/τ for the
    /// one-pole lags in [`WindState::follow_chest`] — pitch follows
    /// pressure within a few speaking periods, amplitude and timbre
    /// only over the pipe's speech time (~tens of periods), so a 16'
    /// bass barely flutters at tremulant rates while a 2' pipe follows
    /// the valve, and every pipe sits at its own phase. Uniform,
    /// instant factors are the single-LFO sound of an electronic
    /// vibrato.
    pub(crate) sens: f32,
    pub(crate) pitch_rate: f32,
    pub(crate) gain_rate: f32,
    /// Per-pipe wind-flow noise (slow, independent per voice).
    pub(crate) wander: wind::Wander,
}

/// Tilt filter: `out = lp + treble·(x − lp)` splits the signal at
/// roughly the pipe's 2nd harmonic so pressure can breathe the timbre.
/// `a` is the one-pole coefficient; 0 bypasses the filter entirely.
#[derive(Clone, Copy)]
pub(crate) struct Brightness {
    pub(crate) a: f32,
    pub(crate) lowpass: [f32; 2],
}

/// One swell box the voice sits inside, with the box factors cached
/// per voice: a Held voice re-reads them each block; a released voice
/// keeps them FROZEN — the tail is room decay that already left the
/// box, so later shutter moves must not touch it (HW's rule; GO bakes
/// the gain in likewise).
#[derive(Clone, Copy)]
pub(crate) struct EnclosureSlot {
    /// Which box ([`ENCLOSURE_NONE`](crate::enclosure::ENCLOSURE_NONE)
    /// = this slot is unused).
    pub(crate) index: u8,
    /// Broadband box gain, de-zippered per frame with a ~5 ms one-pole
    /// toward `gain_target` (block-stepped gain is audible zipper; a
    /// one-pole never overshoots regardless of block size).
    pub(crate) gain: f32,
    pub(crate) gain_target: f32,
    /// Shelf leg: high-frequency gain and one-pole corner coefficient,
    /// same filter form as the brightness tilt but hinged at the box
    /// corner instead of the pipe's 2nd harmonic.
    pub(crate) hi_gain: f32,
    pub(crate) coeff: f32,
    pub(crate) lowpass: [f32; 2],
}

/// Every box the voice sits inside. Boxes nest — an Echo or Solo box
/// standing inside the Swell — and a pipe in a nested box is heard
/// through BOTH shutter fronts, so the legs cascade: gains multiply
/// (GO composes a chest's enclosures the same way) and the shelves
/// filter in series. Unused slots are skipped, which keeps the
/// single-box path bit-identical to the pre-nesting engine.
#[derive(Clone, Copy)]
pub(crate) struct EnclosureState {
    pub(crate) slots: [EnclosureSlot; MAX_VOICE_ENCLOSURES],
}

/// Everything that only starts moving once the key comes up.
#[derive(Clone, Copy)]
pub(crate) struct ReleaseState {
    /// Crossfade progress 0→1.
    pub(crate) fade: f32,
    /// Per-voice crossfade step, set at release from the pipe's
    /// fundamental: ~9 periods, clamped 6–184 ms (GO/HW practice: bass
    /// splices need long fades, treble fades must be short or they smear
    /// the speech-off transient into an "artificial" fade). 0 = use the
    /// engine default.
    pub(crate) fade_step: f32,
    /// FadeOut speed multiplier on the kill ramp: 1.0 = 15 ms (silent
    /// noise voices, panic), 0.1 = ~150 ms (polyphony shedding, where
    /// abruptness would be audible).
    pub(crate) fade_scale: f32,
    /// FadeOut amplitude 1→0.
    pub(crate) amplitude: f32,
    /// Level-matching scale applied to the release tail so it continues
    /// at the voice's current loudness instead of the recording's.
    pub(crate) tail_gain: f32,
    /// Per-frame gain decay during Crossfade/Tail (1.0 = none). GO's
    /// staccato model: a short note hasn't formed the room's reverb
    /// yet, so its recorded (fully-reverberant) release tail is decayed
    /// over seconds to compensate.
    pub(crate) tail_decay: f32,
    /// Staccato room-charge: the tail's LATE diffuse field never built
    /// up for a short note, but its early reflections and speech-off
    /// did. Output is scaled by (charge + deficit) where deficit decays
    /// from (1 - charge) to 0 over ~150 ms: full level at the splice,
    /// settling to the charge level for the developed-reverb portion.
    pub(crate) charge: f32,
    pub(crate) charge_deficit: f32,
    pub(crate) charge_step: f32,
    /// Release pitch drop: as the pallet closes, blowing pressure
    /// collapses and a flue pipe's pitch sags before the sound dies —
    /// small pipes noticeably so (Viscount US7442869 models this; Aeolus
    /// has per-stop release detune). A constant-pitch high release reads
    /// as a struck bell; bells don't bend. `bend` ramps 0 to 1; the
    /// playback rate is scaled by (1 - bend_depth * bend).
    pub(crate) bend: f32,
    pub(crate) bend_depth: f32,
    pub(crate) bend_step: f32,
    /// A scheduled key-release: (frames until the pallet closes, hold
    /// age in ms captured at key-up). Real pallets never close in the
    /// same millisecond across a chord, and spreading the release also
    /// spreads the crossfade CPU spike that a mass release causes.
    pub(crate) pending: Option<(u16, u32)>,
    /// The chest's wave-tremulant state as this voice last saw it —
    /// which recording variant its release should match. Follows the
    /// live state while Held (GO selects by the state at key-off).
    pub(crate) wave_trem: bool,
}

/// A recording switch in flight: the voice is crossfading from its
/// current sample's sustain loop into ANOTHER recording of the same
/// pipe, and will carry on `Held` there — what a wave tremulant does
/// to notes that are already sounding (GO's `SwitchToAnotherAttack`).
///
/// The phase stays [`SamplePhase::Held`] throughout, deliberately: the
/// voice never leaves its loop, so the wind and box following, the
/// pallet's wind draw, and above all the `past_loop` invariant behave
/// exactly as they do for any held note. Only the second leg is new.
#[derive(Clone, Copy)]
pub(crate) struct SwitchState {
    /// The incoming recording; `None` = no switch in flight.
    pub(crate) sample: Option<u32>,
    /// Cursor into the incoming recording, circling ITS sustain loop
    /// (a release leg runs off the end of its tail; this one wraps).
    pub(crate) position: f64,
    pub(crate) loop_index: u8,
    /// Crossfade progress 0→1 and its per-frame step (pitch-scaled,
    /// like the release splice — ~9 fundamental periods).
    pub(crate) fade: f32,
    pub(crate) step: f32,
    /// Level match applied to the incoming leg so the crossfade does
    /// not step in loudness (see `SampledVoice::start_switch`).
    pub(crate) gain: f32,
    /// The incoming recording's playback rate as a factor on the
    /// voice's own — the two variants are the same pipe but need not
    /// be the same file sample rate. Folded into the voice's rate at
    /// handover.
    pub(crate) rate_ratio: f64,
    /// Sinc bucket for the post-handover rate, picked control-side at
    /// switch time so the handover itself stays branch-free.
    pub(crate) kernel: usize,
}

impl SwitchState {
    pub(crate) const IDLE: SwitchState = SwitchState {
        sample: None,
        position: 0.0,
        loop_index: 0,
        fade: 0.0,
        step: 0.0,
        gain: 1.0,
        rate_ratio: 1.0,
        kernel: 0,
    };
}

#[derive(Clone, Copy)]
pub(crate) struct SampledVoice {
    pub(crate) handle: u64,
    pub(crate) gain: f32,
    /// Fast envelope follower on the voice's own (pre-gain) output —
    /// what "how loud am I right now" means at release time.
    pub(crate) envelope: f32,
    /// Output bus the voice renders onto (pre-clamped to `MAX_BUSES`).
    pub(crate) bus: u8,
    /// Output frames still to wait before the pipe speaks (per-pipe
    /// onset delay). While pending the voice renders nothing, draws no
    /// wind, and does not age; released before speaking, it dies
    /// silently — the pallet never opened.
    pub(crate) onset: u32,
    /// Output frames since the voice started — drives the wind model's
    /// pallet-opening attack boost.
    pub(crate) age_frames: u32,
    pub(crate) rng: u32,
    pub(crate) phase: SamplePhase,
    pub(crate) cursor: PlaybackCursor,
    pub(crate) wind: WindState,
    pub(crate) brightness: Brightness,
    pub(crate) enclosure: EnclosureState,
    pub(crate) release: ReleaseState,
    pub(crate) switch: SwitchState,
}

/// Per-block invariants of a sampled voice's render loop, hoisted out
/// of the per-frame path (`Sample::frames()` alone is a u64 division —
/// two of those per frame per voice was a real cost at high polyphony).
/// Must be recomputed when the voice changes sample or loop mid-block.
#[derive(Clone, Copy)]
pub(crate) struct VoiceBlockContext {
    pub(crate) lite: bool,
    pub(crate) rate: f64,
    pub(crate) last: f64,
    pub(crate) tail_last: f64,
    pub(crate) current_loop: Option<(u64, u64)>,
    pub(crate) looping: bool,
    /// The incoming leg's sustain loop while a recording switch is in
    /// flight. Fixed for the whole block: unlike the persistent leg,
    /// the switch cursor never re-draws a random loop mid-fade — it
    /// only lives for the crossfade, and a loop change would strand
    /// this cached range.
    pub(crate) switch_loop: Option<(u64, u64)>,
    /// Engine output sample rate; releases need it to convert dB/s
    /// decay compensation into a per-frame factor.
    pub(crate) output_sr: f32,
}

#[derive(Clone, Copy, Default)]
pub(crate) enum Voice {
    #[default]
    Idle,
    Tone(ToneVoice),
    Sampled(SampledVoice),
}

impl PlaybackCursor {
    /// One block's worth of a pending rate glide: a single geometric
    /// step (the powf is paid only while gliding). A bend can cross a
    /// quarter-octave sinc bucket, so the kernel is re-picked while in
    /// motion — the one case the pick-once rule at StartVoice excludes.
    #[inline]
    pub(crate) fn step_glide(&mut self, frames: usize, held: bool, sinc: &SincTables) {
        if self.glide_frames == 0 {
            return;
        }
        if !held {
            // Released mid-glide: the tail keeps the pitch it reached,
            // as it keeps its box.
            self.glide_frames = 0;
            self.rate_target = self.rate;
            return;
        }
        if self.glide_frames as usize <= frames {
            self.rate = self.rate_target;
            self.glide_frames = 0;
        } else {
            let fraction = frames as f64 / self.glide_frames as f64;
            self.rate *= (self.rate_target / self.rate).powf(fraction);
            self.glide_frames -= frames as u32;
        }
        self.kernel = sinc.select(self.rate);
    }
}

impl WindState {
    /// One block of chest-following. The chest says where pressure is;
    /// the PIPE decides how it answers: its own sensitivity spreads the
    /// depth across the chorus, and the one-pole lags give each pipe its
    /// speech dynamics — pitch follows fast, amplitude and timbre over
    /// ~tens of periods, so basses barely flutter at tremulant rates and
    /// every pipe sits at its own phase. All identity when the chest is
    /// quiet (factors 1, no noise).
    #[inline]
    pub(crate) fn follow_chest(
        &mut self,
        chest: &WindGroup,
        box_loss: f32,
        dt: f32,
        rng: &mut u32,
    ) {
        let params = chest.params();
        // Per-voice flow noise, linearized around the chest factors (a
        // powf per voice per block would also be fine, but ±2 %
        // deviations are firmly linear).
        let mut deviation = 0.0;
        if params.flow_noise > 0.0 && self.weight > 0.0 {
            self.wander.step(dt, params.flow_noise, rng);
            deviation = self.wander.deviation();
        }
        // A swell box is the volume its pipes exhaust into, and a pipe
        // speaks on the DIFFERENCE between its chest and its mouth —
        // so the overpressure a closed box builds is a pressure LOSS
        // for every pipe inside it. It is a pressure, not a new
        // modulation shape, so it enters exactly where flow noise does:
        // one deviation, three exponents, and the per-pipe lags below
        // then let pitch answer within a few periods while amplitude
        // and timbre take the pipe's speech time. Nested boxes were
        // summed by the caller.
        deviation -= box_loss;
        let sens = self.sens;
        let target_rate =
            1.0 + (chest.rate_factor() - 1.0) * sens + params.pitch_exponent * deviation;
        let target_gain =
            1.0 + (chest.gain_factor() - 1.0) * sens + params.gain_exponent * deviation;
        // The brightness exponent is calibrated on the regulator's
        // few-percent sags; at tremulant pressure swings a pipe's
        // spectrum saturates long before P^3 says ±6 dB, so the swing
        // is capped at ≈ ±2.5 dB.
        let target_treble = (1.0
            + (chest.brightness_factor() - 1.0) * sens
            + params.brightness_exponent * deviation)
            .clamp(0.75, 1.33);
        let pitch_alpha = (dt * self.pitch_rate).min(1.0);
        let slow_alpha = (dt * self.gain_rate).min(1.0);
        self.rate += (target_rate - self.rate) * pitch_alpha;
        self.gain += (target_gain - self.gain) * slow_alpha;
        self.treble += (target_treble - self.treble) * slow_alpha;
    }
}

impl Brightness {
    /// The tilt filter, one frame. Callers skip it entirely while it
    /// would do nothing, which keeps untouched-pressure rendering
    /// bit-identical.
    #[inline]
    pub(crate) fn apply(&mut self, left: &mut f32, right: &mut f32, treble: f32) {
        let lp = &mut self.lowpass;
        lp[0] += self.a * (*left - lp[0]);
        lp[1] += self.a * (*right - lp[1]);
        *left = lp[0] + treble * (*left - lp[0]);
        *right = lp[1] + treble * (*right - lp[1]);
    }
}

impl EnclosureSlot {
    /// A slot no voice sits in: skipped everywhere, and the reason a
    /// single-box voice renders exactly as it did before nesting.
    pub(crate) const EMPTY: EnclosureSlot = EnclosureSlot {
        index: ENCLOSURE_NONE,
        gain: 1.0,
        gain_target: 1.0,
        hi_gain: 1.0,
        coeff: 0.0,
        lowpass: [0.0; 2],
    };

    /// Re-read the box. Only Held voices do this; a released voice's
    /// tail keeps the factors it left the box with.
    #[inline]
    pub(crate) fn follow_box(&mut self, box_state: &Enclosure) {
        self.gain_target = box_state.gain();
        self.hi_gain = box_state.hi_gain();
        self.coeff = box_state.coeff();
    }

    /// Shutter shelf plus de-zippered broadband gain, one frame. Lite
    /// mode keeps the gain (the pedal must still do something) and only
    /// skips the filter.
    #[inline]
    pub(crate) fn apply(&mut self, left: &mut f32, right: &mut f32, lite: bool, ramp: f32) {
        if !lite {
            let lp = &mut self.lowpass;
            lp[0] += self.coeff * (*left - lp[0]);
            lp[1] += self.coeff * (*right - lp[1]);
            *left = lp[0] + self.hi_gain * (*left - lp[0]);
            *right = lp[1] + self.hi_gain * (*right - lp[1]);
        }
        self.gain += ramp * (self.gain_target - self.gain);
        *left *= self.gain;
        *right *= self.gain;
    }
}

impl EnclosureState {
    pub(crate) const UNENCLOSED: EnclosureState = EnclosureState {
        slots: [EnclosureSlot::EMPTY; MAX_VOICE_ENCLOSURES],
    };

    /// Does this voice sit in any box at all?
    #[inline]
    pub(crate) fn enclosed(&self) -> bool {
        self.slots.iter().any(|slot| slot.index != ENCLOSURE_NONE)
    }

    /// Re-read every box (Held voices only, as above).
    #[inline]
    pub(crate) fn follow_boxes(&mut self, boxes: &[Enclosure; MAX_ENCLOSURES]) {
        for slot in self.slots.iter_mut() {
            if slot.index != ENCLOSURE_NONE {
                slot.follow_box(&boxes[slot.index as usize]);
            }
        }
    }

    /// The overpressure this voice's mouth sits in, summed over its
    /// boxes. Nesting stacks additively: an inner box vents into the
    /// outer one, so its own rise is measured *relative* to the outer
    /// box's, and referenced to the room the pipe's mouth sees the sum
    /// along the chain.
    #[inline]
    pub(crate) fn pressure_loss(&self, boxes: &[Enclosure; MAX_ENCLOSURES]) -> f32 {
        let mut loss = 0.0;
        for slot in self.slots.iter() {
            if slot.index != ENCLOSURE_NONE {
                loss += boxes[slot.index as usize].pressure_loss();
            }
        }
        loss
    }

    /// Every box the voice sits in, cascaded: through the inner
    /// shutters first, then the outer ones.
    #[inline]
    pub(crate) fn apply(&mut self, left: &mut f32, right: &mut f32, lite: bool, ramp: f32) {
        for slot in self.slots.iter_mut() {
            if slot.index != ENCLOSURE_NONE {
                slot.apply(left, right, lite, ramp);
            }
        }
    }
}

/// What the onset delay says about this voice, this block.
pub(crate) enum Onset {
    /// Speaks from this frame of the chunk onward.
    Speaks(usize),
    /// Still waiting: renders nothing, draws no wind, does not age.
    Waiting,
    /// Released before the pallet ever opened — retire the slot.
    NeverSpoke,
}

impl SampledVoice {
    #[inline]
    /// `second` is the voice's other leg, when it has one: the release
    /// material it is splicing out to, or — while a recording switch is
    /// in flight — the recording it is crossing into. A voice never has
    /// both (a switch is resolved before any release begins), so one
    /// slot carries both cases and the per-frame path stays two reads.
    pub(crate) fn block_context(
        &self,
        sample: &Sample,
        second: Option<&Sample>,
        rate_scale: f64,
        lite: bool,
        output_sr: f32,
    ) -> VoiceBlockContext {
        let current_loop = sample.loop_at(self.cursor.loop_index as usize);
        let switch_loop = match (self.switch.sample, second) {
            (Some(_), Some(target)) => target.loop_at(self.switch.loop_index as usize),
            _ => None,
        };
        VoiceBlockContext {
            lite,
            rate: self.cursor.rate * rate_scale,
            last: (sample.frames() - 1) as f64,
            tail_last: (second.unwrap_or(sample).frames() - 1) as f64,
            current_loop,
            looping: current_loop.is_some(),
            switch_loop,
            output_sr,
        }
    }

    /// Onset delay: silent, un-aged, until it elapses — then the voice
    /// speaks partway into this chunk. A voice killed while still
    /// waiting never speaks: the pallet never opened.
    #[inline]
    pub(crate) fn take_onset(&mut self, frames: usize) -> Onset {
        if self.onset == 0 {
            return Onset::Speaks(0);
        }
        if self.phase != SamplePhase::Held {
            return Onset::NeverSpoke;
        }
        if self.onset as usize >= frames {
            self.onset -= frames as u32;
            return Onset::Waiting;
        }
        let start = self.onset as usize;
        self.onset = 0;
        Onset::Speaks(start)
    }

    /// The other sample the voice reads from this block, if any:
    /// release material during a key-off splice, or the incoming
    /// recording during a wave-tremulant switch. Never both — a switch
    /// is settled before any release begins (`resolve_switch`).
    #[inline]
    pub(crate) fn second_leg(&self) -> Option<u32> {
        self.cursor.external_release.or(self.switch.sample)
    }

    /// Track the voice's own loudness (pre-gain) for release level
    /// matching.
    #[inline]
    pub(crate) fn follow_envelope(&mut self, left: f32, right: f32, step: f32) {
        self.envelope += step * ((left.abs() + right.abs()) * 0.5 - self.envelope);
    }

    /// Render one frame and advance. Returns `None` when the voice ends.
    /// End-of-data checks happen on entry so every read frame is emitted.
    #[inline]
    pub(crate) fn tick(
        &mut self,
        sample: &Sample,
        second: Option<&Sample>,
        tables: &SincTables,
        ctx: &VoiceBlockContext,
        crossfade_step: f32,
        kill_step: f32,
    ) -> Option<(f32, f32)> {
        let rate = self.bend_rate(ctx.rate);
        self.fire_pending_release(sample, ctx.output_sr);
        if self.ended(ctx) {
            return None;
        }
        let (mut left, mut right) = self.read_frame(sample, tables, ctx);
        // This frame's output gain, captured BEFORE anything below
        // mutates self.gain. A completing crossfade folds its level
        // match into the voice gain for FUTURE frames, but its own
        // blend already applied that factor — returning with the
        // mutated gain applied it twice, dipping exactly one frame by
        // up to 5x (tail_gain floor 0.2): an audible tick on every
        // splice handover.
        let frame_gain = self.gain;
        let mut advance_position = if self.switch.sample.is_some() {
            self.blend_switch(&mut left, &mut right, second, tables, ctx, rate)
        } else {
            true
        };
        advance_position &= self.step_phase(
            &mut left,
            &mut right,
            second.unwrap_or(sample),
            tables,
            ctx,
            rate,
            crossfade_step,
            kill_step,
        );
        self.apply_tail_charge(&mut left, &mut right);
        self.apply_eof_guard(&mut left, &mut right, ctx.last);
        self.decay_tail();
        if advance_position {
            self.advance_cursor(sample, ctx, rate);
        }
        Some((left * frame_gain, right * frame_gain))
    }

    /// Release pitch sag, ramped per frame: the block's playback rate
    /// scaled by the pallet's collapsing pressure.
    #[inline]
    fn bend_rate(&mut self, rate: f64) -> f64 {
        if self.release.bend_depth > 0.0
            && matches!(self.phase, SamplePhase::Crossfade | SamplePhase::Tail)
        {
            self.release.bend += (1.0 - self.release.bend) * self.release.bend_step;
            return rate * (1.0 - (self.release.bend_depth * self.release.bend) as f64);
        }
        rate
    }

    /// A scheduled release fires when its pallet-delay runs out — but
    /// never in the middle of a recording switch. Dropping either leg
    /// of a switch mid-crossfade is a step (the two legs are level- and
    /// phase-matched, but a tremmed twin's own undulation is not), and
    /// a step is a click; splicing the release out of a composite of
    /// two recordings would need a third cursor for a coincidence
    /// measured in milliseconds. So the pallet simply waits for the
    /// crossfade to land: at most one fade length (≤ 184 ms, and only
    /// for a key released within that window of a tremulant toggle),
    /// after which the release splices cleanly out of the recording the
    /// voice actually ended up on.
    #[inline]
    fn fire_pending_release(&mut self, sample: &Sample, output_sr: f32) {
        if self.phase != SamplePhase::Held || self.switch.sample.is_some() {
            return;
        }
        if let Some((delay, age_ms)) = self.release.pending {
            if delay == 0 {
                self.release.pending = None;
                self.begin_release(sample, age_ms, output_sr);
            } else {
                self.release.pending = Some((delay - 1, age_ms));
            }
        }
    }

    /// Out of material for the phase the voice is in.
    #[inline]
    fn ended(&self, ctx: &VoiceBlockContext) -> bool {
        match self.phase {
            SamplePhase::Held => !ctx.looping && self.cursor.position >= ctx.last,
            SamplePhase::Crossfade => self.cursor.release_position >= ctx.tail_last,
            SamplePhase::Tail => self.cursor.position >= ctx.last,
            SamplePhase::FadeOut => {
                self.release.amplitude <= 0.0
                    || ((!ctx.looping || self.cursor.past_loop)
                        && self.cursor.position >= ctx.last)
            }
        }
    }

    /// The persistent leg's read.
    #[inline]
    fn read_frame(
        &self,
        sample: &Sample,
        tables: &SincTables,
        ctx: &VoiceBlockContext,
    ) -> (f32, f32) {
        // Cursors still circling the sustain loop wrap their kernel taps
        // across the seam; tail reads clamp at the sample edges.
        let seam = if self.phase == SamplePhase::Tail || self.cursor.past_loop {
            None
        } else {
            ctx.current_loop
        };
        if ctx.lite {
            sample.read(self.cursor.position)
        } else {
            tables.read(self.cursor.kernel, sample, self.cursor.position, seam)
        }
    }

    /// This frame's phase bookkeeping: blend the release leg in, or step
    /// the kill ramp. Returns `false` once the crossfade completed and
    /// handed the (already advanced) tail cursor over.
    #[inline]
    #[allow(clippy::too_many_arguments)]
    fn step_phase(
        &mut self,
        left: &mut f32,
        right: &mut f32,
        tail_sample: &Sample,
        tables: &SincTables,
        ctx: &VoiceBlockContext,
        rate: f64,
        crossfade_step: f32,
        kill_step: f32,
    ) -> bool {
        match self.phase {
            SamplePhase::Held | SamplePhase::Tail => {}
            SamplePhase::Crossfade => {
                // The outgoing crossfade leg once dropped to linear
                // interpolation to halve the double-read cost, but the
                // sinc→linear switch at release put a ~-46 dB kink at
                // the START of every splice — an audible tick on exposed
                // releases. Keep full quality; the pallet stagger
                // already spreads the crossfade CPU spike.
                let (tail_l, tail_r) = if ctx.lite {
                    tail_sample.read(self.cursor.release_position)
                } else {
                    let position = self.cursor.release_position;
                    tables.read(self.cursor.kernel, tail_sample, position, None)
                };
                // Raised-cosine-shaped blend (smoothstep ≈ it, no trig):
                // linear fades dip audibly on the uncorrelated noise
                // floor (Appleton 2019).
                let fade = self.release.fade;
                let weight = fade * fade * (3.0 - 2.0 * fade);
                *left += (tail_l * self.release.tail_gain - *left) * weight;
                *right += (tail_r * self.release.tail_gain - *right) * weight;
                self.release.fade += if self.release.fade_step > 0.0 {
                    self.release.fade_step
                } else {
                    crossfade_step
                };
                self.cursor.release_position += rate;
                if self.release.fade >= 1.0 {
                    // Hand the (already advanced) tail cursor over and
                    // fold the level match into the voice gain. If the
                    // tail is a separate sample, the voice moves there.
                    self.cursor.position = self.cursor.release_position;
                    self.gain *= self.release.tail_gain;
                    self.release.tail_gain = 1.0;
                    if let Some(external_id) = self.cursor.external_release.take() {
                        self.cursor.sample = external_id;
                        self.cursor.loop_index = 0;
                    }
                    self.phase = SamplePhase::Tail;
                    self.cursor.past_loop = true;
                    return false;
                }
            }
            SamplePhase::FadeOut => {
                *left *= self.release.amplitude;
                *right *= self.release.amplitude;
                self.release.amplitude -= kill_step * self.release.fade_scale;
            }
        }
        true
    }

    /// Staccato room-charge: full level at the splice, settling to the
    /// charge level once the deficit has decayed away.
    #[inline]
    fn apply_tail_charge(&mut self, left: &mut f32, right: &mut f32) {
        if matches!(self.phase, SamplePhase::Held) {
            return;
        }
        if self.release.charge_deficit > 0.0 {
            let factor = self.release.charge + self.release.charge_deficit;
            *left *= factor;
            *right *= factor;
            self.release.charge_deficit *= self.release.charge_step;
            if self.release.charge_deficit < 1e-4 {
                // Settled: fold the charge into the gain and stop paying
                // the per-frame cost.
                self.gain *= self.release.charge;
                self.release.charge = 1.0;
                self.release.charge_deficit = 0.0;
            }
        } else if self.release.charge != 1.0 {
            *left *= self.release.charge;
            *right *= self.release.charge;
        }
    }

    /// EOF guard: a tail must reach the end of its material silent.
    /// Decay compensation can leave boosted level near EOF (and some
    /// sets simply end hot); fade the final ~46 ms instead of cutting.
    #[inline]
    fn apply_eof_guard(&self, left: &mut f32, right: &mut f32, last: f64) {
        if !self.cursor.past_loop {
            return;
        }
        const GUARD_FRAMES: f64 = 2048.0;
        let remaining = last - self.cursor.position;
        if remaining < GUARD_FRAMES {
            let scale = (remaining / GUARD_FRAMES).max(0.0) as f32;
            *left *= scale;
            *right *= scale;
        }
    }

    /// Per-frame decay compensation, folded into the voice gain.
    #[inline]
    fn decay_tail(&mut self) {
        if matches!(self.phase, SamplePhase::Crossfade | SamplePhase::Tail)
            && self.release.tail_decay != 1.0
        {
            self.gain *= self.release.tail_decay;
        }
    }

    /// Advance the cursor. Only cursors still circling the sustain loop
    /// wrap; a Tail cursor has left it for the release material. On each
    /// pass a fresh loop is drawn at random (multi-loop sets), which
    /// decorrelates repetition.
    #[inline]
    fn advance_cursor(&mut self, sample: &Sample, ctx: &VoiceBlockContext, rate: f64) {
        self.cursor.position += rate;
        if self.phase == SamplePhase::Tail || self.cursor.past_loop {
            return;
        }
        let Some((start, end)) = ctx.current_loop else {
            return;
        };
        if self.cursor.position < end as f64 {
            return;
        }
        // Wrap to THIS loop's own start — the only splice the set's
        // author guaranteed seamless. Loop variety comes from choosing
        // which loop's end we run toward next (all loops live in one
        // continuous recording, so playing from here to any later end is
        // seamless too). Jumping into a different loop's start pops
        // audibly.
        let overshoot = self.cursor.position - end as f64;
        self.cursor.position = (start as f64 + overshoot).min(end as f64 - 1.0);
        let count = sample.loop_count();
        if count > 1 {
            let candidate =
                (wind::xorshift_unit(&mut self.rng) * count as f32) as u8 % count as u8;
            if let Some((_, candidate_end)) = sample.loop_at(candidate as usize)
                && (candidate_end as f64) > self.cursor.position
            {
                self.cursor.loop_index = candidate;
            }
        }
    }

    /// Begin — or redirect — a crossfade into another recording of the
    /// same pipe. `source` is what the voice plays now, `target` what
    /// it should play; `rate_ratio` carries a differing file sample
    /// rate. Returns false when nothing could be wired (the loader
    /// never paired the two recordings, or neither loops), in which
    /// case the voice simply keeps the recording it has — today's
    /// behaviour, silently.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn start_switch(
        &mut self,
        source: &Sample,
        target: &Sample,
        target_id: u32,
        rate_ratio: f64,
        tables: &SincTables,
        output_rate: f32,
        age_ms: u32,
    ) -> bool {
        if self.switch.sample == Some(target_id) {
            return true; // already on its way there
        }
        if self.cursor.sample == target_id {
            // The player flipped the tremulant back before the first
            // crossfade finished. Reversing is exact: the two legs
            // swap roles and the blend weight is mirrored, and
            // smoothstep(1 − f) = 1 − smoothstep(f), so the output is
            // continuous to the last bit.
            self.reverse_switch();
            return true;
        }
        // A third recording arriving mid-crossfade: settle the one in
        // flight first (see `resolve_switch`), then start afresh.
        self.resolve_switch();
        if self.cursor.sample == target_id {
            return true;
        }
        let Some(option) = source.switch_option(target_id) else {
            return false;
        };
        let position = match (option.alignment(), source.sustain_loop(), target.sustain_loop()) {
            (Some(alignment), Some((loop_start, _)), _) => {
                alignment.target(self.cursor.position, loop_start) as f64
            }
            (None, _, Some((target_start, _))) => target_start as f64,
            _ => return false,
        };
        // Level match. The voice's envelope follower says how loud it
        // is RIGHT NOW (attack still swelling, tremulant at a peak or
        // a trough); the target's stored level says how loud the
        // recording it is about to play is at the landing point. The
        // voice's own gain is divided out because the envelope is
        // measured post-gain — leaving it in would apply that gain a
        // second time. Clamped: these are two recordings of one pipe,
        // so anything past a factor of three is a mislabeled pair and
        // matching it would be worse than not.
        self.switch.gain = if option.level > 1e-5 && self.gain > 1e-5 && self.envelope > 1e-6 {
            (self.envelope / (self.gain * option.level)).clamp(0.33, 3.0)
        } else {
            1.0
        };
        self.switch.sample = Some(target_id);
        self.switch.position = position;
        self.switch.loop_index = 0;
        self.switch.fade = 0.0;
        self.switch.step = {
            let step = pitch_scaled_fade_step(source, self.cursor.rate, output_rate, age_ms);
            // No measured period (unpitched material): fall back to the
            // engine's own 30 ms release crossfade rather than 0, which
            // would leave the fade stuck.
            if step > 0.0 { step } else { 1.0 / (0.03 * output_rate) }
        };
        self.switch.rate_ratio = rate_ratio;
        self.switch.kernel = if rate_ratio == 1.0 {
            self.cursor.kernel
        } else {
            tables.select(self.cursor.rate * rate_ratio)
        };
        true
    }

    /// Swap the two legs of a switch in flight, mirroring the blend
    /// weight so the output does not move. The outgoing leg carried
    /// the voice gain and the incoming one `switch.gain` on top, so
    /// those factors trade places too.
    fn reverse_switch(&mut self) {
        let Some(incoming) = self.switch.sample else {
            return;
        };
        self.switch.sample = Some(self.cursor.sample);
        self.cursor.sample = incoming;
        std::mem::swap(&mut self.switch.position, &mut self.cursor.position);
        std::mem::swap(&mut self.switch.loop_index, &mut self.cursor.loop_index);
        std::mem::swap(&mut self.switch.kernel, &mut self.cursor.kernel);
        self.gain *= self.switch.gain;
        self.switch.gain = 1.0 / self.switch.gain;
        if self.switch.rate_ratio != 1.0 {
            self.cursor.rate *= self.switch.rate_ratio;
            self.cursor.rate_target *= self.switch.rate_ratio;
            self.switch.rate_ratio = 1.0 / self.switch.rate_ratio;
        }
        self.switch.fade = 1.0 - self.switch.fade;
    }

    /// Settle a switch in flight so a crossfade into a THIRD recording
    /// can start: the leg carrying less than half the blend is dropped.
    ///
    /// This is the one place a step is accepted, and the only one where
    /// it cannot be avoided without a third cursor — it takes a set
    /// with three or more variants that disagree on `IsTremulant` and
    /// two tremulant toggles inside one crossfade. The dropped leg is
    /// the minority one, on material that is by construction
    /// near-identical (the same pipe, phase-aligned and level-matched),
    /// and the new crossfade starts on the very next frame. Key-off and
    /// the plain there-and-back toggle both avoid it entirely
    /// (`fire_pending_release`, `reverse_switch`).
    fn resolve_switch(&mut self) {
        if self.switch.sample.is_none() {
            return;
        }
        if self.switch.fade >= 0.5 {
            self.complete_switch();
        } else {
            self.abandon_switch();
        }
    }

    /// Hand the voice over to the incoming recording: its cursor, its
    /// loop, its release options, its rate. `past_loop` stays false and
    /// the phase stays `Held` — the voice never left the sustain loop,
    /// which is exactly what separates this from a release splice.
    fn complete_switch(&mut self) {
        let Some(sample) = self.switch.sample.take() else {
            return;
        };
        self.cursor.sample = sample;
        self.cursor.position = self.switch.position;
        self.cursor.loop_index = self.switch.loop_index;
        self.cursor.kernel = self.switch.kernel;
        self.gain *= self.switch.gain;
        if self.switch.rate_ratio != 1.0 {
            self.cursor.rate *= self.switch.rate_ratio;
            self.cursor.rate_target *= self.switch.rate_ratio;
        }
        self.switch.gain = 1.0;
        self.switch.rate_ratio = 1.0;
        self.switch.fade = 0.0;
    }

    /// Drop the incoming leg and stay where we are.
    fn abandon_switch(&mut self) {
        self.switch.sample = None;
        self.switch.gain = 1.0;
        self.switch.rate_ratio = 1.0;
        self.switch.fade = 0.0;
    }

    /// One frame of the switch crossfade: read the incoming leg out of
    /// its own loop, blend, advance. Returns false on the frame the
    /// handover completes — the new cursor has already advanced, so the
    /// caller must not advance it again against a block context that
    /// still describes the old recording.
    #[inline]
    fn blend_switch(
        &mut self,
        left: &mut f32,
        right: &mut f32,
        target: Option<&Sample>,
        tables: &SincTables,
        ctx: &VoiceBlockContext,
        rate: f64,
    ) -> bool {
        let Some(target) = target else {
            // The incoming recording is not in the bank after all:
            // stay put rather than read from nothing.
            self.abandon_switch();
            return true;
        };
        let (in_l, in_r) = if ctx.lite {
            target.read(self.switch.position)
        } else {
            tables.read(
                self.switch.kernel,
                target,
                self.switch.position,
                ctx.switch_loop,
            )
        };
        // Same raised-cosine blend as the release splice: a linear fade
        // dips audibly where the two legs decorrelate.
        let fade = self.switch.fade;
        let weight = fade * fade * (3.0 - 2.0 * fade);
        let gain = self.switch.gain;
        *left += (in_l * gain - *left) * weight;
        *right += (in_r * gain - *right) * weight;
        self.switch.fade += self.switch.step;
        self.advance_switch_cursor(ctx, rate);
        if self.switch.fade >= 1.0 {
            self.complete_switch();
            return false;
        }
        true
    }

    /// Advance the incoming leg, wrapping in ITS loop. No random
    /// re-draw of the loop on each pass: the leg lives only for the
    /// crossfade, and changing loop mid-fade would strand the block
    /// context's cached range.
    #[inline]
    fn advance_switch_cursor(&mut self, ctx: &VoiceBlockContext, rate: f64) {
        self.switch.position += rate * self.switch.rate_ratio;
        let Some((start, end)) = ctx.switch_loop else {
            return;
        };
        if self.switch.position < end as f64 {
            return;
        }
        let overshoot = self.switch.position - end as f64;
        self.switch.position = (start as f64 + overshoot).min(end as f64 - 1.0);
    }

    /// Key released: splice to a separate release (selected by hold
    /// duration) or the embedded tail, whichever the sample offers.
    pub(crate) fn begin_release(&mut self, sample: &Sample, age_ms: u32, output_rate: f32) {
        match self.phase {
            SamplePhase::Held | SamplePhase::Crossfade => {}
            _ => return,
        }
        // Never out of a switch in flight: `sample` is the recording
        // the voice is *leaving*, so its release options and phase map
        // are the wrong ones, and there is no clean way to drop a
        // half-blended leg. The scheduled release fires a frame after
        // the crossfade lands instead (see `fire_pending_release`).
        if self.switch.sample.is_some() {
            return;
        }
        if self.phase == SamplePhase::Held
            && sample.sustain_loop().is_some()
            && self.splice_to_separate_release(sample, age_ms, output_rate)
        {
            return;
        }
        match sample.release_start() {
            Some(tail) if self.phase == SamplePhase::Held => {
                self.splice_to_embedded_tail(sample, tail, age_ms, output_rate);
            }
            Some(_) => {} // already crossfading
            None => {
                if sample.sustain_loop().is_some() {
                    self.release.amplitude = 1.0;
                    self.phase = SamplePhase::FadeOut;
                }
                // Loop-less (percussive) samples play to the end.
            }
        }
    }

    /// A separate recorded release, if the set offers one this hold
    /// qualifies for. Options are sorted (bounded holds ascending,
    /// unbounded last): the first whose bound covers the hold wins.
    fn splice_to_separate_release(
        &mut self,
        sample: &Sample,
        age_ms: u32,
        output_rate: f32,
    ) -> bool {
        let chosen = sample
            .release_options()
            .iter()
            .filter(|option| {
                option
                    .wave_trem
                    .is_none_or(|wants| wants == self.release.wave_trem)
            })
            .find(|option| option.max_hold_ms.is_none_or(|max| age_ms <= max));
        let Some(option) = chosen else {
            return false;
        };
        self.cursor.external_release = Some(option.sample);
        self.cursor.release_position = match (option.alignment(), sample.sustain_loop()) {
            (Some(alignment), Some((loop_start, _))) => {
                alignment.target(self.cursor.position, loop_start) as f64
            }
            _ => 0.0,
        };
        self.release.tail_gain = if option.level > 1e-5 {
            (self.envelope / option.level).clamp(0.05, 1.1)
        } else {
            1.0
        };
        self.release.fade_step = if option.crossfade_ms > 0 {
            odf_fade_step(option.crossfade_ms, output_rate, age_ms)
        } else {
            pitch_scaled_fade_step(sample, self.cursor.rate, output_rate, age_ms)
        };
        self.release.fade = 0.0;
        self.phase = SamplePhase::Crossfade;
        true
    }

    /// The tail recorded inside the sample itself, from `tail` on.
    fn splice_to_embedded_tail(
        &mut self,
        sample: &Sample,
        tail: u64,
        age_ms: u32,
        output_rate: f32,
    ) {
        self.cursor.release_position =
            match (sample.release_alignment(), sample.sustain_loop()) {
                (Some(alignment), Some((loop_start, _))) => {
                    alignment.target(self.cursor.position, loop_start) as f64
                }
                _ => tail as f64,
            };
        self.match_tail_level(sample);
        let tail_seconds =
            (sample.frames().saturating_sub(tail)) as f32 / sample.sample_rate_hz();
        let staccato_extra_db_per_s = self.charge_room(tail_seconds, age_ms, output_rate);
        self.release.fade_step = if sample.release_crossfade_ms() > 0 {
            odf_fade_step(sample.release_crossfade_ms(), output_rate, age_ms)
        } else {
            pitch_scaled_fade_step(sample, self.cursor.rate, output_rate, age_ms)
        };
        self.arm_release_bend(sample, output_rate);
        let decay =
            self.tail_decay_factor(sample, tail_seconds, staccato_extra_db_per_s, output_rate);
        self.release.tail_decay = decay;
        self.release.fade = 0.0;
        self.phase = SamplePhase::Crossfade;
    }

    /// Level match: scale the tail to continue at the voice's current
    /// loudness (early releases are quieter than the recorded sustain —
    /// unscaled tails strike like a bell). Floor at 0.2 like GO: a
    /// fully-silent-entry release sounds MORE artificial than a slightly
    /// loud one. Exception: a near-silent loop (control-noise samples:
    /// thump → silent loop → thump tail) means the tail is MEANT to be
    /// louder — play it as recorded.
    ///
    /// One gain for both channels, deliberately. A stereo recording's
    /// tail does sit at a different L/R balance from its sustain (demo
    /// set, measured 2026-09-02: median 0.8 dB, p90 4.4 dB, worst
    /// 11.4 dB) — but that is the room, not an artifact: the direct
    /// sound that favours the near mic stops with the pipe and only the
    /// (more symmetric) diffuse field is left. Matching per channel
    /// would overwrite the recorded release's stereo image with the
    /// sustain's. Phase is the opposite case — a phase mismatch buys
    /// nothing and only cancels — which is why *that* one is corrected
    /// per channel (`Sample::alignment_turns`).
    fn match_tail_level(&mut self, sample: &Sample) {
        let reference = sample.tail_reference_level();
        self.release.tail_gain = if reference > 1e-5 && self.envelope > 0.02 * reference {
            (self.envelope / reference).clamp(0.2, 1.1)
        } else {
            1.0
        };
    }

    /// Staccato: a room's decay RATE is fixed by the room — a short note
    /// leaves a QUIETER tail, never a faster-decaying one (GO decays the
    /// rate instead, which turns fast passages into plucks). Model the
    /// room charge as a first-order build-up toward steady state, and
    /// return the extra late-tail decay an undeveloped field costs:
    /// level scaling alone leaves a conspicuous shimmer after high
    /// staccato (the diffuse field wasn't just quieter, it never fully
    /// formed).
    fn charge_room(&mut self, tail_seconds: f32, age_ms: u32, output_rate: f32) -> f32 {
        let full_reverb_ms = (60.0 * tail_seconds + 40.0).clamp(100.0, 350.0);
        if (age_ms as f32) >= full_reverb_ms {
            return 0.0;
        }
        let charge = (1.0 - (-(age_ms as f32) / (0.5 * full_reverb_ms)).exp()).max(0.1);
        self.release.charge = charge;
        self.release.charge_deficit = 1.0 - charge;
        self.release.charge_step = (-1.0 / (0.15 * output_rate)).exp();
        (1.0 - charge) * 25.0
    }

    /// Release pitch drop: depth grows with pipe pitch (~35 cents at
    /// 1 kHz+, ~15 at 250 Hz, negligible for big pipes) over a pressure
    /// collapse of ~12 periods, 15–80 ms.
    fn arm_release_bend(&mut self, sample: &Sample, output_rate: f32) {
        let Some(period) = sample.measured_period() else {
            return;
        };
        let f0 = sample.sample_rate_hz() as f64 / period;
        let cents = (4.0 * (f0 / 100.0).sqrt()).clamp(1.0, 12.0);
        self.release.bend_depth = 1.0 - cents_to_ratio(-cents) as f32;
        let tau_s = (12.0 * period / sample.sample_rate_hz() as f64).clamp(0.015, 0.080);
        self.release.bend_step = 1.0 - (-1.0 / (tau_s as f32 * output_rate)).exp();
        self.release.bend = 0.0;
    }

    /// Repitching by R also plays the recorded room decay R× too fast
    /// (or slow) — ring time must not depend on the key, so compensate
    /// the measured tail decay rate with a per-frame gain factor.
    /// Down-repitched pipes were the "bell": their tails rang up to 40 %
    /// too long. A tail still audible when its recording runs out ends
    /// in a hard cut — the demo set's mixture has a rank 50 dB hot at
    /// EOF that rings bell-like and then vanishes — so add whatever
    /// decay settles the tail to ≈ -60 dB by EOF, counting the level the
    /// decay compensation adds back.
    fn tail_decay_factor(
        &self,
        sample: &Sample,
        tail_seconds: f32,
        staccato_extra_db_per_s: f32,
        output_rate: f32,
    ) -> f32 {
        let lambda = sample.tail_decay_db_per_s();
        let repitch = (self.cursor.rate as f32) * output_rate / sample.sample_rate_hz();
        let comp_db_per_s = if lambda > 0.0 && (repitch - 1.0).abs() > 0.01 {
            (lambda * (repitch - 1.0)).clamp(-25.0, 25.0)
        } else {
            0.0
        };
        let out_tail_seconds = tail_seconds / repitch.max(0.01);
        let settle_db_per_s = if out_tail_seconds > 0.3 {
            let eof_db = sample.tail_eof_level_db() + comp_db_per_s * out_tail_seconds;
            ((eof_db + 60.0) / out_tail_seconds).clamp(0.0, 60.0)
        } else {
            0.0
        };
        let db_per_s = comp_db_per_s - staccato_extra_db_per_s - settle_db_per_s;
        if db_per_s.abs() > 0.01 {
            10.0f32.powf(db_per_s / (20.0 * output_rate))
        } else {
            1.0
        }
    }
}

/// ~9 fundamental periods (GO: 184 ms bass → 6 ms treble), but never
/// longer than the note has lived: a mid-attack release must not keep
/// swelling through a long fade — the drive collapses when the pallet
/// closes. 0 = use the engine default.
fn pitch_scaled_fade_step(sample: &Sample, rate: f64, output_rate: f32, age_ms: u32) -> f32 {
    let Some(period) = sample.measured_period() else {
        return 0.0; // engine default
    };
    let output_period = period / rate.max(1e-6);
    let age_frames = age_ms as f64 * 0.001 * output_rate as f64;
    let frames = (9.0 * output_period)
        .min(age_frames.max(0.006 * output_rate as f64))
        .clamp(0.006 * output_rate as f64, 0.184 * output_rate as f64);
    (1.0 / frames) as f32
}

/// A producer-tuned crossfade (ODF ReleaseCrossfadeLength) overrides the
/// pitch-scaled default; the note-age cap stays — a mid-attack release
/// must still collapse, not swell.
fn odf_fade_step(ms: u16, output_rate: f32, age_ms: u32) -> f32 {
    let age_frames = age_ms as f64 * 0.001 * output_rate as f64;
    let frames = (f64::from(ms) * 0.001 * output_rate as f64)
        .min(age_frames.max(0.006 * output_rate as f64))
        .max(1.0);
    (1.0 / frames) as f32
}
