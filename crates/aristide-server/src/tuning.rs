//! Temperaments, concert pitch, and transposition — the first slice of
//! the contemporary-music tuning layer (DESIGN.md M6).
//!
//! All control-side: a per-note rate multiplier folded into StartVoice,
//! which is why the RT engine needed no changes. Offsets are
//! **a-referenced** (a′ keeps its frequency when the temperament
//! changes — standard practice, and what CBH's tables assume), with the
//! precise C-referenced cent values cross-checked against Carey Beebe's
//! cent-deviation tables (hpschd.nu/tech/tun/cents.html) and the
//! tonalsoft encyclopedia entries for Werckmeister/Kirnberger.

use aristide_model::units::{cents_between, cents_to_ratio, equal_ladder_hz};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Temperament {
    /// The organ's own tuning, as recorded: every pipe plays exactly
    /// as the samples have it, whatever pitch standard and temperament
    /// the instrument was sampled in — measured at load into a
    /// [`HomeTuning`] so the console can *name* it, and so the
    /// reference can pull the whole instrument to another pitch while
    /// keeping its intervals. Not a table: the tables below are
    /// targets that retune every pipe from its measured pitch.
    Original,
    Equal,
    /// Werckmeister III (1691): C–G–D–A and B–F♯ narrowed ¼ Pythagorean
    /// comma; the organ temperament of the Baroque north.
    Werckmeister3,
    /// Kirnberger III (1779): C–E chain narrowed ¼ syntonic comma.
    Kirnberger3,
    /// Quarter-comma meantone (Aron 1523): pure major thirds, wolf at
    /// G♯–E♭ — the Renaissance/early-Baroque organ standard.
    Meantone4,
    /// Pythagorean: pure fifths, ditone thirds — medieval organum.
    Pythagorean,
}

impl Temperament {
    pub fn parse(name: &str) -> Option<Temperament> {
        Some(match name.to_lowercase().replace(['-', '_', ' '], "").as_str() {
            "original" | "asrecorded" | "recorded" | "home" => Temperament::Original,
            "equal" | "et" | "12edo" => Temperament::Equal,
            "werckmeister3" | "werckmeisteriii" | "werckmeister" => Temperament::Werckmeister3,
            "kirnberger3" | "kirnbergeriii" | "kirnberger" => Temperament::Kirnberger3,
            "meantone4" | "meantone" | "quartercommameantone" => Temperament::Meantone4,
            "pythagorean" => Temperament::Pythagorean,
            _ => return None,
        })
    }

    pub fn name(&self) -> &'static str {
        match self {
            Temperament::Original => "original",
            Temperament::Equal => "equal",
            Temperament::Werckmeister3 => "werckmeister3",
            Temperament::Kirnberger3 => "kirnberger3",
            Temperament::Meantone4 => "meantone4",
            Temperament::Pythagorean => "pythagorean",
        }
    }

    /// The twelve-class tables — every temperament that is a *target*.
    pub const ALL: [Temperament; 5] = [
        Temperament::Equal,
        Temperament::Werckmeister3,
        Temperament::Kirnberger3,
        Temperament::Meantone4,
        Temperament::Pythagorean,
    ];

    /// Deviation from equal temperament per pitch class (C = index 0),
    /// in cents, normalized so A = 0. `Original` has no table of its
    /// own — the organ's measured one stands in (see [`HomeTuning`]).
    pub fn offsets_cents(&self) -> [f32; 12] {
        match self {
            Temperament::Original | Temperament::Equal => [0.0; 12],
            Temperament::Werckmeister3 => [
                11.730, 1.955, 3.910, 5.865, 1.955, 9.775, 0.000, 7.820, 3.910, 0.000, 7.820,
                3.910,
            ],
            Temperament::Kirnberger3 => [
                10.265, 0.490, 3.422, 4.400, -3.421, 8.310, 0.489, 6.843, 2.445, 0.000, 6.355,
                -1.466,
            ],
            Temperament::Meantone4 => [
                10.265, -13.686, 3.422, 20.530, -3.421, 13.687, -10.264, 6.843, -17.108, 0.000,
                17.108, -6.843,
            ],
            Temperament::Pythagorean => [
                -5.865, 7.820, -1.955, -11.730, 1.955, -7.820, 5.865, -3.910, 9.775, 0.000,
                -9.775, 3.910,
            ],
        }
    }
}

/// What the samples were recorded in: the organ's *home* tuning, fitted
/// at load from loop-period estimates, with trusted pitch declarations
/// disambiguating compound recordings. The
/// truth the tuning layer works from instead of assuming that every
/// set sits on the 12-EDO/A440 ladder — a Baroque set at a′ = 415 in
/// meantone is exactly that, and a target of "440 equal" or "452
/// Pythagorean" is a per-pipe retune from here, not from a guess.
///
/// Pitch classes are those of the *sounding* pitch (a 4′ rank's pipe
/// under C4 is a C, a 2⅔′ mutation's is a G) and the table is
/// a-referenced like the [`Temperament`] tables, so the two compare
/// directly. Only octave-class ranks (nominals on the equal ladder)
/// feed the table: a mutation is tuned pure against its unison, and
/// would smear the class it lands on.
#[derive(Debug, Clone, PartialEq)]
pub struct HomeTuning {
    /// The a′ the instrument's A pipes sound, on the equal ladder
    /// through the fitted table: the pitch standard it was recorded
    /// at (415 for a Baroque set, 440 for a modern one, 465 chorton).
    pub a4_hz: f64,
    /// Median deviation from equal per pitch class (C = 0), cents,
    /// A = 0 — the temperament the tuner left the organ in.
    pub offsets_cents: [f64; 12],
    /// The named temperament the table matches within
    /// [`HomeTuning::MATCH_CENTS`] RMS, if any; `None` is an unequal
    /// temperament the tables don't name (or a drifted one).
    pub temperament: Option<Temperament>,
    /// Robust spread of the pipes around the fitted table (median
    /// absolute residual), cents: tuning drift, or "this instrument
    /// holds two pitch standards" when it is large.
    pub spread_cents: f64,
    /// Pipes with a usable pitch estimate, and pipes looked at.
    pub measured: usize,
    pub pipes: usize,
}

impl HomeTuning {
    /// RMS distance under which the measured table is called by a
    /// named temperament: Werckmeister and Kirnberger III sit ~4 cents
    /// RMS apart, so the threshold stays under half that.
    pub const MATCH_CENTS: f64 = 1.75;

    /// Fit the home tuning from per-pipe measurements: `(pitch class
    /// of the sounding pitch, deviation from the equal ladder in
    /// cents, whether the pipe's nominal lies on that ladder)`. Pipes
    /// off the ladder (mutations) count towards the anchor spread but
    /// not the class table. `None` without a single measurement.
    pub fn fit(pipes: impl IntoIterator<Item = (usize, f64, bool)>, total: usize) -> Option<HomeTuning> {
        let mut classes: [Vec<f64>; 12] = Default::default();
        let mut all = Vec::new();
        for (class, deviation, on_ladder) in pipes {
            if !deviation.is_finite() {
                continue;
            }
            all.push(deviation);
            if on_ladder {
                classes[class % 12].push(deviation);
            }
        }
        if all.is_empty() {
            return None;
        }
        // A-referenced: the A class anchors when it measured, else the
        // instrument's median stands in for it.
        let measured = all.len();
        let overall = median(&mut all).unwrap_or(0.0);
        let a = median(&mut classes[9].clone()).unwrap_or(overall);
        let mut offsets_cents = [0.0; 12];
        for (class, values) in classes.iter_mut().enumerate() {
            offsets_cents[class] = median(values).map_or(0.0, |m| m - a);
        }
        let mut residuals: Vec<f64> = classes
            .iter()
            .enumerate()
            .flat_map(|(class, values)| {
                values
                    .iter()
                    .map(move |v| (v - a - offsets_cents[class]).abs())
            })
            .collect();
        let spread_cents = median(&mut residuals).unwrap_or(0.0);
        let temperament = Temperament::ALL
            .iter()
            .map(|t| {
                let table = t.offsets_cents();
                let rms = (0..12)
                    .map(|c| (offsets_cents[c] - table[c] as f64).powi(2))
                    .sum::<f64>()
                    .sqrt()
                    / 12f64.sqrt();
                (*t, rms)
            })
            .filter(|(_, rms)| *rms <= Self::MATCH_CENTS)
            .min_by(|x, y| x.1.total_cmp(&y.1))
            .map(|(t, _)| t);
        Some(HomeTuning {
            a4_hz: 440.0 * cents_to_ratio(a),
            offsets_cents,
            temperament,
            spread_cents,
            measured,
            pipes: total,
        })
    }

    /// This tuning with its pitch standard moved to `anchor_cents`
    /// from 440: the home of one set or one rank inside the
    /// instrument, which shares the instrument's class table but sits
    /// at its own pitch (a 415 Positif beside a 440 Great).
    pub fn at_anchor(&self, anchor_cents: f64) -> HomeTuning {
        HomeTuning {
            a4_hz: 440.0 * cents_to_ratio(anchor_cents),
            ..self.clone()
        }
    }

    /// The a′ shift alone: how far the instrument's pitch standard
    /// sits from 440, cents.
    pub fn anchor_cents(&self) -> f64 {
        cents_between(440.0, self.a4_hz)
    }

    /// Where this tuning puts manual key `key` relative to the equal
    /// A440 ladder, cents — the same contract as
    /// [`Tuning::deviation_cents`], for the organ as recorded.
    pub fn deviation_cents(&self, key: u16) -> f64 {
        self.anchor_cents() + self.offsets_cents[(key % 12) as usize]
    }

    /// The pitch anchor that names this tuning on `key`: the Hz the
    /// recorded organ sounds there.
    pub fn reference(&self, key: u8) -> PitchReference {
        PitchReference {
            key,
            hz: equal_ladder_hz(key as f64) * cents_to_ratio(self.deviation_cents(key as u16)),
        }
    }
}

/// Median of a slice, sorting it in place; `None` when empty.
pub(crate) fn median(values: &mut [f64]) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    values.sort_by(f64::total_cmp);
    let mid = values.len() / 2;
    Some(if values.len().is_multiple_of(2) {
        (values[mid - 1] + values[mid]) / 2.0
    } else {
        values[mid]
    })
}

/// What a target tuning does with each pipe's own drift — the few
/// cents every real pipe sits from where its tuner meant it (weather,
/// a knocked slide, a deliberately stretched top), left over once the
/// pitch standard and the temperament are accounted for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PipeRetune {
    /// Each pipe moves by what its neighbours move by and keeps its
    /// own drift: the same instrument, retuned by a tuner exactly as
    /// good as the original one.
    #[default]
    Original,
    /// Each pipe lands on the target to the precision of the
    /// measurement — a clinically in-tune instrument.
    Exact,
}

impl PipeRetune {
    pub fn parse(name: &str) -> Option<PipeRetune> {
        Some(match name.trim().to_lowercase().as_str() {
            "original" | "keep" | "drift" => PipeRetune::Original,
            "exact" | "flat" | "flatten" => PipeRetune::Exact,
            _ => return None,
        })
    }

    pub fn name(&self) -> &'static str {
        match self {
            PipeRetune::Original => "original",
            PipeRetune::Exact => "exact",
        }
    }
}

/// What a stop plays when it has no tuning of its own: which scope
/// above it governs. Division and sample set are two axes that only
/// meet at the stop, so `Auto` — the default — orders them: the
/// division's own tuning wins (what a keyboard plays is a performance
/// fact, and a keyboard silently playing the wrong scale on some stops
/// is the worse failure), else the set's, else the instrument's. The
/// named variants pin one scope and skip the others.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Follow {
    #[default]
    Auto,
    Division,
    Source,
    Organ,
}

impl Follow {
    pub fn parse(name: &str) -> Option<Follow> {
        Some(match name.trim().to_lowercase().replace(['-', '_', ' '], "").as_str() {
            "auto" | "automatic" => Follow::Auto,
            "division" | "manual" => Follow::Division,
            "source" | "set" | "sampleset" => Follow::Source,
            "organ" | "instrument" => Follow::Organ,
            _ => return None,
        })
    }

    pub fn name(&self) -> &'static str {
        match self {
            Follow::Auto => "auto",
            Follow::Division => "division",
            Follow::Source => "source",
            Follow::Organ => "organ",
        }
    }
}

/// The scope whose tuning a voice actually plays under — where
/// resolution landed, for the console to say so.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TuningScope {
    Organ,
    Source,
    Division,
    Stop,
    Rank,
}

impl TuningScope {
    pub fn name(&self) -> &'static str {
        match self {
            TuningScope::Organ => "organ",
            TuningScope::Source => "source",
            TuningScope::Division => "division",
            TuningScope::Stop => "stop",
            TuningScope::Rank => "rank",
        }
    }
}

/// A Scala scale with its keyboard mapping, loaded and ready — one
/// division's whole key→pitch table. Shared by `Arc`: tunings are
/// cloned on every key press's landing walk, and the table itself
/// never changes once built (a new scale is a new `Arc`).
#[derive(Debug, Clone)]
pub struct ScaleTuning {
    /// The `.scl` path as the organ file spelled it (kept verbatim so
    /// edits round-trip; resolved against the organ file's directory).
    pub scl: String,
    /// The `.kbm` path, or `None` for the linear default mapping
    /// (chromatic degrees, anchored at the tuning's own reference).
    pub kbm: Option<String>,
    pub scale: aristide_model::scala::Scale,
    pub mapping: aristide_model::scala::KeyboardMapping,
}

impl ScaleTuning {
    /// Read and parse a scale (and optionally its keyboard mapping)
    /// from disk, `base`-relative for relative paths. `reference`
    /// anchors the linear default mapping when no `.kbm` is given; an
    /// explicit mapping carries its own reference and ignores it.
    pub fn load(
        scl: &str,
        kbm: Option<&str>,
        reference: PitchReference,
        base: Option<&std::path::Path>,
    ) -> Result<ScaleTuning, String> {
        let resolve = |path: &str| -> std::path::PathBuf {
            let path = std::path::Path::new(path);
            match (path.is_relative(), base) {
                (true, Some(base)) => base.join(path),
                _ => path.to_path_buf(),
            }
        };
        let read = |path: &str| -> Result<String, String> {
            let resolved = resolve(path);
            std::fs::read_to_string(&resolved)
                .map_err(|err| format!("{}: {err}", resolved.display()))
        };
        let scale = aristide_model::scala::Scale::parse(&read(scl)?)
            .map_err(|err| format!("{scl}: {err}"))?;
        let mapping = match kbm {
            Some(kbm) => aristide_model::scala::KeyboardMapping::parse(&read(kbm)?)
                .map_err(|err| format!("{kbm}: {err}"))?,
            None => reference.linear_mapping(),
        };
        Ok(ScaleTuning {
            scl: scl.to_string(),
            kbm: kbm.map(str::to_string),
            scale,
            mapping,
        })
    }

    /// A short human name: the scale's description line, else the file
    /// stem.
    pub fn name(&self) -> &str {
        let description = self.scale.description.trim();
        if !description.is_empty() {
            return description;
        }
        std::path::Path::new(&self.scl)
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or(&self.scl)
    }
}

/// The pitch anchor of a tuning: one piano key and what it sounds.
/// "a′ = 440 Hz" is the familiar instance, but it presumes the tuning
/// has an a′ — under 15-EDO or a Bohlen–Pierce scale the only thing
/// that stays meaningful is "this physical key sounds this many Hz",
/// which is exactly how Scala's `.kbm` files anchor pitch too. The key
/// is a MIDI key number, named on the console in scientific pitch
/// notation (C4 = middle C).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PitchReference {
    pub key: u8,
    pub hz: f64,
}

impl PitchReference {
    pub const A440: PitchReference = PitchReference { key: 69, hz: 440.0 };

    /// The a′ this anchor implies on the equal ladder — 415 for "A4 =
    /// 415", 430.5 for "C4 = 256": the one number that says how far
    /// the whole instrument is being pulled from its recorded pitch,
    /// whichever key the player chose to say it with.
    pub fn implied_a4_hz(&self) -> f64 {
        self.hz * ((69.0 - self.key as f64) / 12.0).exp2()
    }

    /// Bound the pull to what the samples can be bent to without
    /// sounding like a different instrument: the same a′ 300–500 Hz
    /// window the console has always offered, applied through the
    /// reference key so "C4 = 200 Hz" is refused the same way "A4 =
    /// 200 Hz" is.
    pub fn clamped(self) -> PitchReference {
        let key = self.key;
        if !(self.hz.is_finite() && self.hz > 0.0) {
            return PitchReference { key, hz: equal_ladder_hz(key as f64) };
        }
        let implied = self.implied_a4_hz();
        let allowed = implied.clamp(300.0, 500.0);
        PitchReference { key, hz: self.hz * allowed / implied }
    }

    /// How far the reference key sits from its recorded pitch, in
    /// cents — the shift every branch of [`Tuning::deviation_cents`]
    /// adds on top of its own interval arithmetic.
    fn anchor_cents(&self) -> f64 {
        cents_between(equal_ladder_hz(self.key as f64), self.hz)
    }

    /// The linear Scala mapping this anchor stands for: successive
    /// degrees on successive keys, the reference key at its Hz.
    pub fn linear_mapping(&self) -> aristide_model::scala::KeyboardMapping {
        aristide_model::scala::KeyboardMapping::linear(self.key as i32, self.hz)
    }
}

impl Default for PitchReference {
    fn default() -> Self {
        Self::A440
    }
}

/// The live tuning state: temperament + pitch anchor + transposition,
/// or a Scala scale standing in for the temperament.
#[derive(Debug, Clone)]
pub struct Tuning {
    pub temperament: Temperament,
    /// Equal divisions of the octave the keys walk: 12 is the common
    /// case and the only one where the temperament tables below mean
    /// anything — they are twelve-class vocabulary, dormant at any
    /// other count. Away from 12, every key is one step of
    /// `1200/edo` cents, anchored so the reference key sounds its Hz.
    pub edo: u16,
    /// When present, the scale supplies every key's pitch and the
    /// temperament and division count above are dormant — a Scala
    /// scale IS a tuning, with its own degree count and period.
    pub scale: Option<std::sync::Arc<ScaleTuning>>,
    /// Which key sounds what: A4 = 440 by default (415 baroque, 465
    /// chorton, … — or any other key, since past 12-EDO there may be
    /// no a′ to name). Under a scale with its own `.kbm` the mapping's
    /// reference governs instead.
    pub reference: PitchReference,
    /// Semitones added to incoming keys before routing — a transposer
    /// selects different pipes, like the real console gadget.
    pub transpose: i8,
    /// Under a target: whether each pipe keeps its own drift or lands
    /// exactly on the target. Moot as recorded.
    pub pipes: PipeRetune,
    /// What the organ was recorded in, when its pipes measured — the
    /// console stamps this into every tuning it installs. Under
    /// `Original` it is what the reference is measured against; under
    /// a target it only names the starting point.
    pub home: Option<std::sync::Arc<HomeTuning>>,
}

/// The one legal range for a divisions-per-octave count: 1 (octaves
/// only) up past 311-EDO, the largest anyone names in practice.
pub const EDO_RANGE: std::ops::RangeInclusive<u16> = 1..=311;

impl Default for Tuning {
    fn default() -> Self {
        Tuning {
            temperament: Temperament::Original,
            edo: 12,
            scale: None,
            reference: PitchReference::A440,
            transpose: 0,
            pipes: PipeRetune::Original,
            home: None,
        }
    }
}

impl Tuning {
    /// How far the pitch this tuning wants for manual key `key` sits
    /// from the 12-EDO/A440 ladder the samples were recorded on, in
    /// cents. `key` is a manual key coordinate — MIDI-note-numbered on
    /// a conventional keyboard, but allowed past 127 on a generalized
    /// one (Lumatone and the like). This is THE key→pitch conversion
    /// (CLAUDE.md's "one replaceable place"): the console turns it into
    /// which pipe to sound (whole semitones) and how far to bend it
    /// (the remainder). `None` means the key sounds nothing — no
    /// temperament says that, but a Scala keyboard mapping's unmapped
    /// keys will.
    pub fn deviation_cents(&self, key: u16) -> Option<f64> {
        if !self.corrects_pipes() {
            // As recorded: every key is its own pipe as the samples
            // have it, moved only by how far the reference was pulled
            // from where the recording puts that key.
            return Some(self.original_shift_cents());
        }
        if let Some(scale) = &self.scale {
            let hz =
                aristide_model::scala::key_frequency(&scale.scale, &scale.mapping, key as i32)?;
            // Distance from the 12-EDO/A440 pitch this key's nominal
            // pipe was recorded at.
            return Some(cents_between(equal_ladder_hz(key as f64), hz));
        }
        let anchor = self.reference.anchor_cents();
        let reference_key = self.reference.key as u16;
        if self.edo != 12 {
            // Equal steps of 1200/edo cents out from the reference key:
            // the same ladder a generated N-EDO scale with the linear
            // mapping would give, without the ceremony of a file.
            let from_reference = key as f64 - reference_key as f64;
            return Some(from_reference * (1200.0 / self.edo.max(1) as f64 - 100.0) + anchor);
        }
        // A temperament table is offsets from equal; the reference
        // key's own offset is what the anchor already accounts for.
        let offsets = self.temperament.offsets_cents();
        let class = (key % 12) as usize;
        let reference_class = (reference_key % 12) as usize;
        Some(offsets[class] as f64 - offsets[reference_class] as f64 + anchor)
    }

    /// Whether this tuning is a *target* that retunes each pipe from
    /// its measured pitch (a temperament table, a division count, a
    /// scale), or the organ as recorded (`Original` at 12), where the
    /// console leaves every pipe's own pitch alone and
    /// [`Tuning::deviation_cents`] is one whole-instrument shift.
    pub fn corrects_pipes(&self) -> bool {
        !(self.temperament == Temperament::Original && self.scale.is_none() && self.edo == 12)
    }

    /// What a target subtracts from its deviation for one pipe: the
    /// pipe's measured offset (`home`) when every pipe must land
    /// exactly, the fitted model's (`model`) when each keeps its own
    /// drift; nothing as recorded.
    pub fn pipe_offset(&self, home: f64, model: f64) -> f64 {
        if !self.corrects_pipes() {
            return 0.0;
        }
        match self.pipes {
            PipeRetune::Exact => home,
            PipeRetune::Original => model,
        }
    }

    /// Under `Original`: how far the reference pulls the instrument
    /// from its recorded pitch — zero while the reference is the
    /// organ's own (the default), +100 for a 415 set asked for 440.
    fn original_shift_cents(&self) -> f64 {
        let recorded = self
            .home
            .as_ref()
            .map_or(0.0, |home| home.deviation_cents(self.reference.key as u16));
        self.reference.anchor_cents() - recorded
    }

    /// The reference that says "as recorded" on `key`: the organ's own
    /// pitch there when it measured, else the equal ladder's.
    pub fn home_reference(&self, key: u8) -> PitchReference {
        match &self.home {
            Some(home) => home.reference(key),
            None => PitchReference { key, hz: equal_ladder_hz(key as f64) },
        }
    }

    /// How many keys step one octave under this tuning: the scale's
    /// degree count when one is loaded, else the declared divisions
    /// per octave. What layout presets and anything else that thinks
    /// in "steps" should ask, instead of assuming 12.
    pub fn steps_per_octave(&self) -> u16 {
        match &self.scale {
            Some(scale) => scale.scale.len().max(1) as u16,
            None => self.edo.max(1),
        }
    }

    /// Keep the linear default mapping anchored to the reference after
    /// it changes — an explicit `.kbm` owns its own reference and stays.
    pub fn refresh_scale_reference(&mut self) {
        if let Some(scale) = &self.scale
            && scale.kbm.is_none()
            && (scale.mapping.reference_hz != self.reference.hz
                || scale.mapping.reference_key != self.reference.key as i32)
        {
            let mut refreshed = (**scale).clone();
            refreshed.mapping = self.reference.linear_mapping();
            self.scale = Some(std::sync::Arc::new(refreshed));
        }
    }

    /// Rate multiplier for a pipe sounding MIDI note `key` (applied on
    /// top of the pipe's own playback rate). The whole deviation as one
    /// bend — callers that re-anchor to a nearer pipe split it instead.
    pub fn rate_multiplier(&self, key: u16) -> f32 {
        let cents = self.deviation_cents(key).unwrap_or(0.0);
        cents_to_ratio(cents) as f32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tables_match_the_cbh_reference() {
        // hpschd.nu quotes whole-cent a-referenced deviations; every
        // entry must round to theirs.
        let cases: [(Temperament, [i32; 12]); 3] = [
            (
                Temperament::Werckmeister3,
                [12, 2, 4, 6, 2, 10, 0, 8, 4, 0, 8, 4],
            ),
            (
                Temperament::Meantone4,
                [10, -14, 3, 21, -3, 14, -10, 7, -17, 0, 17, -7],
            ),
            (
                Temperament::Pythagorean,
                [-6, 8, -2, -12, 2, -8, 6, -4, 10, 0, -10, 4],
            ),
        ];
        for (temperament, expected) in cases {
            let offsets = temperament.offsets_cents();
            for class in 0..12 {
                assert!(
                    (offsets[class] - expected[class] as f32).abs() < 0.6,
                    "{:?} class {class}: {} vs CBH {}",
                    temperament,
                    offsets[class],
                    expected[class]
                );
            }
        }
    }

    #[test]
    fn a_stays_put_and_baroque_pitch_drops() {
        let mut tuning = Tuning::default();
        for temperament in Temperament::ALL {
            tuning.temperament = temperament;
            let a = tuning.rate_multiplier(69);
            assert!((a - 1.0).abs() < 1e-6, "{temperament:?}: a moved to {a}");
        }
        tuning.reference.hz = 415.0;
        let expected = (415.0f64 / 440.0) as f32;
        assert!((tuning.rate_multiplier(69) - expected).abs() < 1e-4);
    }

    /// The anchor may name any key: "C4 = 256 Hz" puts middle C at
    /// 256 exactly under every temperament, and the rest of the
    /// octave keeps its intervals relative to C rather than to A.
    #[test]
    fn reference_key_other_than_a_anchors_that_key() {
        let mut tuning = Tuning {
            reference: PitchReference { key: 60, hz: 256.0 },
            ..Tuning::default()
        };
        let hz = |tuning: &Tuning, key: u16| {
            equal_ladder_hz(key as f64) * tuning.rate_multiplier(key) as f64
        };
        for temperament in Temperament::ALL {
            tuning.temperament = temperament;
            let c = hz(&tuning, 60);
            assert!((c - 256.0).abs() < 0.01, "{temperament:?}: C4 at {c}");
        }
        // Equal: a′ lands nine equal steps up from the anchored C.
        tuning.temperament = Temperament::Equal;
        let a = hz(&tuning, 69);
        assert!((a - 256.0 * 2f64.powf(0.75)).abs() < 0.01, "a′ at {a}");
        // Meantone keeps its major third: E4 sits 5/4 above C4.
        tuning.temperament = Temperament::Meantone4;
        let e = hz(&tuning, 64);
        assert!((e / 256.0 - 1.25).abs() < 1e-3, "E4/C4 = {}", e / 256.0);
        // 19-EDO from the same anchor: the key 19 above C4 is its octave.
        tuning.edo = 19;
        assert!((hz(&tuning, 60) - 256.0).abs() < 0.01);
        assert!((hz(&tuning, 79) - 512.0).abs() < 0.01);
    }

    #[test]
    fn reference_clamps_through_the_implied_a() {
        let fine = PitchReference { key: 60, hz: 256.0 }.clamped();
        assert_eq!(fine, PitchReference { key: 60, hz: 256.0 });
        // C4 = 100 Hz would drag a′ to 168: refused down to the a′ 300
        // floor, expressed back at C4.
        let low = PitchReference { key: 60, hz: 100.0 }.clamped();
        assert_eq!(low.key, 60);
        assert!((low.implied_a4_hz() - 300.0).abs() < 1e-9, "{low:?}");
        let high = PitchReference { key: 69, hz: 900.0 }.clamped();
        assert_eq!(high, PitchReference { key: 69, hz: 500.0 });
        let nonsense = PitchReference { key: 69, hz: f64::NAN }.clamped();
        assert_eq!(nonsense, PitchReference::A440);
    }

    /// Away from 12, keys walk 1200/edo cents from a′ on key 69 and
    /// the temperament tables go dormant; at 12 nothing changes.
    #[test]
    fn edo_steps_from_a_and_silences_the_temperament() {
        let mut tuning = Tuning {
            edo: 24,
            temperament: Temperament::Meantone4,
            ..Tuning::default()
        };
        assert_eq!(tuning.deviation_cents(69), Some(0.0), "a′ stays put");
        assert_eq!(tuning.deviation_cents(70), Some(-50.0), "one 24-EDO step = 50 cents");
        assert_eq!(tuning.deviation_cents(68), Some(50.0));
        assert_eq!(tuning.deviation_cents(69 + 24), Some(-1200.0), "24 steps = the octave");
        assert_eq!(tuning.steps_per_octave(), 24);
        tuning.edo = 12;
        assert_ne!(
            tuning.deviation_cents(70),
            Some(0.0),
            "back at 12 the meantone tables speak again"
        );
        assert_eq!(tuning.steps_per_octave(), 12);
    }

    /// A synthetic organ recorded at a′ = 415 in ¼-comma meantone,
    /// with tuning drift, fits back to exactly that — and a target
    /// tuning then prices each key from the equal ladder as before,
    /// while `Original` prices only the reference's pull.
    #[test]
    fn home_fit_names_a_baroque_organ() {
        let table = Temperament::Meantone4.offsets_cents();
        let anchor = 1200.0 * (415.0f64 / 440.0).log2();
        let pipes = (36u16..=96).map(|key| {
            let drift = ((key * 7) % 11) as f64 * 0.1 - 0.5;
            let class = (key % 12) as usize;
            (class, anchor + table[class] as f64 + drift, true)
        });
        let home = HomeTuning::fit(pipes, 61).expect("fits");
        assert!((home.a4_hz - 415.0).abs() < 0.5, "a′ = {}", home.a4_hz);
        assert_eq!(home.temperament, Some(Temperament::Meantone4), "{home:?}");
        assert!(home.spread_cents < 1.6, "spread {}", home.spread_cents);
        assert_eq!((home.measured, home.pipes), (61, 61));
        // C4 sits where meantone at 415 puts it.
        let c4 = home.reference(60);
        let expected = 440.0 * ((60.0 - 69.0) / 12.0 + (anchor + table[0] as f64) / 1200.0).exp2();
        assert!((c4.hz - expected).abs() < 0.01, "{c4:?} vs {expected}");

        let home = std::sync::Arc::new(home);
        let mut tuning = Tuning {
            reference: home.reference(69),
            home: Some(home.clone()),
            ..Tuning::default()
        };
        assert!(!tuning.corrects_pipes());
        for key in [36u16, 60, 69, 73] {
            assert!(tuning.deviation_cents(key).unwrap().abs() < 1e-9, "as recorded = no shift");
        }
        // Asked for a′ = 440 in its own temperament: one +100 shift.
        tuning.reference = PitchReference::A440;
        assert!((tuning.deviation_cents(60).unwrap() + anchor).abs() < 0.2);
        // A target temperament prices from the ladder, home or not.
        tuning.temperament = Temperament::Equal;
        assert!(tuning.corrects_pipes());
        assert_eq!(tuning.deviation_cents(60), Some(0.0));
    }

    /// A modern equal-tempered organ reads as equal at its a′; a
    /// table nothing names stays unnamed.
    #[test]
    fn home_fit_distinguishes_named_from_unequal() {
        let equal = HomeTuning::fit((0..48).map(|i| (i % 12, 2.0 + (i % 3) as f64 * 0.5, true)), 48)
            .expect("fits");
        assert_eq!(equal.temperament, Some(Temperament::Equal));
        let a4_cents = 1200.0 * (equal.a4_hz / 440.0).log2();
        assert!((2.0..=3.0).contains(&a4_cents), "a′ sits {a4_cents} cents sharp");
        let odd = HomeTuning::fit(
            (0..48).map(|i| (i % 12, if i % 12 == 4 { -30.0 } else { 0.0 }, true)),
            48,
        )
        .expect("fits");
        assert_eq!(odd.temperament, None, "{odd:?}");
        assert_eq!(odd.offsets_cents[4], -30.0);
        assert_eq!(HomeTuning::fit(std::iter::empty(), 10), None);
    }

    #[test]
    fn parse_accepts_friendly_names() {
        assert_eq!(
            Temperament::parse("Werckmeister III"),
            Some(Temperament::Werckmeister3)
        );
        assert_eq!(Temperament::parse("meantone"), Some(Temperament::Meantone4));
        assert_eq!(Temperament::parse("Original"), Some(Temperament::Original));
        assert_eq!(Temperament::parse("as recorded"), Some(Temperament::Original));
        assert_eq!(Temperament::parse("nonsense"), None);
    }
}
