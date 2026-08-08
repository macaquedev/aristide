use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use aristide_engine::{Command, Engine, EngineHandle};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use midir::{Ignore, MidiInput, MidiInputConnection};

fn main() -> Result<()> {
    tracing_subscriber::fmt::init();
    tracing::info!("aristide-server {}", env!("CARGO_PKG_VERSION"));

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

    let (mut engine, handle) = Engine::new(sample_rate);
    let stream = device.build_output_stream(
        &config,
        move |data: &mut [f32], _| engine.process(data, channels),
        |err| tracing::error!("audio stream error: {err}"),
        None,
    )?;
    stream.play()?;

    let handle = Arc::new(Mutex::new(handle));
    let connections = connect_all_midi_inputs(&handle)?;
    if connections.is_empty() {
        tracing::warn!("no MIDI inputs found — plug in the console and restart");
    } else {
        tracing::info!("listening on {} MIDI input(s) — play!", connections.len());
    }

    std::thread::park();
    Ok(())
}

/// M1 supports f32 output only; PipeWire/JACK and ALSA's plug layer all
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

fn connect_all_midi_inputs(
    handle: &Arc<Mutex<EngineHandle>>,
) -> Result<Vec<MidiInputConnection<()>>> {
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
        let handle = Arc::clone(handle);
        match input.connect(
            &port,
            "aristide-in",
            move |_, message, _| handle_midi(message, &handle),
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

fn handle_midi(message: &[u8], handle: &Mutex<EngineHandle>) {
    let &[status, data1, data2] = message else {
        return;
    };
    let command = match (status & 0xF0, data1, data2) {
        (0x90, key, velocity) if velocity > 0 => Command::NoteOn {
            key,
            freq_hz: midi_note_to_hz(key),
        },
        (0x80, key, _) | (0x90, key, 0) => Command::NoteOff { key },
        (0xB0, 120..=123, _) => Command::AllNotesOff,
        _ => return,
    };
    let mut handle = handle.lock().expect("engine handle poisoned");
    if !handle.send(command) {
        tracing::warn!("command queue full, dropped {command:?}");
    }
}

/// 12-EDO A440 lives HERE, control-side, as one replaceable default —
/// the RT engine only ever sees frequencies.
fn midi_note_to_hz(key: u8) -> f32 {
    440.0 * 2f32.powf((key as f32 - 69.0) / 12.0)
}
