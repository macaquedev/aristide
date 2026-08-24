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
    /// (chromatic degrees, a′ anchored at the tuning's `a4_hz`).
    pub kbm: Option<String>,
    pub scale: aristide_model::scala::Scale,
    pub mapping: aristide_model::scala::KeyboardMapping,
}

impl ScaleTuning {
    /// Read and parse a scale (and optionally its keyboard mapping)
    /// from disk, `base`-relative for relative paths. `a4_hz` anchors
    /// the linear default mapping when no `.kbm` is given; an explicit
    /// mapping carries its own reference and ignores it.
    pub fn load(
        scl: &str,
        kbm: Option<&str>,
        a4_hz: f64,
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
            None => aristide_model::scala::KeyboardMapping::linear(a4_hz),
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

/// The live tuning state: temperament + concert pitch + transposition,
/// or a Scala scale standing in for the temperament.
#[derive(Debug, Clone)]
pub struct Tuning {
    pub temperament: Temperament,
    /// When present, the scale supplies every key's pitch and the
    /// temperament above is dormant — a Scala scale IS a temperament,
    /// just one with its own degree count and period.
    pub scale: Option<std::sync::Arc<ScaleTuning>>,
    /// Frequency of a′ in Hz (440 = modern concert pitch; 415 baroque,
    /// 465 chorton, …). Under a scale with its own `.kbm` the mapping's
    /// reference frequency governs instead.
    pub a4_hz: f64,
    /// Semitones added to incoming keys before routing — a transposer
    /// selects different pipes, like the real console gadget.
    pub transpose: i8,
}

impl Default for Tuning {
    fn default() -> Self {
        Tuning {
            temperament: Temperament::Equal,
            scale: None,
            a4_hz: 440.0,
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
        let class = (key % 12) as usize;
        Some(
            self.temperament.offsets_cents()[class] as f64
                + 1200.0 * (self.a4_hz / 440.0).log2(),
        )
    }

    /// Keep the linear default mapping anchored to `a4_hz` after an a′
    /// change — an explicit `.kbm` owns its own reference and stays.
    pub fn refresh_scale_reference(&mut self) {
        if let Some(scale) = &self.scale
            && scale.kbm.is_none()
            && scale.mapping.reference_hz != self.a4_hz
        {
            let mut refreshed = (**scale).clone();
            refreshed.mapping = aristide_model::scala::KeyboardMapping::linear(self.a4_hz);
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
        tuning.a4_hz = 415.0;
        let expected = (415.0f64 / 440.0) as f32;
        assert!((tuning.rate_multiplier(69) - expected).abs() < 1e-4);
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
