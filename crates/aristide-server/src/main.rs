mod bank;
mod console;
mod http;
mod tuning;

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
        set: None,
        stops: Vec::new(),
        list_stops: false,
        master_gain: None,
        http_port: 9669,
        buffer_frames: 256,
        record: None,
        safe: false,
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
            other if args.set.is_none() && !other.starts_with('-') => {
                args.set = Some(PathBuf::from(other))
            }
            other => anyhow::bail!(
                "unknown argument {other:?} (usage: aristide-server [set.organ] \
                 [--stops name,name] [--list-stops] [--gain 0.18])"
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

/// Stop patterns (from `--stops` or the sidecar) resolve exact-first,
/// then shortest-substring (see `sidecar::match_names`), so "plein jeu"
/// draws the mixture and not its drawstop noise. With no patterns at
/// all, each manual's first stop is drawn so a fresh start makes sound
/// on every keyboard.
fn choose_registration(organ: &Organ, patterns: &[String]) -> Vec<StopId> {
    let drawn: Vec<StopId> = if patterns.is_empty() {
        organ
            .manuals
            .iter()
            .filter_map(|m| organ.stops.iter().find(|s| s.manual == m.id))
            .map(|s| s.id)
            .collect()
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
        tracing::warn!("no stops matched — keys will be silent");
    }
    drawn
}

/// One-time setup on the audio callback's own thread (cpal creates it,
/// so this runs on first callback): real-time scheduling — without
/// SCHED_FIFO any desktop load preempts us and no buffer size saves you
/// — and flush-to-zero so denormals can't burn cycles.
fn audio_thread_setup(buffer_frames: u32, sample_rate: u32) {
    #[cfg(target_arch = "x86_64")]
    unsafe {
        // MXCSR bit 15 = flush-to-zero, bit 6 = denormals-are-zero.
        let mut csr: u32 = 0;
        core::arch::asm!("stmxcsr [{}]", in(reg) &mut csr, options(nostack));
        csr |= 0x8000 | 0x0040;
        core::arch::asm!("ldmxcsr [{}]", in(reg) &csr, options(nostack, readonly));
    }
    let _ = (buffer_frames, sample_rate);
    #[cfg(target_os = "linux")]
    unsafe {
        let param = libc::sched_param { sched_priority: 70 };
        let result =
            libc::pthread_setschedparam(libc::pthread_self(), libc::SCHED_FIFO, &param);
        if result == 0 {
            tracing::info!("audio thread: SCHED_FIFO real-time priority acquired");
        } else {
            // Ordinary desktop users have no rtprio rlimit, so this is the
            // common path. Publish our kernel tid; a helper thread will ask
            // RealtimeKit (the mechanism PipeWire and every DAW use) to
            // promote us — rtkit only accepts requests by tid.
            let tid = libc::syscall(libc::SYS_gettid) as i64;
            AUDIO_TID.store(tid, std::sync::atomic::Ordering::Release);
            tracing::info!(
                "audio thread: direct SCHED_FIFO denied (errno {result}); \
                 asking RealtimeKit to promote tid {tid}"
            );
        }
    }
}

/// Kernel tid of the audio callback thread, published only when direct
/// SCHED_FIFO promotion failed and RealtimeKit should be tried.
#[cfg(target_os = "linux")]
static AUDIO_TID: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(0);

/// Promote the audio thread through the RealtimeKit D-Bus service, from a
/// normal thread (never the audio thread — D-Bus is IO). Uses `dbus-send`
/// (present on any desktop that has rtkit) instead of a D-Bus library
/// dependency. rtkit refuses processes without an RLIMIT_RTTIME ceiling —
/// the kernel's runaway-RT-thread guard; the accounting resets every time
/// the callback blocks on the device (~every buffer), so 200 ms of
/// *continuous* RT CPU only happens if we are catastrophically broken.
#[cfg(target_os = "linux")]
fn promote_audio_thread_via_rtkit() {
    use std::sync::atomic::Ordering;
    let mut tid = 0;
    // The first callback (which publishes the tid) fires within one buffer
    // of stream.play(); 3 s covers even a device that starts paused.
    for _ in 0..300 {
        tid = AUDIO_TID.load(Ordering::Acquire);
        if tid != 0 {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    if tid == 0 {
        return; // direct promotion succeeded (or no callback ever ran)
    }
    unsafe {
        let limit = libc::rlimit {
            rlim_cur: 200_000, // µs of continuous RT CPU before SIGKILL
            rlim_max: 200_000,
        };
        libc::setrlimit(libc::RLIMIT_RTTIME, &limit);
    }
    let max_priority = std::process::Command::new("dbus-send")
        .args([
            "--system",
            "--print-reply",
            "--dest=org.freedesktop.RealtimeKit1",
            "/org/freedesktop/RealtimeKit1",
            "org.freedesktop.DBus.Properties.Get",
            "string:org.freedesktop.RealtimeKit1",
            "string:MaxRealtimePriority",
        ])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| {
            String::from_utf8_lossy(&o.stdout)
                .split_whitespace()
                .last()
                .and_then(|v| v.parse::<i64>().ok())
        })
        .unwrap_or(10)
        .clamp(1, 70);
    // MakeThreadRealtime only promotes threads of the D-Bus caller — and
    // our caller is dbus-send, a different process (rtkit answers ENOENT:
    // it looked for our tid inside dbus-send). The WithPID variant names
    // the target process explicitly; rtkit still checks it belongs to the
    // same uid, so this stays unprivileged.
    let request = std::process::Command::new("dbus-send")
        .args([
            "--system",
            "--print-reply",
            "--dest=org.freedesktop.RealtimeKit1",
            "/org/freedesktop/RealtimeKit1",
            "org.freedesktop.RealtimeKit1.MakeThreadRealtimeWithPID",
            &format!("uint64:{}", std::process::id()),
            &format!("uint64:{tid}"),
            &format!("uint32:{max_priority}"),
        ])
        .output();
    let policy = unsafe { libc::sched_getscheduler(tid as libc::pid_t) };
    if policy == libc::SCHED_FIFO || policy == libc::SCHED_RR {
        tracing::info!(
            "audio thread: real-time priority {max_priority} acquired via RealtimeKit"
        );
    } else {
        let detail = match request {
            Ok(out) if !out.status.success() => {
                String::from_utf8_lossy(&out.stderr).trim().to_string()
            }
            Ok(_) => "rtkit accepted but the policy did not change".into(),
            Err(err) => format!("dbus-send unavailable: {err}"),
        };
        tracing::warn!(
            "audio thread: NO real-time priority ({detail}) — audio will glitch \
             under desktop load. Manual fix: add '@audio - rtprio 95' to \
             /etc/security/limits.d/audio.conf, add your user to the 'audio' \
             group, log out/in. Quick test: \
             sudo setcap cap_sys_nice+ep <path-to-aristide-server>"
        );
    }
}

/// Load a reverb impulse response: a wav next to the set, or
/// "synthetic" — a generated 2 s exponentially decaying stereo hall
/// (useful before any IR file exists; also the fallback demo room).
fn load_impulse_response(
    spec: &str,
    set_path: &std::path::Path,
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
        .unwrap_or(std::path::Path::new(""))
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

/// Sidecar manual names → manual indices; empty (or all-unmatched)
/// falls back to the keyboards-first default inside `Console::new`.
fn resolve_channel_map(organ: &Organ, channel_names: &[String]) -> Vec<usize> {
    let names: Vec<&str> = organ.manuals.iter().map(|m| m.name.as_str()).collect();
    let map: Vec<usize> = channel_names
        .iter()
        .filter_map(|pattern| {
            let matched = aristide_formats::sidecar::match_names(&names, pattern);
            if matched.len() != 1 {
                tracing::warn!(
                    "sidecar midi.channels: {pattern:?} matched {} manuals, ignoring map",
                    matched.len()
                );
                None
            } else {
                Some(matched[0])
            }
        })
        .collect();
    if map.len() == channel_names.len() {
        map
    } else {
        Vec::new()
    }
}

/// What MIDI input drives: the sampled organ console, or the M1 tone.
pub enum Control {
    Tone,
    Organ(Console),
}

pub struct State {
    pub engine: EngineHandle,
    pub control: Control,
    /// Wind groups the tremulant acts on (from the sidecar; empty when
    /// no set is loaded).
    pub trem_groups: Vec<u8>,
    pub trem_engaged: bool,
    pub master_gain: f32,
    /// Reverb wet level; `None` = no IR loaded.
    pub reverb_wet: Option<f32>,
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

    let mut wind_params = None;
    let mut trem_setup: Option<(aristide_engine::wind::TremulantParams, Vec<u8>)> = None;
    let mut reverb_ir: Option<Arc<aristide_engine::reverb::PreparedIr>> = None;
    let mut reverb_wet = 0.0f32;
    let (sample_bank, control) = match &args.set {
        Some(path) => {
            let organ = load_organ(path)?;
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
            // CLI wins over sidecar; sidecar wins over the built-in default.
            let patterns = if args.stops.is_empty() {
                sidecar.registration.default.clone()
            } else {
                args.stops.clone()
            };
            let defaults = aristide_engine::wind::WindParams::default();
            let kp = defaults.pitch_exponent as f64;
            // sag_cents is what the user hears; invert P^kp to pressure.
            let sag_cents = sidecar.wind.sag_cents.clamp(0.0, 50.0);
            wind_params = Some(aristide_engine::wind::WindParams {
                sag_depth: (1.0 - 2f64.powf(-sag_cents / (1200.0 * kp))) as f32,
                natural_hz: sidecar.wind.bounce_hz.clamp(0.5, 12.0) as f32,
                damping: sidecar.wind.damping.clamp(0.2, 1.5) as f32,
                flow_noise: (sidecar.wind.flow_noise_percent / 100.0).clamp(0.0, 0.1) as f32,
                ..defaults
            });

            // Tremulant: pitch cents → pressure swing through the same
            // exponent, applied to the sidecar's chests (default: all).
            let depth_cents = sidecar.tremulant.depth_cents.clamp(0.0, 30.0);
            let trem_params = aristide_engine::wind::TremulantParams {
                rate_hz: sidecar.tremulant.rate_hz.clamp(0.5, 12.0) as f32,
                depth: (2f64.powf(depth_cents / (1200.0 * kp)) - 1.0) as f32,
                ..Default::default()
            };
            let max_groups = aristide_engine::wind::MAX_WIND_GROUPS as u32;
            let groups: Vec<u8> = if sidecar.tremulant.chests.is_empty() {
                (0..max_groups as u8).collect()
            } else {
                sidecar
                    .tremulant
                    .chests
                    .iter()
                    .map(|&chest| chest.saturating_sub(1).min(max_groups - 1) as u8)
                    .collect()
            };
            trem_setup = Some((trem_params, groups));

            if !sidecar.reverb.ir.is_empty() {
                reverb_wet = sidecar.reverb.wet.clamp(0.0, 2.0) as f32;
                match load_impulse_response(&sidecar.reverb.ir, path, sample_rate) {
                    Ok(ir) => {
                        tracing::info!(
                            "reverb: {} ({} partitions), wet {:.2}",
                            sidecar.reverb.ir,
                            ir.partition_count(),
                            reverb_wet
                        );
                        reverb_ir = Some(Arc::new(ir));
                    }
                    Err(err) => tracing::warn!("reverb disabled: {err}"),
                }
            }
            let drawn = choose_registration(&organ, &patterns);
            let channel_map = resolve_channel_map(&organ, &sidecar.midi.channels);
            let mut console = Console::new(organ, loaded.specs, drawn, channel_map);
            let temperament = tuning::Temperament::parse(&sidecar.tuning.temperament)
                .unwrap_or_else(|| {
                    tracing::warn!(
                        "sidecar tuning: unknown temperament {:?}, using equal",
                        sidecar.tuning.temperament
                    );
                    tuning::Temperament::Equal
                });
            let live_tuning = tuning::Tuning {
                temperament,
                a4_hz: sidecar.tuning.a4_hz.clamp(300.0, 500.0),
                transpose: sidecar.tuning.transpose.clamp(-12, 12),
            };
            console.set_tuning(live_tuning);
            console.set_noises(
                sidecar.noises.enabled,
                sidecar.noises.volume.clamp(0.0, 2.0) as f32,
            );
            tracing::info!(
                "tuning: {} @ a'={} Hz, transpose {:+}",
                live_tuning.temperament.name(),
                live_tuning.a4_hz,
                live_tuning.transpose
            );
            for (channel, manual) in console.channel_names() {
                tracing::info!("midi: channel {channel} → {manual}");
            }
            (loaded.bank, Control::Organ(console))
        }
        None => {
            tracing::info!("no sample set given — playing the test tone");
            (Default::default(), Control::Tone)
        }
    };

    // Build the stream, falling back to the backend's default buffer if
    // it rejects our fixed size. Each attempt needs a fresh Engine (the
    // callback closure consumes it); the bank is shared via Arc.
    let bank = Arc::new(sample_bank);
    // Fault every sample page in NOW; doing it lazily means page faults
    // inside the audio callback on each pipe's first note.
    let prefault_started = Instant::now();
    let checksum = bank.pre_fault();
    tracing::info!(
        "pre-faulted {:.0} MiB of samples in {:.1?} (checksum {checksum:.3})",
        bank.resident_bytes() as f64 / (1024.0 * 1024.0),
        prefault_started.elapsed()
    );

    let buffer_hint = args.buffer_frames;
    // Overrun detector: the callback timestamps itself; late arrivals
    // (gap > 2x the nominal block time) are counted and reported — the
    // objective signal for delivery-layer glitches.
    let overruns = Arc::new(std::sync::atomic::AtomicU32::new(0));
    // Recording tap: engine output -> lock-free ring -> writer thread.
    let mut tap_consumer = None;
    let record_requested = args.record.is_some();
    let safe_mode = args.safe;
    if safe_mode {
        tracing::warn!(
            "SAFE MODE: linear interpolation, no wind/tremulant/brightness — \
             diagnostic quality floor (GO-grade). If audio still glitches \
             here, the problem is the environment, not the engine's DSP."
        );
    }

    let build_stream = |buffer_size: cpal::BufferSize,
                        tap_out: &mut Option<rtrb::Consumer<f32>>|
     -> Result<(cpal::Stream, EngineHandle)> {
        let (mut engine, handle) = Engine::new(sample_rate, Arc::clone(&bank));
        engine.set_reverb(reverb_ir.clone(), reverb_wet);
        if safe_mode {
            engine.set_lite(true);
        }
        if record_requested {
            // ~90 s of stereo headroom; the writer drains far faster.
            let (producer, consumer) = rtrb::RingBuffer::new(1 << 23);
            engine.set_tap(producer);
            *tap_out = Some(consumer);
        }
        let mut stream_config = config.clone();
        stream_config.buffer_size = buffer_size;
        let mut rt_ready = false;
        let mut last_callback: Option<std::time::Instant> = None;
        let overruns = Arc::clone(&overruns);
        let stream = device.build_output_stream(
            &stream_config,
            move |data: &mut [f32], _| {
                if !rt_ready {
                    rt_ready = true;
                    audio_thread_setup(buffer_hint, sample_rate as u32);
                }
                let now = std::time::Instant::now();
                if let Some(previous) = last_callback {
                    let nominal = data.len() as f64 / channels as f64 / sample_rate as f64;
                    if now.duration_since(previous).as_secs_f64() > nominal * 2.0 {
                        overruns.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    }
                }
                last_callback = Some(now);
                engine.process(data, channels)
            },
            |err| tracing::error!("audio stream error: {err}"),
            None,
        )?;
        Ok((stream, handle))
    };
    let (stream, mut handle) =
        match build_stream(cpal::BufferSize::Fixed(args.buffer_frames), &mut tap_consumer) {
            Ok(pair) => pair,
            Err(err) => {
                tracing::warn!(
                    "device refused a {}-frame buffer ({err}); using its default \
                     (expect higher latency — try another --buffer value)",
                    args.buffer_frames
                );
                tap_consumer = None;
                build_stream(cpal::BufferSize::Default, &mut tap_consumer)?
            }
        };
    stream.play()?;
    #[cfg(target_os = "linux")]
    std::thread::spawn(promote_audio_thread_via_rtkit);

    if let Some(gain) = args.master_gain {
        handle.send(Command::SetMasterGain { linear: gain });
    }
    if let Some(params) = wind_params {
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

    let trem_groups = match &trem_setup {
        Some((params, groups)) => {
            tracing::info!(
                "tremulant: {:.1} Hz, ±{:.0}% pressure, chests {:?}",
                params.rate_hz,
                params.depth * 100.0,
                groups
            );
            for &group in groups {
                handle.send(Command::SetTremulantParams {
                    group,
                    params: *params,
                });
            }
            groups.clone()
        }
        None => Vec::new(),
    };

    let state = Arc::new(Mutex::new(State {
        engine: handle,
        control,
        trem_groups,
        trem_engaged: false,
        master_gain: args.master_gain.unwrap_or(0.178),
        reverb_wet: reverb_ir.is_some().then_some(reverb_wet),
    }));
    if let Err(err) = http::spawn(Arc::clone(&state), args.http_port) {
        tracing::warn!("console ui disabled: {err}");
    }
    let connections = connect_all_midi_inputs(&state)?;
    if connections.is_empty() {
        tracing::warn!("no MIDI inputs found — plug in the console and restart");
    } else {
        tracing::info!("listening on {} MIDI input(s) — play!", connections.len());
    }

    // Recorder thread: drain the tap into a WAV (16-bit PCM).
    let recording = args.record.clone();
    let recorder = tap_consumer.map(|mut consumer| {
        let path = recording.clone().expect("record path");
        tracing::info!("recording engine output to {}", path.display());
        let rate = sample_rate as u32;
        std::thread::Builder::new()
            .name("aristide-record".into())
            .spawn(move || -> std::io::Result<()> {
                use std::io::{Seek, SeekFrom, Write};
                let mut file = std::io::BufWriter::new(std::fs::File::create(&path)?);
                // Placeholder RIFF/data sizes, patched at shutdown.
                file.write_all(b"RIFF\0\0\0\0WAVEfmt ")?;
                file.write_all(&16u32.to_le_bytes())?;
                file.write_all(&1u16.to_le_bytes())?; // PCM
                file.write_all(&2u16.to_le_bytes())?; // stereo
                file.write_all(&rate.to_le_bytes())?;
                file.write_all(&(rate * 4).to_le_bytes())?;
                file.write_all(&4u16.to_le_bytes())?;
                file.write_all(&16u16.to_le_bytes())?;
                file.write_all(b"data\0\0\0\0")?;
                let mut written: u32 = 0;
                while !SHUTDOWN.load(std::sync::atomic::Ordering::Relaxed) {
                    while let Ok(value) = consumer.pop() {
                        let clamped = (value.clamp(-1.0, 1.0) * 32767.0) as i16;
                        file.write_all(&clamped.to_le_bytes())?;
                        written = written.saturating_add(2);
                    }
                    std::thread::sleep(std::time::Duration::from_millis(50));
                }
                let inner = file.get_mut();
                inner.seek(SeekFrom::Start(4))?;
                inner.write_all(&(36 + written).to_le_bytes())?;
                inner.seek(SeekFrom::Start(40))?;
                inner.write_all(&written.to_le_bytes())?;
                file.flush()?;
                Ok(())
            })
            .expect("spawn recorder")
    });

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
        std::thread::sleep(std::time::Duration::from_millis(500));
        let total = overruns.load(std::sync::atomic::Ordering::Relaxed);
        if total > reported_overruns {
            tracing::warn!(
                "audio callback arrived late {} time(s) — delivery-layer \
                 glitches (CPU contention or missing RT priority), not \
                 engine output",
                total
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

static SHUTDOWN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

#[cfg(unix)]
extern "C" fn handle_sigint(_signal: libc::c_int) {
    SHUTDOWN.store(true, std::sync::atomic::Ordering::Relaxed);
}

/// M1+ supports f32 output only; PipeWire/JACK and ALSA's plug layer all
/// offer it. Format conversion becomes the server's job later.
///
/// CRITICAL: never take a range's MAX rate blindly — a device whose
/// default format isn't f32 used to land us on its highest f32 rate
/// (up to 192 kHz = 4× the CPU per voice), guaranteeing overruns that
/// no engine optimization could ever fix.
fn pick_f32_config(device: &cpal::Device) -> Result<cpal::StreamConfig> {
    let default = device.default_output_config()?;
    if default.sample_format() == cpal::SampleFormat::F32 {
        let rate = default.sample_rate().0;
        if rate > 96_000 {
            tracing::warn!(
                "device default is {rate} Hz — unusually high; engine cost \
                 scales with rate"
            );
        }
        return Ok(default.into());
    }
    let target = 48_000u32;
    let mut best: Option<cpal::StreamConfig> = None;
    let mut best_distance = u32::MAX;
    for range in device.supported_output_configs()? {
        if range.sample_format() != cpal::SampleFormat::F32 {
            continue;
        }
        let rate = target.clamp(range.min_sample_rate().0, range.max_sample_rate().0);
        let distance = rate.abs_diff(target);
        if distance < best_distance {
            best_distance = distance;
            best = Some(range.with_sample_rate(cpal::SampleRate(rate)).into());
        }
    }
    best.context("audio device offers no f32 output format")
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
    let State {
        engine, control, ..
    } = &mut *state;

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
                let (starts, retriggered) = console.note_on(channel, key);
                for handle in retriggered {
                    send(Command::StopVoice { handle });
                }
                for start in starts {
                    send(Command::StartVoice {
                        handle: start.handle,
                        sample: start.spec.sample,
                        rate: start.spec.rate,
                        gain: start.spec.gain,
                        group: start.spec.group,
                        wind_weight: start.spec.wind_weight,
                        brightness: start.spec.brightness,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn demo_sidecar_draws_full_organ_and_maps_channels() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../testsets/grandorgue-demo/demo.organ");
        if !path.is_file() {
            eprintln!("skipping: demo set not present");
            return;
        }
        let organ = aristide_formats::grandorgue::load(&path)
            .expect("demo set loads")
            .organ;
        let sidecar = aristide_formats::sidecar::load_for(&path)
            .expect("sidecar readable")
            .expect("sidecar present");

        let drawn = choose_registration(&organ, &sidecar.registration.default);
        // The sidecar default is "*": the full organ, every stop drawn.
        assert_eq!(drawn.len(), organ.stops.len(), "every stop drawn");

        // A named pattern still narrows to exactly what it says.
        let plein = choose_registration(&organ, &["plein jeu".into()]);
        let names: Vec<&str> = organ
            .stops
            .iter()
            .filter(|s| plein.contains(&s.id))
            .map(|s| s.name.as_str())
            .collect();
        assert_eq!(names, ["Plein jeu III"], "no drawstop noises");

        let map = resolve_channel_map(&organ, &sidecar.midi.channels);
        // First Manual, Second Manual, Pedal — Great on channel 0.
        assert_eq!(map, vec![1, 2, 0]);
    }
}
