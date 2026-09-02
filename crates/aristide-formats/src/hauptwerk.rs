//! Hauptwerk organ definitions (`.Organ_Hauptwerk_xml`), unencrypted
//! only — the v2-era XML format that free and older commercial sets
//! ship in. `docs/hw-odf-notes.md` is the format reference this reader
//! follows; extend it (with citations) rather than guessing here.
//!
//! A definition is a set of relational tables keyed by numeric IDs:
//! keyboards play divisions, stops on a division sound ranks through
//! `StopRank` rows, ranks hold pipes, pipes hold layers of attack and
//! release samples, and samples name WAV files inside an installation
//! package. This reader walks that graph into the format-neutral
//! [`Organ`], the same target the GrandOrgue reader produces, so
//! nothing downstream knows which format a set came in.
//!
//! Encrypted sets are refused outright: their definition is not XML
//! and their samples are not WAV, and no attempt is ever made to read
//! either (the project's legal boundary — see CLAUDE.md).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};

use aristide_model::units::equal_ladder_hz;
use aristide_model::{
    AttackSample, Coupler, CouplerRoute, CouplerScope, CouplerTarget, Manual, ManualId,
    ManualKind, Organ, Pipe, PipeSource, Rank, RankId, RankRange, ReleaseSample, Stop, StopId,
    Tremulant, TremulantKind, Windchest,
};
use quick_xml::events::Event;
use thiserror::Error;

use crate::LoadResult;

#[derive(Debug, Error)]
pub enum HwError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error(
        "not a Hauptwerk XML organ definition (compressed or encrypted definitions are not \
         supported)"
    )]
    NotXml,
    #[error("XML: {0}")]
    Xml(String),
    #[error("this sample set is encrypted; Aristide does not read encrypted Hauptwerk sets")]
    Encrypted,
    #[error("{0}")]
    Invalid(String),
}

/// Whether a path names a Hauptwerk organ definition.
pub fn is_definition(path: &Path) -> bool {
    path.extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("organ_hauptwerk_xml"))
}

pub fn load(path: &Path) -> Result<LoadResult, HwError> {
    let bytes = std::fs::read(path)?;
    parse(&bytes, package_root(path))
}

/// Where the set's `OrganInstallationPackages/` tree lives. Hauptwerk
/// keeps definitions in `OrganDefinitions/` beside it, but a set
/// unpacked one folder deeper is common, so walk up a few levels
/// before falling back to the definition folder's parent.
fn package_root(definition: &Path) -> PathBuf {
    let start = definition.parent().unwrap_or(Path::new(""));
    let mut dir = start.to_path_buf();
    for _ in 0..4 {
        if dir.join(PACKAGES_DIR).is_dir() {
            return dir;
        }
        match dir.parent() {
            Some(parent) => dir = parent.to_path_buf(),
            None => break,
        }
    }
    start.parent().unwrap_or(start).to_path_buf()
}

const PACKAGES_DIR: &str = "OrganInstallationPackages";

pub fn parse(bytes: &[u8], base_path: PathBuf) -> Result<LoadResult, HwError> {
    sniff_xml(bytes)?;
    let tables = parse_tables(bytes)?;
    Builder::new(&tables, base_path).build()
}

/// An encrypted definition is an opaque blob and a compressed one
/// starts with a zlib/gzip header; only plain XML is read.
fn sniff_xml(bytes: &[u8]) -> Result<(), HwError> {
    let body = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes);
    match body.iter().find(|b| !b.is_ascii_whitespace()) {
        Some(b'<') => Ok(()),
        _ => Err(HwError::NotXml),
    }
}

// ---------------------------------------------------------------------
// Tables

/// One object: column name (always the full name — compacted letters
/// are expanded while parsing) → its text.
type Row = HashMap<String, String>;

#[derive(Default)]
struct Tables {
    by_type: HashMap<String, Vec<Row>>,
}

impl Tables {
    fn rows(&self, object_type: &str) -> &[Row] {
        self.by_type
            .get(object_type)
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }
}

include!("hauptwerk_columns.rs");

/// Expand a compacted column letter (`a`…`z`, `a1`…) to its full name;
/// full names pass through untouched, so uncompacted files need no
/// special case.
fn column_name(object_type: &str, raw: &str) -> String {
    let is_letter = matches!(raw.len(), 1 | 2)
        && raw.as_bytes()[0].is_ascii_lowercase()
        && raw.as_bytes().get(1).is_none_or(u8::is_ascii_digit);
    if is_letter
        && let Some((_, columns)) = COLUMN_NAMES.iter().find(|(name, _)| *name == object_type)
        && let Some((_, full)) = columns.iter().find(|(letter, _)| *letter == raw)
    {
        return (*full).to_string();
    }
    raw.to_string()
}

/// `<Hauptwerk>` → `<ObjectList ObjectType=…>` → one element per
/// object → one element per column. Depth tracks that nesting.
fn parse_tables(bytes: &[u8]) -> Result<Tables, HwError> {
    let mut reader = quick_xml::Reader::from_reader(bytes);
    reader.config_mut().trim_text(true);
    let mut buf = Vec::new();
    let mut tables = Tables::default();
    let mut depth = 0usize;
    let mut object_type = String::new();
    let mut row = Row::new();
    let mut column = String::new();
    let mut text = String::new();
    let xml_err = |e: quick_xml::Error| HwError::Xml(e.to_string());
    loop {
        match reader.read_event_into(&mut buf).map_err(xml_err)? {
            Event::Start(start) => {
                depth += 1;
                match depth {
                    2 => {
                        object_type = start
                            .try_get_attribute("ObjectType")
                            .map_err(|e| HwError::Xml(e.to_string()))?
                            .map(|attr| attr.normalized_value(quick_xml::XmlVersion::Implicit1_0).map(|v| v.into_owned()))
                            .transpose()
                            .map_err(|e| HwError::Xml(e.to_string()))?
                            .unwrap_or_default();
                    }
                    3 => row = Row::new(),
                    4 => {
                        let local = start.local_name();
                        column = column_name(
                            &object_type,
                            std::str::from_utf8(local.as_ref())
                                .map_err(|e| HwError::Xml(e.to_string()))?,
                        );
                        text.clear();
                    }
                    _ => {}
                }
            }
            Event::Empty(start) => {
                if depth == 3 {
                    let local = start.local_name();
                    let name = std::str::from_utf8(local.as_ref())
                        .map_err(|e| HwError::Xml(e.to_string()))?;
                    row.insert(column_name(&object_type, name), String::new());
                }
            }
            Event::Text(t) => {
                if depth == 4 {
                    text.push_str(
                        &t.xml_content(quick_xml::XmlVersion::Implicit1_0)
                            .map_err(|e| HwError::Xml(e.to_string()))?,
                    );
                }
            }
            Event::CData(c) => {
                if depth == 4 {
                    text.push_str(&String::from_utf8_lossy(&c));
                }
            }
            Event::End(_) => {
                match depth {
                    4 => {
                        row.insert(std::mem::take(&mut column), text.trim().to_string());
                    }
                    3 => {
                        tables
                            .by_type
                            .entry(object_type.clone())
                            .or_default()
                            .push(std::mem::take(&mut row));
                    }
                    _ => {}
                }
                depth = depth.saturating_sub(1);
            }
            Event::Eof => break,
            _ => {}
        }
        buf.clear();
    }
    if tables.rows("_General").is_empty() {
        return Err(HwError::Invalid(
            "no _General table: not an organ definition".to_string(),
        ));
    }
    Ok(tables)
}

/// Typed access to a row's text columns. Absent and empty columns are
/// the same thing (compaction drops columns at their default).
trait RowExt {
    fn text(&self, column: &str) -> &str;
    fn float(&self, column: &str) -> Option<f64>;
    fn int(&self, column: &str) -> Option<i64>;
    fn flag(&self, column: &str) -> Option<bool>;
}

impl RowExt for Row {
    fn text(&self, column: &str) -> &str {
        self.get(column).map(String::as_str).unwrap_or("")
    }

    /// Numbers are written the way a C printf spelled them, exponent
    /// notation included (`-2e+1`), so everything parses as float.
    fn float(&self, column: &str) -> Option<f64> {
        self.text(column).trim().parse::<f64>().ok()
    }

    fn int(&self, column: &str) -> Option<i64> {
        self.float(column).map(|v| v.round() as i64)
    }

    fn flag(&self, column: &str) -> Option<bool> {
        match self.text(column).trim() {
            "Y" | "y" => Some(true),
            "N" | "n" => Some(false),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------
// Model building

/// What a rank's pipes share, and so what one [`Windchest`] stands for
/// here: the wind compartment they draw from, the enclosure(s) around
/// them and the tremulant(s) that reach them. Hauptwerk states all
/// three per pipe; a rank whose pipes disagree follows its majority.
#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
struct WindKey {
    compartment: i64,
    enclosures: Vec<u32>,
    tremulants: Vec<u32>,
}

struct Builder<'a> {
    t: &'a Tables,
    base_path: PathBuf,
    warnings: Vec<String>,
    organ_gain_db: f64,
    organ_tuning_cents: f64,
    samples: HashMap<i64, &'a Row>,
    pipes_by_rank: HashMap<i64, Vec<&'a Row>>,
    layers_by_pipe: HashMap<i64, Vec<&'a Row>>,
    attacks_by_layer: HashMap<i64, Vec<&'a Row>>,
    releases_by_layer: HashMap<i64, Vec<&'a Row>>,
    enclosures_of_pipe: HashMap<i64, Vec<u32>>,
    tremulants_of_pipe: HashMap<i64, Vec<u32>>,
    /// Hauptwerk tremulant ID → index into `organ.tremulants`.
    tremulant_index: HashMap<i64, u32>,
    /// Hauptwerk rank ID → the built rank, or `None` once known to be
    /// empty.
    built_ranks: HashMap<i64, Option<RankId>>,
    windchest_numbers: BTreeMap<WindKey, u32>,
    organ: Organ,
    skipped_noise_ranks: HashSet<i64>,
    conditional_attacks_skipped: usize,
    conditional_releases_skipped: usize,
}

impl<'a> Builder<'a> {
    fn new(t: &'a Tables, base_path: PathBuf) -> Self {
        Builder {
            t,
            base_path,
            warnings: Vec::new(),
            organ_gain_db: 0.0,
            organ_tuning_cents: 0.0,
            samples: HashMap::new(),
            pipes_by_rank: HashMap::new(),
            layers_by_pipe: HashMap::new(),
            attacks_by_layer: HashMap::new(),
            releases_by_layer: HashMap::new(),
            enclosures_of_pipe: HashMap::new(),
            tremulants_of_pipe: HashMap::new(),
            tremulant_index: HashMap::new(),
            built_ranks: HashMap::new(),
            windchest_numbers: BTreeMap::new(),
            organ: Organ::default(),
            skipped_noise_ranks: HashSet::new(),
            conditional_attacks_skipped: 0,
            conditional_releases_skipped: 0,
        }
    }

    fn warn(&mut self, message: impl Into<String>) {
        self.warnings.push(message.into());
    }

    fn build(mut self) -> Result<LoadResult, HwError> {
        self.read_general()?;
        self.refuse_encrypted()?;
        self.index_tables();
        self.read_enclosures();
        self.read_tremulants();
        let keyboards = self.read_manuals();
        self.read_stops(&keyboards);
        self.read_couplers(&keyboards);
        self.check_samples()?;
        if self.organ.stops.is_empty() {
            return Err(HwError::Invalid(
                "no playable stops: every stop is either noise-only or on a division no \
                 keyboard plays"
                    .to_string(),
            ));
        }
        let skipped = self
            .pipes_by_rank
            .keys()
            .filter(|rank| !self.built_ranks.contains_key(rank))
            .count()
            + self.skipped_noise_ranks.len();
        if skipped > 0 {
            self.warn(format!(
                "{skipped} noise ranks (blower, key and stop action) skipped: Aristide has \
                 no noise ranks yet"
            ));
        }
        if self.conditional_attacks_skipped + self.conditional_releases_skipped > 0 {
            self.warn(format!(
                "{} attack and {} release samples chosen by a continuous control or key \
                 velocity were skipped",
                self.conditional_attacks_skipped, self.conditional_releases_skipped
            ));
        }
        self.organ.base_path = self.base_path.clone();
        Ok(LoadResult {
            organ: self.organ,
            warnings: self.warnings,
        })
    }

    fn read_general(&mut self) -> Result<(), HwError> {
        let general = &self.t.rows("_General")[0];
        let name = general.text("Identification_Name").trim();
        self.organ.name = if name.is_empty() {
            "Hauptwerk organ".to_string()
        } else {
            name.to_string()
        };
        // Organ-wide trims, folded into every pipe like GO's organ-level
        // Gain/PitchTuning (notes §2).
        self.organ_gain_db = general
            .float("AudioOut_AmplitudeLevelAdjustDecibels")
            .unwrap_or(0.0);
        let base_pitch = general.float("AudioEngine_BasePitchHz").unwrap_or(0.0);
        if base_pitch > 0.0 && (base_pitch - 440.0).abs() > 1e-6 {
            self.organ_tuning_cents = 1200.0 * (base_pitch / 440.0).log2();
        }
        Ok(())
    }

    /// A sample that needs a licence serial is an encrypted one. The
    /// definition may still be plain XML (older sets encrypted only
    /// the audio), so this is checked on top of the XML sniff.
    fn refuse_encrypted(&self) -> Result<(), HwError> {
        let encrypted = self
            .t
            .rows("Sample")
            .iter()
            .any(|s| s.int("LicenceSerialNumRequiredForSampleFile").unwrap_or(0) != 0);
        if encrypted {
            return Err(HwError::Encrypted);
        }
        Ok(())
    }

    fn index_tables(&mut self) {
        for sample in self.t.rows("Sample") {
            if let Some(id) = sample.int("SampleID") {
                self.samples.insert(id, sample);
            }
        }
        for pipe in self.t.rows("Pipe_SoundEngine01") {
            if let Some(rank) = pipe.int("RankID") {
                self.pipes_by_rank.entry(rank).or_default().push(pipe);
            }
        }
        for layer in self.t.rows("Pipe_SoundEngine01_Layer") {
            if let Some(pipe) = layer.int("PipeID") {
                self.layers_by_pipe.entry(pipe).or_default().push(layer);
            }
        }
        for layers in self.layers_by_pipe.values_mut() {
            layers.sort_by_key(|l| (l.int("PipeLayerNumber").unwrap_or(1), l.int("LayerID")));
        }
        for attack in self.t.rows("Pipe_SoundEngine01_AttackSample") {
            if let Some(layer) = attack.int("LayerID") {
                self.attacks_by_layer.entry(layer).or_default().push(attack);
            }
        }
        for release in self.t.rows("Pipe_SoundEngine01_ReleaseSample") {
            if let Some(layer) = release.int("LayerID") {
                self.releases_by_layer.entry(layer).or_default().push(release);
            }
        }
        for list in self
            .attacks_by_layer
            .values_mut()
            .chain(self.releases_by_layer.values_mut())
        {
            list.sort_by_key(|r| r.int("UniqueID"));
        }
    }

    /// Enclosures: Hauptwerk states the closed-shutter attenuation per
    /// pipe; the median of a box's pipes becomes its amplitude floor.
    fn read_enclosures(&mut self) {
        let mut index_of: HashMap<i64, u32> = HashMap::new();
        for enclosure in self.t.rows("Enclosure") {
            let Some(id) = enclosure.int("EnclosureID") else {
                continue;
            };
            let mut closed_db: Vec<f64> = self
                .t
                .rows("EnclosurePipe")
                .iter()
                .filter(|p| p.int("EnclosureID") == Some(id))
                .filter_map(|p| p.float("FiltParamWhenClsd_OverallAttnDb"))
                .collect();
            closed_db.sort_by(|a, b| a.total_cmp(b));
            let amp_minimum_level = closed_db
                .get(closed_db.len() / 2)
                .map(|db| (100.0 * 10f64.powf(db / 20.0)).clamp(0.0, 100.0))
                .unwrap_or(0.0);
            let name = enclosure.text("Name").trim();
            index_of.insert(id, self.organ.enclosures.len() as u32);
            self.organ.enclosures.push(aristide_model::Enclosure {
                name: if name.is_empty() {
                    format!("Enclosure {id}")
                } else {
                    name.to_string()
                },
                amp_minimum_level,
                midi_input_number: None,
                displayed: true,
            });
        }
        for member in self.t.rows("EnclosurePipe") {
            if let (Some(enclosure), Some(pipe)) =
                (member.int("EnclosureID"), member.int("PipeID"))
                && let Some(&index) = index_of.get(&enclosure)
            {
                self.enclosures_of_pipe.entry(pipe).or_default().push(index);
            }
        }
    }

    /// Tremulants: Hauptwerk drives each from recorded modulation
    /// waveforms (one per pipe range) rather than an LFO figure, so the
    /// rate comes from the engaged frequency or the waveform's loop
    /// and the depth from the waveform's amplitude channel (notes §6).
    /// Alternate-rank sets (tremmed re-recordings) switch the kind to
    /// Wave later, when the stop that uses them is read.
    fn read_tremulants(&mut self) {
        let mut waveform_tremulant: HashMap<i64, i64> = HashMap::new();
        for tremulant in self.t.rows("Tremulant") {
            let Some(id) = tremulant.int("TremulantID") else {
                continue;
            };
            let waveforms: Vec<&Row> = self
                .t
                .rows("TremulantWaveform")
                .iter()
                .filter(|w| w.int("TremulantID") == Some(id))
                .collect();
            for waveform in &waveforms {
                if let Some(wid) = waveform.int("TremulantWaveformID") {
                    waveform_tremulant.insert(wid, id);
                }
            }
            let measured = self.measure_waveforms(&waveforms);
            let frequency = [
                tremulant.float("FrequencyWhenEngagedHz"),
                measured.frequency_hz,
                tremulant.float("FrequencyWhenDisengagedHz"),
            ]
            .into_iter()
            .flatten()
            .find(|hz| *hz > 0.1)
            .unwrap_or(5.0);
            let name = tremulant.text("Name").trim();
            let index = self.organ.tremulants.len() as u32;
            self.tremulant_index.insert(id, index);
            self.organ.tremulants.push(Tremulant {
                name: if name.is_empty() {
                    format!("Tremulant {id}")
                } else {
                    name.to_string()
                },
                kind: TremulantKind::Synth {
                    period_ms: 1000.0 / frequency,
                    amp_mod_depth_percent: measured.depth_percent.unwrap_or(15.0),
                    start_rate: rate_percent(tremulant.float("StartRatePercent")),
                    stop_rate: rate_percent(tremulant.float("StopRatePercent")),
                },
            });
        }
        for member in self.t.rows("TremulantWaveformPipe") {
            let (Some(waveform), Some(pipe)) =
                (member.int("TremulantWaveformID"), member.int("PipeID"))
            else {
                continue;
            };
            if let Some(&index) = waveform_tremulant
                .get(&waveform)
                .and_then(|t| self.tremulant_index.get(t))
            {
                let list = self.tremulants_of_pipe.entry(pipe).or_default();
                if !list.contains(&index) {
                    list.push(index);
                }
            }
        }
    }

    /// Read the pitch-and-amplitude waveform files of a tremulant: the
    /// sustain loop of the first is one modulation cycle, and channel 1
    /// (the fundamental's amplitude) peaks at the modulation depth.
    fn measure_waveforms(&self, waveforms: &[&Row]) -> MeasuredTremulant {
        let mut measured = MeasuredTremulant::default();
        let mut peaks = Vec::new();
        for waveform in waveforms {
            let Some(path) = waveform
                .int("PitchAndFundamentalWaveformSampleID")
                .and_then(|id| self.samples.get(&id))
                .and_then(|sample| self.sample_path(sample))
            else {
                continue;
            };
            let Ok(wav) = crate::wav::read(&self.base_path.join(path)) else {
                continue;
            };
            let channels = wav.info.channels.max(1) as usize;
            if measured.frequency_hz.is_none()
                && let Some(cycle) = wav.info.loops.first()
            {
                let frames = cycle.end.saturating_sub(cycle.start) as f64;
                if frames > 1.0 && wav.info.sample_rate > 0 {
                    measured.frequency_hz = Some(wav.info.sample_rate as f64 / frames);
                }
            }
            if channels >= 2 {
                let peak = wav
                    .samples
                    .iter()
                    .skip(1)
                    .step_by(channels)
                    .fold(0f32, |acc, s| acc.max(s.abs()));
                if peak > 0.0 {
                    peaks.push(peak as f64);
                }
            }
        }
        if !peaks.is_empty() {
            let mean = peaks.iter().sum::<f64>() / peaks.len() as f64;
            measured.depth_percent = Some((mean * 100.0).clamp(1.0, 100.0));
        }
        measured
    }

    /// Playable keyboards become manuals: Hauptwerk's assignment code
    /// says which console position each takes (1 = pedal, 2–5 =
    /// manuals I–IV; 6+ are utility keyboards for noises). Divisions
    /// with pipe stops that no keyboard plays get a manual of their own
    /// so their stops stay reachable (through couplers).
    fn read_manuals(&mut self) -> Keyboards {
        let mut keyboards = Keyboards::default();
        let mut playable: Vec<(i64, &Row)> = self
            .t
            .rows("Keyboard")
            .iter()
            .filter_map(|kb| {
                let code = kb.int("DefaultInputOutputKeyboardAsgnCode").unwrap_or(0);
                (1..=5).contains(&code).then_some((code, kb))
            })
            .collect();
        playable.sort_by_key(|(code, kb)| (*code, kb.int("KeyboardID")));
        let mut next_manual = 1u32;
        for (code, keyboard) in playable {
            let Some(keyboard_id) = keyboard.int("KeyboardID") else {
                continue;
            };
            let manual_id = if code == 1 && !keyboards.has_pedal {
                keyboards.has_pedal = true;
                ManualId(0)
            } else {
                next_manual += 1;
                ManualId(next_manual - 1)
            };
            let (first_midi_note, key_count) = self.keyboard_compass(keyboard, keyboard_id);
            let name = keyboard.text("Name").trim();
            self.organ.manuals.push(Manual {
                id: manual_id,
                name: if name.is_empty() {
                    format!("Keyboard {keyboard_id}")
                } else {
                    name.to_string()
                },
                first_midi_note,
                key_count,
                kind: if manual_id.0 == 0 {
                    ManualKind::Pedal
                } else {
                    ManualKind::Manual
                },
                hex: None,
            });
            keyboards.manual_of_keyboard.insert(keyboard_id, manual_id);
            if let Some(division) = self.keyboard_division(keyboard, keyboard_id) {
                keyboards
                    .manual_of_division
                    .entry(division)
                    .or_insert(manual_id);
                keyboards.division_of_keyboard.insert(keyboard_id, division);
            }
        }
        // Floating divisions: pipe stops, no keyboard.
        let mut floating: Vec<i64> = Vec::new();
        for stop in self.t.rows("Stop") {
            let Some(division) = stop.int("DivisionID") else {
                continue;
            };
            if keyboards.manual_of_division.contains_key(&division) || floating.contains(&division)
            {
                continue;
            }
            if !self.pipe_stop_ranks(stop).is_empty() {
                floating.push(division);
            }
        }
        for division_id in floating {
            let division = self
                .t
                .rows("Division")
                .iter()
                .find(|d| d.int("DivisionID") == Some(division_id));
            let name = division
                .map(|d| d.text("Name").trim().to_string())
                .filter(|n| !n.is_empty())
                .unwrap_or_else(|| format!("Division {division_id}"));
            let first = division
                .and_then(|d| d.int("InpGen_MIDINoteNumberOfFirstInput"))
                .unwrap_or(36);
            let count = division
                .and_then(|d| d.int("InpGen_NumberOfInputs"))
                .unwrap_or(61);
            next_manual += 1;
            let manual_id = ManualId(next_manual - 1);
            self.warn(format!(
                "division {name:?} has stops but no keyboard plays it; added as a floating manual"
            ));
            self.organ.manuals.push(Manual {
                id: manual_id,
                name,
                first_midi_note: first.clamp(0, 127) as u8,
                key_count: count.clamp(1, 512) as u16,
                kind: ManualKind::Manual,
                hex: None,
            });
            keyboards.manual_of_division.insert(division_id, manual_id);
        }
        keyboards
    }

    /// A keyboard's compass: generated (`KeyGen_*`) or, when the set
    /// lists its keys one by one, the span of those `KeyboardKey` rows.
    fn keyboard_compass(&mut self, keyboard: &Row, keyboard_id: i64) -> (u8, u16) {
        let generated = keyboard
            .flag("KeyGen_GenerateKeysAutomatically")
            .unwrap_or(true);
        if !generated {
            let notes: Vec<i64> = self
                .t
                .rows("KeyboardKey")
                .iter()
                .filter(|k| k.int("KeyboardID") == Some(keyboard_id))
                .filter_map(|k| k.int("NormalMIDINoteNumber"))
                .collect();
            if let (Some(&low), Some(&high)) = (notes.iter().min(), notes.iter().max()) {
                return (low.clamp(0, 127) as u8, (high - low + 1).clamp(1, 512) as u16);
            }
            self.warn(format!(
                "keyboard {keyboard_id} lists no keys; using its generated compass"
            ));
        }
        let first = keyboard.int("KeyGen_MIDINoteNumberOfFirstKey").unwrap_or(36);
        let count = keyboard.int("KeyGen_NumberOfKeys").unwrap_or(61);
        (first.clamp(0, 127) as u8, count.clamp(1, 512) as u16)
    }

    /// Which division a keyboard plays: its primary-division hint, else
    /// the unconditional, unshifted key action it sends.
    fn keyboard_division(&self, keyboard: &Row, keyboard_id: i64) -> Option<i64> {
        if let Some(division) = keyboard
            .int("Hint_PrimaryAssociatedDivisionID")
            .filter(|d| *d > 0)
        {
            return Some(division);
        }
        self.t
            .rows("KeyAction")
            .iter()
            .filter(|ka| ka.int("SourceKeyboardID") == Some(keyboard_id))
            .filter(|ka| ka.int("ConditionSwitchID").unwrap_or(0) == 0)
            .filter(|ka| ka.int("MIDINoteNumberIncrement").unwrap_or(0) == 0)
            .find_map(|ka| ka.int("DestDivisionID").filter(|d| *d > 0))
    }

    /// The `StopRank` rows through which a stop sounds pipes: normal
    /// action (type 1, effect 1) onto a rank that has pipes. Rows with
    /// other codes are the stop's own action noises. A stop without
    /// any rows falls back to its primary-rank hint (some sets omit
    /// the table).
    fn pipe_stop_ranks(&self, stop: &Row) -> Vec<StopRankLink> {
        let Some(stop_id) = stop.int("StopID") else {
            return Vec::new();
        };
        let rows: Vec<&Row> = self
            .t
            .rows("StopRank")
            .iter()
            .filter(|sr| sr.int("StopID") == Some(stop_id))
            .collect();
        let mut links = Vec::new();
        for row in &rows {
            let action_type = row.int("ActionTypeCode").unwrap_or(1);
            let effect = row.int("ActionEffectCode").unwrap_or(1);
            let Some(rank) = row.int("RankID").filter(|r| *r > 0) else {
                continue;
            };
            if action_type != 1 || effect != 1 {
                continue;
            }
            if !self.pipes_by_rank.contains_key(&rank) {
                continue;
            }
            links.push(StopRankLink {
                rank,
                first_division_note: row
                    .int("MIDINoteNumOfFirstMappedDivisionInputNode")
                    .filter(|n| *n > 0),
                mapped_count: row
                    .int("NumberOfMappedDivisionInputNodes")
                    .filter(|n| *n > 0),
                shift: row.int("MIDINoteNumIncrementFromDivisionToRank").unwrap_or(0),
                alternate_rank: row.int("AlternateRankID").filter(|r| *r > 0),
                alternate_switch: row
                    .int("SwitchIDToSwitchToAlternateRank")
                    .filter(|s| *s > 0),
            });
        }
        if links.is_empty()
            && rows.is_empty()
            && let Some(rank) = stop
                .int("Hint_PrimaryAssociatedRankID")
                .filter(|r| self.pipes_by_rank.contains_key(r))
        {
            links.push(StopRankLink {
                rank,
                first_division_note: None,
                mapped_count: None,
                shift: 0,
                alternate_rank: None,
                alternate_switch: None,
            });
        }
        links
    }

    fn read_stops(&mut self, keyboards: &Keyboards) {
        let mut noise_only = 0usize;
        let mut next_stop = 0u32;
        let stops: Vec<&Row> = self.t.rows("Stop").iter().collect();
        for stop in stops {
            let links = self.pipe_stop_ranks(stop);
            if links.is_empty() {
                noise_only += 1;
                continue;
            }
            let Some(manual_id) = stop
                .int("DivisionID")
                .and_then(|d| keyboards.manual_of_division.get(&d).copied())
            else {
                self.warn(format!(
                    "stop {:?} is on a division no keyboard plays; skipped",
                    stop.text("Name")
                ));
                continue;
            };
            let manual = self
                .organ
                .manuals
                .iter()
                .find(|m| m.id == manual_id)
                .map(|m| (m.first_midi_note as i64, m.key_count as i64))
                .expect("manual exists");
            let mut ranges = Vec::new();
            for link in links {
                let wave_tremulant = link
                    .alternate_rank
                    .map(|alt| self.wave_tremulant_for(link.alternate_switch, alt));
                let Some(rank_id) = self.ensure_rank(link.rank, wave_tremulant) else {
                    continue;
                };
                let rank_first = self.rank_first_note(link.rank);
                if let (Some(alt), Some(_)) = (link.alternate_rank, wave_tremulant) {
                    self.merge_alternate_rank(rank_id, rank_first, alt);
                }
                let (rank_name, rank_len) = {
                    let rank = self.organ.rank(rank_id).expect("rank just built");
                    (rank.name.clone(), rank.pipes.len() as i64)
                };
                let (manual_first, manual_keys) = manual;
                let first_note = link.first_division_note.unwrap_or(manual_first);
                let count = link.mapped_count.unwrap_or(rank_len);
                let low = first_note
                    .max(manual_first)
                    .max(rank_first - link.shift);
                let high = (first_note + count)
                    .min(manual_first + manual_keys)
                    .min(rank_first + rank_len - link.shift);
                if high <= low {
                    self.warn(format!(
                        "stop {:?}: rank {rank_name} maps onto no key of its manual; skipped",
                        stop.text("Name")
                    ));
                    continue;
                }
                ranges.push(RankRange {
                    rank: rank_id,
                    first_key: (low - manual_first) as u16,
                    key_count: (high - low) as u16,
                    first_pipe: (low + link.shift - rank_first) as u16,
                });
            }
            if ranges.is_empty() {
                continue;
            }
            next_stop += 1;
            let name = stop.text("Name").trim();
            self.organ.stops.push(Stop {
                id: StopId(next_stop),
                name: if name.is_empty() {
                    format!("Stop {next_stop}")
                } else {
                    name.to_string()
                },
                manual: manual_id,
                ranks: ranges,
                own_pipes: false,
            });
        }
        // Noise carriers masquerade as stops (blower, coupler and
        // tremulant action noises); they are skipped silently unless
        // they are all there is.
        if noise_only > 0 {
            self.warn(format!(
                "{noise_only} noise-only stops (blower, action noises) skipped"
            ));
        }
    }

    /// The tremulant whose switch selects a stop's alternate (tremmed)
    /// rank, found directly or one switch linkage away; a switch that
    /// belongs to no tremulant gets a Wave tremulant of its own.
    fn wave_tremulant_for(&mut self, switch: Option<i64>, alternate_rank: i64) -> u32 {
        let switches: Vec<i64> = switch
            .into_iter()
            .chain(self.t.rows("SwitchLinkage").iter().filter_map(|link| {
                (switch.is_some() && link.int("DestSwitchID") == switch)
                    .then(|| link.int("SourceSwitchID"))?
            }))
            .collect();
        let found = self.t.rows("Tremulant").iter().find_map(|tremulant| {
            let controlling = tremulant.int("ControllingSwitchID")?;
            switches
                .contains(&controlling)
                .then(|| tremulant.int("TremulantID"))?
        });
        let index = match found.and_then(|id| self.tremulant_index.get(&id).copied()) {
            Some(index) => index,
            None => {
                let name = switch
                    .and_then(|s| {
                        self.t
                            .rows("Switch")
                            .iter()
                            .find(|row| row.int("SwitchID") == Some(s))
                    })
                    .map(|row| row.text("Name").trim().to_string())
                    .filter(|n| !n.is_empty())
                    .unwrap_or_else(|| format!("Tremulant (rank {alternate_rank})"));
                self.organ.tremulants.push(Tremulant {
                    name,
                    kind: TremulantKind::Wave,
                });
                (self.organ.tremulants.len() - 1) as u32
            }
        };
        self.organ.tremulants[index as usize].kind = TremulantKind::Wave;
        index
    }

    /// Lowest MIDI note among a Hauptwerk rank's pipes.
    fn rank_first_note(&self, hw_rank: i64) -> i64 {
        self.pipes_by_rank
            .get(&hw_rank)
            .and_then(|pipes| pipes.iter().filter_map(|p| pipe_note(p)).min())
            .unwrap_or(36)
    }

    /// Build a Hauptwerk rank once. Its pipes become a contiguous run
    /// from the lowest note, gaps filled with silent placeholders so
    /// that key arithmetic stays index = note − first.
    fn ensure_rank(&mut self, hw_rank: i64, wave_tremulant: Option<u32>) -> Option<RankId> {
        if let Some(built) = self.built_ranks.get(&hw_rank) {
            return *built;
        }
        let rank_row = self
            .t
            .rows("Rank")
            .iter()
            .find(|r| r.int("RankID") == Some(hw_rank));
        let mut by_note: BTreeMap<i64, &Row> = BTreeMap::new();
        for pipe in self.pipes_by_rank.get(&hw_rank).cloned().unwrap_or_default() {
            let Some(note) = pipe_note(pipe) else {
                continue;
            };
            by_note.entry(note).or_insert(pipe);
        }
        let name = rank_row
            .map(|r| r.text("Name").trim().to_string())
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| format!("Rank {hw_rank}"));
        let (Some(&first), Some(&last)) = (by_note.keys().next(), by_note.keys().next_back())
        else {
            self.built_ranks.insert(hw_rank, None);
            return None;
        };
        let id = RankId(self.organ.ranks.len() as u32 + 1);
        let mut pipes = Vec::with_capacity((last - first + 1) as usize);
        let mut keys: Vec<WindKey> = Vec::new();
        let mut gaps = 0usize;
        for note in first..=last {
            match by_note.get(&note) {
                Some(pipe) => {
                    let (built, key) = self.read_pipe(pipe, note, wave_tremulant);
                    keys.push(key);
                    pipes.push(built);
                }
                None => {
                    gaps += 1;
                    pipes.push(silent_pipe(note, 8.0));
                }
            }
        }
        if gaps > 0 {
            self.warn(format!(
                "rank {name:?}: {gaps} missing notes between {first} and {last} left silent"
            ));
        }
        // Majority wind key decides the chest; disagreement is noted.
        let mut tally: BTreeMap<&WindKey, usize> = BTreeMap::new();
        for key in &keys {
            *tally.entry(key).or_default() += 1;
        }
        let winner = tally
            .iter()
            .max_by_key(|(_, count)| **count)
            .map(|(key, _)| (*key).clone())
            .expect("at least one pipe");
        if tally.len() > 1 {
            self.warn(format!(
                "rank {name:?}: pipes sit on {} different wind/enclosure/tremulant \
                 combinations; the whole rank follows the majority",
                tally.len()
            ));
        }
        let windchest = self.windchest_number(&winner);
        self.organ.ranks.push(Rank {
            id,
            name,
            windchest,
            velocity_volume: Default::default(),
            pipes,
        });
        self.built_ranks.insert(hw_rank, Some(id));
        Some(id)
    }

    /// One windchest per distinct wind key, numbered in order of first
    /// use, named after the compartment (and enclosure) it stands for.
    fn windchest_number(&mut self, key: &WindKey) -> u32 {
        if let Some(&number) = self.windchest_numbers.get(key) {
            return number;
        }
        let number = self.organ.windchests.len() as u32 + 1;
        let compartment = self
            .t
            .rows("WindCompartment")
            .iter()
            .find(|c| c.int("WindCompartmentID") == Some(key.compartment))
            .map(|c| c.text("Name").trim().to_string())
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| format!("Wind compartment {}", key.compartment));
        let mut name = compartment;
        for &enclosure in &key.enclosures {
            if let Some(e) = self.organ.enclosures.get(enclosure as usize) {
                name = format!("{name} in {}", e.name);
            }
        }
        self.organ.windchests.push(Windchest {
            number,
            name,
            enclosures: key.enclosures.clone(),
            tremulants: key.tremulants.clone(),
        });
        self.windchest_numbers.insert(key.clone(), number);
        number
    }

    /// A pipe from its first (main) layer: the true sounding pitch from
    /// key and harmonic, gain and detuning trims folded with the
    /// organ's, samples from the layer's attack and release rows.
    fn read_pipe(&mut self, pipe: &Row, note: i64, wave_tremulant: Option<u32>) -> (Pipe, WindKey) {
        let pipe_id = pipe.int("PipeID").unwrap_or(-1);
        let harmonic = harmonic_or_unison(pipe.float("Pitch_Tempered_RankBasePitch64ftHarmonicNum"));
        let mut tremulants = self
            .tremulants_of_pipe
            .get(&pipe_id)
            .cloned()
            .unwrap_or_default();
        if let Some(extra) = wave_tremulant
            && !tremulants.contains(&extra)
        {
            tremulants.push(extra);
        }
        tremulants.sort_unstable();
        let mut enclosures = self
            .enclosures_of_pipe
            .get(&pipe_id)
            .cloned()
            .unwrap_or_default();
        enclosures.sort_unstable();
        let key = WindKey {
            compartment: pipe.int("WindSupply_SourceWindCompartmentID").unwrap_or(0),
            enclosures,
            tremulants,
        };
        let mut built = silent_pipe(note, harmonic);
        let Some(layer) = self
            .layers_by_pipe
            .get(&pipe_id)
            .and_then(|layers| layers.first().copied())
        else {
            self.warn(format!("pipe {pipe_id} (note {note}) has no sample layer; silent"));
            return (built, key);
        };
        let layer_id = layer.int("LayerID").unwrap_or(-1);
        built.gain_db = self.organ_gain_db + layer.float("AmpLvl_LevelAdjustDecibels").unwrap_or(0.0);
        built.pitch_tuning_cents =
            self.organ_tuning_cents + layer.float("PitchLvl_DetuningPercentSemitones").unwrap_or(0.0);
        let (attacks, first_sample) = self.read_attacks(layer_id, wave_tremulant.map(|_| false));
        if attacks.is_empty() {
            if self.releases_by_layer.contains_key(&layer_id) {
                // Release-only layers are action noises (key-off sounds).
                self.skipped_noise_ranks
                    .insert(pipe.int("RankID").unwrap_or(-1));
            } else {
                self.warn(format!("pipe {pipe_id} (note {note}) has no attack sample; silent"));
            }
            return (built, key);
        }
        if let Some(sample) = first_sample {
            let (key_number, fraction) = recorded_key(sample);
            built.midi_key_number = key_number;
            built.midi_pitch_fraction_cents = fraction;
        }
        let releases = self.read_releases(layer_id, wave_tremulant.map(|_| false));
        built.source = PipeSource::Sampled { attacks, releases };
        (built, key)
    }

    /// The attack samples of a layer, with Hauptwerk's "highest
    /// velocity this sample answers to" and "least time since the pipe
    /// last closed" turned into the model's lower bounds by ordering
    /// the thresholds (notes §5).
    fn read_attacks(
        &mut self,
        layer_id: i64,
        wave_tremulant: Option<bool>,
    ) -> (Vec<AttackSample>, Option<&'a Row>) {
        let rows: Vec<&Row> = self
            .attacks_by_layer
            .get(&layer_id)
            .cloned()
            .unwrap_or_default();
        let mut picked: Vec<(&Row, &'a Row, PathBuf)> = Vec::new();
        for row in rows {
            if row.int("AttackSelCriteria_HighestCtsCtrlValue").unwrap_or(127) < 127 {
                self.conditional_attacks_skipped += 1;
                continue;
            }
            let Some(sample) = row.int("SampleID").and_then(|id| self.samples.get(&id).copied())
            else {
                continue;
            };
            let Some(path) = self.sample_path(sample) else {
                continue;
            };
            picked.push((row, sample, path));
        }
        let velocities: Vec<i64> = sorted_distinct(
            picked
                .iter()
                .map(|(row, _, _)| row.int("AttackSelCriteria_HighestVelocity").unwrap_or(127)),
        );
        let times: Vec<i64> = sorted_distinct(picked.iter().map(|(row, _, _)| {
            row.int("AttackSelCriteria_MinTimeSincePrevPipeCloseMs")
                .unwrap_or(0)
        }));
        let first_sample = picked.first().map(|(_, sample, _)| *sample);
        let attacks = picked
            .into_iter()
            .map(|(row, _, path)| {
                let highest = row.int("AttackSelCriteria_HighestVelocity").unwrap_or(127);
                let min_velocity = velocities
                    .iter()
                    .filter(|v| **v < highest)
                    .max()
                    .map(|below| (below + 1).clamp(0, 127) as u8)
                    .unwrap_or(0);
                let min_time = row
                    .int("AttackSelCriteria_MinTimeSincePrevPipeCloseMs")
                    .unwrap_or(0);
                let max_time_since_last_release_ms = times
                    .iter()
                    .filter(|t| **t > min_time)
                    .min()
                    .map(|above| (above - 1).max(0) as u32);
                AttackSample {
                    path,
                    loops: Vec::new(),
                    pitch_offset_cents: 0.0,
                    wave_tremulant,
                    min_velocity,
                    max_time_since_last_release_ms,
                    loop_crossfade_ms: row
                        .int("LoopCrossfadeLengthInSrcSampleMs")
                        .unwrap_or(0)
                        .clamp(0, 3000) as u16,
                    attack_start_frame: 0,
                    cue_point_frame: None,
                    release_end_frame: None,
                    release_crossfade_ms: 0,
                }
            })
            .collect();
        (attacks, first_sample)
    }

    /// The release samples of a layer, shortest key-hold bound first;
    /// the unbounded one (`99999` ms in Hauptwerk) last.
    fn read_releases(&mut self, layer_id: i64, wave_tremulant: Option<bool>) -> Vec<ReleaseSample> {
        let rows: Vec<&Row> = self
            .releases_by_layer
            .get(&layer_id)
            .cloned()
            .unwrap_or_default();
        let mut releases = Vec::new();
        for row in rows {
            if row.int("ReleaseSelCriteria_HighestCtsCtrlValue").unwrap_or(127) < 127
                || row.int("ReleaseSelCriteria_HighestVelocity").unwrap_or(127) < 127
            {
                self.conditional_releases_skipped += 1;
                continue;
            }
            let Some(path) = row
                .int("SampleID")
                .and_then(|id| self.samples.get(&id))
                .and_then(|sample| self.sample_path(sample))
            else {
                continue;
            };
            let latest = row
                .int("ReleaseSelCriteria_LatestKeyReleaseTimeMs")
                .unwrap_or(-1);
            releases.push(ReleaseSample {
                path,
                max_key_press_ms: (0..99999).contains(&latest).then_some(latest as u32),
                wave_tremulant,
                cue_point_frame: None,
                release_end_frame: None,
                release_crossfade_ms: row
                    .int("ReleaseCrossfadeLengthMs")
                    .unwrap_or(0)
                    .clamp(0, 3000) as u16,
            });
        }
        releases.sort_by_key(|r| r.max_key_press_ms.map_or(u32::MAX, |ms| ms));
        releases
    }

    /// Fold an alternate (tremmed) rank's recordings into the main
    /// rank's pipes as wave-tremulant variants, note for note.
    fn merge_alternate_rank(&mut self, rank_id: RankId, first: i64, alternate: i64) {
        let alt_pipes: BTreeMap<i64, &Row> = self
            .pipes_by_rank
            .get(&alternate)
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter_map(|p| pipe_note(p).map(|n| (n, p)))
            .collect();
        let rank_index = self
            .organ
            .ranks
            .iter()
            .position(|r| r.id == rank_id)
            .expect("rank exists");
        let mut merged = 0usize;
        for index in 0..self.organ.ranks[rank_index].pipes.len() {
            let note = first + index as i64;
            let alt_layer = alt_pipes
                .get(&note)
                .and_then(|alt| self.layers_by_pipe.get(&alt.int("PipeID").unwrap_or(-1)))
                .and_then(|layers| layers.first())
                .and_then(|layer| layer.int("LayerID"));
            let (mut alt_attacks, mut alt_releases) = match alt_layer {
                Some(layer_id) => (
                    self.read_attacks(layer_id, Some(true)).0,
                    self.read_releases(layer_id, Some(true)),
                ),
                None => (Vec::new(), Vec::new()),
            };
            let pipe = &mut self.organ.ranks[rank_index].pipes[index];
            let PipeSource::Sampled { attacks, releases } = &mut pipe.source else {
                continue;
            };
            if alt_attacks.is_empty() {
                // No tremmed recording for this note: its plain
                // recordings must serve in both tremulant states.
                for attack in attacks.iter_mut() {
                    attack.wave_tremulant = None;
                }
                for release in releases.iter_mut() {
                    release.wave_tremulant = None;
                }
                continue;
            }
            attacks.append(&mut alt_attacks);
            releases.append(&mut alt_releases);
            releases.sort_by_key(|r| r.max_key_press_ms.map_or(u32::MAX, |ms| ms));
            merged += 1;
        }
        if merged == 0 {
            self.warn(format!(
                "alternate rank {alternate} shares no notes with its main rank; ignored"
            ));
        }
        self.built_ranks.insert(alternate, None);
    }

    /// Couplers are conditional key actions: keyboard → division or
    /// keyboard, shifted by an increment, live while a switch is on.
    /// Actions sharing one switch are one coupler with several routes.
    fn read_couplers(&mut self, keyboards: &Keyboards) {
        let mut grouped: BTreeMap<i64, (String, Vec<CouplerRoute>)> = BTreeMap::new();
        let actions: Vec<&Row> = self.t.rows("KeyAction").iter().collect();
        for action in actions {
            let Some(source) = action
                .int("SourceKeyboardID")
                .and_then(|k| keyboards.manual_of_keyboard.get(&k).copied())
            else {
                continue;
            };
            let dest = match (
                action.int("DestKeyboardID").filter(|k| *k > 0),
                action.int("DestDivisionID").filter(|d| *d > 0),
            ) {
                (Some(keyboard), _) => keyboards.manual_of_keyboard.get(&keyboard).copied(),
                (None, Some(division)) => keyboards.manual_of_division.get(&division).copied(),
                (None, None) => None,
            };
            let Some(dest) = dest else {
                continue;
            };
            let shift = action.int("MIDINoteNumberIncrement").unwrap_or(0);
            let condition = action.int("ConditionSwitchID").unwrap_or(0);
            if condition == 0 && (source == dest || {
                // The unison action onto the keyboard's own division.
                action
                    .int("SourceKeyboardID")
                    .and_then(|k| keyboards.division_of_keyboard.get(&k))
                    .and_then(|d| keyboards.manual_of_division.get(d))
                    == Some(&dest)
                    && shift == 0
            }) {
                continue;
            }
            let action_type = action.int("ActionTypeCode").unwrap_or(1);
            let effect = action.int("ActionEffectCode").unwrap_or(1);
            let name = action.text("Name").trim().to_string();
            if action_type != 1 || effect != 1 {
                self.warn(format!(
                    "key action {name:?} uses action type {action_type}/effect {effect} \
                     (pizzicato or reiteration); treated as a plain coupler"
                ));
            }
            if action.flag("ConditionSwitchLinkIfEngaged") == Some(false) {
                self.warn(format!(
                    "coupler {name:?} is active while its switch is OFF in Hauptwerk; \
                     Aristide engages it the normal way round"
                ));
            }
            let (source_first, source_keys) = self
                .organ
                .manuals
                .iter()
                .find(|m| m.id == source)
                .map(|m| (m.first_midi_note as i64, m.key_count as i64))
                .unwrap_or((0, 128));
            let first = action.int("MIDINoteNumOfFirstSourceKey").unwrap_or(0);
            let count = action.int("NumberOfKeys").unwrap_or(0);
            let (low_key, high_key) = if first > 0 && count > 0 {
                let last = first + count - 1;
                (
                    (first > source_first).then_some(first.clamp(0, 127) as u8),
                    (last < source_first + source_keys - 1).then_some(last.clamp(0, 127) as u8),
                )
            } else {
                (None, None)
            };
            let route = CouplerRoute {
                from_manual: source,
                low_key,
                high_key,
                unison_off: false,
                scope: CouplerScope::AllKeys,
                target: Some(CouplerTarget {
                    manual: dest,
                    key_shift: shift.clamp(-127, 127) as i16,
                    repitch: None,
                    own_pipes: false,
                }),
            };
            if condition == 0 {
                self.warn(format!(
                    "key action {name:?} is permanently engaged in Hauptwerk; listed as a \
                     coupler to engage"
                ));
                self.organ.couplers.push(Coupler {
                    name: if name.is_empty() {
                        "Coupler".to_string()
                    } else {
                        name
                    },
                    routes: vec![route],
                });
                continue;
            }
            let entry = grouped.entry(condition).or_insert_with(|| {
                let switch_name = self
                    .t
                    .rows("Switch")
                    .iter()
                    .find(|s| s.int("SwitchID") == Some(condition))
                    .map(|s| s.text("Name").trim().to_string())
                    .filter(|n| !n.is_empty());
                (
                    if name.is_empty() {
                        switch_name.unwrap_or_else(|| format!("Coupler {condition}"))
                    } else {
                        name.clone()
                    },
                    Vec::new(),
                )
            });
            entry.1.push(route);
        }
        for (_, (name, routes)) in grouped {
            self.organ.couplers.push(Coupler { name, routes });
        }
    }

    /// `OrganInstallationPackages/<id, six digits>/<file>`, forward
    /// slashes, relative to the package root.
    fn sample_path(&self, sample: &Row) -> Option<PathBuf> {
        let package = sample.int("InstallationPackageID")?;
        let file = sample.text("SampleFilename").trim();
        if file.is_empty() {
            return None;
        }
        Some(PathBuf::from(format!(
            "{PACKAGES_DIR}/{package:06}/{}",
            file.replace('\\', "/")
        )))
    }

    /// The package must be there and its audio must be WAV: a
    /// definition can be plain XML while the samples are encrypted.
    fn check_samples(&mut self) -> Result<(), HwError> {
        let mut packages: BTreeMap<i64, String> = BTreeMap::new();
        for sample in self.t.rows("Sample") {
            if let Some(id) = sample.int("InstallationPackageID") {
                packages.entry(id).or_default();
            }
        }
        for package in self.t.rows("RequiredInstallationPackage") {
            if let Some(id) = package.int("InstallationPackageID")
                && let Some(name) = packages.get_mut(&id)
            {
                *name = package.text("Name").trim().to_string();
            }
        }
        let mut first_attack: Option<PathBuf> = None;
        for rank in &self.organ.ranks {
            for pipe in &rank.pipes {
                if let Some((attacks, _)) = pipe.samples()
                    && let Some(attack) = attacks.first()
                {
                    first_attack = Some(attack.path.clone());
                    break;
                }
            }
            if first_attack.is_some() {
                break;
            }
        }
        let Some(first_attack) = first_attack else {
            return Ok(());
        };
        let full = self.base_path.join(&first_attack);
        if !full.is_file() {
            let missing: Vec<String> = packages
                .iter()
                .filter(|(id, _)| {
                    !self
                        .base_path
                        .join(format!("{PACKAGES_DIR}/{id:06}"))
                        .is_dir()
                })
                .map(|(id, name)| format!("{id:06} ({name})"))
                .collect();
            if !missing.is_empty() {
                return Err(HwError::Invalid(format!(
                    "installation package(s) {} not found under {}",
                    missing.join(", "),
                    self.base_path.join(PACKAGES_DIR).display()
                )));
            }
            self.warn(format!(
                "sample {} is missing; the set may be incomplete",
                first_attack.display()
            ));
            return Ok(());
        }
        let mut magic = [0u8; 4];
        if let Ok(mut file) = std::fs::File::open(&full) {
            use std::io::Read;
            if file.read_exact(&mut magic).is_ok() && &magic != b"RIFF" {
                return Err(HwError::Encrypted);
            }
        }
        Ok(())
    }
}

#[derive(Default)]
struct MeasuredTremulant {
    frequency_hz: Option<f64>,
    depth_percent: Option<f64>,
}

#[derive(Default)]
struct Keyboards {
    has_pedal: bool,
    manual_of_keyboard: HashMap<i64, ManualId>,
    manual_of_division: HashMap<i64, ManualId>,
    division_of_keyboard: HashMap<i64, i64>,
}

struct StopRankLink {
    rank: i64,
    first_division_note: Option<i64>,
    mapped_count: Option<i64>,
    shift: i64,
    alternate_rank: Option<i64>,
    alternate_switch: Option<i64>,
}

/// A pipe's key; Hauptwerk leaves it out when it is middle C (60).
fn pipe_note(pipe: &Row) -> Option<i64> {
    Some(pipe.int("NormalMIDINoteNumber").unwrap_or(60))
}

/// Harmonic number on the 64′ ladder (8 = unison, 16 = 4′, 24 = 2⅔′);
/// absent or zero means unison.
fn harmonic_or_unison(value: Option<f64>) -> f64 {
    match value {
        Some(h) if h > 0.0 => h,
        _ => 8.0,
    }
}

fn silent_pipe(note: i64, harmonic: f64) -> Pipe {
    Pipe {
        nominal_frequency_hz: equal_ladder_hz(note as f64) * (harmonic / 8.0),
        pitch_tuning_cents: 0.0,
        pitch_correction_cents: 0.0,
        gain_db: 0.0,
        midi_key_number: None,
        midi_pitch_fraction_cents: None,
        accepts_retuning: true,
        source: PipeSource::Silent,
    }
}

/// What a sample claims about its recorded pitch, as the model's key +
/// fraction. Method 3 states a key on a harmonic ladder, method 4 an
/// exact frequency; anything else defers to the file's own `smpl`
/// chunk (notes §4).
fn recorded_key(sample: &Row) -> (Option<u8>, Option<f64>) {
    let semitones = match sample.int("Pitch_SpecificationMethodCode").unwrap_or(0) {
        3 => {
            let Some(note) = sample.int("Pitch_NormalMIDINoteNumber").filter(|n| *n > 0) else {
                return (None, None);
            };
            let harmonic = harmonic_or_unison(sample.float("Pitch_RankBasePitch64ftHarmonicNum"));
            note as f64 + 12.0 * (harmonic / 8.0).log2()
        }
        4 => {
            let Some(hz) = sample.float("Pitch_ExactSamplePitch").filter(|hz| *hz > 0.0) else {
                return (None, None);
            };
            69.0 + 12.0 * (hz / 440.0).log2()
        }
        _ => return (None, None),
    };
    let key = semitones.floor();
    if !(0.0..=127.0).contains(&key) {
        return (None, None);
    }
    (Some(key as u8), Some((semitones - key) * 100.0))
}

fn rate_percent(value: Option<f64>) -> u32 {
    value
        .map(|v| v.round().clamp(1.0, 100.0) as u32)
        .unwrap_or(8)
}

fn sorted_distinct(values: impl Iterator<Item = i64>) -> Vec<i64> {
    let mut out: Vec<i64> = values.collect();
    out.sort_unstable();
    out.dedup();
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use aristide_model::PipeSource;

    /// A small compacted definition: two keyboards, two pipe stops
    /// (one shifted onto a sub-range), noise carriers, a tremulant, an
    /// enclosure. Letter columns on some tables, full names on others.
    const DEFINITION: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<Hauptwerk FileFormat="Organ" FileFormatVersion="4.20014">
<ObjectList ObjectType="_General">
 <_General>
  <Identification_Name>Testorgel</Identification_Name>
  <Control_FileIsCompacted_AlwaysSetThisToNIfEditingManually>Y</Control_FileIsCompacted_AlwaysSetThisToNIfEditingManually>
  <AudioOut_AmplitudeLevelAdjustDecibels>-6</AudioOut_AmplitudeLevelAdjustDecibels>
  <AudioEngine_BasePitchHz>415</AudioEngine_BasePitchHz>
 </_General>
</ObjectList>
<ObjectList ObjectType="Division">
 <Division><a>1</a><b>Pedal</b></Division>
 <Division><a>2</a><b>Great</b></Division>
 <Division><a>7</a><b>Noises</b></Division>
</ObjectList>
<ObjectList ObjectType="Keyboard">
 <Keyboard><a>1</a><b>Pedal</b><d>1</d><x>1</x><h>30</h><i>36</i></Keyboard>
 <Keyboard><a>2</a><b>Great</b><d>2</d><x>2</x><h>54</h><i>36</i></Keyboard>
 <Keyboard><a>7</a><b>Noise</b><d>7</d><x>7</x><h>61</h><i>36</i></Keyboard>
 <Keyboard><a>66</a><b>Display</b><x>2</x><h>54</h><i>36</i></Keyboard>
</ObjectList>
<ObjectList ObjectType="KeyAction">
 <KeyAction><a>1</a><d>1</d><e>Ped unison</e></KeyAction>
 <KeyAction><a>2</a><d>2</d><e>Gt unison</e></KeyAction>
 <KeyAction><a>66</a><c>2</c><e>Display</e></KeyAction>
 <KeyAction><a>1</a><d>2</d><e>Gt/Ped</e><f>500</f><g>Y</g><l>36</l><m>30</m></KeyAction>
 <KeyAction><a>2</a><c>2</c><e>Gt octave</e><f>501</f><g>Y</g><n>12</n><l>48</l><m>24</m></KeyAction>
</ObjectList>
<ObjectList ObjectType="Switch">
 <Switch><a>500</a><b>Great to Pedal</b></Switch>
 <Switch><a>501</a><b>Great octave</b></Switch>
</ObjectList>
<ObjectList ObjectType="Stop">
 <Stop><a>10</a><b>Principal 8</b><c>2</c></Stop>
 <Stop><a>11</a><b>Octave 4</b><c>2</c></Stop>
 <Stop><a>12</a><b>Blower</b><c>7</c></Stop>
 <Stop><a>13</a><b>Hinted</b><c>1</c><f>1</f></Stop>
</ObjectList>
<ObjectList ObjectType="StopRank">
 <StopRank><a>10</a><b>P8</b><d>1</d><f>1</f><g>1</g></StopRank>
 <StopRank><a>10</a><b>P8 noise</b><d>90</d><f>21</f><g>2</g></StopRank>
 <StopRank><a>11</a><b>O4</b><d>2</d><f>1</f><g>1</g><h>48</h><i>12</i><j>-12</j></StopRank>
 <StopRank><a>12</a><b>Blower</b><d>91</d><f>21</f><g>1</g></StopRank>
</ObjectList>
<ObjectList ObjectType="Rank">
 <Rank><a>1</a><b>Principal 8</b></Rank>
 <Rank><a>2</a><b>Octave 4</b></Rank>
 <Rank><a>90</a><b>Noises: stop</b></Rank>
 <Rank><a>91</a><b>Noises: blower</b></Rank>
</ObjectList>
<ObjectList ObjectType="WindCompartment">
 <WindCompartment><a>3</a><b>Great chest</b></WindCompartment>
</ObjectList>
<ObjectList ObjectType="Enclosure">
 <Enclosure><a>1</a><b>Swell box</b></Enclosure>
</ObjectList>
<ObjectList ObjectType="EnclosurePipe">
 <EnclosurePipe><a>1</a><b>201</b><c>-20</c></EnclosurePipe>
 <EnclosurePipe><a>1</a><b>202</b><c>-20</c></EnclosurePipe>
</ObjectList>
<ObjectList ObjectType="Tremulant">
 <Tremulant><a>1</a><b>Tremulant</b><c>600</c><d>5</d><f>33</f><g>8</g></Tremulant>
</ObjectList>
<ObjectList ObjectType="TremulantWaveform">
 <TremulantWaveform><a>1</a><c>1</c></TremulantWaveform>
</ObjectList>
<ObjectList ObjectType="TremulantWaveformPipe">
 <TremulantWaveformPipe><a>101</a><b>1</b></TremulantWaveformPipe>
 <TremulantWaveformPipe><a>103</a><b>1</b></TremulantWaveformPipe>
</ObjectList>
<ObjectList ObjectType="Pipe_SoundEngine01">
 <Pipe_SoundEngine01><a>101</a><b>1</b><d>36</d><r>3</r></Pipe_SoundEngine01>
 <Pipe_SoundEngine01><a>103</a><b>1</b><d>38</d><r>3</r></Pipe_SoundEngine01>
 <Pipe_SoundEngine01><a>201</a><b>2</b><d>36</d><f>16</f><r>3</r></Pipe_SoundEngine01>
 <Pipe_SoundEngine01><a>202</a><b>2</b><d>37</d><f>16</f><r>3</r></Pipe_SoundEngine01>
 <Pipe_SoundEngine01><a>901</a><b>90</b><d>36</d><r>3</r></Pipe_SoundEngine01>
 <Pipe_SoundEngine01><a>911</a><b>91</b><d>36</d><r>3</r></Pipe_SoundEngine01>
</ObjectList>
<ObjectList ObjectType="Pipe_SoundEngine01_Layer">
 <Pipe_SoundEngine01_Layer><a>101</a><b>101</b><h>2</h></Pipe_SoundEngine01_Layer>
 <Pipe_SoundEngine01_Layer><a>103</a><b>103</b></Pipe_SoundEngine01_Layer>
 <Pipe_SoundEngine01_Layer><a>201</a><b>201</b></Pipe_SoundEngine01_Layer>
 <Pipe_SoundEngine01_Layer><a>202</a><b>202</b></Pipe_SoundEngine01_Layer>
 <Pipe_SoundEngine01_Layer><a>901</a><b>901</b></Pipe_SoundEngine01_Layer>
 <Pipe_SoundEngine01_Layer><a>911</a><b>911</b></Pipe_SoundEngine01_Layer>
</ObjectList>
<ObjectList ObjectType="Pipe_SoundEngine01_AttackSample">
 <Pipe_SoundEngine01_AttackSample><a>1</a><b>101</b><c>1</c><h>63</h><k>5</k></Pipe_SoundEngine01_AttackSample>
 <Pipe_SoundEngine01_AttackSample><a>2</a><b>101</b><c>2</c></Pipe_SoundEngine01_AttackSample>
 <Pipe_SoundEngine01_AttackSample><a>3</a><b>103</b><c>3</c></Pipe_SoundEngine01_AttackSample>
 <Pipe_SoundEngine01_AttackSample><a>4</a><b>201</b><c>4</c></Pipe_SoundEngine01_AttackSample>
 <Pipe_SoundEngine01_AttackSample><a>5</a><b>202</b><c>5</c></Pipe_SoundEngine01_AttackSample>
 <Pipe_SoundEngine01_AttackSample><a>6</a><b>901</b><c>6</c></Pipe_SoundEngine01_AttackSample>
 <Pipe_SoundEngine01_AttackSample><a>7</a><b>911</b><c>7</c></Pipe_SoundEngine01_AttackSample>
</ObjectList>
<ObjectList ObjectType="Pipe_SoundEngine01_ReleaseSample">
 <Pipe_SoundEngine01_ReleaseSample><a>1</a><b>101</b><c>8</c><q>99999</q><n>45</n></Pipe_SoundEngine01_ReleaseSample>
 <Pipe_SoundEngine01_ReleaseSample><a>2</a><b>101</b><c>9</c><q>150</q></Pipe_SoundEngine01_ReleaseSample>
</ObjectList>
<ObjectList ObjectType="Sample">
 <Sample><a>1</a><b>7</b><c>P8\036-c.wav</c></Sample>
 <Sample><a>2</a><b>7</b><c>P8/036-c-loud.wav</c></Sample>
 <Sample><a>3</a><b>7</b><c>P8/038-d.wav</c><d>4</d><g>261.6256</g></Sample>
 <Sample><a>4</a><b>7</b><c>O4/036-c.wav</c><d>3</d><e>16</e><f>36</f></Sample>
 <Sample><a>5</a><b>7</b><c>O4/037-c#.wav</c></Sample>
 <Sample><a>6</a><b>7</b><c>noise/stop.wav</c></Sample>
 <Sample><a>7</a><b>7</b><c>noise/blower.wav</c></Sample>
 <Sample><a>8</a><b>7</b><c>P8/L/036-c.wav</c></Sample>
 <Sample><a>9</a><b>7</b><c>P8/S/036-c.wav</c></Sample>
</ObjectList>
</Hauptwerk>
"#;

    struct Fixture {
        root: PathBuf,
    }

    impl Fixture {
        fn new(tag: &str) -> Self {
            let root = std::env::temp_dir().join(format!(
                "aristide-hw-{tag}-{}",
                std::process::id()
            ));
            let _ = std::fs::remove_dir_all(&root);
            let package = root.join("OrganInstallationPackages/000007/P8");
            std::fs::create_dir_all(&package).expect("mkdir");
            std::fs::write(package.join("036-c.wav"), b"RIFF\0\0\0\0WAVE").expect("write");
            Fixture { root }
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    fn load_fixture(tag: &str) -> (Fixture, LoadResult) {
        let fixture = Fixture::new(tag);
        let result = parse(DEFINITION.as_bytes(), fixture.root.clone()).expect("loads");
        (fixture, result)
    }

    #[test]
    fn compacted_columns_expand_to_their_names() {
        assert_eq!(column_name("Keyboard", "x"), "Hint_PrimaryAssociatedDivisionID");
        assert_eq!(
            column_name("Pipe_SoundEngine01_Layer", "o1"),
            "AudioOut_OptimalChannelFormatCode"
        );
        assert_eq!(column_name("Keyboard", "Name"), "Name");
        assert_eq!(column_name("NoSuchTable", "b"), "b");
    }

    #[test]
    fn playable_keyboards_become_manuals() {
        let (_fixture, result) = load_fixture("manuals");
        let organ = &result.organ;
        assert_eq!(organ.name, "Testorgel");
        let names: Vec<(u32, &str, u8, u16)> = organ
            .manuals
            .iter()
            .map(|m| (m.id.0, m.name.as_str(), m.first_midi_note, m.key_count))
            .collect();
        assert_eq!(names, vec![(0, "Pedal", 36, 30), (1, "Great", 36, 54)]);
        assert_eq!(organ.manuals[0].kind, ManualKind::Pedal);
        assert_eq!(organ.manuals[1].kind, ManualKind::Manual);
    }

    #[test]
    fn stops_map_ranks_onto_keys_with_shift_and_range() {
        let (_fixture, result) = load_fixture("stops");
        let organ = &result.organ;
        let names: Vec<&str> = organ.stops.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, vec!["Principal 8", "Octave 4", "Hinted"]);
        let principal = &organ.stops[0];
        assert_eq!(principal.manual, ManualId(1));
        assert_eq!(principal.ranks.len(), 1);
        assert_eq!(
            (principal.ranks[0].first_key, principal.ranks[0].key_count, principal.ranks[0].first_pipe),
            (0, 3, 0),
            "rank 36..38 sits under keys 36..38 of a manual from 36"
        );
        // Division keys 48..59 sound rank notes 36..47, of which the
        // rank has only 36 and 37: two keys, from key index 12.
        let octave = &organ.stops[1];
        assert_eq!(
            (octave.ranks[0].first_key, octave.ranks[0].key_count, octave.ranks[0].first_pipe),
            (12, 2, 0)
        );
        // A stop with no StopRank rows follows its primary-rank hint.
        assert_eq!(organ.stops[2].manual, ManualId(0));
        assert_eq!(organ.stops[2].ranks[0].rank, principal.ranks[0].rank);
    }

    #[test]
    fn ranks_fill_gaps_and_fold_harmonics() {
        let (_fixture, result) = load_fixture("ranks");
        let organ = &result.organ;
        assert_eq!(organ.ranks.len(), 2, "noise ranks never built");
        let principal = &organ.ranks[0];
        assert_eq!(principal.pipes.len(), 3);
        assert!(matches!(principal.pipes[1].source, PipeSource::Silent));
        assert!(result.warnings.iter().any(|w| w.contains("missing notes")));
        let octave = &organ.ranks[1];
        assert!(
            (octave.pipes[0].nominal_frequency_hz - equal_ladder_hz(36.0) * 2.0).abs() < 1e-9,
            "harmonic 16 = an octave above the key"
        );
        let organ_tuning = 1200.0 * (415f64 / 440.0).log2();
        assert!((principal.pipes[0].pitch_tuning_cents - organ_tuning).abs() < 1e-9);
        assert!((principal.pipes[0].gain_db - (-6.0 + 2.0)).abs() < 1e-9);
        assert!((principal.pipes[2].gain_db + 6.0).abs() < 1e-9);
    }

    #[test]
    fn attack_and_release_thresholds_become_bounds() {
        let (_fixture, result) = load_fixture("samples");
        let pipe = &result.organ.ranks[0].pipes[0];
        let (attacks, releases) = pipe.samples().expect("sampled");
        assert_eq!(attacks.len(), 2);
        assert_eq!(
            attacks[0].path,
            PathBuf::from("OrganInstallationPackages/000007/P8/036-c.wav"),
            "backslashes normalised, package zero-padded"
        );
        assert_eq!(attacks[0].min_velocity, 0, "answers up to velocity 63");
        assert_eq!(attacks[1].min_velocity, 64, "the louder attack takes over above");
        assert_eq!(attacks[0].loop_crossfade_ms, 5);
        assert_eq!(
            releases.iter().map(|r| r.max_key_press_ms).collect::<Vec<_>>(),
            vec![Some(150), None],
            "shortest hold bound first, the unbounded 99999 last"
        );
        assert_eq!(releases[1].release_crossfade_ms, 45);
    }

    #[test]
    fn declared_sample_pitches_become_keys() {
        let (_fixture, result) = load_fixture("pitch");
        let organ = &result.organ;
        let by_smpl = &organ.ranks[0].pipes[0];
        assert_eq!(by_smpl.midi_key_number, None, "no declaration: the file's smpl chunk");
        let exact = &organ.ranks[0].pipes[2];
        assert_eq!(exact.midi_key_number, Some(60));
        assert!(exact.midi_pitch_fraction_cents.unwrap() < 0.01);
        let on_ladder = &organ.ranks[1].pipes[0];
        assert_eq!(on_ladder.midi_key_number, Some(48), "note 36 on the 4' ladder");
        assert!(on_ladder.midi_pitch_fraction_cents.unwrap().abs() < 1e-9);
    }

    #[test]
    fn conditional_key_actions_become_couplers() {
        let (_fixture, result) = load_fixture("couplers");
        let couplers = &result.organ.couplers;
        assert_eq!(couplers.len(), 2);
        assert_eq!(couplers[0].name, "Gt/Ped");
        let route = &couplers[0].routes[0];
        assert_eq!(route.from_manual, ManualId(0));
        assert_eq!(route.target.as_ref().unwrap().manual, ManualId(1));
        assert_eq!(route.target.as_ref().unwrap().key_shift, 0);
        assert_eq!((route.low_key, route.high_key), (None, None), "whole compass: no bounds");
        let octave = &couplers[1].routes[0];
        assert_eq!(octave.target.as_ref().unwrap().key_shift, 12);
        assert_eq!((octave.low_key, octave.high_key), (Some(48), Some(71)));
    }

    #[test]
    fn wind_enclosure_and_tremulant_membership_make_windchests() {
        let (_fixture, result) = load_fixture("wind");
        let organ = &result.organ;
        assert_eq!(organ.enclosures.len(), 1);
        assert!((organ.enclosures[0].amp_minimum_level - 10.0).abs() < 1e-6, "-20 dB closed");
        assert_eq!(organ.tremulants.len(), 1);
        assert!(matches!(
            organ.tremulants[0].kind,
            TremulantKind::Synth { period_ms, start_rate: 33, stop_rate: 8, .. }
                if (period_ms - 200.0).abs() < 1e-9
        ));
        let principal_chest = organ.windchests.iter().find(|w| w.number == organ.ranks[0].windchest).unwrap();
        assert_eq!(principal_chest.tremulants, vec![0]);
        assert!(principal_chest.enclosures.is_empty());
        assert_eq!(principal_chest.name, "Great chest");
        let octave_chest = organ.windchests.iter().find(|w| w.number == organ.ranks[1].windchest).unwrap();
        assert_eq!(octave_chest.enclosures, vec![0]);
        assert_eq!(octave_chest.name, "Great chest in Swell box");
        assert_ne!(principal_chest.number, octave_chest.number);
    }

    #[test]
    fn noise_carriers_are_counted_not_loaded() {
        let (_fixture, result) = load_fixture("noise");
        assert!(result.warnings.iter().any(|w| w.contains("noise-only stops")));
        assert!(result.warnings.iter().any(|w| w.contains("2 noise ranks")), "{:?}", result.warnings);
    }

    #[test]
    fn encrypted_and_foreign_files_are_refused() {
        let fixture = Fixture::new("refuse");
        assert!(matches!(parse(b"\x1f\x8b\x08blob", fixture.root.clone()), Err(HwError::NotXml)));
        let licensed = DEFINITION.replace(
            "<Sample><a>1</a><b>7</b>",
            "<Sample><a>1</a><b>7</b><h>12345</h>",
        );
        assert!(matches!(
            parse(licensed.as_bytes(), fixture.root.clone()),
            Err(HwError::Encrypted)
        ));
        std::fs::write(
            fixture.root.join("OrganInstallationPackages/000007/P8/036-c.wav"),
            b"not a wav at all",
        )
        .expect("write");
        assert!(matches!(
            parse(DEFINITION.as_bytes(), fixture.root.clone()),
            Err(HwError::Encrypted)
        ));
    }

    #[test]
    fn missing_packages_name_themselves() {
        let root = std::env::temp_dir().join(format!("aristide-hw-nopkg-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let err = parse(DEFINITION.as_bytes(), root).expect_err("no package");
        assert!(err.to_string().contains("000007"), "{err}");
    }

    /// The AVO Solignac set (gitignored, see CLAUDE.md); skipped when
    /// absent.
    #[test]
    fn solignac_loads_as_its_definition_says() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../testsets/avo-solignac/OrganDefinitions/Solignac orig.Organ_Hauptwerk_xml");
        if !path.is_file() {
            eprintln!("skipping: Solignac fixture not present");
            return;
        }
        let result = load(&path).expect("loads");
        let organ = &result.organ;
        assert_eq!(organ.name, "Solignac orig");
        assert_eq!(organ.manuals.len(), 2);
        assert_eq!(organ.manuals[0].key_count, 27);
        assert_eq!(organ.manuals[1].key_count, 54);
        assert_eq!(organ.stops.len(), 8);
        assert_eq!(organ.ranks.len(), 8);
        assert_eq!(organ.couplers.len(), 1);
        assert_eq!(organ.couplers[0].name, "Hw. Ped.8");
        assert_eq!(organ.tremulants.len(), 1);
        let sampled = organ
            .ranks
            .iter()
            .flat_map(|r| &r.pipes)
            .filter(|p| p.samples().is_some())
            .count();
        assert_eq!(sampled, 407);
        // The mixture's breaks: harmonics differ within one rank.
        let sesquialtera = organ.ranks.iter().find(|r| r.name.contains("Sesquialtera")).unwrap();
        let mut ratios: Vec<i64> = sesquialtera
            .pipes
            .iter()
            .enumerate()
            .map(|(i, p)| (p.nominal_frequency_hz / equal_ladder_hz(36.0 + i as f64)).round() as i64)
            .collect();
        ratios.sort_unstable();
        ratios.dedup();
        assert_eq!(ratios, vec![3, 6], "a twelfth and a nineteenth, per pipe");
        let TremulantKind::Synth { period_ms, amp_mod_depth_percent, .. } = organ.tremulants[0].kind
        else {
            panic!("waveform tremulant is synth-kind");
        };
        assert!((100.0..250.0).contains(&period_ms), "{period_ms}");
        assert!(amp_mod_depth_percent > 1.0 && amp_mod_depth_percent < 50.0);
        assert!(organ.windchests.iter().any(|w| w.tremulants == vec![0]));
    }
}
