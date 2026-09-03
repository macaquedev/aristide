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

use aristide_formats::instrument;
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
#                general:<n>, divisional:<manual>:<n>,
#                stepper:next, stepper:prev, stepper:goto:<n>,
#                stepper:store, stepper:insert,
#                crescendo (a pedal — bind it to a CC),
#                crescendo:<stage>, set, cancel, panic, enclosure:<name>
#   manual       optional; which keyboard a pitch action shifts. Absent
#                means every keyboard on the same device.
#
# `set` arms the setter: the next general or divisional press *stores*
# the console instead of recalling it, then disarms — as a console's Set
# piston works. Set + crescendo:<stage> stores that crescendo stage.
#
# A [[library]] entry is one organ this machine has loaded — the
# console's picker lists them as Recent, most recent first. Removing
# one only removes it from that list; the organ's file and its
# assignments below are kept.
#
# [samples] is how this machine holds a set's audio — a fact about the
# box, not the organ, which is why it lives here and not in an organ
# file (Preferences → Sample memory edits it):
#
#   streaming     auto | on | off. Release tails play from the disk
#                 (on), stay in RAM (off), or stream only when the set
#                 would not fit the budget (auto). Attacks and sustain
#                 loops are always resident.
#   ram_budget_mb what auto measures against; omit it for half of this
#                 machine's physical memory
#   bits          resident resolution, 16 (default) or 32
#   cache         keep decoded samples on disk so a set reloads fast
#
# Changes apply the next time an organ loads.
#
# Aristide rewrites this file whenever you change an assignment in
# Preferences → MIDI or load an organ. Hand edits are read back on the
# next start.

";

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct MidiConfig {
    /// How this machine holds sample audio: residency, streaming, the
    /// load cache. Per machine because whether a set fits is a fact
    /// about the box's RAM, not about the set.
    #[serde(default)]
    pub samples: SamplePrefs,
    /// Organs this machine has loaded, most recent first — what the
    /// console's picker offers when the server starts with nothing.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub library: Vec<LibraryEntry>,
    /// Organ name (as the loaded set reports it) → its assignments.
    #[serde(default)]
    pub organs: BTreeMap<String, OrganConfig>,
}

/// Whether release tails play from the disk. Attacks and sustain loops
/// never do — a held note must never wait on a disk — so this only
/// decides where the bytes behind the last loop live.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Streaming {
    /// Stream every tail worth a slot, however small the set.
    On,
    /// Everything resident.
    Off,
    /// Stream only when the fully-resident set would not fit the
    /// budget. Also what an unknown word in a hand-edited file means.
    #[default]
    #[serde(other)]
    Auto,
}

impl Streaming {
    pub fn as_str(self) -> &'static str {
        match self {
            Streaming::Auto => "auto",
            Streaming::On => "on",
            Streaming::Off => "off",
        }
    }

    pub fn parse(text: &str) -> Option<Streaming> {
        match text.trim().to_ascii_lowercase().as_str() {
            "auto" => Some(Streaming::Auto),
            "on" | "true" => Some(Streaming::On),
            "off" | "false" => Some(Streaming::Off),
            _ => None,
        }
    }
}

/// The user config's `[samples]`: how this machine holds a set's audio.
/// Read once per load; a change waits for the next one, because the
/// engine's bank is fixed at construction.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SamplePrefs {
    /// Resident sample resolution: 16 (half the RAM of f32, a −96 dB
    /// floor below organ recordings' own room noise) or 32 (bit-exact
    /// f32, for A/B). Analysis always runs at full decode precision
    /// before quantization.
    pub bits: u32,
    /// Persist decoded samples + analysis under the config directory so
    /// unchanged files skip decode on the next load. Costs disk about
    /// the size of the resident bank.
    pub cache: bool,
    pub streaming: Streaming,
    /// RAM the resident bank may use before `auto` streams. `None` is
    /// half of this machine's physical memory.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ram_budget_mb: Option<u64>,
}

impl Default for SamplePrefs {
    fn default() -> Self {
        SamplePrefs {
            bits: 16,
            cache: true,
            streaming: Streaming::Auto,
            ram_budget_mb: None,
        }
    }
}

impl SamplePrefs {
    /// The resolution the loader will actually use: anything but 16 or
    /// 32 in a hand-edited file falls back to 16.
    pub fn resident_bits(&self) -> u32 {
        if self.bits == 32 { 32 } else { 16 }
    }
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
    /// wrappers qualify — `adopted = true` in the file first (the
    /// set's own organ, never a copy saved from it), else the older
    /// signs: `layout = true` on the source, or a bare file with no
    /// structure of its own (how adoption wrote them before the
    /// inventory). An organ merely
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
        let wraps = |candidate: &Path| -> Option<(String, bool, bool)> {
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
            let adopted = def.adopted
                || source.layout()
                || (def.manuals.is_empty()
                    && def.divisions.is_empty()
                    && def.stops.is_empty()
                    && def.moves.is_empty());
            Some((def.name, def.adopted, adopted))
        };
        // The marked original outranks a copy made to edit it (which
        // wraps the set the same way, `layout = true` and all); the
        // heuristic alone is for wrappers written before the mark.
        let mut marked: Option<PathBuf> = None;
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
            let Some((organ_name, is_marked, adopted)) = wraps(&candidate) else {
                continue;
            };
            if name.is_some_and(|wanted| organ_name == wanted) {
                return Some(candidate);
            }
            if is_marked {
                marked.get_or_insert(candidate);
            } else if adopted {
                fallback.get_or_insert(candidate);
            }
        }
        marked.or(fallback)
    }

    /// The library entries whose files still exist — what the picker
    /// shows as Recent. A missing file is hidden, not forgotten: sample
    /// sets live on external drives, and an organ that vanished because
    /// its drive is unplugged must reappear when it is mounted again.
    /// Only `forget` removes an entry for good.
    pub fn present(&self) -> impl Iterator<Item = &LibraryEntry> {
        self.library.iter().filter(|entry| entry.path.exists())
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
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub generals: BTreeMap<u8, Registration>,
    /// Divisionals: manual name → piston slot → registration. The
    /// manual is named, like everything else here, so a divisional
    /// made on "Récit" means nothing at all on an organ without one
    /// rather than quietly landing on the wrong division.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub divisionals: BTreeMap<String, BTreeMap<u8, Registration>>,
    /// The stepper's frames, in playing order — a general-shaped
    /// registration each, walked by one thumb during a piece.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub frames: Vec<Registration>,
    /// The crescendo pedal's stages: stage number (1..=32) → the stops
    /// that stage *adds*. The pedal sounds the union of every stage up
    /// to where it stands, over whatever the hand has drawn.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub crescendo: BTreeMap<u8, Vec<String>>,
}

/// One stored registration: the console state a piston brings back.
/// The same shape serves a general, a divisional and a stepper frame —
/// they differ in what *scope* the recall applies them over, not in
/// what is remembered.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Registration {
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
    write_atomically(&path, body)?;
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
    // The set's own organ, kept as the set defines it: edits refuse
    // until it is saved under another name (`copy_composite_as`).
    doc["adopted"] = toml_edit::value(true);
    let mut sources = toml_edit::Table::new();
    let mut source = toml_edit::InlineTable::new();
    source.insert("path", set.to_string_lossy().as_ref().into());
    source.insert("layout", true.into());
    sources.insert("s1", toml_edit::value(source));
    doc["sources"] = toml_edit::Item::Table(sources);

    let manuals = build_manual_table(organ);
    if !manuals.is_empty() {
        doc["manual"] = toml_edit::Item::ArrayOfTables(manuals);
    }

    let stops = build_stop_pulls(organ);
    if !stops.is_empty() {
        doc["stop"] = toml_edit::Item::ArrayOfTables(stops);
    }

    carry_over_sidecar(&mut doc, set);
    build_coupler_defines(&mut doc, organ)?;

    write_wrapper_organ(&path, doc, wiring)?;
    Ok(path)
}

/// The assembled organ's manual, by id — every extraction below that
/// writes a manual name into the file looks it up the same way.
fn manual_name(organ: &aristide_model::Organ, id: aristide_model::ManualId) -> Option<&str> {
    organ
        .manuals
        .iter()
        .find(|manual| manual.id == id)
        .map(|manual| manual.name.as_str())
}

/// One `[[manual]]` table per manual, in the set's own order.
fn build_manual_table(organ: &aristide_model::Organ) -> toml_edit::ArrayOfTables {
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
    manuals
}

/// One pull per stop, in the set's own order. Exact names match
/// exactly (and all together): two same-named stops on one manual
/// are one line pulling both, so the line is written once.
fn build_stop_pulls(organ: &aristide_model::Organ) -> toml_edit::ArrayOfTables {
    let mut stops = toml_edit::ArrayOfTables::new();
    let mut written: std::collections::HashSet<(String, String)> = Default::default();
    for stop in &organ.stops {
        let Some(manual) = manual_name(organ, stop.manual) else {
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
    stops
}

/// Carry the set's sidecar into the new file verbatim, minus the
/// structural keys this file owns itself — a sidecar has no business
/// declaring them anyway — and resolve the one value that's relative
/// to the set rather than to this file.
fn carry_over_sidecar(doc: &mut toml_edit::DocumentMut, set: &Path) {
    let sidecar_path = aristide_formats::sidecar::path_for(set);
    match std::fs::read_to_string(&sidecar_path) {
        Ok(text) => match text.parse::<toml_edit::DocumentMut>() {
            Ok(sidecar) => {
                for (key, item) in sidecar.iter() {
                    // A sidecar rename became this file's own name
                    // above; the structural keys are this file's own —
                    // a sidecar has no business declaring them anyway.
                    // `[samples]` was the machine's business all
                    // along (residency, streaming) and lives in the
                    // user config now, so a wrapper never inherits it.
                    if !matches!(
                        key,
                        "name" | "sources" | "manual" | "division" | "stop" | "move" | "samples"
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
}

/// The set's couplers, snapshotted as route definitions — ahead of any
/// the sidecar defines, matching the order a direct load gives them
/// (the set's own first, the player's custom ones after).
fn build_coupler_defines(
    doc: &mut toml_edit::DocumentMut,
    organ: &aristide_model::Organ,
) -> Result<(), String> {
    let mut defines = toml_edit::ArrayOfTables::new();
    for coupler in &organ.couplers {
        let mut routes = toml_edit::ArrayOfTables::new();
        let mut sound = true;
        for route in &coupler.routes {
            let mut table = toml_edit::Table::new();
            let Some(from) = manual_name(organ, route.from_manual) else {
                sound = false;
                break;
            };
            table["from"] = toml_edit::value(from);
            if let Some(target) = &route.target {
                let Some(to) = manual_name(organ, target.manual) else {
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
    Ok(())
}

/// Render the finished document with its header comment and write it,
/// then lay down its MIDI wiring when it declares any.
fn write_wrapper_organ(
    path: &Path,
    doc: toml_edit::DocumentMut,
    wiring: Option<&OrganConfig>,
) -> Result<(), String> {
    let body = format!(
        "# An Aristide organ, born from the sample set under [sources].\n\
         # This file is the whole instrument — manuals, stops, couplers,\n\
         # wiring and settings live here; the set only supplies pipes\n\
         # and samples. Edit freely: the console writes back here too.\n\
         {doc}"
    );
    write_atomically(path, body)?;
    if wiring.is_some_and(|organ| organ != &OrganConfig::default()) {
        write_composite_midi(path, wiring)?;
    }
    Ok(())
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

pub fn save(path: &Path, config: &MidiConfig) -> Result<(), String> {
    let body = toml::to_string_pretty(config).map_err(|err| err.to_string())?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|err| format!("{}: {err}", parent.display()))?;
    }
    write_atomically(path, format!("{HEADER}{body}"))
}

/// A composite organ file's `[midi]` wiring and `[combinations]`
/// memory in the shape the server keeps them. The file is that organ's
/// authority: this replaces whatever the user config remembers under
/// its name — which is exactly why the combinations have to come along.
/// (Before they did, every reload of a composite silently wiped the
/// generals: `install` swaps in this value wholesale.)
pub fn organ_config_from_file(
    midi: &aristide_formats::instrument::MidiDef,
    combinations: &aristide_formats::instrument::CombinationsDef,
) -> OrganConfig {
    let mut organ = OrganConfig::default();
    for general in &combinations.generals {
        organ.generals.insert(
            general.n,
            Registration {
                stops: general.stops.clone(),
                couplers: general.couplers.clone(),
                tremulants: general.tremulants.clone(),
            },
        );
    }
    for divisional in &combinations.divisionals {
        organ
            .divisionals
            .entry(divisional.manual.clone())
            .or_default()
            .insert(
                divisional.n,
                Registration {
                    stops: divisional.stops.clone(),
                    couplers: divisional.couplers.clone(),
                    tremulants: divisional.tremulants.clone(),
                },
            );
    }
    // `n` decides the order, not the order the tables happen to sit in
    // the file, so a hand-renumbered sequence reads back as written.
    // Gaps close: the stepper walks positions, not slot numbers.
    let mut frames: Vec<_> = combinations.frames.iter().collect();
    frames.sort_by_key(|frame| frame.n);
    organ.frames = frames
        .into_iter()
        .map(|frame| Registration {
            stops: frame.stops.clone(),
            couplers: frame.couplers.clone(),
            tremulants: frame.tremulants.clone(),
        })
        .collect();
    for stage in &combinations.crescendo {
        organ.crescendo.insert(stage.stage, stage.stops.clone());
    }
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
    write_atomically(path, doc.to_string())
}

/// Rewrite a composite organ file's `[combinations]` tables to match
/// the live combination memory, touching nothing else — same
/// comment-preserving contract as the wiring above, because the same
/// hand-authored file holds both.
///
/// The three `divisional_*` flags are *not* ours to write: absent they
/// mean "as the sample set has it", which is right for almost every
/// organ, and a player who disagrees says so by hand. Leaving them
/// alone here keeps a hand-written override from being erased by the
/// next piston press.
pub fn write_composite_combinations(
    path: &Path,
    organ: Option<&OrganConfig>,
) -> Result<(), String> {
    let text =
        std::fs::read_to_string(path).map_err(|err| format!("{}: {err}", path.display()))?;
    let mut doc: toml_edit::DocumentMut =
        text.parse().map_err(|err| format!("{}: {err}", path.display()))?;
    let table = doc
        .entry("combinations")
        .or_insert(toml_edit::Item::Table(toml_edit::Table::new()));
    let table = table
        .as_table_mut()
        .ok_or_else(|| "[combinations] is not a table".to_string())?;
    let registration = |target: &mut toml_edit::Table, reg: &Registration| {
        for (key, names) in [
            ("stops", &reg.stops),
            ("couplers", &reg.couplers),
            ("tremulants", &reg.tremulants),
        ] {
            if names.is_empty() {
                continue;
            }
            let mut array = toml_edit::Array::new();
            for name in names {
                array.push(name.as_str());
            }
            target[key] = toml_edit::value(array);
        }
    };

    let mut generals = toml_edit::ArrayOfTables::new();
    let mut divisionals = toml_edit::ArrayOfTables::new();
    let mut frames = toml_edit::ArrayOfTables::new();
    let mut crescendo = toml_edit::ArrayOfTables::new();
    if let Some(organ) = organ {
        for (slot, general) in &organ.generals {
            let mut entry = toml_edit::Table::new();
            entry["n"] = toml_edit::value(*slot as i64);
            registration(&mut entry, general);
            generals.push(entry);
        }
        for (manual, slots) in &organ.divisionals {
            for (slot, divisional) in slots {
                let mut entry = toml_edit::Table::new();
                entry["manual"] = toml_edit::value(manual.as_str());
                entry["n"] = toml_edit::value(*slot as i64);
                registration(&mut entry, divisional);
                divisionals.push(entry);
            }
        }
        for (index, frame) in organ.frames.iter().enumerate() {
            let mut entry = toml_edit::Table::new();
            entry["n"] = toml_edit::value(index as i64 + 1);
            registration(&mut entry, frame);
            frames.push(entry);
        }
        for (stage, stops) in &organ.crescendo {
            if stops.is_empty() {
                continue;
            }
            let mut entry = toml_edit::Table::new();
            entry["stage"] = toml_edit::value(*stage as i64);
            let mut array = toml_edit::Array::new();
            for name in stops {
                array.push(name.as_str());
            }
            entry["stops"] = toml_edit::value(array);
            crescendo.push(entry);
        }
    }
    for (key, tables) in [
        ("general", generals),
        ("divisional", divisionals),
        ("frame", frames),
        ("crescendo", crescendo),
    ] {
        if tables.is_empty() {
            table.remove(key);
        } else {
            table[key] = toml_edit::Item::ArrayOfTables(tables);
        }
    }
    // Only the flags need a `[combinations]` header of their own; with
    // just the arrays present, `[[combinations.general]]` says it, and
    // a bare header above them would be noise.
    let has_flags = [
        "divisional_intermanual_couplers",
        "divisional_intramanual_couplers",
        "divisional_tremulants",
    ]
    .iter()
    .any(|key| table.contains_key(key));
    table.set_implicit(!has_flags);
    write_atomically(path, doc.to_string())
}

/// A manual's own tuning as the file spells it: temperament name,
/// pitch reference (which key, what Hz), transpose, and optionally a
/// Scala scale (.scl path and .kbm path) standing in for the
/// temperament.
#[derive(Debug, Clone, PartialEq)]
pub struct ManualTuningFields {
    pub temperament: String,
    /// Divisions per octave; 12 writes as absence, the file's default.
    pub edo: u16,
    pub reference: crate::tuning::PitchReference,
    pub transpose: i8,
    pub scale: Option<String>,
    pub keymap: Option<String>,
    pub pipes: crate::tuning::PipeRetune,
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
            write_tuning_fields(&mut table, tuning, true);
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
    write_atomically(path, doc.to_string())
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
            write_tuning_fields(table, fields, true);
        }
        None => {
            for field in [
                "temperament",
                "edo",
                "reference_key",
                "reference_hz",
                "a4_hz",
                "transpose",
                "scale",
                "keymap",
                "pipes",
            ] {
                table.remove(field);
            }
        }
    })
}

/// The tuning lines of one `[[manual]]` table. A scale replaces the
/// temperament line (a Scala scale IS the temperament); absent fields
/// are removed so the file never says two things at once.
fn write_tuning_fields(table: &mut toml_edit::Table, fields: &ManualTuningFields, transpose: bool) {
    // A scale supersedes both the temperament and the division count;
    // and at 12 divisions — the file's default — the edo line goes,
    // while the temperament line goes at any other count (twelve-class
    // vocabulary means nothing there). The file never says two things
    // at once.
    if fields.scale.is_some() || fields.edo != 12 {
        table.remove("temperament");
    } else {
        table["temperament"] = toml_edit::value(fields.temperament.as_str());
    }
    if fields.scale.is_some() || fields.edo == 12 {
        table.remove("edo");
    } else {
        table["edo"] = toml_edit::value(fields.edo as i64);
    }
    // The anchor is written as the pair it is; the older `a4_hz`
    // spelling (always an A4 anchor) goes so the file says it once.
    table["reference_key"] =
        toml_edit::value(aristide_formats::sidecar::note_name(fields.reference.key));
    table["reference_hz"] = toml_edit::value(fields.reference.hz);
    table.remove("a4_hz");
    // Transposition is a keyboard's: a set's or a stop's tuning never
    // carries the line.
    if transpose {
        table["transpose"] = toml_edit::value(fields.transpose as i64);
    } else {
        table.remove("transpose");
    }
    for (key, value) in [("scale", &fields.scale), ("keymap", &fields.keymap)] {
        match value {
            Some(value) => table[key] = toml_edit::value(value.as_str()),
            None => {
                table.remove(key);
            }
        }
    }
    // Pipes keep their drift by default; only `exact` is worth a line.
    match fields.pipes {
        crate::tuning::PipeRetune::Exact => table["pipes"] = toml_edit::value("exact"),
        crate::tuning::PipeRetune::Original => {
            table.remove("pipes");
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

/// Write the organ file's `[tremulant]` section — the hand-declared
/// synth tremulant, in the sidecar's own vocabulary (Hz, pitch cents,
/// seconds, percent). Creates the table when the file has none;
/// preserves an existing `chests` line, since the console edits the
/// shape, not the wind plan. Note: a declared `[tremulant]` supersedes
/// a sample set's own ODF tremulants at load — declaring one is what
/// this edit means.
pub fn write_composite_tremulant(
    path: &Path,
    rate_hz: f64,
    depth_cents: f64,
    ramp_s: f64,
    wobble_pct: f64,
) -> Result<(), String> {
    let text =
        std::fs::read_to_string(path).map_err(|err| format!("{}: {err}", path.display()))?;
    let mut doc: toml_edit::DocumentMut =
        text.parse().map_err(|err| format!("{}: {err}", path.display()))?;
    let table = doc
        .entry("tremulant")
        .or_insert(toml_edit::Item::Table(toml_edit::Table::new()));
    let Some(table) = table.as_table_mut() else {
        return Err("[tremulant] is not a table".into());
    };
    let round = |v: f64, places: f64| (v * places).round() / places;
    table["rate_hz"] = toml_edit::value(round(rate_hz, 10.0));
    table["depth_cents"] = toml_edit::value(round(depth_cents, 10.0));
    table["ramp_s"] = toml_edit::value(round(ramp_s, 100.0));
    table["wobble_pct"] = toml_edit::value(round(wobble_pct, 10.0));
    write_atomically(path, doc.to_string())
}

/// Write the organ file's top-level `[tuning]` table — the
/// whole-instrument temperament/EDO/a′/transpose/scale the console
/// edits directly (as opposed to one manual's own override, which
/// lives on its `[[manual]]` table). Same absence rules as a manual's
/// tuning: a scale supersedes the temperament and the division count,
/// and 12 EDO is the file's default so it is never written.
pub fn write_composite_tuning(path: &Path, fields: &ManualTuningFields) -> Result<(), String> {
    let mut doc = composite_doc(path)?;
    let table = doc
        .entry("tuning")
        .or_insert(toml_edit::Item::Table(toml_edit::Table::new()));
    let Some(table) = table.as_table_mut() else {
        return Err("[tuning] is not a table".into());
    };
    write_tuning_fields(table, fields, true);
    write_atomically(path, doc.to_string())
}

/// The tuning lines every scope's own tuning is written with — no
/// transpose — minus nothing: the same fields, so a scope's table
/// reads like `[tuning]` itself.
const TUNING_FIELD_KEYS: [&str; 9] = [
    "temperament",
    "edo",
    "reference_key",
    "reference_hz",
    "a4_hz",
    "transpose",
    "scale",
    "keymap",
    "pipes",
];

/// Update (or with `None` remove) one source set's own tuning — its
/// `[sources.<alias>.tuning]` table. A source spelled as a bare path
/// becomes a table with the same `path` so the tuning has somewhere
/// to live. `Ok(false)` when the file has no such source.
pub fn write_composite_source_tuning(
    path: &Path,
    alias: &str,
    tuning: Option<&ManualTuningFields>,
) -> Result<bool, String> {
    let mut doc = composite_doc(path)?;
    let Some(sources) = doc.get_mut("sources").and_then(|s| s.as_table_mut()) else {
        return Ok(false);
    };
    let Some(set_path) = sources.get(alias) else {
        return Ok(false);
    };
    // A bare path becomes a table of its own — a fresh entry, so the
    // path line's decor (a trailing label comment) doesn't cling to
    // the table header.
    if let Some(set_path) = set_path.as_str().map(str::to_string) {
        sources.remove(alias);
        let mut table = toml_edit::Table::new();
        table["path"] = toml_edit::value(set_path);
        sources.insert(alias, toml_edit::Item::Table(table));
    }
    let Some(source) = sources.get_mut(alias).and_then(|item| item.as_table_like_mut()) else {
        return Err(format!("[sources.{alias}] is neither a path nor a table"));
    };
    match tuning {
        Some(fields) => {
            let entry = source
                .entry("tuning")
                .or_insert(toml_edit::Item::Table(toml_edit::Table::new()));
            let Some(table) = entry.as_table_like_mut() else {
                return Err(format!("[sources.{alias}.tuning] is not a table"));
            };
            let mut scratch = toml_edit::Table::new();
            for (key, value) in table.iter() {
                scratch.insert(key, value.clone());
            }
            write_tuning_fields(&mut scratch, fields, false);
            for key in TUNING_FIELD_KEYS {
                match scratch.get(key) {
                    Some(value) => {
                        table.insert(key, value.clone());
                    }
                    None => {
                        table.remove(key);
                    }
                }
            }
        }
        None => {
            source.remove("tuning");
        }
    }
    write_atomically(path, doc.to_string())?;
    Ok(true)
}

/// What a `[[tuning.stop]]` row says about its stop.
#[derive(Debug, Clone, PartialEq)]
pub enum StopTuningEntry {
    /// `follow = "<scope>"` — a pin.
    Follow(String),
    /// A tuning of the stop's (or the rank's) own.
    Own(ManualTuningFields),
}

/// Write (or with `None` remove) the `[[tuning.stop]]` row for one
/// stop — or for one rank within it, with `rank` — matched by console
/// stop name, manual name and rank name (all case-insensitive; a row
/// naming no manual matches any). One row per coordinate: writing
/// replaces what was there. The rows live under `[tuning]`, which is
/// left implicit when the file had none, so a file gains
/// `[[tuning.stop]]` and nothing else.
pub fn write_composite_stop_tuning(
    path: &Path,
    stop_name: &str,
    manual_name: &str,
    rank: Option<&str>,
    entry: Option<StopTuningEntry>,
) -> Result<(), String> {
    let mut doc = composite_doc(path)?;
    let is_row = |table: &toml_edit::Table| {
        let field = |key: &str| table.get(key).and_then(|v| v.as_str());
        field("stop").is_some_and(|name| name.eq_ignore_ascii_case(stop_name))
            && field("manual").is_none_or(|m| m.eq_ignore_ascii_case(manual_name))
            && match (field("rank"), rank) {
                (None, None) => true,
                (Some(a), Some(b)) => a.eq_ignore_ascii_case(b),
                _ => false,
            }
    };
    fn rows_of(doc: &mut toml_edit::DocumentMut) -> Option<&mut toml_edit::ArrayOfTables> {
        doc.get_mut("tuning")?
            .as_table_mut()?
            .get_mut("stop")?
            .as_array_of_tables_mut()
    }
    let Some(entry) = entry else {
        if let Some(rows) = rows_of(&mut doc) {
            rows.retain(|row| !is_row(row));
            if rows.is_empty()
                && let Some(tuning) = doc.get_mut("tuning").and_then(|t| t.as_table_mut())
            {
                tuning.remove("stop");
                if tuning.is_empty() {
                    doc.remove("tuning");
                }
            }
        }
        return write_atomically(path, doc.to_string());
    };
    let tuning = doc
        .entry("tuning")
        .or_insert(toml_edit::Item::Table(toml_edit::Table::new()));
    let Some(tuning) = tuning.as_table_mut() else {
        return Err("[tuning] is not a table".into());
    };
    if tuning.iter().all(|(key, _)| key == "stop") {
        tuning.set_implicit(true);
    }
    let rows = tuning
        .entry("stop")
        .or_insert(toml_edit::Item::ArrayOfTables(toml_edit::ArrayOfTables::new()));
    let Some(rows) = rows.as_array_of_tables_mut() else {
        return Err("[[tuning.stop]] is not an array of tables".into());
    };
    let index = (0..rows.len()).find(|&i| rows.get(i).is_some_and(&is_row));
    let table = match index {
        Some(index) => rows.get_mut(index).expect("row just found"),
        None => {
            let mut table = toml_edit::Table::new();
            table["stop"] = toml_edit::value(stop_name);
            table["manual"] = toml_edit::value(manual_name);
            if let Some(rank) = rank {
                table["rank"] = toml_edit::value(rank);
            }
            rows.push(table);
            let last = rows.len() - 1;
            rows.get_mut(last).expect("row just pushed")
        }
    };
    match entry {
        StopTuningEntry::Follow(scope) => {
            for key in TUNING_FIELD_KEYS {
                table.remove(key);
            }
            table["follow"] = toml_edit::value(scope);
        }
        StopTuningEntry::Own(fields) => {
            table.remove("follow");
            write_tuning_fields(table, &fields, false);
        }
    }
    write_atomically(path, doc.to_string())
}

/// Set `wet` in an existing `[reverb]` table. An organ file with none —
/// no impulse response, nothing to wet — is left exactly as it is
/// rather than growing a `[reverb]` section that would otherwise mean
/// nothing to the loader.
pub fn write_composite_reverb_wet(path: &Path, wet: f64) -> Result<(), String> {
    let mut doc = composite_doc(path)?;
    let Some(item) = doc.get_mut("reverb") else {
        return Ok(());
    };
    let Some(table) = item.as_table_mut() else {
        return Err("[reverb] is not a table".into());
    };
    table["wet"] = toml_edit::value(wet);
    write_atomically(path, doc.to_string())
}

/// Write the organ file's `[noises]` table — drawstop thumps, coupler
/// clacks, the blower — creating it when the file has none.
pub fn write_composite_noises(path: &Path, enabled: bool, volume: f64) -> Result<(), String> {
    let mut doc = composite_doc(path)?;
    let table = doc
        .entry("noises")
        .or_insert(toml_edit::Item::Table(toml_edit::Table::new()));
    let Some(table) = table.as_table_mut() else {
        return Err("[noises] is not a table".into());
    };
    table["enabled"] = toml_edit::value(enabled);
    table["volume"] = toml_edit::value(volume);
    write_atomically(path, doc.to_string())
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
/// Copy the organ file at `path` into `dir` as an organ named `name`:
/// the same file line for line — inventory, sidecar sections, wiring,
/// layout — under the new name, with the `adopted` flag dropped so the
/// copy takes edits. This is how a sample set's own organ becomes the
/// player's: the original stays exactly as the set defines it.
pub fn copy_composite_as(path: &Path, dir: &Path, name: &str) -> Result<PathBuf, String> {
    let mut doc = composite_doc(path)?;
    let current = doc.get("name").and_then(|item| item.as_str()).unwrap_or("");
    if current.trim() == name.trim() {
        return Err("give the copy a name of its own".into());
    }
    let (name, target) = organ_file_path(dir, name)?;
    doc["name"] = toml_edit::value(name);
    doc.remove("adopted");
    write_atomically(&target, doc.to_string())?;
    Ok(target)
}

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
    if let Some(order) = console_order_mut(&mut doc)
        && let Some(item) = order.remove(from)
    {
        order.insert(to, item);
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

// ---- per-stop edits, addressed by provenance ------------------------
//
// The console edits ONE stop; the file speaks in pulls. Provenance
// (which source, which of its manuals, which of its stops, via a
// [[stop]] line or a [[division]] pull) is how each edit finds the
// line that brought the stop in. All of these answer `Ok(false)` when
// the file holds no such line — a hand-edited file the console
// shouldn't guess about.

/// The `[[stop]]` line that pulled this provenance in, by index.
/// `on` breaks ties when the same source stop was pulled twice.
fn stop_pull_index(
    stops: &toml_edit::ArrayOfTables,
    prov: &instrument::StopProvenance,
    on: &str,
) -> Option<usize> {
    let named: Vec<usize> = (0..stops.len())
        .filter(|&i| {
            stops.get(i).is_some_and(|table| {
                field_is(table, "from", &prov.source)
                    && field_is(table, "stop", &prov.source_stop)
                    && table
                        .get("manual")
                        .and_then(|value| value.as_str())
                        .is_none_or(|manual| manual.eq_ignore_ascii_case(&prov.source_manual))
            })
        })
        .collect();
    match named.as_slice() {
        [] => None,
        [one] => Some(*one),
        several => several
            .iter()
            .copied()
            .find(|&i| stops.get(i).is_some_and(|table| field_is(table, "on", on))),
    }
}

/// The `[[division]]` line that pulled this provenance in, by index.
fn division_pull_index(
    doc: &toml_edit::DocumentMut,
    prov: &instrument::StopProvenance,
    on: &str,
) -> Option<usize> {
    let divisions = doc.get("division")?.as_array_of_tables()?;
    let named: Vec<usize> = (0..divisions.len())
        .filter(|&i| {
            divisions.get(i).is_some_and(|table| {
                field_is(table, "from", &prov.source)
                    && table
                        .get("manual")
                        .and_then(|value| value.as_str())
                        .is_some_and(|pattern| {
                            !aristide_formats::sidecar::match_names(
                                &[prov.source_manual.as_str()],
                                pattern,
                            )
                            .is_empty()
                        })
            })
        })
        .collect();
    match named.as_slice() {
        [] => None,
        [one] => Some(*one),
        several => several
            .iter()
            .copied()
            .find(|&i| divisions.get(i).is_some_and(|table| field_is(table, "on", on))),
    }
}

/// Leave one source stop out of a `[[division]]` pull.
fn division_except_add(table: &mut toml_edit::Table, source_stop: &str) {
    let item = table
        .entry("except")
        .or_insert(toml_edit::Item::Value(toml_edit::Array::new().into()));
    if let Some(array) = item.as_value_mut().and_then(|value| value.as_array_mut())
        && !array
            .iter()
            .any(|value| value.as_str().is_some_and(|s| s.eq_ignore_ascii_case(source_stop)))
    {
        array.push(source_stop);
    }
}

/// Set (or with `to == None` drop) one entry of a `[[division]]`
/// pull's per-stop map (`rename`, `pitch_label`), whichever TOML
/// spelling the map uses.
fn division_map_set(table: &mut toml_edit::Table, map: &str, key: &str, to: Option<&str>) {
    if to.is_none() && table.get(map).is_none() {
        return;
    }
    let item = table
        .entry(map)
        .or_insert(toml_edit::Item::Value(toml_edit::InlineTable::new().into()));
    let mut empty = false;
    if let Some(inline) = item.as_value_mut().and_then(|value| value.as_inline_table_mut()) {
        match to {
            Some(to) => {
                inline.insert(key, to.into());
            }
            None => {
                inline.remove(key);
            }
        }
        empty = inline.is_empty();
    } else if let Some(table) = item.as_table_mut() {
        match to {
            Some(to) => table[key] = toml_edit::value(to),
            None => {
                table.remove(key);
            }
        }
        empty = table.is_empty();
    }
    if empty {
        table.remove(map);
    }
}

/// One entry of a `[[division]]` pull's per-stop boolean map.
fn division_map_get_bool(table: &toml_edit::Table, map: &str, key: &str) -> Option<bool> {
    let item = table.get(map)?;
    if let Some(inline) = item.as_value().and_then(|value| value.as_inline_table()) {
        inline.get(key)?.as_bool()
    } else {
        item.as_table()?.get(key)?.as_bool()
    }
}

/// One entry of a `[[division]]` pull's per-stop map, as written.
fn division_map_get(table: &toml_edit::Table, map: &str, key: &str) -> Option<String> {
    let item = table.get(map)?;
    let value = if let Some(inline) = item.as_value().and_then(|value| value.as_inline_table()) {
        inline.get(key)?.as_str()
    } else {
        item.as_table()?.get(key)?.as_str()
    };
    value.map(str::to_string)
}

fn division_rename_set(table: &mut toml_edit::Table, source_stop: &str, to: Option<&str>) {
    division_map_set(table, "rename", source_stop, to);
}

/// Rename every exact `from` in a string array (an enclosure's
/// `stops`, a voicing rule's) — patterns are left alone; only a value
/// that names the stop exactly follows the rename.
fn rename_in_string_array(table: &mut toml_edit::Table, key: &str, from: &str, to: &str) {
    if let Some(array) = table.get_mut(key).and_then(|item| item.as_array_mut()) {
        for value in array.iter_mut() {
            if value.as_str().is_some_and(|s| s.eq_ignore_ascii_case(from)) {
                let decor = value.decor().clone();
                *value = to.into();
                *value.decor_mut() = decor;
            }
        }
    }
}

/// Every `[[voicing.adjust]]` table, mutably.
fn voicing_adjusts_mut(
    doc: &mut toml_edit::DocumentMut,
) -> Option<&mut toml_edit::ArrayOfTables> {
    doc.get_mut("voicing")?
        .get_mut("adjust")?
        .as_array_of_tables_mut()
}

/// Drop the `[[move]]` lines about a console stop name.
fn remove_moves_for(doc: &mut toml_edit::DocumentMut, stop: &str) {
    if let Some(moves) = doc.get_mut("move").and_then(|m| m.as_array_of_tables_mut()) {
        moves.retain(|table| !field_is(table, "stop", stop));
        if moves.is_empty() {
            doc.remove("move");
        }
    }
}

/// Rename a stop in a composite file: the pull that brought it in
/// carries the console name (a `rename` field on its `[[stop]]` line,
/// or an entry in its `[[division]]` line's `rename` map), and every
/// exact reference to the old name — `[[move]]` lines, `[[enclosure]]`
/// member lists, `[[voicing.adjust]]` rules — follows, or the rename
/// would silently unwire them.
pub fn rename_composite_stop(
    path: &Path,
    prov: &instrument::StopProvenance,
    on: &str,
    old: &str,
    new: &str,
) -> Result<bool, String> {
    let mut doc = composite_doc(path)?;
    if prov.via_division {
        let Some(index) = division_pull_index(&doc, prov, on) else {
            return Ok(false);
        };
        let table = doc["division"]
            .as_array_of_tables_mut()
            .and_then(|tables| tables.get_mut(index))
            .expect("division line just found");
        let back_to_source = new.eq_ignore_ascii_case(&prov.source_stop);
        division_rename_set(table, &prov.source_stop, (!back_to_source).then_some(new));
    } else {
        let Some(index) = doc
            .get("stop")
            .and_then(|s| s.as_array_of_tables())
            .and_then(|stops| stop_pull_index(stops, prov, on))
        else {
            return Ok(false);
        };
        let table = doc["stop"]
            .as_array_of_tables_mut()
            .and_then(|tables| tables.get_mut(index))
            .expect("stop line just found");
        if new.eq_ignore_ascii_case(&prov.source_stop) {
            table.remove("rename");
        } else {
            set_string_preserving(table, "rename", new);
        }
    }
    for table in tables_mut(&mut doc, "move") {
        rename_field(table, "stop", old, new);
    }
    for table in tables_mut(&mut doc, "enclosure") {
        rename_in_string_array(table, "stops", old, new);
    }
    if let Some(adjusts) = voicing_adjusts_mut(&mut doc) {
        for table in adjusts.iter_mut() {
            rename_in_string_array(table, "stops", old, new);
        }
    }
    if let Some(order) = console_order_mut(&mut doc) {
        for (_, item) in order.iter_mut() {
            if let Some(array) = item.as_array_mut() {
                for value in array.iter_mut() {
                    if value.as_str().is_some_and(|v| v.eq_ignore_ascii_case(old)) {
                        let decor = value.decor().clone();
                        *value = new.into();
                        *value.decor_mut() = decor;
                    }
                }
            }
        }
    }
    write_atomically(path, doc.to_string())?;
    Ok(true)
}

/// Write (or with `None`, remove) a stop's declared knob engraving —
/// the `pitch_label` field on its `[[stop]]` line, or the entry in its
/// `[[division]]` line's `pitch_label` map. `""` engraves nothing;
/// absent, the knob shows the footage the stop actually speaks at.
pub fn write_composite_stop_pitch_label(
    path: &Path,
    prov: &instrument::StopProvenance,
    on: &str,
    label: Option<&str>,
) -> Result<bool, String> {
    let mut doc = composite_doc(path)?;
    if prov.via_division {
        let Some(index) = division_pull_index(&doc, prov, on) else {
            return Ok(false);
        };
        let table = doc["division"]
            .as_array_of_tables_mut()
            .and_then(|tables| tables.get_mut(index))
            .expect("division line just found");
        division_map_set(table, "pitch_label", &prov.source_stop, label);
    } else {
        let Some(index) = doc
            .get("stop")
            .and_then(|s| s.as_array_of_tables())
            .and_then(|stops| stop_pull_index(stops, prov, on))
        else {
            return Ok(false);
        };
        let table = doc["stop"]
            .as_array_of_tables_mut()
            .and_then(|tables| tables.get_mut(index))
            .expect("stop line just found");
        match label {
            Some(label) => set_string_preserving(table, "pitch_label", label),
            None => {
                table.remove("pitch_label");
            }
        }
    }
    write_atomically(path, doc.to_string())?;
    Ok(true)
}

/// Set (or with `to == None` drop) one entry of a `[[division]]`
/// pull's per-stop *boolean* map (`own_pipes`) — the bool twin of
/// [`division_map_set`].
fn division_map_set_bool(table: &mut toml_edit::Table, map: &str, key: &str, to: Option<bool>) {
    if to.is_none() && table.get(map).is_none() {
        return;
    }
    let item = table
        .entry(map)
        .or_insert(toml_edit::Item::Value(toml_edit::InlineTable::new().into()));
    let mut empty = false;
    if let Some(inline) = item.as_value_mut().and_then(|value| value.as_inline_table_mut()) {
        match to {
            Some(to) => {
                inline.insert(key, to.into());
            }
            None => {
                inline.remove(key);
            }
        }
        empty = inline.is_empty();
    } else if let Some(table) = item.as_table_mut() {
        match to {
            Some(to) => table[key] = toml_edit::value(to),
            None => {
                table.remove(key);
            }
        }
        empty = table.is_empty();
    }
    if empty {
        table.remove(map);
    }
}

/// Write (or with `None`/`false`, remove — shared is the default) a
/// stop's pipe-sharing declaration: the `own_pipes` field on its
/// `[[stop]]` line, or the entry in its `[[division]]` line's
/// `own_pipes` map.
pub fn write_composite_stop_own_pipes(
    path: &Path,
    prov: &instrument::StopProvenance,
    on: &str,
    own: bool,
) -> Result<bool, String> {
    let mut doc = composite_doc(path)?;
    // `false` is the default, so it is spelled by absence.
    let own = own.then_some(true);
    if prov.via_division {
        let Some(index) = division_pull_index(&doc, prov, on) else {
            return Ok(false);
        };
        let table = doc["division"]
            .as_array_of_tables_mut()
            .and_then(|tables| tables.get_mut(index))
            .expect("division line just found");
        division_map_set_bool(table, "own_pipes", &prov.source_stop, own);
    } else {
        let Some(index) = doc
            .get("stop")
            .and_then(|s| s.as_array_of_tables())
            .and_then(|stops| stop_pull_index(stops, prov, on))
        else {
            return Ok(false);
        };
        let table = doc["stop"]
            .as_array_of_tables_mut()
            .and_then(|tables| tables.get_mut(index))
            .expect("stop line just found");
        match own {
            Some(own) => table["own_pipes"] = toml_edit::value(own),
            None => {
                table.remove("own_pipes");
            }
        }
    }
    write_atomically(path, doc.to_string())?;
    Ok(true)
}

/// The key span a `[[voicing.adjust]]` table narrows to, whichever of
/// `key`/`keys` it spells it with.
fn table_key_span(table: &toml_edit::Table) -> Option<(i32, i32)> {
    if let Some(key) = table.get("key") {
        return match key.as_integer() {
            Some(number) => i32::try_from(number).ok().map(|key| (key, key)),
            None => key.as_str().and_then(aristide_formats::sidecar::parse_key_span),
        };
    }
    table
        .get("keys")
        .and_then(|item| item.as_str())
        .and_then(aristide_formats::sidecar::parse_key_span)
}

/// Write (or with everything neutral, remove) one `[[voicing.adjust]]`
/// rule of the console's own — a rule whose `stops` is exactly this
/// stop's console name, narrowed to `scope`. Pattern rules, and rules
/// at other scopes on the same stop, are never touched.
///
/// `named` spells the key span with note names (a hand keyboard) or
/// raw numbers (a microtonal one); matching an existing rule always
/// goes through the parsed span, so a hand-written spelling is found
/// and rewritten in place whichever way it was typed.
#[allow(clippy::too_many_arguments)]
pub fn write_composite_voicing(
    path: &Path,
    stop_name: &str,
    scope: &crate::load::VoicingScope,
    named: bool,
    feet: Option<f64>,
    cents: Option<f64>,
    gain_db: Option<f64>,
    brightness_db: Option<f64>,
) -> Result<(), String> {
    let mut doc = composite_doc(path)?;
    let own = |table: &toml_edit::Table| {
        table
            .get("stops")
            .and_then(|item| item.as_array())
            .is_some_and(|array| {
                array.len() == 1
                    && array
                        .iter()
                        .next()
                        .and_then(|value| value.as_str())
                        .is_some_and(|name| name.eq_ignore_ascii_case(stop_name))
            })
            && table_key_span(table) == scope.keys
            && match (table.get("rank").and_then(|item| item.as_str()), &scope.rank) {
                (None, None) => true,
                (Some(rank), Some(wanted)) => rank.eq_ignore_ascii_case(wanted),
                _ => false,
            }
    };
    let neutral = feet.is_none()
        && cents.is_none_or(|c| c == 0.0)
        && gain_db.is_none_or(|g| g == 0.0)
        && brightness_db.is_none_or(|b| b == 0.0);
    if neutral {
        if let Some(adjusts) = voicing_adjusts_mut(&mut doc) {
            adjusts.retain(|table| !own(table));
            if adjusts.is_empty()
                && let Some(voicing) = doc.get_mut("voicing").and_then(|v| v.as_table_mut())
            {
                voicing.remove("adjust");
                if voicing.is_empty() {
                    doc.remove("voicing");
                }
            }
        }
        return write_atomically(path, doc.to_string());
    }
    let voicing = doc
        .entry("voicing")
        .or_insert(toml_edit::Item::Table(toml_edit::Table::new()));
    let Some(voicing) = voicing.as_table_mut() else {
        return Err("[voicing] is not a table".into());
    };
    voicing.set_implicit(true);
    let adjusts = voicing
        .entry("adjust")
        .or_insert(toml_edit::Item::ArrayOfTables(toml_edit::ArrayOfTables::new()));
    let Some(adjusts) = adjusts.as_array_of_tables_mut() else {
        return Err("[[voicing.adjust]] is not an array of tables".into());
    };
    let index = (0..adjusts.len()).find(|&i| adjusts.get(i).is_some_and(&own));
    let table = match index {
        Some(index) => adjusts.get_mut(index).expect("rule just found"),
        None => {
            let mut table = toml_edit::Table::new();
            let mut stops = toml_edit::Array::new();
            stops.push(stop_name);
            table["stops"] = toml_edit::value(stops);
            adjusts.push(table);
            let last = adjusts.len() - 1;
            adjusts.get_mut(last).expect("rule just pushed")
        }
    };
    // The narrowing is the rule's identity, so it is written before
    // anything else and never left half-spelled: one key goes in as
    // `key`, a span as `keys`.
    table.remove("key");
    table.remove("keys");
    if let Some(span) = scope.keys {
        let spelled = aristide_formats::sidecar::format_key_span(span, named);
        table[if span.0 == span.1 { "key" } else { "keys" }] = toml_edit::value(spelled);
    }
    match &scope.rank {
        Some(rank) => set_string_preserving(table, "rank", rank),
        None => {
            table.remove("rank");
        }
    }
    match feet {
        // Whole feet stay a plain number; a mutation's fraction is
        // written the way it's engraved ("2 2/3").
        Some(feet) if (feet - feet.round()).abs() < 1e-4 => {
            table["pitch"] = toml_edit::value(feet.round() as i64);
        }
        Some(feet) => {
            table["pitch"] = toml_edit::value(aristide_formats::sidecar::format_footage(feet));
        }
        None => {
            table.remove("pitch");
        }
    }
    // A field left unsaid is removed, not written as zero: absence is
    // how a narrow rule leaves a field to the broader one.
    for (key, value) in [
        ("cents", cents),
        ("gain_db", gain_db),
        ("brightness_db", brightness_db),
    ] {
        match value {
            Some(value) if value != 0.0 => table[key] = toml_edit::value(value),
            _ => {
                table.remove(key);
            }
        }
    }
    write_atomically(path, doc.to_string())
}

/// Point a console stop at a different source stop, keeping its place
/// and its label. A `[[stop]]`-pulled stop's own line is rewritten; a
/// division-pulled stop is excepted from its `[[division]]` line and
/// pulled afresh by a `[[stop]]` line of its own. Either way the stop
/// now lands directly on `on`, so its `[[move]]` lines go.
pub fn retarget_composite_stop(
    path: &Path,
    prov: &instrument::StopProvenance,
    console_name: &str,
    on: &str,
    new_from: &str,
    new_manual: &str,
    new_stop: &str,
) -> Result<bool, String> {
    let mut doc = composite_doc(path)?;
    if !doc
        .get("sources")
        .and_then(|sources| sources.as_table())
        .is_some_and(|sources| sources.contains_key(new_from))
    {
        return Err(format!("{new_from:?} is not a [sources] alias of this organ"));
    }
    let keeps_label = !console_name.eq_ignore_ascii_case(new_stop);
    if prov.via_division {
        let Some(index) = division_pull_index(&doc, prov, on) else {
            return Ok(false);
        };
        let table = doc["division"]
            .as_array_of_tables_mut()
            .and_then(|tables| tables.get_mut(index))
            .expect("division line just found");
        division_except_add(table, &prov.source_stop);
        division_rename_set(table, &prov.source_stop, None);
        // The knob engraving is the drawknob's, not the pull's — it
        // rides onto the fresh line along with the label. So does the
        // pipe-sharing declaration.
        let engraving = division_map_get(table, "pitch_label", &prov.source_stop);
        division_map_set(table, "pitch_label", &prov.source_stop, None);
        let owns = division_map_get_bool(table, "own_pipes", &prov.source_stop);
        division_map_set_bool(table, "own_pipes", &prov.source_stop, None);
        let stops = doc
            .entry("stop")
            .or_insert(toml_edit::Item::ArrayOfTables(toml_edit::ArrayOfTables::new()));
        let Some(stops) = stops.as_array_of_tables_mut() else {
            return Err("[[stop]] is not an array of tables".into());
        };
        let mut table = toml_edit::Table::new();
        table["from"] = toml_edit::value(new_from);
        table["manual"] = toml_edit::value(new_manual);
        table["stop"] = toml_edit::value(new_stop);
        table["on"] = toml_edit::value(on);
        if keeps_label {
            table["rename"] = toml_edit::value(console_name);
        }
        if let Some(engraving) = engraving {
            table["pitch_label"] = toml_edit::value(engraving);
        }
        if owns == Some(true) {
            table["own_pipes"] = toml_edit::value(true);
        }
        stops.push(table);
    } else {
        let Some(index) = doc
            .get("stop")
            .and_then(|s| s.as_array_of_tables())
            .and_then(|stops| stop_pull_index(stops, prov, on))
        else {
            return Ok(false);
        };
        let table = doc["stop"]
            .as_array_of_tables_mut()
            .and_then(|tables| tables.get_mut(index))
            .expect("stop line just found");
        set_string_preserving(table, "from", new_from);
        set_string_preserving(table, "manual", new_manual);
        set_string_preserving(table, "stop", new_stop);
        set_string_preserving(table, "on", on);
        if keeps_label {
            set_string_preserving(table, "rename", console_name);
        } else {
            table.remove("rename");
        }
    }
    remove_moves_for(&mut doc, console_name);
    write_atomically(path, doc.to_string())?;
    Ok(true)
}

/// Remove a stop from a composite file: delete its own `[[stop]]`
/// line, or except it from the `[[division]]` pull that brought it in
/// — plus every line that was about it (`[[move]]`, its own voicing
/// rule, exact `[[enclosure]]` memberships). `Ok(false)` when no pull
/// matches.
pub fn remove_composite_stop(
    path: &Path,
    prov: &instrument::StopProvenance,
    console_name: &str,
    on: &str,
) -> Result<bool, String> {
    let mut doc = composite_doc(path)?;
    if prov.via_division {
        let Some(index) = division_pull_index(&doc, prov, on) else {
            return Ok(false);
        };
        let table = doc["division"]
            .as_array_of_tables_mut()
            .and_then(|tables| tables.get_mut(index))
            .expect("division line just found");
        division_except_add(table, &prov.source_stop);
        division_rename_set(table, &prov.source_stop, None);
        division_map_set(table, "pitch_label", &prov.source_stop, None);
    } else {
        let Some(stops) = doc.get_mut("stop").and_then(|s| s.as_array_of_tables_mut()) else {
            return Ok(false);
        };
        let Some(doomed) = stop_pull_index(stops, prov, on) else {
            return Ok(false);
        };
        let mut index = 0;
        stops.retain(|_| {
            let keep = index != doomed;
            index += 1;
            keep
        });
        if stops.is_empty() {
            doc.remove("stop");
        }
    }
    remove_moves_for(&mut doc, console_name);
    if let Some(adjusts) = voicing_adjusts_mut(&mut doc) {
        adjusts.retain(|table| {
            !table
                .get("stops")
                .and_then(|item| item.as_array())
                .is_some_and(|array| {
                    array.len() == 1
                        && array
                            .iter()
                            .next()
                            .and_then(|value| value.as_str())
                            .is_some_and(|name| name.eq_ignore_ascii_case(console_name))
                })
        });
    }
    for table in tables_mut(&mut doc, "enclosure") {
        if let Some(array) = table.get_mut("stops").and_then(|item| item.as_array_mut()) {
            array.retain(|value| {
                !value.as_str().is_some_and(|s| s.eq_ignore_ascii_case(console_name))
            });
        }
    }
    if let Some(order) = console_order_mut(&mut doc) {
        for (_, item) in order.iter_mut() {
            if let Some(array) = item.as_array_mut() {
                array.retain(|value| {
                    !value.as_str().is_some_and(|s| s.eq_ignore_ascii_case(console_name))
                });
            }
        }
    }
    write_atomically(path, doc.to_string())?;
    Ok(true)
}

// ---- per-coupler edits, addressed by the file itself ----------------
//
// Couplers need no recorded provenance: the file says which are its
// own. A name that matches a `[[couplers.define]]` is this organ's
// coupler, edited in place. Anything else came in with a source —
// its console name lives in the `[couplers.rename]` map, and editing
// its routes MATERIALIZES it: a define with the same routes under the
// console name, the original dropped (hidden, still restorable) —
// the coupler twin of excepting a stop out of a division pull.

/// One coupler route as an edit sends it — manual names (the file's
/// vocabulary), fields as `[[couplers.define.route]]` spells them.
#[derive(Debug, Clone)]
pub struct CouplerRouteLine {
    pub from: String,
    pub to: Option<String>,
    pub shift: i16,
    pub low: Option<u8>,
    pub high: Option<u8>,
    pub unison_off: bool,
    pub scope: aristide_model::CouplerScope,
    pub repitch: Option<bool>,
    pub own_pipes: bool,
}

/// Defaults are expressed by absence, so a classic one-route coupler
/// stays the three lines a hand would write.
fn coupler_route_table(route: &CouplerRouteLine) -> toml_edit::Table {
    let mut table = toml_edit::Table::new();
    table["from"] = toml_edit::value(route.from.as_str());
    if let Some(to) = &route.to {
        table["to"] = toml_edit::value(to.as_str());
    }
    if route.shift != 0 {
        table["shift"] = toml_edit::value(route.shift as i64);
    }
    if let Some(low) = route.low {
        table["low"] = toml_edit::value(low as i64);
    }
    if let Some(high) = route.high {
        table["high"] = toml_edit::value(high as i64);
    }
    if route.unison_off {
        table["unison_off"] = toml_edit::value(true);
    }
    if route.scope != aristide_model::CouplerScope::AllKeys {
        table["scope"] = toml_edit::value(match route.scope {
            aristide_model::CouplerScope::Bass => "bass",
            aristide_model::CouplerScope::Melody => "melody",
            aristide_model::CouplerScope::AllKeys => unreachable!(),
        });
    }
    if let Some(repitch) = route.repitch {
        table["repitch"] = toml_edit::value(repitch);
    }
    if route.own_pipes {
        table["own_pipes"] = toml_edit::value(true);
    }
    table
}

/// The `[couplers]` table, created implicit so its arrays and maps
/// render dotted (`[[couplers.define]]`, `[couplers.rename]`).
fn couplers_table(doc: &mut toml_edit::DocumentMut) -> Result<&mut toml_edit::Table, String> {
    let item = doc
        .entry("couplers")
        .or_insert(toml_edit::Item::Table(toml_edit::Table::new()));
    let Some(table) = item.as_table_mut() else {
        return Err("[couplers] is not a table".into());
    };
    table.set_implicit(true);
    Ok(table)
}

fn coupler_define_index(doc: &toml_edit::DocumentMut, name: &str) -> Option<usize> {
    let defines = doc.get("couplers")?.get("define")?.as_array_of_tables()?;
    (0..defines.len()).find(|&i| defines.get(i).is_some_and(|table| field_is(table, "name", name)))
}

fn coupler_defines_mut(
    doc: &mut toml_edit::DocumentMut,
) -> Result<&mut toml_edit::ArrayOfTables, String> {
    let couplers = couplers_table(doc)?;
    let defines = couplers
        .entry("define")
        .or_insert(toml_edit::Item::ArrayOfTables(toml_edit::ArrayOfTables::new()));
    defines
        .as_array_of_tables_mut()
        .ok_or_else(|| "[[couplers.define]] is not an array of tables".into())
}

/// Rename a coupler: a define's own `name` line, or — for one a source
/// carries in — an entry in the `[couplers.rename]` map (keyed by the
/// original name, so the map survives however often the label moves).
/// `[couplers] drop` entries naming it exactly follow.
pub fn rename_composite_coupler(path: &Path, old: &str, new: &str) -> Result<(), String> {
    let new = new.trim();
    if new.is_empty() {
        return Err("the coupler needs a name".into());
    }
    let mut doc = composite_doc(path)?;
    if let Some(index) = coupler_define_index(&doc, old) {
        let table = coupler_defines_mut(&mut doc)?
            .get_mut(index)
            .expect("define just found");
        set_string_preserving(table, "name", new);
    } else {
        let couplers = couplers_table(&mut doc)?;
        let item = couplers
            .entry("rename")
            .or_insert(toml_edit::Item::Table(toml_edit::Table::new()));
        let Some(map) = item.as_table_like_mut() else {
            return Err("[couplers.rename] is not a table".into());
        };
        let original = map
            .iter()
            .find(|(_, value)| value.as_str().is_some_and(|v| v.eq_ignore_ascii_case(old)))
            .map(|(key, _)| key.to_string())
            .unwrap_or_else(|| old.to_string());
        if original.eq_ignore_ascii_case(new) {
            map.remove(&original);
        } else {
            map.insert(&original, toml_edit::value(new));
        }
        if map.is_empty() {
            couplers.remove("rename");
        }
    }
    if let Some(drops) = doc
        .get_mut("couplers")
        .and_then(|couplers| couplers.get_mut("drop"))
        .and_then(|item| item.as_array_mut())
    {
        for value in drops.iter_mut() {
            if value.as_str().is_some_and(|v| v.eq_ignore_ascii_case(old)) {
                let decor = value.decor().clone();
                *value = new.into();
                *value.decor_mut() = decor;
            }
        }
    }
    // Every other name-keyed reference follows too: link groups, the
    // coupled-keys override, and any jamb seat in [console.order].
    let mut groups = coupler_link_groups(&doc);
    let mut linked = false;
    for group in &mut groups {
        for name in group {
            if name.eq_ignore_ascii_case(old) {
                *name = new.to_string();
                linked = true;
            }
        }
    }
    if linked {
        write_coupler_link_groups(&mut doc, groups)?;
    }
    if let Some(map) = console_map_mut(&mut doc, "coupler_keys") {
        let stale: Vec<String> = map
            .iter()
            .filter(|(key, _)| key.eq_ignore_ascii_case(old))
            .map(|(key, _)| key.to_string())
            .collect();
        for key in stale {
            if let Some(value) = map.remove(&key) {
                map.insert(new, value);
            }
        }
    }
    if let Some(order) = console_order_mut(&mut doc) {
        let token = format!("coupler:{old}");
        for (_, item) in order.iter_mut() {
            if let Some(list) = item.as_array_mut() {
                for value in list.iter_mut() {
                    if value.as_str().is_some_and(|v| v.eq_ignore_ascii_case(&token)) {
                        let decor = value.decor().clone();
                        *value = format!("coupler:{new}").into();
                        *value.decor_mut() = decor;
                    }
                }
            }
        }
    }
    write_atomically(path, doc.to_string())
}

/// Replace a coupler's routes. A `[[couplers.define]]` under this name
/// is rewritten in place; anything else is a source's coupler, which
/// materializes — same routes (as edited) under the console name, the
/// original dropped off the console and its rename entry retired.
pub fn write_composite_coupler_routes(
    path: &Path,
    console_name: &str,
    routes: &[CouplerRouteLine],
) -> Result<(), String> {
    if routes.is_empty() {
        return Err("a coupler needs at least one route".into());
    }
    let mut doc = composite_doc(path)?;
    let define = coupler_define_index(&doc, console_name);
    if define.is_none() {
        let couplers = couplers_table(&mut doc)?;
        let original = couplers
            .get_mut("rename")
            .and_then(|item| item.as_table_like_mut())
            .and_then(|map| {
                let original = map
                    .iter()
                    .find(|(_, value)| {
                        value.as_str().is_some_and(|v| v.eq_ignore_ascii_case(console_name))
                    })
                    .map(|(key, _)| key.to_string());
                if let Some(key) = &original {
                    map.remove(key);
                }
                original
            });
        // A never-renamed original still bears the console name — a
        // drop entry under that name would hide the new define along
        // with it, so the original is renamed out of the way first,
        // legibly ("… (set)" is what the prefs list will show it as).
        let dropped = match original {
            Some(original) if !original.eq_ignore_ascii_case(console_name) => original,
            _ => {
                let dropped = format!("{console_name} (set)");
                let item = couplers
                    .entry("rename")
                    .or_insert(toml_edit::Item::Table(toml_edit::Table::new()));
                let Some(map) = item.as_table_like_mut() else {
                    return Err("[couplers.rename] is not a table".into());
                };
                map.insert(console_name, toml_edit::value(dropped.as_str()));
                dropped
            }
        };
        if couplers
            .get("rename")
            .and_then(|item| item.as_table_like())
            .is_some_and(|map| map.is_empty())
        {
            couplers.remove("rename");
        }
        let drops = couplers
            .entry("drop")
            .or_insert(toml_edit::Item::Value(toml_edit::Array::new().into()));
        if let Some(array) = drops.as_value_mut().and_then(|value| value.as_array_mut())
            && !array
                .iter()
                .any(|value| value.as_str().is_some_and(|v| v.eq_ignore_ascii_case(&dropped)))
        {
            array.push(dropped.as_str());
        }
    }
    let defines = coupler_defines_mut(&mut doc)?;
    let mut table = toml_edit::Table::new();
    table["name"] = toml_edit::value(console_name);
    let mut route_tables = toml_edit::ArrayOfTables::new();
    for route in routes {
        route_tables.push(coupler_route_table(route));
    }
    table["route"] = toml_edit::Item::ArrayOfTables(route_tables);
    match define {
        Some(index) => *defines.get_mut(index).expect("define just found") = table,
        None => defines.push(table),
    }
    write_atomically(path, doc.to_string())
}

/// Define a brand-new coupler. Refuses a name a define already holds
/// — the caller checks the console for clashes with carried couplers.
pub fn append_composite_coupler(
    path: &Path,
    name: &str,
    routes: &[CouplerRouteLine],
) -> Result<(), String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("the coupler needs a name".into());
    }
    if routes.is_empty() {
        return Err("a coupler needs at least one route".into());
    }
    let mut doc = composite_doc(path)?;
    if coupler_define_index(&doc, name).is_some() {
        return Err(format!("this organ already defines a coupler named {name:?}"));
    }
    let defines = coupler_defines_mut(&mut doc)?;
    let mut table = toml_edit::Table::new();
    table["name"] = toml_edit::value(name);
    let mut route_tables = toml_edit::ArrayOfTables::new();
    for route in routes {
        route_tables.push(coupler_route_table(route));
    }
    table["route"] = toml_edit::Item::ArrayOfTables(route_tables);
    defines.push(table);
    write_atomically(path, doc.to_string())
}

/// Delete a coupler this file defines: its `[[couplers.define]]` table
/// goes, and every name-keyed reference — link groups, drop entries,
/// a `[console.coupler_keys]` override, its `coupler:` seats in
/// `[console.order]` — goes with it, so nothing dangles. Returns false
/// for a coupler the file doesn't define (a source's — the caller
/// drops it off the console instead, which is as deleted as a set's
/// own coupler can get).
pub fn remove_composite_coupler(path: &Path, name: &str) -> Result<bool, String> {
    let mut doc = composite_doc(path)?;
    let Some(index) = coupler_define_index(&doc, name) else {
        return Ok(false);
    };
    coupler_defines_mut(&mut doc)?.remove(index);
    if let Some(couplers) = doc.get_mut("couplers").and_then(|item| item.as_table_mut()) {
        if couplers
            .get("define")
            .and_then(|item| item.as_array_of_tables())
            .is_some_and(|defines| defines.is_empty())
        {
            couplers.remove("define");
        }
        if let Some(drops) = couplers.get_mut("drop").and_then(|item| item.as_array_mut()) {
            drops.retain(|value| !value.as_str().is_some_and(|v| v.eq_ignore_ascii_case(name)));
            if drops.is_empty() {
                couplers.remove("drop");
            }
        }
    }
    unlink_everywhere(&mut doc, name);
    if let Some(map) = console_map_mut(&mut doc, "coupler_keys") {
        let stale: Vec<String> = map
            .iter()
            .filter(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(key, _)| key.to_string())
            .collect();
        for key in stale {
            map.remove(&key);
        }
    }
    remove_console_order_coupler(&mut doc, name);
    write_atomically(path, doc.to_string())?;
    Ok(true)
}

/// The `[couplers] link` groups as the file holds them.
fn coupler_link_groups(doc: &toml_edit::DocumentMut) -> Vec<Vec<String>> {
    doc.get("couplers")
        .and_then(|couplers| couplers.get("link"))
        .and_then(|item| item.as_array())
        .map(|groups| {
            groups
                .iter()
                .filter_map(|group| group.as_array())
                .map(|group| {
                    group
                        .iter()
                        .filter_map(|value| value.as_str().map(str::to_string))
                        .collect()
                })
                .collect()
        })
        .unwrap_or_default()
}

fn write_coupler_link_groups(
    doc: &mut toml_edit::DocumentMut,
    groups: Vec<Vec<String>>,
) -> Result<(), String> {
    let couplers = couplers_table(doc)?;
    if groups.is_empty() {
        couplers.remove("link");
        return Ok(());
    }
    let mut list = toml_edit::Array::new();
    for group in groups {
        let mut inner = toml_edit::Array::new();
        for name in group {
            inner.push(name.as_str());
        }
        list.push(inner);
    }
    couplers["link"] = toml_edit::value(list);
    Ok(())
}

/// Link or unlink two couplers in `[couplers] link`. Linking merges
/// any groups either name belongs to; unlinking takes `b` out of its
/// group, and a group left with one member dissolves.
pub fn write_composite_coupler_link(
    path: &Path,
    a: &str,
    b: &str,
    on: bool,
) -> Result<(), String> {
    let mut doc = composite_doc(path)?;
    let mut groups = coupler_link_groups(&doc);
    if on {
        let mut merged: Vec<String> = Vec::new();
        groups.retain(|group| {
            let joins = group
                .iter()
                .any(|n| n.eq_ignore_ascii_case(a) || n.eq_ignore_ascii_case(b));
            if joins {
                merged.extend(group.iter().cloned());
            }
            !joins
        });
        for name in [a, b] {
            if !merged.iter().any(|n| n.eq_ignore_ascii_case(name)) {
                merged.push(name.to_string());
            }
        }
        groups.push(merged);
    } else {
        for group in &mut groups {
            if group.iter().any(|n| n.eq_ignore_ascii_case(b))
                && group.iter().any(|n| n.eq_ignore_ascii_case(a))
            {
                group.retain(|n| !n.eq_ignore_ascii_case(b));
            }
        }
        groups.retain(|group| group.len() > 1);
    }
    write_coupler_link_groups(&mut doc, groups)?;
    write_atomically(path, doc.to_string())
}

/// Take one name out of every `[couplers] link` group — deletion's
/// housekeeping (a rename edits in place instead).
fn unlink_everywhere(doc: &mut toml_edit::DocumentMut, name: &str) {
    let mut groups = coupler_link_groups(doc);
    for group in &mut groups {
        group.retain(|n| !n.eq_ignore_ascii_case(name));
    }
    groups.retain(|group| group.len() > 1);
    let _ = write_coupler_link_groups(doc, groups);
}

/// A map directly under `[console]` (`coupler_keys`), if present.
fn console_map_mut<'a>(
    doc: &'a mut toml_edit::DocumentMut,
    key: &str,
) -> Option<&'a mut dyn toml_edit::TableLike> {
    doc.get_mut("console")
        .and_then(|console| console.get_mut(key))
        .and_then(|item| item.as_table_like_mut())
}

/// Drop a coupler's `coupler:<name>` seat from every `[console.order]`
/// list — the jamb spot of a coupler that no longer exists.
fn remove_console_order_coupler(doc: &mut toml_edit::DocumentMut, name: &str) {
    let token = format!("coupler:{name}");
    let Some(order) = console_order_mut(doc) else { return };
    for (_, item) in order.iter_mut() {
        if let Some(list) = item.as_array_mut() {
            list.retain(|value| !value.as_str().is_some_and(|v| v.eq_ignore_ascii_case(&token)));
        }
    }
}

/// The organ-wide coupled-keys default: `[console] coupled_keys`.
/// True is the default, so true removes the line.
pub fn write_composite_coupled_keys(path: &Path, on: bool) -> Result<(), String> {
    let mut doc = composite_doc(path)?;
    let console = doc
        .entry("console")
        .or_insert(toml_edit::Item::Table(toml_edit::Table::new()));
    let Some(console) = console.as_table_mut() else {
        return Err("[console] is not a table".into());
    };
    console.set_implicit(true);
    if on {
        console.remove("coupled_keys");
    } else {
        console["coupled_keys"] = toml_edit::value(false);
    }
    write_atomically(path, doc.to_string())
}

/// One coupler's coupled-keys override in `[console.coupler_keys]`:
/// `"never"`, `"always"`, or None for auto (entry removed).
pub fn write_composite_coupler_key_mode(
    path: &Path,
    name: &str,
    mode: Option<&str>,
) -> Result<(), String> {
    let mut doc = composite_doc(path)?;
    match mode {
        Some(mode) => {
            let map = console_section(&mut doc, "coupler_keys")?;
            map.insert(name, toml_edit::value(mode));
        }
        None => {
            if let Some(map) = console_map_mut(&mut doc, "coupler_keys") {
                let stale: Vec<String> = map
                    .iter()
                    .filter(|(key, _)| key.eq_ignore_ascii_case(name))
                    .map(|(key, _)| key.to_string())
                    .collect();
                for key in stale {
                    map.remove(&key);
                }
                if map.is_empty()
                    && let Some(console) = doc.get_mut("console").and_then(|c| c.as_table_mut())
                {
                    console.remove("coupler_keys");
                }
            }
        }
    }
    write_atomically(path, doc.to_string())
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

fn console_order_mut(doc: &mut toml_edit::DocumentMut) -> Option<&mut toml_edit::Table> {
    doc.get_mut("console")
        .and_then(|console| console.get_mut("order"))
        .and_then(|order| order.as_table_mut())
}

/// Upsert one console panel's canvas position: creates `[console.layout]`
/// if the file doesn't have it yet, and writes (or replaces) the
/// panel's quoted key inside it — `"keyboard:Great" = { x = .., y = .. }`.
/// Purely cosmetic: unlike the structural editors above, nothing calls
/// this expects a reload — the caller updates the in-memory snapshot
/// itself.
pub fn write_composite_panel(
    path: &Path,
    panel: &str,
    pos: instrument::PanelPos,
) -> Result<(), String> {
    let mut doc = composite_doc(path)?;
    let layout = console_section(&mut doc, "layout")?;
    let mut entry = toml_edit::InlineTable::new();
    entry.insert("x", (pos.x as f64).into());
    entry.insert("y", (pos.y as f64).into());
    if let Some(w) = pos.w {
        entry.insert("w", (w as f64).into());
    }
    if let Some(h) = pos.h {
        entry.insert("h", (h as f64).into());
    }
    layout.insert(panel, toml_edit::Item::Value(toml_edit::Value::InlineTable(entry)));
    write_atomically(path, doc.to_string())
}

/// One table under `[console]` (`layout`, `order`), the `[console]`
/// header itself kept implicit so it never crowds a future key.
fn console_section<'a>(
    doc: &'a mut toml_edit::DocumentMut,
    key: &str,
) -> Result<&'a mut toml_edit::Table, String> {
    let console = doc
        .entry("console")
        .or_insert(toml_edit::Item::Table(toml_edit::Table::new()));
    let Some(console) = console.as_table_mut() else {
        return Err("[console] is not a table".into());
    };
    console.set_implicit(true);
    let section = console
        .entry(key)
        .or_insert(toml_edit::Item::Table(toml_edit::Table::new()));
    section
        .as_table_mut()
        .ok_or_else(|| format!("[console.{key}] is not a table"))
}

/// A division's drawknob order — the console names, top of the jamb
/// first. An empty list takes the entry (and an emptied section) out,
/// back to the assembled order.
pub fn write_composite_stop_order(
    path: &Path,
    manual: &str,
    stops: &[String],
) -> Result<(), String> {
    let mut doc = composite_doc(path)?;
    let order = console_section(&mut doc, "order")?;
    if stops.is_empty() {
        order.remove(manual);
        if order.is_empty()
            && let Some(console) = doc.get_mut("console").and_then(|c| c.as_table_mut())
        {
            console.remove("order");
        }
    } else {
        let mut list = toml_edit::Array::new();
        for stop in stops {
            list.push(stop.as_str());
        }
        order.insert(manual, toml_edit::value(list));
    }
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

/// Write via a temporary file and rename, so a crash mid-write cannot
/// leave a half-written file behind.
fn write_atomically(path: &Path, body: String) -> Result<(), String> {
    let temporary = path.with_extension("toml.tmp");
    std::fs::write(&temporary, body)
        .map_err(|err| format!("{}: {err}", temporary.display()))?;
    std::fs::rename(&temporary, path).map_err(|err| format!("{}: {err}", path.display()))
}

#[cfg(test)]
mod tests {
    #[test]
    fn sample_prefs_round_trip_and_tolerate_hand_edits() {
        use super::*;
        let mut config = MidiConfig::default();
        assert_eq!(config.samples, SamplePrefs::default());
        config.samples.streaming = Streaming::On;
        config.samples.ram_budget_mb = Some(4096);
        config.samples.bits = 32;
        config.samples.cache = false;
        config.remember("Demo", Path::new("/tmp/demo.organ"));
        let text = toml::to_string_pretty(&config).expect("serializes");
        assert!(text.contains("[samples]"), "{text}");
        assert!(text.contains("streaming = \"on\""), "{text}");
        let back: MidiConfig = toml::from_str(&text).expect("parses");
        assert_eq!(back.samples, config.samples);
        assert_eq!(back.library.len(), 1);

        // A file from before [samples] existed reads as the defaults.
        let old: MidiConfig = toml::from_str("[[library]]\nname = \"x\"\npath = \"/x\"\n")
            .expect("parses");
        assert_eq!(old.samples, SamplePrefs::default());
        // A typo in a hand edit must not cost the player their wiring:
        // an unknown mode reads as auto and an odd bit depth as 16.
        let typo: MidiConfig =
            toml::from_str("[samples]\nstreaming = \"sometimes\"\nbits = 24\n").expect("parses");
        assert_eq!(typo.samples.streaming, Streaming::Auto);
        assert_eq!(typo.samples.resident_bits(), 16);
        assert_eq!(Streaming::parse("OFF"), Some(Streaming::Off));
        assert_eq!(Streaming::parse("true"), Some(Streaming::On));
        assert_eq!(Streaming::parse("maybe"), None);
    }

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
        assert_eq!(
            organ_config_from_file(&definition.midi, &definition.combinations),
            organ
        );
        // Wiring emptied: the arrays vanish rather than lingering as [].
        write_composite_midi(&path, None).expect("writes back empty");
        let text = std::fs::read_to_string(&path).expect("reads");
        assert!(!text.contains("midi"));
        assert!(text.contains("# my precious hand-written organ"));
        let _ = std::fs::remove_file(&path);
    }

    /// The combination action lands in the organ's own file — the same
    /// home as the wiring, because it is the same kind of fact: how
    /// this player uses this instrument. It reads back exactly, and a
    /// hand-written `divisional_*` flag survives a piston press (they
    /// are never ours to rewrite).
    #[test]
    fn combinations_round_trip_through_the_organ_file() {
        let path = std::env::temp_dir().join("aristide-combinations-test.toml");
        std::fs::write(
            &path,
            "# hand-written\nname = \"Franken\"\n\n\
             [combinations]\ndivisional_tremulants = true # my console does\n",
        )
        .expect("fixture writes");
        let mut organ = OrganConfig::default();
        organ.generals.insert(
            1,
            Registration {
                stops: vec!["Montre 8'".into(), "Prestant 4'".into()],
                couplers: vec!["II/I".into()],
                tremulants: Vec::new(),
            },
        );
        organ.divisionals.entry("Récit".into()).or_default().insert(
            3,
            Registration {
                stops: vec!["Hautbois 8'".into()],
                ..Default::default()
            },
        );
        organ.frames = vec![
            Registration { stops: vec!["Bourdon 8'".into()], ..Default::default() },
            Registration { stops: vec!["Trompette 8'".into()], ..Default::default() },
        ];
        organ.crescendo.insert(1, vec!["Bourdon 8'".into()]);
        organ.crescendo.insert(2, vec!["Montre 8'".into()]);

        write_composite_combinations(&path, Some(&organ)).expect("writes back");
        let text = std::fs::read_to_string(&path).expect("reads");
        assert!(text.contains("# hand-written"));
        assert!(
            text.contains("divisional_tremulants = true"),
            "a hand-written console flag is not ours to erase"
        );
        let definition: aristide_formats::instrument::Definition =
            toml::from_str(&text).expect("still a valid organ file");
        assert_eq!(
            definition.combinations.divisional_tremulants,
            Some(true),
            "the flag reads back as the file's own override"
        );
        assert_eq!(
            organ_config_from_file(&definition.midi, &definition.combinations),
            organ,
            "every general, divisional, frame and crescendo stage returns"
        );
        // Emptied: the tables vanish rather than lingering as [].
        write_composite_combinations(&path, None).expect("writes back empty");
        let text = std::fs::read_to_string(&path).expect("reads");
        assert!(!text.contains("[[combinations"), "no empty arrays left behind");
        assert!(text.contains("divisional_tremulants = true"));
        let _ = std::fs::remove_file(&path);
    }

    /// Frames are ordered by their own `n`, not by where their tables
    /// happen to sit — a hand-renumbered sequence reads back as
    /// written, and gaps close because the stepper walks positions.
    #[test]
    fn stepper_frames_read_back_in_their_numbered_order() {
        let definition: aristide_formats::instrument::Definition = toml::from_str(
            "name = \"F\"\n\
             [[combinations.frame]]\nn = 7\nstops = [\"C\"]\n\
             [[combinations.frame]]\nn = 2\nstops = [\"A\"]\n\
             [[combinations.frame]]\nn = 5\nstops = [\"B\"]\n",
        )
        .expect("parses");
        let organ = organ_config_from_file(&definition.midi, &definition.combinations);
        assert_eq!(
            organ.frames.iter().map(|f| f.stops[0].as_str()).collect::<Vec<_>>(),
            ["A", "B", "C"]
        );
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
                    own_pipes: false,
                },
                Stop {
                    id: StopId(1),
                    name: "Montre 8".into(),
                    manual: ManualId(1),
                    ranks: Vec::new(),
                    own_pipes: false,
                },
            ],
            couplers: vec![Coupler::simple("Great to Pedal", ManualId(1), ManualId(0), 0)],
            ..Default::default()
        }
    }

    /// An adopted wrapper is marked as the set's own organ, and the
    /// copy made to edit it is the same file under another name with
    /// the mark dropped — the original untouched. The copy still wraps
    /// the set, but only the marked original stands in for the set
    /// when the raw set is loaded again.
    #[test]
    fn a_copy_under_another_name_drops_the_adopted_mark() {
        let dir = std::env::temp_dir().join("aristide-copy-as-test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("fixture dir");
        let set = dir.join("village.organ");
        std::fs::write(&set, "[Organ]").expect("fixture set");
        let organs = dir.join("organs");
        let original = create_wrapper_organ(&organs, "Village", &set, &test_organ(), None)
            .expect("wrapper created");
        let before = std::fs::read_to_string(&original).expect("reads");
        assert!(before.contains("adopted = true"), "adoption marks the set's own organ");

        assert!(
            copy_composite_as(&original, &organs, "Village").is_err(),
            "the copy needs a name of its own"
        );
        let copy = copy_composite_as(&original, &organs, "My Village").expect("copied");
        assert_ne!(copy, original);
        assert_eq!(std::fs::read_to_string(&original).expect("reads"), before, "untouched");
        let text = std::fs::read_to_string(&copy).expect("reads");
        assert!(!text.contains("adopted"), "the copy takes edits: {text}");
        let def: aristide_formats::instrument::Definition =
            toml::from_str(&text).expect("a valid organ file");
        assert_eq!(def.name, "My Village");
        assert!(!def.adopted);
        assert_eq!(def.stops.len(), 2, "the inventory came along");

        let mut config = MidiConfig::default();
        config.remember("My Village", &copy);
        config.remember("Village", &original);
        let canonical = set.canonicalize().expect("canonicalizes");
        assert_eq!(
            config.wrapper_for(&canonical, Some("My Village"), None),
            Some(copy.clone()),
            "the copy is reached by its name"
        );
        config.library.retain(|entry| entry.path != original);
        assert_eq!(
            config.wrapper_for(&canonical, None, Some(&organs)),
            Some(original.clone()),
            "the raw set means the marked original, not the copy"
        );
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
    /// (or when nothing carries that name) the set's own organ — the
    /// marked original — wins, never silently a different organ than
    /// the click said, and never a copy saved from it.
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
            Some(adopted.clone()),
            "no name: the set's own organ wins over recency"
        );
        assert_eq!(
            config.wrapper_for(&canonical, Some("Renamed Since"), None),
            Some(adopted.clone()),
            "a stale name still resolves rather than duplicating the organ"
        );
        config.forget(&adopted);
        assert_eq!(
            config.wrapper_for(&canonical, None, None),
            Some(built),
            "with no marked original in reach, the older signs still count"
        );
        config.remember("Chapelle", &adopted);

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

        // The tremulant shape writes into [tremulant] in the sidecar's
        // vocabulary, creating the table when absent and preserving an
        // existing wind plan (chests).
        std::fs::write(
            &path,
            std::fs::read_to_string(&path).expect("reads") + "\n[tremulant]\nchests = [1, 2]\n",
        )
        .expect("appends");
        write_composite_tremulant(&path, 3.5, 15.0, 1.2, 6.0).expect("tremulant");
        let text = std::fs::read_to_string(&path).expect("reads");
        assert!(text.contains("rate_hz = 3.5"), "{text}");
        assert!(text.contains("depth_cents = 15.0"), "{text}");
        assert!(text.contains("ramp_s = 1.2"), "{text}");
        assert!(text.contains("wobble_pct = 6.0"), "{text}");
        assert!(text.contains("chests = [1, 2]"), "the wind plan survives: {text}");

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
            edo: 12,
            reference: crate::tuning::PitchReference { key: 69, hz: 432.0 },
            transpose: 0,
            scale: scale.map(str::to_string),
            keymap: None,
            pipes: crate::tuning::PipeRetune::Original,
        };
        assert!(
            write_composite_manual_tuning(&path, "Grand orgue", Some(tuned("equal", Some("19edo.scl"))))
                .expect("tuning")
        );
        let parsed = def(&path);
        assert_eq!(parsed.manuals[0].scale.as_deref(), Some("19edo.scl"));
        assert_eq!(parsed.manuals[0].temperament, None, "a scale IS the temperament");
        assert_eq!(parsed.manuals[0].reference_hz, Some(432.0));
        assert_eq!(
            parsed.manuals[0].reference_key,
            Some(aristide_formats::sidecar::KeySpec::Name("A4".into()))
        );
        assert!(
            write_composite_manual_tuning(&path, "Grand orgue", Some(tuned("meantone4", None)))
                .expect("tuning")
        );
        let parsed = def(&path);
        assert_eq!(parsed.manuals[0].scale, None);
        assert_eq!(parsed.manuals[0].temperament.as_deref(), Some("meantone4"));

        // A division count away from 12 writes an edo line and drops
        // the temperament (twelve-class vocabulary); back at 12 the
        // temperament line returns and the edo line goes.
        assert!(
            write_composite_manual_tuning(
                &path,
                "Grand orgue",
                Some(ManualTuningFields { edo: 31, ..tuned("meantone4", None) })
            )
            .expect("tuning")
        );
        let parsed = def(&path);
        assert_eq!(parsed.manuals[0].edo, Some(31));
        assert_eq!(parsed.manuals[0].temperament, None, "temperaments are 12-EDO talk");
        assert!(
            write_composite_manual_tuning(&path, "Grand orgue", Some(tuned("meantone4", None)))
                .expect("tuning")
        );
        let parsed = def(&path);
        assert_eq!(parsed.manuals[0].edo, None, "12 is absence");
        assert_eq!(parsed.manuals[0].temperament.as_deref(), Some("meantone4"));

        assert!(write_composite_manual_tuning(&path, "Grand orgue", None).expect("tuning"));
        assert_eq!(def(&path).manuals[0].reference_hz, None);
        assert_eq!(def(&path).manuals[0].reference_key, None);

        // A declared drawknob order rides [console.order], keyed by
        // manual name — so a manual rename must carry the key.
        write_composite_stop_order(
            &path,
            "Grand orgue",
            &["Montre 8".to_string(), "Bourdon 16".to_string()],
        )
        .expect("orders");
        assert_eq!(
            def(&path).console.order.get("Grand orgue").map(Vec::len),
            Some(2)
        );

        // Rename follows the name everywhere the file says it.
        assert!(rename_composite_manual(&path, "Grand orgue", "Hauptwerk").expect("renames"));
        assert!(!rename_composite_manual(&path, "Ghost", "X").expect("no ghost"));
        let parsed = def(&path);
        assert_eq!(parsed.manuals[0].name, "Hauptwerk");
        assert_eq!(parsed.stops[0].on, "Hauptwerk");
        assert!(parsed.console.order.contains_key("Hauptwerk"), "the order key moved");
        assert!(!parsed.console.order.contains_key("Grand orgue"));
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
        let montre = instrument::StopProvenance {
            source: "s1".into(),
            source_manual: "Great".into(),
            source_stop: "Montre 8".into(),
            via_division: false,
        };
        assert!(
            remove_composite_stop(&path, &montre, "Montre 8", "Hauptwerk").expect("unpulls")
        );
        let parsed = def(&path);
        assert!(parsed.stops.is_empty());
        assert!(parsed.moves.is_empty());
        assert!(remove_composite_manual(&path, "Pédale").expect("removes"));
        let parsed = def(&path);
        assert_eq!(parsed.manuals.len(), 1);
        assert!(parsed.divisions.is_empty(), "the pull landing on it went too");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The per-stop editors, addressed by provenance: a rename lands
    /// on the pull that brought the stop in (a `[[stop]]` line's
    /// `rename` field, a `[[division]]` line's rename map) and every
    /// exact name reference follows; a voicing rule is one exact-name
    /// `[[voicing.adjust]]` entry that neutral values remove again;
    /// retargeting rewrites the pull (excepting a division stop into a
    /// `[[stop]]` line of its own) while the label stays.
    #[test]
    fn per_stop_edits_rewrite_the_pulls() {
        let dir = std::env::temp_dir().join("aristide-per-stop-edits-test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("test dir");
        let path = dir.join("orgue.toml");
        std::fs::write(
            &path,
            r#"name = "Edits"

[sources]
anne = "/sets/anne.organ"
gib = "/sets/gib.organ"

[[manual]]
name = "Great"
low = 36
high = 96

[[division]]
from = "anne"
manual = "Hauptwerk"
on = "Great"

[[stop]]
from = "gib"
manual = "Récit"
stop = "Trompette 8"
on = "Great"

[[move]]
stop = "Montre 8"
from = "Great"
to = "Great"

[[enclosure]]
name = "Box"
stops = ["Montre 8"]
"#,
        )
        .expect("fixture");
        let def = |path: &Path| -> aristide_formats::instrument::Definition {
            toml::from_str(&std::fs::read_to_string(path).expect("reads")).expect("parses")
        };
        let division_stop = instrument::StopProvenance {
            source: "anne".into(),
            source_manual: "Hauptwerk".into(),
            source_stop: "Montre 8".into(),
            via_division: true,
        };
        let pulled_stop = instrument::StopProvenance {
            source: "gib".into(),
            source_manual: "Récit".into(),
            source_stop: "Trompette 8".into(),
            via_division: false,
        };

        // Rename a division-pulled stop: the map entry appears and the
        // move/enclosure references follow.
        assert!(
            rename_composite_stop(&path, &division_stop, "Great", "Montre 8", "Principal 8")
                .expect("renames")
        );
        let parsed = def(&path);
        assert_eq!(
            parsed.divisions[0].rename.get("Montre 8").map(String::as_str),
            Some("Principal 8")
        );
        assert_eq!(parsed.moves[0].stop, "Principal 8", "the move followed");
        assert_eq!(parsed.enclosure_defs[0].stops, ["Principal 8"], "the box followed");
        // Renaming back to the source name drops the map again.
        assert!(
            rename_composite_stop(&path, &division_stop, "Great", "Principal 8", "Montre 8")
                .expect("renames back")
        );
        assert!(def(&path).divisions[0].rename.is_empty());

        // Rename a [[stop]]-pulled stop: its own line carries it.
        assert!(
            rename_composite_stop(&path, &pulled_stop, "Great", "Trompette 8", "Tromba")
                .expect("renames")
        );
        assert_eq!(def(&path).stops[0].rename.as_deref(), Some("Tromba"));

        // A knob engraving rides the same lines: a field on a stop
        // pull, a map entry on a division pull — and the empty string
        // (engrave nothing) is a value, not an absence.
        assert!(
            write_composite_stop_pitch_label(&path, &pulled_stop, "Great", Some("8"))
                .expect("labels")
        );
        assert_eq!(def(&path).stops[0].pitch_label.as_deref(), Some("8"));
        assert!(
            write_composite_stop_pitch_label(&path, &division_stop, "Great", Some(""))
                .expect("labels")
        );
        assert_eq!(
            def(&path).divisions[0].pitch_label.get("Montre 8").map(String::as_str),
            Some("")
        );
        assert!(
            write_composite_stop_pitch_label(&path, &pulled_stop, "Great", None)
                .expect("unlabels")
        );
        assert!(def(&path).stops[0].pitch_label.is_none());

        // Pipe sharing rides the same lines, and shared (false) is
        // spelled by absence.
        assert!(
            write_composite_stop_own_pipes(&path, &pulled_stop, "Great", true)
                .expect("declares")
        );
        assert_eq!(def(&path).stops[0].own_pipes, Some(true));
        assert!(
            write_composite_stop_own_pipes(&path, &division_stop, "Great", true)
                .expect("declares")
        );
        assert_eq!(
            def(&path).divisions[0].own_pipes.get("Montre 8").copied(),
            Some(true)
        );
        assert!(
            write_composite_stop_own_pipes(&path, &pulled_stop, "Great", false)
                .expect("shares again")
        );
        assert!(def(&path).stops[0].own_pipes.is_none());
        assert!(
            !std::fs::read_to_string(&path)
                .expect("reads")
                .contains("own_pipes = false"),
            "the default is absence, not a false line"
        );

        // A voicing rule is created exact-name, updated in place, and
        // removed again when everything is neutral.
        let whole_stop = crate::load::VoicingScope::default();
        let voice = |scope: &crate::load::VoicingScope, feet, cents, gain, brightness| {
            write_composite_voicing(&path, "Tromba", scope, true, feet, cents, gain, brightness)
        };
        voice(&whole_stop, Some(16.0 / 3.0), None, Some(-2.0), None).expect("voices");
        let parsed = def(&path);
        assert_eq!(parsed.voicing.adjusts.len(), 1);
        assert_eq!(parsed.voicing.adjusts[0].stops, ["Tromba"]);
        assert_eq!(parsed.voicing.adjusts[0].pitch.as_ref().and_then(|p| p.feet()), Some(16.0 / 3.0));
        assert_eq!(parsed.voicing.adjusts[0].gain_db, Some(-2.0));
        voice(&whole_stop, None, Some(3.5), None, Some(-1.5)).expect("revoices");
        let parsed = def(&path);
        assert_eq!(parsed.voicing.adjusts.len(), 1, "updated, not duplicated");
        assert!(parsed.voicing.adjusts[0].pitch.is_none());
        assert_eq!(parsed.voicing.adjusts[0].cents, Some(3.5));
        assert_eq!(parsed.voicing.adjusts[0].brightness_db, Some(-1.5));

        // A narrowed rule is a rule of its own: it neither replaces the
        // stop's own nor is replaced by it, and its key span round-trips
        // through the spelling the writer chose.
        let bass = crate::load::VoicingScope {
            keys: Some((36, 47)),
            rank: None,
        };
        voice(&bass, None, None, Some(-4.0), None).expect("voices the bass octave");
        let parsed = def(&path);
        assert_eq!(parsed.voicing.adjusts.len(), 2, "a scope of its own");
        let bass_rule = parsed
            .voicing
            .adjusts
            .iter()
            .find(|rule| rule.keys.is_some())
            .expect("the narrowed rule");
        assert_eq!(bass_rule.keys.as_deref(), Some("C2..B2"));
        assert_eq!(bass_rule.gain_db, Some(-4.0));
        assert_eq!(bass_rule.cents, None, "unsaid stays unsaid");
        let one_pipe = crate::load::VoicingScope {
            keys: Some((54, 54)),
            rank: Some("Tierce".into()),
        };
        voice(&one_pipe, None, Some(-3.0), None, None).expect("voices one pipe");
        let parsed = def(&path);
        assert_eq!(parsed.voicing.adjusts.len(), 3);
        let pipe_rule = parsed
            .voicing
            .adjusts
            .iter()
            .find(|rule| rule.rank.is_some())
            .expect("the pipe rule");
        assert_eq!(pipe_rule.key_span(), Some(Ok((54, 54))));
        assert_eq!(pipe_rule.rank.as_deref(), Some("Tierce"));
        voice(&one_pipe, None, None, None, None).expect("clears the pipe");
        voice(&bass, None, None, None, None).expect("clears the bass");
        assert_eq!(def(&path).voicing.adjusts.len(), 1, "the stop's own survives");

        voice(&whole_stop, None, None, None, None).expect("neutral");
        assert!(def(&path).voicing.adjusts.is_empty(), "neutral removes the rule");
        assert!(
            !std::fs::read_to_string(&path).expect("reads").contains("[voicing"),
            "an empty section leaves no residue"
        );

        // Retarget the division stop: excepted from its division and
        // pulled afresh under its console label.
        assert!(
            retarget_composite_stop(
                &path,
                &division_stop,
                "Montre 8",
                "Great",
                "gib",
                "Récit",
                "Principal 8",
            )
            .expect("retargets")
        );
        let parsed = def(&path);
        assert_eq!(parsed.divisions[0].except, ["Montre 8"]);
        assert!(
            parsed.divisions[0].pitch_label.is_empty(),
            "the engraving left the division with the stop"
        );
        let fresh = parsed
            .stops
            .iter()
            .find(|pull| pull.stop == "Principal 8")
            .expect("a pull of its own");
        assert_eq!(fresh.from, "gib");
        assert_eq!(fresh.rename.as_deref(), Some("Montre 8"), "the label stays");
        assert_eq!(fresh.pitch_label.as_deref(), Some(""), "the engraving rides along");
        assert_eq!(fresh.own_pipes, Some(true), "the pipe sharing rides along");
        assert!(
            parsed.divisions[0].own_pipes.is_empty(),
            "and left the division with the stop"
        );
        assert!(parsed.moves.is_empty(), "a fresh pull lands directly");
        assert!(
            retarget_composite_stop(
                &path,
                &division_stop,
                "Montre 8",
                "Great",
                "ghost",
                "Récit",
                "X",
            )
            .is_err(),
            "an unknown alias must not poison the file"
        );

        // Remove the [[stop]]-pulled stop: its line goes.
        assert!(
            remove_composite_stop(&path, &pulled_stop, "Tromba", "Great").expect("removes")
        );
        assert!(
            !def(&path).stops.iter().any(|pull| pull.stop == "Trompette 8"),
            "the pull line is gone"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The per-coupler editors, addressed by the file itself: renames
    /// land on a define's own name line or in the [couplers.rename]
    /// map (keyed by the original, however often the label moves);
    /// editing a carried coupler's routes materializes it as a define
    /// under the console name with the original dropped; drop entries
    /// follow renames; a define name can't be defined twice.
    #[test]
    fn coupler_edits_rewrite_the_file() {
        let dir = std::env::temp_dir().join("aristide-coupler-edits-test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("test dir");
        let path = dir.join("orgue.toml");
        std::fs::write(
            &path,
            r#"name = "Coupled"

[sources]
anne = "/sets/anne.organ"

[[couplers.define]]
name = "Fourths II/I"
[[couplers.define.route]]
from = "Swell"
to = "Great"
shift = -5
"#,
        )
        .expect("fixture");
        let def = |path: &Path| -> aristide_formats::instrument::Definition {
            toml::from_str(&std::fs::read_to_string(path).expect("reads")).expect("parses")
        };
        let route = |from: &str, to: &str, shift: i16| CouplerRouteLine {
            from: from.into(),
            to: Some(to.into()),
            shift,
            low: None,
            high: None,
            unison_off: false,
            scope: aristide_model::CouplerScope::AllKeys,
            repitch: None,
            own_pipes: false,
        };

        // Renaming a define rewrites its own name line.
        rename_composite_coupler(&path, "Fourths II/I", "Quarts").expect("renames");
        assert_eq!(def(&path).couplers.define[0].name, "Quarts");

        // Renaming a carried coupler goes to the map — and a second
        // rename moves the same entry (keyed by the original), while
        // renaming back to the original retires it.
        rename_composite_coupler(&path, "Swell to Great", "II/I").expect("renames");
        assert_eq!(
            def(&path).couplers.rename.get("Swell to Great").map(String::as_str),
            Some("II/I")
        );
        rename_composite_coupler(&path, "II/I", "Récit/G.O.").expect("renames again");
        let parsed = def(&path);
        assert_eq!(parsed.couplers.rename.len(), 1, "one entry, moved");
        assert_eq!(
            parsed.couplers.rename.get("Swell to Great").map(String::as_str),
            Some("Récit/G.O.")
        );
        rename_composite_coupler(&path, "Récit/G.O.", "Swell to Great").expect("back");
        assert!(def(&path).couplers.rename.is_empty(), "back to the original = no entry");

        // Editing a define's routes replaces them in place.
        write_composite_coupler_routes(&path, "Quarts", &[route("Swell", "Great", -7)])
            .expect("routes");
        let parsed = def(&path);
        assert_eq!(parsed.couplers.define.len(), 1);
        assert_eq!(parsed.couplers.define[0].routes[0].shift, -7);

        // Editing a carried (and renamed) coupler's routes materializes
        // it: define under the console name, original dropped, map
        // entry retired.
        rename_composite_coupler(&path, "Swell to Great", "II/I").expect("renames");
        write_composite_coupler_routes(&path, "II/I", &[route("Swell", "Great", -12)])
            .expect("materializes");
        let parsed = def(&path);
        assert!(parsed.couplers.rename.is_empty(), "the define carries the name now");
        assert_eq!(parsed.couplers.drop, ["Swell to Great"], "the original is off the console");
        let own = parsed
            .couplers
            .define
            .iter()
            .find(|define| define.name == "II/I")
            .expect("materialized define");
        assert_eq!(own.routes[0].shift, -12);

        // A dropped name follows a later rename of a define.
        rename_composite_coupler(&path, "II/I", "Sub II/I").expect("renames define");
        assert!(def(&path).couplers.define.iter().any(|d| d.name == "Sub II/I"));

        // Materializing a NEVER-renamed carried coupler: the original
        // still bears the console name, so it's renamed out of the way
        // ("… (set)") before it's dropped — a drop entry under the
        // console name would hide the new define along with it.
        write_composite_coupler_routes(&path, "Swell to Pedal", &[route("Great", "Great", 12)])
            .expect("materializes unrenamed");
        let parsed = def(&path);
        assert!(parsed.couplers.define.iter().any(|d| d.name == "Swell to Pedal"));
        assert_eq!(
            parsed.couplers.rename.get("Swell to Pedal").map(String::as_str),
            Some("Swell to Pedal (set)")
        );
        assert!(
            parsed.couplers.drop.iter().any(|d| d == "Swell to Pedal (set)"),
            "the original hides under its out-of-the-way name: {:?}",
            parsed.couplers.drop
        );

        // A brand-new coupler appends; a taken define name refuses.
        append_composite_coupler(&path, "Great sub", &[route("Great", "Great", -12)])
            .expect("appends");
        assert!(append_composite_coupler(&path, "great SUB", &[route("Great", "Great", -12)])
            .is_err());
        assert_eq!(def(&path).couplers.define.len(), 4);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Couplers are addressed by name everywhere else the file speaks
    /// of them — link groups, the coupled-keys override, jamb seats in
    /// [console.order] — so a rename carries every reference along and
    /// a delete takes them all out; nothing may dangle.
    #[test]
    fn coupler_links_and_delete_keep_the_references_straight() {
        let dir = std::env::temp_dir().join("aristide-coupler-link-test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("test dir");
        let path = dir.join("orgue.toml");
        std::fs::write(
            &path,
            r#"name = "Linked"

[[couplers.define]]
name = "Gt/Ped"
[[couplers.define.route]]
from = "Great"
to = "Pedal"

[[couplers.define]]
name = "Gt/Ped (thumb)"
[[couplers.define.route]]
from = "Great"
to = "Pedal"

[console.order]
"Great" = ["Montre 8", "coupler:Gt/Ped (thumb)", "Bourdon 8"]
"#,
        )
        .expect("fixture");
        let def = |path: &Path| -> aristide_formats::instrument::Definition {
            toml::from_str(&std::fs::read_to_string(path).expect("reads")).expect("parses")
        };

        // Linking writes a group; linking a third into either merges.
        write_composite_coupler_link(&path, "Gt/Ped", "Gt/Ped (thumb)", true).expect("links");
        assert_eq!(def(&path).couplers.link, [["Gt/Ped", "Gt/Ped (thumb)"]]);
        write_composite_coupler_link(&path, "Gt/Ped", "II/I", true).expect("merges");
        assert_eq!(def(&path).couplers.link, [["Gt/Ped", "Gt/Ped (thumb)", "II/I"]]);
        write_composite_coupler_link(&path, "Gt/Ped", "II/I", false).expect("unlinks");
        assert_eq!(def(&path).couplers.link, [["Gt/Ped", "Gt/Ped (thumb)"]]);

        // The coupled-keys settings: an organ-wide default (true is
        // the default, so true removes the line) and per-coupler
        // overrides in [console.coupler_keys].
        write_composite_coupled_keys(&path, false).expect("writes");
        assert_eq!(def(&path).console.coupled_keys, Some(false));
        write_composite_coupled_keys(&path, true).expect("clears");
        assert_eq!(def(&path).console.coupled_keys, None);
        write_composite_coupler_key_mode(&path, "Gt/Ped (thumb)", Some("never")).expect("sets");
        assert_eq!(
            def(&path).console.coupler_keys.get("Gt/Ped (thumb)").map(String::as_str),
            Some("never")
        );

        // A rename carries the link entry, the override and the jamb
        // seat along.
        rename_composite_coupler(&path, "Gt/Ped (thumb)", "Gt/Ped (toe)").expect("renames");
        let parsed = def(&path);
        assert_eq!(parsed.couplers.link, [["Gt/Ped", "Gt/Ped (toe)"]]);
        assert_eq!(
            parsed.console.coupler_keys.get("Gt/Ped (toe)").map(String::as_str),
            Some("never")
        );
        assert_eq!(
            parsed.console.order.get("Great").expect("order")[1],
            "coupler:Gt/Ped (toe)"
        );

        // Deleting the define takes every reference with it — and the
        // group left with one member dissolves. A name the file
        // doesn't define reports false (the caller drops it off the
        // console instead).
        assert!(remove_composite_coupler(&path, "Gt/Ped (toe)").expect("removes"));
        let parsed = def(&path);
        assert_eq!(parsed.couplers.define.len(), 1);
        assert!(parsed.couplers.link.is_empty(), "{:?}", parsed.couplers.link);
        assert!(parsed.console.coupler_keys.is_empty());
        assert_eq!(
            parsed.console.order.get("Great").expect("order").as_slice(),
            ["Montre 8", "Bourdon 8"]
        );
        assert!(!remove_composite_coupler(&path, "Sw/Gt").expect("no such define"));
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

    /// `pipes` is written only as `exact`: keeping drift is the
    /// default and reads as absence.
    #[test]
    fn pipes_mode_writes_only_when_exact() {
        let path = std::env::temp_dir().join("aristide-pipes-mode-test.toml");
        std::fs::write(&path, "name = \"P\"\n[sources]\ns1 = \"x.organ\"\n").expect("writes");
        let fields = |pipes: crate::tuning::PipeRetune| ManualTuningFields {
            temperament: "equal".into(),
            edo: 12,
            reference: crate::tuning::PitchReference::A440,
            transpose: 0,
            scale: None,
            keymap: None,
            pipes,
        };
        write_composite_tuning(&path, &fields(crate::tuning::PipeRetune::Exact)).expect("exact");
        let text = std::fs::read_to_string(&path).expect("reads");
        assert!(text.contains("pipes = \"exact\""), "{text}");
        let def: aristide_formats::instrument::Definition =
            toml::from_str(&text).expect("parses");
        assert_eq!(def.tuning.pipes, "exact");
        write_composite_tuning(&path, &fields(crate::tuning::PipeRetune::Original))
            .expect("original");
        let text = std::fs::read_to_string(&path).expect("reads");
        assert!(!text.contains("pipes"), "{text}");
        let def: aristide_formats::instrument::Definition =
            toml::from_str(&text).expect("parses");
        assert_eq!(def.tuning.pipes, "original");
        let _ = std::fs::remove_file(&path);
    }

    /// The three whole-instrument settings the console commits without
    /// naming a manual: tuning, reverb wet, and the operating noises.
    /// Round-trips through the sidecar's own types, and follows the
    /// same absence rules a manual's own tuning does — a scale
    /// supersedes the temperament and the division count, and 12 EDO
    /// (the file's default) is never written.
    /// A set's own tuning lands under its `[sources]` entry (a bare
    /// path becoming a table with the same path); a stop's pin or own
    /// tuning, and a rank's own, are `[[tuning.stop]]` rows keyed by
    /// stop, manual and rank — one row per coordinate, the `[tuning]`
    /// header implicit unless it has fields of its own.
    #[test]
    fn write_source_and_stop_tuning_round_trip() {
        let dir = std::env::temp_dir().join("aristide-scope-tuning-test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("dir");
        let path = dir.join("scoped.toml");
        std::fs::write(
            &path,
            "name = \"Scoped\"\n\n[sources]\npositif = \"/sets/positif\"\n\n\
             [sources.great]\npath = \"/sets/great\"\nlayout = true\n\n\
             [[manual]]\nname = \"Récit\"\n",
        )
        .expect("writes");
        let def = |path: &Path| -> aristide_formats::instrument::Definition {
            toml::from_str(&std::fs::read_to_string(path).expect("reads")).expect("parses")
        };
        let fields = |temperament: &str| ManualTuningFields {
            temperament: temperament.into(),
            edo: 12,
            reference: crate::tuning::PitchReference { key: 69, hz: 415.0 },
            transpose: -2,
            scale: None,
            keymap: None,
            pipes: crate::tuning::PipeRetune::Original,
        };

        assert!(write_composite_source_tuning(&path, "positif", Some(&fields("meantone4"))).expect("set"));
        assert!(write_composite_source_tuning(&path, "great", Some(&fields("equal"))).expect("set"));
        assert!(!write_composite_source_tuning(&path, "ghost", Some(&fields("equal"))).expect("ghost"));
        let parsed = def(&path);
        let positif = &parsed.sources["positif"];
        assert_eq!(positif.path(), Path::new("/sets/positif"), "the path survives the table");
        assert_eq!(positif.tuning().and_then(|t| t.temperament.as_deref()), Some("meantone4"));
        assert_eq!(positif.tuning().and_then(|t| t.reference_hz), Some(415.0));
        let great = &parsed.sources["great"];
        assert!(great.layout(), "other source options survive");
        assert_eq!(great.tuning().and_then(|t| t.temperament.as_deref()), Some("equal"));
        let text = std::fs::read_to_string(&path).expect("reads");
        assert!(!text.contains("transpose"), "a set never transposes: {text}");
        assert!(write_composite_source_tuning(&path, "positif", None).expect("unset"));
        let parsed = def(&path);
        assert!(parsed.sources["positif"].tuning().is_none());
        assert_eq!(parsed.sources["positif"].path(), Path::new("/sets/positif"));

        // A pin, replaced by an own tuning at the same coordinate; a
        // rank row beside it; removal down to no [tuning] at all.
        let pin = Some(StopTuningEntry::Follow("source".into()));
        write_composite_stop_tuning(&path, "Bourdon 8", "Récit", None, pin).expect("pin");
        let parsed = def(&path);
        assert_eq!(parsed.tuning.stops.len(), 1);
        assert_eq!(parsed.tuning.stops[0].follow.as_deref(), Some("source"));
        assert_eq!(parsed.tuning.stops[0].manual.as_deref(), Some("Récit"));
        let text = std::fs::read_to_string(&path).expect("reads");
        assert!(text.contains("[[tuning.stop]]"), "{text}");
        assert!(!text.contains("[tuning]\n"), "the header stays implicit: {text}");

        let own = Some(StopTuningEntry::Own(fields("pythagorean")));
        write_composite_stop_tuning(&path, "bourdon 8", "récit", None, own.clone()).expect("own");
        let parsed = def(&path);
        assert_eq!(parsed.tuning.stops.len(), 1, "one row per coordinate");
        assert_eq!(parsed.tuning.stops[0].follow, None, "own tuning drops the pin");
        assert_eq!(parsed.tuning.stops[0].temperament.as_deref(), Some("pythagorean"));
        assert_eq!(parsed.tuning.stops[0].stop, "Bourdon 8", "the first spelling stands");

        write_composite_stop_tuning(&path, "Fourniture IV", "Récit", Some("Tierce"), own).expect("rank");
        let parsed = def(&path);
        assert_eq!(parsed.tuning.stops.len(), 2);
        assert_eq!(parsed.tuning.stops[1].rank.as_deref(), Some("Tierce"));

        write_composite_stop_tuning(&path, "Bourdon 8", "Récit", None, None).expect("unpin");
        assert_eq!(def(&path).tuning.stops.len(), 1, "the rank row stays");
        write_composite_stop_tuning(&path, "Fourniture IV", "Récit", Some("Tierce"), None).expect("rank off");
        assert!(def(&path).tuning.stops.is_empty());
        let text = std::fs::read_to_string(&path).expect("reads");
        assert!(!text.contains("[tuning") && !text.contains("follow"), "nothing left behind: {text}");

        // Under a [tuning] with fields of its own, rows append and the
        // fields survive.
        write_composite_tuning(&path, &fields("equal")).expect("instrument tuning");
        let pin = Some(StopTuningEntry::Follow("division".into()));
        write_composite_stop_tuning(&path, "Bourdon 8", "Récit", None, pin).expect("pin");
        let parsed = def(&path);
        assert_eq!(parsed.tuning.temperament, "equal");
        assert_eq!(parsed.tuning.transpose, -2);
        assert_eq!(parsed.tuning.stops.len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn write_composite_tuning_reverb_and_noises_round_trip() {
        let dir = std::env::temp_dir().join("aristide-instrument-settings-test");
        let _ = std::fs::remove_dir_all(&dir);
        let path = create_blank_organ(&dir, "Atelier").expect("creates");

        let def = |path: &Path| -> aristide_formats::instrument::Definition {
            toml::from_str(&std::fs::read_to_string(path).expect("reads")).expect("parses")
        };

        // A blank organ has neither table yet; noises and tuning grow
        // one, reverb refuses to (there is no IR to wet).
        write_composite_tuning(
            &path,
            &ManualTuningFields {
                temperament: "meantone4".into(),
                edo: 12,
                reference: crate::tuning::PitchReference { key: 69, hz: 415.0 },
                transpose: -2,
                scale: None,
                keymap: None,
                pipes: crate::tuning::PipeRetune::Original,
            },
        )
        .expect("tuning");
        write_composite_noises(&path, false, 0.4).expect("noises");
        write_composite_reverb_wet(&path, 0.6).expect("reverb wet is a no-op without [reverb]");

        let parsed = def(&path);
        assert_eq!(parsed.tuning.temperament, "meantone4");
        assert_eq!(parsed.tuning.edo, 12, "the default divisions, absent from the file");
        assert_eq!(parsed.tuning.reference_hz, Some(415.0));
        assert_eq!(parsed.tuning.reference_key.midi_note(), Some(69));
        assert_eq!(parsed.tuning.transpose, -2);
        assert_eq!(parsed.tuning.scale, None);
        assert!(!parsed.noises.enabled);
        assert_eq!(parsed.noises.volume, 0.4);
        assert_eq!(parsed.reverb.wet, 0.25, "no [reverb] table, so the default wet stands");
        let text = std::fs::read_to_string(&path).expect("reads");
        assert!(!text.contains("[reverb]"), "wet must not create a table: {text}");

        // A scale supersedes the temperament and the division count;
        // an EDO away from 12 drops the (12-EDO) temperament line.
        write_composite_tuning(
            &path,
            &ManualTuningFields {
                temperament: "equal".into(),
                edo: 31,
                reference: crate::tuning::PitchReference::A440,
                transpose: 0,
                scale: Some("19edo.scl".into()),
                keymap: Some("19edo.kbm".into()),
                pipes: crate::tuning::PipeRetune::Original,
            },
        )
        .expect("tuning");
        let parsed = def(&path);
        assert_eq!(parsed.tuning.scale.as_deref(), Some("19edo.scl"));
        assert_eq!(parsed.tuning.keymap.as_deref(), Some("19edo.kbm"));
        assert_eq!(parsed.tuning.edo, 12, "a scale supersedes the division count too");
        assert_eq!(parsed.tuning.temperament, "original", "the file's default, absent");

        write_composite_tuning(
            &path,
            &ManualTuningFields {
                temperament: "werckmeister3".into(),
                edo: 19,
                reference: crate::tuning::PitchReference::A440,
                transpose: 0,
                scale: None,
                keymap: None,
                pipes: crate::tuning::PipeRetune::Original,
            },
        )
        .expect("tuning");
        let parsed = def(&path);
        assert_eq!(parsed.tuning.scale, None, "naming a division count drops the scale");
        assert_eq!(parsed.tuning.edo, 19);
        assert_eq!(
            parsed.tuning.temperament, "original",
            "temperaments are 12-EDO vocabulary, dormant away from it"
        );

        // Now that [reverb] exists, wet writes into it.
        let mut doc: toml_edit::DocumentMut =
            std::fs::read_to_string(&path).expect("reads").parse().expect("parses");
        let mut reverb = toml_edit::Table::new();
        reverb["ir"] = toml_edit::value("hall.wav");
        doc["reverb"] = toml_edit::Item::Table(reverb);
        std::fs::write(&path, doc.to_string()).expect("writes");
        write_composite_reverb_wet(&path, 0.6).expect("reverb wet");
        let parsed = def(&path);
        assert_eq!(parsed.reverb.wet, 0.6);
        assert_eq!(parsed.reverb.ir, "hall.wav", "wet must not disturb the ir line");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The three writers touch only their own lines: a hand-authored
    /// file's comments and unrelated sections survive every edit.
    #[test]
    fn instrument_settings_writers_preserve_comments() {
        let dir = std::env::temp_dir().join("aristide-instrument-settings-comments-test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("creates dir");
        let path = dir.join("organ.toml");
        std::fs::write(
            &path,
            r#"# hand-made
name = "Atelier"

[sources]
s1 = "village.organ"

# concert pitch, a shade low
[tuning]
temperament = "equal"
a4_hz = 415.0

# a real IR, not synthetic
[reverb]
ir = "hall.wav"
wet = 0.25

# thumps and clacks
[noises]
enabled = true
volume = 0.7
"#,
        )
        .expect("writes");

        write_composite_tuning(
            &path,
            &ManualTuningFields {
                temperament: "meantone4".into(),
                edo: 12,
                reference: crate::tuning::PitchReference::A440,
                transpose: 0,
                scale: None,
                keymap: None,
                pipes: crate::tuning::PipeRetune::Original,
            },
        )
        .expect("tuning");
        write_composite_reverb_wet(&path, 0.5).expect("reverb wet");
        write_composite_noises(&path, false, 0.3).expect("noises");

        let text = std::fs::read_to_string(&path).expect("reads");
        assert!(text.contains("# hand-made"), "{text}");
        assert!(text.contains("# concert pitch, a shade low"), "{text}");
        assert!(text.contains("# a real IR, not synthetic"), "{text}");
        assert!(text.contains("# thumps and clacks"), "{text}");
        assert!(text.contains(r#"ir = "hall.wav""#), "reverb writer leaves ir alone: {text}");
        assert!(text.contains(r#"s1 = "village.organ""#));

        let def: aristide_formats::instrument::Definition =
            toml::from_str(&text).expect("parses");
        assert_eq!(def.tuning.temperament, "meantone4");
        assert_eq!(def.tuning.reference_hz, Some(440.0));
        assert!(!text.contains("a4_hz"), "the old spelling is rewritten: {text}");
        assert!(text.contains("reference_key = \"A4\""), "{text}");
        assert_eq!(def.reverb.wet, 0.5);
        assert!(!def.noises.enabled);
        assert_eq!(def.noises.volume, 0.3);

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
