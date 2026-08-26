//! GrandOrgue `.organ` (ODF) loader.
//!
//! Implemented against `docs/go-odf-notes.md`, which was compiled from
//! GrandOrgue's own loader source. Playback-only: console/panel keys are
//! ignored. Deliberately lenient — real-world ODFs are messy and GO
//! itself tolerates duplicates, wrong key case, and locale-mangled
//! numbers; non-fatal oddities are reported as warnings, not errors.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use aristide_model::{
    AttackSample, Coupler, CouplerRoute, CouplerTarget, Manual, ManualId, ManualKind, Organ,
    Pipe, PipeRef,
    PipeSource, Rank, RankId, RankRange, ReleaseSample, SampleLoop, Stop, StopId,
};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum OdfError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("line {line}: {reason}")]
    Syntax { line: usize, reason: String },
    #[error("missing section [{0}]")]
    MissingSection(String),
    #[error("[{section}] missing required key {key}")]
    MissingKey { section: String, key: String },
    #[error("[{section}] {key}: {reason}")]
    Invalid {
        section: String,
        key: String,
        reason: String,
    },
}

/// A parsed organ plus non-fatal deviations encountered on the way.
#[derive(Debug)]
pub struct LoadResult {
    pub organ: Organ,
    pub warnings: Vec<String>,
}

pub fn load(path: &Path) -> Result<LoadResult, OdfError> {
    let bytes = std::fs::read(path)?;
    let base = path.parent().unwrap_or(Path::new("")).to_path_buf();
    parse(&bytes, base)
}

pub fn parse(bytes: &[u8], base_path: PathBuf) -> Result<LoadResult, OdfError> {
    let ini = Ini::parse(bytes)?;
    Builder {
        ini: &ini,
        base_path,
        warnings: Vec::new(),
        pending_borrows: Vec::new(),
    }
    .build()
}

/// ISO-8859-1 unless the file opens with a UTF-8 BOM, per GO's loader.
fn decode(bytes: &[u8]) -> String {
    if let Some(stripped) = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        String::from_utf8_lossy(stripped).into_owned()
    } else {
        bytes.iter().map(|&b| b as char).collect()
    }
}

/// Sections and keys are stored lowercased: GO matches exact case first
/// but falls back case-insensitively, so a lenient parser is just
/// case-insensitive throughout.
struct Ini {
    sections: HashMap<String, HashMap<String, String>>,
}

impl Ini {
    fn parse(bytes: &[u8]) -> Result<Ini, OdfError> {
        let text = decode(bytes);
        let mut sections: HashMap<String, HashMap<String, String>> = HashMap::new();
        let mut current: Option<String> = None;

        for (index, raw_line) in text.lines().enumerate() {
            let line = match raw_line.find(';') {
                Some(pos) => &raw_line[..pos],
                None => raw_line,
            }
            .trim();
            if line.is_empty() {
                continue;
            }

            if let Some(name) = line.strip_prefix('[') {
                let Some(name) = name.strip_suffix(']') else {
                    return Err(OdfError::Syntax {
                        line: index + 1,
                        reason: format!("unterminated section header {line:?}"),
                    });
                };
                let name = name.trim().to_ascii_lowercase();
                current = Some(name.clone());
                sections.entry(name).or_default();
                continue;
            }

            let Some((key, value)) = line.split_once('=') else {
                return Err(OdfError::Syntax {
                    line: index + 1,
                    reason: format!("expected key=value, got {line:?}"),
                });
            };
            let Some(section) = &current else {
                return Err(OdfError::Syntax {
                    line: index + 1,
                    reason: "key=value before any [Section]".into(),
                });
            };
            sections
                .get_mut(section)
                .expect("current section always inserted")
                .insert(key.trim().to_ascii_lowercase(), value.trim().to_string());
        }
        Ok(Ini { sections })
    }

    fn section(&self, name: &str) -> Result<SectionReader<'_>, OdfError> {
        let lower = name.to_ascii_lowercase();
        self.sections
            .get(&lower)
            .map(|keys| SectionReader {
                name: name.to_string(),
                keys,
            })
            .ok_or_else(|| OdfError::MissingSection(name.to_string()))
    }
}

struct SectionReader<'a> {
    name: String,
    keys: &'a HashMap<String, String>,
}

impl SectionReader<'_> {
    fn get(&self, key: &str) -> Option<&str> {
        self.keys.get(&key.to_ascii_lowercase()).map(String::as_str)
    }

    fn missing(&self, key: &str) -> OdfError {
        OdfError::MissingKey {
            section: self.name.clone(),
            key: key.to_string(),
        }
    }

    fn invalid(&self, key: &str, reason: impl Into<String>) -> OdfError {
        OdfError::Invalid {
            section: self.name.clone(),
            key: key.to_string(),
            reason: reason.into(),
        }
    }

    fn string(&self, key: &str) -> Result<&str, OdfError> {
        self.get(key).ok_or_else(|| self.missing(key))
    }

    fn int(&self, key: &str) -> Result<i64, OdfError> {
        self.opt_int(key)?.ok_or_else(|| self.missing(key))
    }

    fn opt_int(&self, key: &str) -> Result<Option<i64>, OdfError> {
        self.get(key)
            .map(|value| {
                value
                    .parse::<i64>()
                    .map_err(|_| self.invalid(key, format!("not an integer: {value:?}")))
            })
            .transpose()
    }

    fn int_or(&self, key: &str, default: i64) -> Result<i64, OdfError> {
        Ok(self.opt_int(key)?.unwrap_or(default))
    }

    /// GO tolerates a comma decimal separator (locale damage).
    fn float_or(&self, key: &str, default: f64) -> Result<f64, OdfError> {
        match self.get(key) {
            None => Ok(default),
            Some(value) => value
                .replace(',', ".")
                .parse::<f64>()
                .map_err(|_| self.invalid(key, format!("not a number: {value:?}"))),
        }
    }

    fn bool_or(&self, key: &str, default: bool) -> Result<bool, OdfError> {
        match self.get(key).map(|v| v.chars().next()) {
            None => Ok(default),
            Some(Some('Y' | 'y')) => Ok(true),
            Some(Some('N' | 'n')) => Ok(false),
            Some(other) => Err(self.invalid(key, format!("expected Y or N, got {other:?}"))),
        }
    }
}

/// One level of GO's pipe-config inheritance tree (§8 of the notes):
/// AmplitudeLevel compounds multiplicatively, the rest add.
#[derive(Clone, Copy)]
struct GainChain {
    amplitude_factor: f64,
    gain_db: f64,
    pitch_tuning_cents: f64,
    pitch_correction_cents: f64,
}

impl GainChain {
    fn read(section: &SectionReader<'_>, parent: GainChain) -> Result<GainChain, OdfError> {
        Ok(GainChain {
            amplitude_factor: parent.amplitude_factor
                * (section.float_or("AmplitudeLevel", 100.0)? / 100.0),
            gain_db: parent.gain_db + section.float_or("Gain", 0.0)?,
            pitch_tuning_cents: parent.pitch_tuning_cents + section.float_or("PitchTuning", 0.0)?,
            pitch_correction_cents: parent.pitch_correction_cents
                + section.float_or("PitchCorrection", 0.0)?,
        })
    }

    fn total_gain_db(&self) -> f64 {
        self.gain_db + 20.0 * self.amplitude_factor.max(1e-6).log10()
    }
}

const ROOT_CHAIN: GainChain = GainChain {
    amplitude_factor: 1.0,
    gain_db: 0.0,
    pitch_tuning_cents: 0.0,
    pitch_correction_cents: 0.0,
};

/// A `REF:<manual>:<stop>:<pipe>` borrow, recorded while reading pipes and
/// resolved once every stop exists (the ODF allows forward references).
/// `<stop>` is the 1-based slot within the manual's stop list, `<pipe>` the
/// 1-based index into that stop's *first rank's* pipes (GOReferencePipe.cpp).
struct PendingBorrow {
    /// The borrowing pipe's slot in the organ being built.
    at: PipeRef,
    manual: i64,
    stop_slot: i64,
    pipe_number: i64,
    /// "[Section] PipeNNN", for warnings.
    context: String,
}

struct Builder<'a> {
    ini: &'a Ini,
    base_path: PathBuf,
    warnings: Vec<String>,
    pending_borrows: Vec<PendingBorrow>,
}

impl Builder<'_> {
    fn build(mut self) -> Result<LoadResult, OdfError> {
        let organ_section = self.ini.section("Organ")?;
        let name = organ_section.string("ChurchName")?.to_string();
        let has_pedals = organ_section.bool_or("HasPedals", false)?;
        let manual_count = organ_section.int("NumberOfManuals")?;
        let windchest_count = organ_section.int_or("NumberOfWindchestGroups", 1)?;
        let rank_count = organ_section.int_or("NumberOfRanks", 0)?;

        let organ_chain = GainChain::read(&organ_section, ROOT_CHAIN)?;

        // Enclosures: name + closed-amplitude floor; the engine decides
        // taper and filtering (docs/research/enclosure-modeling.md).
        let enclosure_count = organ_section.int_or("NumberOfEnclosures", 0)?;
        let mut enclosures = Vec::new();
        for index in 1..=enclosure_count {
            let section = self.ini.section(&format!("Enclosure{index:03}"))?;
            let midi_input = section.int_or("MIDIInputNumber", 0)?;
            enclosures.push(aristide_model::Enclosure {
                name: section.string("Name")?.to_string(),
                amp_minimum_level: section.float_or("AmpMinimumLevel", 0.0)?.clamp(0.0, 100.0),
                midi_input_number: (midi_input > 0).then_some(midi_input as u16),
                displayed: section.bool_or("Displayed", true)?,
            });
        }

        let mut windchest_chains = HashMap::new();
        let mut windchests = Vec::new();
        for index in 1..=windchest_count {
            let (chain, windchest) = match self.ini.section(&format!("WindchestGroup{index:03}")) {
                Ok(section) => {
                    // EnclosureNNN values are 1-based global enclosure
                    // indices (GOWindchest.cpp); store them 0-based.
                    let member_count = section.int_or("NumberOfEnclosures", 0)?;
                    let mut members = Vec::new();
                    for member in 1..=member_count {
                        let reference = section.int(&format!("Enclosure{member:03}"))?;
                        if (1..=enclosure_count).contains(&reference) {
                            members.push(reference as u32 - 1);
                        } else {
                            self.warn(format!(
                                "[WindchestGroup{index:03}] Enclosure{member:03}={reference} \
                                 out of range; ignored"
                            ));
                        }
                    }
                    let windchest = aristide_model::Windchest {
                        number: index as u32,
                        name: section
                            .get("Name")
                            .unwrap_or(&format!("Windchest {index}"))
                            .to_string(),
                        enclosures: members,
                    };
                    (GainChain::read(&section, organ_chain)?, windchest)
                }
                Err(_) => {
                    self.warn(format!(
                        "[WindchestGroup{index:03}] missing; using organ-level defaults"
                    ));
                    let windchest = aristide_model::Windchest {
                        number: index as u32,
                        name: format!("Windchest {index}"),
                        enclosures: Vec::new(),
                    };
                    (organ_chain, windchest)
                }
            };
            windchest_chains.insert(index, chain);
            windchests.push(windchest);
        }

        let mut organ = Organ {
            name,
            base_path: std::mem::take(&mut self.base_path),
            enclosures,
            windchests,
            ..Organ::default()
        };

        // Standalone [RankNNN] sections (new-style organs).
        let mut rank_first_midi = HashMap::new();
        for index in 1..=rank_count {
            let section = self.ini.section(&format!("Rank{index:03}"))?;
            let windchest = section.int_or("WindchestGroup", 1)?;
            let chain = windchest_chains
                .get(&windchest)
                .copied()
                .unwrap_or(organ_chain);
            let first_midi = section.int("FirstMidiNoteNumber")?;
            let rank = self.read_rank(&section, RankId(index as u32), first_midi, chain)?;
            rank_first_midi.insert(index, first_midi);
            organ.ranks.push(rank);
        }

        let first_manual = if has_pedals { 0 } else { 1 };
        let mut next_stop_id = 0u32;
        let mut next_inline_rank_id = 1000u32; // clear of the [RankNNN] id space

        // (manual index, 1-based stop slot) → the stop's first rank,
        // the address space REF: borrows resolve against.
        let mut stop_first_rank: HashMap<(i64, i64), RankId> = HashMap::new();

        for manual_index in first_manual..=manual_count {
            let section = self.ini.section(&format!("Manual{manual_index:03}"))?;
            let manual_id = ManualId(manual_index as u32);
            let first_accessible_logical = section.int("FirstAccessibleKeyLogicalKeyNumber")?;
            let first_midi_note = section.int("FirstAccessibleKeyMIDINoteNumber")?;
            let accessible_keys = section.int("NumberOfAccessibleKeys")?;
            organ.manuals.push(Manual {
                id: manual_id,
                name: section.string("Name")?.to_string(),
                first_midi_note: first_midi_note as u8,
                key_count: accessible_keys as u16,
                kind: if manual_index == 0 {
                    ManualKind::Pedal
                } else {
                    ManualKind::Manual
                },
            });
            // MIDI note of logical key 1 (see notes §2) — the pitch origin
            // that old-style stops derive their rank numbering from.
            let first_logical_midi = first_midi_note - first_accessible_logical + 1;

            let stop_count = section.int("NumberOfStops")?;
            for slot in 1..=stop_count {
                let target = section.int(&format!("Stop{slot:03}"))?;
                let stop_section = self.ini.section(&format!("Stop{target:03}"))?;
                next_stop_id += 1;
                let stop = self.read_stop(
                    &stop_section,
                    StopId(next_stop_id),
                    manual_id,
                    first_accessible_logical,
                    first_logical_midi,
                    &windchest_chains,
                    organ_chain,
                    &mut next_inline_rank_id,
                    &mut organ.ranks,
                )?;
                if let Some(range) = stop.ranks.first() {
                    stop_first_rank.insert((manual_index, slot), range.rank);
                }
                organ.stops.push(stop);
            }

            let coupler_count = section.int_or("NumberOfCouplers", 0)?;
            for slot in 1..=coupler_count {
                let target = section.int(&format!("Coupler{slot:03}"))?;
                let coupler_section = self.ini.section(&format!("Coupler{target:03}"))?;
                let name = coupler_section.get("Name").unwrap_or("Coupler").to_string();
                if coupler_section.bool_or("UnisonOff", false)? {
                    // The manual's own sound is silenced while it still
                    // drives other couplers: a route with no target.
                    organ.couplers.push(Coupler {
                        name,
                        routes: vec![CouplerRoute {
                            from_manual: manual_id,
                            low_key: None,
                            high_key: None,
                            unison_off: true,
                            target: None,
                        }],
                    });
                    continue;
                }
                let kind = coupler_section.get("CouplerType").unwrap_or("Normal");
                if !kind.eq_ignore_ascii_case("Normal") {
                    self.warn(format!(
                        "[{}]: {kind} couplers not yet supported, skipped",
                        coupler_section.name
                    ));
                    continue;
                }
                // GO restricts a coupler to source notes in
                // [FirstMIDINoteNumber, FirstMIDINoteNumber+NumberOfKeys);
                // the defaults (0, 127) cover any keyboard, so only a
                // set value becomes a bound.
                let first = coupler_section.int_or("FirstMIDINoteNumber", 0)?;
                let count = coupler_section.int_or("NumberOfKeys", 127)?;
                let last = first + count - 1;
                organ.couplers.push(Coupler {
                    name,
                    routes: vec![CouplerRoute {
                        from_manual: manual_id,
                        low_key: (first > 0).then_some(first.clamp(0, 127) as u8),
                        high_key: (last < 127).then_some(last.clamp(0, 127) as u8),
                        unison_off: false,
                        target: Some(CouplerTarget {
                            manual: ManualId(coupler_section.int("DestinationManual")? as u32),
                            key_shift: coupler_section.int("DestinationKeyshift")? as i16,
                            repitch: None,
                        }),
                    }],
                });
            }
        }

        self.resolve_borrows(&mut organ, &stop_first_rank);

        Ok(LoadResult {
            organ,
            warnings: self.warnings,
        })
    }

    /// Patch every recorded `REF:` borrow now that all stops exist.
    /// Unresolvable or cyclic borrows degrade to silent pipes with a
    /// warning (GO aborts the whole load here; we stay lenient).
    fn resolve_borrows(
        &mut self,
        organ: &mut Organ,
        stop_first_rank: &HashMap<(i64, i64), RankId>,
    ) {
        let pending = std::mem::take(&mut self.pending_borrows);
        for borrow in &pending {
            let target = stop_first_rank
                .get(&(borrow.manual, borrow.stop_slot))
                .and_then(|&rank_id| {
                    let rank = organ.rank(rank_id)?;
                    let index = usize::try_from(borrow.pipe_number).ok()?.checked_sub(1)?;
                    (index < rank.pipes.len()).then_some(PipeRef {
                        rank: rank_id,
                        pipe: index as u16,
                    })
                });
            match target {
                Some(target) => set_pipe_source(organ, borrow.at, PipeSource::Borrowed(target)),
                None => self.warn(format!(
                    "{}: REF:{}:{}:{} does not resolve to a pipe, silent",
                    borrow.context, borrow.manual, borrow.stop_slot, borrow.pipe_number
                )),
            }
        }
        // Every target above was validated, so a chain can only fail to
        // terminate by looping. Silencing the first pipe found on a cycle
        // breaks it for the rest of the chain.
        for borrow in &pending {
            if organ.sounding_pipe(borrow.at).is_none() {
                self.warn(format!("{}: borrow cycle, silent", borrow.context));
                set_pipe_source(organ, borrow.at, PipeSource::Silent);
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn read_stop(
        &mut self,
        section: &SectionReader<'_>,
        id: StopId,
        manual: ManualId,
        first_accessible_logical: i64,
        first_logical_midi: i64,
        windchest_chains: &HashMap<i64, GainChain>,
        organ_chain: GainChain,
        next_inline_rank_id: &mut u32,
        ranks: &mut Vec<Rank>,
    ) -> Result<Stop, OdfError> {
        let name = section.get("Name").unwrap_or(&section.name).to_string();
        let first_key_logical = section.int("FirstAccessiblePipeLogicalKeyNumber")?;
        let accessible_pipes = section.int("NumberOfAccessiblePipes")?;
        // Key index relative to the manual's accessible-key range.
        // Negative for extended-compass stops whose pipes start below
        // the keyboard (e.g. 85 pipes from logical key 1 under a
        // 61-key manual starting at logical key 13): those low pipes
        // are unplayable from the keys, so the range starts at key 0
        // and skips them via `first_pipe` instead.
        let base_key = first_key_logical - first_accessible_logical;
        // …and however many pipes its ranges cover, a stop sounds at
        // most `NumberOfAccessiblePipes` keys from its first — GO drops
        // the rest in `GOStop::SetKeyState`'s outer guard
        // (model/GOStop.cpp lines 120-126).
        let past_last_key = base_key + accessible_pipes;
        let clip = |first_key: i64, key_count: i64, first_pipe: i64| {
            let below = (-first_key).max(0);
            let start = first_key.max(0);
            let end = (first_key + key_count).min(past_last_key);
            RankRange {
                rank: RankId(0),
                first_key: start as u16,
                key_count: (end - start).max(0) as u16,
                first_pipe: (first_pipe + below).max(0) as u16,
            }
        };

        let referenced_ranks = section.int_or("NumberOfRanks", 0)?;
        let mut stop = Stop {
            id,
            name,
            manual,
            ranks: Vec::new(),
        };

        if referenced_ranks > 0 {
            for slot in 1..=referenced_ranks {
                let target = section.int(&format!("Rank{slot:03}"))?;
                let rank_id = RankId(target as u32);
                let Some(rank) = ranks.iter().find(|r| r.id == rank_id) else {
                    return Err(section.invalid(
                        &format!("Rank{slot:03}"),
                        format!("references undefined [Rank{target:03}]"),
                    ));
                };
                let pipes_in_rank = rank.pipes.len() as i64;
                let first_pipe = section.int_or(&format!("Rank{slot:03}FirstPipeNumber"), 1)?;
                let pipe_count = section.int_or(
                    &format!("Rank{slot:03}PipeCount"),
                    pipes_in_rank - first_pipe + 1,
                )?;
                let first_accessible_key =
                    section.int_or(&format!("Rank{slot:03}FirstAccessibleKeyNumber"), 1)?;
                stop.ranks.push(RankRange {
                    rank: rank_id,
                    ..clip(base_key + first_accessible_key - 1, pipe_count, first_pipe - 1)
                });
            }
        } else {
            // Old-style stop: the section doubles as an inline rank.
            let first_pipe_logical = section.int_or("FirstAccessiblePipeLogicalPipeNumber", 1)?;
            let derived_first_midi = first_logical_midi + first_key_logical - first_pipe_logical;
            let first_midi = section.int_or("FirstMidiNoteNumber", derived_first_midi)?;
            let windchest = section.int_or("WindchestGroup", 1)?;
            let chain = windchest_chains
                .get(&windchest)
                .copied()
                .unwrap_or(organ_chain);

            *next_inline_rank_id += 1;
            let rank_id = RankId(*next_inline_rank_id);
            let mut rank =
                self.read_rank_pipes(section, rank_id, accessible_pipes, first_midi, chain)?;
            rank.name.clone_from(&stop.name);
            let pipe_count = rank.pipes.len() as i64;
            ranks.push(rank);
            stop.ranks.push(RankRange {
                rank: rank_id,
                ..clip(base_key, pipe_count, 0)
            });
        }
        Ok(stop)
    }

    fn read_rank(
        &mut self,
        section: &SectionReader<'_>,
        id: RankId,
        first_midi: i64,
        parent_chain: GainChain,
    ) -> Result<Rank, OdfError> {
        let pipe_count = section.int("NumberOfLogicalPipes")?;
        let mut rank = self.read_rank_pipes(section, id, pipe_count, first_midi, parent_chain)?;
        rank.name = section.string("Name")?.to_string();
        Ok(rank)
    }

    fn read_rank_pipes(
        &mut self,
        section: &SectionReader<'_>,
        id: RankId,
        pipe_count: i64,
        first_midi: i64,
        parent_chain: GainChain,
    ) -> Result<Rank, OdfError> {
        let rank_chain = GainChain::read(section, parent_chain)?;
        // Both standalone [RankNNN] and old-style [StopNNN] sections carry
        // their windchest assignment under the same key.
        let windchest = section.int_or("WindchestGroup", 1)?.max(1) as u32;
        // Velocity→volume ramp endpoints, percent (notes §3/§8). GO's
        // "per-pipe" read uses the same unprefixed keys in the same
        // section, so one rank-level read is exactly its behaviour.
        let velocity_volume = aristide_model::VelocityVolume {
            at_zero: section.float_or("MinVelocityVolume", 100.0)?.clamp(0.0, 1000.0) / 100.0,
            at_full: section.float_or("MaxVelocityVolume", 100.0)?.clamp(0.0, 1000.0) / 100.0,
        };
        // The rank's pitch class: 8 = unison (8′), 16 = an octave up
        // (4′), 4 = an octave down (16′), 24 = a twelfth (2⅔′)…
        // Per-pipe overrides default to this (GO GORank/GOSoundingPipe).
        let rank_harmonic = read_harmonic(section, "HarmonicNumber", 8)?;
        let mut pipes = Vec::with_capacity(pipe_count.max(0) as usize);
        for index in 1..=pipe_count {
            let prefix = format!("Pipe{index:03}");
            let nominal_midi = first_midi + index - 1;
            let at = PipeRef {
                rank: id,
                pipe: (index - 1) as u16,
            };
            pipes.push(self.read_pipe(section, &prefix, nominal_midi, rank_harmonic, rank_chain, at)?);
        }
        Ok(Rank {
            id,
            name: String::new(),
            windchest,
            velocity_volume,
            pipes,
        })
    }

    fn read_pipe(
        &mut self,
        section: &SectionReader<'_>,
        prefix: &str,
        nominal_midi: i64,
        rank_harmonic: i64,
        rank_chain: GainChain,
        at: PipeRef,
    ) -> Result<Pipe, OdfError> {
        let value = section.string(prefix)?.to_string();
        let chain = GainChain {
            amplitude_factor: rank_chain.amplitude_factor
                * (section.float_or(&format!("{prefix}AmplitudeLevel"), 100.0)? / 100.0),
            gain_db: rank_chain.gain_db + section.float_or(&format!("{prefix}Gain"), 0.0)?,
            pitch_tuning_cents: rank_chain.pitch_tuning_cents
                + section.float_or(&format!("{prefix}PitchTuning"), 0.0)?,
            pitch_correction_cents: rank_chain.pitch_correction_cents
                + section.float_or(&format!("{prefix}PitchCorrection"), 0.0)?,
        };
        let harmonic = read_harmonic(section, &format!("{prefix}HarmonicNumber"), rank_harmonic)?;
        let mut pipe = Pipe {
            // The true sounding pitch: the key's ladder pitch times the
            // harmonic ratio (8 = unison). GO's expected-pitch formula
            // log2(H/8)·1200 cents, folded into Hz here.
            nominal_frequency_hz: midi_to_hz(nominal_midi as f64) * (harmonic as f64 / 8.0),
            pitch_tuning_cents: chain.pitch_tuning_cents,
            pitch_correction_cents: chain.pitch_correction_cents,
            gain_db: chain.total_gain_db(),
            midi_key_number: match section.int_or(&format!("{prefix}MIDIKeyNumber"), -1)? {
                -1 => None,
                key @ 0..=127 => Some(key as u8),
                other => {
                    return Err(section.invalid(
                        &format!("{prefix}MIDIKeyNumber"),
                        format!("out of range: {other}"),
                    ));
                }
            },
            midi_pitch_fraction_cents: {
                let fraction = section.float_or(&format!("{prefix}MIDIPitchFraction"), -1.0)?;
                if fraction < 0.0 {
                    None
                } else if fraction <= 100.0 {
                    Some(fraction)
                } else {
                    return Err(section.invalid(
                        &format!("{prefix}MIDIPitchFraction"),
                        format!("out of range: {fraction}"),
                    ));
                }
            },
            source: PipeSource::Silent,
        };

        if value.eq_ignore_ascii_case("DUMMY") {
            return Ok(pipe);
        }
        if let Some(reference) = value.strip_prefix("REF:") {
            // Stays Silent for now; resolve_borrows patches it once all
            // stops are loaded (forward references are legal).
            match parse_borrow(reference) {
                Some((manual, stop_slot, pipe_number)) => {
                    self.pending_borrows.push(PendingBorrow {
                        at,
                        manual,
                        stop_slot,
                        pipe_number,
                        context: format!("[{}] {prefix}", section.name),
                    });
                }
                None => self.warn(format!(
                    "[{}] {prefix}: malformed reference REF:{reference}, silent",
                    section.name
                )),
            }
            return Ok(pipe);
        }

        let mut attacks = vec![self.read_attack(section, prefix, &value)?];
        let attack_count = section.int_or(&format!("{prefix}AttackCount"), 0)?;
        for attack in 1..=attack_count {
            let attack_prefix = format!("{prefix}Attack{attack:03}");
            let path = section.string(&attack_prefix)?.to_string();
            attacks.push(self.read_attack(section, &attack_prefix, &path)?);
        }

        let release_count = section.int_or(&format!("{prefix}ReleaseCount"), 0)?;
        let mut releases = Vec::with_capacity(release_count.max(0) as usize);
        for release in 1..=release_count {
            let release_prefix = format!("{prefix}Release{release:03}");
            let path = section.string(&release_prefix)?;
            let max_ms = section.int_or(&format!("{release_prefix}MaxKeyPressTime"), -1)?;
            releases.push(ReleaseSample {
                path: normalize_path(path),
                max_key_press_ms: (max_ms >= 0).then_some(max_ms as u32),
            });
        }
        pipe.source = PipeSource::Sampled { attacks, releases };
        Ok(pipe)
    }

    fn read_attack(
        &mut self,
        section: &SectionReader<'_>,
        prefix: &str,
        path: &str,
    ) -> Result<AttackSample, OdfError> {
        let loop_count = section.int_or(&format!("{prefix}LoopCount"), 0)?;
        let mut loops = Vec::with_capacity(loop_count.max(0) as usize);
        for index in 1..=loop_count {
            let start = section.int(&format!("{prefix}Loop{index:03}Start"))?;
            let end = section.int(&format!("{prefix}Loop{index:03}End"))?;
            if end <= start {
                return Err(section.invalid(
                    &format!("{prefix}Loop{index:03}End"),
                    format!("loop end {end} not after start {start}"),
                ));
            }
            loops.push(SampleLoop {
                start: start as u64,
                end: end as u64,
            });
        }
        // Empty `loops` means: fall back to the WAV's own smpl chunk at
        // sample-load time.
        Ok(AttackSample {
            path: normalize_path(path),
            loops,
            pitch_offset_cents: 0.0,
        })
    }

    fn warn(&mut self, message: String) {
        self.warnings.push(message);
    }
}

/// ODF sample paths use backslashes regardless of host OS.
fn normalize_path(path: &str) -> PathBuf {
    PathBuf::from(path.replace('\\', "/"))
}

/// The `<manual>:<stop>:<pipe>` payload of a `REF:` pipe.
fn parse_borrow(reference: &str) -> Option<(i64, i64, i64)> {
    let mut parts = reference.splitn(3, ':');
    let manual = parts.next()?.trim().parse().ok()?;
    let stop = parts.next()?.trim().parse().ok()?;
    let pipe = parts.next()?.trim().parse().ok()?;
    Some((manual, stop, pipe))
}

fn set_pipe_source(organ: &mut Organ, at: PipeRef, source: PipeSource) {
    if let Some(pipe) = organ
        .ranks
        .iter_mut()
        .find(|r| r.id == at.rank)
        .and_then(|r| r.pipes.get_mut(at.pipe as usize))
    {
        pipe.source = source;
    }
}

fn midi_to_hz(midi: f64) -> f64 {
    440.0 * ((midi - 69.0) / 12.0).exp2()
}

/// A `HarmonicNumber` key: GO accepts 1–1024 (GOSoundingPipe::Load).
fn read_harmonic(
    section: &SectionReader<'_>,
    key: &str,
    default: i64,
) -> Result<i64, OdfError> {
    match section.int_or(key, default)? {
        harmonic @ 1..=1024 => Ok(harmonic),
        other => Err(section.invalid(key, format!("out of range: {other}"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_str(text: &str) -> LoadResult {
        parse(text.as_bytes(), PathBuf::from("/set")).expect("parse failed")
    }

    /// The minimal new-style organ from docs/go-odf-notes.md §10.
    const MINIMAL: &str = "\
[Organ]
ChurchName=Test Organ
HasPedals=N
NumberOfManuals=1
NumberOfWindchestGroups=1
NumberOfEnclosures=0
NumberOfTremulants=0
NumberOfRanks=1

[WindchestGroup001]
Name=Main Windchest
NumberOfEnclosures=0
NumberOfTremulants=0

[Rank001]
Name=Principal 8
FirstMidiNoteNumber=60
NumberOfLogicalPipes=3
WindchestGroup=1
HarmonicNumber=8
Pipe001=samples\\principal8\\c1.wav
Pipe002=samples\\principal8\\cs1.wav
Pipe003=samples\\principal8\\d1.wav

[Manual001]
Name=Manual I
NumberOfLogicalKeys=3
FirstAccessibleKeyLogicalKeyNumber=1
FirstAccessibleKeyMIDINoteNumber=60
NumberOfAccessibleKeys=3
NumberOfStops=1
Stop001=1

[Stop001]
Name=Principal 8
NumberOfRanks=1
FirstAccessiblePipeLogicalKeyNumber=1
NumberOfAccessiblePipes=3
Rank001=1
";

    #[test]
    fn minimal_new_style_organ() {
        let result = parse_str(MINIMAL);
        let organ = &result.organ;
        assert_eq!(organ.name, "Test Organ");
        assert_eq!(organ.manuals.len(), 1);
        assert_eq!(organ.ranks.len(), 1);
        assert_eq!(organ.stops.len(), 1);
        assert!(result.warnings.is_empty(), "{:?}", result.warnings);

        let rank = &organ.ranks[0];
        assert_eq!(rank.name, "Principal 8");
        assert_eq!(rank.pipes.len(), 3);
        let (attacks, _) = rank.pipes[0].samples().expect("sampled pipe");
        assert_eq!(attacks[0].path, PathBuf::from("samples/principal8/c1.wav"));
        // MIDI 60 = middle C ≈ 261.63 Hz.
        assert!((rank.pipes[0].nominal_frequency_hz - 261.6256).abs() < 0.01);

        let stop = &organ.stops[0];
        assert_eq!(stop.manual, ManualId(1));
        assert_eq!(stop.ranks.len(), 1);
        assert_eq!(stop.ranks[0].first_key, 0);
        assert_eq!(stop.ranks[0].key_count, 3);
        assert_eq!(stop.ranks[0].first_pipe, 0);
    }

    #[test]
    fn old_style_stop_builds_inline_rank() {
        let text = "\
[Organ]
ChurchName=Old Style
HasPedals=N
NumberOfManuals=1
NumberOfWindchestGroups=1

[WindchestGroup001]
Name=W

[Manual001]
Name=Great
NumberOfLogicalKeys=2
FirstAccessibleKeyLogicalKeyNumber=1
FirstAccessibleKeyMIDINoteNumber=36
NumberOfAccessibleKeys=2
NumberOfStops=1
Stop001=1

[Stop001]
Name=Subbass 16
FirstAccessiblePipeLogicalKeyNumber=1
NumberOfAccessiblePipes=2
FirstAccessiblePipeLogicalPipeNumber=1
Pipe001=036-C.wav
Pipe002=037-Cis.wav
";
        let result = parse_str(text);
        let organ = &result.organ;
        assert_eq!(organ.ranks.len(), 1);
        assert_eq!(organ.ranks[0].name, "Subbass 16");
        assert_eq!(organ.ranks[0].pipes.len(), 2);
        // Derived first MIDI note should equal the manual's first note.
        assert!((organ.ranks[0].pipes[0].nominal_frequency_hz - midi_to_hz(36.0)).abs() < 1e-9);
        assert_eq!(organ.stops[0].ranks[0].key_count, 2);
    }

    /// The extended-compass mapping, key by key. A stop whose pipes
    /// start below the keyboard (85 pipes from logical key 1 under a
    /// 61-key manual whose first key is logical 13, as the GrandOrgue
    /// demo set's Montre 8' does) must still sound written pitch: key
    /// MIDI 36 takes pipe 13, not pipe 1. Getting this wrong shifts the
    /// whole stop an octave down — silently, since the pipes exist and
    /// speak. GO's chain: manual key → `GOStop::SetKeyState`
    /// (`keyIndex = logical − FirstAccessiblePipeLogicalKeyNumber`) →
    /// `SetRankKeyState` (`pipe = keyIndex + FirstPipeNumber −
    /// FirstAccessibleKeyNumber`), with the rank's pitch origin at
    /// `FirstLogicalKeyMIDINoteNumber` (model/GOStop.cpp lines 83-126,
    /// GOManual.cpp lines 267, 316-326).
    #[test]
    fn extended_compass_stop_sounds_written_pitch() {
        let mut text = String::from(
            "\
[Organ]
ChurchName=Extended
HasPedals=N
NumberOfManuals=1
NumberOfWindchestGroups=1

[WindchestGroup001]
Name=W

[Manual001]
Name=Great
NumberOfLogicalKeys=85
FirstAccessibleKeyLogicalKeyNumber=13
FirstAccessibleKeyMIDINoteNumber=36
NumberOfAccessibleKeys=61
NumberOfStops=1
Stop001=1

[Stop001]
Name=Montre 8
FirstAccessiblePipeLogicalKeyNumber=1
NumberOfAccessiblePipes=85
FirstAccessiblePipeLogicalPipeNumber=1
",
        );
        for pipe in 1..=85 {
            text.push_str(&format!("Pipe{pipe:03}=p{pipe}.wav\n"));
        }

        let result = parse_str(&text);
        let organ = &result.organ;
        let rank = &organ.ranks[0];
        assert_eq!(rank.pipes.len(), 85);
        // Pitch origin: logical key 1 = MIDI 24, an octave below the
        // keyboard's lowest key — those twelve pipes are the extension.
        assert!((rank.pipes[0].nominal_frequency_hz - midi_to_hz(24.0)).abs() < 1e-9);

        let range = &organ.stops[0].ranks[0];
        assert_eq!(range.first_key, 0, "range starts at the lowest key");
        assert_eq!(range.first_pipe, 12, "…on the thirteenth pipe");
        assert_eq!(range.key_count, 73, "85 pipes less the 12 below the keys");

        // Every playable key sounds the pipe pitched to that key.
        let manual = &organ.manuals[0];
        for key_index in 0..manual.key_count {
            let key_midi = f64::from(manual.first_midi_note) + f64::from(key_index);
            let pipe = &rank.pipes[(range.first_pipe + key_index) as usize];
            assert!(
                (pipe.nominal_frequency_hz - midi_to_hz(key_midi)).abs() < 1e-9,
                "key {key_midi} sounds {} Hz, want {} Hz",
                pipe.nominal_frequency_hz,
                midi_to_hz(key_midi)
            );
        }
    }

    /// A stop sounds `NumberOfAccessiblePipes` keys and no more, even
    /// when the rank range it references runs on past them
    /// (`GOStop::SetKeyState`'s outer guard, model/GOStop.cpp:120).
    #[test]
    fn stop_range_stops_at_the_last_accessible_pipe() {
        let text = "\
[Organ]
ChurchName=Capped
HasPedals=N
NumberOfManuals=1
NumberOfWindchestGroups=1
NumberOfRanks=1

[WindchestGroup001]
Name=W

[Rank001]
Name=Principal 8
FirstMidiNoteNumber=36
NumberOfLogicalPipes=4
WindchestGroup=1
Pipe001=a.wav
Pipe002=b.wav
Pipe003=c.wav
Pipe004=d.wav

[Manual001]
Name=Great
NumberOfLogicalKeys=4
FirstAccessibleKeyLogicalKeyNumber=1
FirstAccessibleKeyMIDINoteNumber=36
NumberOfAccessibleKeys=4
NumberOfStops=1
Stop001=1

[Stop001]
Name=Principal 8
NumberOfRanks=1
FirstAccessiblePipeLogicalKeyNumber=1
NumberOfAccessiblePipes=2
Rank001=1
";
        let range = &parse_str(text).organ.stops[0].ranks[0];
        assert_eq!(range.first_key, 0);
        assert_eq!(range.key_count, 2, "four pipes, two accessible keys");
    }

    #[test]
    fn gain_and_tuning_chains_combine() {
        let text = "\
[Organ]
ChurchName=Chained
HasPedals=N
NumberOfManuals=1
NumberOfWindchestGroups=1
Gain=1
PitchTuning=10

[WindchestGroup001]
AmplitudeLevel=50
PitchTuning=5

[Manual001]
Name=M
NumberOfLogicalKeys=1
FirstAccessibleKeyLogicalKeyNumber=1
FirstAccessibleKeyMIDINoteNumber=60
NumberOfAccessibleKeys=1
NumberOfStops=1
Stop001=1

[Stop001]
Name=S
FirstAccessiblePipeLogicalKeyNumber=1
NumberOfAccessiblePipes=1
Gain=2
Pipe001=a.wav
Pipe001Gain=3
Pipe001PitchTuning=-15
";
        let organ = parse_str(text).organ;
        let pipe = &organ.ranks[0].pipes[0];
        // Gains add: 1 + 2 + 3 = 6 dB; amplitude 50% ≈ -6.0206 dB.
        assert!((pipe.gain_db - (6.0 + 20.0 * 0.5f64.log10())).abs() < 1e-6);
        // Tuning adds: 10 + 5 + 0 - 15 = 0 cents.
        assert!(pipe.pitch_tuning_cents.abs() < 1e-9);
    }

    /// `MinVelocityVolume`/`MaxVelocityVolume` (percent, default 100)
    /// become the rank's velocity→volume ramp; a section without them
    /// stays velocity-insensitive (notes §3/§8).
    #[test]
    fn velocity_volume_ramp_reaches_the_model() {
        let text = "\
[Organ]
ChurchName=Touchy
HasPedals=N
NumberOfManuals=1
NumberOfWindchestGroups=1

[WindchestGroup001]
Name=W

[Manual001]
Name=M
NumberOfLogicalKeys=1
FirstAccessibleKeyLogicalKeyNumber=1
FirstAccessibleKeyMIDINoteNumber=60
NumberOfAccessibleKeys=1
NumberOfStops=2
Stop001=1
Stop002=2

[Stop001]
Name=Tracker
FirstAccessiblePipeLogicalKeyNumber=1
NumberOfAccessiblePipes=1
MinVelocityVolume=25
MaxVelocityVolume=150
Pipe001=a.wav

[Stop002]
Name=Plain
FirstAccessiblePipeLogicalKeyNumber=1
NumberOfAccessiblePipes=1
Pipe001=b.wav
";
        let organ = parse_str(text).organ;
        let ramp = organ.ranks[0].velocity_volume;
        assert!((ramp.at_zero - 0.25).abs() < 1e-9);
        assert!((ramp.at_full - 1.5).abs() < 1e-9);
        assert!((ramp.gain(127) - 1.5).abs() < 1e-6);
        assert!((ramp.gain(0) - 0.25).abs() < 1e-6);
        assert_eq!(organ.ranks[1].velocity_volume, Default::default());
        assert!((organ.ranks[1].velocity_volume.gain(1) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn harmonic_correction_and_fraction_reach_the_model() {
        let text = "\
[Organ]
ChurchName=Pitched
HasPedals=N
NumberOfManuals=1
NumberOfWindchestGroups=1
PitchCorrection=7

[WindchestGroup001]
PitchCorrection=3

[Manual001]
Name=M
NumberOfLogicalKeys=2
FirstAccessibleKeyLogicalKeyNumber=1
FirstAccessibleKeyMIDINoteNumber=60
NumberOfAccessibleKeys=2
NumberOfStops=1
Stop001=1

[Stop001]
Name=Prestant 4
FirstAccessiblePipeLogicalKeyNumber=1
NumberOfAccessiblePipes=2
HarmonicNumber=16
PitchCorrection=-4
Pipe001=a.wav
Pipe001MIDIPitchFraction=25
Pipe002=b.wav
Pipe002HarmonicNumber=32
Pipe002PitchCorrection=1
";
        let organ = parse_str(text).organ;
        let pipes = &organ.ranks[0].pipes;
        // A 4′ rank (harmonic 16) sounds the key's octave: C4 key → C5.
        assert!((pipes[0].nominal_frequency_hz - midi_to_hz(72.0)).abs() < 1e-9);
        // The per-pipe override doubles it again (2′-equivalent).
        assert!((pipes[1].nominal_frequency_hz - midi_to_hz(85.0)).abs() < 1e-9);
        // PitchCorrection adds through the chain: 7 + 3 − 4 (+1).
        assert!((pipes[0].pitch_correction_cents - 6.0).abs() < 1e-9);
        assert!((pipes[1].pitch_correction_cents - 7.0).abs() < 1e-9);
        assert_eq!(pipes[0].midi_pitch_fraction_cents, Some(25.0));
        assert_eq!(pipes[1].midi_pitch_fraction_cents, None);
    }

    #[test]
    fn loops_releases_and_midi_key() {
        let text = "\
[Organ]
ChurchName=Detail
HasPedals=N
NumberOfManuals=1
NumberOfWindchestGroups=1

[Manual001]
Name=M
NumberOfLogicalKeys=1
FirstAccessibleKeyLogicalKeyNumber=1
FirstAccessibleKeyMIDINoteNumber=60
NumberOfAccessibleKeys=1
NumberOfStops=1
Stop001=1

[Stop001]
Name=S
FirstAccessiblePipeLogicalKeyNumber=1
NumberOfAccessiblePipes=1
Pipe001=a.wav
Pipe001MIDIKeyNumber=61
Pipe001LoopCount=1
Pipe001Loop001Start=1000
Pipe001Loop001End=48000
Pipe001AttackCount=1
Pipe001Attack001=a-trem.wav
Pipe001ReleaseCount=2
Pipe001Release001=a-short.wav
Pipe001Release001MaxKeyPressTime=500
Pipe001Release002=a-long.wav
";
        let organ = parse_str(text).organ;
        let pipe = &organ.ranks[0].pipes[0];
        assert_eq!(pipe.midi_key_number, Some(61));
        let (attacks, releases) = pipe.samples().expect("sampled pipe");
        assert_eq!(attacks.len(), 2);
        assert_eq!(
            attacks[0].loops,
            vec![SampleLoop {
                start: 1000,
                end: 48000
            }]
        );
        assert!(attacks[1].loops.is_empty());
        assert_eq!(releases.len(), 2);
        assert_eq!(releases[0].max_key_press_ms, Some(500));
        assert_eq!(releases[1].max_key_press_ms, None);
    }

    #[test]
    fn dummy_and_ref_pipes() {
        let text = "\
[Organ]
ChurchName=Silent
HasPedals=N
NumberOfManuals=1
NumberOfWindchestGroups=1

[WindchestGroup001]
Name=W

[Manual001]
Name=M
NumberOfLogicalKeys=3
FirstAccessibleKeyLogicalKeyNumber=1
FirstAccessibleKeyMIDINoteNumber=60
NumberOfAccessibleKeys=3
NumberOfStops=1
Stop001=1

[Stop001]
Name=S
FirstAccessiblePipeLogicalKeyNumber=1
NumberOfAccessiblePipes=3
Pipe001=DUMMY
Pipe002=REF:1:1:3
Pipe003=a.wav
";
        let result = parse_str(text);
        assert!(result.warnings.is_empty(), "{:?}", result.warnings);
        let organ = &result.organ;
        let rank_id = organ.ranks[0].id;
        let pipes = &organ.ranks[0].pipes;
        assert!(matches!(pipes[0].source, PipeSource::Silent));
        let target = PipeRef {
            rank: rank_id,
            pipe: 2,
        };
        assert!(matches!(pipes[1].source, PipeSource::Borrowed(t) if t == target));
        // The borrow chain lands on the sampled pipe.
        let sounding = organ
            .sounding_pipe(PipeRef {
                rank: rank_id,
                pipe: 1,
            })
            .expect("chain terminates");
        assert!(sounding.samples().is_some());
    }

    #[test]
    fn borrow_forward_reference_across_manuals() {
        // The pedal (manual 0, loaded first) borrows from manual 1's
        // stop, which doesn't exist yet when the pedal pipes are read.
        let text = "\
[Organ]
ChurchName=Unit
HasPedals=Y
NumberOfManuals=1
NumberOfWindchestGroups=1

[WindchestGroup001]
Name=W

[Manual000]
Name=Pedal
NumberOfLogicalKeys=1
FirstAccessibleKeyLogicalKeyNumber=1
FirstAccessibleKeyMIDINoteNumber=36
NumberOfAccessibleKeys=1
NumberOfStops=1
Stop001=1

[Stop001]
Name=Borrowed Bass
FirstAccessiblePipeLogicalKeyNumber=1
NumberOfAccessiblePipes=1
Pipe001=REF:1:1:2

[Manual001]
Name=Great
NumberOfLogicalKeys=2
FirstAccessibleKeyLogicalKeyNumber=1
FirstAccessibleKeyMIDINoteNumber=36
NumberOfAccessibleKeys=2
NumberOfStops=1
Stop001=2

[Stop002]
Name=Principal
FirstAccessiblePipeLogicalKeyNumber=1
NumberOfAccessiblePipes=2
Pipe001=036-C.wav
Pipe002=037-Cis.wav
";
        let result = parse_str(text);
        assert!(result.warnings.is_empty(), "{:?}", result.warnings);
        let organ = &result.organ;
        let pedal_rank = organ.stops[0].ranks[0].rank;
        let great_rank = organ.stops[1].ranks[0].rank;
        let PipeSource::Borrowed(target) = organ.ranks[0].pipes[0].source else {
            panic!("expected borrowed pipe in pedal rank");
        };
        assert_eq!(organ.ranks[0].id, pedal_rank);
        assert_eq!(
            target,
            PipeRef {
                rank: great_rank,
                pipe: 1
            }
        );
        let sounding = organ.sounding_pipe(target).expect("resolves");
        let (attacks, _) = sounding.samples().expect("sampled");
        assert_eq!(attacks[0].path, PathBuf::from("037-Cis.wav"));
    }

    #[test]
    fn invalid_and_malformed_borrows_degrade_to_silent() {
        let text = "\
[Organ]
ChurchName=Bad Refs
HasPedals=N
NumberOfManuals=1
NumberOfWindchestGroups=1

[WindchestGroup001]
Name=W

[Manual001]
Name=M
NumberOfLogicalKeys=3
FirstAccessibleKeyLogicalKeyNumber=1
FirstAccessibleKeyMIDINoteNumber=60
NumberOfAccessibleKeys=3
NumberOfStops=1
Stop001=1

[Stop001]
Name=S
FirstAccessiblePipeLogicalKeyNumber=1
NumberOfAccessiblePipes=3
Pipe001=REF:9:9:9
Pipe002=REF:nonsense
Pipe003=a.wav
";
        let result = parse_str(text);
        assert_eq!(result.warnings.len(), 2, "{:?}", result.warnings);
        let pipes = &result.organ.ranks[0].pipes;
        assert!(matches!(pipes[0].source, PipeSource::Silent));
        assert!(matches!(pipes[1].source, PipeSource::Silent));
    }

    #[test]
    fn borrow_cycle_is_broken_with_warning() {
        let text = "\
[Organ]
ChurchName=Cycle
HasPedals=N
NumberOfManuals=1
NumberOfWindchestGroups=1

[WindchestGroup001]
Name=W

[Manual001]
Name=M
NumberOfLogicalKeys=2
FirstAccessibleKeyLogicalKeyNumber=1
FirstAccessibleKeyMIDINoteNumber=60
NumberOfAccessibleKeys=2
NumberOfStops=1
Stop001=1

[Stop001]
Name=S
FirstAccessiblePipeLogicalKeyNumber=1
NumberOfAccessiblePipes=2
Pipe001=REF:1:1:2
Pipe002=REF:1:1:1
";
        let result = parse_str(text);
        assert_eq!(result.warnings.len(), 1, "{:?}", result.warnings);
        let organ = &result.organ;
        let rank_id = organ.ranks[0].id;
        // Every pipe still resolves to something (Silent breaks the loop).
        for pipe in 0..2 {
            assert!(
                organ
                    .sounding_pipe(PipeRef {
                        rank: rank_id,
                        pipe
                    })
                    .is_some(),
                "pipe {pipe} should terminate"
            );
        }
    }

    #[test]
    fn comments_case_and_duplicates() {
        let text = "\
[Organ]
churchname=First ; inline comment
CHURCHNAME=Second
hasPedals=N
NumberOfManuals=1
NumberOfWindchestGroups=1

[Manual001]
Name=M
NumberOfLogicalKeys=1
FirstAccessibleKeyLogicalKeyNumber=1
FirstAccessibleKeyMIDINoteNumber=60
NumberOfAccessibleKeys=1
NumberOfStops=0
";
        let organ = parse_str(text).organ;
        assert_eq!(organ.name, "Second");
    }

    #[test]
    fn latin1_fallback_decoding() {
        let mut bytes = b"[Organ]\nChurchName=Notre-Dame de Lib".to_vec();
        bytes.push(0xE9); // 'é' in ISO-8859-1
        bytes.extend_from_slice(
            b"ration\nHasPedals=N\nNumberOfManuals=1\nNumberOfWindchestGroups=1\n\
[Manual001]\nName=M\nNumberOfLogicalKeys=1\nFirstAccessibleKeyLogicalKeyNumber=1\n\
FirstAccessibleKeyMIDINoteNumber=60\nNumberOfAccessibleKeys=1\nNumberOfStops=0\n",
        );
        let result = parse(&bytes, PathBuf::from("/set")).expect("parse failed");
        assert_eq!(result.organ.name, "Notre-Dame de Libération");
    }

    #[test]
    fn missing_required_key_names_section() {
        let error = parse(b"[Organ]\nHasPedals=N\n", PathBuf::new()).unwrap_err();
        let message = error.to_string();
        assert!(
            message.contains("Organ") && message.contains("ChurchName"),
            "{message}"
        );
    }
}
