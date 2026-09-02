//! Control-side console state: which stops are drawn, and which voices
//! each key press starts. This is deliberately outside the RT engine —
//! registration, couplers, and (later) microtonal key mappings all live
//! at this layer, where allocation and locking are fine.

use std::collections::HashMap;

use aristide_model::{
    CouplerRoute, CouplerScope, ManualId, Organ, PipeSource, RankId, RankRange, StopId,
};

use aristide_model::units::{cents_between, cents_to_ratio};
use aristide_engine::Command;

use crate::bank::VoiceSpec;
use crate::tuning::Tuning;

/// One coupler route as the console editor sees it: manuals as
/// console indexes, the target flattened. `to == None` with
/// `unison_off` is a pure silencing route.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CouplerRouteView {
    pub from: Option<usize>,
    pub to: Option<usize>,
    pub shift: i16,
    pub low: Option<u8>,
    pub high: Option<u8>,
    pub unison_off: bool,
    pub scope: CouplerScope,
    pub repitch: Option<bool>,
    pub own_pipes: bool,
}

/// A voice the console wants started, tagged with the handle it will
/// later be stopped by.
pub struct VoiceStart {
    pub handle: u64,
    pub spec: VoiceSpec,
}

impl VoiceStart {
    /// The engine command that starts this voice.
    pub fn command(&self) -> Command {
        Command::StartVoice {
            handle: self.handle,
            sample: self.spec.sample,
            rate: self.spec.rate,
            gain: self.spec.gain,
            group: self.spec.group,
            wind_weight: self.spec.wind_weight,
            brightness: self.spec.brightness,
            voicing_tilt: self.spec.voicing_tilt,
            enclosures: self.spec.enclosures,
            bus: self.spec.bus,
            delay_frames: self.spec.delay_frames,
            nominal_hz: self.spec.nominal_hz,
        }
    }
}

/// One `[[voicing.adjust]]` rule, resolved against a loaded organ:
/// which pipes of one stop it speaks about, and what it says about
/// them. A field left `None` says nothing — that is what lets a
/// narrow rule leave the rest of the voicing to a broader one.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TrimRule {
    /// Inclusive key span on the stop's own keyboard (absolute key
    /// numbers, the same space `[[manual]] hex` anchors live in);
    /// `None` = every key the stop answers to.
    pub keys: Option<(i32, i32)>,
    /// One rank inside the stop; `None` = all of them.
    pub rank: Option<RankId>,
    /// Linear gain.
    pub gain: Option<f32>,
    /// Pitch trim in cents, with any footage change already folded in.
    pub cents: Option<f64>,
    /// Treble tilt as a linear factor on the pipe's shelf.
    pub tilt: Option<f32>,
    /// Whether this rule is the console's own for its scope — a
    /// `[[voicing.adjust]]` naming exactly this stop, which the stop
    /// editor and the key-voicing popover show and rewrite. Rules that
    /// came from a name PATTERN are nobody's to edit: a live edit
    /// leaves them standing, and they keep contributing to the sound.
    pub owned: bool,
}

impl TrimRule {
    /// How specific this rule is — bigger wins. The measure is how
    /// FEW pipes it speaks about: a narrower key span first (a rule
    /// about one pipe is a statement about that pipe), then whether
    /// it names a rank. Two rules that address the same pipes are
    /// settled by file order at the call site.
    fn specificity(&self) -> (u32, u8) {
        let span = self
            .keys
            .map(|(low, high)| (high - low).unsigned_abs().saturating_add(1))
            .unwrap_or(u32::MAX);
        (u32::MAX - span, self.rank.is_some() as u8)
    }

    fn covers(&self, rank: RankId, key: i32) -> bool {
        self.keys.is_none_or(|(low, high)| (low..=high).contains(&key))
            && self.rank.is_none_or(|only| only == rank)
    }
}

/// What one pipe is priced with once the rules have been resolved.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VoiceTrim {
    pub gain: f32,
    pub cents: f64,
    pub tilt: f32,
}

impl Default for VoiceTrim {
    fn default() -> Self {
        VoiceTrim {
            gain: 1.0,
            cents: 0.0,
            tilt: 1.0,
        }
    }
}

/// What a live voicing edit does to the pipes already speaking.
#[derive(Debug, Clone, Default)]
pub struct Revoicing {
    /// `(handle, gain multiplier against note-on, tilt)` — the engine
    /// ramps the gain and swaps the tilt in place.
    pub trims: Vec<(u64, f32, f32)>,
    /// `(handle, rate)` — a pitch trim that stayed on the same pipe
    /// rides the ordinary glide.
    pub rates: Vec<(u64, f32)>,
    /// The pitch trim moved a key onto a different pipe: nothing can
    /// be glided, and the caller re-prices the stop instead.
    pub reprice: bool,
}

impl Revoicing {
    fn reprice() -> Revoicing {
        Revoicing {
            reprice: true,
            ..Revoicing::default()
        }
    }
}

/// One place a played key lands after coupling: the manual and key
/// that should sound, and the policies the route it travelled grants.
struct Landing {
    manual: ManualId,
    midi_key: i16,
    /// May pipes the division hasn't got be filled in by repitching a
    /// neighbour? True for the played key itself (repitching serves
    /// the player's keyboard) and for routes that opt in.
    fill: bool,
    /// Is the landing bounded by the destination's compass? The played
    /// key always is — the compass *is* the instrument. A repitching
    /// route is explicitly asked to synthesize what the instrument
    /// hasn't got (16' tone off the bottom of an 8' rank), so it
    /// reaches past the compass; a normal route stays inside it.
    bounded: bool,
    /// The pipe-sharing lane this landing speaks in: 0 for the played
    /// key and ordinary routes (one organ, pipes shared), a coupler's
    /// own lane for `own_pipes` routes — their copies double instead
    /// of merging (see [`PipeKey`]).
    lane: u32,
}

/// The refcount identity of one virtual pipe: WHERE the sound comes
/// from — the physical pipe, borrow chains followed — and WHAT it is
/// asked to sound — the offset from its recording, at whole-cent
/// resolution. However many keys, stops and couplers demand the same
/// physical pipe at the same pitch, they hold ONE voice between them
/// (a pipe cannot speak twice); the same physical pipe repitched two
/// ways is two different virtual pipes, and both speak — an out-of-
/// range C# and D both stood in for by top C still make a second.
///
/// `route_lane`/`stop_lane` carve deliberate exceptions: an
/// `own_pipes` coupler route or stop speaks in a lane of its own, so
/// its demands never merge with the shared organ's (or another
/// lane's) — the opt-in doubling.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct PipeKey {
    rank: RankId,
    pipe: u16,
    /// Sounded offset from the pipe's recording in cents: whole
    /// semitones of standing in for a missing neighbour plus the
    /// tuning's sub-semitone bend.
    cents: i32,
    /// 0 = shared; else 1 + the coupler index whose `own_pipes` route
    /// carried the demand.
    route_lane: u32,
    /// 0 = shared; else 1 + the id of the `own_pipes` stop sounding it.
    stop_lane: u32,
}

pub struct Console {
    organ: Organ,
    specs: HashMap<(RankId, u16), VoiceSpec>,
    drawn: Vec<StopId>,
    /// Engaged couplers, as indices into `organ.couplers`.
    engaged_couplers: Vec<usize>,
    /// Per coupler: still on the console? Picked in the Organ setup —
    /// an unavailable coupler is disengaged and hidden, never deleted,
    /// so it can come back without reloading the set.
    available_couplers: Vec<bool>,
    /// Groups of couplers permanently linked (indices into
    /// `organ.couplers`): engaging any member engages the rest,
    /// releasing likewise — one action wearing several rockers.
    coupler_links: Vec<Vec<usize>>,
    tuning: Tuning,
    /// Per manual index: a tuning of that division's own, overriding
    /// the console's — a 415 Hz meantone Positif against a 440 equal
    /// Great in one instrument.
    manual_tuning: Vec<Option<Tuning>>,
    /// Sample sets tuned apart from the instrument, by source alias:
    /// what a set's stops play unless their division has a tuning of
    /// its own or they are pinned elsewhere (see `Follow`).
    source_tuning: HashMap<String, Tuning>,
    /// Per stop: the alias of the source set it came from.
    stop_source: HashMap<StopId, String>,
    /// Per stop: what it follows when it has no tuning of its own —
    /// absent is `Follow::Auto`.
    stop_follow: HashMap<StopId, crate::tuning::Follow>,
    /// Stops with a tuning of their own.
    stop_tuning: HashMap<StopId, Tuning>,
    /// Ranks tuned apart *within* a stop (a mixture's tierce rank on
    /// its own): the stop is the unit the player sees, so a rank's
    /// tuning is keyed by the stop it is heard through.
    rank_tuning: HashMap<(StopId, RankId), Tuning>,
    /// Each source set's own recorded pitch (the instrument's class
    /// table at the set's anchor), stamped into set-scoped tunings so
    /// "as recorded" at set scope reads the set's own a′.
    source_home: HashMap<String, std::sync::Arc<crate::tuning::HomeTuning>>,
    /// Each rank's measured pitch anchor, cents from the 440 ladder —
    /// the same for rank-scoped tunings.
    rank_anchor: HashMap<RankId, f64>,
    /// Stops classified as control noises (drawstop thumps etc.) —
    /// hidden from UIs, triggered by the control they belong to.
    noise_stops: Vec<StopId>,
    stop_noise: HashMap<StopId, VoiceSpec>,
    coupler_noise: HashMap<usize, VoiceSpec>,
    tremulant_noise: Option<VoiceSpec>,
    /// Noise voices currently sounding their silent loop (control is
    /// on); note-off on toggle-off plays the push-in thump.
    stop_noise_open: HashMap<StopId, u64>,
    coupler_noise_open: HashMap<usize, u64>,
    trem_noise_open: Option<u64>,
    noises_enabled: bool,
    noise_volume: f32,
    /// Per manual index: the enclosures (engine indices) its stops sit
    /// inside — an expression pedal on that manual's channel drives
    /// them. Derived from stop→rank→windchest→enclosure membership.
    manual_enclosures: Vec<Vec<u8>>,
    /// Current pedal position per enclosure (1 = open, GO's default).
    enclosure_positions: Vec<f32>,
    next_handle: u64,
    /// (manual index, MIDI key) → pipes held by that key, tagged with
    /// the stop that engaged them (so retiring a stop can release
    /// them). `percussive` voices are excluded (they stop themselves).
    ///
    /// Pipes here (and in `last_pipe_voice` / `speaking`) are
    /// [`PipeKey`]s — the physical pipe plus the pitch it is asked to
    /// sound. That is what lets two keys borrow the same physical pipe
    /// at two pitches simultaneously, while the same pipe at the same
    /// pitch — however many stops, borrows and couplers reach it —
    /// merges into one voice.
    sounding: HashMap<(usize, u16), Vec<(StopId, PipeKey)>>,
    /// The velocity each held key was struck with — a stop drawn
    /// mid-hold prices its new voices at the press it joins, not at
    /// some fresh default.
    held_velocity: HashMap<(usize, u16), u8>,
    /// The most recent voice handle started per pipe — used to expedite
    /// a still-releasing (pallet-staggered) predecessor when the pipe
    /// re-speaks, so a pipe can never overlap itself at full level.
    /// (Repitched borrowings at other pitches don't count as "itself":
    /// different rates aren't phase-coherent, so they may overlap.)
    last_pipe_voice: HashMap<PipeKey, u64>,
    /// Each speaking pipe's voice handle and how many holders (keys,
    /// couplers) currently demand it. A pipe speaks ONCE no matter how
    /// many routes reach it — starting a second voice on the same pipe
    /// sums the identical recording coherently (+6 dB), and on release
    /// the phase aligner makes both tails coherent too: the release
    /// comes out LOUDER than the chord (the octave-coupled F-major pop).
    speaking: HashMap<PipeKey, Speaking>,
    /// Engine output rate; frequency-derived voice parameters have to be
    /// recomputed against it when a pipe is repitched.
    device_rate: f32,
    /// Whether a coupled voice may be repitched from a neighbouring
    /// pipe. False, and deliberately so: see `voices_for_key`.
    couplers_repitch: bool,
    /// Per stop: output bus and onset delay (frames), from the
    /// sidecar's `[routing]`/`[voicing]` — stamped onto each voice's
    /// spec as it is priced. Stops not named route to bus 0, delay 0.
    stop_routing: HashMap<StopId, (u8, u32)>,
    /// Per stop: its `[[voicing.adjust]]` rules in file order — the
    /// user's own level/tone/tuning trims, each narrowed to the pipes
    /// it speaks about. The cents ride the same pricing fold as
    /// tuning: whole semitones re-anchor keys to neighbouring pipes (a
    /// footage change is a unit-organ extension, not a tape-speed
    /// trick), the remainder bends.
    stop_adjust: HashMap<StopId, Vec<TrimRule>>,
    /// Per manual: the inclusive MIDI note range that manual answers to.
    /// Starts as the sample set's own compass and is widened to the
    /// player's keyboard (see `set_compass`) — a key outside it is
    /// silent, which is the locked compass rule with the player's
    /// hardware supplying the number.
    compass: Vec<(i16, i16)>,
    /// Pipes with several recorded attacks (GO multi-attack): the
    /// selection table `price` consults per press. Keyed like `specs`.
    attack_options: HashMap<(RankId, u16), Vec<crate::bank::AttackOption>>,
    /// When each pipe (at a sounded identity) last fully released —
    /// what "re-speaks within N ms" is measured against for the
    /// fast-repetition re-attack samples.
    last_released: HashMap<PipeKey, std::time::Instant>,
    /// Wave-tremulant state per engine wind group, as a bitmask —
    /// which recording variant (`wave_tremulant`) a chest's pipes
    /// should currently prefer.
    wave_trems: u32,
    /// Attack-selection tie-break state (GO randomizes among equally
    /// specific candidates so repetition doesn't machine-gun one file).
    rng: u32,
    /// Borrowed pipe → the physical pipe its chain ends at. What
    /// [`PipeKey`] identities resolve through, so a borrowing rank and
    /// its donor demand the SAME pipe, not two. Pipes absent here are
    /// their own physical selves.
    physical: HashMap<(RankId, u16), (RankId, u16)>,
    /// What the samples were recorded in, when they measured —
    /// stamped into every tuning installed here, so `Original` knows
    /// what the reference is pulling against.
    home: Option<std::sync::Arc<crate::tuning::HomeTuning>>,
}

/// One speaking pipe's voice, with the pitch bookkeeping live retuning
/// rides on: `rate` is the playback rate the voice last had before any
/// performance bend, `deviation` the exact cents its pitch was priced
/// at (the tuning's `deviation_cents` for its key when it started or
/// last drifted), and `bend` the per-note performance bend (MPE)
/// currently on top. Rate updates are exact cent deltas against
/// `deviation`, so a drifting tuning never cares which physical pipe
/// the voice actually plays.
#[derive(Debug, Clone, Copy)]
struct Speaking {
    handle: u64,
    holders: u32,
    rate: f32,
    deviation: f64,
    bend: f64,
    /// The pipe's measured offset from its nominal (`home_cents`) and
    /// the fitted model's (`model_cents`): what a target subtracts
    /// (one or the other, see `Tuning::pipe_offset`).
    home: f64,
    model: f64,
    /// Where the pitch was priced: the (post-transpose) MIDI key on the
    /// sounding manual, and the stop and rank it sounded through —
    /// which decide its tuning (a rank inside a mixture may have its
    /// own). Live retuning re-prices exactly this coordinate, so a
    /// later transpose change — which reroutes future presses — never
    /// moves a held pipe.
    ladder_key: i16,
    stop: StopId,
    rank: RankId,
    /// The voicing trim this voice was priced with, and the gain part
    /// of it as it stood at note-on. A live re-voicing sends the
    /// engine the ratio against the latter, so the press's velocity
    /// and the pipe's own recorded level survive untouched however
    /// many times the voicer moves the knob.
    trim: VoiceTrim,
    start_gain: f32,
    /// The whole-semitone re-anchoring the pitch trim caused. A live
    /// cents edit that leaves it alone is a glide; one that moves it
    /// picks a different pipe, and the stop has to re-speak.
    shift: i16,
}

impl Speaking {
    /// The rate the engine should run: base rate with the bend on top.
    fn bent_rate(&self) -> f32 {
        self.rate * cents_to_ratio(self.bend) as f32
    }
}

/// One voice a key press demands, fully priced (see `voices_for_key`).
#[derive(Debug, Clone, Copy)]
struct KeyVoice {
    stop: StopId,
    /// The range's rank and pipe index in it — the key for the
    /// pipe-level tables (attack options); `key` is the refcount
    /// identity, borrow chains resolved.
    rank: RankId,
    pipe: u16,
    key: PipeKey,
    deviation: f64,
    home: f64,
    model: f64,
    ladder_key: i16,
    trim: VoiceTrim,
    shift: i16,
    spec: VoiceSpec,
}

/// The engine's xorshift, control-side: cheap, stateful, deterministic
/// per console — all attack tie-breaking needs.
fn xorshift(state: &mut u32) -> u32 {
    let mut x = *state;
    x ^= x << 13;
    x ^= x >> 17;
    x ^= x << 5;
    *state = x;
    x
}

impl Console {
    pub fn new(
        organ: Organ,
        specs: HashMap<(RankId, u16), VoiceSpec>,
        drawn: Vec<StopId>,
        device_rate: f32,
    ) -> Console {
        let compass = organ
            .manuals
            .iter()
            .map(|manual| {
                let first = manual.first_midi_note as i16;
                (first, first + manual.key_count as i16 - 1)
            })
            .collect();
        let mut console = Console {
            organ,
            specs,
            drawn,
            device_rate,
            couplers_repitch: false,
            compass,
            engaged_couplers: Vec::new(),
            available_couplers: Vec::new(),
            coupler_links: Vec::new(),
            tuning: Tuning::default(),
            manual_tuning: Vec::new(),
            source_tuning: HashMap::new(),
            stop_source: HashMap::new(),
            stop_follow: HashMap::new(),
            stop_tuning: HashMap::new(),
            rank_tuning: HashMap::new(),
            source_home: HashMap::new(),
            rank_anchor: HashMap::new(),
            home: None,
            noise_stops: Vec::new(),
            stop_noise: HashMap::new(),
            coupler_noise: HashMap::new(),
            tremulant_noise: None,
            stop_noise_open: HashMap::new(),
            coupler_noise_open: HashMap::new(),
            trem_noise_open: None,
            noises_enabled: true,
            noise_volume: 0.7,
            manual_enclosures: Vec::new(),
            enclosure_positions: Vec::new(),
            next_handle: 0,
            sounding: HashMap::new(),
            held_velocity: HashMap::new(),
            speaking: HashMap::new(),
            stop_routing: HashMap::new(),
            stop_adjust: HashMap::new(),
            last_pipe_voice: HashMap::new(),
            attack_options: HashMap::new(),
            last_released: HashMap::new(),
            wave_trems: 0,
            rng: 0x2F6E2B1,
            physical: HashMap::new(),
        };
        console.physical = physical_alias(&console.organ);
        console.classify_noises();
        // Noise stops must never be part of the registration.
        let noise_stops = console.noise_stops.clone();
        console.drawn.retain(|id| !noise_stops.contains(id));
        console.map_enclosures();
        console
    }

    /// Which enclosures each manual's expression pedal drives: every
    /// box that any of the manual's stops (via rank → windchest) sits
    /// inside. Noise stops don't count — a drawstop thump rank on an
    /// effects chest must not capture a pedal.
    fn map_enclosures(&mut self) {
        // Re-mapping (a stop moved) must not slam every box open.
        if self.enclosure_positions.len() != self.organ.enclosures.len() {
            self.enclosure_positions = vec![1.0; self.organ.enclosures.len()];
        }
        self.manual_enclosures = vec![Vec::new(); self.organ.manuals.len()];
        let max = aristide_engine::enclosure::MAX_ENCLOSURES as u32;
        for stop in &self.organ.stops {
            if self.noise_stops.contains(&stop.id) {
                continue;
            }
            let Some(manual_index) = self
                .organ
                .manuals
                .iter()
                .position(|m| m.id == stop.manual)
            else {
                continue;
            };
            for range in &stop.ranks {
                let Some(rank) = self.organ.rank(range.rank) else {
                    continue;
                };
                let Some(chest) = self
                    .organ
                    .windchests
                    .iter()
                    .find(|c| c.number == rank.windchest)
                else {
                    continue;
                };
                // Every box the chest sits in, not just the first: a
                // manual whose stops stand in a box inside another box
                // must drive both from its expression pedal.
                for &enclosure in chest.enclosures.iter() {
                    if enclosure < max
                        && !self.manual_enclosures[manual_index].contains(&(enclosure as u8))
                    {
                        self.manual_enclosures[manual_index].push(enclosure as u8);
                    }
                }
            }
        }
    }

    /// Expression pedal: move every enclosure this manual's stops sit
    /// inside. Returns (engine index, position) pairs for the control
    /// loop to forward.
    pub fn expression_manual(&mut self, manual_index: usize, value: u8) -> Vec<(u8, f32)> {
        if manual_index >= self.manual_enclosures.len() {
            return Vec::new();
        }
        let position = value.min(127) as f32 / 127.0;
        let mut moves = Vec::new();
        for &enclosure in &self.manual_enclosures[manual_index] {
            if let Some(slot) = self.enclosure_positions.get_mut(enclosure as usize) {
                *slot = position;
            }
            moves.push((enclosure, position));
        }
        moves
    }

    /// UI pedal move on one enclosure by model index.
    pub fn set_enclosure(&mut self, index: usize, position: f32) -> Option<(u8, f32)> {
        let position = position.clamp(0.0, 1.0);
        *self.enclosure_positions.get_mut(index)? = position;
        (index < aristide_engine::enclosure::MAX_ENCLOSURES)
            .then_some((index as u8, position))
    }

    /// (model index, name, position, displayed) per enclosure, for UIs.
    pub fn enclosure_states(&self) -> Vec<(usize, String, f32, bool)> {
        self.organ
            .enclosures
            .iter()
            .enumerate()
            .map(|(index, enclosure)| {
                (
                    index,
                    enclosure.name.clone(),
                    self.enclosure_positions.get(index).copied().unwrap_or(1.0),
                    enclosure.displayed,
                )
            })
            .collect()
    }

    /// Control-noise "stops" (drawstop thumps, coupler clacks, blower).
    /// GO-set convention: the noise sample is structured like a pipe —
    /// pull-thump attack → near-silent sustain loop → push-in thump as
    /// the release tail — so the engine's own note lifecycle plays it:
    /// draw = note-on, retire = note-off. Classified by name (the
    /// GO-world convention); mapped to their control by fuzzy match.
    fn classify_noises(&mut self) {
        let mut noise_stops = Vec::new();
        for stop in &self.organ.stops {
            if stop.name.to_lowercase().contains("noise") {
                noise_stops.push(stop.id);
            }
        }
        self.noise_stops = noise_stops;

        for &noise_id in &self.noise_stops {
            let Some(noise_stop) = self.organ.stops.iter().find(|s| s.id == noise_id) else {
                continue;
            };
            let Some(spec) = noise_stop.ranks.first().and_then(|range| {
                self.specs.get(&(range.rank, range.first_pipe)).copied()
            }) else {
                continue;
            };
            let name = noise_stop.name.to_lowercase();

            if name.contains("tremblant") || name.contains("tremulant") {
                self.tremulant_noise = Some(spec);
                continue;
            }
            if name.contains("coupler") {
                let stripped = strip_noise_suffix(&noise_stop.name);
                let best = self
                    .organ
                    .couplers
                    .iter()
                    .enumerate()
                    .map(|(index, coupler)| (index, name_match_score(&stripped, &coupler.name)))
                    .max_by(|a, b| a.1.total_cmp(&b.1));
                if let Some((index, score)) = best {
                    if score >= 0.5 {
                        self.coupler_noise.insert(index, spec);
                    }
                }
                continue;
            }
            // Ordinary drawstop noise: match against real stops on the
            // same manual.
            let stripped = strip_noise_suffix(&noise_stop.name);
            let best = self
                .organ
                .stops
                .iter()
                .filter(|s| s.manual == noise_stop.manual && !self.noise_stops.contains(&s.id))
                .map(|s| (s.id, name_match_score(&stripped, &s.name)))
                .max_by(|a, b| a.1.total_cmp(&b.1));
            if let Some((id, score)) = best {
                if score >= 0.45 {
                    self.stop_noise.insert(id, spec);
                }
            }
        }
    }

    /// Open a noise voice (pull thump into its silent loop). The voice
    /// stays alive until the control toggles back off — its note-off
    /// then plays the push-in thump from the sample's tail. Noise
    /// voices never draw wind or breathe with pressure.
    fn open_noise(&mut self, spec: Option<VoiceSpec>) -> Option<VoiceStart> {
        let mut spec = spec?;
        if !self.noises_enabled || self.noise_volume <= 0.0 {
            return None;
        }
        spec.gain *= self.noise_volume;
        spec.wind_weight = 0.0;
        spec.brightness = 0.0;
        let handle = self.next_handle;
        self.next_handle += 1;
        Some(VoiceStart { handle, spec })
    }

    /// Enable/disable noises and set their volume. Returns handles of
    /// currently open noise voices to KILL (silently) when disabling.
    pub fn set_noises(&mut self, enabled: bool, volume: f32) -> Vec<u64> {
        self.noises_enabled = enabled;
        self.noise_volume = volume.clamp(0.0, 2.0);
        if enabled {
            return Vec::new();
        }
        let mut kills: Vec<u64> = self.stop_noise_open.drain().map(|(_, h)| h).collect();
        kills.extend(self.coupler_noise_open.drain().map(|(_, h)| h));
        kills.extend(self.trem_noise_open.take());
        kills
    }

    pub fn noises(&self) -> (bool, f32) {
        (self.noises_enabled, self.noise_volume)
    }

    /// Tremulant toggled: open the trem noise voice or release it
    /// (note-off plays the stop-side thump). Returns (start, stop).
    pub fn tremulant_toggle_noise(&mut self, engaged: bool) -> (Option<VoiceStart>, Option<u64>) {
        if engaged {
            let start = self.open_noise(self.tremulant_noise);
            self.trem_noise_open = start.as_ref().map(|s| s.handle);
            (start, None)
        } else {
            (None, self.trem_noise_open.take())
        }
    }

    pub fn set_tuning(&mut self, mut tuning: Tuning) {
        tuning.home = self.home.clone();
        self.tuning = tuning;
    }

    /// Install what the samples were recorded in (from the bank's
    /// fit): every tuning held here, now and later, is stamped with
    /// it. The instrument's reference, if it still reads as the
    /// default a′ = 440, becomes the organ's own a′ — "as recorded"
    /// means exactly that until the player pulls it elsewhere.
    pub fn set_home(&mut self, home: Option<std::sync::Arc<crate::tuning::HomeTuning>>) {
        self.home = home.clone();
        self.restamp_homes();
    }

    /// Each source set's own recorded pitch, by alias, and each rank's
    /// measured anchor (cents from the 440 ladder) — what set- and
    /// rank-scoped tunings measure "as recorded" against.
    pub fn set_scope_homes(
        &mut self,
        sources: HashMap<String, std::sync::Arc<crate::tuning::HomeTuning>>,
        ranks: HashMap<RankId, f64>,
    ) {
        self.source_home = sources;
        self.rank_anchor = ranks;
        self.restamp_homes();
    }

    fn restamp_homes(&mut self) {
        self.tuning.home = self.home.clone();
        for tuning in self.manual_tuning.iter_mut().flatten() {
            tuning.home = self.home.clone();
        }
        let sources: Vec<String> = self.source_tuning.keys().cloned().collect();
        for alias in sources {
            let home = self.source_home_of(&alias);
            if let Some(tuning) = self.source_tuning.get_mut(&alias) {
                tuning.home = home;
            }
        }
        let stops: Vec<StopId> = self.stop_tuning.keys().copied().collect();
        for stop in stops {
            let home = self.stop_home_of(stop);
            if let Some(tuning) = self.stop_tuning.get_mut(&stop) {
                tuning.home = home;
            }
        }
        let ranks: Vec<(StopId, RankId)> = self.rank_tuning.keys().copied().collect();
        for at in ranks {
            let home = self.rank_home_of(at.1);
            if let Some(tuning) = self.rank_tuning.get_mut(&at) {
                tuning.home = home;
            }
        }
    }

    pub fn home(&self) -> Option<std::sync::Arc<crate::tuning::HomeTuning>> {
        self.home.clone()
    }

    /// A set's own recorded pitch, else the instrument's.
    pub fn source_home_of(&self, alias: &str) -> Option<std::sync::Arc<crate::tuning::HomeTuning>> {
        self.source_home.get(alias).cloned().or_else(|| self.home.clone())
    }

    fn stop_home_of(&self, stop: StopId) -> Option<std::sync::Arc<crate::tuning::HomeTuning>> {
        match self.stop_source.get(&stop) {
            Some(alias) => self.source_home_of(alias),
            None => self.home.clone(),
        }
    }

    /// A rank's own recorded pitch: the instrument's table at the
    /// rank's measured anchor.
    pub fn rank_home_of(&self, rank: RankId) -> Option<std::sync::Arc<crate::tuning::HomeTuning>> {
        match (self.rank_anchor.get(&rank), &self.home) {
            (Some(&anchor), Some(home)) => Some(std::sync::Arc::new(home.at_anchor(anchor))),
            _ => self.home.clone(),
        }
    }

    /// Which source set each stop was pulled from, by alias.
    pub fn set_stop_sources(&mut self, sources: HashMap<StopId, String>) {
        self.stop_source = sources;
        self.restamp_homes();
    }

    pub fn stop_source(&self, stop: StopId) -> Option<&str> {
        self.stop_source.get(&stop).map(String::as_str)
    }

    /// Give one sample set a tuning of its own, or with `None` return
    /// it to the instrument's. Transposition is a keyboard's, so a
    /// set's tuning never carries one.
    pub fn set_source_tuning(&mut self, alias: &str, tuning: Option<Tuning>) {
        match tuning {
            Some(mut tuning) => {
                tuning.transpose = 0;
                tuning.home = self.source_home_of(alias);
                self.source_tuning.insert(alias.to_string(), tuning);
            }
            None => {
                self.source_tuning.remove(alias);
            }
        }
    }

    pub fn source_tuning(&self, alias: &str) -> Option<Tuning> {
        self.source_tuning.get(alias).cloned()
    }

    /// Every set tuned apart, alias-sorted.
    pub fn source_tunings(&self) -> Vec<(String, Tuning)> {
        let mut all: Vec<(String, Tuning)> = self
            .source_tuning
            .iter()
            .map(|(alias, tuning)| (alias.clone(), tuning.clone()))
            .collect();
        all.sort_by(|a, b| a.0.cmp(&b.0));
        all
    }

    /// Pin what a stop follows (dropping any tuning of its own), or
    /// give it a tuning of its own with `Err(tuning)` — hence the odd
    /// shape: exactly one of the two is ever true of a stop.
    pub fn set_stop_follow(&mut self, stop: StopId, follow: crate::tuning::Follow) {
        self.stop_tuning.remove(&stop);
        if follow == crate::tuning::Follow::Auto {
            self.stop_follow.remove(&stop);
        } else {
            self.stop_follow.insert(stop, follow);
        }
    }

    /// Give one stop a tuning of its own (its pin, if any, goes: a stop
    /// with its own tuning follows nothing), or with `None` return it
    /// to following automatically.
    pub fn set_stop_tuning(&mut self, stop: StopId, tuning: Option<Tuning>) {
        match tuning {
            Some(mut tuning) => {
                tuning.transpose = 0;
                tuning.home = self.stop_home_of(stop);
                self.stop_follow.remove(&stop);
                self.stop_tuning.insert(stop, tuning);
            }
            None => {
                self.stop_tuning.remove(&stop);
                self.stop_follow.remove(&stop);
            }
        }
    }

    pub fn stop_follow(&self, stop: StopId) -> crate::tuning::Follow {
        self.stop_follow.get(&stop).copied().unwrap_or_default()
    }

    pub fn stop_own_tuning(&self, stop: StopId) -> Option<Tuning> {
        self.stop_tuning.get(&stop).cloned()
    }

    /// Stops pinned or tuned apart, in stop-id order: `(stop, follow,
    /// own tuning)`.
    pub fn stop_tunings(&self) -> Vec<(StopId, crate::tuning::Follow, Option<Tuning>)> {
        let mut ids: Vec<StopId> = self
            .stop_follow
            .keys()
            .chain(self.stop_tuning.keys())
            .copied()
            .collect();
        ids.sort_by_key(|id| id.0);
        ids.dedup();
        ids.into_iter()
            .map(|stop| (stop, self.stop_follow(stop), self.stop_own_tuning(stop)))
            .collect()
    }

    /// Tune one rank apart within a stop, or with `None` return it to
    /// the stop's tuning.
    pub fn set_rank_tuning(&mut self, stop: StopId, rank: RankId, tuning: Option<Tuning>) {
        match tuning {
            Some(mut tuning) => {
                tuning.transpose = 0;
                tuning.home = self.rank_home_of(rank);
                self.rank_tuning.insert((stop, rank), tuning);
            }
            None => {
                self.rank_tuning.remove(&(stop, rank));
            }
        }
    }

    pub fn rank_tuning(&self, stop: StopId, rank: RankId) -> Option<Tuning> {
        self.rank_tuning.get(&(stop, rank)).cloned()
    }

    /// Ranks tuned apart within their stops, sorted by (stop, rank).
    pub fn rank_tunings(&self) -> Vec<(StopId, RankId, Tuning)> {
        let mut all: Vec<(StopId, RankId, Tuning)> = self
            .rank_tuning
            .iter()
            .map(|(&(stop, rank), tuning)| (stop, rank, tuning.clone()))
            .collect();
        all.sort_by_key(|(stop, rank, _)| (stop.0, rank.0));
        all
    }

    /// Does this stop's keyboard have note names? Hand manuals and
    /// pedalboards do; a microtonal board's keys are numbers and
    /// nothing else, so writers spell its key spans as numbers (the
    /// declared kind decides, never a guess — see `ManualKind`).
    pub fn stop_has_note_names(&self, stop: StopId) -> bool {
        self.organ
            .stops
            .iter()
            .find(|s| s.id == stop)
            .and_then(|s| self.organ.manuals.iter().find(|m| m.id == s.manual))
            .is_none_or(|manual| manual.kind != aristide_model::ManualKind::Microtonal)
    }

    /// The distinct ranks a stop sounds, in range order, with their
    /// names — the units a mixture can be tuned by.
    pub fn stop_ranks(&self, stop: StopId) -> Vec<(RankId, &str)> {
        let Some(stop) = self.organ.stops.iter().find(|s| s.id == stop) else {
            return Vec::new();
        };
        let mut seen = Vec::new();
        for range in &stop.ranks {
            if seen.iter().any(|(id, _)| *id == range.rank) {
                continue;
            }
            let name = self
                .organ
                .rank(range.rank)
                .map(|rank| rank.name.as_str())
                .unwrap_or("");
            seen.push((range.rank, name));
        }
        seen
    }

    /// What a stop plays under, resolved: its own tuning; else what its
    /// pin names; else (automatically) its division's own, its set's
    /// own, the instrument's — in that order. The division wins over
    /// the set because what a keyboard plays is a performance decision
    /// and a keyboard silently playing the wrong scale on some of its
    /// stops is the worse failure; the pin is there for the set that
    /// must not be retuned whatever its keyboard does.
    pub fn stop_tuning_resolved(&self, stop: StopId) -> (&Tuning, crate::tuning::TuningScope) {
        use crate::tuning::{Follow, TuningScope};
        if let Some(own) = self.stop_tuning.get(&stop) {
            return (own, TuningScope::Stop);
        }
        let division = self
            .organ
            .stops
            .iter()
            .find(|s| s.id == stop)
            .and_then(|s| self.organ.manuals.iter().position(|m| m.id == s.manual))
            .and_then(|index| self.manual_tuning.get(index))
            .and_then(|tuning| tuning.as_ref())
            .map(|tuning| (tuning, TuningScope::Division));
        let source = self
            .stop_source
            .get(&stop)
            .and_then(|alias| self.source_tuning.get(alias))
            .map(|tuning| (tuning, TuningScope::Source));
        let organ = (&self.tuning, TuningScope::Organ);
        match self.stop_follow(stop) {
            Follow::Auto => division.or(source).unwrap_or(organ),
            Follow::Division => division.unwrap_or(organ),
            Follow::Source => source.unwrap_or(organ),
            Follow::Organ => organ,
        }
    }

    /// The tuning one voice is priced under: the rank's own within
    /// this stop, else the stop's resolution.
    fn voice_tuning(&self, stop: StopId, rank: RankId) -> (&Tuning, crate::tuning::TuningScope) {
        match self.rank_tuning.get(&(stop, rank)) {
            Some(own) => (own, crate::tuning::TuningScope::Rank),
            None => self.stop_tuning_resolved(stop),
        }
    }

    pub fn tuning(&self) -> Tuning {
        self.tuning.clone()
    }

    /// Give one division a tuning of its own, or with `None` return it
    /// to the console's. Applies from the next note on; callers that
    /// want held voices to follow call `retune_held` after and forward
    /// the updates as ramped SetVoiceRate commands.
    pub fn set_manual_tuning(&mut self, manual_index: usize, mut tuning: Option<Tuning>) {
        if manual_index >= self.organ.manuals.len() {
            return;
        }
        if let Some(tuning) = tuning.as_mut() {
            tuning.home = self.home.clone();
        }
        if self.manual_tuning.len() < self.organ.manuals.len() {
            self.manual_tuning.resize(self.organ.manuals.len(), None);
        }
        self.manual_tuning[manual_index] = tuning;
    }

    pub fn manual_tuning(&self, manual_index: usize) -> Option<Tuning> {
        self.manual_tuning.get(manual_index).cloned().flatten()
    }

    /// Install the per-stop routing/voicing table (bus index, onset
    /// delay in frames). Applies from the next voice started.
    pub fn set_stop_routing(&mut self, routing: HashMap<StopId, (u8, u32)>) {
        self.stop_routing = routing;
    }

    /// Install the sidecar's voicing rules, resolved per stop.
    pub fn set_stop_adjust(&mut self, adjust: HashMap<StopId, Vec<TrimRule>>) {
        self.stop_adjust = adjust;
    }

    /// One stop's rules, live — the console editor's seam. An empty
    /// list drops the entry. Applies from the next voice priced; the
    /// caller re-voices held keys if the change should land under
    /// them.
    pub fn set_stop_adjust_rules(&mut self, stop: StopId, rules: Vec<TrimRule>) {
        if rules.is_empty() {
            self.stop_adjust.remove(&stop);
        } else {
            self.stop_adjust.insert(stop, rules);
        }
    }

    /// A stop's rules, in file order.
    pub fn stop_adjust_rules(&self, stop: StopId) -> &[TrimRule] {
        self.stop_adjust.get(&stop).map_or(&[], |rules| rules)
    }

    /// What one pipe of one stop is voiced at: the rule for each field
    /// separately, taken from the most specific rule that says
    /// anything about it.
    ///
    /// Rules do NOT stack. A voicer setting a pipe's level is stating
    /// what that pipe should do, not adding an offset to what someone
    /// else said — so a `keys = "C2..B2"` rule that gives a gain
    /// replaces the stop-wide gain over those keys and leaves the
    /// stop-wide cents and tilt exactly as they were. "Most specific"
    /// is measured as "speaks about the fewest keys", then "names a
    /// rank", then "comes later in the file" — see
    /// [`TrimRule::specificity`].
    pub fn trim_for(&self, stop: StopId, rank: RankId, key: i32) -> VoiceTrim {
        let Some(rules) = self.stop_adjust.get(&stop) else {
            return VoiceTrim::default();
        };
        let mut trim = VoiceTrim::default();
        let (mut best_gain, mut best_cents, mut best_tilt) = (None, None, None);
        for (index, rule) in rules.iter().enumerate() {
            if !rule.covers(rank, key) {
                continue;
            }
            let rank = (rule.specificity(), index);
            if rule.gain.is_some() && best_gain.is_none_or(|best| best <= rank) {
                best_gain = Some(rank);
                trim.gain = rule.gain.expect("just checked");
            }
            if rule.cents.is_some() && best_cents.is_none_or(|best| best <= rank) {
                best_cents = Some(rank);
                trim.cents = rule.cents.expect("just checked");
            }
            if rule.tilt.is_some() && best_tilt.is_none_or(|best| best <= rank) {
                best_tilt = Some(rank);
                trim.tilt = rule.tilt.expect("just checked");
            }
        }
        trim
    }

    /// Rename a coupler on the live console — a rocker's engraving,
    /// nothing sounding moves. False if the index names no coupler.
    pub fn rename_coupler(&mut self, index: usize, name: &str) -> bool {
        match self.organ.couplers.get_mut(index) {
            Some(coupler) => {
                coupler.name = name.to_string();
                true
            }
            None => false,
        }
    }

    /// A coupler's routes with the manuals resolved to console indexes
    /// — what the snapshot carries so the editor popover can show and
    /// edit them. A route whose manual the organ hasn't got reads as
    /// None (loaders prevent it, but JSON must stay honest).
    pub fn coupler_route_views(&self, index: usize) -> Vec<CouplerRouteView> {
        let position =
            |id: ManualId| self.organ.manuals.iter().position(|manual| manual.id == id);
        self.organ
            .couplers
            .get(index)
            .map(|coupler| coupler.routes.as_slice())
            .unwrap_or_default()
            .iter()
            .map(|route| CouplerRouteView {
                from: position(route.from_manual),
                to: route.target.as_ref().and_then(|t| position(t.manual)),
                shift: route.target.as_ref().map_or(0, |t| t.key_shift),
                low: route.low_key,
                high: route.high_key,
                unison_off: route.unison_off,
                scope: route.scope,
                repitch: route.target.as_ref().and_then(|t| t.repitch),
                own_pipes: route.target.as_ref().is_some_and(|t| t.own_pipes),
            })
            .collect()
    }

    /// Whether a stop speaks pipes of its own (doubling pipes other
    /// stops sound) rather than sharing them — see [`PipeKey`].
    pub fn stop_own_pipes(&self, stop: StopId) -> bool {
        self.organ
            .stops
            .iter()
            .find(|s| s.id == stop)
            .is_some_and(|s| s.own_pipes)
    }

    /// Redeclare a stop's pipe sharing, live: held keys re-derive at
    /// once, so a voice merges into the pipe it now shares (or an own
    /// lane's copy starts). Returns (handles to stop, voices to start)
    /// like the other live re-derivations.
    pub fn set_stop_own_pipes(&mut self, stop: StopId, own: bool) -> (Vec<u64>, Vec<VoiceStart>) {
        let Some(entry) = self.organ.stops.iter_mut().find(|s| s.id == stop) else {
            return (Vec::new(), Vec::new());
        };
        if entry.own_pipes == own {
            return (Vec::new(), Vec::new());
        }
        entry.own_pipes = own;
        let mut stops = Vec::new();
        let mut starts = Vec::new();
        self.recouple_held_keys(&mut stops, &mut starts);
        stops.sort_unstable();
        stops.dedup();
        (stops, starts)
    }

    /// Rename a stop on the live console — a label, nothing sounding
    /// moves. False if the id names no stop.
    pub fn rename_stop(&mut self, stop: StopId, name: &str) -> bool {
        match self.organ.stops.iter_mut().find(|s| s.id == stop) {
            Some(entry) => {
                entry.name = name.to_string();
                true
            }
            None => false,
        }
    }

    /// The footage this stop's own samples speak at, before any trim:
    /// the recorded pitch of the pipe under a key, against the 8'
    /// unison for that key (the recorded 12-EDO ladder). None for a
    /// stop whose ranges disagree by more than a quarter tone — a
    /// mixture speaks several footages and has no single number — or
    /// whose pipes carry no pitch at all.
    pub fn stop_native_footage(&self, stop: StopId) -> Option<f64> {
        let stop = self.organ.stops.iter().find(|s| s.id == stop)?;
        let manual = self.organ.manuals.iter().find(|m| m.id == stop.manual)?;
        let mut footage: Option<f64> = None;
        for range in &stop.ranks {
            let feet = (0..range.key_count).find_map(|at| {
                let spec = self.specs.get(&(range.rank, range.first_pipe + at))?;
                if spec.nominal_hz <= 0.0 {
                    return None;
                }
                let key_midi = manual.first_midi_note as f64 + (range.first_key + at) as f64;
                let unison = 440.0 * ((key_midi - 69.0) / 12.0).exp2();
                Some(8.0 * unison / spec.nominal_hz as f64)
            });
            let Some(feet) = feet else {
                continue; // a range of silent placeholders says nothing
            };
            match footage {
                None => footage = Some(feet),
                Some(seen) if cents_between(feet, seen).abs() <= 50.0 => {}
                Some(_) => return None,
            }
        }
        footage
    }

    /// Re-price one drawn stop under whatever keys are held — how a
    /// live pitch trim lands without a rebuild. Its voices restart (a
    /// key re-anchored to another pipe is a different recording, so a
    /// mid-speech glide can't cover every case): the caller sends the
    /// stops, then the starts.
    /// Land a voicing change on the pipes that are already speaking.
    ///
    /// Level and tone move in place: the engine ramps them under the
    /// held key, so a knob drag is a fade and no pipe re-attacks. A
    /// pitch trim glides the same way — unless it crossed a semitone,
    /// which re-anchors the key onto a DIFFERENT pipe (a footage
    /// change is a unit-organ extension, not a tape-speed trick); then
    /// nothing can be glided and the caller re-prices the stop.
    pub fn revoice_stop(&mut self, stop: StopId) -> Revoicing {
        let voices: Vec<(PipeKey, i16, RankId, VoiceTrim, f32, i16)> = self
            .speaking
            .iter()
            .filter(|(_, voice)| voice.stop == stop)
            .map(|(&at, voice)| {
                (at, voice.ladder_key, voice.rank, voice.trim, voice.start_gain, voice.shift)
            })
            .collect();
        // Read everything first: whether a re-price is needed is a
        // property of the whole stop, and half-glided voices under a
        // stop that then re-speaks would be a mess.
        let mut settled = Vec::new();
        for (at, ladder_key, rank, was, start_gain, shift) in voices {
            let now = self.trim_for(stop, rank, ladder_key as i32);
            let (tuning, _) = self.voice_tuning(stop, rank);
            let Some(deviation) = tuning.deviation_cents(ladder_key.max(0) as u16) else {
                return Revoicing::reprice();
            };
            // The same anchoring `voices_for_key` does, asked again
            // with the new trim.
            let anchored_on = if tuning.corrects_pipes() {
                deviation + now.cents
            } else {
                now.cents
            };
            if (anchored_on / 100.0).round() as i16 != shift {
                return Revoicing::reprice();
            }
            settled.push((at, was, now, start_gain));
        }
        let mut out = Revoicing::default();
        for (at, was, now, start_gain) in settled {
            let Some(voice) = self.speaking.get_mut(&at) else {
                continue;
            };
            if (now.cents - was.cents).abs() > 1e-6 {
                voice.rate *= cents_to_ratio(now.cents - was.cents) as f32;
                out.rates.push((voice.handle, voice.bent_rate()));
            }
            if now.gain != was.gain || now.tilt != was.tilt {
                // Against the gain the voice STARTED at, not the last
                // edit's: the engine's trim is absolute, so a dropped
                // command can never compound into the wrong level.
                out.trims
                    .push((voice.handle, now.gain / start_gain.max(1e-6), now.tilt));
            }
            voice.trim = now;
        }
        out
    }

    pub fn reprice_stop(&mut self, stop: StopId) -> (Vec<u64>, Vec<VoiceStart>) {
        if !self.is_drawn(stop) {
            return (Vec::new(), Vec::new());
        }
        let (stopped, _) = self.set_drawn(stop, false);
        let (_, starts) = self.set_drawn(stop, true);
        (stopped, starts)
    }

    /// Re-price every held voice under the current tunings and return
    /// `(handle, rate)` for those that moved — the live-drift seam: a
    /// tuning change lands on sounding pipes as a glide instead of
    /// waiting for the next press. Each voice is re-priced at the
    /// coordinate it was priced at when it started (its sounding
    /// manual and post-transpose key), so a transpose change — which
    /// reroutes future presses to other pipes — never moves a held
    /// one. The voice keeps the sample it started with (a pipe mid-
    /// speech is not re-recorded): the new rate is the exact cent
    /// delta applied to the old, and which physical pipe sounds is
    /// irrelevant. A key the new tuning unmaps keeps sounding as it
    /// was (note-off still finds it); a pipe several keys share
    /// follows the holder that started it.
    pub fn retune_held(&mut self) -> Vec<(u64, f32)> {
        let voices: Vec<(PipeKey, i16, f64, f64, StopId, RankId)> = self
            .speaking
            .iter()
            .map(|(&at, voice)| {
                (at, voice.ladder_key, voice.home, voice.model, voice.stop, voice.rank)
            })
            .collect();
        let mut updates = Vec::new();
        for (at, ladder_key, home, model, stop, rank) in voices {
            let (tuning, _) = self.voice_tuning(stop, rank);
            let Some(deviation) = tuning.deviation_cents(ladder_key.max(0) as u16) else {
                continue;
            };
            // Priced net of the pipe's own offset under a target, as
            // at note-on — so leaving `Original` for "440 equal" pulls
            // each held pipe from where it really is.
            let deviation = deviation - tuning.pipe_offset(home, model);
            let Some(voice) = self.speaking.get_mut(&at) else {
                continue;
            };
            let delta = deviation - voice.deviation;
            if delta.abs() < 1e-3 {
                continue;
            }
            voice.rate *= cents_to_ratio(delta) as f32;
            voice.deviation = deviation;
            updates.push((voice.handle, voice.bent_rate()));
        }
        updates
    }

    /// Apply a per-note performance bend to everything a held key is
    /// sounding — the MPE/MIDI 2.0 per-note pitch seam. `cents` is
    /// absolute (each message replaces the last; 0 clears), applied on
    /// top of whatever the tuning gave the voice, and it survives a
    /// tuning drift. Returns `(handle, rate)` updates for the engine.
    /// A pipe shared with other holders bends wholly — one pipe, one
    /// pitch.
    pub fn bend_key(&mut self, manual_index: usize, key: u16, cents: f64) -> Vec<(u64, f32)> {
        let mut updates = Vec::new();
        let Some(holds) = self.sounding.get(&(manual_index, key)).cloned() else {
            return updates;
        };
        for (_, held) in holds {
            let Some(voice) = self.speaking.get_mut(&held) else {
                continue;
            };
            if (voice.bend - cents).abs() < 1e-3 {
                continue;
            }
            voice.bend = cents;
            updates.push((voice.handle, voice.bent_rate()));
        }
        updates
    }

    /// The tuning a division plays under as a *keyboard*: its own,
    /// else the console's — what the transposer reads, and the
    /// division rung of `stop_tuning_resolved`. Couplers make tuning
    /// physical — a coupled copy sounds the destination's pipes, and
    /// pipes are tuned where they stand, so the copy speaks in the
    /// destination's tuning, resolved stop by stop.
    fn effective_tuning(&self, manual_index: usize) -> &Tuning {
        self.manual_tuning
            .get(manual_index)
            .and_then(|tuning| tuning.as_ref())
            .unwrap_or(&self.tuning)
    }

    /// Expand one played key through the engaged couplers' routes into
    /// every (manual, MIDI key) that should sound, each landing tagged
    /// with the policies the route it travelled grants it. Couplers act
    /// on *played* keys only — coupler-produced notes don't re-couple
    /// (matching default organ behaviour, and making self-couplers like
    /// a 16' II trivially finite; GO's opt-in propagation flags can
    /// come later).
    ///
    /// The played key itself lands first — unless an engaged route with
    /// `unison_off` covers it, in which case its own division is silent
    /// and only the coupled copies speak: the note moves rather than
    /// doubles.
    fn couple(&self, manual: ManualId, midi_key: i16) -> Vec<Landing> {
        let mut unison_off = false;
        let mut copies: Vec<Landing> = Vec::new();
        for &engaged in &self.engaged_couplers {
            let Some(coupler) = self.organ.couplers.get(engaged) else {
                continue;
            };
            for route in &coupler.routes {
                if route.from_manual != manual
                    || !route.covers(midi_key)
                    || !self.route_hears(manual, midi_key, route)
                {
                    continue;
                }
                unison_off |= route.unison_off;
                let Some(target) = &route.target else {
                    continue;
                };
                let repitch = target.repitch.unwrap_or(self.couplers_repitch);
                let landing = Landing {
                    manual: target.manual,
                    midi_key: midi_key.saturating_add(target.key_shift),
                    fill: repitch,
                    bounded: !repitch,
                    lane: if target.own_pipes { engaged as u32 + 1 } else { 0 },
                };
                match copies.iter_mut().find(|c| {
                    (c.manual, c.midi_key, c.lane)
                        == (landing.manual, landing.midi_key, landing.lane)
                }) {
                    // Two routes onto the same note speak one pipe; if
                    // either may fill or escape the compass, the
                    // landing may.
                    Some(existing) => {
                        existing.fill |= landing.fill;
                        existing.bounded &= landing.bounded;
                    }
                    None => copies.push(landing),
                }
            }
        }
        let mut landings = Vec::with_capacity(copies.len() + 1);
        if !unison_off {
            landings.push(Landing {
                manual,
                midi_key,
                fill: true,
                bounded: true,
                lane: 0,
            });
        }
        // A copy that lands exactly on the played key adds nothing —
        // unless it speaks pipes of its own, which is precisely a
        // deliberate unison doubler.
        landings.extend(copies.into_iter().filter(|c| {
            unison_off || c.lane != 0 || (c.manual, c.midi_key) != (manual, midi_key)
        }));
        landings
    }

    /// Whether a route fires for this key right now. A classic route
    /// always does (its range is already checked); a Bass/Melody route
    /// hears only the extreme of the keys currently held on its manual.
    /// That extreme moves as keys go down and up, which is why key
    /// changes on such a manual re-judge every held key (see
    /// `note_on_manual` / `note_off_manual`).
    fn route_hears(&self, manual: ManualId, midi_key: i16, route: &CouplerRoute) -> bool {
        route.scope == CouplerScope::AllKeys || self.extreme_held(manual, route) == Some(midi_key)
    }

    /// The lowest/highest currently-held key on `manual` among those
    /// the route's range covers, in the transposed coordinates `couple`
    /// judges keys in. "Held" is `held_velocity`, which gains a key
    /// before its voices are derived and loses it before its release —
    /// so a press sees itself, and a release doesn't.
    fn extreme_held(&self, manual: ManualId, route: &CouplerRoute) -> Option<i16> {
        let index = self.organ.manuals.iter().position(|m| m.id == manual)?;
        let transpose = self.effective_tuning(index).transpose as i16;
        let held = self
            .held_velocity
            .keys()
            .filter(|&&(at, _)| at == index)
            .map(|&(_, key)| key as i16 + transpose)
            .filter(|&key| route.covers(key));
        match route.scope {
            CouplerScope::AllKeys => None,
            CouplerScope::Bass => held.min(),
            CouplerScope::Melody => held.max(),
        }
    }

    /// Whether any engaged coupler follows held-key extremes on this
    /// manual — if one does, a key change can move coupling on *other*
    /// keys than the one that changed.
    fn tracks_extremes(&self, manual_index: usize) -> bool {
        let Some(id) = self.organ.manuals.get(manual_index).map(|m| m.id) else {
            return false;
        };
        self.engaged_couplers
            .iter()
            .filter_map(|&engaged| self.organ.couplers.get(engaged))
            .flat_map(|coupler| &coupler.routes)
            .any(|route| route.scope != CouplerScope::AllKeys && route.from_manual == id)
    }

    /// The inclusive MIDI range a manual answers to, and the widening
    /// of it to the player's keyboard. Notes outside are silent: the
    /// compass is the instrument, and the player's hardware now says
    /// how wide it is.
    pub fn compass(&self, manual_index: usize) -> Option<(i16, i16)> {
        self.compass.get(manual_index).copied()
    }

    /// The compass the sample set itself declares — what the player's
    /// keyboard is measured against, and what `reset_compass` restores.
    pub fn native_compass(&self, manual_index: usize) -> Option<(i16, i16)> {
        self.organ.manuals.get(manual_index).map(|manual| {
            let first = manual.first_midi_note as i16;
            (first, first + manual.key_count as i16 - 1)
        })
    }

    pub fn set_compass(&mut self, manual_index: usize, low: i16, high: i16) {
        let Some(manual) = self.organ.manuals.get(manual_index) else {
            return;
        };
        let (low, high) = (low.min(high).max(0), high.max(low).min(127));
        let native = (
            manual.first_midi_note as i16,
            manual.first_midi_note as i16 + manual.key_count as i16 - 1,
        );
        if (low, high) != native {
            tracing::info!(
                "compass: {} plays {}..{} (the set gives {}..{})",
                manual.name,
                low,
                high,
                native.0,
                native.1
            );
        }
        self.compass[manual_index] = (low, high);
    }

    /// Restore a manual to the compass its sample set declares.
    pub fn reset_compass(&mut self, manual_index: usize) {
        let Some(manual) = self.organ.manuals.get(manual_index) else {
            return;
        };
        let first = manual.first_midi_note as i16;
        self.compass[manual_index] = (first, first + manual.key_count as i16 - 1);
    }

    /// Let couplers reach pipes their division hasn't got by repitching
    /// a neighbour. Off by default (`[couplers] repitch` in the
    /// sidecar); see `voices_for_key` for why.
    pub fn set_coupler_repitch(&mut self, repitch: bool) {
        self.couplers_repitch = repitch;
    }

    pub fn coupler_repitch(&self) -> bool {
        self.couplers_repitch
    }

    /// Which pipe of `range` speaks for a key, the *nominal* pipe index
    /// it stands in for (the rank-ladder position the key demands, equal
    /// to the pipe itself when nothing is borrowed), and the ratio its
    /// playback rate must be scaled by. The nominal index is the voice's
    /// identity for refcounting: two keys borrowing the same physical
    /// pipe at different pitches are two voices, not one.
    ///
    /// The pipe a rank *nominally* holds for a key may be missing: the
    /// set's compass may be narrower than the keyboard the player has
    /// widened this manual to, the range may stop short of it, or the
    /// set may simply have a hole where a sample failed to load. In
    /// every one of those cases the honest answer is not silence — it
    /// is the nearest pipe the rank does have, played at the pitch the
    /// missing one would have sounded. That is what a real organ
    /// builder's borrowing does, and what makes a 56-note set playable
    /// from a 61-note keyboard.
    ///
    /// The ratio is exactly 1 (before tuning) whenever the nominal pipe
    /// exists, so nothing in the ordinary compass is touched by this.
    fn pipe_for(&self, range: &RankRange, key_index: i16, fill: bool) -> Option<(u16, i32, f32)> {
        let first = range.first_pipe as i32;
        let last = first + range.key_count as i32 - 1;
        if last < first {
            return None;
        }
        // Where the key would fall in the rank if the rank ran forever.
        // Outside the range this is the pipe the organ doesn't have.
        let wanted = first + (key_index as i32 - range.first_key as i32);
        if !fill {
            // Nothing stands in for this one: it speaks if the rank has
            // it, and is silent if it hasn't.
            let exact = u16::try_from(wanted).ok()?;
            let present = (first..=last).contains(&wanted)
                && self.specs.contains_key(&(range.rank, exact));
            return present.then_some((exact, wanted, 1.0));
        }
        // The rank may run past this stop's window into it — a unit
        // rank drawn at 16' on the pedal and 8' on a manual overlaps
        // both stops' windows. The stop's *coverage* is the range's
        // business (settled in `range_covers` before we get here); the
        // pipe that stands in is the rank's, so the search spans the
        // whole rank. A real pipe at the wanted ladder position beats
        // any repitched neighbour.
        let rank_last = self
            .organ
            .rank(range.rank)
            .map_or(last, |rank| rank.pipes.len() as i32 - 1);
        let mut source = wanted.clamp(0, rank_last);
        // A hole at the wanted position is a defect, not a decision:
        // step outwards until a pipe that actually loaded turns up.
        if !self.specs.contains_key(&(range.rank, source as u16)) {
            let mut found = None;
            for distance in 1..=rank_last {
                for candidate in [source - distance, source + distance] {
                    if (0..=rank_last).contains(&candidate)
                        && self.specs.contains_key(&(range.rank, candidate as u16))
                    {
                        found = Some(candidate);
                        break;
                    }
                }
                if found.is_some() {
                    break;
                }
            }
            source = found?;
        }
        // Ranks are semitone ladders, so the pitch the missing pipe
        // would have sounded is its distance from the one we found.
        let semitones = (wanted - source) as f32;
        let ratio = if semitones == 0.0 {
            1.0
        } else {
            (semitones / 12.0).exp2()
        };
        Some((source as u16, wanted, ratio))
    }

    fn within_compass(&self, manual_index: usize, midi_key: i16) -> bool {
        self.compass
            .get(manual_index)
            .is_some_and(|&(low, high)| (low..=high).contains(&midi_key))
    }

    /// Whether this rank range answers for a key.
    ///
    /// Inside the set's own compass the range's limits are respected to
    /// the key: a stop that covers half the keyboard (divided registers,
    /// treble-only mixtures) is a musical decision, and filling it in
    /// would be inventing an instrument. Beyond the set's compass —
    /// where the player's keyboard is wider than the organ — there is no
    /// such decision to respect, so a range that reaches the edge
    /// carries on past it.
    fn range_covers(
        &self,
        range: &RankRange,
        key_index: i16,
        manual_index: usize,
        fill: bool,
    ) -> bool {
        let first = range.first_key as i16;
        let last = first + range.key_count as i16 - 1;
        if (first..=last).contains(&key_index) {
            return true;
        }
        if !fill {
            return false;
        }
        let native_last = self.organ.manuals[manual_index].key_count as i16 - 1;
        if key_index < 0 {
            first == 0
        } else if key_index > native_last {
            last == native_last
        } else {
            false
        }
    }

    /// The pipes one key press sounds, with each voice's parameters
    /// settled — couplers expanded, compass enforced, missing pipes
    /// filled by repitching. Both the key press and drawing a stop
    /// under a held key come through here, so they cannot disagree.
    ///
    /// `only` restricts the walk to one stop (drawing it mid-hold).
    ///
    /// Each entry's `key` is the voice's identity for refcounting: the
    /// physical pipe (borrow chains followed) plus the cent-resolution
    /// pitch it is asked to sound — see [`PipeKey`]. `deviation` is
    /// the exact tuning deviation the pitch was priced at — what live
    /// retuning diffs against.
    fn voices_for_key(
        &self,
        manual_index: usize,
        key: u16,
        only: Option<StopId>,
    ) -> Vec<KeyVoice> {
        let Some(origin) = self.organ.manuals.get(manual_index).map(|m| m.id) else {
            return Vec::new();
        };
        // The transposer shifts which pipes sound, like the console
        // gadget — the *played* division's transpose, since it is the
        // keyboard that shifts; temperament + concert pitch retune each
        // pipe where it lands.
        let played = key as i16 + self.effective_tuning(manual_index).transpose as i16;
        let mut voices = Vec::new();
        // Each landing carries its own policies: the played key fills
        // (repitching serves the player's keyboard) inside the compass;
        // a coupled copy fills, and escapes the compass, only if its
        // route asked to. A 16' running off the bottom of a rank, or a
        // coupler into a division with a shorter compass, sounds
        // nothing there unless its route opts into repitching.
        for landing in self.couple(origin, played) {
            let Landing {
                manual: manual_id,
                midi_key,
                fill,
                bounded,
                lane,
            } = landing;
            let Some(target) = self
                .organ
                .manuals
                .iter()
                .position(|m| m.id == manual_id)
                .filter(|&index| !bounded || self.within_compass(index, midi_key))
            else {
                continue;
            };
            // The pitch this key wants under the sounding division's
            // tuning, as a deviation from the recorded 12-EDO ladder.
            // Whole semitones of it re-anchor the key to a nearer pipe
            // (a 19-EDO key mid-compass is closer to another pipe than
            // to its nominal one); only the sub-semitone remainder is
            // bent, so no pipe is ever pulled more than 50 cents from
            // how it was recorded. A key the tuning's keyboard mapping
            // leaves unmapped sounds nothing here. Temperaments deviate
            // well under a semitone, so for them shift is always 0 and
            // this is exactly the old behaviour.
            // Negative below the set's own bottom key: the rank ladder
            // is extrapolated in both directions, so keep the sign.
            let key_index = midi_key - self.organ.manuals[target].first_midi_note as i16;
            for stop in &self.organ.stops {
                if stop.manual != manual_id
                    || !self.drawn.contains(&stop.id)
                    || only.is_some_and(|only| only != stop.id)
                {
                    continue;
                }
                for range in &stop.ranks {
                    // Coverage is judged at the played key — a divided
                    // register is a decision about the keyboard, not
                    // about where the pitches land on the ladder.
                    if !self.range_covers(range, key_index, target, fill) {
                        continue;
                    }
                    // The pitch this key wants under the tuning this
                    // stop — this rank of it — resolves to, as a
                    // deviation from the recorded 12-EDO ladder. A key
                    // the tuning's keyboard mapping leaves unmapped
                    // sounds nothing here.
                    let (tuning, _) = self.voice_tuning(stop.id, range.rank);
                    let Some(deviation) = tuning.deviation_cents(midi_key.max(0) as u16)
                    else {
                        continue;
                    };
                    // The voicer's trim for THIS pipe: level, tone and
                    // pitch, resolved from the stop's rules at the key
                    // that sounds it on the stop's own keyboard (a
                    // coupled press is voiced where it lands, since it
                    // is that pipe that speaks). The pitch trim (a
                    // footage override, a fine-tune) folds into the
                    // same deviation the tuning asked for, so a
                    // repitched stop re-anchors to the pipes that
                    // really sound there — the octave of an 8' drawn
                    // at 4' comes from the pipes an octave up, not
                    // from doubling the tape speed.
                    let trim = self.trim_for(stop.id, range.rank, midi_key as i32);
                    let adjust_cents = trim.cents;
                    // A target tuning bends each pipe from where it
                    // was measured to sound (or from the organ's model
                    // of it, when it keeps its drift), not from the
                    // nominal the ladder assumes. `Original` leaves
                    // every pipe as it is.
                    let corrects_pipes = tuning.corrects_pipes();
                    let priced = deviation + adjust_cents;
                    // As recorded, a key IS its pipe: pulling the
                    // organ's pitch bends that pipe however far, never
                    // a neighbour — only the stop's own footage
                    // re-anchors.
                    let anchored_on = if corrects_pipes { priced } else { adjust_cents };
                    let shift = (anchored_on / 100.0).round() as i16;
                    let key_bend_cents = priced - shift as f64 * 100.0;
                    let Some((pipe, nominal, ratio)) =
                        self.pipe_for(range, key_index + shift, fill)
                    else {
                        continue;
                    };
                    let Some(spec) = self.specs.get(&(range.rank, pipe)) else {
                        continue;
                    };
                    let home = tuning.pipe_offset(spec.home_cents as f64, spec.model_cents as f64);
                    let bend_cents = key_bend_cents - home;
                    let bend_ratio = cents_to_ratio(bend_cents) as f32;
                    // Routing is a property of the STOP (its speakers,
                    // its speaking delay), stamped here so borrowed
                    // pipes travel with the stop that sounds them.
                    let mut spec = *spec;
                    if let Some(&(bus, delay_frames)) = self.stop_routing.get(&stop.id) {
                        spec.bus = bus;
                        spec.delay_frames = delay_frames;
                    }
                    // The user's voicing trim: level and tone directly
                    // (the cents were folded into `priced` above).
                    spec.gain *= trim.gain;
                    spec.voicing_tilt = trim.tilt;
                    // Identity at cent resolution on the PHYSICAL pipe:
                    // two keys anchored to the same pipe but bent apart
                    // are two virtual pipes; the same pipe at the same
                    // pitch — through any stop, borrow or coupler — is
                    // one (an organ pipe speaks once).
                    let (phys_rank, phys_pipe) = self
                        .physical
                        .get(&(range.rank, pipe))
                        .copied()
                        .unwrap_or((range.rank, pipe));
                    let key = PipeKey {
                        rank: phys_rank,
                        pipe: phys_pipe,
                        cents: (nominal - pipe as i32) * 100 + bend_cents.round() as i32,
                        route_lane: lane,
                        stop_lane: if stop.own_pipes { stop.id.0 + 1 } else { 0 },
                    };
                    voices.push(KeyVoice {
                        stop: stop.id,
                        rank: range.rank,
                        pipe,
                        key,
                        deviation: deviation - home,
                        home: spec.home_cents as f64,
                        model: spec.model_cents as f64,
                        ladder_key: midi_key,
                        trim,
                        shift,
                        spec: self.voiced(spec, ratio * bend_ratio),
                    });
                }
            }
        }
        voices
    }

    /// Install the multi-attack selection tables from the loaded bank.
    pub fn set_attack_options(
        &mut self,
        options: HashMap<(RankId, u16), Vec<crate::bank::AttackOption>>,
    ) {
        self.attack_options = options;
    }

    /// Record which recording variant pipes on `group` should prefer —
    /// the wave tremulant's contribution to attack/release selection.
    pub fn set_wave_tremulant(&mut self, group: u8, engaged: bool) {
        let bit = 1u32 << (group as u32).min(31);
        if engaged {
            self.wave_trems |= bit;
        } else {
            self.wave_trems &= !bit;
        }
    }

    fn wave_trem_engaged(&self, group: u8) -> bool {
        self.wave_trems & (1u32 << (group as u32).min(31)) != 0
    }

    /// Price a voice for one press: velocity through the rank's volume
    /// ramp, and — when the pipe has recorded variants — GO's attack
    /// selection (`GetAttack`): among candidates whose wave-trem state
    /// matches, whose `min_velocity` is within the press, and whose
    /// re-attack window covers the time since the pipe last released,
    /// the most specific wins (highest velocity bound, then tightest
    /// window), ties broken at random so repetition never machine-guns
    /// one file.
    fn price(&mut self, voice: &mut KeyVoice, velocity: u8) {
        voice.spec.gain *= voice.spec.velocity.gain(velocity);
        let Some(options) = self.attack_options.get(&(voice.rank, voice.pipe)) else {
            return;
        };
        let trem_on = self.wave_trem_engaged(voice.spec.group);
        let since_ms = self
            .last_released
            .get(&voice.key)
            .map(|at| at.elapsed().as_millis().min(u128::from(u32::MAX)) as u32);
        let eligible = |option: &crate::bank::AttackOption| {
            option.wave_tremulant.is_none_or(|wants| wants == trem_on)
                && option.min_velocity <= velocity
                && option
                    .max_since_release_ms
                    .is_none_or(|max| since_ms.is_some_and(|since| since <= max))
        };
        // Most specific first: highest velocity bound, then the
        // tightest re-attack window (a bounded window beats none).
        let best = options
            .iter()
            .filter(|option| eligible(option))
            .map(|option| {
                (
                    option.min_velocity,
                    match option.max_since_release_ms {
                        Some(max) => u64::from(u32::MAX) - u64::from(max),
                        None => 0,
                    },
                )
            })
            .max();
        let Some(best) = best else { return };
        let candidates: Vec<&crate::bank::AttackOption> = options
            .iter()
            .filter(|option| {
                eligible(option)
                    && option.min_velocity == best.0
                    && match option.max_since_release_ms {
                        Some(max) => u64::from(u32::MAX) - u64::from(max),
                        None => 0,
                    } == best.1
            })
            .collect();
        let pick = if candidates.len() > 1 {
            (xorshift(&mut self.rng) as usize) % candidates.len()
        } else {
            0
        };
        let Some(chosen) = candidates.get(pick) else { return };
        if chosen.sample != voice.spec.sample {
            voice.spec.rate *= chosen.rate_factor;
            voice.spec.sample = chosen.sample;
        }
    }

    /// A pipe's spec as it must sound at one pitch: `ratio` is the
    /// whole distance from how the pipe was recorded — re-anchoring,
    /// tuning bend and standing in for a missing pipe all folded into
    /// one number by the caller.
    ///
    /// Everything downstream of pitch has to move with it. Wind draw and
    /// the pressure→brightness hinge are properties of the *sounding*
    /// pitch, not of the recording, so a pipe pressed into service five
    /// semitones up draws wind like the pipe it is imitating.
    fn voiced(&self, mut spec: VoiceSpec, ratio: f32) -> VoiceSpec {
        spec.rate *= ratio;
        if ratio != 1.0 {
            let sounding_hz = (spec.nominal_hz * ratio) as f64;
            spec.wind_weight = crate::bank::wind_weight(sounding_hz, spec.percussive);
            spec.brightness =
                crate::bank::brightness_coefficient(sounding_hz, self.device_rate, spec.percussive);
        }
        spec
    }

    /// Voices retired by this press: a re-press before the note-off
    /// (key bounce, fast repetition) must release the previous voices —
    /// a pipe can't speak twice, and doubling correlated audio jumps
    /// +6 dB into clipping.
    /// `note_on` addressed by manual index — the coordinate UIs speak
    /// (a clicked on-screen key has no MIDI channel). `velocity` prices
    /// each voice through its rank's velocity→volume ramp; ranks
    /// without one (the organ norm) ignore it, and sources without a
    /// velocity (UI clicks) pass 127, GO's on-screen behaviour.
    pub fn note_on_manual(
        &mut self,
        manual_index: usize,
        key: u16,
        velocity: u8,
    ) -> (Vec<VoiceStart>, Vec<u64>) {
        if manual_index >= self.organ.manuals.len() {
            return (Vec::new(), Vec::new());
        }
        let mut retriggered = Vec::new();
        for (_, held) in self
            .sounding
            .remove(&(manual_index, key))
            .unwrap_or_default()
        {
            if let Some(handle) = self.release_pipe(held) {
                retriggered.push(handle);
            }
        }
        self.held_velocity.insert((manual_index, key), velocity);
        let mut starts = Vec::new();
        let mut held = Vec::new();
        for mut voice in self.voices_for_key(manual_index, key, None) {
            self.price(&mut voice, velocity);
            if voice.spec.percussive {
                // One-shots (noises) aren't refcounted.
                let handle = self.next_handle;
                self.next_handle += 1;
                starts.push(VoiceStart {
                    handle,
                    spec: voice.spec,
                });
                continue;
            }
            held.push((voice.stop, voice.key));
            self.hold_pipe(&voice, &mut starts, &mut retriggered);
        }
        // Track the key even when no stops are drawn: the UI lights it,
        // note-off clears it, and drawing a stop mid-hold must find it
        // to start pipes under it.
        self.sounding.insert((manual_index, key), held);
        // A Bass/Melody coupler may have just changed which key is the
        // extreme, moving coupled pipes off a key that stays held.
        if self.tracks_extremes(manual_index) {
            self.recouple_held_keys(&mut retriggered, &mut starts);
        }
        // Expedites can duplicate handles already queued by the
        // retrigger drain; the engine tolerates it but keep it clean.
        retriggered.sort_unstable();
        retriggered.dedup();
        (starts, retriggered)
    }

    /// `note_off` addressed by manual index (see `note_on_manual`).
    /// Returns (handles to stop, voices to start) — a note-off can
    /// *start* sound when a Bass/Melody coupler hands its coupled note
    /// to the next-extreme key still held.
    pub fn note_off_manual(&mut self, manual_index: usize, key: u16) -> (Vec<u64>, Vec<VoiceStart>) {
        self.held_velocity.remove(&(manual_index, key));
        let mut released = Vec::new();
        for (_, held) in self
            .sounding
            .remove(&(manual_index, key))
            .unwrap_or_default()
        {
            if let Some(handle) = self.release_pipe(held) {
                released.push(handle);
            }
        }
        let mut starts = Vec::new();
        if self.tracks_extremes(manual_index) {
            self.recouple_held_keys(&mut released, &mut starts);
            released.sort_unstable();
            released.dedup();
        }
        (released, starts)
    }

    /// One more holder demands a pipe. A pipe speaks ONCE no matter how
    /// many routes reach it, so this either bumps the refcount or
    /// starts a new voice — expediting the pipe's still-releasing
    /// (pallet-staggered) predecessor so it never overlaps itself.
    fn hold_pipe(
        &mut self,
        voice: &KeyVoice,
        starts: &mut Vec<VoiceStart>,
        expedited: &mut Vec<u64>,
    ) {
        let at = voice.key;
        match self.speaking.get_mut(&at) {
            Some(speaking) => speaking.holders += 1,
            None => {
                let handle = self.next_handle;
                self.next_handle += 1;
                self.speaking.insert(
                    at,
                    Speaking {
                        handle,
                        holders: 1,
                        rate: voice.spec.rate,
                        deviation: voice.deviation,
                        bend: 0.0,
                        home: voice.home,
                        model: voice.model,
                        ladder_key: voice.ladder_key,
                        stop: voice.stop,
                        rank: voice.rank,
                        trim: voice.trim,
                        start_gain: voice.trim.gain,
                        shift: voice.shift,
                    },
                );
                if let Some(previous) = self.last_pipe_voice.insert(at, handle) {
                    expedited.push(previous);
                }
                starts.push(VoiceStart {
                    handle,
                    spec: voice.spec,
                });
            }
        }
    }

    /// One holder lets go of a pipe; the voice stops only when the
    /// last holder does.
    fn release_pipe(&mut self, at: PipeKey) -> Option<u64> {
        let voice = self.speaking.get_mut(&at)?;
        voice.holders -= 1;
        if voice.holders == 0 {
            let handle = voice.handle;
            self.speaking.remove(&at);
            // The re-attack clock: fast-repetition attack variants are
            // selected against how long ago the pipe stopped speaking.
            self.last_released.insert(at, std::time::Instant::now());
            Some(handle)
        } else {
            None
        }
    }

    /// Draw or retire a stop. Returns the handles of voices to stop
    /// (retired pipes, or the noise voice whose note-off plays the
    /// push-in thump) and the voices to start: the pull-thump noise,
    /// plus — the pallets under held keys being open — the stop's own
    /// pipes on those keys, as on a tracker.
    pub fn set_drawn(&mut self, stop: StopId, drawn: bool) -> (Vec<u64>, Vec<VoiceStart>) {
        // Noise stops aren't directly drawable, and a no-op change
        // shouldn't thump.
        if self.noise_stops.contains(&stop) || self.drawn.contains(&stop) == drawn {
            return (Vec::new(), Vec::new());
        }
        if drawn {
            self.drawn.push(stop);
            let mut starts = Vec::new();
            if let Some(noise) = self.open_noise(self.stop_noise.get(&stop).copied()) {
                self.stop_noise_open.insert(stop, noise.handle);
                starts.push(noise);
            }
            let mut expedited = self.start_stop_under_held_keys(stop, &mut starts);
            expedited.sort_unstable();
            expedited.dedup();
            return (expedited, starts);
        }
        self.drawn.retain(|&id| id != stop);
        let mut to_release: Vec<PipeKey> = Vec::new();
        for entries in self.sounding.values_mut() {
            entries.retain(|&(owner, held)| {
                if owner == stop {
                    to_release.push(held);
                    false
                } else {
                    true
                }
            });
        }
        let mut released = Vec::new();
        for held in to_release {
            if let Some(handle) = self.release_pipe(held) {
                released.push(handle);
            }
        }
        // Note-off on the open noise voice = the push-in thump.
        released.extend(self.stop_noise_open.remove(&stop));
        (released, Vec::new())
    }

    /// Start `stop`'s pipes under every currently held key (coupling
    /// expanded with the *current* coupler state, like a fresh press).
    /// Appends the new voices to `starts` and returns handles of
    /// previous pipe voices to expedite (see `note_on_manual`).
    fn start_stop_under_held_keys(&mut self, stop: StopId, starts: &mut Vec<VoiceStart>) -> Vec<u64> {
        let mut expedited = Vec::new();
        let held_keys: Vec<(usize, u16)> = self.sounding.keys().copied().collect();
        for (manual_index, key) in held_keys {
            let velocity = self
                .held_velocity
                .get(&(manual_index, key))
                .copied()
                .unwrap_or(127);
            let mut new_entries = Vec::new();
            for mut voice in self.voices_for_key(manual_index, key, Some(stop)) {
                self.price(&mut voice, velocity);
                // One-shots strike on key press, not on drawing the
                // stop mid-hold.
                if voice.spec.percussive {
                    continue;
                }
                new_entries.push((voice.stop, voice.key));
                self.hold_pipe(&voice, starts, &mut expedited);
            }
            if !new_entries.is_empty() {
                self.sounding
                    .entry((manual_index, key))
                    .or_default()
                    .extend(new_entries);
            }
        }
        expedited
    }

    pub fn is_drawn(&self, stop: StopId) -> bool {
        self.drawn.contains(&stop)
    }

    pub fn coupler_engaged(&self, index: usize) -> bool {
        self.engaged_couplers.contains(&index)
    }

    pub fn coupler_available(&self, index: usize) -> bool {
        self.available_couplers.get(index).copied().unwrap_or(true)
    }

    /// Keep a coupler on the console or take it off it. Taking it off
    /// releases it first (with its clack, like any release); the routes
    /// stay in the organ, so putting it back needs no reload.
    pub fn set_coupler_available(
        &mut self,
        index: usize,
        available: bool,
    ) -> (Vec<u64>, Vec<VoiceStart>) {
        if index >= self.organ.couplers.len() {
            return (Vec::new(), Vec::new());
        }
        if self.available_couplers.len() < self.organ.couplers.len() {
            self.available_couplers.resize(self.organ.couplers.len(), true);
        }
        let released = if !available && self.coupler_engaged(index) {
            self.set_coupler(index, false)
        } else {
            (Vec::new(), Vec::new())
        };
        self.available_couplers[index] = available;
        released
    }

    /// Move a stop to another manual, re-anchoring its key windows by
    /// pitch — the key that meant tenor C keeps meaning tenor C — and
    /// trimming to the destination's key count. A drawn stop is
    /// retired and redrawn across the move, so held keys release its
    /// old pipes and (where the destination holds them too) sound its
    /// new ones.
    pub fn move_stop(
        &mut self,
        stop: StopId,
        manual_index: usize,
    ) -> (Vec<u64>, Vec<VoiceStart>) {
        let Some(target) = self.organ.manuals.get(manual_index) else {
            return (Vec::new(), Vec::new());
        };
        let (target_id, target_first, target_count) = (
            target.id,
            target.first_midi_note as i32,
            target.key_count as i32,
        );
        let Some(entry) = self.organ.stops.iter().position(|s| s.id == stop) else {
            return (Vec::new(), Vec::new());
        };
        if self.organ.stops[entry].manual == target_id || self.noise_stops.contains(&stop) {
            return (Vec::new(), Vec::new());
        }
        let was_drawn = self.is_drawn(stop);
        let (mut stopped, _) = if was_drawn {
            self.set_drawn(stop, false)
        } else {
            (Vec::new(), Vec::new())
        };
        let source_first = self
            .organ
            .manuals
            .iter()
            .find(|m| m.id == self.organ.stops[entry].manual)
            .map(|m| m.first_midi_note as i32)
            .unwrap_or(target_first);
        let shift = source_first - target_first;
        let moved = &mut self.organ.stops[entry];
        moved.manual = target_id;
        moved.ranks = moved
            .ranks
            .iter()
            .filter_map(|range| {
                let mut first_key = range.first_key as i32 + shift;
                let mut key_count = range.key_count as i32;
                let mut first_pipe = range.first_pipe as i32;
                if first_key < 0 {
                    first_pipe -= first_key;
                    key_count += first_key;
                    first_key = 0;
                }
                key_count = key_count.min(target_count - first_key);
                (key_count > 0).then_some(aristide_model::RankRange {
                    rank: range.rank,
                    first_key: first_key as u16,
                    key_count: key_count as u16,
                    first_pipe: first_pipe as u16,
                })
            })
            .collect();
        if self.organ.stops[entry].ranks.is_empty() {
            tracing::warn!(
                "moved stop {:?} lies entirely outside its new manual's compass",
                self.organ.stops[entry].name
            );
        }
        tracing::info!(
            "stop {:?} moved to {:?}",
            self.organ.stops[entry].name,
            self.organ.manuals[manual_index].name
        );
        // Expression routing follows the stop to its new division.
        self.map_enclosures();
        let starts = if was_drawn {
            let (also_stopped, starts) = self.set_drawn(stop, true);
            stopped.extend(also_stopped);
            starts
        } else {
            Vec::new()
        };
        (stopped, starts)
    }

    /// Engage or release a coupler by its index in `organ.couplers` —
    /// and, with it, every coupler linked to it: a link group is one
    /// action wearing several rockers, so its members move together.
    /// Takes effect under held notes immediately, as an electric-action
    /// console does (and as drawing a stop mid-hold already did):
    /// engaging starts the coupled pipes under the held keys, releasing
    /// lets go of them, and a unison-off coupler moves the held notes.
    /// Returns (voice handles to stop, voices to start) like
    /// `set_drawn` — the clack noise rides along in them.
    pub fn set_coupler(&mut self, index: usize, engaged: bool) -> (Vec<u64>, Vec<VoiceStart>) {
        let mut stops = Vec::new();
        let mut starts = Vec::new();
        let mut changed = false;
        for member in self.link_group(index) {
            changed |= self.flip_coupler(member, engaged, &mut stops, &mut starts);
        }
        if changed {
            self.recouple_held_keys(&mut stops, &mut starts);
        }
        stops.sort_unstable();
        stops.dedup();
        (stops, starts)
    }

    /// One coupler's own engagement flip — state and clack only, no
    /// recoupling: `set_coupler` recouples once for the whole link
    /// group. Returns whether anything actually changed.
    fn flip_coupler(
        &mut self,
        index: usize,
        engaged: bool,
        stops: &mut Vec<u64>,
        starts: &mut Vec<VoiceStart>,
    ) -> bool {
        if index >= self.organ.couplers.len()
            || self.engaged_couplers.contains(&index) == engaged
            || (engaged && !self.coupler_available(index))
        {
            return false;
        }
        if engaged {
            self.engaged_couplers.push(index);
            if let Some(noise) = self.open_noise(self.coupler_noise.get(&index).copied()) {
                self.coupler_noise_open.insert(index, noise.handle);
                starts.push(noise);
            }
        } else {
            self.engaged_couplers.retain(|&i| i != index);
            // Note-off on the open noise voice = the release clack.
            stops.extend(self.coupler_noise_open.remove(&index));
        }
        true
    }

    /// The link group `index` belongs to — itself first, then its
    /// linked partners; just itself when unlinked.
    fn link_group(&self, index: usize) -> Vec<usize> {
        let mut members = vec![index];
        if let Some(group) = self.coupler_links.iter().find(|g| g.contains(&index)) {
            members.extend(group.iter().copied().filter(|&i| i != index));
        }
        members
    }

    /// Install the link groups (indices into `organ.couplers`) — from
    /// the organ file at load, and live when the console links two.
    pub fn set_coupler_links(&mut self, groups: Vec<Vec<usize>>) {
        self.coupler_links = groups;
    }

    /// The couplers linked with `index`, itself excluded — what the
    /// snapshot reports so the editor can show and undo the link.
    pub fn coupler_linked_with(&self, index: usize) -> Vec<usize> {
        let mut linked = self.link_group(index);
        linked.retain(|&i| i != index);
        linked.sort_unstable();
        linked
    }

    /// Link or unlink two couplers, live. Linking merges any groups
    /// either belongs to; unlinking takes `b` out of `a`'s group (a
    /// group left with one member dissolves). The engaged states are
    /// then reconciled — linked couplers may not disagree, so if
    /// either is on, both end on. Returns (stops, starts) like
    /// `set_coupler`.
    pub fn link_couplers(&mut self, a: usize, b: usize, on: bool) -> (Vec<u64>, Vec<VoiceStart>) {
        if a == b || a >= self.organ.couplers.len() || b >= self.organ.couplers.len() {
            return (Vec::new(), Vec::new());
        }
        if on {
            let mut merged: Vec<usize> = Vec::new();
            self.coupler_links.retain(|group| {
                if group.contains(&a) || group.contains(&b) {
                    merged.extend(group.iter().copied());
                    false
                } else {
                    true
                }
            });
            merged.extend([a, b]);
            merged.sort_unstable();
            merged.dedup();
            self.coupler_links.push(merged);
            if self.coupler_engaged(a) != self.coupler_engaged(b) {
                return self.set_coupler(a, true);
            }
        } else if let Some(group) = self.coupler_links.iter_mut().find(|g| g.contains(&b)) {
            group.retain(|&i| i != b);
            self.coupler_links.retain(|g| g.len() > 1);
        }
        (Vec::new(), Vec::new())
    }

    /// The keys engaged couplers are pulling down right now, per manual
    /// index — the mechanical-action view for on-screen keyboards,
    /// never consulted by the sound path. `show` filters by coupler
    /// index (the per-organ default and per-coupler overrides live in
    /// the server's State, not here). Keys are in each board's own
    /// coordinates: the played key's ladder landing minus the sounding
    /// division's transpose — the key a tracker rod would move.
    pub fn coupled_display_keys(&self, show: &dyn Fn(usize) -> bool) -> Vec<Vec<u16>> {
        let mut out = vec![Vec::new(); self.organ.manuals.len()];
        for &(manual_index, key) in self.sounding.keys() {
            let Some(origin) = self.organ.manuals.get(manual_index).map(|m| m.id) else {
                continue;
            };
            let played = key as i16 + self.effective_tuning(manual_index).transpose as i16;
            for &engaged in &self.engaged_couplers {
                if !show(engaged) {
                    continue;
                }
                let Some(coupler) = self.organ.couplers.get(engaged) else {
                    continue;
                };
                for route in &coupler.routes {
                    if route.from_manual != origin
                        || !route.covers(played)
                        || !self.route_hears(origin, played, route)
                    {
                        continue;
                    }
                    let Some(target) = &route.target else { continue };
                    let Some(index) = self
                        .organ
                        .manuals
                        .iter()
                        .position(|m| m.id == target.manual)
                    else {
                        continue;
                    };
                    let shown = played.saturating_add(target.key_shift)
                        - self.effective_tuning(index).transpose as i16;
                    let (low, high) = self.compass[index];
                    if shown < low || shown > high {
                        continue;
                    }
                    out[index].push(shown as u16);
                }
            }
        }
        for keys in &mut out {
            keys.sort_unstable();
            keys.dedup();
        }
        out
    }

    /// Re-derive what every held key should sound under the current
    /// coupler state and diff it against what it does sound: pipes no
    /// longer demanded are released, newly demanded ones started. This
    /// is what makes a coupler change land on held notes instead of
    /// waiting for the next press.
    fn recouple_held_keys(&mut self, stops: &mut Vec<u64>, starts: &mut Vec<VoiceStart>) {
        let held: Vec<(usize, u16)> = self.sounding.keys().copied().collect();
        for (manual_index, key) in held {
            // Voices started here price at the press they join, like
            // drawing a stop mid-hold; one-shots strike on key press
            // only, not on a coupler change.
            let velocity = self
                .held_velocity
                .get(&(manual_index, key))
                .copied()
                .unwrap_or(127);
            let mut desired: Vec<KeyVoice> = self
                .voices_for_key(manual_index, key, None)
                .into_iter()
                .filter(|voice| !voice.spec.percussive)
                .collect();
            for voice in &mut desired {
                self.price(voice, velocity);
            }
            let mut remaining = self
                .sounding
                .remove(&(manual_index, key))
                .unwrap_or_default();
            let mut entries = Vec::with_capacity(desired.len());
            let mut to_start = Vec::new();
            for voice in desired {
                // Each sounding entry holds one refcount, so match
                // multiset-style: a demand already held is kept, not
                // restarted.
                let entry = (voice.stop, voice.key);
                match remaining.iter().position(|&e| e == entry) {
                    Some(at) => {
                        remaining.swap_remove(at);
                        entries.push(entry);
                    }
                    None => to_start.push(voice),
                }
            }
            for &(_, held) in &remaining {
                stops.extend(self.release_pipe(held));
            }
            for voice in to_start {
                entries.push((voice.stop, voice.key));
                self.hold_pipe(&voice, starts, stops);
            }
            self.sounding.insert((manual_index, key), entries);
        }
    }

    /// General cancel: retire every drawn stop and release every
    /// engaged coupler, as the cancel piston does on a real console.
    /// Returns the voice handles to stop — retired pipes, plus the open
    /// noise voices whose note-off is the push-in thump / coupler clack.
    /// Keys held through a cancel keep sounding only what survives it,
    /// which is nothing: the organ goes silent.
    pub fn cancel(&mut self) -> Vec<u64> {
        let mut stopped = Vec::new();
        for stop in self.drawn.clone() {
            let (released, _) = self.set_drawn(stop, false);
            stopped.extend(released);
        }
        for index in self.engaged_couplers.clone() {
            // Stops are all retired by now, so releasing couplers can
            // only stop voices (the clack noise), never start any —
            // even a unison-off coupler has nothing to give back.
            let (released, _) = self.set_coupler(index, false);
            stopped.extend(released);
        }
        stopped
    }

    /// Every coupler with its engaged and on-console states, for UIs.
    pub fn coupler_states(&self) -> Vec<(usize, &str, bool, bool)> {
        self.organ
            .couplers
            .iter()
            .enumerate()
            .map(|(index, coupler)| {
                (
                    index,
                    coupler.name.as_str(),
                    self.engaged_couplers.contains(&index),
                    self.coupler_available(index),
                )
            })
            .collect()
    }

    /// Every *playable* stop with its manual (name and index) and drawn
    /// state, for UIs — control noises are hidden (they belong to
    /// their controls).
    pub fn stop_states(&self) -> Vec<(StopId, &str, &str, usize, bool)> {
        self.organ
            .stops
            .iter()
            .filter(|stop| !self.noise_stops.contains(&stop.id))
            .map(|stop| {
                let manual = self
                    .organ
                    .manuals
                    .iter()
                    .position(|m| m.id == stop.manual);
                (
                    stop.id,
                    stop.name.as_str(),
                    manual
                        .map(|index| self.organ.manuals[index].name.as_str())
                        .unwrap_or("?"),
                    manual.unwrap_or(usize::MAX),
                    self.drawn.contains(&stop.id),
                )
            })
            .collect()
    }

    /// Per-manual UI state: index, name, first MIDI note, key count,
    /// and the keys currently held on it (sorted). Held keys are the
    /// player's — the origin manual of each press, not coupled echoes.
    pub fn manual_states(&self) -> Vec<(usize, &str, u8, u16, Vec<u16>)> {
        self.organ
            .manuals
            .iter()
            .enumerate()
            .map(|(index, manual)| {
                let mut held: Vec<u16> = self
                    .sounding
                    .keys()
                    .filter(|&&(m, _)| m == index)
                    .map(|&(_, key)| key)
                    .collect();
                held.sort_unstable();
                // The compass, not the set's own key count: a manual
                // widened to the player's keyboard has more keys to
                // draw, and they play.
                let (low, high) = self.compass[index];
                (
                    index,
                    manual.name.as_str(),
                    low.clamp(0, 127) as u8,
                    (high - low + 1).max(0) as u16,
                    held,
                )
            })
            .collect()
    }

    pub fn organ_name(&self) -> &str {
        &self.organ.name
    }

    /// Whether a manual is the pedalboard, straight from the model —
    /// UIs render it at the bottom and stop guessing from the name.
    pub fn manual_pedal(&self, manual: usize) -> bool {
        self.organ.manuals.get(manual).is_some_and(|m| m.pedal())
    }

    /// A manual's declared kind — what geometry the console draws.
    pub fn manual_kind(&self, manual: usize) -> aristide_model::ManualKind {
        self.organ.manuals.get(manual).map(|m| m.kind).unwrap_or_default()
    }

    /// Redeclare a microtonal manual's hex layout on the live organ.
    /// A console fact only — no rank, compass or engine state moves —
    /// so unlike a kind change this needs no rebuild: the next
    /// snapshot simply draws the new grid. `None` returns the manual
    /// to the derived default.
    pub fn set_manual_hex(&mut self, manual: usize, layout: Option<aristide_model::HexLayout>) {
        if let Some(declared) = self.organ.manuals.get_mut(manual) {
            declared.hex = layout;
        }
    }

    /// Test-only: redeclare a manual's kind on the live organ, so
    /// state tests reach the microtonal paths without a file reload.
    #[cfg(test)]
    pub fn force_manual_kind(&mut self, manual: usize, kind: aristide_model::ManualKind) {
        if let Some(declared) = self.organ.manuals.get_mut(manual) {
            declared.kind = kind;
        }
    }

    /// The hex-field layout a microtonal manual draws with: the
    /// declared one, or the default derived from the (possibly
    /// widened) compass. `None` for the other kinds — they have no
    /// hex field.
    pub fn manual_hex(&self, manual: usize) -> Option<aristide_model::HexLayout> {
        let declared = self.organ.manuals.get(manual)?;
        if declared.kind != aristide_model::ManualKind::Microtonal {
            return None;
        }
        Some(declared.hex.unwrap_or_else(|| {
            let (low, high) = self.compass[manual];
            aristide_model::HexLayout::default_for(
                low.clamp(0, 127) as u16,
                (high - low + 1).max(1) as u16,
            )
        }))
    }

    /// Which enclosures a stop's own pipes sit in (indices into the
    /// snapshot's enclosure list): its ranges' ranks → their chests →
    /// the boxes those chests are inside. Borrowed pipes stand with
    /// their own rank and answer to its boxes at sounding time.
    pub fn stop_enclosures(&self, stop: StopId) -> Vec<u32> {
        let Some(stop) = self.organ.stops.iter().find(|s| s.id == stop) else {
            return Vec::new();
        };
        let mut boxes: Vec<u32> = stop
            .ranks
            .iter()
            .filter_map(|range| self.organ.rank(range.rank))
            .filter_map(|rank| {
                self.organ
                    .windchests
                    .iter()
                    .find(|chest| chest.number == rank.windchest)
            })
            .flat_map(|chest| chest.enclosures.iter().copied())
            .collect();
        boxes.sort_unstable();
        boxes.dedup();
        boxes
    }

    /// Rename the loaded organ. Only the name changes — every stop,
    /// manual and coupler keeps its identity; persisting the new name
    /// (and re-keying the assignments stored under it) is the caller's
    /// business.
    pub fn set_organ_name(&mut self, name: String) {
        self.organ.name = name;
    }

    /// Forget everything sounding (the engine is told separately).
    pub fn all_off(&mut self) {
        self.sounding.clear();
        self.held_velocity.clear();
        self.speaking.clear();
    }

}

/// Every borrowed pipe resolved to the physical pipe its chain ends
/// at. Dangling references resolve to the last pipe the chain reached
/// (the spec table won't have it either, so nothing sounds); the hop
/// cap mirrors [`Organ::sounding_pipe`]'s cycle guard.
fn physical_alias(organ: &Organ) -> HashMap<(RankId, u16), (RankId, u16)> {
    let total_pipes: usize = organ.ranks.iter().map(|r| r.pipes.len()).sum();
    let mut map = HashMap::new();
    for rank in &organ.ranks {
        for (index, pipe) in rank.pipes.iter().enumerate() {
            if !matches!(pipe.source, PipeSource::Borrowed(_)) {
                continue;
            }
            let mut at = aristide_model::PipeRef {
                rank: rank.id,
                pipe: index as u16,
            };
            let mut hops = total_pipes + 1;
            while let Some(PipeSource::Borrowed(next)) =
                organ.pipe(at).map(|p| &p.source)
            {
                if hops == 0 {
                    break;
                }
                hops -= 1;
                at = *next;
            }
            map.insert((rank.id, index as u16), (at.rank, at.pipe));
        }
    }
    map
}

/// "Montre 8' stop noise" → "Montre 8'"; "I/P coupler stop noise" → "I/P".
fn strip_noise_suffix(name: &str) -> String {
    let lower = name.to_lowercase();
    let mut stripped = lower.as_str();
    for suffix in [" coupler stop noise", " stop noise", " noise"] {
        if let Some(prefix) = stripped.strip_suffix(suffix) {
            stripped = prefix;
            break;
        }
    }
    stripped.to_string()
}

/// Fuzzy score between a stripped noise name and a control name:
/// normalized-prefix containment, else token overlap (tokens match on
/// equality or ≥2-char prefix). Handles "Fl Harm 8" vs "Flute Harm. 8'"
/// and "Ped Flute 4" vs "Flute 4'".
fn name_match_score(noise: &str, candidate: &str) -> f32 {
    let normalize = |s: &str| -> String {
        s.chars()
            .filter(|c| c.is_alphanumeric())
            .collect::<String>()
            .to_lowercase()
    };
    let (a, b) = (normalize(noise), normalize(candidate));
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    if b.starts_with(&a) || a.starts_with(&b) {
        return 1.0;
    }
    let tokens = |s: &str| -> Vec<String> {
        s.to_lowercase()
            .split(|c: char| !c.is_alphanumeric())
            .filter(|t| !t.is_empty())
            .map(str::to_string)
            .collect()
    };
    let (ta, tb) = (tokens(noise), tokens(candidate));
    let mut matched = 0usize;
    for token_a in &ta {
        if tb.iter().any(|token_b| {
            token_a == token_b
                || (token_a.len() >= 2 && token_b.starts_with(token_a.as_str()))
                || (token_b.len() >= 2 && token_a.starts_with(token_b.as_str()))
        }) {
            matched += 1;
        }
    }
    matched as f32 / ta.len().max(tb.len()) as f32
}

#[cfg(test)]
mod tests {
    use super::*;
    use aristide_model::{Manual, ManualId, Pipe, PipeSource, Rank, RankRange, Stop};

    fn trim_rule(
        keys: Option<(i32, i32)>,
        rank: Option<RankId>,
        gain: Option<f32>,
        cents: Option<f64>,
        tilt: Option<f32>,
    ) -> TrimRule {
        TrimRule {
            keys,
            rank,
            gain,
            cents,
            tilt,
            owned: true,
        }
    }

    fn test_console() -> Console {
        let organ = Organ {
            name: "T".into(),
            base_path: Default::default(),
            manuals: vec![Manual {
                id: ManualId(1),
                name: "Great".into(),
                first_midi_note: 36,
                key_count: 61,
                    kind: Default::default(),
                    hex: None,
            }],
            stops: vec![
                Stop {
                    id: StopId(1),
                    name: "Principal 8".into(),
                    manual: ManualId(1),
                    ranks: vec![RankRange {
                        rank: RankId(1),
                        first_key: 0,
                        key_count: 61,
                        first_pipe: 0,
                    }],
                    own_pipes: false,
                },
                Stop {
                    id: StopId(2),
                    name: "Octave 4".into(),
                    manual: ManualId(1),
                    ranks: vec![RankRange {
                        rank: RankId(2),
                        first_key: 0,
                        key_count: 61,
                        first_pipe: 0,
                    }],
                    own_pipes: false,
                },
            ],
            ranks: (1..=2)
                .map(|id| Rank {
                    id: RankId(id),
                    name: format!("rank {id}"),
                    windchest: 1,
                    velocity_volume: Default::default(),
                    pipes: (0..61)
                        .map(|_| Pipe {
                            nominal_frequency_hz: 440.0,
                            pitch_tuning_cents: 0.0,
                            pitch_correction_cents: 0.0,
                            gain_db: 0.0,
                            midi_key_number: None,
                            midi_pitch_fraction_cents: None,
                            accepts_retuning: true,
                            source: PipeSource::Silent,
                        })
                        .collect(),
                })
                .collect(),
            couplers: vec![],
            enclosures: vec![],
            windchests: vec![],
            tremulants: vec![],
        };
        let mut specs = HashMap::new();
        for rank in 1..=2u32 {
            for pipe in 0..61u16 {
                specs.insert(
                    (RankId(rank), pipe),
                    VoiceSpec {
                        sample: rank - 1,
                        rate: 1.0,
                        gain: 1.0,
                        velocity: Default::default(),
                        percussive: false,
                        group: 0,
                        wind_weight: 1.0,
                        brightness: 0.02,
                        voicing_tilt: 1.0,
                        nominal_hz: 440.0,
                        home_cents: 0.0,
                        model_cents: 0.0,
                        enclosures: [aristide_engine::enclosure::ENCLOSURE_NONE;
                            aristide_engine::enclosure::MAX_VOICE_ENCLOSURES],
                        bus: 0,
                        delay_frames: 0,
                    },
                );
            }
        }
        Console::new(organ, specs, vec![StopId(1), StopId(2)], 48_000.0)
    }

    /// The hex layout is a microtonal-manual fact: other kinds have
    /// none, an undeclared microtonal manual gets the compass-derived
    /// default, and a declared layout passes through untouched.
    #[test]
    fn manual_hex_follows_kind_and_declaration() {
        let console = test_console();
        assert_eq!(console.manual_hex(0), None, "hand keyboards have no hex field");

        let mut console = test_console();
        console.organ.manuals[0].kind = aristide_model::ManualKind::Microtonal;
        let derived = console.manual_hex(0).expect("derived default");
        assert_eq!(derived, aristide_model::HexLayout::default_for(36, 61));

        let declared = aristide_model::HexLayout {
            rows: 7,
            cols: 12,
            right: 2,
            upright: 7,
            anchor: 48,
        };
        console.organ.manuals[0].hex = Some(declared);
        assert_eq!(console.manual_hex(0), Some(declared));
    }

    fn attack(
        sample: u32,
        min_velocity: u8,
        max_since_release_ms: Option<u32>,
        wave_tremulant: Option<bool>,
    ) -> crate::bank::AttackOption {
        crate::bank::AttackOption {
            sample,
            rate_factor: 1.0,
            wave_tremulant,
            min_velocity,
            max_since_release_ms,
        }
    }

    /// GO's `GetAttack` semantics: the most specific eligible attack
    /// wins — highest velocity bound first, then the tightest
    /// re-attack window — and the `IsTremulant` tri-state gates
    /// candidacy against the chest's wave-trem state.
    #[test]
    fn attack_selection_by_velocity_reattack_and_trem_state() {
        let mut console = test_console();
        console.set_drawn(StopId(2), false);
        let variants = vec![
            attack(0, 0, None, Some(false)),  // the plain attack
            attack(10, 80, None, Some(false)), // hard strike
            attack(11, 0, Some(60_000), Some(false)), // re-attack
            attack(12, 0, None, Some(true)),  // tremmed variant
        ];
        let mut options = HashMap::new();
        options.insert((RankId(1), 24u16), variants.clone()); // key 60
        options.insert((RankId(1), 26u16), variants); // key 62
        console.set_attack_options(options);

        // A first gentle press: nothing has released yet and the trem
        // is off, so only the plain attack qualifies.
        let (starts, _) = console.note_on_manual(0, 60, 64);
        assert_eq!(starts.len(), 1);
        assert_eq!(starts[0].spec.sample, 0, "plain attack on a first press");
        console.note_off_manual(0, 60);

        // Re-pressed while the pipe is still speaking down: the
        // fast-repetition variant is more specific than the plain one.
        let (starts, _) = console.note_on_manual(0, 60, 64);
        assert_eq!(starts[0].spec.sample, 11, "re-attack within the window");
        console.note_off_manual(0, 60);

        // A hard strike: the velocity bound outranks the window.
        let (starts, _) = console.note_on_manual(0, 60, 127);
        assert_eq!(starts[0].spec.sample, 10, "velocity-bound attack");
        console.note_off_manual(0, 60);

        // Wave tremulant on: only the tremmed recording matches now
        // (fresh key so the re-attack window stays out of the picture).
        console.set_wave_tremulant(0, true);
        let (starts, _) = console.note_on_manual(0, 62, 64);
        assert_eq!(starts[0].spec.sample, 12, "tremmed variant under the trem");
    }

    /// Equally specific candidates rotate randomly (GO's tie-break) so
    /// repetition doesn't machine-gun one recording of the transient.
    #[test]
    fn equal_attack_candidates_are_rotated_at_random() {
        let mut console = test_console();
        console.set_drawn(StopId(2), false);
        let mut options = HashMap::new();
        options.insert(
            (RankId(1), 24u16),
            vec![attack(0, 0, None, None), attack(10, 0, None, None)],
        );
        console.set_attack_options(options);
        let mut seen = std::collections::HashSet::new();
        for _ in 0..40 {
            let (starts, _) = console.note_on_manual(0, 60, 64);
            seen.insert(starts[0].spec.sample);
            console.note_off_manual(0, 60);
        }
        assert!(
            seen.contains(&0) && seen.contains(&10),
            "40 presses never rotated: {seen:?}"
        );
    }

    /// An attack recorded at another sample rate carries its rate
    /// factor onto the spec so it still sounds at the pipe's pitch.
    #[test]
    fn selected_attack_applies_its_rate_factor() {
        let mut console = test_console();
        console.set_drawn(StopId(2), false);
        let mut options = HashMap::new();
        options.insert(
            (RankId(1), 24u16),
            vec![
                attack(0, 0, None, Some(false)),
                crate::bank::AttackOption {
                    rate_factor: 2.0,
                    ..attack(12, 0, None, Some(true))
                },
            ],
        );
        console.set_attack_options(options);
        console.set_wave_tremulant(0, true);
        let (starts, _) = console.note_on_manual(0, 60, 64);
        assert_eq!(starts[0].spec.sample, 12);
        assert!(
            (starts[0].spec.rate - 2.0).abs() < 1e-6,
            "rate follows the variant: {}",
            starts[0].spec.rate
        );
    }

    /// A whole-octave stop trim is an extension, not a tape-speed
    /// trick: mid-compass it re-anchors each key to the real pipe an
    /// octave up (rate untouched); past the rank's top the edge pipe
    /// stands in, repitched — the same borrowing a widened compass
    /// gets.
    #[test]
    fn stop_pitch_shift_reanchors_to_real_pipes() {
        let mut console = test_console();
        console.set_drawn(StopId(2), false);
        let mut adjust = HashMap::new();
        adjust.insert(StopId(1), vec![trim_rule(None, None, None, Some(1200.0), None)]);
        console.set_stop_adjust(adjust);
        let (starts, _) = console.note_on_manual(0, 60, 64);
        assert_eq!(starts.len(), 1);
        assert!(
            (starts[0].spec.rate - 1.0).abs() < 1e-6,
            "a real pipe an octave up speaks at its own rate: {}",
            starts[0].spec.rate
        );
        console.note_off_manual(0, 60);
        // Key 90 wants pipe 66 of a 61-pipe rank: the top pipe (60)
        // stands in, pulled up the remaining six semitones.
        let (starts, _) = console.note_on_manual(0, 90, 64);
        assert_eq!(starts.len(), 1);
        let expected = (6.0f32 / 12.0).exp2();
        assert!(
            (starts[0].spec.rate - expected).abs() < 1e-4,
            "past the rank's top the edge pipe is repitched: {}",
            starts[0].spec.rate
        );
    }

    /// Native footage is read off the recorded pitches: the pipe under
    /// a key against that key's 8' unison. Ranges that disagree — a
    /// mixture — yield no single number.
    #[test]
    fn stop_native_footage_reads_the_recorded_pitch() {
        let mut console = test_console();
        let ladder = |midi: f64| 440.0 * ((midi - 69.0) / 12.0).exp2();
        for pipe in 0..61u16 {
            console.specs.get_mut(&(RankId(1), pipe)).expect("spec").nominal_hz =
                ladder(36.0 + pipe as f64) as f32; // unison: an 8'
            console.specs.get_mut(&(RankId(2), pipe)).expect("spec").nominal_hz =
                ladder(48.0 + pipe as f64) as f32; // an octave up: a 4'
        }
        let principal = console.stop_native_footage(StopId(1)).expect("footage");
        assert!((principal - 8.0).abs() < 1e-3, "{principal}");
        let octave = console.stop_native_footage(StopId(2)).expect("footage");
        assert!((octave - 4.0).abs() < 1e-3, "{octave}");
        console.organ.stops[0].ranks.push(RankRange {
            rank: RankId(2),
            first_key: 0,
            key_count: 61,
            first_pipe: 0,
        });
        assert_eq!(
            console.stop_native_footage(StopId(1)),
            None,
            "a mixture speaks several footages"
        );
    }

    /// A `[[voicing.adjust]]` trim lands on every voice the stop
    /// prices: level directly, cents through the pitch fold.
    #[test]
    fn voicing_adjust_trims_level_and_pitch() {
        let mut console = test_console();
        console.set_drawn(StopId(2), false);
        let mut adjust = HashMap::new();
        // 1200·log2(1.01) cents — under half a semitone, so the pipe
        // stays its own and the whole trim lands as bend.
        adjust.insert(
            StopId(1),
            vec![trim_rule(None, None, Some(0.5), Some(1200.0 * 1.01f64.log2()), None)],
        );
        console.set_stop_adjust(adjust);
        let (starts, _) = console.note_on_manual(0, 60, 127);
        assert_eq!(starts.len(), 1);
        assert!(
            (starts[0].spec.gain - 0.5).abs() < 1e-6,
            "gain trim: {}",
            starts[0].spec.gain
        );
        assert!(
            (starts[0].spec.rate - 1.01).abs() < 1e-4,
            "pitch trim: {}",
            starts[0].spec.rate
        );
    }

    /// Voicing rules do not stack: per field, the rule that speaks
    /// about the fewest pipes wins, and a field it leaves unsaid falls
    /// through to the broader rule. The table is the whole contract.
    #[test]
    fn narrower_voicing_rules_win_per_field_and_never_stack() {
        let mut console = test_console();
        let rank = console.stop_ranks(StopId(1))[0].0;
        let other = RankId(999);
        let mut adjust = HashMap::new();
        adjust.insert(
            StopId(1),
            vec![
                // The stop: −6 dB, +10 cents, a touch dark.
                trim_rule(None, None, Some(0.5), Some(10.0), Some(0.8)),
                // Its bass octave: quieter still, and nothing about
                // pitch or tone.
                trim_rule(Some((36, 47)), None, Some(0.25), None, None),
                // One pipe of it: bright, and nothing about level.
                trim_rule(Some((40, 40)), None, None, None, Some(2.0)),
                // One rank across the whole compass: its own pitch.
                trim_rule(None, Some(rank), None, Some(-4.0), None),
            ],
        );
        console.set_stop_adjust(adjust);

        // Above the bass octave, on the named rank: the stop's level
        // and tone, the rank's pitch.
        let mid = console.trim_for(StopId(1), rank, 60);
        assert_eq!((mid.gain, mid.cents, mid.tilt), (0.5, -4.0, 0.8));
        // The same key on a rank nobody named: the stop's pitch.
        let mid_other = console.trim_for(StopId(1), other, 60);
        assert_eq!((mid_other.gain, mid_other.cents, mid_other.tilt), (0.5, 10.0, 0.8));
        // In the bass: the octave rule's level REPLACES the stop's
        // (0.25, not 0.5 × 0.25), and pitch/tone still fall through.
        let bass = console.trim_for(StopId(1), rank, 36);
        assert_eq!((bass.gain, bass.cents, bass.tilt), (0.25, -4.0, 0.8));
        // The single pipe: its own tone, the octave's level, the
        // rank's pitch — three rules, one pipe, no addition anywhere.
        let pipe = console.trim_for(StopId(1), rank, 40);
        assert_eq!((pipe.gain, pipe.cents, pipe.tilt), (0.25, -4.0, 2.0));
        // A stop nobody voiced is untouched.
        assert_eq!(console.trim_for(StopId(2), rank, 60), VoiceTrim::default());
    }

    /// The trim is stamped where the pipe is priced, so a key inside a
    /// narrowed rule speaks at its own level and tone and its
    /// neighbours do not.
    #[test]
    fn pricing_stamps_the_trim_of_the_key_that_sounds() {
        let mut console = test_console();
        console.set_drawn(StopId(2), false);
        let mut adjust = HashMap::new();
        adjust.insert(
            StopId(1),
            vec![
                trim_rule(None, None, Some(0.5), None, None),
                trim_rule(Some((60, 60)), None, Some(0.125), None, Some(2.0)),
            ],
        );
        console.set_stop_adjust(adjust);
        let (voiced, _) = console.note_on_manual(0, 60, 127);
        assert_eq!(voiced.len(), 1);
        assert!((voiced[0].spec.gain - 0.125).abs() < 1e-6, "{}", voiced[0].spec.gain);
        assert!((voiced[0].spec.voicing_tilt - 2.0).abs() < 1e-6);
        let (neighbour, _) = console.note_on_manual(0, 61, 127);
        assert_eq!(neighbour.len(), 1);
        assert!((neighbour[0].spec.gain - 0.5).abs() < 1e-6, "{}", neighbour[0].spec.gain);
        assert!((neighbour[0].spec.voicing_tilt - 1.0).abs() < 1e-6, "untouched");
    }

    /// A live level/tone edit reaches the pipe already speaking as a
    /// trim, not as a re-attack; a pitch edit big enough to re-anchor
    /// the key onto another pipe asks for a re-price instead.
    #[test]
    fn revoicing_trims_held_pipes_and_reprices_only_when_the_pipe_changes() {
        let mut console = test_console();
        console.set_drawn(StopId(2), false);
        let (started, _) = console.note_on_manual(0, 60, 127);
        assert_eq!(started.len(), 1);
        let handle = started[0].handle;

        let mut adjust = HashMap::new();
        adjust.insert(StopId(1), vec![trim_rule(None, None, Some(0.5), None, Some(2.0))]);
        console.set_stop_adjust(adjust);
        let update = console.revoice_stop(StopId(1));
        assert!(!update.reprice, "a level change re-speaks nothing");
        assert!(update.rates.is_empty(), "and moves no pitch");
        assert_eq!(update.trims.len(), 1);
        assert_eq!(update.trims[0].0, handle);
        assert!((update.trims[0].1 - 0.5).abs() < 1e-6, "gain against note-on");
        assert!((update.trims[0].2 - 2.0).abs() < 1e-6);

        // A second edit is still measured against the note-on gain, so
        // repeated drags can never compound.
        let mut adjust = HashMap::new();
        adjust.insert(StopId(1), vec![trim_rule(None, None, Some(0.25), None, Some(1.0))]);
        console.set_stop_adjust(adjust);
        let update = console.revoice_stop(StopId(1));
        assert!((update.trims[0].1 - 0.25).abs() < 1e-6);

        // A few cents glides; a whole octave re-anchors the key onto
        // the real pipe an octave up, which nothing can glide to.
        let mut adjust = HashMap::new();
        adjust.insert(StopId(1), vec![trim_rule(None, None, None, Some(12.0), None)]);
        console.set_stop_adjust(adjust);
        let update = console.revoice_stop(StopId(1));
        assert!(!update.reprice);
        assert_eq!(update.rates.len(), 1, "the held pipe glides");

        let mut adjust = HashMap::new();
        adjust.insert(StopId(1), vec![trim_rule(None, None, None, Some(1200.0), None)]);
        console.set_stop_adjust(adjust);
        assert!(console.revoice_stop(StopId(1)).reprice, "another pipe now");
    }

    /// The fixture manual is MIDI 36..96. Widening it to a keyboard
    /// that runs past both ends is the case the whole feature exists
    /// for: a 56-note set under a 61-note keyboard.
    #[test]
    fn keys_past_the_set_are_repitched_from_the_pipes_that_exist() {
        let mut console = test_console();
        console.set_compass(0, 31, 101);

        let (top, _) = console.note_on_manual(0, 101, 127);
        assert_eq!(top.len(), 2, "both stops speak five keys above the set");
        for start in &top {
            let semitones = start.spec.rate.log2() * 12.0;
            assert!(
                (semitones - 5.0).abs() < 1e-3,
                "top pipe stretched a fourth up, got {semitones} semitones"
            );
        }
        console.note_off_manual(0, 101);

        // Downward is the same mechanism and sounds better: a longer,
        // slower pipe is what the bottom of a keyboard wants anyway.
        let (bottom, _) = console.note_on_manual(0, 31, 127);
        assert_eq!(bottom.len(), 2);
        for start in &bottom {
            let semitones = start.spec.rate.log2() * 12.0;
            assert!((semitones + 5.0).abs() < 1e-3, "got {semitones} semitones");
        }
        console.note_off_manual(0, 31);

        // Inside the set's own compass nothing is repitched at all.
        let (native, _) = console.note_on_manual(0, 60, 127);
        assert!(native.iter().all(|s| (s.spec.rate - 1.0).abs() < 1e-6));
    }

    /// Two keys past the compass edge borrow the *same* physical pipe
    /// at two different pitches. They are two notes, and both must
    /// sound at once — a voice's identity is the key it stands in for,
    /// not the sample that happens to feed it.
    #[test]
    fn two_repitched_keys_off_the_same_pipe_sound_together() {
        let mut console = test_console();
        console.set_compass(0, 31, 101);

        let (first, _) = console.note_on_manual(0, 101, 127);
        assert_eq!(first.len(), 2);
        let (second, _) = console.note_on_manual(0, 100, 127);
        assert_eq!(
            second.len(),
            2,
            "the second repitched key must start its own voices"
        );
        for start in &second {
            let semitones = start.spec.rate.log2() * 12.0;
            assert!((semitones - 4.0).abs() < 1e-3, "got {semitones} semitones");
        }

        // Each key releases its own voices, not the other's.
        let released = console.note_off_manual(0, 101).0;
        assert_eq!(
            released,
            first.iter().map(|s| s.handle).collect::<Vec<_>>(),
            "the first key's voices stop while the second still holds"
        );
        let released = console.note_off_manual(0, 100).0;
        assert_eq!(released, second.iter().map(|s| s.handle).collect::<Vec<_>>());
    }

    #[test]
    fn keys_outside_the_compass_stay_silent() {
        let mut console = test_console();
        assert!(
            console.note_on_manual(0, 97, 127).0.is_empty(),
            "the set's compass is the default, and 97 is past it"
        );
        console.set_compass(0, 36, 97);
        assert_eq!(console.note_on_manual(0, 97, 127).0.len(), 2, "now it plays");
        console.note_off_manual(0, 97);
        assert!(
            console.note_on_manual(0, 98, 127).0.is_empty(),
            "one key past the keyboard is still nothing"
        );
    }

    /// A missing sample mid-compass is a defect in the set, not a
    /// musical decision, so its neighbour stands in.
    #[test]
    fn a_hole_in_a_rank_is_filled_by_its_neighbour() {
        let mut console = test_console();
        console.specs.remove(&(RankId(1), 24));

        let (starts, _) = console.note_on_manual(0, 60, 127);
        assert_eq!(starts.len(), 2, "the hole is filled, not skipped");
        let stretched: Vec<f32> = starts
            .iter()
            .map(|s| s.spec.rate.log2() * 12.0)
            .filter(|semitones| semitones.abs() > 1e-3)
            .collect();
        assert_eq!(stretched.len(), 1, "only the rank with the hole moves");
        assert!(
            stretched[0].abs() - 1.0 < 1e-3,
            "filled from a pipe one semitone away, got {}",
            stretched[0]
        );
    }

    /// Half-compass stops are real (divided registers, treble mixtures).
    /// Widening the keyboard must not invent the other half.
    #[test]
    fn a_stop_that_ends_early_is_not_extended() {
        let mut console = test_console();
        for stop in &mut console.organ.stops {
            if stop.id == StopId(2) {
                stop.ranks[0].key_count = 31;
            }
        }
        console.set_compass(0, 36, 101);

        assert_eq!(
            console.note_on_manual(0, 80, 127).0.len(),
            1,
            "inside the set's compass the short stop simply doesn't cover it"
        );
        console.note_off_manual(0, 80);
        assert_eq!(
            console.note_on_manual(0, 101, 127).0.len(),
            1,
            "and it is not carried past the set's compass either"
        );
    }

    /// A unit rank drawn at two pitches: Bourdon 16' on the Pedal, the
    /// same pipes again as a Bourdon 8' on the Swell. Each stop sees a
    /// window into one 73-pipe rank — the 16' the bottom 32, the 8'
    /// pipes 12..72.
    fn unit_rank_console() -> Console {
        let organ = Organ {
            name: "U".into(),
            base_path: Default::default(),
            manuals: vec![
                Manual {
                    id: ManualId(1),
                    name: "Pedal".into(),
                    first_midi_note: 36,
                    key_count: 32,
                    kind: Default::default(),
                    hex: None,
                },
                Manual {
                    id: ManualId(2),
                    name: "Swell".into(),
                    first_midi_note: 36,
                    key_count: 61,
                    kind: Default::default(),
                    hex: None,
                },
            ],
            stops: vec![
                Stop {
                    id: StopId(1),
                    name: "Bourdon 16".into(),
                    manual: ManualId(1),
                    ranks: vec![RankRange {
                        rank: RankId(1),
                        first_key: 0,
                        key_count: 32,
                        first_pipe: 0,
                    }],
                    own_pipes: false,
                },
                Stop {
                    id: StopId(2),
                    name: "Bourdon 8".into(),
                    manual: ManualId(2),
                    ranks: vec![RankRange {
                        rank: RankId(1),
                        first_key: 0,
                        key_count: 61,
                        first_pipe: 12,
                    }],
                    own_pipes: false,
                },
            ],
            ranks: vec![Rank {
                id: RankId(1),
                name: "Bourdon unit".into(),
                windchest: 1,
                velocity_volume: Default::default(),
                pipes: (0..73)
                    .map(|_| Pipe {
                        nominal_frequency_hz: 440.0,
                        pitch_tuning_cents: 0.0,
                        pitch_correction_cents: 0.0,
                        gain_db: 0.0,
                        midi_key_number: None,
                        midi_pitch_fraction_cents: None,
                        accepts_retuning: true,
                        source: PipeSource::Silent,
                    })
                    .collect(),
            }],
            couplers: vec![],
            enclosures: vec![],
            windchests: vec![],
            tremulants: vec![],
        };
        let mut specs = HashMap::new();
        for pipe in 0..73u16 {
            specs.insert(
                (RankId(1), pipe),
                VoiceSpec {
                    sample: 0,
                    rate: 1.0,
                    gain: 1.0,
                    velocity: Default::default(),
                    percussive: false,
                    group: 0,
                    wind_weight: 1.0,
                    brightness: 0.02,
                    voicing_tilt: 1.0,
                    nominal_hz: 440.0,
                    home_cents: 0.0,
                    model_cents: 0.0,
                    enclosures: [aristide_engine::enclosure::ENCLOSURE_NONE;
                        aristide_engine::enclosure::MAX_VOICE_ENCLOSURES],
                    bus: 0,
                    delay_frames: 0,
                },
            );
        }
        Console::new(organ, specs, vec![StopId(1), StopId(2)], 48_000.0)
    }

    /// GO's velocity ramp: gain runs linearly from `at_zero` (velocity
    /// 0) to `at_full` (127). Ranks without a ramp — the organ norm —
    /// must be untouched by whatever velocity arrives, and a stop drawn
    /// mid-hold joins the press at the velocity it was struck with.
    #[test]
    fn velocity_prices_voices_through_the_rank_ramp() {
        let mut console = test_console();
        for pipe in 0..61u16 {
            console
                .specs
                .get_mut(&(RankId(1), pipe))
                .expect("spec")
                .velocity = aristide_model::VelocityVolume {
                at_zero: 0.25,
                at_full: 1.0,
            };
        }
        // sample = rank - 1 in the fixture: 0 is the ramped Principal,
        // 1 the unramped Octave.
        let gain_of = |starts: &[VoiceStart], sample: u32| {
            starts
                .iter()
                .find(|s| s.spec.sample == sample)
                .expect("both stops speak")
                .spec
                .gain
        };

        let (starts, _) = console.note_on_manual(0, 60, 127);
        assert!((gain_of(&starts, 0) - 1.0).abs() < 1e-6, "full touch = full gain");
        assert!((gain_of(&starts, 1) - 1.0).abs() < 1e-6);
        console.note_off_manual(0, 60);

        let (starts, _) = console.note_on_manual(0, 64, 0);
        assert!((gain_of(&starts, 0) - 0.25).abs() < 1e-6, "softest touch = at_zero");
        assert!(
            (gain_of(&starts, 1) - 1.0).abs() < 1e-6,
            "an unramped rank never hears velocity"
        );
        console.note_off_manual(0, 64);

        let (starts, _) = console.note_on_manual(0, 67, 64);
        let expected = 0.25 + 0.75 * 64.0 / 127.0;
        assert!((gain_of(&starts, 0) - expected).abs() < 1e-6, "linear in between");
        console.note_off_manual(0, 67);

        // Mid-hold: the stop's late voices join the press's velocity.
        console.set_drawn(StopId(1), false);
        console.note_on_manual(0, 60, 0);
        let (_, starts) = console.set_drawn(StopId(1), true);
        assert!((gain_of(&starts, 0) - 0.25).abs() < 1e-6);
    }

    /// Widening a manual reaches keys the *stop* never had — but when
    /// the stop's rank is a unit rank, the pipes those keys want are
    /// often real (the other stop's window covers them). A real pipe at
    /// true pitch always beats the window's edge pipe stretched to
    /// imitate it.
    #[test]
    fn extended_keys_use_the_real_pipes_of_a_shared_rank() {
        let mut console = unit_rank_console();

        // Pedal native 36..67, widened a fourth up. Key 72 wants pipe
        // 36 — past the 16' window, squarely inside the 8' treble.
        console.set_compass(0, 36, 72);
        let (starts, _) = console.note_on_manual(0, 72, 127);
        assert_eq!(starts.len(), 1, "the extended pedal key speaks");
        assert!(
            (starts[0].spec.rate - 1.0).abs() < 1e-6,
            "pipe 36 exists in the unit rank; nothing may be repitched"
        );
        console.note_off_manual(0, 72);

        // Downward off the Swell 8': pipes 7..11 are the 16' bottom
        // the 8' window never reached, and they are equally real.
        console.set_compass(1, 31, 96);
        let (starts, _) = console.note_on_manual(1, 31, 127);
        assert_eq!(starts.len(), 1);
        assert!((starts[0].spec.rate - 1.0).abs() < 1e-6);
        console.note_off_manual(1, 31);

        // Past the rank's REAL end the old rule still holds: swell key
        // 99 wants pipe 75 of 73, so the last pipe stretches to serve.
        console.set_compass(1, 31, 101);
        let (starts, _) = console.note_on_manual(1, 99, 127);
        assert_eq!(starts.len(), 1);
        let semitones = starts[0].spec.rate.log2() * 12.0;
        assert!((semitones - 3.0).abs() < 1e-3, "got {semitones}");
    }

    /// Repitching fills in what the *player's keyboard* can reach. A
    /// coupler is not a keyboard: it may only sound pipes the division
    /// it points at actually has. A 16' coupler running off the bottom
    /// of a rank, or a coupler into a shorter division, sounds nothing
    /// there — inventing those pipes would be inventing an organ.
    #[test]
    fn couplers_never_repitch_by_default() {
        let mut console = coupled_console();
        console.set_compass(0, 24, 96); // a keyboard wider than the set
        console.set_coupler(1, true); // 16' I: −12 onto its own manual

        // The played key speaks at pitch, and its 16' copy speaks only
        // because that pipe exists.
        let (starts, _) = console.note_on_manual(0, 60, 127);
        assert_eq!(starts.len(), 2, "the key and its 16' copy");
        assert!(
            starts.iter().all(|s| (s.spec.rate - 1.0).abs() < 1e-6),
            "nothing is repitched inside the rank"
        );
        console.note_off_manual(0, 60);

        // Twelve keys from the bottom, the 16' copy runs off the end of
        // the rank. The key itself still speaks; the copy does not.
        let (starts, _) = console.note_on_manual(0, 40, 127);
        assert_eq!(
            starts.len(),
            1,
            "the 16' coupler must not repitch a pipe to reach below the rank"
        );
        assert!((starts[0].spec.rate - 1.0).abs() < 1e-6);
        console.note_off_manual(0, 40);

        // The same key played five below the set's own compass is
        // repitched — that is the player's keyboard, not a coupler.
        let (starts, _) = console.note_on_manual(0, 31, 127);
        assert_eq!(starts.len(), 1, "only the direct voice, repitched");
        let semitones = starts[0].spec.rate.log2() * 12.0;
        assert!((semitones + 5.0).abs() < 1e-3, "got {semitones}");
    }

    /// Coupling into a division whose rank stops short: the keys past
    /// its end are silent there, however wide the keyboard is.
    #[test]
    fn coupling_into_a_shorter_division_stops_where_it_stops() {
        let mut console = coupled_console();
        for stop in &mut console.organ.stops {
            if stop.id == StopId(2) {
                stop.ranks[0].key_count = 49; // the Swell is a short rank
            }
        }
        console.set_coupler(0, true); // II/I, unison

        let (starts, _) = console.note_on_manual(0, 60, 127);
        assert_eq!(starts.len(), 2, "both divisions have this key");
        console.note_off_manual(0, 60);

        let (starts, _) = console.note_on_manual(0, 90, 127);
        assert_eq!(
            starts.len(),
            1,
            "past the Swell's last pipe only the Great speaks"
        );
    }

    /// Sets built for it can ask for the other behaviour. The compass
    /// still bounds the coupler either way — that rule is older.
    #[test]
    fn a_set_can_ask_couplers_to_repitch() {
        let mut console = coupled_console();
        console.set_coupler_repitch(true);
        console.set_compass(0, 24, 96);
        console.set_coupler(1, true); // 16' I

        let (starts, _) = console.note_on_manual(0, 40, 127);
        assert_eq!(starts.len(), 2, "now the 16' copy is filled in");
        let stretched: Vec<f32> = starts
            .iter()
            .map(|s| s.spec.rate.log2() * 12.0)
            .filter(|semitones| semitones.abs() > 1e-3)
            .collect();
        assert_eq!(stretched.len(), 1);
        assert!((stretched[0] + 8.0).abs() < 1e-3, "got {}", stretched[0]);
    }

    #[test]
    fn note_on_starts_one_voice_per_drawn_stop() {
        let mut console = test_console();
        let (starts, _) = console.note_on_manual(0, 60, 127);
        assert_eq!(starts.len(), 2);
        let stops = console.note_off_manual(0, 60).0;
        assert_eq!(stops.len(), 2);
        assert_eq!(
            stops,
            starts.iter().map(|s| s.handle).collect::<Vec<_>>()
        );
    }

    /// Routing is a property of the stop: once installed, every voice
    /// the stop sounds carries its bus and speaking delay.
    #[test]
    fn stop_routing_stamps_bus_and_delay_on_voices() {
        let mut console = test_console();
        let (starts, _) = console.note_on_manual(0, 60, 127);
        assert!(starts.iter().all(|s| s.spec.bus == 0));
        assert!(starts.iter().all(|s| s.spec.delay_frames == 0));
        console.note_off_manual(0, 60);

        let routed = console.stop_states()[0].0;
        console.set_stop_routing([(routed, (3u8, 480u32))].into_iter().collect());
        let (starts, _) = console.note_on_manual(0, 60, 127);
        assert_eq!(starts.len(), 2);
        let stamped: Vec<(u8, u32)> = starts
            .iter()
            .map(|s| (s.spec.bus, s.spec.delay_frames))
            .collect();
        assert!(
            stamped.contains(&(3, 480)) && stamped.contains(&(0, 0)),
            "only the routed stop moves: {stamped:?}"
        );
    }

    #[test]
    fn drawing_a_stop_starts_pipes_under_held_keys() {
        let mut console = test_console();
        // With nothing drawn a press starts no voices but is still
        // tracked: the key lights and can be released cleanly.
        console.set_drawn(StopId(1), false);
        console.set_drawn(StopId(2), false);
        let (starts, _) = console.note_on_manual(0, 60, 127);
        assert!(starts.is_empty());
        assert_eq!(console.manual_states()[0].4, vec![60]);

        // Drawing a stop mid-hold speaks immediately under the key.
        let (stopped, starts) = console.set_drawn(StopId(1), true);
        assert!(stopped.is_empty());
        assert_eq!(starts.len(), 1);
        let first = starts[0].handle;

        // A second stop adds its own rank without restarting the first.
        let (_, starts) = console.set_drawn(StopId(2), true);
        assert_eq!(starts.len(), 1);
        let second = starts[0].handle;

        // Pushing a stop in releases only its pipe, via the normal
        // note-off path (release tail, not a cut).
        let (released, starts) = console.set_drawn(StopId(1), false);
        assert!(starts.is_empty());
        assert_eq!(released, vec![first]);

        // Releasing the key stops the remaining pipe and clears the light.
        assert_eq!(console.note_off_manual(0, 60).0, vec![second]);
        assert!(console.manual_states()[0].4.is_empty());
    }

    #[test]
    fn drawing_a_stop_reaches_held_keys_through_couplers() {
        let mut console = coupled_console();
        console.set_drawn(StopId(1), false);
        console.set_drawn(StopId(2), false);
        console.set_coupler(0, true); // II/I
        assert!(console.note_on_manual(0, 60, 127).0.is_empty());

        // The Swell stop drawn mid-hold sounds through the coupler.
        let (_, starts) = console.set_drawn(StopId(2), true);
        assert_eq!(starts.len(), 1);
        assert_eq!(starts[0].spec.sample, 1, "rank 2's sample expected");
        let handle = starts[0].handle;
        assert_eq!(console.note_off_manual(0, 60).0, vec![handle]);
    }

    #[test]
    fn keys_outside_the_manual_are_ignoredable() {
        let mut console = test_console();
        assert!(console.note_on_manual(0, 20, 127).0.is_empty());
        assert!(console.note_on_manual(0, 120, 127).0.is_empty());
        assert!(console.note_off_manual(0, 20).0.is_empty());
    }

    /// Two manuals with one stop each, plus II/I unison, 16' I (self,
    /// −12), and a deliberate I→II / II→I cycle pair.
    fn coupled_console() -> Console {
        let manual = |id: u32, name: &str| Manual {
            id: ManualId(id),
            name: name.into(),
            first_midi_note: 36,
            key_count: 61,
                    kind: Default::default(),
                    hex: None,
        };
        let stop = |id: u32, manual: u32, rank: u32| Stop {
            id: StopId(id),
            name: format!("stop {id}"),
            manual: ManualId(manual),
            ranks: vec![RankRange {
                rank: RankId(rank),
                first_key: 0,
                key_count: 61,
                first_pipe: 0,
            }],
            own_pipes: false,
        };
        let rank = |id: u32| Rank {
            id: RankId(id),
            name: format!("rank {id}"),
            windchest: 1,
            velocity_volume: Default::default(),
            pipes: (0..61)
                .map(|_| Pipe {
                    nominal_frequency_hz: 440.0,
                    pitch_tuning_cents: 0.0,
                    pitch_correction_cents: 0.0,
                    gain_db: 0.0,
                    midi_key_number: None,
                    midi_pitch_fraction_cents: None,
                    accepts_retuning: true,
                    source: PipeSource::Silent,
                })
                .collect(),
        };
        let coupler = |name: &str, from: u32, to: u32, shift: i16| {
            aristide_model::Coupler::simple(name, ManualId(from), ManualId(to), shift)
        };
        let organ = Organ {
            name: "C".into(),
            base_path: Default::default(),
            manuals: vec![manual(1, "Great"), manual(2, "Swell")],
            stops: vec![stop(1, 1, 1), stop(2, 2, 2)],
            ranks: vec![rank(1), rank(2)],
            couplers: vec![
                coupler("II/I", 1, 2, 0),
                coupler("16' I", 1, 1, -12),
                coupler("I/II", 2, 1, 0),
            ],
            enclosures: vec![],
            windchests: vec![],
            tremulants: vec![],
        };
        let mut specs = HashMap::new();
        for rank in 1..=2u32 {
            for pipe in 0..61u16 {
                specs.insert(
                    (RankId(rank), pipe),
                    VoiceSpec {
                        sample: rank - 1,
                        rate: 1.0,
                        gain: 1.0,
                        velocity: Default::default(),
                        percussive: false,
                        group: 0,
                        wind_weight: 1.0,
                        brightness: 0.0,
                        voicing_tilt: 1.0,
                        nominal_hz: 440.0,
                        home_cents: 0.0,
                        model_cents: 0.0,
                        enclosures: [aristide_engine::enclosure::ENCLOSURE_NONE;
                            aristide_engine::enclosure::MAX_VOICE_ENCLOSURES],
                        bus: 0,
                        delay_frames: 0,
                    },
                );
            }
        }
        Console::new(organ, specs, vec![StopId(1), StopId(2)], 48_000.0)
    }

    /// "A fourths coupler that plays a fourth down, but only from
    /// tenor C": a route with a source-key range.
    #[test]
    fn a_coupler_can_act_on_a_key_range_only() {
        let mut console = coupled_console();
        console.organ.couplers.push(aristide_model::Coupler {
            name: "Fourths II/I from tenor C".into(),
            routes: vec![aristide_model::CouplerRoute {
                from_manual: ManualId(1),
                low_key: Some(48),
                high_key: None,
                unison_off: false,
                scope: Default::default(),
                target: Some(aristide_model::CouplerTarget {
                    manual: ManualId(2),
                    key_shift: -5,
                    repitch: None,
                    own_pipes: false,
                }),
            }],
        });
        let index = console.organ.couplers.len() - 1;
        console.set_coupler(index, true);

        // Below tenor C the coupler simply isn't there.
        assert_eq!(console.note_on_manual(0, 47, 127).0.len(), 1);
        console.note_off_manual(0, 47);

        // From tenor C up: the played key plus its fourth-down copy on
        // II — a real pipe, nothing repitched.
        let (starts, _) = console.note_on_manual(0, 48, 127);
        assert_eq!(starts.len(), 2);
        let copy = starts.iter().find(|s| s.spec.sample == 1).expect("II speaks");
        assert!((copy.spec.rate - 1.0).abs() < 1e-6);
        console.note_off_manual(0, 48);
    }

    /// "A 16' coupler which transposes the bottom octave down instead
    /// of leaving it on": two routes — a classic doubling above the
    /// break, and below it a unison-off + repitch route that *moves*
    /// the note down, inventing the pipes the rank hasn't got.
    #[test]
    fn a_sixteen_foot_that_transposes_the_bottom_octave() {
        let mut console = coupled_console();
        let split = 36 + 12; // an octave above the fixture's bottom key
        let target = |repitch| {
            Some(aristide_model::CouplerTarget {
                manual: ManualId(1),
                key_shift: -12,
                repitch,
                own_pipes: false,
            })
        };
        console.organ.couplers.push(aristide_model::Coupler {
            name: "16' I".into(),
            routes: vec![
                aristide_model::CouplerRoute {
                    from_manual: ManualId(1),
                    low_key: Some(split),
                    high_key: None,
                    unison_off: false,
                    scope: Default::default(),
                    target: target(None),
                },
                aristide_model::CouplerRoute {
                    from_manual: ManualId(1),
                    low_key: None,
                    high_key: Some(split - 1),
                    unison_off: true,
                    scope: Default::default(),
                    target: target(Some(true)),
                },
            ],
        });
        let index = console.organ.couplers.len() - 1;
        console.set_coupler(index, true);

        // Above the break: the classic doubling, real pipes only.
        let (starts, _) = console.note_on_manual(0, 60, 127);
        assert_eq!(starts.len(), 2, "the key and its 16' copy");
        assert!(starts.iter().all(|s| (s.spec.rate - 1.0).abs() < 1e-6));
        console.note_off_manual(0, 60);

        // In the bottom octave the note moves: one voice, sounding an
        // octave below the played key, bent down from the deepest pipe
        // the rank has — past the compass, because this route asked to.
        let (starts, _) = console.note_on_manual(0, 40, 127);
        assert_eq!(starts.len(), 1, "unison off: the played key itself is silent");
        let semitones = starts[0].spec.rate.log2() * 12.0;
        assert!(
            (semitones + 8.0).abs() < 1e-3,
            "pipe 0 bent down to sound an octave below key 40, got {semitones}"
        );
        console.note_off_manual(0, 40);
    }

    /// Engaging or releasing a coupler lands on held notes at once,
    /// as an electric-action console does — the same way drawing a
    /// stop mid-hold already behaves.
    #[test]
    fn coupler_changes_land_on_held_notes() {
        let mut console = coupled_console();
        let (starts, _) = console.note_on_manual(0, 60, 127);
        assert_eq!(starts.len(), 1, "only the Great before coupling");

        // Engaging II/I under the held key speaks the Swell at once.
        let (stopped, starts) = console.set_coupler(0, true);
        assert!(stopped.is_empty());
        assert_eq!(starts.len(), 1);
        assert_eq!(starts[0].spec.sample, 1, "rank 2's sample");
        let swell_voice = starts[0].handle;

        // Releasing it lets go of exactly that voice.
        let (stopped, starts) = console.set_coupler(0, false);
        assert!(starts.is_empty());
        assert_eq!(stopped, vec![swell_voice]);

        // The key still sounds its own pipe, and note-off finds it.
        assert_eq!(console.note_off_manual(0, 60).0.len(), 1);
    }

    /// A pure unison-off coupler (GO's `UnisonOff=Y`): the manual's own
    /// sound is silenced, held notes included, and given back on
    /// release.
    #[test]
    fn a_unison_off_coupler_moves_held_notes() {
        let mut console = coupled_console();
        console.organ.couplers.push(aristide_model::Coupler {
            name: "Unison Off I".into(),
            routes: vec![aristide_model::CouplerRoute {
                from_manual: ManualId(1),
                low_key: None,
                high_key: None,
                unison_off: true,
                scope: Default::default(),
                target: None,
            }],
        });
        let index = console.organ.couplers.len() - 1;

        let (starts, _) = console.note_on_manual(0, 60, 127);
        let direct = starts[0].handle;

        // Engaging unison-off silences the held key's own division…
        let (stopped, starts) = console.set_coupler(index, true);
        assert_eq!(stopped, vec![direct]);
        assert!(starts.is_empty());

        // …and releasing it gives the note back. The silenced voice's
        // release tail is expedited so the pipe can't overlap itself.
        let (stopped, starts) = console.set_coupler(index, false);
        assert_eq!(stopped, vec![direct]);
        assert_eq!(starts.len(), 1);
        assert_eq!(console.note_off_manual(0, 60).0.len(), 1);
    }

    /// A Bass coupler (GO `CouplerType=Bass`): an automatic pedal —
    /// only the lowest currently-held key is coupled, and the coupled
    /// note follows that extreme as keys come and go.
    #[test]
    fn a_bass_coupler_follows_the_lowest_held_key() {
        let mut console = coupled_console();
        console.organ.couplers.push(aristide_model::Coupler {
            name: "Bass I/II".into(),
            routes: vec![aristide_model::CouplerRoute {
                from_manual: ManualId(1),
                low_key: None,
                high_key: None,
                unison_off: false,
                scope: aristide_model::CouplerScope::Bass,
                target: Some(aristide_model::CouplerTarget {
                    manual: ManualId(2),
                    key_shift: 0,
                    repitch: None,
                    own_pipes: false,
                }),
            }],
        });
        let index = console.organ.couplers.len() - 1;
        console.set_coupler(index, true);

        // The first key is trivially the bass: it doubles onto II.
        let (starts, stopped) = console.note_on_manual(0, 60, 127);
        assert!(stopped.is_empty());
        assert_eq!(starts.len(), 2, "the key and its coupled bass");
        let coupled_c = starts
            .iter()
            .find(|s| s.spec.sample == 1)
            .expect("II speaks")
            .handle;

        // A higher key adds no coupling: C4 stays the bass.
        let (starts, stopped) = console.note_on_manual(0, 64, 127);
        assert!(stopped.is_empty());
        assert_eq!(starts.len(), 1, "E4 sounds only itself");

        // A lower key takes the bass with it: the coupled note moves.
        let (starts, stopped) = console.note_on_manual(0, 55, 127);
        assert_eq!(stopped, vec![coupled_c], "the old bass copy lets go");
        assert_eq!(starts.len(), 2, "G3 and the bass at its new home");
        let coupled_g = starts
            .iter()
            .find(|s| s.spec.sample == 1)
            .expect("II speaks")
            .handle;

        // Releasing the bass key hands the coupled note back up to C4:
        // a note-off that *starts* a voice.
        let (stopped, starts) = console.note_off_manual(0, 55);
        assert!(stopped.contains(&coupled_g));
        assert_eq!(starts.len(), 1);
        assert_eq!(starts[0].spec.sample, 1, "the bass re-speaks under C4");

        // Once every key is up, nothing is left sounding.
        console.note_off_manual(0, 64);
        let (stopped, starts) = console.note_off_manual(0, 60);
        assert!(starts.is_empty());
        assert_eq!(stopped.len(), 2, "C4's own pipe and its coupled bass");
        assert!(console.speaking.is_empty());
    }

    /// A Melody coupler mirrors the Bass one at the top of the chord,
    /// and engaged mid-hold it lands on the current top note only.
    #[test]
    fn a_melody_coupler_follows_the_highest_held_key() {
        let mut console = coupled_console();
        console.organ.couplers.push(aristide_model::Coupler {
            name: "Melody I/II".into(),
            routes: vec![aristide_model::CouplerRoute {
                from_manual: ManualId(1),
                low_key: None,
                high_key: None,
                unison_off: false,
                scope: aristide_model::CouplerScope::Melody,
                target: Some(aristide_model::CouplerTarget {
                    manual: ManualId(2),
                    key_shift: 0,
                    repitch: None,
                    own_pipes: false,
                }),
            }],
        });
        let index = console.organ.couplers.len() - 1;

        // A chord held before the coupler is engaged.
        console.note_on_manual(0, 60, 127);
        console.note_on_manual(0, 67, 127);

        // Engaging mid-hold couples exactly the top note.
        let (stopped, starts) = console.set_coupler(index, true);
        assert!(stopped.is_empty());
        assert_eq!(starts.len(), 1, "one coupled copy: the melody");
        assert_eq!(starts[0].spec.sample, 1, "rank 2's sample");
        let coupled_g = starts[0].handle;

        // A higher key takes the melody over.
        let (starts, stopped) = console.note_on_manual(0, 72, 127);
        assert_eq!(stopped, vec![coupled_g]);
        assert_eq!(starts.len(), 2, "C5 and the melody moved onto it");

        // Releasing it hands the melody back to G4.
        let (_, starts) = console.note_off_manual(0, 72);
        assert_eq!(starts.len(), 1);
        assert_eq!(starts[0].spec.sample, 1, "the melody re-speaks under G4");

        console.note_off_manual(0, 67);
        console.note_off_manual(0, 60);
        assert!(console.speaking.is_empty());
    }

    #[test]
    fn couplers_route_between_manuals_and_octaves() {
        let mut console = coupled_console();
        // Channel 0 → Great (no pedal in this organ → identity map).
        assert_eq!(console.note_on_manual(0, 60, 127).0.len(), 1, "no couplers yet");
        console.note_off_manual(0, 60);

        console.set_coupler(0, true); // II/I
        assert_eq!(console.note_on_manual(0, 60, 127).0.len(), 2, "unison coupler adds II");
        assert_eq!(console.note_off_manual(0, 60).0.len(), 2, "note-off kills both");

        console.set_coupler(1, true); // 16' I (self, −12)
        // Great C + Swell C (II/I) + Great C−12 (16' I). Coupled notes
        // don't re-couple, so the sub-octave stays on the Great.
        assert_eq!(console.note_on_manual(0, 60, 127).0.len(), 3);
        console.note_off_manual(0, 60);

        // Out-of-compass shifted notes drop out quietly.
        assert_eq!(console.note_on_manual(0, 37, 127).0.len(), 2, "37-12 is below compass");
        console.note_off_manual(0, 37);
    }

    /// The organ as recorded is the default and touches nothing: a set
    /// whose pipes all measured a semitone flat plays them as they
    /// are. A target tuning bends each pipe from where it *measured*,
    /// so "440 equal" lands exactly there; `Original` with the
    /// reference pulled to 440 moves the whole instrument as one; and
    /// a held note follows the switch live.
    #[test]
    fn targets_bend_from_the_measured_pitch_and_original_leaves_it() {
        let mut console = test_console();
        let anchor = 1200.0 * (415.0f64 / 440.0).log2();
        // Every pipe sits 3 cents sharp of where the organ's tuning
        // puts it: its own drift.
        let drift = 3.0;
        for spec in console.specs.values_mut() {
            spec.home_cents = (anchor + drift) as f32;
            spec.model_cents = anchor as f32;
        }
        let home = std::sync::Arc::new(crate::tuning::HomeTuning {
            a4_hz: 415.0,
            offsets_cents: [0.0; 12],
            temperament: Some(crate::tuning::Temperament::Equal),
            spread_cents: 0.0,
            measured: 61,
            pipes: 61,
        });
        console.set_home(Some(home.clone()));
        let rate_of = |console: &mut Console| {
            let rate = console.note_on_manual(0, 60, 127).0[0].spec.rate;
            console.note_off_manual(0, 60);
            rate
        };
        let cents = |rate: f32| 1200.0 * (rate as f64).log2();

        // As recorded, at the organ's own a′: untouched.
        console.set_tuning(crate::tuning::Tuning {
            reference: home.reference(69),
            ..crate::tuning::Tuning::default()
        });
        assert!(console.tuning().home.is_some(), "the console stamps its home");
        assert!(cents(rate_of(&mut console)).abs() < 1e-3, "as recorded");

        // As recorded, pulled to a′ = 440: the whole organ up 101 cents.
        console.set_tuning(crate::tuning::Tuning {
            reference: crate::tuning::PitchReference::A440,
            ..crate::tuning::Tuning::default()
        });
        assert!((cents(rate_of(&mut console)) + anchor).abs() < 0.01, "pulled as one");

        // Equal at 440, pipes original: each pipe moves by what the
        // model moves by and keeps its 3 cents of drift.
        console.set_tuning(crate::tuning::Tuning {
            temperament: crate::tuning::Temperament::Equal,
            reference: crate::tuning::PitchReference::A440,
            ..crate::tuning::Tuning::default()
        });
        assert!((cents(rate_of(&mut console)) + anchor).abs() < 0.01, "drift kept");
        // Pipes exact: from the measured pitch, the drift goes too.
        console.set_tuning(crate::tuning::Tuning {
            temperament: crate::tuning::Temperament::Equal,
            reference: crate::tuning::PitchReference::A440,
            pipes: crate::tuning::PipeRetune::Exact,
            ..crate::tuning::Tuning::default()
        });
        assert!(
            (cents(rate_of(&mut console)) + anchor + drift).abs() < 0.01,
            "target is exact"
        );

        // Meantone at 440: C sits +10.265 above equal, from measured.
        console.set_tuning(crate::tuning::Tuning {
            temperament: crate::tuning::Temperament::Meantone4,
            reference: crate::tuning::PitchReference::A440,
            ..crate::tuning::Tuning::default()
        });
        assert!((cents(rate_of(&mut console)) + anchor - 10.265).abs() < 0.01);

        // Held under Original, switched to 440 equal: glides the same
        // 101 cents.
        console.set_tuning(crate::tuning::Tuning {
            reference: home.reference(69),
            ..crate::tuning::Tuning::default()
        });
        let started = console.note_on_manual(0, 60, 127).0[0].spec.rate;
        console.set_tuning(crate::tuning::Tuning {
            temperament: crate::tuning::Temperament::Equal,
            reference: crate::tuning::PitchReference::A440,
            ..crate::tuning::Tuning::default()
        });
        let moved = console.retune_held();
        assert_eq!(moved.len(), 2, "both drawn stops' voices move");
        for (_, rate) in moved {
            assert!((cents(rate / started) + anchor).abs() < 0.01, "held voice follows");
        }
        console.note_off_manual(0, 60);
    }

    #[test]
    fn tuning_retunes_and_transposes() {
        let mut console = test_console();
        // Equal temperament, a=440: everything at unity rate.
        let baseline = console.note_on_manual(0, 60, 127).0[0].spec.rate;
        assert!((baseline - 1.0).abs() < 1e-6);
        console.note_off_manual(0, 60);

        // Meantone C sits +10.265 cents above equal (a-referenced).
        console.set_tuning(crate::tuning::Tuning {
            edo: 12,
            temperament: crate::tuning::Temperament::Meantone4,
            scale: None,
            reference: crate::tuning::PitchReference::A440,
            transpose: 0,
            pipes: crate::tuning::PipeRetune::Original,
            home: None,
        });
        let meantone_c = console.note_on_manual(0, 60, 127).0[0].spec.rate;
        let expected = (10.265f32 / 1200.0).exp2();
        assert!(
            (meantone_c - expected).abs() < 1e-4,
            "meantone C rate {meantone_c} vs {expected}"
        );
        console.note_off_manual(0, 60);

        // Transpose +2: key 60 routes to pipe 62 (rate reflects D's
        // offset, and the sounding pipe index shifts).
        console.set_tuning(crate::tuning::Tuning {
            edo: 12,
            temperament: crate::tuning::Temperament::Equal,
            scale: None,
            reference: crate::tuning::PitchReference::A440,
            transpose: 2,
            pipes: crate::tuning::PipeRetune::Original,
            home: None,
        });
        let (transposed, _) = console.note_on_manual(0, 60, 127);
        assert_eq!(transposed.len(), 2, "both drawn stops sound");
        // Pipe index = key 62 − first_midi 36 = 26; sample index equals
        // rank − 1 in the fixture, so instead verify by keying at the
        // compass edge: 96 + 2 is out of range → silent.
        console.note_off_manual(0, 60);
        assert!(console.note_on_manual(0, 96, 127).0.is_empty(), "96+2 exceeds compass");
    }

    /// One instrument, two pitches: the Swell tuned apart speaks its
    /// own temperament even when a coupler reaches it from the Great —
    /// pipes are tuned where they stand. And a per-division transpose
    /// moves only its own keyboard.
    #[test]
    fn a_division_can_be_tuned_apart() {
        let mut console = coupled_console();
        console.set_manual_tuning(
            1,
            Some(crate::tuning::Tuning {
                edo: 12,
                temperament: crate::tuning::Temperament::Meantone4,
                scale: None,
                reference: crate::tuning::PitchReference::A440,
                transpose: 0,
                pipes: crate::tuning::PipeRetune::Original,
                home: None,
            }),
        );
        console.set_coupler(0, true); // II/I: playing the Great adds the Swell
        let (starts, _) = console.note_on_manual(0, 60, 127);
        assert_eq!(starts.len(), 2);
        let mut rates: Vec<f32> = starts.iter().map(|s| s.spec.rate).collect();
        rates.sort_by(f32::total_cmp);
        let meantone_c = (10.265f32 / 1200.0).exp2();
        assert!((rates[0] - 1.0).abs() < 1e-6, "Great stays equal: {rates:?}");
        assert!(
            (rates[1] - meantone_c).abs() < 1e-4,
            "coupled copy speaks the Swell's meantone: {rates:?}"
        );
        console.note_off_manual(0, 60);
        console.set_coupler(0, false);

        console.set_manual_tuning(
            0,
            Some(crate::tuning::Tuning {
                edo: 12,
                temperament: crate::tuning::Temperament::Equal,
                scale: None,
                reference: crate::tuning::PitchReference::A440,
                transpose: 2,
                pipes: crate::tuning::PipeRetune::Original,
                home: None,
            }),
        );
        assert!(console.note_on_manual(0, 96, 127).0.is_empty(), "96+2 runs off the Great");
        assert_eq!(console.note_on_manual(1, 96, 127).0.len(), 1, "the Swell is unmoved");
        // Back on the shared tuning, the Great answers again.
        console.set_manual_tuning(0, None);
        assert_eq!(console.note_on_manual(0, 96, 127).0.len(), 1);
    }

    fn temperament_tuning(temperament: crate::tuning::Temperament) -> crate::tuning::Tuning {
        crate::tuning::Tuning {
            temperament,
            ..crate::tuning::Tuning::default()
        }
    }

    /// The rate the Great's stop speaks middle C at, then released.
    fn great_c_rate(console: &mut Console) -> f32 {
        let (starts, _) = console.note_on_manual(0, 60, 127);
        assert_eq!(starts.len(), 1, "one voice");
        let rate = starts[0].spec.rate;
        console.note_off_manual(0, 60);
        rate
    }

    /// A stop plays its own tuning; else what its pin names; else its
    /// division's own, its set's own, the instrument's — in that order.
    #[test]
    fn a_stop_resolves_division_over_set_over_instrument() {
        use crate::tuning::{Follow, Temperament, TuningScope};
        let mut console = coupled_console();
        console.set_stop_sources(HashMap::from([(StopId(1), "positif".to_string())]));
        let meantone_c = (10.265f32 / 1200.0).exp2();
        let pythagorean_c = (-5.865f32 / 1200.0).exp2();
        let werckmeister_c = (11.730f32 / 1200.0).exp2();
        let near = |a: f32, b: f32| (a - b).abs() < 1e-4;

        console.set_tuning(temperament_tuning(Temperament::Equal));
        assert!(near(great_c_rate(&mut console), 1.0));
        assert_eq!(console.stop_tuning_resolved(StopId(1)).1, TuningScope::Organ);

        // The set tuned apart: its stop follows it…
        console.set_source_tuning("positif", Some(temperament_tuning(Temperament::Meantone4)));
        assert!(near(great_c_rate(&mut console), meantone_c), "follows the set");
        assert_eq!(console.stop_tuning_resolved(StopId(1)).1, TuningScope::Source);
        // …and a stop from another set does not.
        assert_eq!(console.stop_tuning_resolved(StopId(2)).1, TuningScope::Organ);

        // The division tuned apart wins over the set.
        console.set_manual_tuning(0, Some(temperament_tuning(Temperament::Pythagorean)));
        assert!(near(great_c_rate(&mut console), pythagorean_c), "the division wins");
        assert_eq!(console.stop_tuning_resolved(StopId(1)).1, TuningScope::Division);

        // Pinned, the stop skips whatever the pin doesn't name.
        console.set_stop_follow(StopId(1), Follow::Source);
        assert!(near(great_c_rate(&mut console), meantone_c), "pinned to the set");
        console.set_stop_follow(StopId(1), Follow::Organ);
        assert!(near(great_c_rate(&mut console), 1.0), "pinned to the instrument");
        // A pin naming a scope with no tuning of its own falls through
        // to the instrument, never to the other axis.
        console.set_manual_tuning(0, None);
        console.set_stop_follow(StopId(1), Follow::Division);
        assert!(near(great_c_rate(&mut console), 1.0), "an untuned division is the instrument");
        assert_eq!(console.stop_tuning_resolved(StopId(1)).1, TuningScope::Organ);

        // A tuning of its own beats everything and drops the pin.
        console.set_stop_tuning(StopId(1), Some(temperament_tuning(Temperament::Werckmeister3)));
        assert!(near(great_c_rate(&mut console), werckmeister_c));
        assert_eq!(console.stop_tuning_resolved(StopId(1)).1, TuningScope::Stop);
        assert_eq!(console.stop_follow(StopId(1)), Follow::Auto);
        assert_eq!(console.stop_tunings().len(), 1);
        // Back to automatic: the set again.
        console.set_stop_tuning(StopId(1), None);
        assert!(near(great_c_rate(&mut console), meantone_c));
        assert!(console.stop_tunings().is_empty());
    }

    /// A mixture: one stop sounding two ranks at once. Tuning one rank
    /// apart moves that rank only, only when heard through that stop,
    /// and live retuning follows the same seam.
    #[test]
    fn a_rank_tunes_apart_within_its_stop() {
        use crate::tuning::Temperament;
        let mut console = coupled_console();
        // Turn stop 1 into a two-rank mixture on the Great: rank 1 and
        // rank 2 under every key; stop 2 (the Swell's) keeps rank 2.
        console.organ.stops[0].ranks.push(RankRange {
            rank: RankId(2),
            first_key: 0,
            key_count: 61,
            first_pipe: 0,
        });
        console.set_tuning(temperament_tuning(Temperament::Equal));
        console.set_rank_tuning(StopId(1), RankId(2), Some(temperament_tuning(Temperament::Meantone4)));
        let meantone_c = (10.265f32 / 1200.0).exp2();

        let (starts, _) = console.note_on_manual(0, 60, 127);
        assert_eq!(starts.len(), 2, "both ranks speak");
        let rate_of = |sample: u32| {
            starts
                .iter()
                .find(|s| s.spec.sample == sample)
                .map(|s| s.spec.rate)
                .expect("rank speaks")
        };
        assert!((rate_of(0) - 1.0).abs() < 1e-6, "rank 1 stays equal");
        assert!((rate_of(1) - meantone_c).abs() < 1e-4, "rank 2 plays meantone");

        // Held, retuning the rank moves exactly its voice.
        console.set_rank_tuning(StopId(1), RankId(2), Some(temperament_tuning(Temperament::Pythagorean)));
        let moved = console.retune_held();
        assert_eq!(moved.len(), 1, "one voice drifts: {moved:?}");
        let pythagorean_c = (-5.865f32 / 1200.0).exp2();
        assert!((moved[0].1 - pythagorean_c).abs() < 1e-4, "to Pythagorean: {moved:?}");
        console.note_off_manual(0, 60);

        // Heard through the Swell's stop, rank 2 is untouched.
        let (starts, _) = console.note_on_manual(1, 60, 127);
        assert_eq!(starts.len(), 1);
        assert!((starts[0].spec.rate - 1.0).abs() < 1e-6, "another stop's rank 2 is the instrument's");
        console.note_off_manual(1, 60);

        assert_eq!(console.stop_ranks(StopId(1)), vec![(RankId(1), "rank 1"), (RankId(2), "rank 2")]);
        assert_eq!(console.rank_tunings().len(), 1);
        console.set_rank_tuning(StopId(1), RankId(2), None);
        assert!(console.rank_tunings().is_empty());
    }

    /// A Scala scale re-anchors keys to the nearest recorded pipe: a
    /// 19-EDO key mid-ladder is closer to another pipe than to its
    /// own, and it sounds that one, bent under half a semitone. The
    /// anchor key and every whole period above it come out exactly
    /// unbent — the octave is a real pipe.
    #[test]
    fn a_scala_scale_reanchors_keys_to_near_pipes() {
        let mut scl = String::from("! 19edo.scl\n19-EDO\n19\n");
        for degree in 1..=19 {
            scl.push_str(&format!("{:.6}\n", degree as f64 * 1200.0 / 19.0));
        }
        let scale = aristide_model::scala::Scale::parse(&scl).expect("parses");
        let tuning = crate::tuning::Tuning {
            edo: 12,
            temperament: crate::tuning::Temperament::Equal,
            scale: Some(std::sync::Arc::new(crate::tuning::ScaleTuning {
                scl: "19edo.scl".into(),
                kbm: None,
                scale,
                mapping: crate::tuning::PitchReference::A440.linear_mapping(),
            })),
            reference: crate::tuning::PitchReference::A440,
            transpose: 0,
            pipes: crate::tuning::PipeRetune::Original,
            home: None,
        };
        let mut console = coupled_console();
        console.set_manual_tuning(0, Some(tuning));

        // The anchor: a′ is a′, the nominal pipe untouched.
        let (starts, _) = console.note_on_manual(0, 69, 127);
        assert_eq!(starts.len(), 1);
        assert!((starts[0].spec.rate - 1.0).abs() < 1e-6, "a' unbent: {}", starts[0].spec.rate);
        console.note_off_manual(0, 69);

        // Five 19-EDO steps up = 315.79¢, where the ladder has 500¢:
        // re-anchored two pipes down, bent +15.79¢.
        let (starts, _) = console.note_on_manual(0, 74, 127);
        assert_eq!(starts.len(), 1);
        let expected = ((5.0 * 1200.0 / 19.0 - 300.0) / 1200.0f32).exp2();
        assert!(
            (starts[0].spec.rate - expected).abs() < 1e-4,
            "bend under half a semitone: {} vs {expected}",
            starts[0].spec.rate
        );
        console.note_off_manual(0, 74);

        // Nineteen steps = one octave exactly: key 88 sounds the pipe
        // seven below its own, unbent.
        let (starts, _) = console.note_on_manual(0, 88, 127);
        assert_eq!(starts.len(), 1);
        assert!(
            (starts[0].spec.rate - 1.0).abs() < 1e-6,
            "the octave is a real pipe: {}",
            starts[0].spec.rate
        );
        console.note_off_manual(0, 88);

        // The Swell follows the shared tuning, untouched by all this.
        let (starts, _) = console.note_on_manual(1, 74, 127);
        assert_eq!(starts.len(), 1);
        assert!((starts[0].spec.rate - 1.0).abs() < 1e-6);
    }

    /// A tuning change lands on held voices: retune_held returns each
    /// sounding voice's handle with its rate moved by the exact cent
    /// delta, and a second call reports nothing left to move.
    #[test]
    fn a_tuning_change_drifts_held_voices() {
        let mut console = test_console();
        let (starts, _) = console.note_on_manual(0, 60, 127);
        assert_eq!(starts.len(), 2, "both drawn stops sound");
        let mut tuning = console.tuning();
        tuning.reference.hz = 415.0;
        console.set_tuning(tuning);
        let updates = console.retune_held();
        assert_eq!(updates.len(), starts.len(), "every held voice drifts");
        let drop = (415.0f64 / 440.0) as f32;
        for (handle, rate) in &updates {
            let start = starts
                .iter()
                .find(|s| s.handle == *handle)
                .expect("update names a started voice");
            assert!(
                (rate / (start.spec.rate * drop) - 1.0).abs() < 1e-4,
                "handle {handle}: rate {rate} vs {} × 415/440",
                start.spec.rate
            );
        }
        assert!(console.retune_held().is_empty(), "already settled");
        // Transpose changes reroute future presses, not held pipes.
        let mut tuning = console.tuning();
        tuning.transpose = 2;
        console.set_tuning(tuning);
        assert!(console.retune_held().is_empty(), "transposer leaves held notes");
    }

    /// Drifting onto a Scala scale re-prices a held key against the
    /// pitch the scale wants — on the voice's own pipe, however far
    /// that now is: a pipe mid-speech glides, it is not re-recorded.
    #[test]
    fn a_scale_change_drifts_a_held_key_to_its_scale_pitch() {
        let mut scl = String::from("! 19edo.scl\n19-EDO\n19\n");
        for degree in 1..=19 {
            scl.push_str(&format!("{:.6}\n", degree as f64 * 1200.0 / 19.0));
        }
        let scale = aristide_model::scala::Scale::parse(&scl).expect("parses");
        let tuning = crate::tuning::Tuning {
            edo: 12,
            temperament: crate::tuning::Temperament::Equal,
            scale: Some(std::sync::Arc::new(crate::tuning::ScaleTuning {
                scl: "19edo.scl".into(),
                kbm: None,
                scale,
                mapping: crate::tuning::PitchReference::A440.linear_mapping(),
            })),
            reference: crate::tuning::PitchReference::A440,
            transpose: 0,
            pipes: crate::tuning::PipeRetune::Original,
            home: None,
        };
        let mut console = coupled_console();
        let (starts, _) = console.note_on_manual(0, 74, 127);
        assert_eq!(starts.len(), 1);
        assert!((starts[0].spec.rate - 1.0).abs() < 1e-6);
        console.set_manual_tuning(0, Some(tuning));
        let updates = console.retune_held();
        assert_eq!(updates.len(), 1);
        // Five 19-EDO steps above a′ = 315.79¢ where the key's own
        // pipe sits at 500¢: the held voice bends down the difference.
        let expected = ((5.0 * 1200.0 / 19.0 - 500.0) / 1200.0f32).exp2();
        assert!(
            (updates[0].1 - expected).abs() < 1e-4,
            "drift to the scale pitch on the old pipe: {} vs {expected}",
            updates[0].1
        );
        // The anchor key holds still under the same change.
        console.set_manual_tuning(0, None);
        console.note_off_manual(0, 74);
    }

    /// Per-note bends ride on top of the tuning: absolute, replaced by
    /// each message, cleared by zero, and preserved across a drift.
    #[test]
    fn per_note_bends_stack_with_tuning_drift() {
        let mut console = coupled_console();
        let (starts, _) = console.note_on_manual(0, 60, 127);
        assert_eq!(starts.len(), 1);
        let base = starts[0].spec.rate;
        let handle = starts[0].handle;

        let up = console.bend_key(0, 60, 25.0);
        assert_eq!(up, vec![(handle, base * (25.0f64 / 1200.0).exp2() as f32)]);
        assert!(console.bend_key(0, 60, 25.0).is_empty(), "no-op repeat");

        // A drift under the bend: both apply.
        let mut tuning = console.tuning();
        tuning.reference.hz = 415.0;
        console.set_tuning(tuning);
        let updates = console.retune_held();
        assert_eq!(updates.len(), 1);
        let expected =
            base * (415.0f64 / 440.0) as f32 * (25.0f64 / 1200.0).exp2() as f32;
        assert!(
            (updates[0].1 - expected).abs() < 1e-4,
            "bend survives the drift: {} vs {expected}",
            updates[0].1
        );

        let cleared = console.bend_key(0, 60, 0.0);
        assert_eq!(cleared.len(), 1);
        assert!(
            (cleared[0].1 - base * (415.0f64 / 440.0) as f32).abs() < 1e-4,
            "zero bend returns to the tuning's pitch"
        );
        console.note_off_manual(0, 60);
        assert!(console.bend_key(0, 60, 10.0).is_empty(), "nothing held");
    }

    /// A keyboard mapping's unmapped keys (`x` entries) sound nothing:
    /// silence is what the mapping says, not a defect to heal over.
    #[test]
    fn unmapped_scala_keys_are_silent() {
        let scl = "! whole.scl\nWhole tones\n6\n200.0\n400.0\n600.0\n800.0\n1000.0\n1200.0\n";
        let kbm = "! every other key\n2\n0\n127\n60\n60\n261.625565\n6\n0\nx\n";
        let scale = aristide_model::scala::Scale::parse(scl).expect("scl parses");
        let mapping = aristide_model::scala::KeyboardMapping::parse(kbm).expect("kbm parses");
        let tuning = crate::tuning::Tuning {
            edo: 12,
            temperament: crate::tuning::Temperament::Equal,
            scale: Some(std::sync::Arc::new(crate::tuning::ScaleTuning {
                scl: "whole.scl".into(),
                kbm: Some("every-other.kbm".into()),
                scale,
                mapping,
            })),
            reference: crate::tuning::PitchReference::A440,
            transpose: 0,
            pipes: crate::tuning::PipeRetune::Original,
            home: None,
        };
        let mut console = coupled_console();
        console.set_manual_tuning(0, Some(tuning));
        assert_eq!(console.note_on_manual(0, 60, 127).0.len(), 1, "mapped key sounds");
        console.note_off_manual(0, 60);
        assert!(console.note_on_manual(0, 61, 127).0.is_empty(), "unmapped key is silent");
    }

    /// Moving a stop re-homes it mid-hold: the key holding its new
    /// manual picks it up, its old manual gives it up.
    #[test]
    fn moving_a_stop_rehomes_it_under_held_keys() {
        let mut console = coupled_console();
        let (starts, _) = console.note_on_manual(0, 60, 127);
        assert_eq!(starts.len(), 1, "only the Great's own stop");
        let (stopped, starts) = console.move_stop(StopId(2), 0);
        assert!(stopped.is_empty(), "nothing sounded on the Swell");
        assert_eq!(starts.len(), 1, "the moved stop speaks under the held key");
        assert!(console.note_on_manual(1, 62, 127).0.is_empty(), "the Swell gave it up");
        assert_eq!(console.note_off_manual(0, 60).0.len(), 2);
        assert_eq!(console.stop_states()[1].3, 0, "stop 2 reports the Great");
    }

    /// A coupler taken off the console releases, hides, and refuses
    /// engagement — and comes back whole when restored.
    #[test]
    fn a_coupler_off_the_console_stays_restorable() {
        let mut console = coupled_console();
        console.set_coupler(0, true);
        assert_eq!(console.note_on_manual(0, 60, 127).0.len(), 2);
        console.note_off_manual(0, 60);

        console.set_coupler_available(0, false);
        assert!(!console.coupler_engaged(0));
        assert!(!console.coupler_states()[0].3);
        assert_eq!(console.note_on_manual(0, 60, 127).0.len(), 1);
        console.note_off_manual(0, 60);
        console.set_coupler(0, true);
        assert!(!console.coupler_engaged(0), "off the console means unpullable");

        console.set_coupler_available(0, true);
        console.set_coupler(0, true);
        assert_eq!(console.note_on_manual(0, 60, 127).0.len(), 2);
    }

    #[test]
    fn shared_pipes_speak_once_across_octave_coupling() {
        // The F-major pop: with a 16' coupler, key 72's coupled pipe IS
        // key 60's direct pipe. It must not start a second voice — the
        // identical recording would sum coherently (+6 dB) and the
        // phase-aligned release would make both tails coherent too:
        // a release LOUDER than the chord.
        let mut console = coupled_console();
        console.set_coupler(1, true); // 16' I: self-coupler at −12

        let (first, _) = console.note_on_manual(0, 72, 127);
        assert_eq!(first.len(), 2, "72 direct + coupled 60");
        let (second, _) = console.note_on_manual(0, 60, 127);
        assert_eq!(
            second.len(),
            1,
            "60's direct pipe already speaks via 72's coupling — only \
             the new 48-pipe may start"
        );

        // Releasing 72 must NOT stop the shared pipe (60 still holds it).
        let stopped = console.note_off_manual(0, 72).0;
        assert_eq!(stopped.len(), 1, "only 72's unshared pipe stops");
        // Releasing 60 stops the shared pipe and 60's own coupled pipe.
        let stopped = console.note_off_manual(0, 60).0;
        assert_eq!(stopped.len(), 2, "shared pipe + 48-pipe stop last");

        // Every started voice eventually stopped exactly once.
        assert!(console.note_off_manual(0, 60).0.is_empty());
        assert!(console.note_off_manual(0, 72).0.is_empty());
    }

    /// A rank borrowed whole onto another manual, the unit-organ way:
    /// rank 2's pipes are `Borrowed` references into rank 1, and (as
    /// the bank loader does) their specs are the donor's copied.
    fn borrowed_console(borrower_owns_pipes: bool) -> Console {
        let manual = |id: u32, name: &str| Manual {
            id: ManualId(id),
            name: name.into(),
            first_midi_note: 36,
            key_count: 61,
            kind: Default::default(),
            hex: None,
        };
        let stop = |id: u32, manual: u32, rank: u32, own_pipes: bool| Stop {
            id: StopId(id),
            name: format!("stop {id}"),
            manual: ManualId(manual),
            ranks: vec![RankRange {
                rank: RankId(rank),
                first_key: 0,
                key_count: 61,
                first_pipe: 0,
            }],
            own_pipes,
        };
        let pipe = |source: PipeSource| Pipe {
            nominal_frequency_hz: 440.0,
            pitch_tuning_cents: 0.0,
            pitch_correction_cents: 0.0,
            gain_db: 0.0,
            midi_key_number: None,
            midi_pitch_fraction_cents: None,
            accepts_retuning: true,
            source,
        };
        let organ = Organ {
            name: "B".into(),
            base_path: Default::default(),
            manuals: vec![manual(1, "Great"), manual(2, "Swell")],
            stops: vec![
                stop(1, 1, 1, false),
                stop(2, 2, 2, borrower_owns_pipes),
            ],
            ranks: vec![
                Rank {
                    id: RankId(1),
                    name: "donor".into(),
                    windchest: 1,
                    velocity_volume: Default::default(),
                    pipes: (0..61).map(|_| pipe(PipeSource::Silent)).collect(),
                },
                Rank {
                    id: RankId(2),
                    name: "borrower".into(),
                    windchest: 1,
                    velocity_volume: Default::default(),
                    pipes: (0..61u16)
                        .map(|at| {
                            pipe(PipeSource::Borrowed(aristide_model::PipeRef {
                                rank: RankId(1),
                                pipe: at,
                            }))
                        })
                        .collect(),
                },
            ],
            couplers: vec![],
            enclosures: vec![],
            windchests: vec![],
            tremulants: vec![],
        };
        let mut specs = HashMap::new();
        for rank in 1..=2u32 {
            for pipe in 0..61u16 {
                specs.insert(
                    (RankId(rank), pipe),
                    VoiceSpec {
                        sample: 0,
                        rate: 1.0,
                        gain: 1.0,
                        velocity: Default::default(),
                        percussive: false,
                        group: 0,
                        wind_weight: 1.0,
                        brightness: 0.0,
                        voicing_tilt: 1.0,
                        nominal_hz: 440.0,
                        home_cents: 0.0,
                        model_cents: 0.0,
                        enclosures: [aristide_engine::enclosure::ENCLOSURE_NONE;
                            aristide_engine::enclosure::MAX_VOICE_ENCLOSURES],
                        bus: 0,
                        delay_frames: 0,
                    },
                );
            }
        }
        Console::new(organ, specs, vec![StopId(1), StopId(2)], 48_000.0)
    }

    /// THE unit-organ rule: a rank borrowed onto another manual is the
    /// same pipes, not a copy of them. The same note demanded from
    /// both places speaks ONE pipe — starting a second voice would sum
    /// the identical recording coherently (+6 dB).
    #[test]
    fn borrowed_rank_shares_one_pipe_with_its_donor() {
        let mut console = borrowed_console(false);
        let (first, _) = console.note_on_manual(0, 60, 127);
        assert_eq!(first.len(), 1, "the donor stop speaks");
        let (second, _) = console.note_on_manual(1, 60, 127);
        assert!(
            second.is_empty(),
            "the borrowing stop holds the pipe already speaking, it \
             does not double it"
        );
        assert!(
            console.note_off_manual(0, 60).0.is_empty(),
            "the Swell key still holds the shared pipe"
        );
        assert_eq!(
            console.note_off_manual(1, 60).0.len(),
            1,
            "the last holder stops it"
        );
    }

    /// The same physical pipe REPITCHED two ways is not the same pipe:
    /// an out-of-range C# and D both stood in for by the top pipe must
    /// still sound as two, and neither merges with the donor's own
    /// at-pitch voice either.
    #[test]
    fn repitched_borrows_stay_separate_voices() {
        let mut console = borrowed_console(false);
        console.set_compass(1, 36, 101);

        let (at_pitch, _) = console.note_on_manual(0, 96, 127); // donor top pipe, as recorded
        assert_eq!(at_pitch.len(), 1);
        let (csharp, _) = console.note_on_manual(1, 101, 127); // same pipe +5 semitones
        assert_eq!(csharp.len(), 1, "another pitch is another virtual pipe");
        let (d, _) = console.note_on_manual(1, 100, 127); // same pipe +4 semitones
        assert_eq!(d.len(), 1, "and so is a third");
        let semis =
            |starts: &[VoiceStart]| starts[0].spec.rate.log2() * 12.0;
        assert!((semis(&csharp) - 5.0).abs() < 1e-3);
        assert!((semis(&d) - 4.0).abs() < 1e-3);
    }

    /// `own_pipes` on the borrowing stop is the opt-out: it speaks an
    /// independent (virtual) set of pipes and doubles the donor.
    #[test]
    fn an_own_pipes_stop_doubles_its_donor() {
        let mut console = borrowed_console(true);
        let (first, _) = console.note_on_manual(0, 60, 127);
        assert_eq!(first.len(), 1);
        let (second, _) = console.note_on_manual(1, 60, 127);
        assert_eq!(second.len(), 1, "the own-pipes stop starts its own voice");
        assert_eq!(console.note_off_manual(0, 60).0.len(), 1);
        assert_eq!(console.note_off_manual(1, 60).0.len(), 1);
    }

    /// Toggling a stop's pipe sharing lands on held keys at once, like
    /// every other console change: the opt-out splits a shared pipe
    /// into a second voice, opting back in folds it home.
    #[test]
    fn stop_own_pipes_toggles_live_under_held_keys() {
        let mut console = borrowed_console(false);
        console.note_on_manual(0, 60, 127);
        assert!(console.note_on_manual(1, 60, 127).0.is_empty());
        let (stopped, started) = console.set_stop_own_pipes(StopId(2), true);
        assert!(stopped.is_empty(), "the donor still holds the shared pipe");
        assert_eq!(started.len(), 1, "the stop's own copy starts");
        let (stopped, started) = console.set_stop_own_pipes(StopId(2), false);
        assert_eq!(stopped.len(), 1, "the copy folds back into the shared pipe");
        assert!(started.is_empty());
    }

    /// `own_pipes` on a coupler route: its copies stop merging with
    /// direct presses — including a unison self-coupler, which becomes
    /// a deliberate doubler instead of a no-op.
    #[test]
    fn an_own_pipes_coupler_route_doubles() {
        let mut console = coupled_console();
        console.organ.couplers.push(aristide_model::Coupler {
            name: "I/I doubler".into(),
            routes: vec![aristide_model::CouplerRoute {
                from_manual: ManualId(1),
                low_key: None,
                high_key: None,
                unison_off: false,
                scope: Default::default(),
                target: Some(aristide_model::CouplerTarget {
                    manual: ManualId(1),
                    key_shift: 0,
                    repitch: None,
                    own_pipes: true,
                }),
            }],
        });
        let index = console.organ.couplers.len() - 1;

        let (starts, _) = console.note_on_manual(0, 60, 127);
        assert_eq!(starts.len(), 1, "without the coupler, one voice");
        console.note_off_manual(0, 60);

        console.set_coupler(index, true);
        let (starts, _) = console.note_on_manual(0, 60, 127);
        assert_eq!(
            starts.len(),
            2,
            "the own-pipes unison copy doubles the played key"
        );
        assert_eq!(console.note_off_manual(0, 60).0.len(), 2);
    }

    #[test]
    fn re_pressed_pipe_expedites_its_staggered_predecessor() {
        // Press a chord, release it (voices enter the pallet-stagger
        // window at full level), then instantly re-press a key that
        // reaches one of those pipes: the new press must carry the old
        // voice's handle so the engine releases it NOW — otherwise the
        // pipe overlaps itself ("you hear the opening of another note").
        let mut console = coupled_console();
        console.set_coupler(1, true); // 16' I self-coupler

        let (starts, _) = console.note_on_manual(0, 72, 127); // pipes 36 and 24
        let shared_pipe_voice = starts
            .iter()
            .map(|s| s.handle)
            .max()
            .expect("voices started");
        let released = console.note_off_manual(0, 72).0;
        assert_eq!(released.len(), 2);

        // Immediately press 60 → its direct pipe IS 72's coupled pipe.
        let (_, expedited) = console.note_on_manual(0, 60, 127);
        assert!(
            expedited.contains(&shared_pipe_voice)
                || released.contains(&shared_pipe_voice),
            "the shared pipe's previous voice must be expedited on re-press"
        );
        assert!(
            !expedited.is_empty(),
            "re-press within the stagger window must expedite predecessors"
        );
    }

    #[test]
    fn coupler_cycles_terminate() {
        let mut console = coupled_console();
        console.set_coupler(0, true); // II/I
        console.set_coupler(2, true); // I/II — cycle
        // I→II and II→I at unison collapse to the same two notes.
        assert_eq!(console.note_on_manual(0, 60, 127).0.len(), 2);
        console.note_off_manual(0, 60);
    }

    #[test]
    fn retrigger_stops_previous_voices_first() {
        // A re-press before note-off (key bounce, fast repetition) must
        // release the first press's voices — a pipe can't speak twice,
        // and doubling correlated audio is an instant +6 dB.
        let mut console = test_console();
        let (first, retriggered) = console.note_on_manual(0, 60, 127);
        assert_eq!(first.len(), 2);
        assert!(retriggered.is_empty());
        let first_handles: Vec<u64> = first.iter().map(|s| s.handle).collect();

        let (second, retriggered) = console.note_on_manual(0, 60, 127);
        assert_eq!(second.len(), 2);
        assert_eq!(retriggered, first_handles, "old voices released");

        // Note-off stops only the live (second) voices.
        let stopped = console.note_off_manual(0, 60).0;
        assert_eq!(
            stopped,
            second.iter().map(|s| s.handle).collect::<Vec<_>>()
        );
        assert!(console.note_off_manual(0, 60).0.is_empty());
    }
}
