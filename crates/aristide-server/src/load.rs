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
    /// The organ file's `[console.layout]` — empty unless it is a
    /// composite loaded alone, same condition as `composite` above.
    pub layout: std::collections::BTreeMap<String, instrument::PanelPos>,
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

/// Assemble `paths` (sample sets and composite definitions; several are
/// combined into one implicit composite), decode the samples, and build
/// the console. `stops` are CLI registration patterns; empty means each
/// source's sidecar default. `progress` is told what is happening, for
/// a UI that is watching the load.
/// One configured output bus, engine-ready.
pub struct BusSetup {
    /// Engine bus index (1-based; 0 is the untouched main bus).
    pub bus: u8,
    /// 0-based interface channel pair; `None` keeps the main pair.
    pub output: Option<(u8, u8)>,
    pub gain: f32,
    pub delay: Option<aristide_engine::routing::DelayParams>,
}

pub fn prepare(
    paths: &[PathBuf],
    stops: &[String],
    sample_rate: f32,
    progress: &dyn Fn(String),
) -> Result<PreparedInstrument> {
    anyhow::ensure!(!paths.is_empty(), "no sample set given");
    let first_path = &paths[0];
    let mut composite_midi: Option<(PathBuf, instrument::MidiDef)> = None;
    let mut manual_tuning_defs: Vec<instrument::ManualTuningDef> = Vec::new();
    let mut console_layout: std::collections::BTreeMap<String, instrument::PanelPos> =
        std::collections::BTreeMap::new();
    let mut setup = Setup::default();
    let mut load_warnings: Vec<String> = Vec::new();

    // Every path is a source: a sample set with its sidecar, or a
    // composite definition (`.toml`), which assembles first and then
    // acts like any other source. Per-set decisions — sidecar couplers,
    // the default registration, channel suggestions — resolve against
    // each source's own names here, then ride the id maps across.
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
                manual_tuning_defs = assembled.manual_tuning;
                console_layout = assembled.console_layout;
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
            // A sidecar rename: the set is read as-is, the name the
            // player gave it lives beside it. The name is also the key
            // MIDI assignments are stored under, so it applies before
            // anything downstream sees the organ.
            let renamed = sidecar.name.trim();
            if !renamed.is_empty() {
                organ.name = renamed.to_string();
            }
            (organ, sidecar)
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

    // With CLI --stops the patterns match the whole combined organ;
    // each sidecar's default registration instead means its own set's
    // stops and nothing else's.
    let per_source_drawn: Vec<Vec<StopId>> = if stops.is_empty() {
        sources
            .iter()
            .zip(&sidecars)
            .map(|((_, organ), sidecar)| {
                choose_registration(organ, &sidecar.registration.default)
            })
            .collect()
    } else {
        Vec::new()
    };
    let per_source_suggested: Vec<Vec<Option<u8>>> = sources
        .iter()
        .zip(&sidecars)
        .map(|((_, organ), sidecar)| suggested_channels(organ, &sidecar.midi.channels))
        .collect();
    // One source stands as itself; several become an implicit composite
    // — assembled exactly as a definition file with sources and no
    // pulls would be, every manual and coupler as its own set provides.
    // Engine-wide settings (wind, tremulant, reverb, tuning…) come from
    // the first source.
    let sidecar = &sidecars[0];
    let (organ, drawn) = if sources.len() == 1 {
        let organ = sources.pop().expect("one source").1;
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
    };

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
    let loaded = bank::build(&organ, sample_rate, sample_bits, cache_path.as_deref())?;
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

    let defaults = aristide_engine::wind::WindParams::default();
    let kp = defaults.pitch_exponent as f64;
    // sag_cents is what the user hears; invert P^kp to pressure.
    let sag_cents = sidecar.wind.sag_cents.clamp(0.0, 50.0);
    let wind = Some(aristide_engine::wind::WindParams {
        sag_depth: (1.0 - 2f64.powf(-sag_cents / (1200.0 * kp))) as f32,
        natural_hz: sidecar.wind.bounce_hz.clamp(0.5, 12.0) as f32,
        damping: sidecar.wind.damping.clamp(0.2, 1.5) as f32,
        flow_noise: (sidecar.wind.flow_noise_percent / 100.0).clamp(0.0, 0.1) as f32,
        ..defaults
    });

    // Tremulants. Precedence: a hand-written sidecar `[tremulant]`
    // replaces everything (explicit override); else the set's own
    // `[Tremulant]` definitions each become an engageable control on
    // their member chests; else the historical fallback — one default
    // tremulant over every chest, so the `tremulant` binding always
    // means something.
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
                depth: (2f64.powf(depth_cents / (1200.0 * kp)) - 1.0) as f32,
                ..Default::default()
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

    // Enclosures: one engine box per ODF enclosure, floor from the
    // set's AmpMinimumLevel unless the sidecar overrides, filter and
    // inertia constants from the sidecar.
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

    let mut reverb = None;
    if !sidecar.reverb.ir.is_empty() {
        let wet = sidecar.reverb.wet.clamp(0.0, 2.0) as f32;
        match load_impulse_response(&sidecar.reverb.ir, first_path, sample_rate) {
            Ok(ir) => {
                tracing::info!(
                    "reverb: {} ({} partitions), wet {:.2}",
                    sidecar.reverb.ir,
                    ir.partition_count(),
                    wet
                );
                reverb = Some((Arc::new(ir), wet));
            }
            Err(err) => tracing::warn!("reverb disabled: {err}"),
        }
    }

    let suggested: Vec<Option<u8>> = per_source_suggested.concat();
    let mut console = Console::new(organ, loaded.specs, drawn, sample_rate);
    console.set_attack_options(loaded.attack_options);
    let temperament = tuning::Temperament::parse(&sidecar.tuning.temperament)
        .unwrap_or_else(|| {
            tracing::warn!(
                "sidecar tuning: unknown temperament {:?}, using equal",
                sidecar.tuning.temperament
            );
            tuning::Temperament::Equal
        });
    // Scale files resolve against the organ's own directory: the file
    // that names them. A scale that fails to load warns and leaves the
    // temperament standing — a missing .scl must not brick the organ.
    let scale_base = first_path.parent().map(std::path::Path::to_path_buf);
    let load_scale = |scl: &str, kbm: Option<&str>, a4_hz: f64| {
        match tuning::ScaleTuning::load(scl, kbm, a4_hz, scale_base.as_deref()) {
            Ok(scale) => Some(std::sync::Arc::new(scale)),
            Err(err) => {
                tracing::warn!("tuning scale not loaded: {err} — keeping the temperament");
                None
            }
        }
    };
    let mut live_tuning = tuning::Tuning {
        temperament,
        scale: None,
        a4_hz: sidecar.tuning.a4_hz.clamp(300.0, 500.0),
        transpose: sidecar.tuning.transpose.clamp(-12, 12),
    };
    if let Some(scl) = &sidecar.tuning.scale {
        live_tuning.scale = load_scale(scl, sidecar.tuning.keymap.as_deref(), live_tuning.a4_hz);
    }
    console.set_tuning(live_tuning.clone());
    // Divisions the definition tunes apart from the rest: missing
    // fields follow the instrument-wide tuning.
    for def in &manual_tuning_defs {
        let temperament = def
            .temperament
            .as_deref()
            .map(|name| {
                tuning::Temperament::parse(name).unwrap_or_else(|| {
                    tracing::warn!(
                        "manual tuning: unknown temperament {name:?}, using the \
                         instrument's"
                    );
                    live_tuning.temperament
                })
            })
            .unwrap_or(live_tuning.temperament);
        let mut own = tuning::Tuning {
            temperament,
            scale: None,
            a4_hz: def.a4_hz.unwrap_or(live_tuning.a4_hz).clamp(300.0, 500.0),
            transpose: def.transpose.unwrap_or(live_tuning.transpose).clamp(-12, 12),
        };
        if let Some(scl) = &def.scale {
            own.scale = load_scale(scl, def.keymap.as_deref(), own.a4_hz);
        }
        tracing::info!(
            "tuning: manual {} plays {} @ a'={} Hz, transpose {:+}",
            def.manual,
            own.scale
                .as_ref()
                .map(|scale| scale.name())
                .unwrap_or(own.temperament.name()),
            own.a4_hz,
            own.transpose
        );
        console.set_manual_tuning(def.manual, Some(own));
    }
    console.set_coupler_repitch(sidecar.couplers.repitch);
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
    console.set_noises(
        sidecar.noises.enabled,
        sidecar.noises.volume.clamp(0.0, 2.0) as f32,
    );
    // Audio routing: stops onto buses, speaking delays — resolved by
    // the same name-pattern rules couplers use. A pattern that matches
    // nothing warns to the console; it never fails the load.
    let mut buses = Vec::new();
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
        // Voicing trims: level and cents per stop pattern.
        let mut adjust: std::collections::HashMap<aristide_model::StopId, (f32, f32)> =
            std::collections::HashMap::new();
        for rule in &sidecar.voicing.adjusts {
            let gain = 10f32.powf((rule.gain_db.clamp(-40.0, 20.0) as f32) / 20.0);
            let ratio = ((rule.cents.clamp(-200.0, 200.0) / 1200.0) as f32).exp2();
            for pattern in &rule.stops {
                let matched = aristide_formats::sidecar::match_names(&stop_names, pattern);
                if matched.is_empty() {
                    load_warnings.push(format!("voicing.adjust: {pattern:?} matches nothing"));
                }
                for at in matched {
                    let entry = adjust.entry(stop_ids[at].0).or_insert((1.0, 1.0));
                    entry.0 *= gain;
                    entry.1 *= ratio;
                }
            }
        }
        if !adjust.is_empty() {
            tracing::info!("voicing: {} stop(s) trimmed", adjust.len());
            console.set_stop_adjust(adjust);
        }
    }
    tracing::info!(
        "tuning: {} @ a'={} Hz, transpose {:+}",
        live_tuning.temperament.name(),
        live_tuning.a4_hz,
        live_tuning.transpose
    );

    Ok(PreparedInstrument {
        console,
        bank: loaded.bank,
        wind,
        tremulants,
        enclosures,
        expression_cc,
        reverb,
        composite: composite_midi,
        suggested_channels: suggested,
        setup,
        layout: console_layout,
        buses,
        warnings: load_warnings,
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
        assert_eq!(
            format!("{:?}", adopted.console.tuning()),
            format!("{:?}", direct.console.tuning())
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
