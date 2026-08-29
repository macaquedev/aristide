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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Temperament {
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
            Temperament::Equal => "equal",
            Temperament::Werckmeister3 => "werckmeister3",
            Temperament::Kirnberger3 => "kirnberger3",
            Temperament::Meantone4 => "meantone4",
            Temperament::Pythagorean => "pythagorean",
        }
    }

    pub const ALL: [Temperament; 5] = [
        Temperament::Equal,
        Temperament::Werckmeister3,
        Temperament::Kirnberger3,
        Temperament::Meantone4,
        Temperament::Pythagorean,
    ];

    /// Deviation from equal temperament per pitch class (C = index 0),
    /// in cents, normalized so A = 0.
    pub fn offsets_cents(&self) -> [f32; 12] {
        match self {
            Temperament::Equal => [0.0; 12],
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

    /// The pitch the samples' own 12-EDO/A440 ladder gives `key`.
    fn ladder_hz(key: f64) -> f64 {
        440.0 * ((key - 69.0) / 12.0).exp2()
    }

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
            return PitchReference { key, hz: Self::ladder_hz(key as f64) };
        }
        let implied = self.implied_a4_hz();
        let allowed = implied.clamp(300.0, 500.0);
        PitchReference { key, hz: self.hz * allowed / implied }
    }

    /// How far the reference key sits from its recorded pitch, in
    /// cents — the shift every branch of [`Tuning::deviation_cents`]
    /// adds on top of its own interval arithmetic.
    fn anchor_cents(&self) -> f64 {
        1200.0 * (self.hz / Self::ladder_hz(self.key as f64)).log2()
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
}

/// The one legal range for a divisions-per-octave count: 1 (octaves
/// only) up past 311-EDO, the largest anyone names in practice.
pub const EDO_RANGE: std::ops::RangeInclusive<u16> = 1..=311;

impl Default for Tuning {
    fn default() -> Self {
        Tuning {
            temperament: Temperament::Equal,
            edo: 12,
            scale: None,
            reference: PitchReference::A440,
            transpose: 0,
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
        if let Some(scale) = &self.scale {
            let hz =
                aristide_model::scala::key_frequency(&scale.scale, &scale.mapping, key as i32)?;
            // Distance from the 12-EDO/A440 pitch this key's nominal
            // pipe was recorded at.
            let ladder_hz = 440.0 * (((key as f64) - 69.0) / 12.0).exp2();
            return Some(1200.0 * (hz / ladder_hz).log2());
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
        ((cents / 1200.0).exp2()) as f32
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
            PitchReference::ladder_hz(key as f64) * tuning.rate_multiplier(key) as f64
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

    #[test]
    fn parse_accepts_friendly_names() {
        assert_eq!(
            Temperament::parse("Werckmeister III"),
            Some(Temperament::Werckmeister3)
        );
        assert_eq!(Temperament::parse("meantone"), Some(Temperament::Meantone4));
        assert_eq!(Temperament::parse("nonsense"), None);
    }
}
