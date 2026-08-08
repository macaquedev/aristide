mod bank;
mod console;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use anyhow::{Context, Result};
use aristide_engine::{Command, Engine, EngineHandle};
use aristide_model::{Organ, StopId};
use console::Console;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use midir::{Ignore, MidiInput, MidiInputConnection};

struct Args {
    /// Path to a GrandOrgue `.organ` file; without it the server plays
    /// the M1 test tone.
    set: Option<PathBuf>,
    /// Case-insensitive substrings choosing which stops to draw.
    stops: Vec<String>,
    list_stops: bool,
    master_gain: Option<f32>,
}

fn parse_args() -> Result<Args> {
    let mut args = Args {
        set: None,
        stops: Vec::new(),
        list_stops: false,
        master_gain: None,
    };
    let mut iter = std::env::args().skip(1);
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--set" => args.set = Some(PathBuf::from(iter.next().context("--set needs a path")?)),
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
            other if args.set.is_none() && !other.starts_with('-') => {
                args.set = Some(PathBuf::from(other))
            }
            other => anyhow::bail!(
                "unknown argument {other:?} (usage: aristide-server [set.organ] \
                 [--stops name,name] [--list-stops] [--gain 0.35])"
            ),
        }
    }
    Ok(args)
}

fn load_organ(path: &std::path::Path) -> Result<Organ> {
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

/// `--stops` patterns select by name substring; the default draws each
/// manual's first stop so a fresh start makes sound on every keyboard.
fn choose_registration(organ: &Organ, patterns: &[String]) -> Vec<StopId> {
    let drawn: Vec<StopId> = if patterns.is_empty() {
        organ
            .manuals
            .iter()
            .filter_map(|m| organ.stops.iter().find(|s| s.manual == m.id))
            .map(|s| s.id)
            .collect()
    } else {
        organ
            .stops
            .iter()
            .filter(|s| {
                let name = s.name.to_lowercase();
                patterns.iter().any(|p| name.contains(p))
            })
            .map(|s| s.id)
            .collect()
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
        tracing::warn!("no stops matched — keys will be silent");
    }
    drawn
}

/// What MIDI input drives: the sampled organ console, or the M1 tone.
enum Control {
    Tone,
    Organ(Console),
}

struct State {
    engine: EngineHandle,
    control: Control,
}

fn main() -> Result<()> {
    tracing_subscriber::fmt::init();
    tracing::info!("aristide-server {}", env!("CARGO_PKG_VERSION"));
    let args = parse_args()?;

    if args.list_stops {
        let path = args.set.context("--list-stops needs a set path")?;
        let organ = load_organ(&path)?;
        for manual in &organ.manuals {
            println!("{}:", manual.name);
            for stop in organ.stops.iter().filter(|s| s.manual == manual.id) {
                println!("  {}", stop.name);
            }
        }
        return Ok(());
    }

    let host = cpal::default_host();
    let device = host
        .default_output_device()
        .context("no default audio output device")?;
    let config = pick_f32_config(&device)?;
    let sample_rate = config.sample_rate.0 as f32;
    let channels = config.channels as usize;
    tracing::info!(
        "audio: {} @ {} Hz, {} channels",
        device.name().unwrap_or_else(|_| "<unnamed>".into()),
        config.sample_rate.0,
        channels
    );

    let (sample_bank, control) = match &args.set {
        Some(path) => {
            let organ = load_organ(path)?;
            let started = Instant::now();
            let loaded = bank::build(&organ, sample_rate)?;
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
            let drawn = choose_registration(&organ, &args.stops);
            (
                loaded.bank,
                Control::Organ(Console::new(organ, loaded.specs, drawn)),
            )
        }
        None => {
            tracing::info!("no sample set given — playing the test tone");
            (Default::default(), Control::Tone)
        }
    };

    let (mut engine, mut handle) = Engine::new(sample_rate, Arc::new(sample_bank));
    if let Some(gain) = args.master_gain {
        handle.send(Command::SetMasterGain { linear: gain });
    }

    let stream = device.build_output_stream(
        &config,
        move |data: &mut [f32], _| engine.process(data, channels),
        |err| tracing::error!("audio stream error: {err}"),
        None,
    )?;
    stream.play()?;

    let state = Arc::new(Mutex::new(State {
        engine: handle,
        control,
    }));
    let connections = connect_all_midi_inputs(&state)?;
    if connections.is_empty() {
        tracing::warn!("no MIDI inputs found — plug in the console and restart");
    } else {
        tracing::info!("listening on {} MIDI input(s) — play!", connections.len());
    }

    std::thread::park();
    Ok(())
}

/// M1+ supports f32 output only; PipeWire/JACK and ALSA's plug layer all
/// offer it. Format conversion becomes the server's job later.
fn pick_f32_config(device: &cpal::Device) -> Result<cpal::StreamConfig> {
    let default = device.default_output_config()?;
    if default.sample_format() == cpal::SampleFormat::F32 {
        return Ok(default.into());
    }
    device
        .supported_output_configs()?
        .find(|c| c.sample_format() == cpal::SampleFormat::F32)
        .map(|c| c.with_max_sample_rate().into())
        .context("audio device offers no f32 output format")
}

fn connect_all_midi_inputs(state: &Arc<Mutex<State>>) -> Result<Vec<MidiInputConnection<()>>> {
    let mut probe = MidiInput::new("aristide-probe")?;
    probe.ignore(Ignore::All);
    let port_count = probe.ports().len();

    let mut connections = Vec::new();
    for index in 0..port_count {
        let mut input = MidiInput::new("aristide")?;
        input.ignore(Ignore::All);
        let Some(port) = input.ports().into_iter().nth(index) else {
            continue;
        };
        let name = input
            .port_name(&port)
            .unwrap_or_else(|_| format!("port {index}"));
        let state = Arc::clone(state);
        match input.connect(
            &port,
            "aristide-in",
            move |_, message, _| handle_midi(message, &state),
            (),
        ) {
            Ok(connection) => {
                tracing::info!("midi: connected to {name}");
                connections.push(connection);
            }
            Err(err) => tracing::warn!("midi: failed to connect to {name}: {err}"),
        }
    }
    Ok(connections)
}

fn handle_midi(message: &[u8], state: &Mutex<State>) {
    let &[status, data1, data2] = message else {
        return;
    };
    let channel = status & 0x0F;
    let mut state = state.lock().expect("state poisoned");
    let State { engine, control } = &mut *state;

    let mut send = |command: Command| {
        if !engine.send(command) {
            tracing::warn!("command queue full, dropped {command:?}");
        }
    };

    match (status & 0xF0, data1, data2) {
        (0x90, key, velocity) if velocity > 0 => match control {
            Control::Tone => send(Command::NoteOn {
                key,
                freq_hz: midi_note_to_hz(key),
            }),
            Control::Organ(console) => {
                for start in console.note_on(channel, key) {
                    send(Command::StartVoice {
                        handle: start.handle,
                        sample: start.spec.sample,
                        rate: start.spec.rate,
                        gain: start.spec.gain,
                    });
                }
            }
        },
        (0x80, key, _) | (0x90, key, 0) => match control {
            Control::Tone => send(Command::NoteOff { key }),
            Control::Organ(console) => {
                for handle in console.note_off(channel, key) {
                    send(Command::StopVoice { handle });
                }
            }
        },
        (0xB0, 120..=123, _) => {
            if let Control::Organ(console) = control {
                console.all_off();
            }
            send(Command::AllNotesOff);
        }
        _ => {}
    }
}

/// 12-EDO A440 lives HERE, control-side, as one replaceable default —
/// the RT engine only ever sees frequencies. (Tone mode only; sampled
/// pipes carry their own pitch.)
fn midi_note_to_hz(key: u8) -> f32 {
    440.0 * 2f32.powf((key as f32 - 69.0) / 12.0)
}
