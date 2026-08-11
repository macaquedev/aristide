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
    #[serde(default)]
    pub midi: Midi,
    #[serde(default)]
    pub registration: Registration,
    #[serde(default)]
    pub wind: Wind,
    #[serde(default)]
    pub tremulant: Tremulant,
    #[serde(default)]
    pub tuning: TuningConfig,
    #[serde(default)]
    pub reverb: ReverbConfig,
    #[serde(default)]
    pub noises: NoisesConfig,
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
    #[serde(default = "default_temperament")]
    pub temperament: String,
    #[serde(default = "default_a4_hz")]
    pub a4_hz: f64,
    /// Semitones added to incoming keys (a transposer).
    #[serde(default)]
    pub transpose: i8,
}

fn default_temperament() -> String {
    "equal".into()
}

fn default_a4_hz() -> f64 {
    440.0
}

impl Default for TuningConfig {
    fn default() -> Self {
        TuningConfig {
            temperament: default_temperament(),
            a4_hz: default_a4_hz(),
            transpose: 0,
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
