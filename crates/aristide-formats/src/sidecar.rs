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
