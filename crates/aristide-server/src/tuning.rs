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

use aristide_model::units::{cents_between, cents_to_ratio, equal_ladder_hz, ratio_to_cents};

/// One twelve-class temperament's definition: the size, in cents, of
/// each of its 11 consecutive fifths starting from pitch class `root`
/// (the 12th fifth, closing the circle, follows from whatever the
/// other eleven leave — it is never itself needed to fill in pitch
/// classes 0..11). Every entry but [`FifthsDef::PURE`] is a comma
/// fraction narrowed from the pure 3/2 fifth; deviations from equal
/// temperament are derived from this chain, never hand-transcribed —
/// this is the catalogue commit's whole point.
struct FifthsDef {
    id: &'static str,
    name: &'static str,
    root: usize,
    fifths: [f64; 11],
}

/// The pure 3/2 fifth, 701.955 cents: every "narrowed by a comma
/// fraction" entry below subtracts from this, not from the equal
/// 700-cent fifth (a fifth narrowed by a *quarter comma* still isn't
/// the equal fifth — that would take a third of a comma more).
fn pure_fifth_cents() -> f64 {
    ratio_to_cents(1.5)
}

fn syntonic_comma_cents() -> f64 {
    ratio_to_cents(81.0 / 80.0)
}

fn pythagorean_comma_cents() -> f64 {
    ratio_to_cents(531_441.0 / 524_288.0)
}

fn schisma_cents() -> f64 {
    pythagorean_comma_cents() - syntonic_comma_cents()
}

/// The full twelve-class catalogue, in menu order. `equal` sorts
/// first since it is the common case and the identity of every other
/// entry; `Original` and `Custom` are not tables and are not listed
/// here (see [`Temperament::ALL`] and [`Temperament::CATALOGUE_IDS`]).
fn catalogue() -> Vec<FifthsDef> {
    let pure = pure_fifth_cents();
    let q_syn = syntonic_comma_cents() / 4.0;
    let s_syn = syntonic_comma_cents() / 6.0;
    let q_pyth = pythagorean_comma_cents() / 4.0;
    let s_pyth = pythagorean_comma_cents() / 6.0;
    let schisma = schisma_cents();

    let mut werckmeister3 = [pure; 11];
    // C–G, G–D, D–A, B–F♯ narrowed ¼ Pythagorean comma; the rest pure.
    for i in [0, 1, 2, 5] {
        werckmeister3[i] = pure - q_pyth;
    }

    let mut kirnberger3 = [pure; 11];
    // C–G–D–A–E narrowed ¼ syntonic comma, F♯–C♯ by the schisma.
    for i in [0, 1, 2, 3] {
        kirnberger3[i] = pure - q_syn;
    }
    kirnberger3[6] = pure - schisma;

    let mut rameau1726 = [pure; 11];
    // Bb–F–C–G–D–A–E–B: seven ¼-comma meantone fifths; B–F♯–C♯–G♯ pure;
    // the remainder (G♯–E♭, and the E♭–B♭ that closes the circle) split
    // equally, ≈709.045¢ each — only G♯–E♭ falls inside these eleven.
    rameau1726[..7].fill(pure - q_syn);
    rameau1726[10] = 709.045;

    let mut vallotti = [pure; 11];
    // F–C–G–D–A–E–B narrowed 1/6 Pythagorean comma; the rest pure.
    vallotti[..6].fill(pure - s_pyth);

    let mut young2 = [pure; 11];
    // C–G–D–A–E–B–F♯ narrowed 1/6 Pythagorean comma; the rest pure.
    young2[..6].fill(pure - s_pyth);

    vec![
        FifthsDef { id: "equal", name: "Equal", root: 0, fifths: [700.0; 11] },
        FifthsDef {
            id: "meantone4",
            name: "Quarter-comma meantone",
            root: 3, // E♭
            fifths: [pure - q_syn; 11],
        },
        FifthsDef {
            id: "meantone6",
            name: "Sixth-comma meantone",
            root: 3, // E♭
            fifths: [pure - s_syn; 11],
        },
        FifthsDef { id: "pythagorean", name: "Pythagorean", root: 3, fifths: [pure; 11] },
        FifthsDef {
            id: "werckmeister3",
            name: "Werckmeister III",
            root: 0, // C
            fifths: werckmeister3,
        },
        FifthsDef {
            id: "kirnberger3",
            name: "Kirnberger III",
            root: 0, // C
            fifths: kirnberger3,
        },
        FifthsDef {
            id: "rameau1726",
            name: "Rameau 1726",
            root: 10, // B♭
            fifths: rameau1726,
        },
        FifthsDef { id: "vallotti", name: "Vallotti", root: 5 /* F */, fifths: vallotti },
        FifthsDef { id: "young2", name: "Young II", root: 0 /* C */, fifths: young2 },
    ]
}

/// Walk `def`'s chain of fifths and return each pitch class's
/// deviation from equal temperament, in cents, relative to whatever
/// class the chain started from (i.e. not yet re-referenced to A or
/// C — see [`a_referenced`] and [`c_rooted`]). Sound: each step is an
/// *absolute*, un-octave-reduced cents position, so the deviation at
/// step `k` is that position minus `k` equal-tempered fifths (`k *
/// 700`); this only works because every real temperament's total
/// drift stays far inside one octave, which it does for all of these.
fn class_deviations(def: &FifthsDef) -> [f64; 12] {
    let mut raw = [0.0f64; 12];
    let mut k_of = [0usize; 12];
    let mut pc = def.root;
    let mut acc = 0.0;
    for (i, fifth) in def.fifths.iter().enumerate() {
        acc += fifth;
        pc = (pc + 7) % 12;
        raw[pc] = acc;
        k_of[pc] = i + 1;
    }
    let mut dev = [0.0f64; 12];
    for pc in 0..12 {
        dev[pc] = raw[pc] - k_of[pc] as f64 * 700.0;
    }
    dev
}

/// `dev`, re-referenced so pitch class A (9) is 0 — the convention
/// [`Temperament::offsets_cents`] has always returned (a′ keeps its
/// frequency when the temperament changes, matching Carey Beebe's
/// published tables).
fn a_referenced(dev: [f64; 12]) -> [f32; 12] {
    let a = dev[9];
    std::array::from_fn(|pc| (dev[pc] - a) as f32)
}

/// `dev`, re-referenced so pitch class C (0) is 0 — used for the
/// UI-facing catalogue (`temperaments.json`) and this module's own
/// cross-checks against published C-referenced tables.
#[cfg(test)]
fn c_rooted(dev: [f64; 12]) -> [f64; 12] {
    let c = dev[0];
    std::array::from_fn(|pc| dev[pc] - c)
}

#[derive(Debug, Clone, Copy, PartialEq)]
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
    /// Sixth-comma meantone: the same construction at 1/6 comma, a
    /// gentler wolf.
    Meantone6,
    /// Pythagorean: pure fifths, ditone thirds — medieval organum.
    Pythagorean,
    /// Claudi Meneghin's reading of Rameau's 1726 temperament.
    Rameau1726,
    /// Vallotti: F..B narrowed 1/6 Pythagorean comma, the rest pure.
    Vallotti,
    /// Young's second (1799): the same construction rooted on C.
    Young2,
    /// A player-supplied table: 12 deviations from equal temperament,
    /// cents, indexed C..B as *absolute* pitch classes — unlike the
    /// named tables above, [`Tuning::temperament_root`] does not
    /// rotate this one; the caller already placed every class where
    /// they want it.
    Custom([f32; 12]),
}

impl Temperament {
    pub fn parse(name: &str) -> Option<Temperament> {
        let key = name.to_lowercase().replace(['-', '_', ' '], "");
        Some(match key.as_str() {
            "original" | "asrecorded" | "recorded" | "home" => Temperament::Original,
            "equal" | "et" | "12edo" => Temperament::Equal,
            "werckmeister3" | "werckmeisteriii" | "werckmeister" => Temperament::Werckmeister3,
            "kirnberger3" | "kirnbergeriii" | "kirnberger" => Temperament::Kirnberger3,
            "meantone4" | "meantone" | "quartercommameantone" => Temperament::Meantone4,
            "meantone6" | "sixthcommameantone" => Temperament::Meantone6,
            "pythagorean" => Temperament::Pythagorean,
            "rameau1726" | "rameau" => Temperament::Rameau1726,
            "vallotti" => Temperament::Vallotti,
            "young2" | "youngii" | "young" => Temperament::Young2,
            // A bare "custom" is a placeholder the caller must fill
            // with real deviations (the HTTP layer's `offsets=`, or
            // the sidecar's `offsets` array) — parsing it alone never
            // fabricates a table.
            "custom" => Temperament::Custom([0.0; 12]),
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
            Temperament::Meantone6 => "meantone6",
            Temperament::Pythagorean => "pythagorean",
            Temperament::Rameau1726 => "rameau1726",
            Temperament::Vallotti => "vallotti",
            Temperament::Young2 => "young2",
            Temperament::Custom(_) => "custom",
        }
    }

    /// The twelve-class tables — every named temperament that is a
    /// *target*, in menu order. Neither `Original` (no table of its
    /// own) nor `Custom` (carries no fixed table) appears here.
    pub const ALL: [Temperament; 9] = [
        Temperament::Equal,
        Temperament::Meantone4,
        Temperament::Meantone6,
        Temperament::Pythagorean,
        Temperament::Werckmeister3,
        Temperament::Kirnberger3,
        Temperament::Rameau1726,
        Temperament::Vallotti,
        Temperament::Young2,
    ];

    /// Deviation from equal temperament per pitch class (C = index 0),
    /// in cents, normalized so A = 0. `Original` has no table of its
    /// own — the organ's measured one stands in (see [`HomeTuning`]).
    /// `Custom` returns exactly its stored table, unreferenced (the
    /// caller's deviations are already absolute).
    pub fn offsets_cents(&self) -> [f32; 12] {
        match self {
            Temperament::Original | Temperament::Equal => [0.0; 12],
            Temperament::Custom(offsets) => *offsets,
            _ => catalogue()
                .into_iter()
                .find(|def| def.id == self.name())
                .map(|def| a_referenced(class_deviations(&def)))
                .unwrap_or([0.0; 12]),
        }
    }
}

/// What the samples were recorded in: the organ's *home* tuning, fitted
/// at load from declared recording pitches and authored playback relationships.
/// This describes the instrument's playback pitch, not a waveform measurement.
/// The tuning layer uses these declared facts instead of assuming that every
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
    /// The pitch class (0 = C .. 11 = B) a named temperament is
    /// centred on: the table rotates so this class plays what the
    /// table calls class 0, moving the wolf and every other interval
    /// to a different key without changing which key sounds what
    /// reference pitch. Dormant under `Original`, `Custom` (which
    /// carries its own absolute classes) and away from 12-EDO.
    pub temperament_root: u8,
    /// A fine offset, cents, added to every key's deviation — on top
    /// of a temperament table, an equal division, a scale, or even
    /// `Original`.
    pub offset_cents: f64,
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
            temperament_root: 0,
            offset_cents: 0.0,
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
        self.deviation_cents_core(key).map(|d| d + self.offset_cents)
    }

    /// [`Tuning::deviation_cents`] before the fine offset is added —
    /// split out so the offset applies exactly once, on every branch,
    /// without duplicating it at each early return.
    fn deviation_cents_core(&self, key: u16) -> Option<f64> {
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
        // A temperament table is offsets from equal, rotated onto
        // `temperament_root`; the reference key's own offset is what
        // the anchor already accounts for.
        let offsets = self.rooted_offsets_cents();
        let class = (key % 12) as usize;
        let reference_class = (reference_key % 12) as usize;
        Some(offsets[class] as f64 - offsets[reference_class] as f64 + anchor)
    }

    /// The temperament's table rotated onto [`Tuning::temperament_root`]:
    /// `deviation[pc] = table[(pc - root) mod 12]`, so root 0 (C, the
    /// default) is the identity and leaves every table exactly as
    /// published. `Custom` is exempt — its 12 deviations are already
    /// absolute pitch classes, per its own doc comment.
    pub fn rooted_offsets_cents(&self) -> [f32; 12] {
        let table = self.temperament.offsets_cents();
        if matches!(self.temperament, Temperament::Custom(_)) {
            return table;
        }
        let root = (self.temperament_root % 12) as usize;
        std::array::from_fn(|pc| table[(pc + 12 - root) % 12])
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
        assert_eq!(Temperament::parse("Young II"), Some(Temperament::Young2));
        assert_eq!(Temperament::parse("Rameau"), Some(Temperament::Rameau1726));
        assert_eq!(
            Temperament::parse("sixth comma meantone"),
            Some(Temperament::Meantone6)
        );
        assert_eq!(Temperament::parse("vallotti"), Some(Temperament::Vallotti));
        assert_eq!(Temperament::parse("nonsense"), None);
        assert_eq!(Temperament::parse("custom"), Some(Temperament::Custom([0.0; 12])));
    }

    /// Every catalogue entry's fifths-derived, C-rooted table (C..B,
    /// C=0) against Alex's cross-check figures, within 0.05 cents —
    /// the point of building temperaments from their fifths instead of
    /// hand-transcribing deviation tables.
    #[test]
    fn catalogue_cross_checks_against_c_rooted_reference() {
        let expected: &[(&str, [f64; 12])] = &[
            (
                "meantone4",
                [0.0, -24.0, -6.8, 10.3, -13.7, 3.4, -20.5, -3.4, -27.4, -10.3, 6.8, -17.1],
            ),
            (
                "meantone6",
                [0.0, -11.4, -3.3, 4.9, -6.5, 1.6, -9.8, -1.6, -13.0, -4.9, 3.3, -8.1],
            ),
            (
                "rameau1726",
                [0.0, -13.2, -6.8, -2.2, -13.7, 3.4, -15.2, -3.4, -11.2, -10.3, 6.8, -17.1],
            ),
            (
                "werckmeister3",
                [0.0, -9.8, -7.8, -5.9, -9.8, -2.0, -11.7, -3.9, -7.8, -11.7, -3.9, -7.8],
            ),
            (
                "kirnberger3",
                [0.0, -9.8, -6.8, -5.9, -13.7, -2.0, -9.8, -3.4, -7.8, -10.3, -3.9, -11.7],
            ),
            (
                "vallotti",
                [0.0, -5.9, -3.9, -2.0, -7.8, 2.0, -7.8, -2.0, -3.9, -5.9, 0.0, -9.8],
            ),
            (
                "young2",
                [0.0, -9.8, -3.9, -5.9, -7.8, -2.0, -11.7, -2.0, -7.8, -5.9, -3.9, -9.8],
            ),
        ];
        let catalogue = catalogue();
        for (id, table) in expected {
            let def = catalogue.iter().find(|def| def.id == *id).expect(id);
            let cr = c_rooted(class_deviations(def));
            for pc in 0..12 {
                assert!(
                    (cr[pc] - table[pc]).abs() < 0.05,
                    "{id} class {pc}: {} vs {}",
                    cr[pc],
                    table[pc]
                );
            }
        }
    }

    /// `temperament_root` rotates the table without moving the
    /// reference key: any key can anchor, and the root only decides
    /// which key gets which comma-tempered position.
    #[test]
    fn root_rotates_the_table_without_moving_the_reference() {
        let mut tuning = Tuning {
            temperament: Temperament::Meantone4,
            reference: PitchReference::A440,
            ..Tuning::default()
        };
        // A4 stays exactly 440 whatever the root.
        for root in 0..12u8 {
            tuning.temperament_root = root;
            assert!(
                (tuning.rate_multiplier(69) - 1.0).abs() < 1e-6,
                "root {root}: a′ moved"
            );
        }
        // Rooting on E (4) shifts the wolf and every other interval:
        // C's deviation under root E differs from root C (0).
        tuning.temperament_root = 0;
        let c_at_root_c = tuning.deviation_cents(60).unwrap();
        tuning.temperament_root = 4;
        let c_at_root_e = tuning.deviation_cents(60).unwrap();
        assert!(
            (c_at_root_c - c_at_root_e).abs() > 1.0,
            "rooting on E should move C's deviation: {c_at_root_c} vs {c_at_root_e}"
        );
    }

    /// The fine offset lands on every key, including under `Original`.
    #[test]
    fn fine_offset_shifts_every_key_including_original() {
        let mut tuning = Tuning { offset_cents: 25.0, ..Tuning::default() };
        assert_eq!(tuning.deviation_cents(60), Some(25.0), "original + offset");
        tuning.temperament = Temperament::Equal;
        assert_eq!(tuning.deviation_cents(60), Some(25.0), "equal + offset");
        tuning.temperament = Temperament::Meantone4;
        let base = Tuning { temperament: Temperament::Meantone4, ..Tuning::default() }
            .deviation_cents(60)
            .unwrap();
        assert!((tuning.deviation_cents(60).unwrap() - (base + 25.0)).abs() < 1e-9);
    }

    /// The UI-facing catalogue (`desktop/src/tuning/temperaments.json`)
    /// is generated from this module, not hand-maintained: this test
    /// is that generator, and the checked-in file its golden output.
    /// Run with `UPDATE_GOLDEN=1 cargo test -p aristide-server` after
    /// a catalogue change to regenerate it.
    #[test]
    fn temperament_catalogue_matches_the_generated_json() {
        let entries: Vec<serde_json::Value> = catalogue()
            .iter()
            .map(|def| {
                let offsets: Vec<f64> = c_rooted(class_deviations(def))
                    .iter()
                    .map(|c| (c * 1000.0).round() / 1000.0)
                    .collect();
                serde_json::json!({"id": def.id, "name": def.name, "offsets": offsets})
            })
            .collect();
        let json = serde_json::to_string_pretty(&entries).expect("serializes") + "\n";
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../desktop/src/tuning/temperaments.json");
        if std::env::var("UPDATE_GOLDEN").as_deref() == Ok("1") {
            std::fs::write(&path, &json).expect("writes the golden catalogue");
        } else {
            let existing = std::fs::read_to_string(&path)
                .unwrap_or_else(|err| panic!("{}: {err}", path.display()));
            assert_eq!(
                existing, json,
                "temperaments.json is stale — regenerate with \
                 `UPDATE_GOLDEN=1 cargo test -p aristide-server temperament_catalogue`"
            );
        }
    }

    /// A custom table plays exactly the deviations it was given,
    /// unrotated by any root.
    #[test]
    fn custom_temperament_plays_its_own_table_unrotated() {
        let mut offsets = [0.0f32; 12];
        offsets[1] = 17.0; // C#
        let tuning = Tuning {
            temperament: Temperament::Custom(offsets),
            temperament_root: 5, // must not rotate a custom table
            reference: PitchReference::A440,
            ..Tuning::default()
        };
        let c = tuning.deviation_cents(60).unwrap();
        let c_sharp = tuning.deviation_cents(61).unwrap();
        assert!((c_sharp - c - 17.0).abs() < 1e-6, "c={c} c#={c_sharp}");
    }
}
