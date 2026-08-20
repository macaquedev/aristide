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
#   low, high    the keyboard's own compass as MIDI note numbers,
#                learned by playing its lowest and highest key. Notes
#                outside it are ignored; keys past the end of the sample
#                set's own compass are filled in by repitching the
#                nearest pipe. Omit both for the set's compass as-is.
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
# A [[...controls]] entry is a binding: what a message that isn't a note
# does. Both halves are text —
#
#   trigger      note:36, cc:64, program:5, or key:Equal for a computer
#                key (named by physical position)
#   action       octave-up, octave-down, transpose-up, transpose-down,
#                transpose:<n>, transpose-reset, stop:<name>,
#                coupler:<name>, tremulant, cancel, panic,
#                enclosure:<name>
#   manual       optional; which keyboard a pitch action shifts. Absent
#                means every keyboard on the same device.
#
# A [[library]] entry is one organ this machine has loaded — the
# console's picker lists them as Recent, most recent first. Removing
# one only removes it from that list; the organ's file and its
# assignments below are kept.
#
# Aristide rewrites this file whenever you change an assignment in
# Preferences → MIDI or load an organ. Hand edits are read back on the
# next start.

";

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct MidiConfig {
    /// Organs this machine has loaded, most recent first — what the
    /// console's picker offers when the server starts with nothing.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub library: Vec<LibraryEntry>,
    /// Organ name (as the loaded set reports it) → its assignments.
    #[serde(default)]
    pub organs: BTreeMap<String, OrganConfig>,
}

/// One organ the picker can load again: its name and the path that
/// produced it (a `.organ` sample set or a composite `.toml`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LibraryEntry {
    pub name: String,
    pub path: PathBuf,
}

impl MidiConfig {
    /// Put an organ at the top of the library, replacing any entry
    /// that already names its path.
    pub fn remember(&mut self, name: &str, path: &Path) {
        self.library.retain(|entry| entry.path != path);
        self.library.insert(
            0,
            LibraryEntry {
                name: name.to_string(),
                path: path.to_path_buf(),
            },
        );
    }

    /// The organ file that wraps `set`, when one exists: a composite
    /// whose one source is that set. Loading the set again means that
    /// organ — the file is the organ, the set only feeds it. Recent
    /// entries are tried first (most recently played wins), then every
    /// file in `organs_dir`: an organ removed from Recent is not gone,
    /// and reloading its set must find it rather than silently making
    /// a second organ without its name and wiring. `set` should be
    /// canonical; the candidates' sources are canonicalized to compare.
    pub fn wrapper_for(&self, set: &Path, organs_dir: Option<&Path>) -> Option<PathBuf> {
        let wraps = |candidate: &Path| {
            if !aristide_formats::instrument::is_definition(candidate) {
                return false;
            }
            let Ok(text) = std::fs::read_to_string(candidate) else {
                return false;
            };
            let Ok(def) = toml::from_str::<aristide_formats::instrument::Definition>(&text)
            else {
                return false;
            };
            if def.sources.len() != 1 {
                return false;
            }
            let source = def.sources.values().next().expect("one source");
            let resolved = if source.is_absolute() {
                source.clone()
            } else {
                candidate.parent().unwrap_or(Path::new("")).join(source)
            };
            resolved.canonicalize().ok().as_deref() == Some(set)
        };
        for entry in &self.library {
            if wraps(&entry.path) {
                return Some(entry.path.clone());
            }
        }
        let mut candidates: Vec<PathBuf> = std::fs::read_dir(organs_dir?)
            .ok()?
            .flatten()
            .map(|entry| entry.path())
            .collect();
        candidates.sort();
        candidates.into_iter().find(|path| wraps(path))
    }

    /// Drop an organ from the picker. Its assignments stay: forgetting
    /// where a set lives must not silently unwire it.
    pub fn forget(&mut self, path: &Path) -> bool {
        let before = self.library.len();
        self.library.retain(|entry| entry.path != path);
        self.library.len() != before
    }
}

#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
pub struct OrganConfig {
    /// Manual name → the inputs that play it, in the order the player
    /// added them. The order is the slot numbering the UI edits by.
    #[serde(default)]
    pub manuals: BTreeMap<String, Vec<Input>>,
    /// Everything an input does that isn't playing a note: pistons,
    /// the transposer, an expression shoe. Order is the slot numbering.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub controls: Vec<Control>,
}

/// One binding: this message, from this device, does this.
///
/// Trigger and action are stored as their text (`"note:36"`,
/// `"stop:Montre 8'"`) rather than as parsed enums, so the file stays
/// readable and a binding naming something this organ hasn't got is
/// kept and reported rather than silently dropped on the next save.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Control {
    pub device: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel: Option<u8>,
    pub trigger: String,
    pub action: String,
    /// Which manual a pitch action shifts. Absent = every keyboard on
    /// the device the trigger came from, which is what a transposer on
    /// a console means.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manual: Option<String>,
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
    /// The keyboard's own compass as MIDI notes, learned by playing its
    /// lowest and highest key. `None` = however wide the sample set's
    /// manual is.
    ///
    /// This is a fact about the *hardware*, which is why it sits on the
    /// input and not on the manual: two keyboards playing one division
    /// may well be different widths. It decides two things at once —
    /// which notes this keyboard is allowed to play (outside it,
    /// silence) and, where it reaches past the set's own compass, which
    /// keys get filled in by repitching a neighbouring pipe.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub low: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub high: Option<u8>,
    /// Semitones added to every note from this keyboard. The octave
    /// buttons move it; it belongs to the *input* because "this
    /// controller is playing an octave down" is a fact about the
    /// hardware in front of the player, not about the division.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub transpose: i8,
}

fn is_zero(value: &i8) -> bool {
    *value == 0
}

impl Input {
    /// The learned compass, lowest first, when both ends are known.
    pub fn compass(&self) -> Option<(u8, u8)> {
        match (self.low, self.high) {
            (Some(low), Some(high)) if low <= high => Some((low, high)),
            (Some(low), Some(high)) => Some((high, low)),
            _ => None,
        }
    }
}

impl MidiConfig {
    pub fn organ(&self, organ: &str) -> Option<&OrganConfig> {
        self.organs.get(organ)
    }

    pub fn controls(&self, organ: &str) -> &[Control] {
        self.organs
            .get(organ)
            .map_or(&[], |organ| organ.controls.as_slice())
    }

    /// Replace the binding at `slot`, or append when it is past the end.
    pub fn set_control(&mut self, organ: &str, slot: usize, control: Control) {
        let controls = &mut self.organs.entry(organ.to_string()).or_default().controls;
        match controls.get_mut(slot) {
            Some(existing) => *existing = control,
            None => controls.push(control),
        }
    }

    pub fn remove_control(&mut self, organ: &str, slot: usize) {
        if let Some(organ) = self.organs.get_mut(organ)
            && slot < organ.controls.len()
        {
            organ.controls.remove(slot);
        }
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

    /// The inputs of one manual, to be edited in place — what an
    /// octave button moves.
    pub fn inputs_mut(&mut self, organ: &str, manual: &str) -> &mut [Input] {
        self.organs
            .get_mut(organ)
            .and_then(|organ| organ.manuals.get_mut(manual))
            .map_or(&mut [], Vec::as_mut_slice)
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

/// Where organs born blank in the console are written:
/// `organs/` next to `midi.toml`. They are ordinary composite files —
/// the player is free to edit them or move them anywhere; the library
/// tracks them by path like any other organ.
pub fn organs_dir() -> Option<PathBuf> {
    Some(default_path()?.parent()?.join("organs"))
}

/// Create a composite file holding nothing but `name` — an organ with
/// no manuals and no stops yet, ready to load and grow. The filename
/// is a slug of the name, uniquified so creating "Chapel" twice yields
/// two files rather than one organ silently replacing another.
pub fn create_blank_organ(dir: &Path, name: &str) -> Result<PathBuf, String> {
    let (name, path) = organ_file_path(dir, name)?;
    let mut doc = toml_edit::DocumentMut::new();
    doc["name"] = toml_edit::value(name);
    let body = format!(
        "# An Aristide organ. Point [sources] at sample sets and pull stops\n\
         # or divisions onto manuals — this file is the whole instrument.\n\
         {doc}"
    );
    std::fs::write(&path, body).map_err(|err| format!("{}: {err}", path.display()))?;
    Ok(path)
}

/// A fresh file under `dir` for an organ called `name`: the filename is
/// a slug of the name, uniquified so a second organ with the same name
/// gets its own file rather than silently replacing the first.
fn organ_file_path<'a>(dir: &Path, name: &'a str) -> Result<(&'a str, PathBuf), String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("the organ needs a name".into());
    }
    std::fs::create_dir_all(dir).map_err(|err| format!("{}: {err}", dir.display()))?;
    let mut slug = String::new();
    for c in name.to_lowercase().chars() {
        if c.is_alphanumeric() {
            slug.push(c);
        } else if !slug.ends_with('-') {
            slug.push('-');
        }
    }
    let slug = slug.trim_matches('-');
    let slug = if slug.is_empty() { "organ" } else { slug };
    let mut path = dir.join(format!("{slug}.toml"));
    let mut nth = 2;
    while path.exists() {
        path = dir.join(format!("{slug}-{nth}.toml"));
        nth += 1;
    }
    Ok((name, path))
}

/// Wrap a sample set as an organ of its own: a composite file whose one
/// source is the set, carrying the set's sidecar sections verbatim (a
/// composite is its own sidecar, and the two files share those
/// sections' schema) so the organ loads exactly as the set did, plus
/// whatever `wiring` this machine already remembers for it — the file
/// owns the wiring from here on, and adopting a set must not unwire it.
pub fn create_wrapper_organ(
    dir: &Path,
    name: &str,
    set: &Path,
    wiring: Option<&OrganConfig>,
) -> Result<PathBuf, String> {
    let (name, path) = organ_file_path(dir, name)?;
    let mut doc = toml_edit::DocumentMut::new();
    doc["name"] = toml_edit::value(name);
    let mut sources = toml_edit::Table::new();
    sources.insert("s1", toml_edit::value(set.to_string_lossy().as_ref()));
    doc["sources"] = toml_edit::Item::Table(sources);
    let sidecar_path = aristide_formats::sidecar::path_for(set);
    match std::fs::read_to_string(&sidecar_path) {
        Ok(text) => match text.parse::<toml_edit::DocumentMut>() {
            Ok(sidecar) => {
                for (key, item) in sidecar.iter() {
                    // A sidecar rename became this file's own name above.
                    if key != "name" {
                        doc.insert(key, item.clone());
                    }
                }
                // The one sidecar value that is relative to the set:
                // this file doesn't live next to the set, so resolve it.
                if let Some(ir) = doc
                    .get_mut("reverb")
                    .and_then(|reverb| reverb.get_mut("ir"))
                    && let Some(spec) = ir.as_str()
                    && !spec.is_empty()
                    && !spec.eq_ignore_ascii_case("synthetic")
                    && Path::new(spec).is_relative()
                {
                    let resolved = set.parent().unwrap_or(Path::new("")).join(spec);
                    *ir = toml_edit::value(resolved.to_string_lossy().as_ref());
                }
            }
            Err(err) => {
                // The direct load ignores an unreadable sidecar too.
                tracing::warn!(
                    "sidecar not carried into the organ file: {}: {err}",
                    sidecar_path.display()
                );
            }
        },
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => {
            tracing::warn!(
                "sidecar not carried into the organ file: {}: {err}",
                sidecar_path.display()
            );
        }
    }
    let body = format!(
        "# An Aristide organ, made from the sample set under [sources].\n\
         # This file is the instrument: its name, wiring and settings\n\
         # live here, not in the set.\n\
         {doc}"
    );
    std::fs::write(&path, body).map_err(|err| format!("{}: {err}", path.display()))?;
    if wiring.is_some_and(|organ| organ != &OrganConfig::default()) {
        write_composite_midi(&path, wiring)?;
    }
    Ok(path)
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

/// A composite organ file's `[midi]` wiring in the shape the server
/// keeps it. The file is that organ's authority: this replaces whatever
/// the user config remembers under its name.
pub fn organ_config_from_file(midi: &aristide_formats::instrument::MidiDef) -> OrganConfig {
    let mut organ = OrganConfig::default();
    for input in &midi.inputs {
        organ.manuals.entry(input.manual.clone()).or_default().push(Input {
            device: input.device.clone(),
            channel: input.channel,
            low: input.low,
            high: input.high,
            transpose: input.transpose,
        });
    }
    organ.controls = midi
        .controls
        .iter()
        .map(|control| Control {
            device: control.device.clone(),
            channel: control.channel,
            trigger: control.trigger.clone(),
            action: control.action.clone(),
            manual: control.manual.clone(),
        })
        .collect();
    organ
}

/// Rewrite a composite organ file's `[[midi.input]]`/`[[midi.control]]`
/// tables to match the live assignments, touching nothing else — the
/// file is hand-authored, so its comments and layout must survive
/// every learned binding.
pub fn write_composite_midi(path: &Path, organ: Option<&OrganConfig>) -> Result<(), String> {
    let text =
        std::fs::read_to_string(path).map_err(|err| format!("{}: {err}", path.display()))?;
    let mut doc: toml_edit::DocumentMut =
        text.parse().map_err(|err| format!("{}: {err}", path.display()))?;
    let midi = doc
        .entry("midi")
        .or_insert(toml_edit::Item::Table(toml_edit::Table::new()));
    let midi = midi
        .as_table_mut()
        .ok_or_else(|| "[midi] is not a table".to_string())?;
    // Only the arrays are ours; a bare [midi] header with nothing else
    // in it would be noise.
    midi.set_implicit(true);
    let mut inputs = toml_edit::ArrayOfTables::new();
    let mut controls = toml_edit::ArrayOfTables::new();
    if let Some(organ) = organ {
        for (manual, list) in &organ.manuals {
            for input in list {
                let mut table = toml_edit::Table::new();
                table["manual"] = toml_edit::value(manual.as_str());
                table["device"] = toml_edit::value(input.device.as_str());
                if let Some(channel) = input.channel {
                    table["channel"] = toml_edit::value(channel as i64);
                }
                if let Some(low) = input.low {
                    table["low"] = toml_edit::value(low as i64);
                }
                if let Some(high) = input.high {
                    table["high"] = toml_edit::value(high as i64);
                }
                if input.transpose != 0 {
                    table["transpose"] = toml_edit::value(input.transpose as i64);
                }
                inputs.push(table);
            }
        }
        for control in &organ.controls {
            let mut table = toml_edit::Table::new();
            table["device"] = toml_edit::value(control.device.as_str());
            if let Some(channel) = control.channel {
                table["channel"] = toml_edit::value(channel as i64);
            }
            table["trigger"] = toml_edit::value(control.trigger.as_str());
            table["action"] = toml_edit::value(control.action.as_str());
            if let Some(manual) = &control.manual {
                table["manual"] = toml_edit::value(manual.as_str());
            }
            controls.push(table);
        }
    }
    if inputs.is_empty() {
        midi.remove("input");
    } else {
        midi["input"] = toml_edit::Item::ArrayOfTables(inputs);
    }
    if controls.is_empty() {
        midi.remove("control");
    } else {
        midi["control"] = toml_edit::Item::ArrayOfTables(controls);
    }
    let temporary = path.with_extension("toml.tmp");
    std::fs::write(&temporary, doc.to_string())
        .map_err(|err| format!("{}: {err}", temporary.display()))?;
    std::fs::rename(&temporary, path).map_err(|err| format!("{}: {err}", path.display()))
}

/// One manual as `save_composite` writes it: name, compass, and any
/// tuning of its own as (temperament name, a4 Hz, transpose).
pub struct SavedManual {
    pub name: String,
    pub low: u8,
    pub high: u8,
    pub tuning: Option<(String, f64, i8)>,
}

/// Write a combined instrument as a composite organ file: sources by
/// alias, every manual declared with its compass (and its own tuning
/// where it has one), the division pulls that rebuild it, the stop
/// moves on top of them, and the couplers taken off the console.
/// `[midi]` wiring is written separately (the caller follows with
/// `write_composite_midi`), and the sidecar-style sections are the
/// player's to add by hand.
pub fn save_composite(
    path: &Path,
    name: &str,
    sources: &[(String, PathBuf)],
    manuals: &[SavedManual],
    pulls: &[(usize, String, usize)],
    moves: &[(String, String, String)],
    dropped_couplers: &[String],
) -> Result<(), String> {
    let mut doc = toml_edit::DocumentMut::new();
    doc["name"] = toml_edit::value(name);
    let alias = |index: usize| format!("s{}", index + 1);
    let mut table = toml_edit::Table::new();
    for (index, (label, source)) in sources.iter().enumerate() {
        let mut entry = toml_edit::value(source.to_string_lossy().as_ref());
        if let Some(decor) = entry.as_value_mut() {
            decor.decor_mut().set_suffix(format!(" # {label}"));
        }
        table.insert(&alias(index), entry);
    }
    doc["sources"] = toml_edit::Item::Table(table);
    let mut manual_tables = toml_edit::ArrayOfTables::new();
    for manual in manuals {
        let mut table = toml_edit::Table::new();
        table["name"] = toml_edit::value(manual.name.as_str());
        table["low"] = toml_edit::value(manual.low as i64);
        table["high"] = toml_edit::value(manual.high as i64);
        if let Some((temperament, a4, transpose)) = &manual.tuning {
            table["temperament"] = toml_edit::value(temperament.as_str());
            table["a4_hz"] = toml_edit::value(*a4);
            table["transpose"] = toml_edit::value(*transpose as i64);
        }
        manual_tables.push(table);
    }
    doc["manual"] = toml_edit::Item::ArrayOfTables(manual_tables);
    let mut division_tables = toml_edit::ArrayOfTables::new();
    for (source, source_manual, target) in pulls {
        let Some(manual) = manuals.get(*target) else {
            continue;
        };
        let mut table = toml_edit::Table::new();
        table["from"] = toml_edit::value(alias(*source));
        table["manual"] = toml_edit::value(source_manual.as_str());
        table["on"] = toml_edit::value(manual.name.as_str());
        division_tables.push(table);
    }
    doc["division"] = toml_edit::Item::ArrayOfTables(division_tables);
    if !moves.is_empty() {
        let mut move_tables = toml_edit::ArrayOfTables::new();
        for (stop, from, to) in moves {
            let mut table = toml_edit::Table::new();
            table["stop"] = toml_edit::value(stop.as_str());
            table["from"] = toml_edit::value(from.as_str());
            table["to"] = toml_edit::value(to.as_str());
            move_tables.push(table);
        }
        doc["move"] = toml_edit::Item::ArrayOfTables(move_tables);
    }
    if !dropped_couplers.is_empty() {
        let mut couplers = toml_edit::Table::new();
        couplers["drop"] = toml_edit::value(
            dropped_couplers
                .iter()
                .map(|name| name.as_str())
                .collect::<toml_edit::Array>(),
        );
        doc["couplers"] = toml_edit::Item::Table(couplers);
    }
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent).map_err(|err| format!("{}: {err}", parent.display()))?;
    }
    let temporary = path.with_extension("toml.tmp");
    std::fs::write(&temporary, doc.to_string())
        .map_err(|err| format!("{}: {err}", temporary.display()))?;
    std::fs::rename(&temporary, path).map_err(|err| format!("{}: {err}", path.display()))
}

/// Update (or with `None` remove) one declared manual's compass in a
/// composite file, leaving everything else — comments included —
/// untouched. `Ok(false)` when the file declares no such manual.
pub fn write_composite_compass(
    path: &Path,
    manual: &str,
    compass: Option<(u8, u8)>,
) -> Result<bool, String> {
    edit_composite_manual(path, manual, |table| match compass {
        Some((low, high)) => {
            table["low"] = toml_edit::value(low as i64);
            table["high"] = toml_edit::value(high as i64);
        }
        None => {
            table.remove("low");
            table.remove("high");
        }
    })
}

/// Update (or with `None` remove) one declared manual's own tuning in
/// a composite file. `Ok(false)` when the file declares no such manual.
pub fn write_composite_manual_tuning(
    path: &Path,
    manual: &str,
    tuning: Option<(String, f64, i8)>,
) -> Result<bool, String> {
    edit_composite_manual(path, manual, |table| match &tuning {
        Some((temperament, a4, transpose)) => {
            table["temperament"] = toml_edit::value(temperament.as_str());
            table["a4_hz"] = toml_edit::value(*a4);
            table["transpose"] = toml_edit::value(*transpose as i64);
        }
        None => {
            table.remove("temperament");
            table.remove("a4_hz");
            table.remove("transpose");
        }
    })
}

/// Apply one edit to a named `[[manual]]` table of a composite file,
/// comment-preservingly. `Ok(false)` when no such manual is declared.
fn edit_composite_manual(
    path: &Path,
    manual: &str,
    edit: impl FnOnce(&mut toml_edit::Table),
) -> Result<bool, String> {
    let text =
        std::fs::read_to_string(path).map_err(|err| format!("{}: {err}", path.display()))?;
    let mut doc: toml_edit::DocumentMut =
        text.parse().map_err(|err| format!("{}: {err}", path.display()))?;
    let Some(tables) = doc.get_mut("manual").and_then(|m| m.as_array_of_tables_mut()) else {
        return Ok(false);
    };
    let Some(table) = tables.iter_mut().find(|table| {
        table
            .get("name")
            .and_then(|name| name.as_str())
            .is_some_and(|name| name == manual)
    }) else {
        return Ok(false);
    };
    edit(table);
    write_atomically(path, doc.to_string())?;
    Ok(true)
}

/// Append one `[[move]]` to a composite file: this stop, from this
/// manual, to that one. Appending (rather than rewriting the list)
/// keeps chains replayable — a stop moved twice moves twice.
pub fn append_composite_move(
    path: &Path,
    stop: &str,
    from: &str,
    to: &str,
) -> Result<(), String> {
    let text =
        std::fs::read_to_string(path).map_err(|err| format!("{}: {err}", path.display()))?;
    let mut doc: toml_edit::DocumentMut =
        text.parse().map_err(|err| format!("{}: {err}", path.display()))?;
    let moves = doc
        .entry("move")
        .or_insert(toml_edit::Item::ArrayOfTables(toml_edit::ArrayOfTables::new()));
    let Some(moves) = moves.as_array_of_tables_mut() else {
        return Err("[[move]] is not an array of tables".into());
    };
    let mut table = toml_edit::Table::new();
    table["stop"] = toml_edit::value(stop);
    table["from"] = toml_edit::value(from);
    table["to"] = toml_edit::value(to);
    moves.push(table);
    write_atomically(path, doc.to_string())
}

/// Replace the `[couplers] drop` list of a composite file with the
/// couplers currently off the console; an empty pick removes the key.
pub fn write_composite_drops(path: &Path, dropped: &[String]) -> Result<(), String> {
    let text =
        std::fs::read_to_string(path).map_err(|err| format!("{}: {err}", path.display()))?;
    let mut doc: toml_edit::DocumentMut =
        text.parse().map_err(|err| format!("{}: {err}", path.display()))?;
    let couplers = doc
        .entry("couplers")
        .or_insert(toml_edit::Item::Table(toml_edit::Table::new()));
    let Some(couplers) = couplers.as_table_mut() else {
        return Err("[couplers] is not a table".into());
    };
    couplers.set_implicit(true);
    if dropped.is_empty() {
        couplers.remove("drop");
    } else {
        couplers["drop"] = toml_edit::value(
            dropped
                .iter()
                .map(|name| name.as_str())
                .collect::<toml_edit::Array>(),
        );
    }
    write_atomically(path, doc.to_string())
}

/// Rename a composite organ in its own file: only the `name` key
/// changes, every other line — comments included — survives. The file
/// itself stays where it is, so the library and anything else that
/// refers to it by path keeps working.
pub fn write_composite_name(path: &Path, name: &str) -> Result<(), String> {
    let text =
        std::fs::read_to_string(path).map_err(|err| format!("{}: {err}", path.display()))?;
    let mut doc: toml_edit::DocumentMut =
        text.parse().map_err(|err| format!("{}: {err}", path.display()))?;
    doc["name"] = toml_edit::value(name);
    write_atomically(path, doc.to_string())
}

/// Rename a sample-set organ without touching the set: the name goes
/// into the set's Aristide sidecar (created if the set has none),
/// where the loader reads it back as the organ's name. Any existing
/// sidecar content is edited in place, comments kept.
pub fn write_sidecar_name(set: &Path, name: &str) -> Result<(), String> {
    let path = aristide_formats::sidecar::path_for(set);
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(err) => return Err(format!("{}: {err}", path.display())),
    };
    let mut doc: toml_edit::DocumentMut =
        text.parse().map_err(|err| format!("{}: {err}", path.display()))?;
    doc["name"] = toml_edit::value(name);
    write_atomically(&path, doc.to_string())
}

fn write_atomically(path: &Path, body: String) -> Result<(), String> {
    let temporary = path.with_extension("toml.tmp");
    std::fs::write(&temporary, body)
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
            low: None,
            high: None,
            transpose: 0,
        }
    }

    /// Learned wiring lands in the composite's own file — new
    /// `[[midi.input]]`/`[[midi.control]]` tables — while every
    /// comment and unrelated section survives untouched, and the
    /// result reads back as the same wiring.
    #[test]
    fn composite_midi_write_back_preserves_the_rest_of_the_file() {
        let path = std::env::temp_dir().join("aristide-composite-test.toml");
        std::fs::write(
            &path,
            "# my precious hand-written organ\nname = \"Franken\"\n\n\
             [sources]\nanne = \"demo.organ\" # the good one\n\n\
             [[midi.input]]\nmanual = \"Old\"\ndevice = \"Gone\"\n",
        )
        .expect("fixture writes");
        let mut organ = OrganConfig::default();
        organ.manuals.insert(
            "Great".into(),
            vec![Input {
                device: "KeyLab 61".into(),
                channel: Some(1),
                low: Some(36),
                high: Some(96),
                transpose: -12,
            }],
        );
        organ.controls.push(Control {
            device: "KeyLab 61".into(),
            channel: None,
            trigger: "cc:64".into(),
            action: "tremulant".into(),
            manual: None,
        });
        write_composite_midi(&path, Some(&organ)).expect("writes back");
        let text = std::fs::read_to_string(&path).expect("reads");
        assert!(text.contains("# my precious hand-written organ"));
        assert!(text.contains("# the good one"));
        assert!(!text.contains("Gone"), "stale wiring replaced");
        let definition: aristide_formats::instrument::Definition =
            toml::from_str(&text).expect("still a valid organ file");
        assert_eq!(definition.midi.inputs.len(), 1);
        assert_eq!(definition.midi.inputs[0].transpose, -12);
        assert_eq!(organ_config_from_file(&definition.midi), organ);
        // Wiring emptied: the arrays vanish rather than lingering as [].
        write_composite_midi(&path, None).expect("writes back empty");
        let text = std::fs::read_to_string(&path).expect("reads");
        assert!(!text.contains("midi"));
        assert!(text.contains("# my precious hand-written organ"));
        let _ = std::fs::remove_file(&path);
    }

    /// Adopting a set makes a real organ file: the set as the one
    /// source, the sidecar's sections carried in (its rename becoming
    /// the file's own name, its relative reverb IR resolved — the file
    /// doesn't live next to the set), and the wiring this machine
    /// already had for the organ, so adoption never unwires anything.
    #[test]
    fn a_wrapper_organ_carries_the_sidecar_and_the_wiring() {
        let dir = std::env::temp_dir().join("aristide-wrapper-test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("fixture dir");
        let set = dir.join("village.organ");
        std::fs::write(&set, "[Organ]").expect("fixture set");
        std::fs::write(
            aristide_formats::sidecar::path_for(&set),
            "name = \"Chapelle\"\n\n[wind]\n# gentle bellows\nsag_cents = 3.0\n\n\
             [reverb]\nir = \"hall.wav\"\nwet = 0.2\n",
        )
        .expect("fixture sidecar");
        let mut wiring = OrganConfig::default();
        wiring
            .manuals
            .insert("Great".into(), vec![input("Test Keys", Some(1))]);

        let organs = dir.join("organs");
        let path = create_wrapper_organ(&organs, "Chapelle", &set, Some(&wiring))
            .expect("wrapper created");
        let text = std::fs::read_to_string(&path).expect("reads");
        assert!(text.contains("# gentle bellows"), "sidecar comments ride along");
        let def: aristide_formats::instrument::Definition =
            toml::from_str(&text).expect("a valid organ file");
        assert_eq!(def.name, "Chapelle", "one name — the file's");
        assert_eq!(def.sources.len(), 1);
        assert_eq!(def.sources.values().next().expect("source"), &set);
        assert_eq!(def.wind.sag_cents, 3.0, "engine settings carried in");
        assert_eq!(
            std::path::Path::new(&def.reverb.ir),
            dir.join("hall.wav"),
            "the set-relative IR is resolved"
        );
        assert_eq!(def.midi.inputs.len(), 1, "existing wiring carried in");
        assert_eq!(def.midi.inputs[0].device, "Test Keys");

        // A set with no sidecar wraps to just a name and a source.
        let bare_set = dir.join("bare.organ");
        std::fs::write(&bare_set, "[Organ]").expect("fixture set");
        let bare = create_wrapper_organ(&organs, "Bare", &bare_set, None)
            .expect("wrapper created");
        let def: aristide_formats::instrument::Definition =
            toml::from_str(&std::fs::read_to_string(&bare).expect("reads"))
                .expect("a valid organ file");
        assert_eq!(def.name, "Bare");
        assert!(def.midi.inputs.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Loading a set that already has an organ file means that organ:
    /// the library lookup finds the composite whose one source is the
    /// set, and ignores everything else.
    #[test]
    fn the_library_finds_the_wrapper_for_a_set() {
        let dir = std::env::temp_dir().join("aristide-wrapper-lookup-test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("fixture dir");
        let set = dir.join("village.organ");
        std::fs::write(&set, "[Organ]").expect("fixture set");
        let canonical = set.canonicalize().expect("canonicalizes");
        let wrapper = create_wrapper_organ(&dir.join("organs"), "Chapelle", &canonical, None)
            .expect("wrapper created");
        // Distractors: a multi-source composite and a plain set entry.
        let combo = dir.join("combo.toml");
        std::fs::write(
            &combo,
            format!(
                "name = \"Combo\"\n[sources]\na = {:?}\nb = \"other.organ\"\n",
                canonical.display().to_string()
            ),
        )
        .expect("fixture combo");

        let mut config = MidiConfig::default();
        config.remember("Chapelle", &wrapper);
        config.remember("Combo", &combo);
        config.remember("Raw", &canonical);
        assert_eq!(config.wrapper_for(&canonical, None), Some(wrapper.clone()));
        assert_eq!(
            config.wrapper_for(&dir.join("elsewhere.organ"), None),
            None,
            "nothing wraps a set this machine has never seen"
        );

        // Removed from Recent is not gone: the organs folder still
        // holds the file, and reloading the set finds it there rather
        // than making a second organ without its name and wiring.
        config.forget(&wrapper);
        assert_eq!(config.wrapper_for(&canonical, None), None);
        assert_eq!(
            config.wrapper_for(&canonical, Some(&dir.join("organs"))),
            Some(wrapper)
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Renaming a composite touches the `name` key and nothing else;
    /// renaming a set writes its sidecar — creating one when the set
    /// has none, editing in place (comments kept) when it has.
    #[test]
    fn rename_writers_change_the_name_and_keep_the_rest() {
        let dir = std::env::temp_dir().join("aristide-rename-writers-test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("fixture dir");

        let composite = dir.join("organ.toml");
        std::fs::write(
            &composite,
            "# hand-written\nname = \"Old Name\"\n\n[sources]\na = \"demo.organ\" # keep me\n",
        )
        .expect("fixture writes");
        write_composite_name(&composite, "New Name").expect("renames");
        let text = std::fs::read_to_string(&composite).expect("reads");
        assert!(text.contains("name = \"New Name\""));
        assert!(!text.contains("Old Name"));
        assert!(text.contains("# hand-written") && text.contains("# keep me"));

        let set = dir.join("village.organ");
        write_sidecar_name(&set, "Chapel").expect("creates a sidecar");
        let sidecar_path = aristide_formats::sidecar::path_for(&set);
        let sidecar: aristide_formats::sidecar::Sidecar =
            toml::from_str(&std::fs::read_to_string(&sidecar_path).expect("reads"))
                .expect("parses as a sidecar");
        assert_eq!(sidecar.name, "Chapel");

        // A second rename edits the sidecar it made — or any the set
        // already had — without disturbing other keys.
        std::fs::write(
            &sidecar_path,
            "# per-set notes\nname = \"Chapel\"\n\n[wind]\nsag_cents = 3.0\n",
        )
        .expect("fixture writes");
        write_sidecar_name(&set, "Chapel Royal").expect("edits the sidecar");
        let text = std::fs::read_to_string(&sidecar_path).expect("reads");
        assert!(text.contains("name = \"Chapel Royal\""));
        assert!(text.contains("# per-set notes") && text.contains("sag_cents = 3.0"));
        let _ = std::fs::remove_dir_all(&dir);
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

    /// The file a blank organ is born as must itself be a loadable
    /// composite: the exact name kept, no manuals, no stops — and a
    /// second organ with the same name gets its own file.
    #[test]
    fn a_blank_organ_is_a_loadable_composite() {
        let dir = std::env::temp_dir().join("aristide-blank-organ-test");
        let _ = std::fs::remove_dir_all(&dir);

        assert!(create_blank_organ(&dir, "   ").is_err(), "a name is required");

        let path = create_blank_organ(&dir, "Église St-Jean").expect("creates");
        assert_eq!(path.file_name().unwrap().to_str(), Some("église-st-jean.toml"));
        let assembled = aristide_formats::instrument::load(&path).expect("loads");
        assert_eq!(assembled.organ.name, "Église St-Jean");
        assert!(assembled.organ.manuals.is_empty());
        assert!(assembled.organ.stops.is_empty());

        let second = create_blank_organ(&dir, "Église St-Jean").expect("creates again");
        assert_ne!(second, path, "same name, own file");
        assert!(second.is_file());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_missing_file_is_an_empty_config() {
        let path = std::env::temp_dir().join("aristide-midi-absent.toml");
        std::fs::remove_file(&path).ok();
        assert!(load(&path).expect("missing is fine").organs.is_empty());
    }

    /// The picker's memory: most recent first, one entry per path, and
    /// it survives the TOML round trip alongside the assignments.
    #[test]
    fn the_library_remembers_most_recent_first() {
        let mut config = MidiConfig::default();
        config.remember("A", Path::new("/sets/a.organ"));
        config.remember("B", Path::new("/sets/b.toml"));
        config.remember("A renamed", Path::new("/sets/a.organ"));
        let names: Vec<&str> = config.library.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["A renamed", "B"], "reloading moves an organ up");

        let text = toml::to_string_pretty(&config).expect("serializes");
        let back: MidiConfig = toml::from_str(&text).expect("parses");
        assert_eq!(back.library, config.library);

        assert!(config.forget(Path::new("/sets/b.toml")));
        assert!(!config.forget(Path::new("/sets/b.toml")), "already gone");
        assert_eq!(config.library.len(), 1);
    }
}
