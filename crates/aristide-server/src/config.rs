//! Which physical input plays what, remembered **per organ** in one
//! human-editable file under the user's config directory.
//!
//! Two facts are being joined here and they belong to different places.
//! A port name (`"Midiplus AKM320 MIDI 1"`) is a fact about *this
//! machine* — it differs between the user's desktop and this box, and
//! changes when a cable moves. A manual name (`"Récit"`) is a fact about
//! *the organ* — it doesn't exist at all on a set that calls its
//! keyboards First and Second. Sample-set sidecars are meant to travel
//! with a set, so hardware names must not go in them; this file is the
//! machine's half, keyed by organ so one rig can drive many instruments
//! differently.
//!
//! An organ the file has never seen has no assignments, and an
//! unassigned input is silent (see `Route`). That is deliberate: the
//! alternative — guessing from MIDI channels — is what makes a strange
//! keyboard blast a random division the first time it is plugged in.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Written above the serialized tables so the file explains itself to
/// anyone who opens it in an editor.
const HEADER: &str = "\
# Aristide — MIDI input assignments, per organ.
#
# Each input device is named by its MIDI port, exactly as the operating
# system reports it. Its value is one of:
#
#   \"channels\"     obey the organ's channel map (a console whose
#                  manuals speak on separate MIDI channels)
#   \"<manual>\"     pin every note from this device to that manual
#                  (a plain keyboard that only ever sends one channel)
#
# A device that is not listed here is unassigned and stays silent.
# Manual names are matched against the loaded organ's own names, so a
# renamed or missing manual leaves the device unassigned rather than
# playing the wrong division.
#
# Aristide rewrites this file whenever you change an assignment in
# Preferences → MIDI. Hand edits are read back on the next start.

";

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct MidiConfig {
    /// Organ name (as the loaded set reports it) → its assignments.
    #[serde(default)]
    pub organs: BTreeMap<String, OrganConfig>,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct OrganConfig {
    /// Port name → `"channels"` or a manual name.
    #[serde(default)]
    pub devices: BTreeMap<String, String>,
    /// The 16-channel map as manual indices; empty = the organ's default.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub channels: Vec<usize>,
}

/// The value stored for a device that follows the channel map. Manual
/// names are stored verbatim, so this is the one reserved word.
pub const FOLLOW_CHANNELS: &str = "channels";

impl MidiConfig {
    pub fn organ(&self, organ: &str) -> Option<&OrganConfig> {
        self.organs.get(organ)
    }

    /// Record one device's assignment. `None` removes it — an organ's
    /// table lists only what the player has actually assigned.
    pub fn set_device(&mut self, organ: &str, port: &str, assignment: Option<&str>) {
        let entry = self.organs.entry(organ.to_string()).or_default();
        match assignment {
            Some(value) => {
                entry.devices.insert(port.to_string(), value.to_string());
            }
            None => {
                entry.devices.remove(port);
            }
        }
    }

    pub fn set_channels(&mut self, organ: &str, channels: Vec<usize>) {
        self.organs.entry(organ.to_string()).or_default().channels = channels;
    }
}

/// `$XDG_CONFIG_HOME/aristide/midi.toml`, else `~/.config/…`. `None`
/// when neither is set, in which case nothing is persisted and the
/// server says so once.
pub fn default_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))?;
    Some(base.join("aristide").join("midi.toml"))
}

/// A missing file is not an error: it is the state before the player
/// has assigned anything. A malformed one is kept, not overwritten —
/// losing someone's hand edits to a typo would be rude.
pub fn load(path: &Path) -> Result<MidiConfig, String> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(MidiConfig::default()),
        Err(err) => return Err(format!("{}: {err}", path.display())),
    };
    toml::from_str(&text).map_err(|err| format!("{}: {err}", path.display()))
}

/// Write via a temporary file and rename, so a crash mid-write cannot
/// leave a half-written config behind.
pub fn save(path: &Path, config: &MidiConfig) -> Result<(), String> {
    let body = toml::to_string_pretty(config).map_err(|err| err.to_string())?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|err| format!("{}: {err}", parent.display()))?;
    }
    let temporary = path.with_extension("toml.tmp");
    std::fs::write(&temporary, format!("{HEADER}{body}"))
        .map_err(|err| format!("{}: {err}", temporary.display()))?;
    std::fs::rename(&temporary, path).map_err(|err| format!("{}: {err}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assignments_round_trip_per_organ() {
        let mut config = MidiConfig::default();
        config.set_device("Friesach", "Johannus DIN IN", Some(FOLLOW_CHANNELS));
        config.set_device("Friesach", "AKM320 MIDI 1", Some("Second Manual"));
        config.set_channels("Friesach", vec![1, 2, 0]);
        config.set_device("Sankt Nikolaus", "AKM320 MIDI 1", Some("Récit"));

        let path = std::env::temp_dir().join("aristide-midi-test.toml");
        save(&path, &config).expect("config saves");
        let text = std::fs::read_to_string(&path).expect("written");
        assert!(text.starts_with("# Aristide"), "header explains the file");

        let read = load(&path).expect("config loads");
        let friesach = read.organ("Friesach").expect("organ remembered");
        assert_eq!(friesach.devices["AKM320 MIDI 1"], "Second Manual");
        assert_eq!(friesach.devices["Johannus DIN IN"], FOLLOW_CHANNELS);
        assert_eq!(friesach.channels, vec![1, 2, 0]);
        // Assignments are per organ: the same keyboard plays a different
        // manual on a different instrument, and knows nothing about an
        // organ that was never configured.
        assert_eq!(
            read.organs["Sankt Nikolaus"].devices["AKM320 MIDI 1"],
            "Récit"
        );
        assert!(read.organ("Some Other Organ").is_none());

        let mut cleared = read;
        cleared.set_device("Friesach", "AKM320 MIDI 1", None);
        assert!(
            !cleared.organs["Friesach"]
                .devices
                .contains_key("AKM320 MIDI 1")
        );
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn a_missing_file_is_an_empty_config() {
        let path = std::env::temp_dir().join("aristide-midi-absent.toml");
        std::fs::remove_file(&path).ok();
        assert!(load(&path).expect("missing is fine").organs.is_empty());
    }
}
