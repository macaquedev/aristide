//! The organ model: a format-neutral description of an instrument.
//!
//! Loaders in `aristide-formats` populate this; the engine renders it.
//! No 12-EDO assumptions: pitch identity lives in explicit key→pipe
//! mappings and per-pipe pitch metadata, never in note-number math.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

pub mod lumatone;
pub mod scala;

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
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttackSample {
    pub path: PathBuf,
    pub loops: Vec<SampleLoop>,
    /// Recorded pitch in cents relative to the pipe's nominal pitch
    /// (0 = in tune as recorded).
    pub pitch_offset_cents: f64,
}

/// One recorded release tail, selected by how long the note was held.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReleaseSample {
    pub path: PathBuf,
    /// Only used when the note was held at most this long (ms);
    /// `None` = the default/longest release.
    pub max_key_press_ms: Option<u32>,
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
    pub source: PipeSource,
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
                target: Some(CouplerTarget {
                    manual: to,
                    key_shift,
                    repitch: None,
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
