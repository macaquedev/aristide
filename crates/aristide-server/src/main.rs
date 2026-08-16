mod bank;
mod config;
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
/// all the organ starts cancelled — no stop drawn, as an organist finds
/// it — and the player registers from silence.
fn choose_registration(organ: &Organ, patterns: &[String]) -> Vec<StopId> {
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

/// Where one input's notes land.
///
/// `Unassigned` is the default for an organ nobody has configured, and
/// it is silent: an input the player has not placed must not guess. The
/// other two are the two shapes real hardware comes in — a console whose
/// manuals already speak on separate MIDI channels (`ChannelMap`), and a
/// plain keyboard that only ever sends one channel and therefore has to
/// be pinned (`Manual`).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Route {
    #[default]
    Unassigned,
    ChannelMap,
    Manual(usize),
}

impl Route {
    /// The wire form, shared by the HTTP API and the config file.
    pub fn as_str(&self) -> String {
        match self {
            Route::Unassigned => "none".into(),
            Route::ChannelMap => crate::config::FOLLOW_CHANNELS.into(),
            Route::Manual(manual) => manual.to_string(),
        }
    }
}

/// One MIDI input as the console sees it.
pub struct MidiPort {
    pub name: String,
    pub route: Route,
}

pub struct State {
    pub engine: EngineHandle,
    pub control: Control,
    /// MIDI inputs in connection order; the index is the port id the
    /// HTTP API and the input callbacks both use.
    pub midi_ports: Vec<MidiPort>,
    /// Saved assignments for every organ this machine has played.
    pub midi_config: config::MidiConfig,
    /// Where to write them back. `None` in tests and when the user has
    /// no config directory: assignments then last only for this run.
    pub config_path: Option<PathBuf>,
    /// The loaded organ's name — the key its assignments are stored
    /// under, so one rig can drive many instruments differently.
    pub organ_key: String,
    /// Wind groups the tremulant acts on (from the sidecar; empty when
    /// no set is loaded).
    pub trem_groups: Vec<u8>,
    pub trem_engaged: bool,
    pub master_gain: f32,
    /// Reverb wet level; `None` = no IR loaded.
    pub reverb_wet: Option<f32>,
    /// MIDI controller number driving swell boxes (sidecar `[enclosures] cc`).
    pub expression_cc: u8,
}

impl State {
    fn manual_names(&self) -> Vec<String> {
        match &self.control {
            Control::Organ(console) => console
                .manual_states()
                .iter()
                .map(|(_, name, _, _, _)| name.to_string())
                .collect(),
            Control::Tone => Vec::new(),
        }
    }

    /// What the config file says this port plays on this organ. A
    /// manual that the loaded set doesn't have leaves the device
    /// unassigned — silent — rather than sounding the wrong division.
    fn saved_route(&self, port: &str) -> Route {
        let Some(assignment) = self
            .midi_config
            .organ(&self.organ_key)
            .and_then(|organ| organ.devices.get(port))
        else {
            return Route::Unassigned;
        };
        if assignment == config::FOLLOW_CHANNELS {
            return Route::ChannelMap;
        }
        let names = self.manual_names();
        let names: Vec<&str> = names.iter().map(String::as_str).collect();
        match aristide_formats::sidecar::match_names(&names, assignment).as_slice() {
            [manual] => Route::Manual(*manual),
            _ => {
                tracing::warn!(
                    "midi: {port:?} is saved as {assignment:?}, which this organ \
                     has no manual for — leaving it unassigned"
                );
                Route::Unassigned
            }
        }
    }

    /// Write the live assignments back for this organ. Called after any
    /// change from the console, so quitting never loses one.
    fn persist(&mut self) {
        let Some(path) = self.config_path.clone() else {
            return;
        };
        let names = self.manual_names();
        let organ = self.organ_key.clone();
        let assignments: Vec<(String, Option<String>)> = self
            .midi_ports
            .iter()
            .map(|port| {
                let value = match port.route {
                    Route::Unassigned => None,
                    Route::ChannelMap => Some(config::FOLLOW_CHANNELS.to_string()),
                    // Stored by name, so it survives a set that renumbers
                    // its manuals and reads as English in the file.
                    Route::Manual(manual) => names.get(manual).cloned(),
                };
                (port.name.clone(), value)
            })
            .collect();
        for (port, value) in assignments {
            self.midi_config.set_device(&organ, &port, value.as_deref());
        }
        if let Control::Organ(console) = &self.control {
            let channels = console.channel_map();
            self.midi_config.set_channels(&organ, channels);
        }
        if let Err(err) = config::save(&path, &self.midi_config) {
            tracing::warn!("midi assignments not saved: {err}");
        }
    }
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
    let mut enclosure_setup: Vec<(u8, aristide_engine::enclosure::EnclosureParams)> = Vec::new();
    let mut expression_cc = 11u8;
    let mut reverb_ir: Option<Arc<aristide_engine::reverb::PreparedIr>> = None;
    let mut reverb_wet = 0.0f32;
    let (sample_bank, mut control) = match &args.set {
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

            // Enclosures: one engine box per ODF enclosure, floor from
            // the set's AmpMinimumLevel unless the sidecar overrides,
            // filter/inertia constants from the sidecar.
            let boxes = &sidecar.enclosures;
            expression_cc = boxes.cc.min(119);
            for (index, enclosure) in organ
                .enclosures
                .iter()
                .enumerate()
                .take(aristide_engine::enclosure::MAX_ENCLOSURES)
            {
                let floor_db = if boxes.floor_db < 0.0 {
                    boxes.floor_db.max(-40.0)
                } else {
                    // GO: AmpMinimumLevel % linear amplitude closed.
                    // Clamp at −40 dB (a 0 would be −∞; measured real
                    // boxes span 10–20 dB broadband).
                    20.0 * (enclosure.amp_minimum_level / 100.0).max(0.01).log10()
                };
                enclosure_setup.push((
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
    // DSP-load telemetry: the callback times engine.process() so the
    // overrun report can say WHICH side missed — a too-slow engine and a
    // preempted callback both arrive late, and only this measurement
    // tells them apart.
    let dsp_peak_ns = Arc::new(std::sync::atomic::AtomicU64::new(0));
    let dsp_over_budget = Arc::new(std::sync::atomic::AtomicU32::new(0));
    let dsp_budget_ns = Arc::new(std::sync::atomic::AtomicU64::new(0));
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
        let dsp_peak_ns = Arc::clone(&dsp_peak_ns);
        let dsp_over_budget = Arc::clone(&dsp_over_budget);
        let dsp_budget_ns = Arc::clone(&dsp_budget_ns);
        let stream = device.build_output_stream(
            &stream_config,
            move |data: &mut [f32], _| {
                use std::sync::atomic::Ordering::Relaxed;
                if !rt_ready {
                    rt_ready = true;
                    audio_thread_setup(buffer_hint, sample_rate as u32);
                }
                let now = std::time::Instant::now();
                let nominal = data.len() as f64 / channels as f64 / sample_rate as f64;
                if let Some(previous) = last_callback {
                    if now.duration_since(previous).as_secs_f64() > nominal * 2.0 {
                        overruns.fetch_add(1, Relaxed);
                    }
                }
                last_callback = Some(now);
                engine.process(data, channels);
                let spent_ns = now.elapsed().as_nanos() as u64;
                let budget_ns = (nominal * 1e9) as u64;
                dsp_budget_ns.store(budget_ns, Relaxed);
                dsp_peak_ns.fetch_max(spent_ns, Relaxed);
                if spent_ns > budget_ns {
                    dsp_over_budget.fetch_add(1, Relaxed);
                }
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

    for &(enclosure, params) in &enclosure_setup {
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

    // Assignments are per organ, so the loaded set's own name is the
    // key; with no set loaded there is nothing to assign to.
    let organ_key = match &control {
        Control::Organ(console) => console.organ_name().to_string(),
        Control::Tone => String::new(),
    };
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
    if let (Control::Organ(console), Some(organ)) = (&mut control, midi_config.organ(&organ_key))
        && !organ.channels.is_empty()
    {
        console.set_channel_map(organ.channels.clone());
    }

    let state = Arc::new(Mutex::new(State {
        engine: handle,
        control,
        midi_ports: Vec::new(),
        midi_config,
        config_path,
        organ_key,
        trem_groups,
        trem_engaged: false,
        master_gain: args.master_gain.unwrap_or(0.178),
        reverb_wet: reverb_ir.is_some().then_some(reverb_wet),
        expression_cc,
    }));
    if let Err(err) = http::spawn(Arc::clone(&state), args.http_port) {
        tracing::warn!("console ui disabled: {err}");
    }
    // MIDI is optional: the console UI can play notes on its own, so a
    // box with no sequencer access still gets a working instrument.
    spawn_midi_supervisor(Arc::clone(&state));

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
        use std::sync::atomic::Ordering::Relaxed;
        let total = overruns.load(Relaxed);
        if total > reported_overruns {
            // Name the guilty side: the peak engine.process() time since
            // the last report either fits the block budget (the OS starved
            // us) or doesn't (the DSP is too heavy for this machine).
            let peak_ms = dsp_peak_ns.swap(0, Relaxed) as f64 / 1e6;
            let budget_ms = dsp_budget_ns.load(Relaxed) as f64 / 1e6;
            let engine_over = dsp_over_budget.load(Relaxed);
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

/// Hardware consoles and flaky cables can dump a burst of stale note-ons
/// the instant a client subscribes to their port — heard as a random-note
/// bang at every server start. Note-ons arriving within this window of a
/// port's subscription are swallowed (note-offs and CCs still pass, so
/// held-key state and pedals stay sane).
const MIDI_CONNECT_GRACE: std::time::Duration = std::time::Duration::from_millis(400);

/// How often the supervisor re-reads the port list. A keyboard plugged
/// in mid-session should appear in Preferences without a restart, and a
/// second of latency is imperceptible for that.
const MIDI_SCAN_INTERVAL: std::time::Duration = std::time::Duration::from_millis(1000);

/// Set by the HTTP API to force a reconnect even when the port list
/// looks unchanged (a cable re-seated behind an unchanged name).
static MIDI_RESCAN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn request_midi_rescan() {
    MIDI_RESCAN.store(true, std::sync::atomic::Ordering::Relaxed);
}

/// Owns every open MIDI input for the life of the process. Connections
/// are not `Send`-friendly to pass around, and they must outlive the
/// call that made them, so one thread holds them and rebuilds the whole
/// set whenever the hardware changes — that is also the cheapest correct
/// answer to hot-plug.
fn spawn_midi_supervisor(state: Arc<Mutex<State>>) {
    std::thread::Builder::new()
        .name("aristide-midi".into())
        .spawn(move || {
            let mut connections: Vec<MidiInputConnection<()>> = Vec::new();
            let mut known: Vec<String> = Vec::new();
            loop {
                let forced = MIDI_RESCAN.swap(false, std::sync::atomic::Ordering::Relaxed);
                match port_names() {
                    Ok(names) if forced || names != known => {
                        // Drop first: a port can only be subscribed once,
                        // and the old callbacks index the old port list.
                        connections.clear();
                        if !known.is_empty() || !names.is_empty() {
                            tracing::info!("midi: {} input(s) found", names.len());
                        }
                        connections = connect_all_midi_inputs(&state, &names);
                        if connections.is_empty() {
                            tracing::warn!(
                                "no MIDI inputs connected — console UI and computer \
                                 keyboard still play"
                            );
                        }
                        known = names;
                    }
                    Ok(_) => {}
                    Err(err) => {
                        if !known.is_empty() || connections.is_empty() {
                            tracing::warn!("MIDI unavailable ({err}) — console UI input only");
                        }
                        known.clear();
                    }
                }
                if SHUTDOWN.load(std::sync::atomic::Ordering::Relaxed) {
                    return;
                }
                std::thread::sleep(MIDI_SCAN_INTERVAL);
            }
        })
        .ok();
}

fn port_names() -> Result<Vec<String>> {
    let mut probe = MidiInput::new("aristide-probe")?;
    probe.ignore(Ignore::All);
    Ok(probe
        .ports()
        .iter()
        .enumerate()
        .map(|(index, port)| {
            probe
                .port_name(port)
                .unwrap_or_else(|_| format!("port {index}"))
        })
        .collect())
}

/// Connect every input and publish the port list into the shared state.
/// Per-port settings survive a rescan by name, so unplugging one
/// keyboard never re-routes the others.
fn connect_all_midi_inputs(
    state: &Arc<Mutex<State>>,
    names: &[String],
) -> Vec<MidiInputConnection<()>> {
    use std::sync::atomic::{AtomicU32, Ordering::Relaxed};

    let mut ports: Vec<MidiPort> = Vec::new();
    let mut connections = Vec::new();
    let mut grace_counters: Vec<(String, Arc<AtomicU32>)> = Vec::new();

    for (index, name) in names.iter().enumerate() {
        // Assignments come from the config file, keyed by this organ:
        // a device the player has not placed on THIS instrument is
        // unassigned, and silent, however it was set on another.
        let route = state.lock().expect("state poisoned").saved_route(name);
        // The port id the callback carries is this port's slot in
        // `state.midi_ports`, which is what the UI edits.
        let id = ports.len();
        let mut input = match MidiInput::new("aristide") {
            Ok(input) => input,
            Err(err) => {
                tracing::warn!("midi: {name}: {err}");
                continue;
            }
        };
        input.ignore(Ignore::All);
        let Some(port) = input.ports().into_iter().nth(index) else {
            continue;
        };
        let state = Arc::clone(state);
        let suppressed = Arc::new(AtomicU32::new(0));
        let counter = Arc::clone(&suppressed);
        let subscribed_at = Instant::now();
        match input.connect(
            &port,
            "aristide-in",
            move |_, message, _| {
                if subscribed_at.elapsed() < MIDI_CONNECT_GRACE
                    && matches!(message, &[status, _, velocity]
                        if status & 0xF0 == 0x90 && velocity > 0)
                {
                    counter.fetch_add(1, Relaxed);
                    return;
                }
                handle_midi(message, id, &state)
            },
            (),
        ) {
            Ok(connection) => {
                tracing::info!("midi: connected to {name}");
                connections.push(connection);
                grace_counters.push((name.clone(), suppressed));
                match route {
                    Route::Unassigned => tracing::info!(
                        "midi: {name} is unassigned — assign it in Preferences → MIDI"
                    ),
                    Route::ChannelMap => tracing::info!("midi: {name} → channel map"),
                    Route::Manual(manual) => {
                        tracing::info!("midi: {name} → manual {manual}")
                    }
                }
                ports.push(MidiPort {
                    name: name.clone(),
                    route,
                });
            }
            Err(err) => tracing::warn!("midi: failed to connect to {name}: {err}"),
        }
    }
    state.lock().expect("state poisoned").midi_ports = ports;

    // Report once the grace has passed on every port; a count of 0 in the
    // log rules the connect-time burst out, a nonzero count confirms it.
    if !grace_counters.is_empty() {
        std::thread::Builder::new()
            .name("aristide-midi-grace".into())
            .spawn(move || {
                std::thread::sleep(MIDI_CONNECT_GRACE + MIDI_CONNECT_GRACE / 4);
                for (name, suppressed) in grace_counters {
                    let count = suppressed.load(Relaxed);
                    tracing::info!(
                        "midi: suppressed {count} note-on(s) during connect grace on {name}"
                    );
                }
            })
            .ok();
    }
    connections
}

fn handle_midi(message: &[u8], port: usize, state: &Mutex<State>) {
    let &[status, data1, data2] = message else {
        return;
    };
    let channel = status & 0x0F;
    let mut state = state.lock().expect("state poisoned");
    let expression_cc = state.expression_cc;
    // An unassigned port is deaf to everything, including note-offs:
    // it sent no note-ons either, so nothing can hang. The M1 test tone
    // has no manuals to assign to and always sounds.
    let route = match (&state.control, state.midi_ports.get(port)) {
        (Control::Tone, _) => Route::ChannelMap,
        (_, Some(port)) => port.route,
        (_, None) => Route::Unassigned,
    };
    if route == Route::Unassigned {
        return;
    }
    let State {
        engine, control, ..
    } = &mut *state;

    let mut send = |command: Command| {
        if !engine.send(command) {
            tracing::warn!("command queue full, dropped {command:?}");
        }
    };

    // Note-ons are the one message class that makes sound out of nowhere,
    // so each gets a timestamped log line — that's what lets a user tell
    // "phantom notes came in over MIDI" apart from every other suspect.
    if status & 0xF0 == 0x90 && data2 > 0 {
        tracing::info!("midi: note-on ch={channel} key={data1} vel={data2}");
    }
    match (status & 0xF0, data1, data2) {
        (0x90, key, velocity) if velocity > 0 => match control {
            Control::Tone => send(Command::NoteOn {
                key,
                freq_hz: midi_note_to_hz(key),
            }),
            Control::Organ(console) => {
                let (starts, retriggered) = match route {
                    Route::Manual(manual) => console.note_on_manual(manual, key),
                    _ => console.note_on(channel, key),
                };
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
                        enclosure: start.spec.enclosure,
                    });
                }
            }
        },
        (0x80, key, _) | (0x90, key, 0) => match control {
            Control::Tone => send(Command::NoteOff { key }),
            Control::Organ(console) => {
                let released = match route {
                    Route::Manual(manual) => console.note_off_manual(manual, key),
                    _ => console.note_off(channel, key),
                };
                for handle in released {
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
        // Expression pedal: drive the swell boxes of the channel's manual.
        (0xB0, cc, value) if cc == expression_cc => {
            if let Control::Organ(console) = control {
                let moves = match route {
                    Route::Manual(manual) => console.expression_manual(manual, value),
                    _ => console.expression(channel, value),
                };
                for (enclosure, position) in moves {
                    send(Command::SetEnclosurePosition {
                        enclosure,
                        position,
                    });
                }
            }
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

    /// A live state on the demo set, with `stop` drawn so notes make
    /// voices (held keys are only recorded for pipes that speak).
    fn demo_state(stop: &str) -> Option<(Arc<Mutex<State>>, usize)> {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../testsets/grandorgue-demo/demo.organ");
        if !path.is_file() {
            eprintln!("skipping: demo set not present");
            return None;
        }
        let organ = aristide_formats::grandorgue::load(&path)
            .expect("demo set loads")
            .organ;
        let target = organ
            .stops
            .iter()
            .find(|s| s.name == stop)
            .expect("named stop exists");
        let manual = organ
            .manuals
            .iter()
            .position(|m| m.id == target.manual)
            .expect("its manual exists");
        let drawn = vec![target.id];
        let loaded = bank::build(&organ, 48000.0).expect("bank builds");
        let console = Console::new(organ, loaded.specs, drawn, Vec::new());
        let (_engine, engine) = Engine::new(48000.0, Arc::new(loaded.bank));
        let state = Arc::new(Mutex::new(State {
            engine,
            control: Control::Organ(console),
            midi_ports: vec![MidiPort {
                name: "Test Keyboard".into(),
                route: Route::ChannelMap,
            }],
            midi_config: Default::default(),
            config_path: None,
            organ_key: "test organ".into(),
            trem_groups: Vec::new(),
            trem_engaged: false,
            master_gain: 0.178,
            reverb_wet: None,
            expression_cc: 11,
        }));
        Some((state, manual))
    }

    fn held_on(state: &Mutex<State>, manual: usize) -> Vec<u8> {
        let state = state.lock().expect("state poisoned");
        let Control::Organ(console) = &state.control else {
            panic!("organ expected");
        };
        console.manual_states()[manual].4.clone()
    }

    /// The point of per-device routing: a plain keyboard that only ever
    /// speaks on channel 1 can still be the second manual.
    #[test]
    fn a_pinned_device_ignores_the_channel_map() {
        // "Gamba 8'" lives on the demo's Second Manual, which the
        // default map reaches from channel 2, not channel 0.
        let Some((state, manual)) = demo_state("Gamba 8'") else {
            return;
        };
        assert_eq!(manual, 2, "the fixture's stop is on the second manual");

        handle_midi(&[0x90, 60, 100], 0, &state);
        assert!(
            held_on(&state, 2).is_empty(),
            "channel 0 is not that manual"
        );
        handle_midi(&[0x80, 60, 0], 0, &state);

        state.lock().expect("state").midi_ports[0].route = Route::Manual(2);
        handle_midi(&[0x90, 60, 100], 0, &state);
        assert_eq!(
            held_on(&state, 2),
            vec![60],
            "pinned device plays its manual"
        );
        handle_midi(&[0x80, 60, 0], 0, &state);
        assert!(held_on(&state, 2).is_empty(), "and releases it again");
    }

    /// The default on an organ nobody has configured: an input the
    /// player has not placed sounds nothing at all, rather than guessing
    /// a division from its MIDI channel.
    #[test]
    fn an_unassigned_device_is_silent() {
        let Some((state, manual)) = demo_state("Gamba 8'") else {
            return;
        };
        state.lock().expect("state").midi_ports[0].route = Route::Unassigned;
        handle_midi(&[0x90, 60, 100], 0, &state);
        for index in 0..=manual {
            assert!(
                held_on(&state, index).is_empty(),
                "unassigned input plays nothing, manual {index}"
            );
        }
    }

    /// Assignments are per organ and are stored by name, so the same
    /// keyboard can be the Récit here and the Great on another set.
    #[test]
    fn saved_assignments_resolve_against_the_loaded_organ() {
        let Some((state, manual)) = demo_state("Gamba 8'") else {
            return;
        };
        let mut state = state.lock().expect("state");
        let organ = state.organ_key.clone();
        state
            .midi_config
            .set_device(&organ, "Test Keyboard", Some("Second Manual"));
        assert_eq!(state.saved_route("Test Keyboard"), Route::Manual(manual));

        state
            .midi_config
            .set_device(&organ, "Test Keyboard", Some(config::FOLLOW_CHANNELS));
        assert_eq!(state.saved_route("Test Keyboard"), Route::ChannelMap);

        // A manual this organ hasn't got, and a device saved under a
        // different organ, both leave it unassigned rather than sounding
        // the wrong division.
        state
            .midi_config
            .set_device(&organ, "Test Keyboard", Some("Positif de dos"));
        assert_eq!(state.saved_route("Test Keyboard"), Route::Unassigned);
        state
            .midi_config
            .set_device("Another Organ", "Test Keyboard", Some("Second Manual"));
        state.midi_config.set_device(&organ, "Test Keyboard", None);
        assert_eq!(state.saved_route("Test Keyboard"), Route::Unassigned);
    }

    #[test]
    fn demo_sidecar_starts_cancelled_and_maps_channels() {
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
        // The sidecar names no stops: the organ starts cancelled.
        assert!(drawn.is_empty(), "no stop drawn at startup");
        // "*" is still available as an explicit full-organ pattern.
        let full = choose_registration(&organ, &["*".into()]);
        assert_eq!(full.len(), organ.stops.len(), "\"*\" draws every stop");

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
