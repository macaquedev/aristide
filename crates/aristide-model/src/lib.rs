//! The organ model: a format-neutral description of an instrument.
//!
//! Loaders in `aristide-formats` populate this; the engine renders it.
//! No 12-EDO assumptions: pitch identity lives in explicit key→pipe
//! mappings and per-pipe pitch metadata, never in note-number math.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

pub mod lumatone;
pub mod scala;
pub mod units;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ManualId(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct StopId(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RankId(pub u32);

/// What sort of keyboard a manual is. Declared, never deduced: loaders
/// set it from the format (GO's `Manual000` is the pedal), composites
/// declare it (`kind = "..."`). The kind is a console fact — how the
/// keyboard is drawn and where it sits — not a sounding one: pitch
/// still comes from the tuning/key-mapping layer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ManualKind {
    /// A hand keyboard with conventional piano geometry.
    #[default]
    Manual,
    /// Played by the feet: renders as the pedalboard, at the bottom of
    /// the console.
    Pedal,
    /// A generalized keyboard (Terpstra/Lumatone style): a hex-grid
    /// key field rather than naturals and sharps. Key numbers are
    /// still the manual's contiguous key range; what each key sounds
    /// is the key→pitch mapping layer's business.
    Microtonal,
}

impl ManualKind {
    /// The `kind = "..."` vocabulary of composite files, lowercase.
    pub fn as_str(self) -> &'static str {
        match self {
            ManualKind::Manual => "manual",
            ManualKind::Pedal => "pedal",
            ManualKind::Microtonal => "microtonal",
        }
    }

    /// Parse the composite-file vocabulary, case-insensitively.
    /// `None` for anything unrecognized — callers choose whether that
    /// warns or errors.
    pub fn parse(text: &str) -> Option<Self> {
        let text = text.trim();
        [ManualKind::Manual, ManualKind::Pedal, ManualKind::Microtonal]
            .into_iter()
            .find(|kind| kind.as_str().eq_ignore_ascii_case(text))
    }
}

/// The isomorphic layout of a microtonal manual's hex field. Like the
/// kind it belongs to, this is a console fact — which key number each
/// hex addresses — not a sounding one: pitch still comes from the
/// tuning layer, per key number.
///
/// The parameterization is the one every generalized keyboard since
/// Bosanquet shares (and the one the Terpstra web app and Lumatone
/// editor expose): two step-vectors over a hex grid. Moving one hex
/// right advances the key number by `right`; one hex up-right by
/// `upright`; the third axis, up-left, is their difference. Every
/// named layout — Bosanquet/Wilson, Wicki–Hayden, the harmonic
/// table — is a choice of that pair. Distinct hexes may land on the
/// same key number: that is isomorphic boards' duplicate notes, not
/// an error, and they sound (and light) as one key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct HexLayout {
    /// Hex rows, counted bottom-up (pitch conventionally rises upward).
    pub rows: u8,
    /// Hexes per row. Odd rows sit half a hex right of even ones, so
    /// the board reads as a staggered rectangle, Lumatone-fashion.
    pub cols: u8,
    /// Key-number step for one hex to the right.
    pub right: i16,
    /// Key-number step for one hex up and to the right.
    pub upright: i16,
    /// Key number of the bottom-left hex.
    pub anchor: u16,
}

impl HexLayout {
    pub const MAX_ROWS: u8 = 24;
    pub const MAX_COLS: u8 = 48;

    /// The key number a hex sounds: `col` counts hexes from the row's
    /// left edge, `row` from the bottom. Rows re-center every other
    /// row (the axial column is `col - row/2`), which is what keeps a
    /// staggered rectangle isomorphic instead of a leaning
    /// parallelogram.
    pub fn key_at(&self, col: u8, row: u8) -> i32 {
        let axial = col as i32 - (row as i32) / 2;
        self.anchor as i32 + axial * self.right as i32 + row as i32 * self.upright as i32
    }

    /// The key number a hex sounds on a *left-leaning* grid: rows that
    /// march half a cell left per row up with no re-centering — the
    /// physical geometry of QWERTY rows (Z above-left of A above-left
    /// of Q), unlike the on-screen board's staggered rectangle. `col`
    /// counts within the row, `row` from the bottom. One cell right
    /// still advances by `right` and the cell physically up-right by
    /// `upright`, so isomorphic shapes land under the fingers exactly
    /// as they lie on the board.
    pub fn key_at_slanted(&self, col: u8, row: u8) -> i32 {
        self.anchor as i32
            + (col as i32 - row as i32) * self.right as i32
            + row as i32 * self.upright as i32
    }

    /// The layout an undeclared microtonal manual gets: Bosanquet-style
    /// step-vectors (a two-step right, a one-step up-right — the
    /// Lumatone factory layout, in key numbers), five rows, and just
    /// enough columns to reach the top of the compass from the bottom.
    pub fn default_for(first_key: u16, key_count: u16) -> HexLayout {
        let mut layout = HexLayout {
            rows: 5,
            cols: 1,
            right: 2,
            upright: 1,
            anchor: first_key,
        };
        layout.fit_cols(first_key as i32 + key_count.max(1) as i32 - 1);
        layout
    }

    /// Widen the board until some hex reaches key `top` — how a layout
    /// with everything but its column count settled gets sized to a
    /// compass. Capped at [`MAX_COLS`](Self::MAX_COLS): step-vectors
    /// that never climb (both non-positive) simply get the full width.
    pub fn fit_cols(&mut self, top: i32) {
        self.cols = self.cols.max(1);
        while self.cols < Self::MAX_COLS && self.highest_key() < top {
            self.cols += 1;
        }
    }

    fn highest_key(&self) -> i32 {
        (0..self.rows)
            .map(|row| self.key_at(self.cols - 1, row))
            .max()
            .unwrap_or(self.anchor as i32)
    }

    /// The step-vector pair of a named layout, derived rather than
    /// tabulated: with `steps` tuning steps to the octave and `fifth`
    /// the nearest approximation of 3:2 among them, every classic
    /// layout is a combination of octaves and fifths. Bosanquet walks
    /// whole tones (two fifths less an octave) right and chromas
    /// (seven fifths less four octaves) up-right; Wicki–Hayden keeps
    /// the whole tone but climbs by whole fifths; the harmonic table
    /// pairs the major third (four fifths less two octaves) with the
    /// fifth, so triads sit in a cluster. In 12-EDO these come out
    /// (2,1), (2,7) and (4,7); in 31-EDO (5,2), (5,18) and (10,18).
    pub fn preset_steps(name: &str, steps: u16) -> Option<(i16, i16)> {
        let steps = steps.max(1) as f64;
        let fifth = (steps * 1.5f64.log2()).round() as i16;
        let octave = steps as i16;
        match name {
            "bosanquet" => Some((2 * fifth - octave, 7 * fifth - 4 * octave)),
            "wicki-hayden" => Some((2 * fifth - octave, fifth)),
            "harmonic-table" => Some((4 * fifth - 2 * octave, fifth)),
            _ => None,
        }
    }
}

/// A keyboard (or pedalboard). "Manual" is used inclusively, as GO does.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manual {
    pub id: ManualId,
    pub name: String,
    /// MIDI note number of the manual's lowest key (conventional default
    /// mapping; the input-mapping layer may override arbitrarily).
    pub first_midi_note: u8,
    pub key_count: u16,
    #[serde(default)]
    pub kind: ManualKind,
    /// A microtonal manual's declared hex-field layout; `None` leaves
    /// the console to derive [`HexLayout::default_for`]. Meaningless
    /// (and ignored) on the other kinds.
    #[serde(default)]
    pub hex: Option<HexLayout>,
}

impl Manual {
    pub fn pedal(&self) -> bool {
        self.kind == ManualKind::Pedal
    }
}

/// A sustain loop within a sample, in frames.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SampleLoop {
    pub start: u64,
    pub end: u64,
}

/// One recorded attack/sustain sample of a pipe.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AttackSample {
    pub path: PathBuf,
    pub loops: Vec<SampleLoop>,
    /// Recorded pitch in cents relative to the pipe's nominal pitch
    /// (0 = in tune as recorded).
    pub pitch_offset_cents: f64,
    /// GO `IsTremulant` tri-state: `Some(true)` = candidate only while
    /// a wave tremulant on the pipe's chest is engaged, `Some(false)` =
    /// only while it is not, `None` = either. Wave-tremmed sets record
    /// each pipe twice and switch recordings instead of modulating.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wave_tremulant: Option<bool>,
    /// Lowest MIDI velocity this attack answers to (GO
    /// `AttackVelocity`). Selection prefers the highest qualifying
    /// bound — the most specific match for the press.
    #[serde(default, skip_serializing_if = "is_zero_u8")]
    pub min_velocity: u8,
    /// Candidate only when the pipe re-speaks within this many ms of
    /// its previous release (GO `MaxTimeSinceLastRelease`): the
    /// fast-repetition re-attack of a pipe still speaking down.
    /// `None` = always a candidate.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_time_since_last_release_ms: Option<u32>,
    /// Crossfade to bake across each sustain-loop seam, in
    /// milliseconds (GO `LoopCrossfadeLength`, 0–3000; 0 = butt loop
    /// as recorded). Sets whose loop points don't quite line up depend
    /// on this to keep the loop from thumping every pass.
    #[serde(default, skip_serializing_if = "is_zero_u16")]
    pub loop_crossfade_ms: u16,
    /// Frame where playback begins (GO `AttackStart`; 0 = the file's
    /// start). Producers use it to skip lead-in silence or noise.
    #[serde(default, skip_serializing_if = "is_zero_u32")]
    pub attack_start_frame: u32,
    /// Frame where this attack's embedded release tail begins (GO
    /// `CuePoint`; `None` = trust the file's own `cue` chunk).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cue_point_frame: Option<u32>,
    /// Frame where the embedded release tail ends (GO `ReleaseEnd`;
    /// `None` = end of file). Material past it is the producer saying
    /// "don't play this".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub release_end_frame: Option<u32>,
    /// Producer-tuned key-off crossfade in milliseconds (GO
    /// `ReleaseCrossfadeLength`; 0 = the engine's pitch-scaled fade).
    #[serde(default, skip_serializing_if = "is_zero_u16")]
    pub release_crossfade_ms: u16,
}

fn is_zero_u16(value: &u16) -> bool {
    *value == 0
}

fn is_zero_u32(value: &u32) -> bool {
    *value == 0
}

fn is_zero_u8(value: &u8) -> bool {
    *value == 0
}

/// One recorded release tail, selected by how long the note was held.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ReleaseSample {
    pub path: PathBuf,
    /// Only used when the note was held at most this long (ms);
    /// `None` = the default/longest release.
    pub max_key_press_ms: Option<u32>,
    /// GO `IsTremulant` tri-state, as on [`AttackSample`]: which
    /// wave-tremulant state this release was recorded under.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wave_tremulant: Option<bool>,
    /// Frame where the release proper begins within the file (GO
    /// `CuePoint`; `None` = the file's start) — lead-in is skipped.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cue_point_frame: Option<u32>,
    /// Frame where the release ends (GO `ReleaseEnd`; `None` = end of
    /// file) — the rest is trimmed at load.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub release_end_frame: Option<u32>,
    /// Producer-tuned key-off crossfade in milliseconds (GO
    /// `ReleaseCrossfadeLength`; 0 = the engine's pitch-scaled fade).
    #[serde(default, skip_serializing_if = "is_zero_u16")]
    pub release_crossfade_ms: u16,
}

/// Address of one pipe within an [`Organ`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PipeRef {
    pub rank: RankId,
    /// Index into the rank's `pipes`.
    pub pipe: u16,
}

/// Where a pipe's sound comes from.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PipeSource {
    /// The pipe owns recorded samples.
    Sampled {
        attacks: Vec<AttackSample>,
        releases: Vec<ReleaseSample>,
    },
    /// Unit-organ borrowing: sounding this pipe sounds another pipe.
    /// The target may itself be borrowed; consumers follow the chain
    /// (loaders guarantee it is acyclic).
    Borrowed(PipeRef),
    /// A placeholder occupying a key slot but never sounding.
    Silent,
}

/// A single pipe: the atomic sounding unit. Everything in Aristide is
/// ultimately addressed per-pipe (tuning, voicing, effects, routing).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Pipe {
    /// Nominal pitch this pipe sounds at concert tuning, before
    /// temperament/retuning layers. Loaders fold the rank's harmonic
    /// in: a 4′ rank's pipe under key C4 has a C5 nominal, a 2⅔′
    /// mutation the twelfth — this is the true sounding pitch, and
    /// wind draw, brightness and release alignment all key off it.
    pub nominal_frequency_hz: f64,
    /// Set-author tuning correction in cents (already combined across
    /// the organ→windchest→rank→pipe inheritance chain by the loader).
    /// Applied when the sample is played as recorded (GO `PitchTuning`).
    pub pitch_tuning_cents: f64,
    /// Set-author correction in cents for the *retuned* path only:
    /// when playback is reconciled from recorded-pitch metadata, this
    /// replaces `pitch_tuning_cents` (GO `PitchCorrection`, likewise
    /// chain-combined). A baroque-pitch set declares "yes, I really am
    /// a semitone flat — keep me there" through this key.
    #[serde(default)]
    pub pitch_correction_cents: f64,
    /// Total gain in dB (amplitude chain folded in by the loader).
    pub gain_db: f64,
    /// Explicit MIDI key of the recording, overriding the sample file's
    /// own `smpl`-chunk unity note when present.
    pub midi_key_number: Option<u8>,
    /// Explicit recorded-pitch fraction in cents above
    /// `midi_key_number` (GO `MIDIPitchFraction`, 0–100), overriding
    /// the sample's own `smpl` fraction. When `midi_key_number` is
    /// given without this, the fraction is 0 — an explicit ODF key
    /// silences the file's fraction too (GO's rule).
    #[serde(default)]
    pub midi_pitch_fraction_cents: Option<f64>,
    /// Whether pitch metadata may retune this pipe away from how it
    /// was voiced (GO `AcceptsRetuning`, rank default folded in by the
    /// loader). False = play as recorded, whatever the metadata claims
    /// — chiffs, percussions and effects opt out this way.
    #[serde(default = "default_true", skip_serializing_if = "is_true")]
    pub accepts_retuning: bool,
    pub source: PipeSource,
}

fn default_true() -> bool {
    true
}

fn is_true(value: &bool) -> bool {
    *value
}

fn is_false(value: &bool) -> bool {
    !*value
}

impl Pipe {
    /// The pipe's own samples, if it has any (borrowed/silent pipes don't).
    pub fn samples(&self) -> Option<(&[AttackSample], &[ReleaseSample])> {
        match &self.source {
            PipeSource::Sampled { attacks, releases } => Some((attacks, releases)),
            _ => None,
        }
    }
}

/// Key-velocity→volume ramp (GO `MinVelocityVolume`/`MaxVelocityVolume`
/// ÷ 100): a linear gain multiplier running from `at_zero` at MIDI
/// velocity 0 to `at_full` at velocity 127. The default (1.0 at both
/// ends) is velocity-insensitive — what a pipe organ's pallet valve is.
/// Tracker-touch sets declare a slope to let key speed shade the tone.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct VelocityVolume {
    pub at_zero: f64,
    pub at_full: f64,
}

impl Default for VelocityVolume {
    fn default() -> Self {
        VelocityVolume {
            at_zero: 1.0,
            at_full: 1.0,
        }
    }
}

impl VelocityVolume {
    /// The gain multiplier a press at `velocity` earns.
    pub fn gain(&self, velocity: u8) -> f32 {
        let along = f64::from(velocity.min(127)) / 127.0;
        (self.at_zero + (self.at_full - self.at_zero) * along) as f32
    }
}

/// A rank: one row of pipes of common construction (e.g. "Principal 8'").
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Rank {
    pub id: RankId,
    pub name: String,
    /// Which wind supply this rank speaks on (1-based, format-side
    /// numbering; GO `WindchestGroup`). Drives the wind model.
    pub windchest: u32,
    /// How key velocity shades this rank's volume; identity by default.
    #[serde(default, skip_serializing_if = "is_default_velocity")]
    pub velocity_volume: VelocityVolume,
    pub pipes: Vec<Pipe>,
}

fn is_default_velocity(v: &VelocityVolume) -> bool {
    *v == VelocityVolume::default()
}

/// Maps a contiguous run of a manual's keys onto a rank's pipes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RankRange {
    pub rank: RankId,
    /// Index of the first manual key this range covers.
    pub first_key: u16,
    pub key_count: u16,
    /// Index into the rank's pipes for `first_key`.
    pub first_pipe: u16,
}

/// A stop: a drawable voice on a manual, sounding one or more ranks.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Stop {
    pub id: StopId,
    pub name: String,
    pub manual: ManualId,
    pub ranks: Vec<RankRange>,
    /// Whether this stop speaks pipes of its own instead of sharing.
    /// By default two stops that reach the same physical pipe at the
    /// same pitch (unit-organ borrowing, duplexed ranks) hold ONE
    /// voice between them, as the real action would. `true` gives the
    /// stop an independent (virtual) set of pipes, so it doubles what
    /// other stops already sound.
    #[serde(default, skip_serializing_if = "is_false")]
    pub own_pipes: bool,
}

/// A swell box / expression enclosure: a shuttered chamber whose
/// shutter position attenuates and filters every pipe on its member
/// windchests. Membership lives on [`Windchest`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Enclosure {
    pub name: String,
    /// Linear amplitude percentage when fully closed (GO
    /// `AmpMinimumLevel`, 0–100). The loader passes the set's value
    /// through; taper law and filtering are engine policy.
    pub amp_minimum_level: f64,
    /// Producer's suggested expression-controller ordering (GO
    /// `MIDIInputNumber`); 0/absent = none.
    pub midi_input_number: Option<u16>,
    /// Whether the set shows this enclosure on its console.
    pub displayed: bool,
}

/// A windchest group: the unit ranks reference for wind supply and
/// enclosure membership (GO `[WindchestGroupNNN]`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Windchest {
    /// 1-based format-side number, as referenced by [`Rank::windchest`].
    pub number: u32,
    pub name: String,
    /// Indices into [`Organ::enclosures`] this chest sits inside.
    pub enclosures: Vec<u32>,
    /// Indices into [`Organ::tremulants`] that modulate this chest.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tremulants: Vec<u32>,
}

/// How a tremulant makes its undulation.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TremulantKind {
    /// Synthesized modulation of whatever already sounds on the member
    /// chests (GO `Synth`). GO applies block-rate amplitude only; our
    /// engine renders it as pressure modulation (FM+AM+brightness), with
    /// the author's depth honoured on the amplitude leg.
    Synth {
        /// One full cycle in milliseconds (GO `Period`; 196 → ~5.1 Hz).
        period_ms: f64,
        /// Peak amplitude modulation in percent (GO `AmpModDepth`).
        amp_mod_depth_percent: f64,
        /// Engage ramp: GO synthesizes a `1/start_rate`-second fade-in
        /// (GOSoundProviderSynthedTrem::Create), so 1–100 maps to
        /// 1 s – 10 ms. Disengage likewise via `stop_rate`.
        start_rate: u32,
        stop_rate: u32,
    },
    /// Sample-switching tremulant (GO `Wave`): pipes on the member
    /// chests carry `wave_tremulant`-marked attack/release variants,
    /// and engaging the tremulant prefers those recordings.
    Wave,
}

/// A tremulant as the set defines it (GO `[TremulantNNN]`): a console
/// control undulating every pipe on its member windchests. Membership
/// lives on [`Windchest::tremulants`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tremulant {
    pub name: String,
    pub kind: TremulantKind,
}

/// Where a coupler route sends its copies.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CouplerTarget {
    pub manual: ManualId,
    /// Keyboard-relative shift in keys (sub/super octave = ∓12 at
    /// 12-EDO, but stored as a key delta since shift semantics are
    /// keyboard-relative, never frequency math).
    pub key_shift: i16,
    /// Whether the copy may repitch a neighbouring pipe to sound a
    /// pitch the destination hasn't got — which also lets it land past
    /// the destination's compass, since the whole point of such a
    /// route is tone the instrument can't otherwise make (a 16' from
    /// an 8' rank's bottom octave). `None` = the console-wide default.
    pub repitch: Option<bool>,
    /// Whether this route's copies speak pipes of their own instead of
    /// sharing. By default a copy that reaches a pipe already speaking
    /// at the same pitch merely holds it — one pipe, one voice, as a
    /// unit organ's action works. `true` gives the route an independent
    /// (virtual) set of pipes, so its copies double what other routes
    /// or keys already sound.
    #[serde(default, skip_serializing_if = "is_false")]
    pub own_pipes: bool,
}

/// Which source keys a route listens to. The classic coupler hears
/// them all; the "intelligent" Bass and Melody couplers hear only the
/// extreme of the keys *currently held* — an automatic pedal under the
/// lowest note of a chord, a solo stop singing its highest. Which key
/// is extreme changes as keys go down and up, so these routes retarget
/// live rather than routing each press independently.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CouplerScope {
    /// Every key the route's range covers (the classic coupler).
    #[default]
    AllKeys,
    /// Only the lowest currently-held key in range (GO `Bass`).
    Bass,
    /// Only the highest currently-held key in range (GO `Melody`).
    Melody,
}

/// One rule inside a coupler: for source keys in a range, optionally
/// silence the key's own division and/or send a shifted copy somewhere.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CouplerRoute {
    pub from_manual: ManualId,
    /// Inclusive MIDI-note bounds on the source keys this route acts
    /// on; `None` = unbounded on that side. A fourths coupler "from
    /// tenor C" is `low_key: Some(48)`.
    pub low_key: Option<u8>,
    pub high_key: Option<u8>,
    /// Silence the played key's own division inside the range, so the
    /// note *moves* instead of doubling (GO's `UnisonOff` is this over
    /// the whole compass, with no target).
    pub unison_off: bool,
    /// Which keys in range the route hears: all of them, or only the
    /// lowest/highest currently held.
    #[serde(default)]
    pub scope: CouplerScope,
    /// The coupled copy; `None` for a pure unison-off route.
    pub target: Option<CouplerTarget>,
}

impl CouplerRoute {
    pub fn covers(&self, midi_key: i16) -> bool {
        self.low_key.is_none_or(|low| midi_key >= low as i16)
            && self.high_key.is_none_or(|high| midi_key <= high as i16)
    }
}

/// A coupler: one engageable console control bundling any number of
/// routes. The classic couplers are single full-compass routes; ranges,
/// unison-off and per-route repitch make the flexible ones ("fourths
/// from tenor C", "16' that transposes the bottom octave") expressible
/// in the same vocabulary.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Coupler {
    pub name: String,
    pub routes: Vec<CouplerRoute>,
}

impl Coupler {
    /// The classic organ coupler: every key of `from`, shifted onto `to`.
    pub fn simple(
        name: impl Into<String>,
        from: ManualId,
        to: ManualId,
        key_shift: i16,
    ) -> Coupler {
        Coupler {
            name: name.into(),
            routes: vec![CouplerRoute {
                from_manual: from,
                low_key: None,
                high_key: None,
                unison_off: false,
                scope: CouplerScope::default(),
                target: Some(CouplerTarget {
                    manual: to,
                    key_shift,
                    repitch: None,
                    own_pipes: false,
                }),
            }],
        }
    }

    /// Whether any route carries notes from `from` onto `to` — what
    /// "the II/I couplers" means when scanning a set's coupler list.
    pub fn couples(&self, from: ManualId, to: ManualId) -> bool {
        self.routes.iter().any(|route| {
            route.from_manual == from
                && route.target.as_ref().is_some_and(|t| t.manual == to)
        })
    }
}

/// How far a divisional piston reaches on this console.
///
/// A divisional always sets the stops of its own division — that is
/// what makes it a divisional. Whether it *also* moves that division's
/// couplers and tremulants is a wiring choice, and consoles genuinely
/// differ: on many instruments the Swell divisionals leave "Swell to
/// Great" exactly where the hand left it, on others they take it with
/// them. GrandOrgue states the answer in the ODF's `[Organ]` header —
/// `DivisionalsStoreIntermanualCouplers`,
/// `DivisionalsStoreIntramanualCouplers`, `DivisionalsStoreTremulants`
/// (docs/go-odf-notes.md; applied in GO's `GOCoupler.cpp:259-264` and
/// `GOTremulant.cpp:79-83`) — and GO's own defaults are all `false`
/// (`GOOrganModel.cpp:44-47`), so a set that says nothing gets a
/// stops-only divisional, which is ours too.
///
/// Inter- versus intramanual is GO's distinction and it is the useful
/// one: a coupler is intermanual when it carries keys onto a
/// *different* manual (`GOCoupler::IsIntermanual` — source manual ≠
/// destination), intramanual when it stays on its own (octave
/// couplers, unison off).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CombinationScope {
    /// Divisionals also store couplers reaching another manual.
    #[serde(default)]
    pub divisional_intermanual_couplers: bool,
    /// Divisionals also store the division's own octave/unison-off
    /// couplers.
    #[serde(default)]
    pub divisional_intramanual_couplers: bool,
    /// Divisionals also store the division's tremulants.
    #[serde(default)]
    pub divisional_tremulants: bool,
}

/// A complete instrument, format-neutral.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Organ {
    pub name: String,
    /// Directory the sample paths are relative to.
    pub base_path: PathBuf,
    pub manuals: Vec<Manual>,
    pub stops: Vec<Stop>,
    pub ranks: Vec<Rank>,
    pub couplers: Vec<Coupler>,
    pub enclosures: Vec<Enclosure>,
    pub windchests: Vec<Windchest>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tremulants: Vec<Tremulant>,
    /// How far this console's divisional pistons reach.
    #[serde(default)]
    pub combinations: CombinationScope,
}

impl Organ {
    pub fn rank(&self, id: RankId) -> Option<&Rank> {
        self.ranks.iter().find(|r| r.id == id)
    }

    pub fn pipe(&self, at: PipeRef) -> Option<&Pipe> {
        self.rank(at.rank)?.pipes.get(at.pipe as usize)
    }

    /// Follow a borrow chain to the pipe that actually sounds.
    /// Returns `None` for dangling references (loaders prevent cycles).
    pub fn sounding_pipe(&self, at: PipeRef) -> Option<&Pipe> {
        let mut hops = self.ranks.iter().map(|r| r.pipes.len()).sum::<usize>() + 1;
        let mut pipe = self.pipe(at)?;
        while let PipeSource::Borrowed(target) = &pipe.source {
            pipe = self.pipe(*target)?;
            hops = hops.checked_sub(1)?;
        }
        Some(pipe)
    }
}

#[cfg(test)]
mod hex_layout_tests {
    use super::HexLayout;

    #[test]
    fn presets_derive_from_the_fifth() {
        assert_eq!(HexLayout::preset_steps("bosanquet", 12), Some((2, 1)));
        assert_eq!(HexLayout::preset_steps("wicki-hayden", 12), Some((2, 7)));
        assert_eq!(HexLayout::preset_steps("harmonic-table", 12), Some((4, 7)));
        assert_eq!(HexLayout::preset_steps("bosanquet", 31), Some((5, 2)));
        assert_eq!(HexLayout::preset_steps("wicki-hayden", 31), Some((5, 18)));
        assert_eq!(HexLayout::preset_steps("harmonic-table", 31), Some((10, 18)));
        assert_eq!(HexLayout::preset_steps("bosanquet", 19), Some((3, 1)));
        assert_eq!(HexLayout::preset_steps("qwerty", 12), None);
    }

    #[test]
    fn keys_walk_the_two_step_vectors() {
        let layout = HexLayout { rows: 5, cols: 8, right: 2, upright: 1, anchor: 36 };
        assert_eq!(layout.key_at(0, 0), 36);
        assert_eq!(layout.key_at(1, 0), 38);
        assert_eq!(layout.key_at(0, 1), 37); // up-right from the anchor
        // Row 2 re-centers left by one axial column: same key as row 0's
        // start plus two uprights minus one right — a duplicate note.
        assert_eq!(layout.key_at(0, 2), 36);
    }

    /// The slanted (QWERTY-shaped) reading: no re-centering, each row
    /// up starts a NW step (upright − right) from the row below, and
    /// the cell physically up-right of a key is exactly +upright.
    #[test]
    fn slanted_keys_follow_the_physical_stagger() {
        let layout = HexLayout { rows: 5, cols: 8, right: 2, upright: 1, anchor: 36 };
        assert_eq!(layout.key_at_slanted(0, 0), 36); // Z
        assert_eq!(layout.key_at_slanted(1, 1), 37, "S, up-right of Z, is +upright");
        assert_eq!(layout.key_at_slanted(0, 1), 35, "A, up-left of Z, is upright − right");
        assert_eq!(layout.key_at_slanted(1, 2), 36, "W, straight above Z, duplicates it");
        // In 31-EDO Bosanquet the same shapes carry: +2 up-right, −3 up-left.
        let wide = HexLayout { rows: 5, cols: 8, right: 5, upright: 2, anchor: 48 };
        assert_eq!(wide.key_at_slanted(1, 1) - wide.key_at_slanted(0, 0), 2);
        assert_eq!(wide.key_at_slanted(0, 1) - wide.key_at_slanted(0, 0), -3);
    }

    #[test]
    fn default_covers_the_compass() {
        let layout = HexLayout::default_for(36, 61);
        assert_eq!((layout.right, layout.upright, layout.anchor), (2, 1, 36));
        assert!(layout.highest_key() >= 96);
        assert!(layout.cols <= HexLayout::MAX_COLS);
        // A one-key manual still gets a board, just a narrow one.
        assert_eq!(HexLayout::default_for(60, 1).cols, 1);
    }
}
