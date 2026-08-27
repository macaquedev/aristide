//! Aristide sidecar files: instrument-specific configuration in a TOML
//! file next to the sample set, never modifying the set itself.
//!
//! `<set>.aristide.toml` beside `<set>` (e.g. `demo.organ.aristide.toml`)
//! is the first sidecar layer: startup registration and MIDI channel
//! mapping. Voicing, tuning, routing, and effects land here as their
//! engine features arrive (DESIGN.md "Sidecar files").

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum SidecarError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("TOML error: {0}")]
    Toml(#[from] toml::de::Error),
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Sidecar {
    /// What to call this organ instead of the name the set declares —
    /// how a set is renamed without touching it. Applies when the set
    /// is loaded as the instrument itself; inside a composite the
    /// composite's own name stands. Empty means the set's own name.
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub midi: Midi,
    #[serde(default)]
    pub registration: Registration,
    #[serde(default)]
    pub wind: Wind,
    /// A hand-declared tremulant. `None` (section absent) lets the
    /// set's own `[Tremulant]` definitions stand; writing the section
    /// replaces them with this single instrument-wide one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tremulant: Option<Tremulant>,
    #[serde(default)]
    pub samples: SamplesConfig,
    #[serde(default)]
    pub tuning: TuningConfig,
    #[serde(default)]
    pub reverb: ReverbConfig,
    #[serde(default)]
    pub noises: NoisesConfig,
    #[serde(default)]
    pub enclosures: EnclosuresConfig,
    #[serde(default)]
    pub couplers: CouplersConfig,
    #[serde(default)]
    pub routing: RoutingConfig,
    #[serde(default)]
    pub voicing: VoicingConfig,
}

/// Audio routing: which stops render onto which output bus, where each
/// bus lands on the interface, and the bus's insert effects. Everything
/// not named stays on the main bus (the first output pair) — a stereo
/// rig never writes this table.
///
/// ```toml
/// [[routing.bus]]
/// name = "chamade"
/// stops = ["Trompette en chamade*"]
/// output = [3, 4]          # 1-based interface channels
/// gain_db = -3.0
/// [routing.bus.delay]      # optional insert: displace or echo
/// ms = 120
/// mix = 1.0
/// dry = 0.0                # dry 0 = the division moves in time
/// ```
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoutingConfig {
    #[serde(default, rename = "bus")]
    pub buses: Vec<BusDef>,
}

/// One routed bus: name patterns pick its members (stops directly, or
/// every stop of named manuals), `output` the 1-based interface channel
/// pair it lands on (omit for the main pair).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BusDef {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub stops: Vec<String>,
    #[serde(default)]
    pub manuals: Vec<String>,
    pub output: Option<[u8; 2]>,
    #[serde(default)]
    pub gain_db: f64,
    pub delay: Option<BusDelayDef>,
}

/// A bus's delay insert. `mix` is the wet level, `dry` the undelayed
/// level (1 = echo on top; 0 = the sound itself arrives late), and
/// `feedback` recirculates for repeating echoes.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BusDelayDef {
    pub ms: f64,
    #[serde(default)]
    pub feedback: f64,
    #[serde(default = "one")]
    pub mix: f64,
    #[serde(default = "one")]
    pub dry: f64,
}

fn one() -> f64 {
    1.0
}

/// Per-pipe voicing adjustments. First resident: speaking delays.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VoicingConfig {
    /// Onset (tracker/speaking) delays by stop pattern: every pipe the
    /// stop sounds waits `ms` before speaking — long mechanical runs,
    /// distant chests, or a canon trick per division.
    ///
    /// ```toml
    /// [[voicing.delay]]
    /// stops = ["Montre*"]
    /// ms = 12.5
    /// ```
    #[serde(default, rename = "delay")]
    pub delays: Vec<OnsetDelayDef>,
    /// Level and fine-tuning adjustments by stop pattern — the fix for
    /// one honking stop or a division out of balance, without touching
    /// the set:
    ///
    /// ```toml
    /// [[voicing.adjust]]
    /// stops = ["Trompette*"]
    /// gain_db = -2.5
    /// cents = 1.5
    /// ```
    #[serde(default, rename = "adjust")]
    pub adjusts: Vec<VoicingAdjustDef>,
}

/// One onset-delay rule.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OnsetDelayDef {
    pub stops: Vec<String>,
    pub ms: f64,
}

/// One level/tuning voicing rule. Cents ride the same pitch math as
/// tuning (wind draw and brightness follow the sounding pitch).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VoicingAdjustDef {
    pub stops: Vec<String>,
    #[serde(default)]
    pub gain_db: f64,
    #[serde(default)]
    pub cents: f64,
}

/// How couplers behave at the edges of a division, and any couplers the
/// user defines on top of the set's own.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CouplersConfig {
    /// Whether a coupled note may be repitched from a neighbouring pipe
    /// when the division it lands on hasn't got that one — a 16'
    /// running off the bottom of a rank, or a coupler into a shorter
    /// compass.
    ///
    /// Off, and this is the important default. Repitching exists to
    /// cover the gap between a sample set's compass and the player's
    /// keyboard; a coupler is not a keyboard, and letting it invent
    /// pipes would change the instrument rather than reach it. Sets
    /// built for the other behaviour (or a piece that wants it) can
    /// turn it on — or a single defined route can, below.
    #[serde(default)]
    pub repitch: bool,
    /// Couplers of the user's own devising, appended to the set's and
    /// engageable like any other. Each is a named bundle of routes:
    ///
    /// ```toml
    /// [[couplers.define]]
    /// name = "Fourths II/I"
    /// [[couplers.define.route]]
    /// from = "II"
    /// to = "I"
    /// shift = -5
    /// low = "C3"          # tenor C; a MIDI number works too
    /// ```
    ///
    /// A coupler may carry several routes (a 16' that transposes the
    /// bottom octave instead of doubling it is two), and a route may
    /// set `unison_off`, `repitch`, and `scope` (`"bass"`/`"melody"`
    /// for the intelligent couplers that follow only the lowest/highest
    /// held key).
    #[serde(default, rename = "define")]
    pub define: Vec<CouplerDef>,
    /// Couplers taken off the console, by name pattern — the set (or a
    /// combination) provides them, this instrument doesn't show them.
    /// They stay restorable from the console's Organ preferences; this
    /// never edits the loaded set itself.
    #[serde(default)]
    pub drop: Vec<String>,
}

/// One user-defined coupler: a name for the console rocker and the
/// routes it engages.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CouplerDef {
    pub name: String,
    #[serde(default, rename = "route")]
    pub routes: Vec<RouteDef>,
}

/// One route of a user-defined coupler. Manuals are named with the same
/// pattern rules as `[midi] channels`; keys are MIDI numbers or note
/// names ("C3", "F#2", "Bb1" — middle C is C4 = 60, so tenor C = "C3").
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RouteDef {
    /// Manual the route listens on.
    pub from: String,
    /// Destination manual; omit for a pure unison-off route.
    pub to: Option<String>,
    /// Keys added to the source key: -12 = sub-octave, -5 = a fourth
    /// down, 0 = unison.
    #[serde(default)]
    pub shift: i16,
    /// Inclusive bounds on the source keys the route acts on; omit for
    /// the whole compass.
    pub low: Option<KeySpec>,
    pub high: Option<KeySpec>,
    /// Silence the source keys' own division in this range, so the
    /// note moves instead of doubling.
    #[serde(default)]
    pub unison_off: bool,
    /// `"all-keys"` (the default), or `"bass"`/`"melody"`: couple only
    /// the lowest/highest currently-held key in the route's range.
    #[serde(default)]
    pub scope: aristide_model::CouplerScope,
    /// Let this route repitch pipes the destination hasn't got (and so
    /// reach past its compass); omit to follow `[couplers] repitch`.
    pub repitch: Option<bool>,
}

/// A key in a route definition: a raw MIDI note number or a name.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum KeySpec {
    Number(i64),
    Name(String),
}

impl KeySpec {
    pub fn midi_note(&self) -> Option<u8> {
        match self {
            KeySpec::Number(n) => u8::try_from(*n).ok().filter(|&n| n <= 127),
            KeySpec::Name(name) => parse_note_name(name),
        }
    }
}

/// "C4" → 60 (middle C), scientific pitch notation, any number of
/// `#`/`b` accidentals, octaves -1..9.
pub fn parse_note_name(name: &str) -> Option<u8> {
    let name = name.trim();
    let mut chars = name.chars();
    let letter = chars.next()?.to_ascii_uppercase();
    let semitone: i32 = match letter {
        'C' => 0,
        'D' => 2,
        'E' => 4,
        'F' => 5,
        'G' => 7,
        'A' => 9,
        'B' => 11,
        _ => return None,
    };
    let rest = chars.as_str();
    let octave_at = rest
        .find(|c: char| c.is_ascii_digit() || c == '-' || c == '+')
        .unwrap_or(rest.len());
    let (accidentals, octave) = rest.split_at(octave_at);
    let mut offset = 0i32;
    for c in accidentals.chars() {
        match c {
            '#' | '♯' => offset += 1,
            'b' | '♭' => offset -= 1,
            _ => return None,
        }
    }
    let octave: i32 = octave.parse().ok()?;
    u8::try_from((octave + 1) * 12 + semitone + offset)
        .ok()
        .filter(|&n| n <= 127)
}

/// Resolve user-defined couplers against a loaded organ's manuals.
/// Definitions that name what the organ hasn't got (or keys that don't
/// parse) are reported and skipped, never fatal — the same rule
/// bindings follow.
pub fn resolve_couplers(
    organ: &aristide_model::Organ,
    defs: &[CouplerDef],
) -> (Vec<aristide_model::Coupler>, Vec<String>) {
    let names: Vec<&str> = organ.manuals.iter().map(|m| m.name.as_str()).collect();
    let mut warnings = Vec::new();
    let mut couplers = Vec::new();
    for def in defs {
        match resolve_coupler(organ, &names, def) {
            Ok(coupler) => couplers.push(coupler),
            Err(warning) => warnings.push(warning),
        }
    }
    (couplers, warnings)
}

fn resolve_coupler(
    organ: &aristide_model::Organ,
    names: &[&str],
    def: &CouplerDef,
) -> Result<aristide_model::Coupler, String> {
    use aristide_model::{Coupler, CouplerRoute, CouplerTarget};
    let manual = |pattern: &str| -> Result<aristide_model::ManualId, String> {
        match match_names(names, pattern).as_slice() {
            [index] => Ok(organ.manuals[*index].id),
            [] => Err(format!(
                "{:?}: no manual matches {pattern:?} — coupler skipped",
                def.name
            )),
            _ => Err(format!(
                "{:?}: {pattern:?} is ambiguous — coupler skipped",
                def.name
            )),
        }
    };
    let key = |spec: &Option<KeySpec>| -> Result<Option<u8>, String> {
        match spec {
            None => Ok(None),
            Some(spec) => spec.midi_note().map(Some).ok_or_else(|| {
                format!("{:?}: unparseable key {spec:?} — coupler skipped", def.name)
            }),
        }
    };
    let mut routes = Vec::new();
    for route in &def.routes {
        let target = match &route.to {
            Some(to) => Some(CouplerTarget {
                manual: manual(to)?,
                key_shift: route.shift,
                repitch: route.repitch,
            }),
            None => None,
        };
        routes.push(CouplerRoute {
            from_manual: manual(&route.from)?,
            low_key: key(&route.low)?,
            high_key: key(&route.high)?,
            unison_off: route.unison_off,
            scope: route.scope,
            target,
        });
    }
    if routes.is_empty() {
        return Err(format!("{:?}: no routes — coupler skipped", def.name));
    }
    Ok(Coupler {
        name: def.name.clone(),
        routes,
    })
}

/// Swell box behaviour (applied to every enclosure the set defines;
/// per-box overrides can come later with the voicing layer). Constants
/// and defaults grounded in docs/research/enclosure-modeling.md.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EnclosuresConfig {
    /// MIDI expression controller number driving the boxes (11 =
    /// expression, the convention; 7 works for volume-pedal rigs).
    #[serde(default = "default_expression_cc")]
    pub cc: u8,
    /// Broadband attenuation fully closed, dB (negative). 0 = derive
    /// from the set's own AmpMinimumLevel (GO semantics).
    #[serde(default)]
    pub floor_db: f64,
    /// Extra high-shelf attenuation fully closed, dB (negative) —
    /// the "muffle". Measured boxes lose ~10 dB more treble than bass.
    #[serde(default = "default_shelf_db")]
    pub shelf_db: f64,
    /// Shelf corner fully open / fully closed, Hz.
    #[serde(default = "default_corner_open_hz")]
    pub corner_open_hz: f64,
    #[serde(default = "default_corner_closed_hz")]
    pub corner_closed_hz: f64,
    /// Pedal-to-dB curve: 1 = dB-linear (Hauptwerk's law), >1 bunches
    /// the change near closed (raw physics), <1 the opposite.
    #[serde(default = "default_taper")]
    pub taper: f64,
    /// Shutter inertia: full-sweep settle time in seconds (0 = the
    /// pedal drives the shutters directly).
    #[serde(default = "default_full_sweep_s")]
    pub full_sweep_s: f64,
}

fn default_expression_cc() -> u8 {
    11
}

fn default_shelf_db() -> f64 {
    -10.0
}

fn default_corner_open_hz() -> f64 {
    8_000.0
}

fn default_corner_closed_hz() -> f64 {
    1_000.0
}

fn default_taper() -> f64 {
    1.0
}

fn default_full_sweep_s() -> f64 {
    0.5
}

impl Default for EnclosuresConfig {
    fn default() -> Self {
        EnclosuresConfig {
            cc: default_expression_cc(),
            floor_db: 0.0,
            shelf_db: default_shelf_db(),
            corner_open_hz: default_corner_open_hz(),
            corner_closed_hz: default_corner_closed_hz(),
            taper: default_taper(),
            full_sweep_s: default_full_sweep_s(),
        }
    }
}

/// Control-operating noises (drawstop thumps, coupler clacks, blower).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NoisesConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_noise_volume")]
    pub volume: f64,
}

fn default_true() -> bool {
    true
}

fn default_noise_volume() -> f64 {
    0.7
}

impl Default for NoisesConfig {
    fn default() -> Self {
        NoisesConfig {
            enabled: true,
            volume: default_noise_volume(),
        }
    }
}

/// Convolution reverb: an impulse-response file next to the set (or
/// "synthetic" for a generated hall) mixed at `wet`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReverbConfig {
    /// Path to an IR wav relative to the set, "synthetic", or "" = off.
    #[serde(default)]
    pub ir: String,
    #[serde(default = "default_reverb_wet")]
    pub wet: f64,
}

fn default_reverb_wet() -> f64 {
    0.25
}

impl Default for ReverbConfig {
    fn default() -> Self {
        ReverbConfig {
            ir: String::new(),
            wet: default_reverb_wet(),
        }
    }
}

/// Temperament / concert pitch / transposition defaults for the set.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TuningConfig {
    /// equal | werckmeister3 | kirnberger3 | meantone4 | pythagorean
    /// — twelve-class vocabulary, meaningful only when `edo = 12`.
    #[serde(default = "default_temperament")]
    pub temperament: String,
    /// Equal divisions of the octave the keys walk (12 unless said
    /// otherwise); away from 12 the temperament is dormant.
    #[serde(default = "default_edo")]
    pub edo: u16,
    #[serde(default = "default_a4_hz")]
    pub a4_hz: f64,
    /// Semitones added to incoming keys (a transposer).
    #[serde(default)]
    pub transpose: i8,
    /// A Scala `.scl` file standing in for the temperament, resolved
    /// against the organ file's directory.
    #[serde(default)]
    pub scale: Option<String>,
    /// Its `.kbm` keyboard mapping; omitted, keys map linearly with a′
    /// anchored at `a4_hz`.
    #[serde(default)]
    pub keymap: Option<String>,
}

fn default_temperament() -> String {
    "equal".into()
}

fn default_edo() -> u16 {
    12
}

fn default_a4_hz() -> f64 {
    440.0
}

impl Default for TuningConfig {
    fn default() -> Self {
        TuningConfig {
            temperament: default_temperament(),
            edo: default_edo(),
            a4_hz: default_a4_hz(),
            transpose: 0,
            scale: None,
            keymap: None,
        }
    }
}

/// How decoded audio stays resident in RAM.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SamplesConfig {
    /// Resident sample resolution: 16 (default — half the RAM of f32,
    /// −96 dB floor, below organ recordings' own room noise and what
    /// GO/HW effectively play from) or 32 (bit-exact f32, for A/B).
    /// Analysis (loop periods, phase maps, tail measurement) always
    /// runs at full decode precision before quantization.
    #[serde(default = "default_sample_bits")]
    pub bits: u32,
    /// Persist decoded samples + analysis next to the user config so
    /// unchanged files skip decode on the next load. Costs disk about
    /// the size of the resident bank.
    #[serde(default = "default_true")]
    pub cache: bool,
}

fn default_sample_bits() -> u32 {
    16
}

impl Default for SamplesConfig {
    fn default() -> Self {
        SamplesConfig {
            bits: default_sample_bits(),
            cache: true,
        }
    }
}

/// Synthesized tremulant, rendered as periodic wind-pressure modulation
/// (measured behaviour: ~6 Hz, FM ±10–15 cents typical / ±24 ceiling,
/// see docs/research/organ-wind-acoustics.md §5).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Tremulant {
    /// Beat rate in Hz.
    #[serde(default = "default_trem_rate_hz")]
    pub rate_hz: f64,
    /// Peak pitch swing in cents.
    #[serde(default = "default_trem_depth_cents")]
    pub depth_cents: f64,
    /// 1-based ODF windchest numbers the tremulant acts on; empty = all.
    #[serde(default)]
    pub chests: Vec<u32>,
}

fn default_trem_rate_hz() -> f64 {
    5.0
}

fn default_trem_depth_cents() -> f64 {
    12.0
}

impl Default for Tremulant {
    fn default() -> Self {
        Tremulant {
            rate_hz: default_trem_rate_hz(),
            depth_cents: default_trem_depth_cents(),
            chests: Vec::new(),
        }
    }
}

/// Wind-supply behaviour for this instrument (applied to every chest;
/// per-chest overrides can come later with the voicing layer).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Wind {
    /// Steady pitch sag of a full chorus, in cents. 0 disables the
    /// wind model. Transient dips reach a bit deeper, briefly.
    #[serde(default = "default_sag_cents")]
    pub sag_cents: f64,
    /// Regulator resonance in Hz: response speed and where the bellows
    /// bounce sits.
    #[serde(default = "default_bounce_hz")]
    pub bounce_hz: f64,
    /// Damping ratio: below 1 is bouncy, 1 is critically damped.
    #[serde(default = "default_damping")]
    pub damping: f64,
    /// Per-pipe wind-flow noise in percent (measured 1–5; reeds more).
    /// Slow independent wander per pipe; 0 disables.
    #[serde(default = "default_flow_noise_percent")]
    pub flow_noise_percent: f64,
}

fn default_flow_noise_percent() -> f64 {
    2.0
}

fn default_sag_cents() -> f64 {
    3.0
}

fn default_bounce_hz() -> f64 {
    3.5
}

fn default_damping() -> f64 {
    0.5
}

impl Default for Wind {
    fn default() -> Self {
        Wind {
            sag_cents: default_sag_cents(),
            bounce_hz: default_bounce_hz(),
            damping: default_damping(),
            flow_noise_percent: default_flow_noise_percent(),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Midi {
    /// Manual names in MIDI-channel order: `channels[0]` is the manual
    /// MIDI channel 0 plays, and so on. Names match like stop patterns
    /// (exact first, then shortest substring), case-insensitively.
    #[serde(default)]
    pub channels: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Registration {
    /// Stop patterns drawn at startup (same matching rules as
    /// `--stops`): a pattern selects every stop whose name matches it
    /// exactly (case-insensitive); if none do, the substring matches
    /// with the shortest names win — so "plein jeu" draws
    /// "Plein jeu III", not "Plein jeu stop noise".
    #[serde(default)]
    pub default: Vec<String>,
}

/// The sidecar path for a given sample-set file.
pub fn path_for(set: &Path) -> PathBuf {
    let mut name = set.file_name().unwrap_or_default().to_os_string();
    name.push(".aristide.toml");
    set.with_file_name(name)
}

/// Load the sidecar next to `set`, if one exists.
pub fn load_for(set: &Path) -> Result<Option<Sidecar>, SidecarError> {
    let path = path_for(set);
    if !path.is_file() {
        return Ok(None);
    }
    let text = std::fs::read_to_string(&path)?;
    Ok(Some(toml::from_str(&text)?))
}

/// The shared pattern-matching rule: indices of `names` selected by
/// `pattern`. Exact (case-insensitive) matches win outright; otherwise
/// every substring match tied for the shortest name is selected.
pub fn match_names(names: &[&str], pattern: &str) -> Vec<usize> {
    if pattern == "*" {
        return (0..names.len()).collect();
    }
    let pattern = pattern.to_lowercase();
    let lowered: Vec<String> = names.iter().map(|n| n.to_lowercase()).collect();
    let exact: Vec<usize> = (0..names.len())
        .filter(|&i| lowered[i] == pattern)
        .collect();
    if !exact.is_empty() {
        return exact;
    }
    let matches: Vec<usize> = (0..names.len())
        .filter(|&i| lowered[i].contains(&pattern))
        .collect();
    let Some(shortest) = matches.iter().map(|&i| names[i].len()).min() else {
        return Vec::new();
    };
    matches
        .into_iter()
        .filter(|&i| names[i].len() == shortest)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn routing_and_voicing_tables_parse() {
        let text = r#"
[[routing.bus]]
name = "chamade"
stops = ["Trompette*"]
output = [3, 4]
gain_db = -3.0
[routing.bus.delay]
ms = 120
mix = 1.0
dry = 0.0
[[voicing.delay]]
stops = ["Montre*"]
ms = 12.5
"#;
        let sidecar: Sidecar = toml::from_str(text).expect("parses");
        assert_eq!(sidecar.routing.buses.len(), 1);
        let bus = &sidecar.routing.buses[0];
        assert_eq!(bus.output, Some([3, 4]));
        assert_eq!(bus.gain_db, -3.0);
        let delay = bus.delay.as_ref().expect("delay configured");
        assert_eq!(delay.ms, 120.0);
        assert_eq!(delay.feedback, 0.0, "defaults to no feedback");
        assert_eq!(delay.mix, 1.0);
        assert_eq!(delay.dry, 0.0, "a displaced division kills its dry");
        assert_eq!(sidecar.voicing.delays.len(), 1);
        assert_eq!(sidecar.voicing.delays[0].ms, 12.5);
        // Absent tables cost nothing.
        let empty: Sidecar = toml::from_str("").expect("parses");
        assert!(empty.routing.buses.is_empty());
        assert!(empty.voicing.delays.is_empty());
    }

    #[test]
    fn parses_a_full_sidecar() {
        let text = r#"
[midi]
channels = ["First Manual", "Second Manual", "Pedal"]

[registration]
default = ["Bourdon 16'", "Montre 8'", "Prestant 4'", "Plein jeu III"]
"#;
        let sidecar: Sidecar = toml::from_str(text).expect("parses");
        assert_eq!(sidecar.midi.channels.len(), 3);
        assert_eq!(sidecar.registration.default.len(), 4);
    }

    #[test]
    fn unknown_fields_are_rejected() {
        let result: Result<Sidecar, _> = toml::from_str("[typo]\nx = 1\n");
        assert!(result.is_err(), "typos should not pass silently");
    }

    #[test]
    fn sidecar_path_appends_suffix() {
        assert_eq!(
            path_for(Path::new("/sets/demo.organ")),
            PathBuf::from("/sets/demo.organ.aristide.toml")
        );
    }

    #[test]
    fn note_names_parse_scientific_pitch() {
        assert_eq!(parse_note_name("C4"), Some(60), "middle C");
        assert_eq!(parse_note_name("C3"), Some(48), "tenor C");
        assert_eq!(parse_note_name("A0"), Some(21));
        assert_eq!(parse_note_name("c#2"), Some(37));
        assert_eq!(parse_note_name("Bb1"), Some(34));
        assert_eq!(parse_note_name("C-1"), Some(0));
        assert_eq!(parse_note_name("H2"), None);
        assert_eq!(parse_note_name("C"), None, "an octave is required");
    }

    fn two_manual_organ() -> aristide_model::Organ {
        let manual = |id: u32, name: &str| aristide_model::Manual {
            id: aristide_model::ManualId(id),
            name: name.into(),
            first_midi_note: 36,
            key_count: 61,
                    kind: Default::default(),
                    hex: None,
        };
        aristide_model::Organ {
            manuals: vec![manual(1, "Grand Orgue"), manual(2, "Récit")],
            ..Default::default()
        }
    }

    #[test]
    fn defined_couplers_resolve_by_manual_name() {
        let text = r#"
[[couplers.define]]
name = "Fourths II/I"
[[couplers.define.route]]
from = "récit"
to = "grand"
shift = -5
low = "C3"

[[couplers.define]]
name = "16' GO"
[[couplers.define.route]]
from = "grand"
to = "grand"
shift = -12
low = 60
[[couplers.define.route]]
from = "grand"
to = "grand"
shift = -12
high = 59
unison_off = true
repitch = true
"#;
        let sidecar: Sidecar = toml::from_str(text).expect("parses");
        let (couplers, warnings) = resolve_couplers(&two_manual_organ(), &sidecar.couplers.define);
        assert_eq!(warnings, Vec::<String>::new());
        assert_eq!(couplers.len(), 2);

        let fourths = &couplers[0];
        assert_eq!(fourths.routes.len(), 1);
        let route = &fourths.routes[0];
        assert_eq!(route.from_manual, aristide_model::ManualId(2));
        assert_eq!(route.low_key, Some(48));
        assert_eq!(route.high_key, None);
        let target = route.target.as_ref().expect("has a target");
        assert_eq!((target.manual, target.key_shift), (aristide_model::ManualId(1), -5));

        let sixteen = &couplers[1];
        assert_eq!(sixteen.routes.len(), 2);
        assert!(sixteen.routes[1].unison_off);
        assert_eq!(sixteen.routes[1].target.as_ref().unwrap().repitch, Some(true));
    }

    #[test]
    fn couplers_naming_missing_manuals_are_reported_not_fatal() {
        let defs = vec![CouplerDef {
            name: "Chamade on V".into(),
            routes: vec![RouteDef {
                from: "Bombardewerk".into(),
                to: Some("Grand".into()),
                shift: 0,
                low: None,
                high: None,
                unison_off: false,
                scope: aristide_model::CouplerScope::AllKeys,
                repitch: None,
            }],
        }];
        let (couplers, warnings) = resolve_couplers(&two_manual_organ(), &defs);
        assert!(couplers.is_empty());
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("Bombardewerk"), "{}", warnings[0]);
    }

    #[test]
    fn matching_prefers_exact_then_shortest() {
        let names = [
            "Plein jeu III",
            "Plein jeu stop noise",
            "Montre 8'",
            "Montre 8' stop noise",
        ];
        assert_eq!(match_names(&names, "plein jeu iii"), vec![0]);
        assert_eq!(match_names(&names, "plein jeu"), vec![0]);
        assert_eq!(match_names(&names, "montre"), vec![2]);
        // Both noise stops are 20 chars — tied shortest, both selected.
        assert_eq!(match_names(&names, "noise"), vec![1, 3]);
        assert!(match_names(&names, "trompette").is_empty());
    }
}
