mod audio;
mod bank;
mod bindings;
mod cache;
mod config;
mod console;
mod control;
mod http;
mod load;
mod state;
mod tuning;

// Re-exported so http.rs, load.rs, bank.rs, console.rs, config.rs,
// tuning.rs and cache.rs — which predate this module split — keep
// naming these at the crate root.
pub use bindings::{request_midi_rescan, COMPUTER_KEYBOARD};
pub use state::{
    Control, CouplerRouteEdit, LoadRequest, Pending, RankItem, Resolution, Setup, State,
    TremControl,
};

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use anyhow::{Context, Result};
use aristide_engine::Command;
use aristide_formats::instrument;
use cpal::traits::{DeviceTrait, HostTrait};

use audio::{pick_f32_config, spawn_recorder, AudioOutput};
use bindings::spawn_midi_supervisor;

struct Args {
    /// Paths to GrandOrgue `.organ` files. One is the usual case; more
    /// are merged into a single composite instrument (all their
    /// manuals side by side, ids renumbered into one namespace). With
    /// none the server plays the M1 test tone.
    sets: Vec<PathBuf>,
    /// Case-insensitive substrings choosing which stops to draw.
    stops: Vec<String>,
    list_stops: bool,
    master_gain: Option<f32>,
    /// Local web console port.
    http_port: u16,
    /// Requested audio buffer size in frames.
    buffer_frames: u32,
    /// Record the engine's exact output to this WAV file (diagnostics).
    record: Option<PathBuf>,
    /// Safe mode: GO-grade minimal DSP, for isolating environment issues.
    safe: bool,
}

fn parse_args() -> Result<Args> {
    let mut args = Args {
        sets: Vec::new(),
        stops: Vec::new(),
        list_stops: false,
        master_gain: None,
        http_port: 9669,
        buffer_frames: 512,
        record: None,
        safe: false,
    };
    let mut iter = std::env::args().skip(1);
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--set" => args
                .sets
                .push(PathBuf::from(iter.next().context("--set needs a path")?)),
            "--stops" => args.stops.extend(
                iter.next()
                    .context("--stops needs a comma-separated list")?
                    .split(',')
                    .map(|s| s.trim().to_lowercase())
                    .filter(|s| !s.is_empty()),
            ),
            "--list-stops" => args.list_stops = true,
            "--gain" => {
                args.master_gain = Some(
                    iter.next()
                        .context("--gain needs a value")?
                        .parse()
                        .context("--gain must be a number")?,
                )
            }
            "--http-port" => {
                args.http_port = iter
                    .next()
                    .context("--http-port needs a value")?
                    .parse()
                    .context("--http-port must be a port number")?
            }
            "--safe" => args.safe = true,
            "--record" => {
                args.record = Some(PathBuf::from(
                    iter.next().context("--record needs a wav path")?,
                ))
            }
            "--buffer" => {
                args.buffer_frames = iter
                    .next()
                    .context("--buffer needs a frame count")?
                    .parse::<u32>()
                    .context("--buffer must be a frame count, e.g. 128/256/512")?
                    .clamp(16, 8192)
            }
            other if !other.starts_with('-') => args.sets.push(PathBuf::from(other)),
            other => anyhow::bail!(
                "unknown argument {other:?} (usage: aristide-server [set.organ…] \
                 [--stops name,name] [--list-stops] [--gain 0.18])"
            ),
        }
    }
    Ok(args)
}

/// `--list-stops`: print every manual's registered stop names for each
/// named set or organ file, without opening any audio device.
fn list_stops(paths: &[PathBuf]) -> Result<()> {
    anyhow::ensure!(!paths.is_empty(), "--list-stops needs a set path");
    for path in paths {
        let organ = if instrument::is_definition(path) {
            instrument::load(path)
                .with_context(|| format!("loading {}", path.display()))?
                .organ
        } else {
            load::load_organ(path)?
        };
        for manual in &organ.manuals {
            println!("{}:", manual.name);
            for stop in organ.stops.iter().filter(|s| s.manual == manual.id) {
                println!("  {}", stop.name);
            }
        }
    }
    Ok(())
}

/// The default output device and the best f32 format it offers,
/// logging the diagnostic line every audio bug report should open
/// with.
fn select_audio_config(args: &Args) -> Result<(cpal::Device, cpal::StreamConfig, f32, usize)> {
    let host = cpal::default_host();
    let device = host
        .default_output_device()
        .context("no default audio output device")?;
    // Latency = buffer size ÷ sample rate (plus device/driver stack).
    // Left at the backend default this can be tens of milliseconds —
    // we request a small fixed buffer instead; --buffer overrides.
    let config = pick_f32_config(&device)?;
    let sample_rate = config.sample_rate.0 as f32;
    let channels = config.channels as usize;
    tracing::info!(
        "=== DIAGNOSTIC: commit {} | {} | device '{}' | {} Hz | {} ch | {} frame buffer ({:.1} ms) — paste this line when reporting audio issues ===",
        option_env!("ARISTIDE_COMMIT").unwrap_or("unknown"),
        if cfg!(debug_assertions) { "DEBUG BUILD (bad!)" } else { "release" },
        device.name().unwrap_or_else(|_| "<unnamed>".into()),
        config.sample_rate.0,
        channels,
        args.buffer_frames,
        args.buffer_frames as f32 * 1000.0 / sample_rate
    );
    Ok((device, config, sample_rate, channels))
}

fn main() -> Result<()> {
    tracing_subscriber::fmt::init();
    tracing::info!(
        "aristide-server {} (commit {})",
        env!("CARGO_PKG_VERSION"),
        option_env!("ARISTIDE_COMMIT").unwrap_or("unknown")
    );
    if cfg!(debug_assertions) {
        tracing::warn!(
            "DEBUG BUILD — 10-20x slower than release; audio WILL crackle. \
             Run: cargo run --release -p aristide-server"
        );
    }
    let args = parse_args()?;

    if args.list_stops {
        return list_stops(&args.sets);
    }

    let (device, config, sample_rate, channels) = select_audio_config(&args)?;

    if args.safe {
        tracing::warn!(
            "SAFE MODE: linear interpolation, no wind/tremulant/brightness — \
             diagnostic quality floor (GO-grade). If audio still glitches \
             here, the problem is the environment, not the engine's DSP."
        );
    }

    // The audio output is up before any organ is: the server starts on
    // an empty bank (the M1 test tone), and every organ — named on the
    // CLI or picked in the console later — arrives through the same
    // load path in the main loop below.
    let (record_tx, recorder) = match args.record.clone() {
        Some(path) => {
            let (sender, worker) = spawn_recorder(path, config.sample_rate.0)?;
            (Some(sender), Some(worker))
        }
        None => (None, None),
    };
    let mut audio = AudioOutput {
        device,
        config,
        channels,
        sample_rate,
        buffer_frames: args.buffer_frames,
        safe: args.safe,
        overruns: Arc::new(std::sync::atomic::AtomicU32::new(0)),
        dsp_peak_ns: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        dsp_over_budget: Arc::new(std::sync::atomic::AtomicU32::new(0)),
        dsp_budget_ns: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        record: record_tx,
        stream: None,
    };
    let mut handle = audio.start(Arc::new(aristide_engine::bank::SampleBank::default()), None)?;
    if let Some(gain) = args.master_gain {
        handle.send(Command::SetMasterGain { linear: gain });
    }

    let config_path = config::default_path();
    let midi_config = match &config_path {
        Some(path) => config::load(path).unwrap_or_else(|err| {
            tracing::warn!("midi assignments unreadable, starting empty: {err}");
            Default::default()
        }),
        None => {
            tracing::warn!(
                "no config directory (XDG_CONFIG_HOME/HOME unset) — MIDI \
                 assignments will last only for this run"
            );
            Default::default()
        }
    };
    // CLI paths are an explicit selection, queued like any picker
    // request; without them nothing loads until the console asks.
    let pending_load = (!args.sets.is_empty()).then(|| LoadRequest {
        paths: args.sets.clone(),
        stops: args.stops.clone(),
        initial: true,
    });
    if pending_load.is_none() {
        tracing::info!("no organ loaded — pick one in the console");
    }
    let state = Arc::new(Mutex::new(State::new(
        handle,
        config_path,
        midi_config,
        args.master_gain.unwrap_or(0.178),
        pending_load,
    )));
    // Assignments exist before any hardware does: the computer
    // keyboard and every binding are live from the first note.
    state.lock().expect("state poisoned").resolve_routes();
    if let Err(err) = http::spawn(Arc::clone(&state), args.http_port) {
        tracing::warn!("console ui disabled: {err}");
    }
    // MIDI is optional: the console UI can play notes on its own, so a
    // box with no sequencer access still gets a working instrument.
    spawn_midi_supervisor(Arc::clone(&state));

    // Ctrl-C: finish the WAV cleanly instead of truncating it.
    #[cfg(unix)]
    unsafe {
        libc::signal(
            libc::SIGINT,
            handle_sigint as extern "C" fn(libc::c_int) as usize,
        );
    }

    let mut reported_overruns = 0u32;
    loop {
        // Loads run here, on the thread that owns the stream. The lock
        // is NOT held while loading: the console keeps answering, and
        // the old organ keeps playing until the new one is ready.
        let request = state.lock().expect("state poisoned").pending_load.take();
        if let Some(request) = request {
            let initial = request.initial;
            if let Err(err) = perform_load(&state, &mut audio, request) {
                if initial {
                    return Err(err);
                }
                tracing::warn!("organ load failed: {err:#}");
                let mut state = state.lock().expect("state poisoned");
                // A pick queued during this load keeps the narration
                // alive — the console must not flash "done" between
                // back-to-back loads.
                if state.pending_load.is_none() {
                    state.loading = None;
                }
                state.load_error = Some(format!("{err:#}"));
                state.load_warnings.clear();
            }
            // Another pick may have queued while this one loaded;
            // start it now rather than after the sleep below.
            if !SHUTDOWN.load(std::sync::atomic::Ordering::Relaxed) {
                continue;
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
        use std::sync::atomic::Ordering::Relaxed;
        let total = audio.overruns.load(Relaxed);
        if total > reported_overruns {
            // Name the guilty side: the peak engine.process() time since
            // the last report either fits the block budget (the OS starved
            // us) or doesn't (the DSP is too heavy for this machine).
            let peak_ms = audio.dsp_peak_ns.swap(0, Relaxed) as f64 / 1e6;
            let budget_ms = audio.dsp_budget_ns.load(Relaxed) as f64 / 1e6;
            let engine_over = audio.dsp_over_budget.load(Relaxed);
            let verdict = if engine_over == 0 {
                "engine within budget — suspect OS scheduling / missing RT \
                 priority / CPU frequency governor"
            } else {
                "the ENGINE is blowing its deadline — DSP overload on this \
                 machine (try --safe or a larger --buffer)"
            };
            tracing::warn!(
                "audio callback arrived late {total} time(s); engine DSP \
                 peak {peak_ms:.2} ms of {budget_ms:.2} ms budget, \
                 {engine_over} block(s) ever over — {verdict}"
            );
            reported_overruns = total;
        }
        if SHUTDOWN.load(std::sync::atomic::Ordering::Relaxed) {
            break;
        }
    }
    if let Some(worker) = recorder {
        tracing::info!("finalizing recording…");
        let _ = worker.join();
    }
    tracing::info!("bye ({} late callbacks total)", reported_overruns);
    Ok(())
}
/// Every organ is a composite with an organ file of its own; a sample
/// set is only ever a source. A load that names a raw set is adopted
/// before it happens: the library's organ file that already wraps this
/// set is loaded when there is one, else a wrapper file is created —
/// the set's name, the set as its one source, its sidecar sections
/// carried in, and whatever wiring this machine already remembers for
/// that organ (the file owns the wiring from then on). Loading then
/// proceeds as for any composite, so renaming, wiring and every other
/// per-organ edit has a durable home from the very first load.
///
/// Two loads stay un-adopted: a multi-set launch (CLI) remains the
/// implicit combination, with saving as its way to a file; and with no
/// config directory there is nowhere to keep organ files, so the set
/// loads as itself. Adoption failures fall back the same way — a
/// read-only disk must not make an organ unplayable.
fn adopt_set(
    state: &Mutex<State>,
    request: LoadRequest,
    progress: &dyn Fn(String),
) -> LoadRequest {
    if request.paths.len() != 1 || instrument::is_definition(&request.paths[0]) {
        return request;
    }
    let Some(dir) = config::organs_dir() else {
        return request;
    };
    let set = request.paths[0].clone();
    let canonical = set.canonicalize().unwrap_or_else(|_| set.clone());
    let wrapper = {
        let state = state.lock().expect("state poisoned");
        // The library entry being loaded names WHICH organ the player
        // picked; several organs can wrap the same set, and clicking
        // "GrandOrgue demo" must never quietly reload another of them.
        let clicked = state
            .midi_config
            .library
            .iter()
            .find(|entry| entry.path == canonical || entry.path == set)
            .map(|entry| entry.name.clone());
        state
            .midi_config
            .wrapper_for(&canonical, clicked.as_deref(), Some(&dir))
    };
    if let Some(path) = wrapper {
        tracing::info!(
            "organ file for {}: {}",
            set.display(),
            path.display()
        );
        let mut state = state.lock().expect("state poisoned");
        state.midi_config.forget(&canonical);
        return LoadRequest {
            paths: vec![path],
            ..request
        };
    }
    // No wrapper yet: read the set for its name (its sidecar may rename
    // it — the same override the direct load applies) and make one.
    // The set is parsed once more inside the load that follows; ODF
    // parsing is cheap next to decoding the samples.
    progress("adopting the set as an organ…".to_string());
    let organ = match load::load_organ(&set) {
        Ok(organ) => organ,
        Err(err) => {
            tracing::warn!("set unreadable, loading it directly: {err:#}");
            return request;
        }
    };
    let name = match aristide_formats::sidecar::load_for(&set) {
        Ok(Some(sidecar)) if !sidecar.name.trim().is_empty() => {
            sidecar.name.trim().to_string()
        }
        _ => organ.name.clone(),
    };
    let wiring = state
        .lock()
        .expect("state poisoned")
        .midi_config
        .organ(&name)
        .cloned();
    match config::create_wrapper_organ(&dir, &name, &canonical, &organ, wiring.as_ref()) {
        Ok(path) => {
            tracing::info!("organ file created: {}", path.display());
            let mut state = state.lock().expect("state poisoned");
            state.midi_config.forget(&canonical);
            LoadRequest {
                paths: vec![path],
                ..request
            }
        }
        Err(err) => {
            tracing::warn!(
                "no organ file for {} ({err}) — loading the set directly",
                set.display()
            );
            request
        }
    }
}

/// Prepare the requested instrument off the shared lock, then swap it
/// in: new engine and stream, engine-wide settings, console, routing.
/// On error the running organ (or the bare test tone) stays untouched.
fn perform_load(
    state: &Arc<Mutex<State>>,
    audio: &mut AudioOutput,
    request: LoadRequest,
) -> Result<()> {
    let progress = |phase: String| {
        state.lock().expect("state poisoned").loading = Some(phase);
    };
    progress("loading…".to_string());
    let request = adopt_set(state, request, &progress);
    let load::PreparedInstrument {
        console,
        bank,
        wind,
        tremulants,
        enclosures,
        expression_cc,
        reverb,
        composite,
        suggested_channels,
        setup,
        provenance,
        stop_voicing,
        stop_labels,
        stop_order,
        layout,
        coupled_keys,
        coupler_key_modes,
        buses,
        warnings,
    } = load::prepare(&request.paths, &request.stops, audio.sample_rate, &progress)?;

    // Routed buses may want interface channels past the stereo pair;
    // try to reopen the device wide enough BEFORE the stream starts.
    // A device that can't (or a bus with no explicit output) is fine —
    // the engine folds unreachable pairs back onto the main output.
    let wanted_channels = buses
        .iter()
        .filter_map(|setup| setup.output)
        .map(|(left, right)| left.max(right) as usize + 1)
        .max()
        .unwrap_or(0);
    if wanted_channels > 0 {
        audio.ensure_channels(wanted_channels);
    }

    // Fault every sample page in NOW; doing it lazily means page faults
    // inside the audio callback on each pipe's first note.
    progress("waking samples…".to_string());
    let bank = Arc::new(bank);
    let prefault_started = Instant::now();
    let checksum = bank.pre_fault();
    tracing::info!(
        "pre-faulted {:.0} MiB of samples in {:.1?} (checksum {checksum:.3})",
        bank.resident_bytes() as f64 / (1024.0 * 1024.0),
        prefault_started.elapsed()
    );

    // Let whatever the outgoing organ is sounding fade before its
    // engine goes away with the stream.
    {
        let mut state = state.lock().expect("state poisoned");
        let State {
            engine, control, ..
        } = &mut *state;
        if let Control::Organ(old) = control {
            old.all_off();
        }
        engine.send(Command::AllNotesOff);
    }
    std::thread::sleep(std::time::Duration::from_millis(200));

    progress("starting audio…".to_string());
    let mut handle = audio.start(Arc::clone(&bank), reverb.clone())?;

    let master_gain = state.lock().expect("state poisoned").master_gain;
    handle.send(Command::SetMasterGain {
        linear: master_gain,
    });
    for setup in &buses {
        if let Some((left, right)) = setup.output {
            tracing::info!(
                "routing: bus {} → channels {}/{} at ×{:.2}",
                setup.bus,
                left + 1,
                right + 1,
                setup.gain
            );
            handle.send(Command::SetBusOutput {
                bus: setup.bus,
                left,
                right,
                gain: setup.gain,
            });
        } else if setup.gain != 1.0 {
            handle.send(Command::SetBusOutput {
                bus: setup.bus,
                left: 0,
                right: 1,
                gain: setup.gain,
            });
        }
        if let Some(params) = setup.delay {
            tracing::info!(
                "routing: bus {} delay {:.0} ms (mix {:.2}, dry {:.2}, feedback {:.2})",
                setup.bus,
                params.seconds * 1000.0,
                params.mix,
                params.dry,
                params.feedback
            );
            handle.send(Command::SetBusDelay {
                bus: setup.bus,
                params,
            });
        }
    }
    if let Some(params) = wind {
        tracing::info!(
            "wind: {:.2}% pressure sag @ {:.1} Hz, ζ={:.2}{}",
            params.sag_depth * 100.0,
            params.natural_hz,
            params.damping,
            if params.sag_depth == 0.0 { " (off)" } else { "" }
        );
        for group in 0..aristide_engine::wind::MAX_WIND_GROUPS as u8 {
            handle.send(Command::SetWind { group, params });
        }
    }
    for &(enclosure, params) in &enclosures {
        tracing::info!(
            "enclosure {}: floor {:.1} dB, shelf {:.1} dB @ {:.0}→{:.0} Hz, sweep {:.2} s (CC{})",
            enclosure,
            params.floor_db,
            params.shelf_db,
            params.corner_open_hz,
            params.corner_closed_hz,
            params.full_sweep_s,
            expression_cc
        );
        handle.send(Command::SetEnclosure { enclosure, params });
    }
    let mut trems = Vec::new();
    for setup in tremulants {
        if setup.wave {
            tracing::info!(
                "tremulant {:?}: wave (sample-switching), chests {:?}",
                setup.name,
                setup.groups
            );
        } else {
            tracing::info!(
                "tremulant {:?}: {:.1} Hz, ±{:.0}% pressure, ramp {:.2} s, chests {:?}",
                setup.name,
                setup.params.rate_hz,
                setup.params.depth * 100.0,
                setup.params.ramp_seconds,
                setup.groups
            );
            for &group in &setup.groups {
                handle.send(Command::SetTremulantParams {
                    group,
                    params: setup.params,
                });
            }
        }
        trems.push(TremControl {
            name: setup.name,
            wave: setup.wave,
            groups: setup.groups,
            engaged: false,
            params: setup.params,
        });
    }

    let mut state = state.lock().expect("state poisoned");
    state.install(state::Installed {
        engine: handle,
        console,
        suggested_channels,
        trems,
        reverb_wet: reverb.map(|(_, wet)| wet),
        expression_cc,
        composite,
        setup,
        provenance,
        stop_voicing,
        stop_labels,
        stop_order,
        layout,
        coupled_keys,
        coupler_key_modes,
        load_warnings: warnings,
    });
    tracing::info!("organ ready: {}", state.organ_key);
    Ok(())
}

pub(crate) static SHUTDOWN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

#[cfg(unix)]
extern "C" fn handle_sigint(_signal: libc::c_int) {
    SHUTDOWN.store(true, std::sync::atomic::Ordering::Relaxed);
}
