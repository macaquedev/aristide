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
//! The table is written **manual first**: an organ's manual lists the
//! inputs that play it. That direction is the one the player thinks in
//! ("what drives the Récit?"), and it is the one that generalises — a
//! manual holds a *list*, so two keyboards can share one division, and
//! each entry has room to grow a key range or a transposition without
//! disturbing anything else.
//!
//! A manual with no inputs is silent, and that is the default for an
//! organ the file has never seen. That is deliberate: the alternative —
//! guessing from MIDI channels — is what makes a strange keyboard blast
//! a random division the first time it is plugged in.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Written above the serialized tables so the file explains itself to
/// anyone who opens it in an editor.
const HEADER: &str = "\
# Aristide — MIDI input assignments, per organ.
#
# Each entry gives one of an organ's manuals an input to listen to:
#
#   device       the MIDI port, named exactly as the operating system
#                reports it
#   channel      1-16; omit it to accept every channel (a plain USB
#                keyboard that only ever sends one)
#
# A manual may list several inputs — two keyboards playing one division
# is a valid thing to want — and one device may drive several manuals by
# giving each a different channel, which is how a DIN console with its
# manuals on separate channels is set up.
#
# A manual that is not listed here has no input and stays silent. Manual
# names are matched against the loaded organ's own names, so a renamed
# or missing manual drops its inputs rather than playing the wrong
# division.
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
    /// Manual name → the inputs that play it, in the order the player
    /// added them. The order is the slot numbering the UI edits by.
    #[serde(default)]
    pub manuals: BTreeMap<String, Vec<Input>>,
}

/// One source of notes for one manual.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Input {
    /// MIDI port name, as the OS reports it. Kept even while the device
    /// is unplugged — a config that forgets a keyboard because it was
    /// off at startup would be worse than useless.
    pub device: String,
    /// MIDI channel 1-16, as printed on hardware. `None` = any channel.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel: Option<u8>,
}

impl MidiConfig {
    pub fn organ(&self, organ: &str) -> Option<&OrganConfig> {
        self.organs.get(organ)
    }

    /// Every (manual name, inputs) pair saved for one organ.
    pub fn assignments(&self, organ: &str) -> impl Iterator<Item = (&str, &[Input])> {
        self.organs
            .get(organ)
            .into_iter()
            .flat_map(|organ| organ.manuals.iter())
            .map(|(name, inputs)| (name.as_str(), inputs.as_slice()))
    }

    pub fn inputs(&self, organ: &str, manual: &str) -> &[Input] {
        self.organs
            .get(organ)
            .and_then(|organ| organ.manuals.get(manual))
            .map_or(&[], Vec::as_slice)
    }

    /// Replace the input at `slot`, or append when `slot` is past the
    /// end — one call covers "change this row" and "add a row".
    pub fn set_input(&mut self, organ: &str, manual: &str, slot: usize, input: Input) {
        let inputs = self
            .organs
            .entry(organ.to_string())
            .or_default()
            .manuals
            .entry(manual.to_string())
            .or_default();
        match inputs.get_mut(slot) {
            Some(existing) => *existing = input,
            None => inputs.push(input),
        }
    }

    /// Remove one input. A manual left with none drops out of the file
    /// entirely: unassigned is the absence of an entry, not an entry
    /// saying nothing.
    pub fn remove_input(&mut self, organ: &str, manual: &str, slot: usize) {
        let Some(organ_config) = self.organs.get_mut(organ) else {
            return;
        };
        let Some(inputs) = organ_config.manuals.get_mut(manual) else {
            return;
        };
        if slot < inputs.len() {
            inputs.remove(slot);
        }
        if inputs.is_empty() {
            organ_config.manuals.remove(manual);
        }
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

    fn input(device: &str, channel: Option<u8>) -> Input {
        Input {
            device: device.into(),
            channel,
        }
    }

    #[test]
    fn assignments_round_trip_per_organ() {
        let mut config = MidiConfig::default();
        // One DIN console split across channels, plus a USB keyboard
        // doubling the Great — the two shapes the file has to hold.
        let din = input("Johannus DIN IN", Some(1));
        let usb = input("AKM320 MIDI 1", None);
        config.set_input("Friesach", "First Manual", 0, din);
        config.set_input("Friesach", "First Manual", 1, usb.clone());
        let din2 = input("Johannus DIN IN", Some(2));
        config.set_input("Friesach", "Second Manual", 0, din2);
        config.set_input("Sankt Nikolaus", "Récit", 0, usb);

        let path = std::env::temp_dir().join("aristide-midi-test.toml");
        save(&path, &config).expect("config saves");
        let text = std::fs::read_to_string(&path).expect("written");
        assert!(text.starts_with("# Aristide"), "header explains the file");

        let read = load(&path).expect("config loads");
        assert_eq!(
            read.inputs("Friesach", "First Manual"),
            [
                input("Johannus DIN IN", Some(1)),
                input("AKM320 MIDI 1", None),
            ]
        );
        assert_eq!(
            read.inputs("Friesach", "Second Manual"),
            [input("Johannus DIN IN", Some(2))]
        );
        // Assignments are per organ: the same keyboard plays a different
        // manual on a different instrument, and knows nothing about an
        // organ that was never configured.
        assert_eq!(
            read.inputs("Sankt Nikolaus", "Récit"),
            [input("AKM320 MIDI 1", None)]
        );
        assert!(read.organ("Some Other Organ").is_none());
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn a_slot_past_the_end_appends_and_the_last_removal_clears_the_manual() {
        let mut config = MidiConfig::default();
        config.set_input("Organ", "Great", 7, input("Keyboard", None));
        let great = config.inputs("Organ", "Great");
        assert_eq!(great.len(), 1, "a slot past the end appends, not sparse");

        config.set_input("Organ", "Great", 0, input("Keyboard", Some(3)));
        assert_eq!(config.inputs("Organ", "Great"), [input("Keyboard", Some(3))]);

        config.remove_input("Organ", "Great", 0);
        assert!(config.inputs("Organ", "Great").is_empty());
        assert!(
            !config.organs["Organ"].manuals.contains_key("Great"),
            "a manual with no inputs leaves no entry behind"
        );
        config.remove_input("Organ", "Great", 0);
    }

    #[test]
    fn a_missing_file_is_an_empty_config() {
        let path = std::env::temp_dir().join("aristide-midi-absent.toml");
        std::fs::remove_file(&path).ok();
        assert!(load(&path).expect("missing is fine").organs.is_empty());
    }
}
