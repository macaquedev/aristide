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
# is a valid thing to want — and one device may drive several manuals:
# with a different channel each, which is how a DIN console with its
# manuals on separate channels is set up, or outright, one keyboard
# sounding two divisions at once. The console asks before creating the
# latter; written here by hand it is simply believed.
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
#                coupler:<name>, tremulant, tremulant:<name>,
#                general:<n>, set, cancel, panic, enclosure:<name>
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
    /// organ — the file is the organ, the set only feeds it. Several
    /// organs can share one set (the adopted wrapper plus anything
    /// built on the console from it), so `name` — the library entry
    /// the player actually clicked — decides between them: an organ
    /// named exactly that wins. Without a name match, only *adopted*
    /// wrappers qualify — `layout = true` on the source (adoption
    /// writes it), or a bare file with no structure of its own (how
    /// adoption wrote them before the inventory). An organ merely
    /// *built from* the set — its own manuals, its own pulls — is
    /// reached by its name, never by naming the raw set: browsing to
    /// the set means the set's own organ, not silently the most
    /// recent thing built on it. Among wrappers, Recent order decides
    /// (most recently played wins), then every file in `organs_dir`:
    /// an organ removed from Recent is not gone, and reloading its
    /// set must find it rather than silently making a second organ
    /// without its name and wiring. `set` should be canonical; the
    /// candidates' sources are canonicalized to compare.
    pub fn wrapper_for(
        &self,
        set: &Path,
        name: Option<&str>,
        organs_dir: Option<&Path>,
    ) -> Option<PathBuf> {
        let wraps = |candidate: &Path| -> Option<(String, bool)> {
            if !aristide_formats::instrument::is_definition(candidate) {
                return None;
            }
            let text = std::fs::read_to_string(candidate).ok()?;
            let def = toml::from_str::<aristide_formats::instrument::Definition>(&text).ok()?;
            if def.sources.len() != 1 {
                return None;
            }
            let source = def.sources.values().next().expect("one source");
            let path = source.path();
            let resolved = if path.is_absolute() {
                path.to_path_buf()
            } else {
                candidate.parent().unwrap_or(Path::new("")).join(path)
            };
            if resolved.canonicalize().ok().as_deref() != Some(set) {
                return None;
            }
            let adopted = source.layout()
                || (def.manuals.is_empty()
                    && def.divisions.is_empty()
                    && def.stops.is_empty()
                    && def.moves.is_empty());
            Some((def.name, adopted))
        };
        let mut fallback: Option<PathBuf> = None;
        let mut candidates: Vec<PathBuf> =
            self.library.iter().map(|entry| entry.path.clone()).collect();
        if let Some(dir) = organs_dir {
            let mut on_disk: Vec<PathBuf> = std::fs::read_dir(dir)
                .into_iter()
                .flatten()
                .flatten()
                .map(|entry| entry.path())
                .collect();
            on_disk.sort();
            candidates.extend(on_disk);
        }
        for candidate in candidates {
            let Some((organ_name, adopted)) = wraps(&candidate) else {
                continue;
            };
            if name.is_some_and(|wanted| organ_name == wanted) {
                return Some(candidate);
            }
            if adopted {
                fallback.get_or_insert(candidate);
            }
        }
        fallback
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
    /// Recallable registrations — the combination action's generals,
    /// keyed by piston slot. Stored as names (the text vocabulary
    /// bindings use), so a combination survives a rename honestly:
    /// anything the loaded organ hasn't got is reported and skipped,
    /// never silently dropped from the file.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub generals: std::collections::BTreeMap<u8, General>,
}

/// One stored general: the console state a piston brings back.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct General {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub stops: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub couplers: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tremulants: Vec<String>,
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
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
    /// Pitch-bend range in semitones: how far this keyboard's bend
    /// messages reach at full deflection, applied per channel to the
    /// notes that channel is holding (which is what makes an MPE
    /// controller's per-note bends work — each member channel carries
    /// one note). Absent = bends are ignored, as an organ console
    /// ignores them; MPE members conventionally use 48.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bend: Option<f32>,
    /// Lumatone key-mapping file (`.ltn`) for a generalized keyboard:
    /// the map decides which (channel, note) pairs this input plays and
    /// which manual key each addresses, in place of the channel/compass
    /// fields above. Resolved against the organ file's directory; a map
    /// that fails to load warns and leaves the input deaf, never
    /// bricking the organ.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub map: Option<String>,
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

/// Where decoded-sample caches live: `cache/` next to `midi.toml`.
/// One file per (source set, residency) combination; safe to delete
/// wholesale — the next load simply decodes.
pub fn cache_dir() -> Option<PathBuf> {
    Some(default_path()?.parent()?.join("cache"))
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

/// Adopt a sample set as an organ of its own: a composite file that
/// *inventories* the set — every manual declared with its compass,
/// every stop an explicit pull, every coupler a `[[couplers.define]]`
/// — so the file, not the set, is the instrument from here on. The set
/// stays what the pulls read pipes and samples from (`layout = true`
/// keeps its windchest and enclosure numbering whole); its opinions
/// about the console are snapshotted here and never consulted again.
/// The set's sidecar sections are carried verbatim (a composite is its
/// own sidecar), plus whatever `wiring` this machine already remembers
/// — the file owns the wiring from here on, and adopting a set must
/// not unwire it. Proven byte-equivalent to the direct load by test.
pub fn create_wrapper_organ(
    dir: &Path,
    name: &str,
    set: &Path,
    organ: &aristide_model::Organ,
    wiring: Option<&OrganConfig>,
) -> Result<PathBuf, String> {
    let (name, path) = organ_file_path(dir, name)?;
    let mut doc = toml_edit::DocumentMut::new();
    doc["name"] = toml_edit::value(name);
    let mut sources = toml_edit::Table::new();
    let mut source = toml_edit::InlineTable::new();
    source.insert("path", set.to_string_lossy().as_ref().into());
    source.insert("layout", true.into());
    sources.insert("s1", toml_edit::value(source));
    doc["sources"] = toml_edit::Item::Table(sources);

    let manual_name = |id: aristide_model::ManualId| {
        organ
            .manuals
            .iter()
            .find(|manual| manual.id == id)
            .map(|manual| manual.name.as_str())
    };

    let mut manuals = toml_edit::ArrayOfTables::new();
    for manual in &organ.manuals {
        let mut table = toml_edit::Table::new();
        table["name"] = toml_edit::value(manual.name.as_str());
        if manual.kind != aristide_model::ManualKind::Manual {
            table["kind"] = toml_edit::value(manual.kind.as_str());
        }
        table["low"] = toml_edit::value(manual.first_midi_note as i64);
        table["high"] = toml_edit::value(
            (manual.first_midi_note as i64 + manual.key_count as i64 - 1).clamp(0, 127),
        );
        manuals.push(table);
    }
    if !manuals.is_empty() {
        doc["manual"] = toml_edit::Item::ArrayOfTables(manuals);
    }

    // One pull per stop, in the set's own order. Exact names match
    // exactly (and all together): two same-named stops on one manual
    // are one line pulling both, so the line is written once.
    let mut stops = toml_edit::ArrayOfTables::new();
    let mut written: std::collections::HashSet<(String, String)> = Default::default();
    for stop in &organ.stops {
        let Some(manual) = manual_name(stop.manual) else {
            tracing::warn!(
                "adoption: stop {:?} sits on a manual the set hasn't got — not inventoried",
                stop.name
            );
            continue;
        };
        if !written.insert((manual.to_lowercase(), stop.name.to_lowercase())) {
            continue;
        }
        let mut table = toml_edit::Table::new();
        table["from"] = toml_edit::value("s1");
        table["manual"] = toml_edit::value(manual);
        table["stop"] = toml_edit::value(stop.name.as_str());
        table["on"] = toml_edit::value(manual);
        stops.push(table);
    }
    if !stops.is_empty() {
        doc["stop"] = toml_edit::Item::ArrayOfTables(stops);
    }

    let sidecar_path = aristide_formats::sidecar::path_for(set);
    match std::fs::read_to_string(&sidecar_path) {
        Ok(text) => match text.parse::<toml_edit::DocumentMut>() {
            Ok(sidecar) => {
                for (key, item) in sidecar.iter() {
                    // A sidecar rename became this file's own name
                    // above; the structural keys are this file's own —
                    // a sidecar has no business declaring them anyway.
                    if !matches!(
                        key,
                        "name" | "sources" | "manual" | "division" | "stop" | "move"
                    ) {
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

    // The set's couplers, snapshotted as route definitions — ahead of
    // any the sidecar defines, matching the order a direct load gives
    // them (the set's own first, the player's custom ones after).
    let mut defines = toml_edit::ArrayOfTables::new();
    for coupler in &organ.couplers {
        let mut routes = toml_edit::ArrayOfTables::new();
        let mut sound = true;
        for route in &coupler.routes {
            let mut table = toml_edit::Table::new();
            let Some(from) = manual_name(route.from_manual) else {
                sound = false;
                break;
            };
            table["from"] = toml_edit::value(from);
            if let Some(target) = &route.target {
                let Some(to) = manual_name(target.manual) else {
                    sound = false;
                    break;
                };
                table["to"] = toml_edit::value(to);
                if target.key_shift != 0 {
                    table["shift"] = toml_edit::value(target.key_shift as i64);
                }
                if let Some(repitch) = target.repitch {
                    table["repitch"] = toml_edit::value(repitch);
                }
            }
            if let Some(low) = route.low_key {
                table["low"] = toml_edit::value(low as i64);
            }
            if let Some(high) = route.high_key {
                table["high"] = toml_edit::value(high as i64);
            }
            if route.unison_off {
                table["unison_off"] = toml_edit::value(true);
            }
            routes.push(table);
        }
        if !sound {
            tracing::warn!(
                "adoption: coupler {:?} routes to a manual the set hasn't got — not inventoried",
                coupler.name
            );
            continue;
        }
        let mut table = toml_edit::Table::new();
        table["name"] = toml_edit::value(coupler.name.as_str());
        table["route"] = toml_edit::Item::ArrayOfTables(routes);
        defines.push(table);
    }
    if !defines.is_empty() {
        let couplers = doc
            .entry("couplers")
            .or_insert(toml_edit::Item::Table(toml_edit::Table::new()));
        let couplers = couplers
            .as_table_mut()
            .ok_or_else(|| "[couplers] is not a table".to_string())?;
        couplers.set_implicit(true);
        if let Some(existing) = couplers.get("define").and_then(|d| d.as_array_of_tables()) {
            for table in existing.iter() {
                defines.push(table.clone());
            }
        }
        couplers["define"] = toml_edit::Item::ArrayOfTables(defines);
    }

    let body = format!(
        "# An Aristide organ, born from the sample set under [sources].\n\
         # This file is the whole instrument — manuals, stops, couplers,\n\
         # wiring and settings live here; the set only supplies pipes\n\
         # and samples. Edit freely: the console writes back here too.\n\
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
            bend: input.bend,
            map: input.map.clone(),
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
                if let Some(bend) = input.bend {
                    table["bend"] = toml_edit::value(bend as f64);
                }
                if let Some(map) = &input.map {
                    table["map"] = toml_edit::value(map.as_str());
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

/// A manual's own tuning as the file spells it: temperament name,
/// a4 Hz, transpose, and optionally a Scala scale (.scl path and .kbm
/// path) standing in for the temperament.
#[derive(Debug, Clone, PartialEq)]
pub struct ManualTuningFields {
    pub temperament: String,
    pub a4_hz: f64,
    pub transpose: i8,
    pub scale: Option<String>,
    pub keymap: Option<String>,
}

/// One manual as `save_composite` writes it: name, compass, and any
/// tuning of its own.
pub struct SavedManual {
    pub name: String,
    pub kind: aristide_model::ManualKind,
    pub low: u8,
    pub high: u8,
    pub tuning: Option<ManualTuningFields>,
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
        if manual.kind != aristide_model::ManualKind::Manual {
            table["kind"] = toml_edit::value(manual.kind.as_str());
        }
        table["low"] = toml_edit::value(manual.low as i64);
        table["high"] = toml_edit::value(manual.high as i64);
        if let Some(tuning) = &manual.tuning {
            write_manual_tuning_fields(&mut table, tuning);
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
    tuning: Option<ManualTuningFields>,
) -> Result<bool, String> {
    edit_composite_manual(path, manual, |table| match &tuning {
        Some(fields) => {
            write_manual_tuning_fields(table, fields);
        }
        None => {
            for field in ["temperament", "a4_hz", "transpose", "scale", "keymap"] {
                table.remove(field);
            }
        }
    })
}

/// The tuning lines of one `[[manual]]` table. A scale replaces the
/// temperament line (a Scala scale IS the temperament); absent fields
/// are removed so the file never says two things at once.
fn write_manual_tuning_fields(table: &mut toml_edit::Table, fields: &ManualTuningFields) {
    if fields.scale.is_some() {
        table.remove("temperament");
    } else {
        table["temperament"] = toml_edit::value(fields.temperament.as_str());
    }
    table["a4_hz"] = toml_edit::value(fields.a4_hz);
    table["transpose"] = toml_edit::value(fields.transpose as i64);
    for (key, value) in [("scale", &fields.scale), ("keymap", &fields.keymap)] {
        match value {
            Some(value) => table[key] = toml_edit::value(value.as_str()),
            None => {
                table.remove(key);
            }
        }
    }
}

/// Change one declared manual's kind in a composite file. The default
/// kind is expressed by absence, so the file reads as it always has:
/// a plain hand keyboard carries no `kind` line. `Ok(false)` when the
/// file declares no such manual.
pub fn write_composite_manual_kind(
    path: &Path,
    manual: &str,
    kind: aristide_model::ManualKind,
) -> Result<bool, String> {
    edit_composite_manual(path, manual, |table| {
        if kind == aristide_model::ManualKind::Manual {
            table.remove("kind");
        } else {
            table["kind"] = toml_edit::value(kind.as_str());
        }
    })
}

/// Write (or, with `None`, remove) one declared manual's hex-field
/// layout in a composite file, as an inline `hex = { ... }` table.
/// Absence means "derive the default", so a reset reads as a plain
/// microtonal manual again. `Ok(false)` when the file declares no
/// such manual.
pub fn write_composite_manual_hex(
    path: &Path,
    manual: &str,
    layout: Option<aristide_model::HexLayout>,
) -> Result<bool, String> {
    edit_composite_manual(path, manual, |table| match layout {
        None => {
            table.remove("hex");
        }
        Some(layout) => {
            let mut hex = toml_edit::InlineTable::new();
            hex.insert("rows", (layout.rows as i64).into());
            hex.insert("cols", (layout.cols as i64).into());
            hex.insert("right", (layout.right as i64).into());
            hex.insert("upright", (layout.upright as i64).into());
            hex.insert("anchor", (layout.anchor as i64).into());
            table["hex"] = toml_edit::value(hex);
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

// ---- organ-pane editor writers -------------------------------------
//
// Every edit the organ pane makes is a line of the composite file:
// these writers add, rename, reorder and remove those lines,
// comment-preservingly, and the server reloads the file afterwards so
// the console always plays exactly what the file says.

fn composite_doc(path: &Path) -> Result<toml_edit::DocumentMut, String> {
    let text =
        std::fs::read_to_string(path).map_err(|err| format!("{}: {err}", path.display()))?;
    text.parse().map_err(|err| format!("{}: {err}", path.display()))
}

fn field_is(table: &toml_edit::Table, key: &str, wanted: &str) -> bool {
    table
        .get(key)
        .and_then(|value| value.as_str())
        .is_some_and(|value| value.eq_ignore_ascii_case(wanted))
}

/// Replace a string value, keeping its decor — a trailing `# comment`
/// on the line belongs to the value and must survive the edit.
fn set_string_preserving(table: &mut toml_edit::Table, key: &str, to: &str) {
    if let Some(value) = table.get_mut(key).and_then(|item| item.as_value_mut()) {
        let decor = value.decor().clone();
        *value = to.into();
        *value.decor_mut() = decor;
    } else {
        table[key] = toml_edit::value(to);
    }
}

fn rename_field(table: &mut toml_edit::Table, key: &str, from: &str, to: &str) {
    if field_is(table, key, from) {
        set_string_preserving(table, key, to);
    }
}

/// Every table of an array-of-tables at `doc[key]`, mutably; a missing
/// key yields nothing.
fn tables_mut<'a>(
    doc: &'a mut toml_edit::DocumentMut,
    key: &str,
) -> impl Iterator<Item = &'a mut toml_edit::Table> {
    doc.get_mut(key)
        .and_then(|item| item.as_array_of_tables_mut())
        .into_iter()
        .flat_map(|tables| tables.iter_mut())
}

/// Declare a new manual in a composite file. The compass is written
/// out (a declared manual with nothing pulled yet has no other way to
/// have one); a non-default kind is written as `kind = "..."`.
pub fn append_composite_manual(
    path: &Path,
    name: &str,
    low: u8,
    high: u8,
    kind: aristide_model::ManualKind,
) -> Result<(), String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("the manual needs a name".into());
    }
    if low > high {
        return Err("low is above high".into());
    }
    let mut doc = composite_doc(path)?;
    if let Some(manuals) = doc.get("manual").and_then(|m| m.as_array_of_tables())
        && manuals.iter().any(|table| field_is(table, "name", name))
    {
        return Err(format!("this organ already has a manual named {name:?}"));
    }
    let manuals = doc
        .entry("manual")
        .or_insert(toml_edit::Item::ArrayOfTables(toml_edit::ArrayOfTables::new()));
    let Some(manuals) = manuals.as_array_of_tables_mut() else {
        return Err("[[manual]] is not an array of tables".into());
    };
    let mut table = toml_edit::Table::new();
    table["name"] = toml_edit::value(name);
    if kind != aristide_model::ManualKind::Manual {
        table["kind"] = toml_edit::value(kind.as_str());
    }
    table["low"] = toml_edit::value(low as i64);
    table["high"] = toml_edit::value(high as i64);
    manuals.push(table);
    write_atomically(path, doc.to_string())
}

/// Rename a declared manual, everywhere the file says its name: the
/// `[[manual]]` itself, pulls landing on it, moves, coupler routes and
/// MIDI wiring. Source-side names (`[[stop]] manual`, which division a
/// pull reads from) are the source's own and stay. `Ok(false)` when
/// the file declares no such manual.
pub fn rename_composite_manual(path: &Path, from: &str, to: &str) -> Result<bool, String> {
    let to = to.trim();
    if to.is_empty() {
        return Err("the manual needs a name".into());
    }
    let mut doc = composite_doc(path)?;
    let mut found = false;
    for table in tables_mut(&mut doc, "manual") {
        if field_is(table, "name", from) {
            set_string_preserving(table, "name", to);
            found = true;
        }
    }
    if !found {
        return Ok(false);
    }
    for table in tables_mut(&mut doc, "stop") {
        rename_field(table, "on", from, to);
    }
    for table in tables_mut(&mut doc, "division") {
        rename_field(table, "on", from, to);
    }
    for table in tables_mut(&mut doc, "move") {
        rename_field(table, "from", from, to);
        rename_field(table, "to", from, to);
    }
    if let Some(defines) = doc
        .get_mut("couplers")
        .and_then(|couplers| couplers.get_mut("define"))
        .and_then(|define| define.as_array_of_tables_mut())
    {
        for define in defines.iter_mut() {
            if let Some(routes) = define.get_mut("route").and_then(|r| r.as_array_of_tables_mut())
            {
                for route in routes.iter_mut() {
                    rename_field(route, "from", from, to);
                    rename_field(route, "to", from, to);
                }
            }
        }
    }
    if let Some(midi) = doc.get_mut("midi").and_then(|midi| midi.as_table_like_mut()) {
        for key in ["input", "control"] {
            if let Some(tables) = midi.get_mut(key).and_then(|i| i.as_array_of_tables_mut()) {
                for table in tables.iter_mut() {
                    rename_field(table, "manual", from, to);
                }
            }
        }
    }
    if let Some(layout) = console_layout_mut(&mut doc) {
        for prefix in ["keyboard", "jamb"] {
            if let Some(item) = layout.remove(&format!("{prefix}:{from}")) {
                layout.insert(&format!("{prefix}:{to}"), item);
            }
        }
    }
    write_atomically(path, doc.to_string())?;
    Ok(true)
}

/// Remove a declared manual and everything the file lands on it:
/// pulls, moves, coupler routes and MIDI wiring naming it. `Ok(false)`
/// when the file declares no such manual — the bin can only take what
/// the file owns.
pub fn remove_composite_manual(path: &Path, name: &str) -> Result<bool, String> {
    let mut doc = composite_doc(path)?;
    let Some(manuals) = doc.get_mut("manual").and_then(|m| m.as_array_of_tables_mut()) else {
        return Ok(false);
    };
    let before = manuals.len();
    manuals.retain(|table| !field_is(table, "name", name));
    if manuals.len() == before {
        return Ok(false);
    }
    if manuals.is_empty() {
        doc.remove("manual");
    }
    for key in ["stop", "division"] {
        if let Some(tables) = doc.get_mut(key).and_then(|i| i.as_array_of_tables_mut()) {
            tables.retain(|table| !field_is(table, "on", name));
            if tables.is_empty() {
                doc.remove(key);
            }
        }
    }
    if let Some(moves) = doc.get_mut("move").and_then(|m| m.as_array_of_tables_mut()) {
        moves.retain(|table| !field_is(table, "from", name) && !field_is(table, "to", name));
        if moves.is_empty() {
            doc.remove("move");
        }
    }
    if let Some(defines) = doc
        .get_mut("couplers")
        .and_then(|couplers| couplers.get_mut("define"))
        .and_then(|define| define.as_array_of_tables_mut())
    {
        for define in defines.iter_mut() {
            if let Some(routes) = define.get_mut("route").and_then(|r| r.as_array_of_tables_mut())
            {
                routes.retain(|route| {
                    !field_is(route, "from", name) && !field_is(route, "to", name)
                });
            }
        }
        defines.retain(|define| {
            define
                .get("route")
                .and_then(|r| r.as_array_of_tables())
                .is_some_and(|routes| !routes.is_empty())
        });
    }
    if let Some(midi) = doc.get_mut("midi").and_then(|midi| midi.as_table_like_mut()) {
        for key in ["input", "control"] {
            if let Some(tables) = midi.get_mut(key).and_then(|i| i.as_array_of_tables_mut()) {
                tables.retain(|table| !field_is(table, "manual", name));
            }
        }
    }
    if let Some(layout) = console_layout_mut(&mut doc) {
        for prefix in ["keyboard", "jamb"] {
            layout.remove(&format!("{prefix}:{name}"));
        }
        if layout.is_empty()
            && let Some(console) = doc.get_mut("console").and_then(|c| c.as_table_mut())
        {
            console.remove("layout");
        }
    }
    write_atomically(path, doc.to_string())?;
    Ok(true)
}

/// Move a declared manual to another position among the declarations —
/// declaration order is console stacking order. `Ok(false)` when the
/// file declares no such manual.
pub fn reorder_composite_manual(path: &Path, name: &str, to: usize) -> Result<bool, String> {
    let mut doc = composite_doc(path)?;
    let Some(manuals) = doc.get_mut("manual").and_then(|m| m.as_array_of_tables_mut()) else {
        return Ok(false);
    };
    let mut tables: Vec<toml_edit::Table> = manuals.iter().cloned().collect();
    let Some(at) = tables.iter().position(|table| field_is(table, "name", name)) else {
        return Ok(false);
    };
    // A cloned table remembers where in the document it was; the new
    // order must take over the old positions or the file serializes
    // exactly as before.
    let mut positions: Vec<Option<isize>> = tables.iter().map(|table| table.position()).collect();
    positions.sort_unstable();
    let table = tables.remove(at);
    tables.insert(to.min(tables.len()), table);
    manuals.clear();
    for (table, position) in tables.iter_mut().zip(positions) {
        table.set_position(position);
        manuals.push(table.clone());
    }
    write_atomically(path, doc.to_string())?;
    Ok(true)
}

/// Add a sample set to a composite's `[sources]`, returning the alias
/// it got. Adding a source pulls nothing by itself — its material
/// becomes available to pull.
pub fn append_composite_source(path: &Path, set: &Path) -> Result<String, String> {
    let mut doc = composite_doc(path)?;
    let sources = doc
        .entry("sources")
        .or_insert(toml_edit::Item::Table(toml_edit::Table::new()));
    let Some(sources) = sources.as_table_mut() else {
        return Err("[sources] is not a table".into());
    };
    let set_text = set.to_string_lossy();
    for (_, value) in sources.iter() {
        let existing = value
            .as_str()
            .or_else(|| value.get("path").and_then(|p| p.as_str()));
        if existing == Some(set_text.as_ref()) {
            return Err("this organ already has that set as a source".into());
        }
    }
    let mut nth = 1;
    let alias = loop {
        let alias = format!("s{nth}");
        if !sources.contains_key(&alias) {
            break alias;
        }
        nth += 1;
    };
    sources.insert(&alias, toml_edit::value(set_text.as_ref()));
    write_atomically(path, doc.to_string())?;
    Ok(alias)
}

/// Append one `[[stop]]` pull (or with no stop pattern a whole
/// `[[division]]` pull) onto a manual.
pub fn append_composite_pull(
    path: &Path,
    from: &str,
    source_manual: &str,
    stop: Option<&str>,
    on: &str,
) -> Result<(), String> {
    let mut doc = composite_doc(path)?;
    // A pull naming a source the file hasn't got would poison the next
    // load; refuse it here rather than bricking the file.
    if !doc
        .get("sources")
        .and_then(|sources| sources.as_table())
        .is_some_and(|sources| sources.contains_key(from))
    {
        return Err(format!("{from:?} is not a [sources] alias of this organ"));
    }
    let key = if stop.is_some() { "stop" } else { "division" };
    let tables = doc
        .entry(key)
        .or_insert(toml_edit::Item::ArrayOfTables(toml_edit::ArrayOfTables::new()));
    let Some(tables) = tables.as_array_of_tables_mut() else {
        return Err(format!("[[{key}]] is not an array of tables"));
    };
    let mut table = toml_edit::Table::new();
    table["from"] = toml_edit::value(from);
    table["manual"] = toml_edit::value(source_manual);
    if let Some(stop) = stop {
        table["stop"] = toml_edit::value(stop);
    }
    table["on"] = toml_edit::value(on);
    tables.push(table);
    write_atomically(path, doc.to_string())
}

/// Remove the `[[stop]]` pull that brought `stop` in, and any
/// `[[move]]` lines about it. `on` is the manual the pull named (where
/// the stop first landed). `Ok(false)` when no pull matches — a stop
/// that came in as part of a `[[division]]` pull has no line of its
/// own to remove.
pub fn remove_composite_stop_pull(path: &Path, stop: &str, on: &str) -> Result<bool, String> {
    let mut doc = composite_doc(path)?;
    let Some(stops) = doc.get_mut("stop").and_then(|s| s.as_array_of_tables_mut()) else {
        return Ok(false);
    };
    let named: Vec<usize> = (0..stops.len())
        .filter(|&i| stops.get(i).is_some_and(|table| field_is(table, "stop", stop)))
        .collect();
    let landed: Vec<usize> = named
        .iter()
        .copied()
        .filter(|&i| stops.get(i).is_some_and(|table| field_is(table, "on", on)))
        .collect();
    let doomed = if !landed.is_empty() {
        landed
    } else if named.len() == 1 {
        named
    } else {
        return Ok(false);
    };
    let mut index = 0;
    stops.retain(|_| {
        let keep = !doomed.contains(&index);
        index += 1;
        keep
    });
    if stops.is_empty() {
        doc.remove("stop");
    }
    if let Some(moves) = doc.get_mut("move").and_then(|m| m.as_array_of_tables_mut()) {
        moves.retain(|table| !field_is(table, "stop", stop));
        if moves.is_empty() {
            doc.remove("move");
        }
    }
    write_atomically(path, doc.to_string())?;
    Ok(true)
}

/// Define a new (empty) swell box in a composite file. The pane fills
/// it by dragging stops in afterwards.
pub fn append_composite_enclosure(path: &Path, name: &str) -> Result<(), String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("the swell box needs a name".into());
    }
    let mut doc = composite_doc(path)?;
    if let Some(enclosures) = doc.get("enclosure").and_then(|e| e.as_array_of_tables())
        && enclosures.iter().any(|table| field_is(table, "name", name))
    {
        return Err(format!("this organ already defines a swell box named {name:?}"));
    }
    let enclosures = doc
        .entry("enclosure")
        .or_insert(toml_edit::Item::ArrayOfTables(toml_edit::ArrayOfTables::new()));
    let Some(enclosures) = enclosures.as_array_of_tables_mut() else {
        return Err("[[enclosure]] is not an array of tables".into());
    };
    let mut table = toml_edit::Table::new();
    table["name"] = toml_edit::value(name);
    table["stops"] = toml_edit::value(toml_edit::Array::new());
    enclosures.push(table);
    write_atomically(path, doc.to_string())
}

/// Remove a file-defined swell box. `Ok(false)` when the file defines
/// no such box — a box carried in from a source has no line to remove.
pub fn remove_composite_enclosure(path: &Path, name: &str) -> Result<bool, String> {
    let mut doc = composite_doc(path)?;
    let Some(enclosures) = doc.get_mut("enclosure").and_then(|e| e.as_array_of_tables_mut())
    else {
        return Ok(false);
    };
    let before = enclosures.len();
    enclosures.retain(|table| !field_is(table, "name", name));
    if enclosures.len() == before {
        return Ok(false);
    }
    if enclosures.is_empty() {
        doc.remove("enclosure");
    }
    write_atomically(path, doc.to_string())?;
    Ok(true)
}

/// Put a stop into (or take it out of) a file-defined swell box, by
/// exact name in the box's `stops` list. `Ok(false)` when the file
/// defines no such box.
pub fn assign_composite_enclosure_stop(
    path: &Path,
    enclosure: &str,
    stop: &str,
    inside: bool,
) -> Result<bool, String> {
    let mut doc = composite_doc(path)?;
    let Some(enclosures) = doc.get_mut("enclosure").and_then(|e| e.as_array_of_tables_mut())
    else {
        return Ok(false);
    };
    let Some(table) = enclosures
        .iter_mut()
        .find(|table| field_is(table, "name", enclosure))
    else {
        return Ok(false);
    };
    let stops = table
        .entry("stops")
        .or_insert(toml_edit::value(toml_edit::Array::new()));
    let Some(stops) = stops.as_array_mut() else {
        return Err("the box's stops list is not an array".into());
    };
    let listed = stops
        .iter()
        .position(|value| value.as_str().is_some_and(|v| v.eq_ignore_ascii_case(stop)));
    match (inside, listed) {
        (true, None) => stops.push(stop),
        (false, Some(at)) => {
            stops.remove(at);
        }
        _ => return Ok(true), // already as asked
    }
    write_atomically(path, doc.to_string())?;
    Ok(true)
}

/// Every `[console.layout]` entry, mutably. Missing sections yield
/// nothing rather than creating one — cosmetic geometry is optional,
/// so renaming/removing a manual before any panel was ever placed is a
/// no-op here.
fn console_layout_mut(doc: &mut toml_edit::DocumentMut) -> Option<&mut toml_edit::Table> {
    doc.get_mut("console")
        .and_then(|console| console.get_mut("layout"))
        .and_then(|layout| layout.as_table_mut())
}

/// Upsert one console panel's canvas position: creates `[console.layout]`
/// if the file doesn't have it yet, and writes (or replaces) the
/// panel's quoted key inside it — `"keyboard:Great" = { x = .., y = .. }`.
/// Purely cosmetic: unlike the structural editors above, nothing calls
/// this expects a reload — the caller updates the in-memory snapshot
/// itself.
pub fn write_composite_panel(path: &Path, panel: &str, x: f32, y: f32) -> Result<(), String> {
    let mut doc = composite_doc(path)?;
    let console = doc
        .entry("console")
        .or_insert(toml_edit::Item::Table(toml_edit::Table::new()));
    let Some(console) = console.as_table_mut() else {
        return Err("[console] is not a table".into());
    };
    // Only "layout" lives under it so far; stay out of the way of a
    // future `[console]` key of its own by not forcing a header here.
    console.set_implicit(true);
    let layout = console
        .entry("layout")
        .or_insert(toml_edit::Item::Table(toml_edit::Table::new()));
    let Some(layout) = layout.as_table_mut() else {
        return Err("[console.layout] is not a table".into());
    };
    let mut pos = toml_edit::InlineTable::new();
    pos.insert("x", (x as f64).into());
    pos.insert("y", (y as f64).into());
    layout.insert(panel, toml_edit::Item::Value(toml_edit::Value::InlineTable(pos)));
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
            bend: None,
            map: None,
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
                bend: None,
                map: None,
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

    /// A small instrument to inventory: two manuals, two stops, one
    /// coupler — enough shape for adoption to write every section.
    fn test_organ() -> aristide_model::Organ {
        use aristide_model::{Coupler, Manual, ManualId, Organ, Stop, StopId};
        Organ {
            name: "Village".into(),
            manuals: vec![
                Manual {
                    id: ManualId(0),
                    name: "Pedal".into(),
                    first_midi_note: 36,
                    key_count: 32,
                    kind: aristide_model::ManualKind::Pedal,
                    hex: None,
                },
                Manual {
                    id: ManualId(1),
                    name: "Great".into(),
                    first_midi_note: 36,
                    key_count: 61,
                    kind: Default::default(),
                    hex: None,
                },
            ],
            stops: vec![
                Stop {
                    id: StopId(0),
                    name: "Subbass 16".into(),
                    manual: ManualId(0),
                    ranks: Vec::new(),
                },
                Stop {
                    id: StopId(1),
                    name: "Montre 8".into(),
                    manual: ManualId(1),
                    ranks: Vec::new(),
                },
            ],
            couplers: vec![Coupler::simple("Great to Pedal", ManualId(1), ManualId(0), 0)],
            ..Default::default()
        }
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
        let path = create_wrapper_organ(&organs, "Chapelle", &set, &test_organ(), Some(&wiring))
            .expect("wrapper created");
        let text = std::fs::read_to_string(&path).expect("reads");
        assert!(text.contains("# gentle bellows"), "sidecar comments ride along");
        let def: aristide_formats::instrument::Definition =
            toml::from_str(&text).expect("a valid organ file");
        assert_eq!(def.name, "Chapelle", "one name — the file's");
        assert_eq!(def.sources.len(), 1);
        let source = def.sources.values().next().expect("source");
        assert_eq!(source.path(), &set);
        assert!(source.layout(), "the set's chest and box numbering is kept whole");
        assert_eq!(def.manuals.len(), 2, "every manual declared");
        assert_eq!(def.manuals[0].name, "Pedal");
        assert_eq!(def.manuals[0].kind.as_deref(), Some("pedal"));
        assert_eq!(def.manuals[1].kind, None);
        assert_eq!(def.stops.len(), 2, "every stop an explicit pull");
        assert_eq!(def.stops[1].stop, "Montre 8");
        assert_eq!(def.stops[1].manual.as_deref(), Some("Great"));
        assert_eq!(def.stops[1].on, "Great");
        assert_eq!(def.couplers.define.len(), 1, "the set's couplers are route definitions now");
        assert_eq!(def.couplers.define[0].name, "Great to Pedal");
        assert_eq!(def.couplers.define[0].routes[0].from, "Great");
        assert_eq!(def.couplers.define[0].routes[0].to.as_deref(), Some("Pedal"));
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
        let bare = create_wrapper_organ(&organs, "Bare", &bare_set, &test_organ(), None)
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
        let wrapper =
            create_wrapper_organ(&dir.join("organs"), "Chapelle", &canonical, &test_organ(), None)
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
        assert_eq!(
            config.wrapper_for(&canonical, None, None),
            Some(wrapper.clone())
        );
        assert_eq!(
            config.wrapper_for(&dir.join("elsewhere.organ"), None, None),
            None,
            "nothing wraps a set this machine has never seen"
        );

        // Removed from Recent is not gone: the organs folder still
        // holds the file, and reloading the set finds it there rather
        // than making a second organ without its name and wiring.
        config.forget(&wrapper);
        assert_eq!(config.wrapper_for(&canonical, None, None), None);
        assert_eq!(
            config.wrapper_for(&canonical, None, Some(&dir.join("organs"))),
            Some(wrapper)
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Several organs can wrap one set — the adopted wrapper plus
    /// organs built on the console from it. The name of the library
    /// entry the player clicked decides which one loads; without it
    /// (or when nothing carries that name) the most recently played
    /// one wins, never silently a different organ than the click said.
    #[test]
    fn the_clicked_name_picks_among_organs_sharing_a_set() {
        let dir = std::env::temp_dir().join("aristide-wrapper-name-test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("fixture dir");
        let set = dir.join("village.organ");
        std::fs::write(&set, "[Organ]").expect("fixture set");
        let canonical = set.canonicalize().expect("canonicalizes");
        let organs = dir.join("organs");
        let adopted =
            create_wrapper_organ(&organs, "Chapelle", &canonical, &test_organ(), None)
                .expect("wrapper created");
        let built = dir.join("my-organ.toml");
        std::fs::write(
            &built,
            format!(
                "name = \"My Organ\"\n[sources]\ns1 = {:?}\n",
                canonical.display().to_string()
            ),
        )
        .expect("fixture built organ");

        let mut config = MidiConfig::default();
        config.remember("Chapelle", &adopted);
        config.remember("My Organ", &built); // most recently played
        assert_eq!(
            config.wrapper_for(&canonical, Some("Chapelle"), None),
            Some(adopted.clone()),
            "the clicked name wins over recency"
        );
        assert_eq!(
            config.wrapper_for(&canonical, Some("My Organ"), None),
            Some(built.clone())
        );
        assert_eq!(
            config.wrapper_for(&canonical, None, None),
            Some(built.clone()),
            "no name: most recent wins"
        );
        assert_eq!(
            config.wrapper_for(&canonical, Some("Renamed Since"), None),
            Some(built),
            "a stale name still resolves rather than duplicating the organ"
        );

        // The name also reaches into the organs folder for an organ
        // taken off Recent.
        config.forget(&adopted);
        assert_eq!(
            config.wrapper_for(&canonical, Some("Chapelle"), Some(&organs)),
            Some(adopted)
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// An organ built ON a set — its own manuals, its own pulls — is
    /// not that set's wrapper. Browsing to the raw set means the
    /// set's own organ: the adopted wrapper wins however recently the
    /// built organ played, and with no wrapper at all the lookup
    /// comes up empty so adoption makes one, rather than silently
    /// loading whatever was built from the set. The built organ is
    /// still reachable the way organs are: by its name.
    #[test]
    fn a_built_organ_never_hijacks_a_direct_set_load() {
        let dir = std::env::temp_dir().join("aristide-wrapper-hijack-test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("fixture dir");
        let set = dir.join("village.organ");
        std::fs::write(&set, "[Organ]").expect("fixture set");
        let canonical = set.canonicalize().expect("canonicalizes");
        let adopted =
            create_wrapper_organ(&dir.join("organs"), "Chapelle", &canonical, &test_organ(), None)
                .expect("wrapper created");
        let built = dir.join("built.toml");
        std::fs::write(
            &built,
            format!(
                "name = \"Built\"\n[sources]\ns1 = {:?}\n\
                 [[manual]]\nname = \"Great\"\nlow = 36\nhigh = 96\n\
                 [[stop]]\nfrom = \"s1\"\nstop = \"Montre\"\non = \"Great\"\n",
                canonical.display().to_string()
            ),
        )
        .expect("fixture built organ");

        let mut config = MidiConfig::default();
        config.remember("Chapelle", &adopted);
        config.remember("Built", &built); // most recently played
        assert_eq!(
            config.wrapper_for(&canonical, None, None),
            Some(adopted.clone()),
            "the adopted wrapper wins over a more recent built organ"
        );
        assert_eq!(
            config.wrapper_for(&canonical, Some("Built"), None),
            Some(built.clone()),
            "the clicked name still reaches the built organ"
        );
        config.forget(&adopted);
        assert_eq!(
            config.wrapper_for(&canonical, None, None),
            None,
            "no wrapper: adopt a fresh one instead of hijacking the built organ"
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

    /// The organ-pane editor's whole file vocabulary, exercised on one
    /// growing file: declare manuals, add a source, pull a stop, then
    /// rename, reorder and delete — every edit a line, comments kept.
    #[test]
    fn editor_writers_grow_and_prune_a_composite() {
        let dir = std::env::temp_dir().join("aristide-editor-writers-test");
        let _ = std::fs::remove_dir_all(&dir);
        let path = create_blank_organ(&dir, "Atelier").expect("creates");

        use aristide_model::ManualKind;
        append_composite_manual(&path, "Grand orgue", 36, 96, ManualKind::Manual)
            .expect("manual");
        append_composite_manual(&path, "Pédale", 36, 67, ManualKind::Pedal).expect("pedal");
        assert!(
            append_composite_manual(&path, "grand ORGUE", 36, 96, ManualKind::Manual).is_err(),
            "duplicate names refused"
        );

        let set = dir.join("village.organ");
        std::fs::write(&set, "[Organ]").expect("fixture set");
        let alias = append_composite_source(&path, &set).expect("source added");
        assert_eq!(alias, "s1");
        assert!(
            append_composite_source(&path, &set).is_err(),
            "the same set twice is one source"
        );

        assert!(
            append_composite_pull(&path, "s9", "Great", Some("Montre 8"), "Grand orgue")
                .is_err(),
            "a pull naming no source must not poison the file"
        );
        append_composite_pull(&path, "s1", "Great", Some("Montre 8"), "Grand orgue")
            .expect("stop pull");
        append_composite_pull(&path, "s1", "Pedal", None, "Pédale").expect("division pull");
        append_composite_move(&path, "Montre 8", "Grand orgue", "Pédale").expect("move");

        let def = |path: &Path| -> aristide_formats::instrument::Definition {
            toml::from_str(&std::fs::read_to_string(path).expect("reads")).expect("parses")
        };
        let parsed = def(&path);
        assert_eq!(parsed.manuals.len(), 2);
        assert_eq!(parsed.manuals[1].kind.as_deref(), Some("pedal"));
        assert_eq!(parsed.stops.len(), 1);
        assert_eq!(parsed.stops[0].manual.as_deref(), Some("Great"));
        assert_eq!(parsed.divisions.len(), 1);

        // A kind edit rewrites the kind line; the default kind is
        // expressed by absence, so a hand keyboard carries no line.
        assert!(
            write_composite_manual_kind(&path, "Grand orgue", ManualKind::Microtonal)
                .expect("kind")
        );
        assert_eq!(def(&path).manuals[0].kind.as_deref(), Some("microtonal"));
        assert!(
            write_composite_manual_kind(&path, "Grand orgue", ManualKind::Manual).expect("kind")
        );
        assert_eq!(def(&path).manuals[0].kind, None);
        assert!(!write_composite_manual_kind(&path, "Ghost", ManualKind::Pedal).expect("ghost"));

        // A hex layout writes as one inline table; removal restores
        // the derived default by absence, and a ghost manual is Ok(false).
        let layout = aristide_model::HexLayout {
            rows: 5,
            cols: 17,
            right: 2,
            upright: 7,
            anchor: 36,
        };
        assert!(write_composite_manual_hex(&path, "Grand orgue", Some(layout)).expect("hex"));
        let hex = def(&path).manuals[0].hex.clone().expect("hex parsed back");
        assert_eq!((hex.rows, hex.cols), (Some(5), Some(17)));
        assert_eq!((hex.right, hex.upright), (Some(2), Some(7)));
        assert!(write_composite_manual_hex(&path, "Grand orgue", None).expect("hex off"));
        assert!(def(&path).manuals[0].hex.is_none());
        assert!(!write_composite_manual_hex(&path, "Ghost", None).expect("ghost"));

        // A scale replaces the temperament line (a Scala scale IS the
        // temperament); naming a temperament again drops the scale,
        // and None clears the whole tuning.
        let tuned = |temperament: &str, scale: Option<&str>| ManualTuningFields {
            temperament: temperament.into(),
            a4_hz: 432.0,
            transpose: 0,
            scale: scale.map(str::to_string),
            keymap: None,
        };
        assert!(
            write_composite_manual_tuning(&path, "Grand orgue", Some(tuned("equal", Some("19edo.scl"))))
                .expect("tuning")
        );
        let parsed = def(&path);
        assert_eq!(parsed.manuals[0].scale.as_deref(), Some("19edo.scl"));
        assert_eq!(parsed.manuals[0].temperament, None, "a scale IS the temperament");
        assert_eq!(parsed.manuals[0].a4_hz, Some(432.0));
        assert!(
            write_composite_manual_tuning(&path, "Grand orgue", Some(tuned("meantone4", None)))
                .expect("tuning")
        );
        let parsed = def(&path);
        assert_eq!(parsed.manuals[0].scale, None);
        assert_eq!(parsed.manuals[0].temperament.as_deref(), Some("meantone4"));
        assert!(write_composite_manual_tuning(&path, "Grand orgue", None).expect("tuning"));
        assert_eq!(def(&path).manuals[0].a4_hz, None);

        // Rename follows the name everywhere the file says it.
        assert!(rename_composite_manual(&path, "Grand orgue", "Hauptwerk").expect("renames"));
        assert!(!rename_composite_manual(&path, "Ghost", "X").expect("no ghost"));
        let parsed = def(&path);
        assert_eq!(parsed.manuals[0].name, "Hauptwerk");
        assert_eq!(parsed.stops[0].on, "Hauptwerk");
        assert_eq!(parsed.moves[0].from, "Hauptwerk");
        assert_eq!(
            parsed.stops[0].manual.as_deref(),
            Some("Great"),
            "the source's own division name is the source's business"
        );

        assert!(reorder_composite_manual(&path, "Pédale", 0).expect("reorders"));
        assert_eq!(def(&path).manuals[0].name, "Pédale");

        // Deleting the stop's pull also forgets its moves; deleting a
        // manual takes every line landing on it.
        assert!(remove_composite_stop_pull(&path, "Montre 8", "Hauptwerk").expect("unpulls"));
        let parsed = def(&path);
        assert!(parsed.stops.is_empty());
        assert!(parsed.moves.is_empty());
        assert!(remove_composite_manual(&path, "Pédale").expect("removes"));
        let parsed = def(&path);
        assert_eq!(parsed.manuals.len(), 1);
        assert!(parsed.divisions.is_empty(), "the pull landing on it went too");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A swell box lives and dies as `[[enclosure]]` lines: created
    /// empty, filled a stop at a time, and only file-defined boxes can
    /// be touched at all.
    #[test]
    fn enclosure_writers_define_fill_and_remove_boxes() {
        let dir = std::env::temp_dir().join("aristide-enclosure-writers-test");
        let _ = std::fs::remove_dir_all(&dir);
        let path = create_blank_organ(&dir, "Atelier").expect("creates");

        append_composite_enclosure(&path, "Boîte").expect("box defined");
        assert!(
            append_composite_enclosure(&path, "boîte").is_err(),
            "duplicate box names refused"
        );
        assert!(
            assign_composite_enclosure_stop(&path, "Boîte", "Hautbois 8", true).expect("assigns"),
            "the defined box takes a stop"
        );
        assert!(
            !assign_composite_enclosure_stop(&path, "Swell box", "Hautbois 8", true)
                .expect("no ghost"),
            "a box the file doesn't define isn't editable"
        );
        let def: aristide_formats::instrument::Definition =
            toml::from_str(&std::fs::read_to_string(&path).expect("reads")).expect("parses");
        assert_eq!(def.enclosure_defs.len(), 1);
        assert_eq!(def.enclosure_defs[0].stops, ["Hautbois 8"]);

        assign_composite_enclosure_stop(&path, "Boîte", "Hautbois 8", false).expect("unassigns");
        let def: aristide_formats::instrument::Definition =
            toml::from_str(&std::fs::read_to_string(&path).expect("reads")).expect("parses");
        assert!(def.enclosure_defs[0].stops.is_empty());

        assert!(remove_composite_enclosure(&path, "Boîte").expect("removes"));
        assert!(!remove_composite_enclosure(&path, "Boîte").expect("already gone"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Renaming a manual keeps couplers and wiring pointing at it.
    #[test]
    fn manual_rename_and_removal_follow_couplers_and_wiring() {
        let dir = std::env::temp_dir().join("aristide-editor-rename-test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("fixture dir");
        let path = dir.join("organ.toml");
        std::fs::write(
            &path,
            r#"# hand-made
name = "Atelier"

[sources]
s1 = "village.organ"

[[manual]]
name = "Récit" # expressive
low = 36
high = 96

[[manual]]
name = "Positif"
low = 36
high = 96

[[couplers.define]]
name = "Récit au Positif"
[[couplers.define.route]]
from = "Récit"
to = "Positif"

[[midi.input]]
manual = "Récit"
device = "Keys"
"#,
        )
        .expect("writes");

        assert!(rename_composite_manual(&path, "Récit", "Solo").expect("renames"));
        let text = std::fs::read_to_string(&path).expect("reads");
        assert!(text.contains("# expressive"), "comments survive");
        let def: aristide_formats::instrument::Definition =
            toml::from_str(&text).expect("parses");
        assert_eq!(def.couplers.define[0].routes[0].from, "Solo");
        assert_eq!(def.couplers.define[0].routes[0].to.as_deref(), Some("Positif"));
        assert_eq!(def.midi.inputs[0].manual, "Solo");

        assert!(remove_composite_manual(&path, "Solo").expect("removes"));
        let def: aristide_formats::instrument::Definition =
            toml::from_str(&std::fs::read_to_string(&path).expect("reads")).expect("parses");
        assert_eq!(def.manuals.len(), 1);
        assert!(
            def.couplers.define.is_empty(),
            "a coupler with no surviving routes goes with its manual"
        );
        assert!(def.midi.inputs.is_empty());
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
