//! Loading an instrument: source paths → one `Organ` → decoded sample
//! bank → a fully configured console, plus every engine setting the
//! sources imply. Pure control-side work — no device, stream, or shared
//! state is touched — so the same path serves the CLI at startup and
//! the console's organ picker at runtime.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use anyhow::{Context, Result};
use aristide_engine::bank::SampleBank;
use aristide_formats::instrument;
use aristide_model::units::{cents_between, cents_to_ratio};
use aristide_model::{Organ, StopId};

use crate::console::Console;
use crate::{bank, tuning, Setup};

/// Everything a load produces: the playable console, the samples it
/// plays, and the engine-wide settings the caller sends once the new
/// engine exists.
/// One engageable tremulant, ready for the console and the engine: the
/// ODF's own tremulants when the set defines any, else the sidecar's
/// (or default) instrument-wide one.
#[derive(Debug, Clone)]
pub struct TremulantSetup {
    pub name: String,
    /// Wave tremulants switch sample variants instead of modulating
    /// pressure; their `params` are unused.
    pub wave: bool,
    pub params: aristide_engine::wind::TremulantParams,
    /// 0-based engine wind groups (ODF chest number − 1).
    pub groups: Vec<u8>,
}

/// One stop's own `[[voicing.adjust]]` rule — the file fields the
/// console editor shows and writes back (a rule whose `stops` is
/// exactly that one stop's name). Pattern rules still apply to the
/// sound; they just aren't editable per stop.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct StopVoicing {
    /// Target footage, absent = the stop's native pitch.
    pub feet: Option<f64>,
    pub cents: f64,
    pub gain_db: f64,
}

impl StopVoicing {
    pub fn is_neutral(&self) -> bool {
        *self == StopVoicing::default()
    }
}

/// One configured output bus, engine-ready.
pub struct BusSetup {
    /// Engine bus index (1-based; 0 is the untouched main bus).
    pub bus: u8,
    /// 0-based interface channel pair; `None` keeps the main pair.
    pub output: Option<(u8, u8)>,
    pub gain: f32,
    pub delay: Option<aristide_engine::routing::DelayParams>,
}

pub struct PreparedInstrument {
    pub console: Console,
    pub bank: SampleBank,
    pub wind: Option<aristide_engine::wind::WindParams>,
    pub tremulants: Vec<TremulantSetup>,
    pub enclosures: Vec<(u8, aristide_engine::enclosure::EnclosureParams)>,
    pub expression_cc: u8,
    pub reverb: Option<(Arc<aristide_engine::reverb::PreparedIr>, f32)>,
    /// Set when the loaded organ is a composite definition file loaded
    /// alone: that file owns the rig's MIDI wiring.
    pub composite: Option<(PathBuf, instrument::MidiDef)>,
    pub suggested_channels: Vec<Option<u8>>,
    pub setup: Setup,
    /// Per stop: where it came from — the coordinates per-stop file
    /// edits address their lines by.
    pub provenance: std::collections::HashMap<StopId, instrument::StopProvenance>,
    /// Per stop: its own editable `[[voicing.adjust]]` rule, if any.
    pub stop_voicing: std::collections::HashMap<StopId, StopVoicing>,
    /// Per stop: its declared knob engraving (`""` = engrave nothing).
    /// Stops absent here engrave the footage they actually speak at.
    pub stop_labels: std::collections::HashMap<StopId, String>,
    /// The file's `[console.order]` — per manual name, its drawknob
    /// display order. Empty unless a composite loaded alone declared
    /// one, same condition as `layout`.
    pub stop_order: std::collections::BTreeMap<String, Vec<String>>,
    /// The organ file's `[console.layout]` — empty unless it is a
    /// composite loaded alone, same condition as `composite` above.
    pub layout: std::collections::BTreeMap<String, instrument::PanelPos>,
    /// The file's `[console] coupled_keys` — whether engaged couplers
    /// pull the coupled keys down on the on-screen keyboards. Display
    /// only; true unless the file says otherwise.
    pub coupled_keys: bool,
    /// The file's `[console.coupler_keys]` — per-coupler `"never"` /
    /// `"always"` overrides of `coupled_keys`, by console name.
    pub coupler_key_modes: std::collections::BTreeMap<String, String>,
    /// Bus setups from the sidecar's `[routing]`, ready to send to the
    /// engine (bus 0, the main pair, is never listed). The per-stop
    /// half of the plan is already installed in the console.
    pub buses: Vec<BusSetup>,
    /// Everything this load skipped or ignored (dangling references
    /// healed over, sidecar lines that didn't resolve) — surfaced to
    /// the console, because "loaded, but emptier than the file says"
    /// must never be silent.
    pub warnings: Vec<String>,
}

pub fn load_organ(path: &Path) -> Result<Organ> {
    let started = Instant::now();
    let result = aristide_formats::grandorgue::load(path)
        .with_context(|| format!("loading {}", path.display()))?;
    tracing::info!(
        "organ: {} ({} stops, {} ranks) in {:.1?}",
        result.organ.name,
        result.organ.stops.len(),
        result.organ.ranks.len(),
        started.elapsed()
    );
    for warning in result.warnings.iter().take(10) {
        tracing::warn!("odf: {warning}");
    }
    if result.warnings.len() > 10 {
        tracing::warn!("odf: … and {} more warnings", result.warnings.len() - 10);
    }
    Ok(result.organ)
}

/// Stop patterns (from `--stops` or the sidecar) resolve exact-first,
/// then shortest-substring (see `sidecar::match_names`), so "plein jeu"
/// draws the mixture and not its drawstop noise. With no patterns at
/// all the organ starts cancelled — no stop drawn, as an organist finds
/// it — and the player registers from silence.
pub fn choose_registration(organ: &Organ, patterns: &[String]) -> Vec<StopId> {
    let drawn: Vec<StopId> = if patterns.is_empty() {
        Vec::new()
    } else {
        let names: Vec<&str> = organ.stops.iter().map(|s| s.name.as_str()).collect();
        let mut drawn: Vec<StopId> = patterns
            .iter()
            .flat_map(|p| aristide_formats::sidecar::match_names(&names, p))
            .map(|i| organ.stops[i].id)
            .collect();
        drawn.sort_by_key(|id| id.0);
        drawn.dedup();
        drawn
    };
    for stop in &organ.stops {
        if drawn.contains(&stop.id) {
            let manual = organ
                .manuals
                .iter()
                .find(|m| m.id == stop.manual)
                .map(|m| m.name.as_str())
                .unwrap_or("?");
            tracing::info!("drawn: {} ({manual})", stop.name);
        }
    }
    if drawn.is_empty() {
        if patterns.is_empty() {
            tracing::info!("registration cancelled — draw stops in the console");
        } else {
            tracing::warn!("no stops matched — keys will be silent");
        }
    }
    drawn
}

/// The sidecar's `midi.channels` (manual names in channel order) read
/// backwards: per manual index, the channel it conventionally speaks on.
///
/// This is a *suggestion*, never a route. A set can say "the Récit is
/// channel 2" because that is how its console was built, and the dialog
/// then pre-fills channel 2 when you hand-assign a device to the Récit;
/// nothing sounds until you assign one.
pub fn suggested_channels(organ: &Organ, channel_names: &[String]) -> Vec<Option<u8>> {
    let names: Vec<&str> = organ.manuals.iter().map(|m| m.name.as_str()).collect();
    let mut suggested = vec![None; organ.manuals.len()];
    for (channel, pattern) in channel_names.iter().enumerate().take(16) {
        match aristide_formats::sidecar::match_names(&names, pattern).as_slice() {
            [manual] if suggested[*manual].is_none() => {
                suggested[*manual] = Some(channel as u8 + 1);
            }
            [_] => {}
            matched => tracing::warn!(
                "sidecar midi.channels: {pattern:?} matched {} manuals, ignoring it",
                matched.len()
            ),
        }
    }
    suggested
}

/// Load a reverb impulse response: a wav next to the set, or
/// "synthetic" — a generated 2 s exponentially decaying stereo hall
/// (useful before any IR file exists; also the fallback demo room).
fn load_impulse_response(
    spec: &str,
    set_path: &Path,
    device_rate: f32,
) -> Result<aristide_engine::reverb::PreparedIr> {
    if spec.eq_ignore_ascii_case("synthetic") {
        let frames = (2.0 * device_rate) as usize;
        let mut rng = 0x1357_9BDFu32;
        let mut noise = move || {
            rng ^= rng << 13;
            rng ^= rng >> 17;
            rng ^= rng << 5;
            (rng >> 8) as f32 / (1u32 << 24) as f32 - 0.5
        };
        let mut data = Vec::with_capacity(frames * 2);
        for i in 0..frames {
            let t = i as f32 / device_rate;
            // ~1.4 s RT60; highs die faster via a crude progressive tilt.
            let envelope = (-t * 4.9).exp() * (1.0 - (-t * 60.0).exp());
            data.push(noise() * envelope);
            data.push(noise() * envelope);
        }
        return aristide_engine::reverb::PreparedIr::prepare(&data, 2, device_rate, device_rate)
            .map_err(|e| anyhow::anyhow!(e));
    }
    let ir_path = set_path
        .parent()
        .unwrap_or(Path::new(""))
        .join(spec);
    let file = aristide_formats::wav::read(&ir_path)
        .map_err(|e| anyhow::anyhow!("{}: {e}", ir_path.display()))?;
    aristide_engine::reverb::PreparedIr::prepare(
        &file.samples,
        file.info.channels,
        file.info.sample_rate as f32,
        device_rate,
    )
    .map_err(|e| anyhow::anyhow!(e))
}

/// Every source path's contribution before assembly: the organs and
/// sidecars themselves, plus whatever a lone composite definition
/// contributed (MIDI wiring, provenance, tuning, layout) — populated
/// only when it was the sole path.
struct Sources {
    sources: Vec<(String, Organ)>,
    sidecars: Vec<aristide_formats::sidecar::Sidecar>,
    composite_midi: Option<(PathBuf, instrument::MidiDef)>,
    single_provenance: std::collections::HashMap<StopId, instrument::StopProvenance>,
    stop_labels: std::collections::HashMap<StopId, String>,
    manual_tuning_defs: Vec<instrument::ManualTuningDef>,
    source_tuning_defs:
        std::collections::BTreeMap<String, aristide_formats::sidecar::TuningOverride>,
    rank_sources: Vec<String>,
    console_order: std::collections::BTreeMap<String, Vec<String>>,
    console_layout: std::collections::BTreeMap<String, instrument::PanelPos>,
    console_coupled_keys: bool,
    coupler_key_modes: std::collections::BTreeMap<String, String>,
}

/// A non-composite sample set: parsed as-is, its sidecar loaded (or
/// defaulted). A sidecar `name` renames the organ before anything
/// downstream sees it — that name is also the key MIDI assignments
/// are stored under.
fn load_plain_source(path: &Path) -> Result<(Organ, aristide_formats::sidecar::Sidecar)> {
    let mut organ = load_organ(path)?;
    let sidecar = match aristide_formats::sidecar::load_for(path) {
        Ok(Some(sidecar)) => {
            tracing::info!(
                "sidecar: {}",
                aristide_formats::sidecar::path_for(path).display()
            );
            sidecar
        }
        Ok(None) => Default::default(),
        Err(err) => {
            tracing::warn!("sidecar unreadable, ignoring: {err}");
            Default::default()
        }
    };
    // A sidecar rename: the set is read as-is, the name the player
    // gave it lives beside it. The name is also the key MIDI
    // assignments are stored under, so it applies before anything
    // downstream sees the organ.
    let renamed = sidecar.name.trim();
    if !renamed.is_empty() {
        organ.name = renamed.to_string();
    }
    Ok((organ, sidecar))
}

/// Load and label every source path: a sample set with its sidecar, or
/// a composite definition (`.toml`), which assembles first and then
/// acts like any other source. Per-set decisions — sidecar couplers,
/// the default registration, channel suggestions — resolve against
/// each source's own names here, then ride the id maps across.
fn load_sources(
    paths: &[PathBuf],
    progress: &dyn Fn(String),
    setup: &mut Setup,
    load_warnings: &mut Vec<String>,
) -> Result<Sources> {
    let mut composite_midi: Option<(PathBuf, instrument::MidiDef)> = None;
    let mut single_provenance: std::collections::HashMap<StopId, instrument::StopProvenance> =
        std::collections::HashMap::new();
    let mut stop_labels: std::collections::HashMap<StopId, String> =
        std::collections::HashMap::new();
    let mut manual_tuning_defs: Vec<instrument::ManualTuningDef> = Vec::new();
    let mut source_tuning_defs: std::collections::BTreeMap<
        String,
        aristide_formats::sidecar::TuningOverride,
    > = std::collections::BTreeMap::new();
    // Per assembled rank: the alias of the set it came from.
    let mut rank_sources: Vec<String> = Vec::new();
    let mut console_order: std::collections::BTreeMap<String, Vec<String>> =
        std::collections::BTreeMap::new();
    let mut console_layout: std::collections::BTreeMap<String, instrument::PanelPos> =
        std::collections::BTreeMap::new();
    let mut console_coupled_keys = true;
    let mut coupler_key_modes: std::collections::BTreeMap<String, String> =
        std::collections::BTreeMap::new();

    let mut sources: Vec<(String, Organ)> = Vec::new();
    let mut sidecars = Vec::new();
    for path in paths {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.display().to_string());
        progress(format!("reading {name}…"));
        let (mut organ, sidecar) = if instrument::is_definition(path) {
            let assembled = instrument::load(path)
                .with_context(|| format!("loading {}", path.display()))?;
            for warning in &assembled.warnings {
                tracing::warn!("instrument: {warning}");
            }
            load_warnings.extend(assembled.warnings.iter().cloned());
            tracing::info!(
                "instrument: {} ({} manuals, {} stops) from {}",
                assembled.organ.name,
                assembled.organ.manuals.len(),
                assembled.organ.stops.len(),
                path.display()
            );
            if paths.len() == 1 {
                composite_midi = Some((path.clone(), assembled.midi));
                setup.adopted = assembled.adopted;
                // Assembled stops are ids in placement order, so the
                // provenance vec zips onto them by index.
                single_provenance = assembled
                    .provenance
                    .into_iter()
                    .enumerate()
                    .map(|(index, prov)| (StopId(index as u32), prov))
                    .collect();
                stop_labels = assembled
                    .pitch_labels
                    .into_iter()
                    .enumerate()
                    .filter_map(|(index, label)| Some((StopId(index as u32), label?)))
                    .collect();
                manual_tuning_defs = assembled.manual_tuning;
                source_tuning_defs = assembled.source_tuning;
                rank_sources = assembled.rank_sources;
                console_layout = assembled.console_layout;
                console_order = assembled.console_order;
                console_coupled_keys = assembled.console_coupled_keys.unwrap_or(true);
                coupler_key_modes = assembled.console_coupler_keys;
            } else if !assembled.midi.inputs.is_empty()
                || !assembled.midi.controls.is_empty()
                || !assembled.manual_tuning.is_empty()
            {
                tracing::warn!(
                    "{}: [midi] wiring and per-manual tuning apply only when \
                     the organ is loaded alone — ignored",
                    path.display()
                );
            }
            (assembled.organ, assembled.sidecar)
        } else {
            load_plain_source(path)?
        };
        // User-defined couplers join the source's own on the rail; a
        // composite's resolve against its assembled console the same way.
        let (custom, warnings) =
            aristide_formats::sidecar::resolve_couplers(&organ, &sidecar.couplers.define);
        for warning in warnings {
            tracing::warn!("sidecar couplers: {warning}");
            load_warnings.push(format!("sidecar couplers: {warning}"));
        }
        if !custom.is_empty() {
            tracing::info!(
                "sidecar couplers: {}",
                custom
                    .iter()
                    .map(|c| c.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            );
            organ.couplers.extend(custom);
        }
        // The label suffixed onto colliding names when sources combine;
        // the same set twice must stay tellable apart.
        let mut label = organ.name.clone();
        let mut nth = 2;
        while sources.iter().any(|(l, _)| *l == label) {
            label = format!("{} {nth}", organ.name);
            nth += 1;
        }
        setup
            .sources
            .push((label.clone(), path.canonicalize().unwrap_or_else(|_| path.clone())));
        sources.push((label, organ));
        sidecars.push(sidecar);
    }

    Ok(Sources {
        sources,
        sidecars,
        composite_midi,
        single_provenance,
        stop_labels,
        manual_tuning_defs,
        source_tuning_defs,
        rank_sources,
        console_order,
        console_layout,
        console_coupled_keys,
        coupler_key_modes,
    })
}

/// With CLI `--stops` the patterns match the whole combined organ;
/// each sidecar's default registration instead means its own set's
/// stops and nothing else's. Channel suggestions are always per source.
fn register_and_suggest(
    sources: &[(String, Organ)],
    sidecars: &[aristide_formats::sidecar::Sidecar],
    stops: &[String],
) -> (Vec<Vec<StopId>>, Vec<Vec<Option<u8>>>) {
    let per_source_drawn: Vec<Vec<StopId>> = if stops.is_empty() {
        sources
            .iter()
            .zip(sidecars)
            .map(|((_, organ), sidecar)| {
                choose_registration(organ, &sidecar.registration.default)
            })
            .collect()
    } else {
        Vec::new()
    };
    let per_source_suggested: Vec<Vec<Option<u8>>> = sources
        .iter()
        .zip(sidecars)
        .map(|((_, organ), sidecar)| suggested_channels(organ, &sidecar.midi.channels))
        .collect();
    (per_source_drawn, per_source_suggested)
}

/// The mutable state `assemble_organ` updates as it resolves the
/// combined organ: each rank's owning set, each stop's provenance, and
/// its pitch label, kept as one bundle so the function stays under the
/// usual argument count.
struct SourceExtras<'a> {
    rank_sources: &'a mut Vec<String>,
    single_provenance: &'a mut std::collections::HashMap<StopId, instrument::StopProvenance>,
    stop_labels: &'a mut std::collections::HashMap<StopId, String>,
}

/// One source stands as itself; several become an implicit composite
/// — assembled exactly as a definition file with sources and no pulls
/// would be, every manual and coupler as its own set provides.
/// Engine-wide settings (wind, tremulant, reverb, tuning…) come from
/// the first source's sidecar, resolved by the caller.
fn assemble_organ(
    mut sources: Vec<(String, Organ)>,
    per_source_drawn: Vec<Vec<StopId>>,
    stops: &[String],
    setup: &mut Setup,
    extras: &mut SourceExtras,
    load_warnings: &mut Vec<String>,
) -> Result<(Organ, Vec<StopId>)> {
    Ok(if sources.len() == 1 {
        let (label, organ) = sources.pop().expect("one source");
        // A composite already reported where each stop came from; a
        // bare set standing as itself is its own provenance — each
        // stop from its own manual, as a division pull would record it.
        if extras.rank_sources.is_empty() {
            *extras.rank_sources = vec![label.clone(); organ.ranks.len()];
        }
        if extras.single_provenance.is_empty() {
            *extras.single_provenance = organ
                .stops
                .iter()
                .map(|stop| {
                    (
                        stop.id,
                        instrument::StopProvenance {
                            source: label.clone(),
                            source_manual: organ
                                .manuals
                                .iter()
                                .find(|m| m.id == stop.manual)
                                .map(|m| m.name.clone())
                                .unwrap_or_default(),
                            source_stop: stop.name.clone(),
                            // The adoption inventory pulls stop by
                            // stop, so edits look for [[stop]] lines.
                            via_division: false,
                        },
                    )
                })
                .collect();
        }
        setup.pulls = organ
            .manuals
            .iter()
            .enumerate()
            .map(|(index, manual)| (0, manual.name.clone(), index))
            .collect();
        let drawn = if stops.is_empty() {
            per_source_drawn.into_iter().next().unwrap_or_default()
        } else {
            choose_registration(&organ, stops)
        };
        (organ, drawn)
    } else {
        let implicit = instrument::Definition {
            name: sources
                .iter()
                .map(|(label, _)| label.as_str())
                .collect::<Vec<_>>()
                .join(" + "),
            ..Default::default()
        };
        let assembled = instrument::assemble(&implicit, &sources, Vec::new())
            .map_err(|e| anyhow::anyhow!("combining sets: {e}"))?;
        for warning in &assembled.warnings {
            tracing::warn!("instrument: {warning}");
        }
        load_warnings.extend(assembled.warnings.iter().cloned());
        tracing::info!(
            "composite: {} ({} manuals) — engine settings from the first \
             set's sidecar",
            assembled.organ.name,
            assembled.organ.manuals.len()
        );
        let groups = assembled
            .organ
            .windchests
            .iter()
            .map(|c| c.number)
            .max()
            .unwrap_or(0);
        if groups as usize > aristide_engine::wind::MAX_WIND_GROUPS {
            tracing::warn!(
                "composite spans {groups} windchests; the engine models {} — \
                 the rest share the last wind group",
                aristide_engine::wind::MAX_WIND_GROUPS
            );
        }
        setup.pulls = assembled.division_pulls.clone();
        setup.implicit = true;
        *extras.rank_sources = assembled.rank_sources.clone();
        *extras.single_provenance = assembled
            .provenance
            .iter()
            .cloned()
            .enumerate()
            .map(|(index, prov)| (StopId(index as u32), prov))
            .collect();
        *extras.stop_labels = assembled
            .pitch_labels
            .iter()
            .cloned()
            .enumerate()
            .filter_map(|(index, label)| Some((StopId(index as u32), label?)))
            .collect();
        let stop_map = &assembled.stop_map;
        let drawn = if stops.is_empty() {
            per_source_drawn
                .iter()
                .enumerate()
                .flat_map(|(source, ids)| {
                    ids.iter().filter_map(move |id| stop_map.get(&(source, *id)))
                })
                .copied()
                .collect()
        } else {
            choose_registration(&assembled.organ, stops)
        };
        (assembled.organ, drawn)
    })
}

/// Decode `organ`'s samples for the engine, honoring the sidecar's bit
/// depth and on-disk cache. `progress` reports the current phase for a
/// UI watching the load.
fn decode_samples(
    organ: &Organ,
    sidecar: &aristide_formats::sidecar::Sidecar,
    paths: &[PathBuf],
    sample_rate: f32,
    progress: &dyn Fn(String),
) -> Result<bank::LoadedBank> {
    progress(format!("decoding samples for {}…", organ.name));
    let started = Instant::now();
    let sample_bits = match sidecar.samples.bits {
        16 | 32 => sidecar.samples.bits,
        other => {
            tracing::warn!("[samples] bits = {other} is not 16 or 32; using 16");
            16
        }
    };
    // One cache file per (source paths, residency) combination, under
    // the user config; no config dir (or `[samples] cache = false`)
    // means no cache and nothing else changes.
    let cache_path = if sidecar.samples.cache {
        crate::config::cache_dir().map(|dir| {
            use std::hash::{Hash, Hasher};
            let mut hasher = std::hash::DefaultHasher::new();
            let mut keys: Vec<String> = paths
                .iter()
                .map(|p| {
                    p.canonicalize()
                        .unwrap_or_else(|_| p.clone())
                        .to_string_lossy()
                        .into_owned()
                })
                .collect();
            keys.sort();
            keys.hash(&mut hasher);
            sample_bits.hash(&mut hasher);
            dir.join(format!("{:016x}.samples", hasher.finish()))
        })
    } else {
        None
    };
    let loaded = bank::build(organ, sample_rate, sample_bits, cache_path.as_deref())?;
    tracing::info!(
        "samples: {} files, {:.1} MiB resident, {} skipped, in {:.1?}",
        loaded.bank.len(),
        loaded.bank.resident_bytes() as f64 / (1024.0 * 1024.0),
        loaded.skipped.len(),
        started.elapsed()
    );
    for note in loaded.skipped.iter().take(10) {
        tracing::warn!("skipped: {note}");
    }
    Ok(loaded)
}

/// The wind model from the sidecar's `[wind]` table, cents-based knobs
/// converted to the engine's pressure-domain parameters.
fn configure_wind(
    sidecar: &aristide_formats::sidecar::Sidecar,
) -> Option<aristide_engine::wind::WindParams> {
    let defaults = aristide_engine::wind::WindParams::default();
    let kp = defaults.pitch_exponent as f64;
    // sag_cents is what the user hears; invert P^kp to pressure.
    let sag_cents = sidecar.wind.sag_cents.clamp(0.0, 50.0);
    Some(aristide_engine::wind::WindParams {
        sag_depth: (1.0 - cents_to_ratio(-sag_cents / kp)) as f32,
        natural_hz: sidecar.wind.bounce_hz.clamp(0.5, 12.0) as f32,
        damping: sidecar.wind.damping.clamp(0.2, 1.5) as f32,
        flow_noise: (sidecar.wind.flow_noise_percent / 100.0).clamp(0.0, 0.1) as f32,
        ..defaults
    })
}

/// Tremulants, ready for the engine. Precedence: a hand-written
/// sidecar `[tremulant]` replaces everything (explicit override); else
/// the set's own `[Tremulant]` definitions each become an engageable
/// control on their member chests; else the historical fallback — one
/// default tremulant over every chest, so the `tremulant` binding
/// always means something.
fn configure_tremulants(
    sidecar: &aristide_formats::sidecar::Sidecar,
    organ: &Organ,
) -> Vec<TremulantSetup> {
    let defaults = aristide_engine::wind::WindParams::default();
    let kp = defaults.pitch_exponent as f64;
    let max_groups = aristide_engine::wind::MAX_WIND_GROUPS as u32;
    let group_of = |chest: u32| chest.saturating_sub(1).min(max_groups - 1) as u8;
    let sidecar_setup = |declared: &aristide_formats::sidecar::Tremulant| {
        // Pitch cents → pressure swing through the pitch exponent.
        let depth_cents = declared.depth_cents.clamp(0.0, 30.0);
        TremulantSetup {
            name: "Tremulant".to_string(),
            wave: false,
            params: aristide_engine::wind::TremulantParams {
                rate_hz: declared.rate_hz.clamp(0.5, 12.0) as f32,
                depth: (cents_to_ratio(depth_cents / kp) - 1.0) as f32,
                ramp_seconds: declared.ramp_s.clamp(0.05, 3.0) as f32,
                wobble: (declared.wobble_pct / 100.0).clamp(0.0, 0.25) as f32,
            },
            groups: if declared.chests.is_empty() {
                (0..max_groups as u8).collect()
            } else {
                declared.chests.iter().map(|&c| group_of(c)).collect()
            },
        }
    };
    let tremulants: Vec<TremulantSetup> = match (&sidecar.tremulant, organ.tremulants.len()) {
        (Some(declared), _) => vec![sidecar_setup(declared)],
        (None, 0) => vec![sidecar_setup(&Default::default())],
        (None, _) => organ
            .tremulants
            .iter()
            .enumerate()
            .map(|(index, tremulant)| {
                let groups: Vec<u8> = organ
                    .windchests
                    .iter()
                    .filter(|chest| chest.tremulants.contains(&(index as u32)))
                    .map(|chest| group_of(chest.number))
                    .collect();
                match tremulant.kind {
                    aristide_model::TremulantKind::Synth {
                        period_ms,
                        amp_mod_depth_percent,
                        start_rate,
                        stop_rate,
                    } => {
                        // GO's AmpModDepth is percent amplitude swing;
                        // our depth is a pressure swing the engine maps
                        // to gain as P^gain_exponent — invert that so
                        // the author's amplitude depth comes out, and
                        // FM/brightness follow physically.
                        let kg = aristide_engine::wind::WindParams::default().gain_exponent
                            as f64;
                        let amplitude = 1.0 + (amp_mod_depth_percent / 100.0).min(0.9);
                        // GO ramps: 1/StartRate s up, 1/StopRate s down;
                        // one engine knob, so split the difference.
                        let ramp = 0.5 * (1.0 / start_rate as f64 + 1.0 / stop_rate as f64);
                        TremulantSetup {
                            name: tremulant.name.clone(),
                            wave: false,
                            params: aristide_engine::wind::TremulantParams {
                                rate_hz: (1000.0 / period_ms).clamp(0.5, 12.0) as f32,
                                depth: (amplitude.powf(1.0 / kg) - 1.0) as f32,
                                ramp_seconds: ramp.clamp(0.05, 2.0) as f32,
                                ..Default::default()
                            },
                            groups,
                        }
                    }
                    aristide_model::TremulantKind::Wave => TremulantSetup {
                        name: tremulant.name.clone(),
                        wave: true,
                        params: Default::default(),
                        groups,
                    },
                }
            })
            .collect(),
    };
    for setup in &tremulants {
        if setup.groups.is_empty() {
            tracing::warn!(
                "tremulant {:?}: no windchest references it — engaging it will do nothing",
                setup.name
            );
        }
    }
    tremulants
}

/// One engine box per ODF enclosure: floor from the set's
/// `AmpMinimumLevel` unless the sidecar overrides, filter and inertia
/// constants from the sidecar. Also resolves the expression-pedal CC.
fn configure_enclosures(
    sidecar: &aristide_formats::sidecar::Sidecar,
    organ: &Organ,
) -> (Vec<(u8, aristide_engine::enclosure::EnclosureParams)>, u8) {
    let boxes = &sidecar.enclosures;
    let expression_cc = boxes.cc.min(119);
    let mut enclosures = Vec::new();
    for (index, enclosure) in organ
        .enclosures
        .iter()
        .enumerate()
        .take(aristide_engine::enclosure::MAX_ENCLOSURES)
    {
        let floor_db = if boxes.floor_db < 0.0 {
            boxes.floor_db.max(-40.0)
        } else {
            // GO: AmpMinimumLevel % linear amplitude closed. Clamp at
            // −40 dB (a 0 would be −∞; measured real boxes span 10–20
            // dB broadband).
            20.0 * (enclosure.amp_minimum_level / 100.0).max(0.01).log10()
        };
        enclosures.push((
            index as u8,
            aristide_engine::enclosure::EnclosureParams {
                floor_db: floor_db as f32,
                shelf_db: boxes.shelf_db.clamp(-40.0, 0.0) as f32,
                corner_open_hz: boxes.corner_open_hz.clamp(100.0, 20_000.0) as f32,
                corner_closed_hz: boxes.corner_closed_hz.clamp(100.0, 20_000.0) as f32,
                taper: boxes.taper.clamp(0.2, 5.0) as f32,
                full_sweep_s: boxes.full_sweep_s.clamp(0.0, 5.0) as f32,
            },
        ));
    }
    if organ.enclosures.len() > aristide_engine::enclosure::MAX_ENCLOSURES {
        tracing::warn!(
            "set defines {} enclosures; engine tracks the first {}",
            organ.enclosures.len(),
            aristide_engine::enclosure::MAX_ENCLOSURES
        );
    }
    (enclosures, expression_cc)
}

/// The sidecar's `[reverb]`, loaded into an engine-ready impulse
/// response. A missing or unreadable IR disables reverb with a
/// warning rather than failing the load.
fn configure_reverb(
    sidecar: &aristide_formats::sidecar::Sidecar,
    first_path: &Path,
    sample_rate: f32,
) -> Option<(Arc<aristide_engine::reverb::PreparedIr>, f32)> {
    if sidecar.reverb.ir.is_empty() {
        return None;
    }
    let wet = sidecar.reverb.wet.clamp(0.0, 2.0) as f32;
    match load_impulse_response(&sidecar.reverb.ir, first_path, sample_rate) {
        Ok(ir) => {
            tracing::info!(
                "reverb: {} ({} partitions), wet {:.2}",
                sidecar.reverb.ir,
                ir.partition_count(),
                wet
            );
            Some((Arc::new(ir), wet))
        }
        Err(err) => {
            tracing::warn!("reverb disabled: {err}");
            None
        }
    }
}

/// The instrument's recorded home tuning, logged and installed on the
/// console, plus each source's own recorded pitch (its rank anchors'
/// median) when more than one scope could plausibly want its own.
fn install_home_tuning(
    console: &mut Console,
    home: Option<Arc<tuning::HomeTuning>>,
    rank_anchors: &std::collections::HashMap<aristide_model::RankId, f64>,
    rank_sources: &[String],
    single_provenance: &std::collections::HashMap<StopId, instrument::StopProvenance>,
    sources_len: usize,
    source_tuning_defs_len: usize,
) -> Option<Arc<tuning::HomeTuning>> {
    match &home {
        Some(home) => tracing::info!(
            "recorded tuning: a′ = {:.1} Hz, {} (±{:.1} cents over {} of {} pipes)",
            home.a4_hz,
            match home.temperament {
                Some(t) => t.name().to_string(),
                None => "an unequal temperament the tables don't name".to_string(),
            },
            home.spread_cents,
            home.measured,
            home.pipes
        ),
        None => tracing::info!("recorded tuning: no pipe measured; assuming a′ = 440 equal"),
    }
    console.set_home(home.clone());
    // Each set's own recorded pitch: the instrument's class table at
    // the median anchor of the set's ranks — so "as recorded" at set
    // scope names the 415 the Positif was sampled at, not the
    // instrument's blend of 415 and 440. One set is the instrument.
    let source_homes: std::collections::HashMap<String, Arc<tuning::HomeTuning>> = match &home {
        Some(home) if sources_len > 1 || source_tuning_defs_len > 1 => {
            let mut per_source: std::collections::HashMap<String, Vec<f64>> =
                std::collections::HashMap::new();
            for (rank, anchor) in rank_anchors {
                if let Some(alias) = rank_sources.get(rank.0 as usize) {
                    per_source.entry(alias.clone()).or_default().push(*anchor);
                }
            }
            per_source
                .into_iter()
                .filter_map(|(alias, mut anchors)| {
                    tuning::median(&mut anchors)
                        .map(|anchor| (alias, Arc::new(home.at_anchor(anchor))))
                })
                .collect()
        }
        _ => Default::default(),
    };
    console.set_scope_homes(source_homes, rank_anchors.clone());
    console.set_stop_sources(
        single_provenance
            .iter()
            .map(|(id, prov)| (*id, prov.source.clone()))
            .collect(),
    );
    home
}

/// A resolved scope tuning from a base tuning plus a file's
/// `TuningOverride` fields, given the scope's own recorded home (if
/// any). Shared by every scope that tunes apart from what it follows:
/// manuals, sets, stops, and ranks.
type OverrideTuning<'a> = &'a dyn Fn(
    &tuning::Tuning,
    &aristide_formats::sidecar::TuningOverride,
    Option<Arc<tuning::HomeTuning>>,
    &str,
) -> tuning::Tuning;

/// A one-line human description of a tuning, for log lines.
type DescribeTuning<'a> = &'a dyn Fn(&tuning::Tuning) -> String;

/// Divisions the definition tunes apart from the rest ([[manual]]
/// tuning in a composite file): missing fields follow the
/// instrument-wide tuning.
fn apply_manual_tuning(
    console: &mut Console,
    defs: &[instrument::ManualTuningDef],
    live_tuning: &tuning::Tuning,
    override_tuning: OverrideTuning,
    describe: DescribeTuning,
) {
    for def in defs {
        let fields = aristide_formats::sidecar::TuningOverride {
            temperament: def.temperament.clone(),
            edo: def.edo,
            reference_key: def.reference_key.clone(),
            reference_hz: def.reference_hz,
            scale: def.scale.clone(),
            keymap: def.keymap.clone(),
            pipes: def.pipes.clone(),
        };
        let mut own = override_tuning(live_tuning, &fields, None, "manual tuning");
        own.transpose = def.transpose.unwrap_or(live_tuning.transpose).clamp(-12, 12);
        tracing::info!(
            "tuning: manual {} plays {}, transpose {:+}",
            def.manual,
            describe(&own),
            own.transpose
        );
        console.set_manual_tuning(def.manual, Some(own));
    }
}

/// Sets tuned apart ([sources.<alias>.tuning]): what their stops play
/// unless a division of their own or a pin says otherwise.
fn apply_source_tuning(
    console: &mut Console,
    defs: &std::collections::BTreeMap<String, aristide_formats::sidecar::TuningOverride>,
    live_tuning: &tuning::Tuning,
    override_tuning: OverrideTuning,
    describe: DescribeTuning,
) {
    for (alias, def) in defs {
        let own = override_tuning(
            live_tuning,
            def,
            console.source_home_of(alias),
            &format!("source {alias:?} tuning"),
        );
        tracing::info!("tuning: set {alias:?} plays {}", describe(&own));
        console.set_source_tuning(alias, Some(own));
    }
}

/// Stops pinned or tuned apart, and ranks tuned apart within them
/// ([[tuning.stop]]): rows name console stops (and manuals) as the
/// console spells them; a row naming nothing warns and does nothing.
fn apply_stop_tuning(
    console: &mut Console,
    rows: &[aristide_formats::sidecar::StopTuningDef],
    override_tuning: OverrideTuning,
    describe: DescribeTuning,
    load_warnings: &mut Vec<String>,
) {
    let stop_rows: Vec<(StopId, String, String)> = console
        .stop_states()
        .iter()
        .map(|(id, name, manual, ..)| (*id, name.to_string(), manual.to_string()))
        .collect();
    for row in rows {
        let matches: Vec<StopId> = stop_rows
            .iter()
            .filter(|(_, name, manual)| {
                name.eq_ignore_ascii_case(&row.stop)
                    && row.manual.as_deref().is_none_or(|m| m.eq_ignore_ascii_case(manual))
            })
            .map(|(id, ..)| *id)
            .collect();
        if matches.is_empty() {
            let note = format!(
                "tuning.stop: {:?}{} names no stop",
                row.stop,
                row.manual.as_deref().map(|m| format!(" on {m:?}")).unwrap_or_default()
            );
            tracing::warn!("{note}");
            load_warnings.push(note);
            continue;
        }
        for stop in matches {
            let fields = row.tuning();
            if let Some(rank_name) = &row.rank {
                let Some((rank, _)) = console
                    .stop_ranks(stop)
                    .into_iter()
                    .find(|(_, name)| name.eq_ignore_ascii_case(rank_name))
                else {
                    let note = format!("tuning.stop: {:?} has no rank {rank_name:?}", row.stop);
                    tracing::warn!("{note}");
                    load_warnings.push(note);
                    continue;
                };
                let base = console.stop_tuning_resolved(stop).0.clone();
                let own = override_tuning(
                    &base,
                    &fields,
                    console.rank_home_of(rank),
                    &format!("rank {rank_name:?} of {:?}", row.stop),
                );
                tracing::info!("tuning: {:?} rank {rank_name:?} plays {}", row.stop, describe(&own));
                console.set_rank_tuning(stop, rank, Some(own));
                continue;
            }
            match row.follow.as_deref() {
                Some(name) => match tuning::Follow::parse(name) {
                    Some(follow) => console.set_stop_follow(stop, follow),
                    None => {
                        let note = format!(
                            "tuning.stop: {:?} follow = {name:?} is none of auto, division, \
                             source, organ",
                            row.stop
                        );
                        tracing::warn!("{note}");
                        load_warnings.push(note);
                    }
                },
                None => {
                    let base = console.stop_tuning_resolved(stop).0.clone();
                    let own = override_tuning(
                        &base,
                        &fields,
                        console.source_home_of(console.stop_source(stop).unwrap_or_default()),
                        &format!("stop {:?} tuning", row.stop),
                    );
                    tracing::info!("tuning: stop {:?} plays {}", row.stop, describe(&own));
                    console.set_stop_tuning(stop, Some(own));
                }
            }
        }
    }
}

/// The instrument's tuning, resolved from the sidecar's `[tuning]`
/// table against the home tuning, then applied to every scope that
/// tunes apart from it: manuals, sets, and individual stops/ranks.
/// Returns the instrument-wide tuning the console now plays.
fn configure_tuning(
    console: &mut Console,
    sidecar: &aristide_formats::sidecar::Sidecar,
    home: Option<Arc<tuning::HomeTuning>>,
    first_path: &Path,
    manual_tuning_defs: &[instrument::ManualTuningDef],
    source_tuning_defs: &std::collections::BTreeMap<String, aristide_formats::sidecar::TuningOverride>,
    load_warnings: &mut Vec<String>,
) -> tuning::Tuning {
    let temperament = tuning::Temperament::parse(&sidecar.tuning.temperament)
        .unwrap_or_else(|| {
            tracing::warn!(
                "sidecar tuning: unknown temperament {:?}, playing as recorded",
                sidecar.tuning.temperament
            );
            tuning::Temperament::Original
        });
    // Scale files resolve against the organ's own directory: the file
    // that names them. A scale that fails to load warns and leaves the
    // temperament standing — a missing .scl must not brick the organ.
    let scale_base = first_path.parent().map(std::path::Path::to_path_buf);
    let load_scale = |scl: &str, kbm: Option<&str>, reference: tuning::PitchReference| {
        match tuning::ScaleTuning::load(scl, kbm, reference, scale_base.as_deref()) {
            Ok(scale) => Some(std::sync::Arc::new(scale)),
            Err(err) => {
                tracing::warn!("tuning scale not loaded: {err} — keeping the temperament");
                None
            }
        }
    };
    let edo = sidecar
        .tuning
        .edo
        .clamp(*tuning::EDO_RANGE.start(), *tuning::EDO_RANGE.end());
    // The anchor the file leaves unsaid: the organ's own pitch on the
    // reference key when it plays as recorded (a 415 set reads "A4 =
    // 415.3", not a 440 it never sounded), the equal ladder's under a
    // target.
    let anchor_key = reference_key(&sidecar.tuning.reference_key, None);
    let unsaid_reference = |temperament: tuning::Temperament, edo: u16, scale: bool| {
        let as_recorded = temperament == tuning::Temperament::Original && edo == 12 && !scale;
        match &home {
            Some(home) if as_recorded => home.reference(anchor_key),
            _ => tuning::PitchReference {
                key: anchor_key,
                hz: 440.0 * ((anchor_key as f64 - 69.0) / 12.0).exp2(),
            },
        }
    };
    let mut live_tuning = tuning::Tuning {
        temperament,
        edo,
        scale: None,
        reference: match sidecar.tuning.reference_hz {
            Some(hz) => tuning::PitchReference { key: anchor_key, hz },
            None => unsaid_reference(temperament, edo, sidecar.tuning.scale.is_some()),
        }
        .clamped(),
        transpose: sidecar.tuning.transpose.clamp(-12, 12),
        pipes: parse_pipes(&sidecar.tuning.pipes).unwrap_or_default(),
        home: home.clone(),
    };
    if let Some(scl) = &sidecar.tuning.scale {
        live_tuning.scale = load_scale(
            scl,
            sidecar.tuning.keymap.as_deref(),
            live_tuning.reference,
        );
    }
    console.set_tuning(live_tuning.clone());
    // A scope's own tuning from its file fields: every missing field
    // is the tuning it would otherwise play (`base`), so a row saying
    // only `temperament = "meantone4"` keeps the pitch it had. An
    // as-recorded override at a scope with a home of its own (a set, a
    // rank) and no reference of its own sits at *that* home's pitch —
    // a 415 set marked `original` inside a 440 instrument plays at 415.
    let override_tuning = |base: &tuning::Tuning,
                           def: &aristide_formats::sidecar::TuningOverride,
                           scope_home: Option<Arc<tuning::HomeTuning>>,
                           what: &str| {
        let temperament = def
            .temperament
            .as_deref()
            .map(|name| {
                tuning::Temperament::parse(name).unwrap_or_else(|| {
                    tracing::warn!("{what}: unknown temperament {name:?}, keeping the tuning above");
                    base.temperament
                })
            })
            .unwrap_or(base.temperament);
        let edo = def
            .edo
            .unwrap_or(base.edo)
            .clamp(*tuning::EDO_RANGE.start(), *tuning::EDO_RANGE.end());
        let scale_named = def.scale.is_some() || (def.temperament.is_none() && def.edo.is_none() && base.scale.is_some());
        let as_recorded = temperament == tuning::Temperament::Original && edo == 12 && !scale_named;
        let key = def
            .reference_key
            .as_ref()
            .map_or(base.reference.key, |key| reference_key(key, Some(base.reference.key)));
        let hz = def.reference_hz.unwrap_or_else(|| match (&scope_home, as_recorded) {
            (Some(home), true) => home.reference(key).hz,
            _ => base.reference.hz,
        });
        let mut own = tuning::Tuning {
            temperament,
            edo,
            scale: None,
            reference: tuning::PitchReference { key, hz }.clamped(),
            transpose: base.transpose,
            pipes: def.pipes.as_deref().and_then(parse_pipes).unwrap_or(base.pipes),
            home: scope_home.or_else(|| home.clone()),
        };
        match &def.scale {
            Some(scl) => own.scale = load_scale(scl, def.keymap.as_deref(), own.reference),
            // Naming a temperament or a count leaves the scale above.
            None if def.temperament.is_none() && def.edo.is_none() => {
                own.scale = base.scale.clone();
            }
            None => {}
        }
        own
    };
    let describe = |tuning: &tuning::Tuning| {
        format!(
            "{} @ {}={} Hz",
            match &tuning.scale {
                Some(scale) => scale.name().to_string(),
                None if tuning.edo != 12 => format!("{}-EDO", tuning.edo),
                None => tuning.temperament.name().to_string(),
            },
            aristide_formats::sidecar::note_name(tuning.reference.key),
            tuning.reference.hz
        )
    };
    apply_manual_tuning(console, manual_tuning_defs, &live_tuning, &override_tuning, &describe);
    apply_source_tuning(console, source_tuning_defs, &live_tuning, &override_tuning, &describe);
    apply_stop_tuning(console, &sidecar.tuning.stops, &override_tuning, &describe, load_warnings);
    live_tuning
}

/// Assemble `paths` (sample sets and composite definitions; several are
/// combined into one implicit composite), decode the samples, and build
/// the console. `stops` are CLI registration patterns; empty means each
/// source's sidecar default. `progress` is told what is happening, for
/// a UI that is watching the load.
pub fn prepare(
    paths: &[PathBuf],
    stops: &[String],
    sample_rate: f32,
    progress: &dyn Fn(String),
) -> Result<PreparedInstrument> {
    anyhow::ensure!(!paths.is_empty(), "no sample set given");
    let first_path = &paths[0];
    let mut setup = Setup::default();
    let mut load_warnings: Vec<String> = Vec::new();

    // Every path is a source: a sample set with its sidecar, or a
    // composite definition (`.toml`), which assembles first and then
    // acts like any other source. Per-set decisions — sidecar couplers,
    // the default registration, channel suggestions — resolve against
    // each source's own names here, then ride the id maps across.
    let Sources {
        sources: source_list,
        sidecars,
        composite_midi,
        mut single_provenance,
        mut stop_labels,
        manual_tuning_defs,
        source_tuning_defs,
        mut rank_sources,
        console_order,
        console_layout,
        console_coupled_keys,
        coupler_key_modes,
    } = load_sources(paths, progress, &mut setup, &mut load_warnings)?;

    let (per_source_drawn, per_source_suggested) =
        register_and_suggest(&source_list, &sidecars, stops);
    let suggested: Vec<Option<u8>> = per_source_suggested.concat();
    // Engine-wide settings (wind, tremulant, reverb, tuning…) come from
    // the first source.
    let sidecar = sidecars[0].clone();

    let (organ, drawn) = assemble_organ(
        source_list,
        per_source_drawn,
        stops,
        &mut setup,
        &mut SourceExtras {
            rank_sources: &mut rank_sources,
            single_provenance: &mut single_provenance,
            stop_labels: &mut stop_labels,
        },
        &mut load_warnings,
    )?;

    let loaded = decode_samples(&organ, &sidecar, paths, sample_rate, progress)?;
    let bank::LoadedBank {
        bank,
        specs,
        attack_options,
        home,
        rank_anchors,
        ..
    } = loaded;

    let wind = configure_wind(&sidecar);
    let tremulants = configure_tremulants(&sidecar, &organ);
    let (enclosures, expression_cc) = configure_enclosures(&sidecar, &organ);
    let reverb = configure_reverb(&sidecar, first_path, sample_rate);

    let mut console = Console::new(organ, specs, drawn, sample_rate);
    console.set_attack_options(attack_options);
    let home = home.map(Arc::new);
    let sources_len = setup.sources.len();
    let source_tuning_defs_len = source_tuning_defs.len();
    let home = install_home_tuning(
        &mut console,
        home,
        &rank_anchors,
        &rank_sources,
        &single_provenance,
        sources_len,
        source_tuning_defs_len,
    );
    let live_tuning = configure_tuning(
        &mut console,
        &sidecar,
        home,
        first_path,
        &manual_tuning_defs,
        &source_tuning_defs,
        &mut load_warnings,
    );
    console.set_coupler_repitch(sidecar.couplers.repitch);
    // Console names for carried couplers ([couplers.rename]): applied
    // before the drop pass, so drop entries written by the console
    // (which speak current names) mean what they say at load too.
    for (original, name) in &sidecar.couplers.rename {
        let index = console
            .coupler_states()
            .iter()
            .position(|(_, existing, _, _)| existing.eq_ignore_ascii_case(original));
        match index {
            Some(index) => {
                console.rename_coupler(index, name);
            }
            None => load_warnings.push(format!(
                "couplers.rename: no coupler named {original:?}"
            )),
        }
    }
    // Couplers this instrument takes off its console — they stay
    // restorable from the Organ preferences.
    {
        let names: Vec<String> = console
            .coupler_states()
            .iter()
            .map(|(_, name, _, _)| name.to_string())
            .collect();
        let names: Vec<&str> = names.iter().map(String::as_str).collect();
        for pattern in &sidecar.couplers.drop {
            let matches = aristide_formats::sidecar::match_names(&names, pattern);
            if matches.is_empty() {
                tracing::warn!("couplers.drop: {pattern:?} matches nothing");
            }
            for index in matches {
                tracing::info!("coupler off the console: {}", names[index]);
                console.set_coupler_available(index, false);
            }
        }
    }
    // Linked couplers ([couplers] link): engaging one engages the rest.
    // Names that don't resolve are reported and skipped — a group left
    // with one member links nothing.
    {
        let names: Vec<String> = console
            .coupler_states()
            .iter()
            .map(|(_, name, _, _)| name.to_string())
            .collect();
        let mut groups: Vec<Vec<usize>> = Vec::new();
        for group in &sidecar.couplers.link {
            let mut indices: Vec<usize> = Vec::new();
            for name in group {
                match names.iter().position(|n| n.eq_ignore_ascii_case(name)) {
                    Some(index) => indices.push(index),
                    None => {
                        load_warnings
                            .push(format!("couplers.link: no coupler named {name:?}"));
                    }
                }
            }
            indices.sort_unstable();
            indices.dedup();
            if indices.len() > 1 {
                groups.push(indices);
            }
        }
        console.set_coupler_links(groups);
    }
    console.set_noises(
        sidecar.noises.enabled,
        sidecar.noises.volume.clamp(0.0, 2.0) as f32,
    );
    // Audio routing: stops onto buses, speaking delays — resolved by
    // the same name-pattern rules couplers use. A pattern that matches
    // nothing warns to the console; it never fails the load.
    let mut buses = Vec::new();
    let mut stop_voicing: std::collections::HashMap<StopId, StopVoicing> =
        std::collections::HashMap::new();
    {
        let stop_ids: Vec<(aristide_model::StopId, String, usize)> = console
            .stop_states()
            .iter()
            .map(|(id, name, _, manual, _)| (*id, name.to_string(), *manual))
            .collect();
        let stop_names: Vec<&str> = stop_ids.iter().map(|(_, name, _)| name.as_str()).collect();
        let manual_names: Vec<String> = console
            .manual_states()
            .iter()
            .map(|(_, name, _, _, _)| name.to_string())
            .collect();
        let manual_names: Vec<&str> = manual_names.iter().map(String::as_str).collect();
        let mut plan: std::collections::HashMap<aristide_model::StopId, (u8, u32)> =
            std::collections::HashMap::new();
        for (index, def) in sidecar.routing.buses.iter().enumerate() {
            if index + 1 >= aristide_engine::routing::MAX_BUSES {
                load_warnings.push(format!(
                    "routing: at most {} buses; {:?} and beyond ignored",
                    aristide_engine::routing::MAX_BUSES - 1,
                    def.name
                ));
                break;
            }
            let bus = (index + 1) as u8;
            let mut members: Vec<aristide_model::StopId> = Vec::new();
            for pattern in &def.stops {
                let matched = aristide_formats::sidecar::match_names(&stop_names, pattern);
                if matched.is_empty() {
                    load_warnings.push(format!("routing: stop {pattern:?} matches nothing"));
                }
                members.extend(matched.iter().map(|&at| stop_ids[at].0));
            }
            for pattern in &def.manuals {
                let matched = aristide_formats::sidecar::match_names(&manual_names, pattern);
                if matched.is_empty() {
                    load_warnings.push(format!("routing: manual {pattern:?} matches nothing"));
                }
                members.extend(
                    stop_ids
                        .iter()
                        .filter(|(_, _, manual)| matched.contains(manual))
                        .map(|(id, _, _)| *id),
                );
            }
            for id in members {
                let entry = plan.entry(id).or_insert((bus, 0));
                if entry.0 != bus && entry.0 != 0 {
                    load_warnings
                        .push("routing: a stop matched two buses; keeping the first".to_string());
                } else {
                    entry.0 = bus;
                }
            }
            buses.push(BusSetup {
                bus,
                output: def
                    .output
                    .map(|[left, right]| (left.saturating_sub(1), right.saturating_sub(1))),
                gain: 10f32.powf((def.gain_db.clamp(-60.0, 20.0) as f32) / 20.0),
                delay: def.delay.as_ref().map(|delay| {
                    aristide_engine::routing::DelayParams {
                        seconds: (delay.ms.clamp(0.0, 60_000.0) / 1000.0) as f32,
                        feedback: delay.feedback as f32,
                        mix: delay.mix as f32,
                        dry: delay.dry as f32,
                    }
                }),
            });
        }
        for rule in &sidecar.voicing.delays {
            let frames = (rule.ms.clamp(0.0, 30_000.0) / 1000.0 * sample_rate as f64) as u32;
            for pattern in &rule.stops {
                let matched = aristide_formats::sidecar::match_names(&stop_names, pattern);
                if matched.is_empty() {
                    load_warnings.push(format!("voicing.delay: {pattern:?} matches nothing"));
                }
                for at in matched {
                    plan.entry(stop_ids[at].0).or_insert((0, 0)).1 = frames;
                }
            }
        }
        if !plan.is_empty() {
            console.set_stop_routing(plan);
        }
        // Voicing trims: level, cents, and footage per stop pattern.
        // A `pitch` footage becomes a cents shift against the stop's
        // own recorded footage, then rides the same fold as `cents`.
        let mut adjust: std::collections::HashMap<aristide_model::StopId, (f32, f64)> =
            std::collections::HashMap::new();
        for rule in &sidecar.voicing.adjusts {
            let gain = 10f32.powf((rule.gain_db.clamp(-40.0, 20.0) as f32) / 20.0);
            let cents = rule.cents.clamp(-2400.0, 2400.0);
            let feet = match &rule.pitch {
                None => None,
                Some(spec) => match spec.feet() {
                    Some(feet) => Some(feet),
                    None => {
                        load_warnings
                            .push(format!("voicing.adjust: {spec:?} names no footage"));
                        None
                    }
                },
            };
            for pattern in &rule.stops {
                let matched = aristide_formats::sidecar::match_names(&stop_names, pattern);
                if matched.is_empty() {
                    load_warnings.push(format!("voicing.adjust: {pattern:?} matches nothing"));
                }
                for at in matched {
                    let footage_cents = match feet {
                        None => 0.0,
                        Some(feet) => match console.stop_native_footage(stop_ids[at].0) {
                            Some(native) => cents_between(feet, native),
                            None => {
                                load_warnings.push(format!(
                                    "voicing.adjust: {:?} speaks no single footage — \
                                     pitch {feet} not applied",
                                    stop_names[at]
                                ));
                                0.0
                            }
                        },
                    };
                    let entry = adjust.entry(stop_ids[at].0).or_insert((1.0, 0.0));
                    entry.0 *= gain;
                    entry.1 += cents + footage_cents;
                }
            }
            // A rule naming exactly one stop, exactly, is that stop's
            // own — the one the console editor shows and edits. The
            // last such rule wins the mirror (the sound still gets
            // every rule).
            if let [pattern] = rule.stops.as_slice()
                && let [at] = aristide_formats::sidecar::match_names(&stop_names, pattern)
                    .as_slice()
                && stop_names[*at].eq_ignore_ascii_case(pattern)
            {
                stop_voicing.insert(
                    stop_ids[*at].0,
                    StopVoicing {
                        feet,
                        cents: rule.cents,
                        gain_db: rule.gain_db,
                    },
                );
            }
        }
        if !adjust.is_empty() {
            tracing::info!("voicing: {} stop(s) trimmed", adjust.len());
            console.set_stop_adjust(adjust);
        }
    }
    tracing::info!(
        "tuning: {} @ {}={} Hz, transpose {:+}",
        live_tuning.temperament.name(),
        aristide_formats::sidecar::note_name(live_tuning.reference.key),
        live_tuning.reference.hz,
        live_tuning.transpose
    );

    Ok(PreparedInstrument {
        console,
        bank,
        wind,
        tremulants,
        enclosures,
        expression_cc,
        reverb,
        composite: composite_midi,
        suggested_channels: suggested,
        setup,
        provenance: single_provenance,
        stop_voicing,
        stop_labels,
        stop_order: console_order,
        layout: console_layout,
        coupled_keys: console_coupled_keys,
        coupler_key_modes,
        buses,
        warnings: load_warnings,
    })
}

/// A `reference_key` as the file spells it ("C4", "F#3", or a MIDI
/// number) resolved to a key, falling back — with a warning, never a
/// failed load — to `inherited` (the instrument's anchor) or A4.
/// `pipes = "original" | "exact"`, warning (and keeping the default)
/// on anything else — a typo must not brick the organ.
fn parse_pipes(name: &str) -> Option<tuning::PipeRetune> {
    let parsed = tuning::PipeRetune::parse(name);
    if parsed.is_none() {
        tracing::warn!("tuning: unknown pipes mode {name:?}; pipes keep their drift");
    }
    parsed
}

fn reference_key(spec: &aristide_formats::sidecar::KeySpec, inherited: Option<u8>) -> u8 {
    spec.midi_note().unwrap_or_else(|| {
        let fallback = inherited.unwrap_or(tuning::PitchReference::A440.key);
        tracing::warn!(
            "tuning: reference_key {spec:?} names no key, anchoring on {}",
            aristide_formats::sidecar::note_name(fallback)
        );
        fallback
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A blank organ — the file `/api/organ/new` writes — must come out
    /// of the same pipeline as a real set: an empty console, no samples,
    /// nothing to trip over downstream.
    #[test]
    fn prepare_accepts_a_blank_composite() {
        let dir = std::env::temp_dir().join("aristide-blank-prepare-test");
        std::fs::create_dir_all(&dir).expect("fixture dir");
        let path = crate::config::create_blank_organ(&dir, "Blank Chapel").expect("creates");
        let prepared = prepare(&[path], &[], 48_000.0, &|_| {}).expect("blank organ prepares");
        assert_eq!(prepared.console.organ_name(), "Blank Chapel");
        assert!(prepared.console.stop_states().is_empty());
        assert!(prepared.bank.is_empty());
        assert!(
            prepared.composite.is_some(),
            "the blank file owns its own MIDI wiring like any composite"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The adoption invariant: a set wrapped as an organ file loads
    /// exactly as the set does directly — same console, same engine
    /// settings. If this drifts, adopting a set silently changes the
    /// organ, and the demo set must load exactly as its ODF defines.
    #[test]
    fn an_adopted_set_loads_exactly_like_the_set_itself() {
        let demo = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../testsets/grandorgue-demo/demo.organ");
        if !demo.is_file() {
            eprintln!("skipping: demo set not present");
            return;
        }
        let dir = std::env::temp_dir().join("aristide-adopt-fidelity-test");
        let _ = std::fs::remove_dir_all(&dir);

        let direct =
            prepare(std::slice::from_ref(&demo), &[], 48_000.0, &|_| {}).expect("direct load");
        let canonical = demo.canonicalize().expect("demo canonicalizes");
        let organ = load_organ(&canonical).expect("demo parses");
        let wrapper = crate::config::create_wrapper_organ(
            &dir,
            direct.console.organ_name(),
            &canonical,
            &organ,
            None,
        )
        .expect("wrapper created");
        let adopted = prepare(&[wrapper], &[], 48_000.0, &|_| {}).expect("adopted load");

        assert_eq!(adopted.console.organ_name(), direct.console.organ_name());
        let stops = |prepared: &PreparedInstrument| {
            prepared
                .console
                .stop_states()
                .iter()
                .map(|(_, name, manual, midx, drawn)| {
                    (name.to_string(), manual.to_string(), *midx, *drawn)
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(stops(&adopted), stops(&direct), "same stops on the same manuals");
        let manuals = |prepared: &PreparedInstrument| {
            prepared
                .console
                .manual_states()
                .iter()
                .map(|(idx, name, first, count, _)| (*idx, name.to_string(), *first, *count))
                .collect::<Vec<_>>()
        };
        assert_eq!(manuals(&adopted), manuals(&direct), "same manuals, same compasses");
        assert_eq!(
            format!("{:?}", adopted.console.coupler_states()),
            format!("{:?}", direct.console.coupler_states()),
            "same couplers"
        );
        assert_eq!(
            format!("{:?}", adopted.console.enclosure_states()),
            format!("{:?}", direct.console.enclosure_states()),
            "same enclosures"
        );
        assert_eq!(
            format!("{:?}", adopted.wind),
            format!("{:?}", direct.wind),
            "the sidecar's wind model survives adoption"
        );
        assert_eq!(
            format!("{:?}", adopted.tremulants),
            format!("{:?}", direct.tremulants),
            "the set's tremulants survive adoption, same chests"
        );
        // The demo's ODF Tremblant: 196 ms period ≈ 5.1 Hz, Récit
        // chest only (group 2), instead of the old sidecar default
        // sweeping every chest.
        assert_eq!(direct.tremulants.len(), 1);
        let tremblant = &direct.tremulants[0];
        assert_eq!(tremblant.name, "Tremblant");
        assert!(!tremblant.wave);
        assert_eq!(tremblant.groups, vec![2]);
        assert!((tremblant.params.rate_hz - 1000.0 / 196.0).abs() < 0.01);
        assert_eq!(
            format!("{:?}", adopted.enclosures),
            format!("{:?}", direct.enclosures)
        );
        assert_eq!(adopted.expression_cc, direct.expression_cc);
        assert_eq!(
            adopted.suggested_channels, direct.suggested_channels,
            "the sidecar's channel suggestions survive"
        );
        // The home fit is compared by its coverage: the adopted organ
        // pulls the same pipes, but the two organs list them in a
        // different order (and the fit takes medians of them).
        let strip = |tuning: tuning::Tuning| tuning::Tuning {
            home: None,
            ..tuning
        };
        assert_eq!(
            format!("{:?}", strip(adopted.console.tuning())),
            format!("{:?}", strip(direct.console.tuning()))
        );
        let coverage = |home: Option<std::sync::Arc<tuning::HomeTuning>>| {
            home.map(|home| (home.measured, home.pipes, home.temperament))
        };
        assert_eq!(
            coverage(adopted.console.home()),
            coverage(direct.console.home())
        );
        assert_eq!(adopted.console.noises(), direct.console.noises());
        assert_eq!(
            adopted.console.coupler_repitch(),
            direct.console.coupler_repitch()
        );
        assert_eq!(adopted.bank.len(), direct.bank.len(), "every sample decoded");
        assert!(adopted.composite.is_some(), "the organ file owns the wiring");
        assert!(direct.composite.is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A sidecar `name` renames the organ on load — that name is what
    /// the console shows and what the assignments are keyed under. The
    /// real demo set is symlinked into a scratch directory so it gets a
    /// sidecar of this test's own instead of the one it ships with.
    #[test]
    #[cfg(unix)]
    fn a_sidecar_rename_takes_effect_on_load() {
        let demo_dir =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../testsets/grandorgue-demo");
        if !demo_dir.join("demo.organ").is_file() {
            eprintln!("skipping: demo set not present");
            return;
        }
        let dir = std::env::temp_dir().join("aristide-sidecar-rename-load-test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch dir");
        for entry in std::fs::read_dir(&demo_dir).expect("demo listable").flatten() {
            if entry.file_name() == "demo.organ.aristide.toml" {
                continue;
            }
            std::os::unix::fs::symlink(entry.path(), dir.join(entry.file_name()))
                .expect("symlink");
        }
        std::fs::write(
            dir.join("demo.organ.aristide.toml"),
            "name = \"Église Fictive\"\n",
        )
        .expect("sidecar writes");
        let prepared = prepare(&[dir.join("demo.organ")], &[], 48_000.0, &|_| {})
            .expect("renamed set prepares");
        assert_eq!(prepared.console.organ_name(), "Église Fictive");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A set tuned apart in its `[sources]` entry, a stop pinned past
    /// it, a stop with a tuning of its own, and a rank tuned apart
    /// within a stop all install from the file.
    #[test]
    fn sets_stops_and_ranks_tune_apart_from_the_file() {
        let demo = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../testsets/grandorgue-demo/demo.organ");
        if !demo.is_file() {
            eprintln!("skipping: demo set not present");
            return;
        }
        let direct =
            prepare(std::slice::from_ref(&demo), &[], 48_000.0, &|_| {}).expect("direct load");
        let stops: Vec<(String, String, Vec<String>)> = direct
            .console
            .stop_states()
            .iter()
            .map(|(id, name, manual, ..)| {
                let ranks = direct
                    .console
                    .stop_ranks(*id)
                    .iter()
                    .map(|(_, name)| name.to_string())
                    .collect();
                (name.to_string(), manual.to_string(), ranks)
            })
            .collect();
        let manual = stops[0].1.clone();
        let on_manual: Vec<&(String, String, Vec<String>)> =
            stops.iter().filter(|(_, m, _)| *m == manual).collect();
        assert!(on_manual.len() >= 2, "the demo's first division has stops");
        let (pinned, _, _) = on_manual[0];
        let (own, _, _) = on_manual[1];
        let mixture = stops.iter().find(|(_, _, ranks)| ranks.len() > 1);

        let dir = std::env::temp_dir().join("aristide-scoped-tuning-load-test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("dir");
        let path = dir.join("scoped.toml");
        let mut text = format!(
            "name = \"Scoped demo\"\n\n[sources.demo]\npath = {:?}\nlayout = true\n\n\
             [sources.demo.tuning]\ntemperament = \"meantone4\"\n\n\
             [[division]]\nfrom = \"demo\"\nmanual = \"*\"\n\n\
             [[tuning.stop]]\nstop = {pinned:?}\nmanual = {manual:?}\nfollow = \"organ\"\n\n\
             [[tuning.stop]]\nstop = {own:?}\nmanual = {manual:?}\ntemperament = \"pythagorean\"\n\n",
            demo.canonicalize().expect("demo").display().to_string()
        );
        if let Some((name, manual, ranks)) = mixture {
            text.push_str(&format!(
                "[[tuning.stop]]\nstop = {name:?}\nmanual = {manual:?}\nrank = {:?}\n\
                 temperament = \"werckmeister3\"\n",
                ranks[1]
            ));
        }
        std::fs::write(&path, text).expect("writes");
        let prepared = prepare(&[path], &[], 48_000.0, &|_| {}).expect("scoped load");
        let console = &prepared.console;
        assert_eq!(
            console.source_tuning("demo").map(|t| t.temperament),
            Some(tuning::Temperament::Meantone4)
        );
        let id_of = |name: &str| {
            console
                .stop_states()
                .iter()
                .find(|(_, n, ..)| *n == name)
                .map(|(id, ..)| *id)
                .expect("stop reloads")
        };
        let third = on_manual
            .iter()
            .map(|(name, ..)| name.as_str())
            .find(|name| *name != pinned && *name != own)
            .map(id_of);
        if let Some(third) = third {
            assert_eq!(
                console.stop_tuning_resolved(third).1,
                tuning::TuningScope::Source,
                "an unpinned stop follows its set"
            );
        }
        assert_eq!(console.stop_tuning_resolved(id_of(pinned)).1, tuning::TuningScope::Organ);
        assert_eq!(console.stop_follow(id_of(pinned)), tuning::Follow::Organ);
        let (own_tuning, scope) = console.stop_tuning_resolved(id_of(own));
        assert_eq!(scope, tuning::TuningScope::Stop);
        assert_eq!(own_tuning.temperament, tuning::Temperament::Pythagorean);
        if let Some((name, _, _)) = mixture {
            let id = id_of(name);
            let ranks = console.stop_ranks(id);
            assert_eq!(
                console.rank_tuning(id, ranks[1].0).map(|t| t.temperament),
                Some(tuning::Temperament::Werckmeister3)
            );
            assert!(console.rank_tuning(id, ranks[0].0).is_none(), "only the named rank");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The whole pipeline against the demo set: the same function the
    /// picker calls at runtime must produce a playable console.
    #[test]
    fn prepare_builds_a_playable_console_from_the_demo_set() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../testsets/grandorgue-demo/demo.organ");
        if !path.is_file() {
            eprintln!("skipping: demo set not present");
            return;
        }
        let phases = std::sync::Mutex::new(Vec::new());
        let prepared = prepare(&[path], &[], 48_000.0, &|phase| {
            phases.lock().expect("phases").push(phase);
        })
        .expect("demo set prepares");
        assert!(!prepared.console.stop_states().is_empty(), "stops exist");
        assert!(!prepared.bank.is_empty(), "samples decoded");
        assert!(
            !phases.lock().expect("phases").is_empty(),
            "progress was reported"
        );
    }
}
