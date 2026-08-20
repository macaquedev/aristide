//! Aristide-native composite organs: a small TOML file that *is* an
//! organ, pointing at big sample sets elsewhere.
//!
//! The file declares manuals out of thin air, pulls stops or whole
//! divisions from any number of source sets (loading only the ranks it
//! actually uses), places them on any manual, defines couplers across
//! all of it, and carries the instrument-wide settings a sidecar would
//! (tuning, wind, reverb…) plus its own MIDI wiring. Source sets are
//! read, never modified, and never need converting: a three-line file
//! naming one source and pulling nothing is that whole organ.
//!
//! ```toml
//! name = "Frankenorgan"
//!
//! [sources]
//! anne = "../sets/st-anne/demo.organ"     # relative to this file
//! gib = "/sets/giubiasco/giubiasco.organ"
//!
//! [[manual]]
//! name = "Great"
//! low = "C2"          # optional; omitted, the compass wraps what lands
//! high = "C7"
//!
//! [[division]]        # a whole division, stops and all
//! from = "gib"
//! manual = "hauptwerk"
//! on = "Great"        # omit to create a manual named like the source's
//!
//! [[stop]]            # one stop, surgically
//! from = "anne"
//! stop = "trompette"
//! on = "Great"
//! rename = "Trompette royale"
//!
//! [[couplers.define]] # cross-source couplers, same syntax as sidecars
//! name = "Great sub"
//! [[couplers.define.route]]
//! from = "Great"
//! to = "Great"
//! shift = -12
//!
//! [[midi.input]]      # this file owns its rig wiring
//! manual = "Great"
//! device = "KeyLab 61"
//! channel = 1
//! ```
//!
//! With no `[[stop]]`/`[[division]]` at all, every source contributes
//! everything — so wrapping an existing GrandOrgue or Hauptwerk set as
//! an Aristide organ is just `name` plus one `[sources]` line.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};

use serde::Deserialize;

use aristide_model::{
    Coupler, Enclosure, Manual, ManualId, Organ, PipeSource, Rank, RankId, RankRange, Stop,
    StopId, Windchest,
};

use crate::sidecar::{self, KeySpec, Sidecar};

#[derive(Debug, thiserror::Error)]
pub enum InstrumentError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("TOML error: {0}")]
    Toml(#[from] toml::de::Error),
    #[error("{0}")]
    Invalid(String),
    #[error("source {alias:?} ({path}): {message}", path = .path.display())]
    Source {
        alias: String,
        path: PathBuf,
        message: String,
    },
}

fn invalid(message: impl Into<String>) -> InstrumentError {
    InstrumentError::Invalid(message.into())
}

/// The file as written. Sections not listed here are the sidecar's,
/// carried through verbatim — a composite is its own sidecar.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Definition {
    pub name: String,
    /// Alias → source set, relative to this file.
    #[serde(default)]
    pub sources: BTreeMap<String, SourceDef>,
    #[serde(default, rename = "manual")]
    pub manuals: Vec<ManualDef>,
    #[serde(default, rename = "division")]
    pub divisions: Vec<DivisionPull>,
    #[serde(default, rename = "stop")]
    pub stops: Vec<StopPull>,
    #[serde(default, rename = "move")]
    pub moves: Vec<MoveDef>,
    #[serde(default)]
    pub midi: MidiDef,
    #[serde(default)]
    pub registration: sidecar::Registration,
    #[serde(default)]
    pub wind: sidecar::Wind,
    #[serde(default)]
    pub tremulant: sidecar::Tremulant,
    #[serde(default)]
    pub tuning: sidecar::TuningConfig,
    #[serde(default)]
    pub reverb: sidecar::ReverbConfig,
    #[serde(default)]
    pub noises: sidecar::NoisesConfig,
    #[serde(default)]
    pub enclosures: sidecar::EnclosuresConfig,
    #[serde(default)]
    pub couplers: sidecar::CouplersConfig,
}

/// One `[sources]` entry: a bare path, or a table adding options.
///
/// `layout = true` registers the source's windchests and enclosures up
/// front, whole and in the source's own order, even when the pulls are
/// selective — so chest numbers (`[tremulant] chests`…) and enclosure
/// order keep meaning exactly what they mean in the source. Adoption
/// writes it, which is what keeps an inventory of per-stop pulls
/// loading identically to the set itself. Without it, selective pulls
/// carry only the chests and boxes their pipes actually touch.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum SourceDef {
    Path(PathBuf),
    Detailed {
        path: PathBuf,
        #[serde(default)]
        layout: bool,
    },
}

impl SourceDef {
    pub fn path(&self) -> &Path {
        match self {
            SourceDef::Path(path) => path,
            SourceDef::Detailed { path, .. } => path,
        }
    }

    pub fn layout(&self) -> bool {
        match self {
            SourceDef::Path(_) => false,
            SourceDef::Detailed { layout, .. } => *layout,
        }
    }
}

/// A manual declared by the composite. Compass is optional: omitted,
/// it wraps exactly what lands on the manual. A manual may also carry
/// a tuning of its own — temperament, concert pitch, transpose — so
/// merged sources can disagree about pitch (415 meantone against 440
/// equal); fields left out follow the instrument-wide `[tuning]`.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManualDef {
    pub name: String,
    pub low: Option<KeySpec>,
    pub high: Option<KeySpec>,
    pub temperament: Option<String>,
    pub a4_hz: Option<f64>,
    pub transpose: Option<i8>,
}

/// Move one stop between manuals, after all pulls: the stop named on
/// `from` lands on `to`, re-anchored by pitch. This is how the console
/// persists "move this stop" without rewriting the pulls that brought
/// it in.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MoveDef {
    pub stop: String,
    pub from: String,
    pub to: String,
}

/// Pull a whole division: every stop of one source manual.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DivisionPull {
    pub from: String,
    /// Source manual, by the usual name-pattern rules.
    pub manual: String,
    /// Composite manual it lands on; omitted, a new manual is created
    /// with the source manual's name and compass.
    pub on: Option<String>,
}

/// Pull one stop (or every stop a pattern matches) onto a manual.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StopPull {
    pub from: String,
    /// Restrict the stop pattern to one source manual's stops — two
    /// divisions routinely both have a "Bourdon 8", and an inventory
    /// must be able to mean exactly one of them.
    pub manual: Option<String>,
    pub stop: String,
    pub on: String,
    /// New console name; only applied when the pattern matched exactly
    /// one stop.
    pub rename: Option<String>,
}

/// The composite's own MIDI wiring — this file is where the rig lives,
/// so bindings learned live are written back here.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MidiDef {
    /// Producer-style channel suggestions, as in a sidecar.
    #[serde(default)]
    pub channels: Vec<String>,
    #[serde(default, rename = "input")]
    pub inputs: Vec<InputDef>,
    #[serde(default, rename = "control")]
    pub controls: Vec<ControlDef>,
}

/// One keyboard playing one manual — the file-side twin of the server's
/// learned assignment.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InputDef {
    pub manual: String,
    pub device: String,
    pub channel: Option<u8>,
    pub low: Option<u8>,
    pub high: Option<u8>,
    #[serde(default)]
    pub transpose: i8,
}

/// One control binding: this message, from this device, does this.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ControlDef {
    pub device: String,
    pub channel: Option<u8>,
    pub trigger: String,
    pub action: String,
    pub manual: Option<String>,
}

impl Definition {
    /// Everything downstream of loading treats a composite exactly
    /// like a set + sidecar; this is that sidecar.
    pub fn to_sidecar(&self) -> Sidecar {
        Sidecar {
            // A composite is named by its own `name`, not a sidecar
            // override.
            name: String::new(),
            midi: sidecar::Midi {
                channels: self.midi.channels.clone(),
            },
            registration: self.registration.clone(),
            wind: self.wind.clone(),
            tremulant: self.tremulant.clone(),
            tuning: self.tuning.clone(),
            reverb: self.reverb.clone(),
            noises: self.noises.clone(),
            enclosures: self.enclosures.clone(),
            couplers: self.couplers.clone(),
        }
    }

    /// Aliases a pull actually names; with no pulls at all, every
    /// source contributes everything, so all of them. A `layout = true`
    /// source is used by definition — its chests and boxes shape the
    /// instrument even before anything is pulled from it.
    fn used_sources(&self) -> HashSet<&str> {
        if self.divisions.is_empty() && self.stops.is_empty() {
            self.sources.keys().map(String::as_str).collect()
        } else {
            self.divisions
                .iter()
                .map(|d| d.from.as_str())
                .chain(self.stops.iter().map(|s| s.from.as_str()))
                .chain(
                    self.sources
                        .iter()
                        .filter(|(_, source)| source.layout())
                        .map(|(alias, _)| alias.as_str()),
                )
                .collect()
        }
    }
}

/// One declared manual's own tuning: (manual index, temperament name,
/// a4 Hz, transpose), each `None` meaning "follow the instrument-wide
/// `[tuning]`".
pub type ManualTuningDef = (usize, Option<String>, Option<f64>, Option<i8>);

#[derive(Debug)]
pub struct Assembled {
    pub organ: Organ,
    /// The composite's instrument-wide settings, in sidecar form.
    pub sidecar: Sidecar,
    pub midi: MidiDef,
    /// (source index, source stop id) → the assembled stop, so
    /// decisions made against a source's own names (a sidecar's
    /// default registration) can ride across the assembly.
    pub stop_map: HashMap<(usize, StopId), StopId>,
    /// Every whole-division pull that happened: (source index, source
    /// manual name, assembled manual index). This is the composite's
    /// structure in saveable form — a definition file that replays
    /// these pulls rebuilds the same instrument.
    pub division_pulls: Vec<(usize, String, usize)>,
    /// Declared manuals that carry a tuning of their own. Parsing the
    /// temperament is the server's business — the format stays a name.
    pub manual_tuning: Vec<ManualTuningDef>,
    pub warnings: Vec<String>,
}

/// Whether a set path names a composite definition rather than a
/// sample-set format.
pub fn is_definition(path: &Path) -> bool {
    path.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("toml"))
}

/// Load a composite organ: parse the file, load the sources it uses,
/// assemble the instrument.
pub fn load(path: &Path) -> Result<Assembled, InstrumentError> {
    let text = std::fs::read_to_string(path)?;
    let def: Definition = toml::from_str(&text)?;
    if def.name.trim().is_empty() {
        return Err(invalid("the organ needs a name"));
    }
    let dir = path.parent().unwrap_or(Path::new(""));
    let used = def.used_sources();
    let mut sources = Vec::new();
    let mut warnings = Vec::new();
    for (alias, source) in &def.sources {
        if !used.contains(alias.as_str()) {
            warnings.push(format!("source {alias:?} is never used — not loaded"));
            continue;
        }
        let source_path = source.path();
        let resolved = if source_path.is_absolute() {
            source_path.to_path_buf()
        } else {
            dir.join(source_path)
        };
        let loaded =
            crate::grandorgue::load(&resolved).map_err(|e| InstrumentError::Source {
                alias: alias.clone(),
                path: resolved.clone(),
                message: e.to_string(),
            })?;
        warnings.extend(loaded.warnings.iter().map(|w| format!("{alias}: {w}")));
        sources.push((alias.clone(), loaded.organ));
    }
    assemble(&def, &sources, warnings)
}

/// A stop placed but not yet fixed to a compass: its ranges anchor to
/// absolute MIDI notes (what the source keys meant), and only once
/// every manual's compass is settled do they become key indices again.
struct PlacedStop {
    name: String,
    manual: usize,
    ranges: Vec<PlacedRange>,
}

struct PlacedRange {
    rank: RankId,
    first_midi: i32,
    key_count: i32,
    first_pipe: i32,
}

struct Assembly<'a> {
    sources: &'a [(String, Organ)],
    stop_map: HashMap<(usize, StopId), StopId>,
    division_pulls: Vec<(usize, String, usize)>,
    manuals: Vec<Manual>,
    /// Per manual: the file-declared compass, if any.
    declared: Vec<Option<(u8, u8)>>,
    placed: Vec<PlacedStop>,
    ranks: Vec<Rank>,
    windchests: Vec<Windchest>,
    enclosures: Vec<Enclosure>,
    /// Source manuals pulled as whole divisions, per source: the only
    /// pulls that carry the source's own couplers with them.
    division_map: Vec<HashMap<ManualId, usize>>,
    rank_map: HashMap<(usize, RankId), RankId>,
    chest_map: HashMap<(usize, u32), u32>,
    enclosure_map: HashMap<(usize, u32), u32>,
    warnings: Vec<String>,
}

/// Assemble a composite from already-loaded sources. `load` is this
/// after parsing and source loading; the server also calls it directly
/// to make a multi-set launch an implicit composite (a [`Definition`]
/// with sources and no pulls — everything, as the sources define it).
pub fn assemble(
    def: &Definition,
    sources: &[(String, Organ)],
    warnings: Vec<String>,
) -> Result<Assembled, InstrumentError> {
    let mut assembly = Assembly {
        sources,
        stop_map: HashMap::new(),
        division_pulls: Vec::new(),
        manuals: Vec::new(),
        declared: Vec::new(),
        placed: Vec::new(),
        ranks: Vec::new(),
        windchests: Vec::new(),
        enclosures: Vec::new(),
        division_map: vec![HashMap::new(); sources.len()],
        rank_map: HashMap::new(),
        chest_map: HashMap::new(),
        enclosure_map: HashMap::new(),
        warnings,
    };
    for manual in &def.manuals {
        let compass = declared_compass(manual)?;
        assembly.create_manual(manual.name.clone(), compass);
    }
    if def.divisions.is_empty() && def.stops.is_empty() {
        // No pulls: every source contributes everything — wrapping a
        // set as an organ is just naming it as a source. Wind and
        // boxes register up front in the source's own order, so a
        // wrapped set keeps its numbering exactly (its sidecar's
        // `[tremulant] chests` still mean the same chests); selective
        // pulls instead carry only what their pipes touch.
        for (source_idx, (_, organ)) in sources.iter().enumerate() {
            for enclosure in 0..organ.enclosures.len() as u32 {
                assembly.enclosure_index(source_idx, enclosure);
            }
            let mut numbers: Vec<u32> = organ.windchests.iter().map(|c| c.number).collect();
            numbers.sort_unstable();
            for number in numbers {
                assembly.chest_number(source_idx, number);
            }
            for manual_idx in 0..organ.manuals.len() {
                assembly.pull_division(source_idx, manual_idx, None)?;
            }
        }
    } else {
        // Layout sources first: their chests and boxes register whole,
        // in the source's own order, before any pull touches them —
        // the same up-front pass the implicit branch does, so numbering
        // survives selective pulls.
        for (alias, source) in &def.sources {
            if !source.layout() {
                continue;
            }
            let Ok(source_idx) = assembly.source(alias) else {
                continue; // not loaded (unreadable earlier, already reported)
            };
            let organ = &sources[source_idx].1;
            for enclosure in 0..organ.enclosures.len() as u32 {
                assembly.enclosure_index(source_idx, enclosure);
            }
            let mut numbers: Vec<u32> = organ.windchests.iter().map(|c| c.number).collect();
            numbers.sort_unstable();
            for number in numbers {
                assembly.chest_number(source_idx, number);
            }
        }
        for pull in &def.divisions {
            let source_idx = assembly.source(&pull.from)?;
            let organ = &sources[source_idx].1;
            let names: Vec<&str> = organ.manuals.iter().map(|m| m.name.as_str()).collect();
            let matches = sidecar::match_names(&names, &pull.manual);
            if matches.is_empty() {
                assembly.warnings.push(format!(
                    "division {:?}: {:?} has no such manual — skipped",
                    pull.manual, pull.from
                ));
            }
            for manual_idx in matches {
                assembly.pull_division(source_idx, manual_idx, pull.on.as_deref())?;
            }
        }
        for pull in &def.stops {
            let source_idx = assembly.source(&pull.from)?;
            let organ = &sources[source_idx].1;
            // A `manual` filter narrows the candidates to one source
            // division's stops before the name pattern runs.
            let candidates: Vec<usize> = match &pull.manual {
                Some(pattern) => {
                    let manual_names: Vec<&str> =
                        organ.manuals.iter().map(|m| m.name.as_str()).collect();
                    let manuals: HashSet<ManualId> =
                        sidecar::match_names(&manual_names, pattern)
                            .into_iter()
                            .map(|index| organ.manuals[index].id)
                            .collect();
                    if manuals.is_empty() {
                        assembly.warnings.push(format!(
                            "stop {:?}: {:?} has no manual {pattern:?} — skipped",
                            pull.stop, pull.from
                        ));
                        continue;
                    }
                    organ
                        .stops
                        .iter()
                        .enumerate()
                        .filter(|(_, stop)| manuals.contains(&stop.manual))
                        .map(|(index, _)| index)
                        .collect()
                }
                None => (0..organ.stops.len()).collect(),
            };
            let names: Vec<&str> = candidates
                .iter()
                .map(|&index| organ.stops[index].name.as_str())
                .collect();
            let matches: Vec<usize> = sidecar::match_names(&names, &pull.stop)
                .into_iter()
                .map(|at| candidates[at])
                .collect();
            if matches.is_empty() {
                assembly.warnings.push(format!(
                    "stop {:?}: {:?} has no such stop — skipped",
                    pull.stop, pull.from
                ));
                continue;
            }
            let rename = match (matches.len(), &pull.rename) {
                (1, rename) => rename.clone(),
                (_, Some(rename)) => {
                    assembly.warnings.push(format!(
                        "stop {:?} matched {} stops — rename {rename:?} not applied",
                        pull.stop,
                        matches.len()
                    ));
                    None
                }
                _ => None,
            };
            let target = assembly.find_manual(&pull.on)?;
            for stop_idx in matches {
                assembly.place_stop(source_idx, stop_idx, target, rename.clone());
            }
        }
    }
    // Moves come last: whatever the pulls assembled, a [[move]] entry
    // relocates by name — ranges stay pitch-anchored until `finish`,
    // so re-anchoring onto the new manual is automatic.
    for wanted in &def.moves {
        let from = assembly.find_manual(&wanted.from)?;
        let to = assembly.find_manual(&wanted.to)?;
        let on_from: Vec<usize> = assembly
            .placed
            .iter()
            .enumerate()
            .filter(|(_, stop)| stop.manual == from)
            .map(|(index, _)| index)
            .collect();
        let names: Vec<&str> = on_from
            .iter()
            .map(|&index| assembly.placed[index].name.as_str())
            .collect();
        let matches = sidecar::match_names(&names, &wanted.stop);
        if matches.is_empty() {
            assembly.warnings.push(format!(
                "move: no stop {:?} on {:?} — skipped",
                wanted.stop, wanted.from
            ));
        }
        for at in matches {
            assembly.placed[on_from[at]].manual = to;
        }
    }
    let manual_tuning = def
        .manuals
        .iter()
        .enumerate()
        .filter(|(_, manual)| {
            manual.temperament.is_some() || manual.a4_hz.is_some() || manual.transpose.is_some()
        })
        .map(|(index, manual)| {
            (
                index,
                manual.temperament.clone(),
                manual.a4_hz,
                manual.transpose,
            )
        })
        .collect();
    let organ = assembly.finish(def.name.clone());
    Ok(Assembled {
        organ,
        sidecar: def.to_sidecar(),
        midi: def.midi.clone(),
        stop_map: assembly.stop_map,
        division_pulls: assembly.division_pulls,
        manual_tuning,
        warnings: assembly.warnings,
    })
}

fn declared_compass(manual: &ManualDef) -> Result<Option<(u8, u8)>, InstrumentError> {
    let note = |spec: &KeySpec, end: &str| {
        spec.midi_note().ok_or_else(|| {
            invalid(format!("manual {:?}: {end} is not a key", manual.name))
        })
    };
    match (&manual.low, &manual.high) {
        (Some(low), Some(high)) => {
            let (low, high) = (note(low, "low")?, note(high, "high")?);
            if low > high {
                return Err(invalid(format!(
                    "manual {:?}: low is above high",
                    manual.name
                )));
            }
            Ok(Some((low, high)))
        }
        (None, None) => Ok(None),
        _ => Err(invalid(format!(
            "manual {:?}: give both low and high, or neither",
            manual.name
        ))),
    }
}

impl Assembly<'_> {
    fn source(&self, alias: &str) -> Result<usize, InstrumentError> {
        self.sources
            .iter()
            .position(|(name, _)| name == alias)
            .ok_or_else(|| invalid(format!("{alias:?} is not a [sources] alias")))
    }

    fn create_manual(&mut self, name: String, declared: Option<(u8, u8)>) -> usize {
        let index = self.manuals.len();
        self.manuals.push(Manual {
            id: ManualId(index as u32),
            name,
            // Placeholders; `finish` settles every compass at once.
            first_midi_note: 36,
            key_count: 61,
        });
        self.declared.push(declared);
        index
    }

    /// The composite manual a name means. Creation is never implicit
    /// here: a typo must fail loudly, not conjure a silent manual.
    fn find_manual(&self, pattern: &str) -> Result<usize, InstrumentError> {
        let names: Vec<&str> = self.manuals.iter().map(|m| m.name.as_str()).collect();
        match sidecar::match_names(&names, pattern).as_slice() {
            [index] => Ok(*index),
            [] => Err(invalid(format!(
                "{pattern:?} names no manual of this organ — declare it with [[manual]]"
            ))),
            _ => Err(invalid(format!("{pattern:?} is ambiguous between manuals"))),
        }
    }

    fn pull_division(
        &mut self,
        source_idx: usize,
        manual_idx: usize,
        on: Option<&str>,
    ) -> Result<(), InstrumentError> {
        let (alias, organ) = &self.sources[source_idx];
        let source_manual = organ.manuals[manual_idx].clone();
        let target = match on {
            Some(name) => self.find_manual(name)?,
            None => {
                // A created manual keeps the source's name (suffixed
                // only on collision) and its exact compass.
                let collides = self
                    .manuals
                    .iter()
                    .any(|m| m.name.eq_ignore_ascii_case(&source_manual.name));
                let name = if collides {
                    format!("{} — {alias}", source_manual.name)
                } else {
                    source_manual.name.clone()
                };
                let low = source_manual.first_midi_note;
                let high =
                    (low as i32 + source_manual.key_count as i32 - 1).clamp(0, 127) as u8;
                self.create_manual(name, Some((low, high)))
            }
        };
        self.division_map[source_idx].insert(source_manual.id, target);
        self.division_pulls
            .push((source_idx, source_manual.name.clone(), target));
        let stops: Vec<usize> = organ
            .stops
            .iter()
            .enumerate()
            .filter(|(_, stop)| stop.manual == source_manual.id)
            .map(|(index, _)| index)
            .collect();
        for stop_idx in stops {
            self.place_stop(source_idx, stop_idx, target, None);
        }
        Ok(())
    }

    fn place_stop(
        &mut self,
        source_idx: usize,
        stop_idx: usize,
        target: usize,
        rename: Option<String>,
    ) {
        let organ = &self.sources[source_idx].1;
        let stop = &organ.stops[stop_idx];
        self.stop_map
            .insert((source_idx, stop.id), StopId(self.placed.len() as u32));
        // Anchoring is by pitch position, not key index: the key that
        // meant tenor C on the source manual means tenor C wherever
        // the stop lands.
        let first_midi = organ
            .manuals
            .iter()
            .find(|m| m.id == stop.manual)
            .map(|m| m.first_midi_note as i32)
            .unwrap_or_else(|| {
                self.warnings.push(format!(
                    "stop {:?} sits on a manual its own set hasn't got — anchored at 36",
                    stop.name
                ));
                36
            });
        let name = rename.unwrap_or_else(|| stop.name.clone());
        let ranges = stop
            .ranks
            .clone()
            .iter()
            .map(|range| PlacedRange {
                rank: self.pull_rank(source_idx, range.rank),
                first_midi: first_midi + range.first_key as i32,
                key_count: range.key_count as i32,
                first_pipe: range.first_pipe as i32,
            })
            .collect();
        self.placed.push(PlacedStop {
            name,
            manual: target,
            ranges,
        });
    }

    /// Pull one rank (once per source rank, however many stops share
    /// it), its samples pointed at the source's own directory, its
    /// borrows followed so a unit rank's donor comes along even when
    /// no pulled stop names it.
    fn pull_rank(&mut self, source_idx: usize, old: RankId) -> RankId {
        if let Some(new) = self.rank_map.get(&(source_idx, old)) {
            return *new;
        }
        let index = self.ranks.len();
        let new = RankId(index as u32);
        self.rank_map.insert((source_idx, old), new);
        // Reserve the slot before following borrows: the donor chain
        // appends while this rank is mid-pull.
        self.ranks.push(Rank {
            id: new,
            name: String::new(),
            windchest: 0,
            pipes: Vec::new(),
        });
        let organ = &self.sources[source_idx].1;
        let Some(source_rank) = organ.rank(old) else {
            self.warnings.push(format!(
                "a stop references rank {} which {:?} hasn't got",
                old.0, self.sources[source_idx].0
            ));
            return new;
        };
        let mut rank = source_rank.clone();
        let base = organ.base_path.clone();
        rank.id = new;
        rank.windchest = match rank.windchest {
            0 => 0,
            chest => self.chest_number(source_idx, chest),
        };
        for pipe in &mut rank.pipes {
            match &mut pipe.source {
                PipeSource::Sampled { attacks, releases } => {
                    for attack in attacks {
                        attack.path = base.join(&attack.path);
                    }
                    for release in releases {
                        release.path = base.join(&release.path);
                    }
                }
                PipeSource::Borrowed(target) => {
                    target.rank = self.pull_rank(source_idx, target.rank);
                }
                PipeSource::Silent => {}
            }
        }
        self.ranks[index] = rank;
        new
    }

    /// The composite windchest number for a source chest, pulling the
    /// chest (and the enclosures it sits in) on first use — so the
    /// composite carries exactly the wind and boxes its pipes need.
    fn chest_number(&mut self, source_idx: usize, old: u32) -> u32 {
        if let Some(new) = self.chest_map.get(&(source_idx, old)) {
            return *new;
        }
        let new = self.windchests.len() as u32 + 1;
        self.chest_map.insert((source_idx, old), new);
        let (alias, organ) = &self.sources[source_idx];
        let mut chest = organ
            .windchests
            .iter()
            .find(|c| c.number == old)
            .cloned()
            .unwrap_or_else(|| Windchest {
                number: old,
                name: format!("{alias} chest {old}"),
                enclosures: Vec::new(),
            });
        chest.number = new;
        chest.enclosures = chest
            .enclosures
            .clone()
            .into_iter()
            .filter_map(|e| self.enclosure_index(source_idx, e))
            .collect();
        self.windchests.push(chest);
        new
    }

    fn enclosure_index(&mut self, source_idx: usize, old: u32) -> Option<u32> {
        if let Some(new) = self.enclosure_map.get(&(source_idx, old)) {
            return Some(*new);
        }
        let organ = &self.sources[source_idx].1;
        let Some(enclosure) = organ.enclosures.get(old as usize) else {
            self.warnings.push(format!(
                "{:?}: a windchest sits in enclosure {old}, which it hasn't got",
                self.sources[source_idx].0
            ));
            return None;
        };
        let new = self.enclosures.len() as u32;
        self.enclosure_map.insert((source_idx, old), new);
        self.enclosures.push(enclosure.clone());
        Some(new)
    }

    /// Source couplers ride along only when every manual they touch
    /// was pulled as a whole division — a lone stop doesn't drag its
    /// old console's couplers behind it.
    fn carry_couplers(&mut self) -> Vec<Coupler> {
        let mut carried = Vec::new();
        for (source_idx, (_, organ)) in self.sources.iter().enumerate() {
            let map = &self.division_map[source_idx];
            for coupler in &organ.couplers {
                let complete = coupler.routes.iter().all(|route| {
                    map.contains_key(&route.from_manual)
                        && route
                            .target
                            .as_ref()
                            .is_none_or(|t| map.contains_key(&t.manual))
                });
                if !complete {
                    continue;
                }
                let mut coupler = coupler.clone();
                for route in &mut coupler.routes {
                    route.from_manual = ManualId(map[&route.from_manual] as u32);
                    if let Some(target) = &mut route.target {
                        target.manual = ManualId(map[&target.manual] as u32);
                    }
                }
                carried.push((self.sources[source_idx].0.clone(), coupler));
            }
        }
        // Two sources' "Swell to Great" must stay tellable apart.
        let mut counts: HashMap<String, u32> = HashMap::new();
        for (_, coupler) in &carried {
            *counts.entry(coupler.name.to_lowercase()).or_insert(0) += 1;
        }
        carried
            .into_iter()
            .map(|(alias, mut coupler)| {
                if counts[&coupler.name.to_lowercase()] > 1 {
                    coupler.name = format!("{} — {alias}", coupler.name);
                }
                coupler
            })
            .collect()
    }

    /// Settle every manual's compass, turn the pitch-anchored ranges
    /// back into key indices (trimming what a declared compass cuts
    /// off), and produce the organ.
    fn finish(&mut self, name: String) -> Organ {
        let couplers = self.carry_couplers();
        for (index, manual) in self.manuals.iter_mut().enumerate() {
            let (low, high) = match self.declared[index] {
                Some(compass) => compass,
                None => {
                    let spans: Vec<(i32, i32)> = self
                        .placed
                        .iter()
                        .filter(|stop| stop.manual == index)
                        .flat_map(|stop| &stop.ranges)
                        .map(|r| (r.first_midi, r.first_midi + r.key_count - 1))
                        .collect();
                    match spans.iter().copied().reduce(|a, b| (a.0.min(b.0), a.1.max(b.1)))
                    {
                        Some((low, high)) => (low.clamp(0, 127) as u8, high.clamp(0, 127) as u8),
                        None => {
                            self.warnings.push(format!(
                                "manual {:?} has no stops — 61-key default compass",
                                manual.name
                            ));
                            (36, 96)
                        }
                    }
                }
            };
            manual.first_midi_note = low;
            manual.key_count = (high as i32 - low as i32 + 1) as u16;
        }
        let stops = self
            .placed
            .iter()
            .enumerate()
            .map(|(index, placed)| {
                let manual = &self.manuals[placed.manual];
                let ranks: Vec<RankRange> = placed
                    .ranges
                    .iter()
                    .filter_map(|range| {
                        let mut first_key = range.first_midi - manual.first_midi_note as i32;
                        let mut key_count = range.key_count;
                        let mut first_pipe = range.first_pipe;
                        if first_key < 0 {
                            first_pipe -= first_key;
                            key_count += first_key;
                            first_key = 0;
                        }
                        key_count = key_count.min(manual.key_count as i32 - first_key);
                        (key_count > 0).then_some(RankRange {
                            rank: range.rank,
                            first_key: first_key as u16,
                            key_count: key_count as u16,
                            first_pipe: first_pipe as u16,
                        })
                    })
                    .collect();
                if ranks.is_empty() && !placed.ranges.is_empty() {
                    self.warnings.push(format!(
                        "stop {:?} lies entirely outside {:?}'s compass",
                        placed.name, manual.name
                    ));
                }
                Stop {
                    id: StopId(index as u32),
                    name: placed.name.clone(),
                    manual: ManualId(placed.manual as u32),
                    ranks,
                }
            })
            .collect();
        Organ {
            name,
            base_path: PathBuf::new(),
            manuals: std::mem::take(&mut self.manuals),
            stops,
            ranks: std::mem::take(&mut self.ranks),
            couplers,
            enclosures: std::mem::take(&mut self.enclosures),
            windchests: std::mem::take(&mut self.windchests),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aristide_model::{AttackSample, Pipe, PipeRef, ReleaseSample};

    /// A two-manual source: Great (36..96) with "Principal 8" on chest
    /// 1 (unenclosed), Swell (36..96) with "Hautbois 8" on chest 2
    /// inside an enclosure, a Swell-to-Great coupler, and a borrow in
    /// the Hautbois rank pointing at a donor rank no stop names.
    fn source(name: &str, base: &str) -> Organ {
        let pipe = |source: PipeSource| Pipe {
            nominal_frequency_hz: 440.0,
            pitch_tuning_cents: 0.0,
            gain_db: 0.0,
            midi_key_number: None,
            source,
        };
        let sampled = |path: &str| {
            pipe(PipeSource::Sampled {
                attacks: vec![AttackSample {
                    path: PathBuf::from(path),
                    loops: Vec::new(),
                    pitch_offset_cents: 0.0,
                }],
                releases: vec![ReleaseSample {
                    path: PathBuf::from(path),
                    max_key_press_ms: None,
                }],
            })
        };
        let rank = |id: u32, name: &str, windchest: u32, pipes: Vec<Pipe>| Rank {
            id: RankId(id),
            name: name.into(),
            windchest,
            pipes,
        };
        Organ {
            name: name.into(),
            base_path: PathBuf::from(base),
            manuals: vec![
                Manual {
                    id: ManualId(0),
                    name: "Great".into(),
                    first_midi_note: 36,
                    key_count: 61,
                },
                Manual {
                    id: ManualId(1),
                    name: "Swell".into(),
                    first_midi_note: 36,
                    key_count: 61,
                },
            ],
            stops: vec![
                Stop {
                    id: StopId(10),
                    name: "Principal 8".into(),
                    manual: ManualId(0),
                    ranks: vec![RankRange {
                        rank: RankId(1),
                        first_key: 0,
                        key_count: 61,
                        first_pipe: 0,
                    }],
                },
                Stop {
                    id: StopId(11),
                    name: "Hautbois 8".into(),
                    manual: ManualId(1),
                    ranks: vec![RankRange {
                        rank: RankId(2),
                        first_key: 0,
                        key_count: 61,
                        first_pipe: 0,
                    }],
                },
            ],
            ranks: vec![
                rank(1, "Principal 8", 1, vec![sampled("prin.wav"), sampled("prin2.wav")]),
                rank(
                    2,
                    "Hautbois 8",
                    2,
                    vec![
                        sampled("haut.wav"),
                        pipe(PipeSource::Borrowed(PipeRef {
                            rank: RankId(3),
                            pipe: 0,
                        })),
                    ],
                ),
                rank(3, "Donor", 2, vec![sampled("donor.wav")]),
            ],
            couplers: vec![Coupler::simple("Swell to Great", ManualId(1), ManualId(0), 0)],
            enclosures: vec![
                Enclosure {
                    name: "Unused box".into(),
                    amp_minimum_level: 30.0,
                    midi_input_number: None,
                    displayed: false,
                },
                Enclosure {
                    name: "Swell box".into(),
                    amp_minimum_level: 20.0,
                    midi_input_number: None,
                    displayed: true,
                },
            ],
            windchests: vec![
                Windchest {
                    number: 1,
                    name: "Main".into(),
                    enclosures: vec![],
                },
                Windchest {
                    number: 2,
                    name: "Swell chest".into(),
                    enclosures: vec![1],
                },
            ],
        }
    }

    fn def(name: &str) -> Definition {
        Definition {
            name: name.into(),
            ..Default::default()
        }
    }

    fn manual(name: &str, low: Option<i64>, high: Option<i64>) -> ManualDef {
        ManualDef {
            name: name.into(),
            low: low.map(KeySpec::Number),
            high: high.map(KeySpec::Number),
            temperament: None,
            a4_hz: None,
            transpose: None,
        }
    }

    #[test]
    fn wrapping_sources_preserves_compass_couplers_and_names() {
        let sources = vec![
            ("A".to_string(), source("A", "/a")),
            ("B".to_string(), source("B", "/b")),
        ];
        let built = assemble(&def("Both"), &sources, Vec::new()).expect("assembles");
        let organ = &built.organ;
        let names: Vec<&str> = organ.manuals.iter().map(|m| m.name.as_str()).collect();
        assert_eq!(names, ["Great", "Swell", "Great — B", "Swell — B"]);
        for manual in &organ.manuals {
            assert_eq!((manual.first_midi_note, manual.key_count), (36, 61));
        }
        // Both sources' couplers carried, routed within their own
        // manuals, names disambiguated.
        assert_eq!(organ.couplers.len(), 2);
        assert!(organ.couplers[0].couples(ManualId(1), ManualId(0)));
        assert!(organ.couplers[1].couples(ManualId(3), ManualId(2)));
        assert_eq!(organ.couplers[1].name, "Swell to Great — B");
        // Stop ranges land exactly where the sources put them.
        assert_eq!(organ.stops.len(), 4);
        assert_eq!(organ.stops[3].manual, ManualId(3));
        assert_eq!(organ.stops[3].ranks[0].first_key, 0);
        // B's registration decisions can follow their stop across.
        assert_eq!(built.stop_map[&(1, StopId(11))], organ.stops[3].id);
        // Wrapping is faithful: ALL wind and boxes carried, in source
        // order, so A's chest numbers survive unchanged.
        assert_eq!(organ.enclosures.len(), 4);
        assert_eq!(organ.windchests.len(), 4);
        assert_eq!(organ.windchests[0].number, 1);
        assert_eq!(organ.windchests[1].enclosures, vec![1]);
        assert_eq!(organ.windchests[3].enclosures, vec![3]);
        assert_eq!(organ.ranks.iter().map(|r| r.windchest).collect::<Vec<_>>(), [1, 2, 2, 3, 4, 4]);
    }

    #[test]
    fn stop_pull_anchors_by_pitch_and_trims_to_compass() {
        let sources = vec![("A".to_string(), source("A", "/a"))];
        let mut definition = def("Solo");
        definition.manuals = vec![manual("Solo", Some(48), Some(96))];
        definition.stops = vec![StopPull {
            from: "A".into(),
            manual: None,
            stop: "Principal 8".into(),
            on: "Solo".into(),
            rename: Some("Montre 8".into()),
        }];
        let built = assemble(&definition, &sources, Vec::new()).expect("assembles");
        let organ = &built.organ;
        assert_eq!(organ.stops.len(), 1);
        assert_eq!(organ.stops[0].name, "Montre 8");
        // Source keys meant MIDI 36..96; the manual starts at 48, so
        // the first 12 keys fall away and the pipes shift with them.
        let range = &organ.stops[0].ranks[0];
        assert_eq!(range.first_key, 0);
        assert_eq!(range.first_pipe, 12);
        assert_eq!(range.key_count, 49);
        // Only the Principal rank came along — not the whole organ.
        assert_eq!(organ.ranks.len(), 1);
        assert!(organ.couplers.is_empty());
        // Sample paths absorbed the source root.
        let (attacks, _) = organ.ranks[0].pipes[0].samples().unwrap();
        assert_eq!(attacks[0].path, PathBuf::from("/a/prin.wav"));
    }

    #[test]
    fn borrows_pull_their_donor_rank() {
        let sources = vec![("A".to_string(), source("A", "/a"))];
        let mut definition = def("Reed");
        definition.manuals = vec![manual("Solo", None, None)];
        definition.stops = vec![StopPull {
            from: "A".into(),
            manual: None,
            stop: "Hautbois 8".into(),
            on: "Solo".into(),
            rename: None,
        }];
        let built = assemble(&definition, &sources, Vec::new()).expect("assembles");
        let organ = &built.organ;
        // The Hautbois rank and its donor, nothing else.
        assert_eq!(organ.ranks.len(), 2);
        let PipeSource::Borrowed(target) = &organ.ranks[0].pipes[1].source else {
            panic!("expected a borrow");
        };
        assert_eq!(target.rank, organ.ranks[1].id);
        assert!(organ
            .sounding_pipe(PipeRef {
                rank: organ.ranks[0].id,
                pipe: 1
            })
            .is_some());
        // An undeclared compass wraps what landed: 61 keys from 36.
        assert_eq!(organ.manuals[0].first_midi_note, 36);
        assert_eq!(organ.manuals[0].key_count, 61);
    }

    #[test]
    fn only_referenced_wind_and_boxes_carried() {
        let sources = vec![("A".to_string(), source("A", "/a"))];
        let mut definition = def("Reed");
        definition.manuals = vec![manual("Solo", None, None)];
        definition.stops = vec![StopPull {
            from: "A".into(),
            manual: None,
            stop: "Hautbois 8".into(),
            on: "Solo".into(),
            rename: None,
        }];
        let built = assemble(&definition, &sources, Vec::new()).expect("assembles");
        let organ = &built.organ;
        // Chest 2 only, renumbered to 1; its box only, index 0; the
        // unused box and chest 1 stayed home.
        assert_eq!(organ.windchests.len(), 1);
        assert_eq!(organ.windchests[0].number, 1);
        assert_eq!(organ.windchests[0].enclosures, vec![0]);
        assert_eq!(organ.enclosures.len(), 1);
        assert_eq!(organ.enclosures[0].name, "Swell box");
        assert_eq!(organ.ranks[0].windchest, 1);
    }

    #[test]
    fn division_pull_onto_shared_manual() {
        let sources = vec![
            ("A".to_string(), source("A", "/a")),
            ("B".to_string(), source("B", "/b")),
        ];
        let mut definition = def("One manual to rule them");
        definition.manuals = vec![manual("Great", Some(36), Some(96))];
        definition.divisions = vec![
            DivisionPull {
                from: "A".into(),
                manual: "Great".into(),
                on: Some("Great".into()),
            },
            DivisionPull {
                from: "B".into(),
                manual: "Swell".into(),
                on: Some("Great".into()),
            },
        ];
        let built = assemble(&definition, &sources, Vec::new()).expect("assembles");
        let organ = &built.organ;
        assert_eq!(organ.manuals.len(), 1);
        assert_eq!(organ.stops.len(), 2);
        assert!(organ.stops.iter().all(|s| s.manual == ManualId(0)));
        // A coupler whose Swell wasn't division-pulled must not ride
        // along on either side.
        assert!(organ.couplers.is_empty());
    }

    #[test]
    fn unknown_alias_and_unknown_manual_error() {
        let sources = vec![("A".to_string(), source("A", "/a"))];
        let mut definition = def("Broken");
        definition.stops = vec![StopPull {
            from: "nope".into(),
            manual: None,
            stop: "Principal 8".into(),
            on: "Great".into(),
            rename: None,
        }];
        assert!(assemble(&definition, &sources, Vec::new()).is_err());
        let mut definition = def("Broken too");
        definition.stops = vec![StopPull {
            from: "A".into(),
            manual: None,
            stop: "Principal 8".into(),
            on: "Choir".into(),
            rename: None,
        }];
        assert!(assemble(&definition, &sources, Vec::new()).is_err());
    }

    /// A [[move]] relocates a pulled stop by name after all pulls,
    /// with the ranges re-anchored by pitch onto the new manual.
    #[test]
    fn moves_relocate_stops_after_pulls() {
        let sources = vec![("A".to_string(), source("A", "/a"))];
        let mut definition = def("Rearranged");
        definition.manuals = vec![
            manual("Great", Some(36), Some(96)),
            manual("Solo", Some(48), Some(96)),
        ];
        definition.divisions = vec![
            DivisionPull {
                from: "A".into(),
                manual: "Great".into(),
                on: Some("Great".into()),
            },
            DivisionPull {
                from: "A".into(),
                manual: "Swell".into(),
                on: Some("Great".into()),
            },
        ];
        definition.moves = vec![MoveDef {
            stop: "Hautbois 8".into(),
            from: "Great".into(),
            to: "Solo".into(),
        }];
        let built = assemble(&definition, &sources, Vec::new()).expect("assembles");
        let organ = &built.organ;
        let hautbois = organ.stops.iter().find(|s| s.name == "Hautbois 8").unwrap();
        assert_eq!(hautbois.manual, organ.manuals[1].id);
        // Source keys meant MIDI 36..96; Solo starts at 48 — trimmed
        // and re-anchored, pipes shifted with the cut.
        assert_eq!(hautbois.ranks[0].first_key, 0);
        assert_eq!(hautbois.ranks[0].first_pipe, 12);
        // The Principal stayed home.
        assert_eq!(organ.stops[0].manual, organ.manuals[0].id);
    }

    #[test]
    fn per_manual_tuning_and_coupler_drops_parse() {
        let text = r#"
name = "Two pitches"

[sources]
a = "a.organ"

[[manual]]
name = "Great"

[[manual]]
name = "Positif"
temperament = "meantone"
a4_hz = 415.0

[couplers]
drop = ["Swell to Great"]
"#;
        let definition: Definition = toml::from_str(text).expect("parses");
        assert_eq!(definition.couplers.drop, ["Swell to Great"]);
        let sources = vec![("A".to_string(), source("A", "/a"))];
        let built = assemble(&definition, &sources, Vec::new()).expect("assembles");
        assert_eq!(
            built.manual_tuning,
            [(1, Some("meantone".to_string()), Some(415.0), None)]
        );
        assert_eq!(built.sidecar.couplers.drop, ["Swell to Great"]);
    }

    #[test]
    fn the_doc_example_parses() {
        let text = r#"
name = "Frankenorgan"

[sources]
anne = "../sets/st-anne/demo.organ"
gib = "/sets/giubiasco/giubiasco.organ"

[[manual]]
name = "Great"
low = "C2"
high = "C7"

[[division]]
from = "gib"
manual = "hauptwerk"
on = "Great"

[[stop]]
from = "anne"
stop = "trompette"
on = "Great"
rename = "Trompette royale"

[[couplers.define]]
name = "Great sub"
[[couplers.define.route]]
from = "Great"
to = "Great"
shift = -12

[[midi.input]]
manual = "Great"
device = "KeyLab 61"
channel = 1

[[midi.control]]
device = "KeyLab 61"
trigger = "cc:64"
action = "tremulant"

[tuning]
temperament = "equal"
a4_hz = 415.0
"#;
        let definition: Definition = toml::from_str(text).expect("parses");
        assert_eq!(definition.sources.len(), 2);
        assert_eq!(definition.manuals[0].name, "Great");
        assert_eq!(definition.midi.inputs[0].device, "KeyLab 61");
        assert_eq!(definition.to_sidecar().tuning.a4_hz, 415.0);
        assert_eq!(definition.couplers.define[0].routes[0].shift, -12);
    }
}
