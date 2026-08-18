mod bank;
mod config;
mod console;
mod control;
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

/// The sidecar's `midi.channels` (manual names in channel order) read
/// backwards: per manual index, the channel it conventionally speaks on.
///
/// This is a *suggestion*, never a route. A set can say "the Récit is
/// channel 2" because that is how its console was built, and the dialog
/// then pre-fills channel 2 when you hand-assign a device to the Récit;
/// nothing sounds until you assign one.
fn suggested_channels(organ: &Organ, channel_names: &[String]) -> Vec<Option<u8>> {
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

/// What MIDI input drives: the sampled organ console, or the M1 tone.
pub enum Control {
    Tone,
    Organ(Console),
}

/// A binding resolved against the loaded organ: the message it answers
/// to, and what it does, with every name already looked up so the MIDI
/// callback never searches for one.
#[derive(Clone)]
pub struct Binding {
    pub channel: Option<u8>,
    pub trigger: control::Trigger,
    pub action: control::Action,
    /// Pre-resolved subject of the action: a stop, a coupler, an
    /// enclosure, or the manual a pitch shift applies to.
    pub subject: Subject,
}

#[derive(Clone, Copy, PartialEq)]
pub enum Subject {
    None,
    Stop(StopId),
    Coupler(usize),
    Enclosure(usize),
    /// A pitch action's target: one manual, or every keyboard on the
    /// device the trigger arrived on.
    Manual(usize),
    Device,
}

/// One assignment as the MIDI callback sees it: already resolved to a
/// manual index and a key range, so no names are touched per message.
#[derive(Clone, Copy)]
pub struct Route {
    /// MIDI channel 1-16; `None` accepts any.
    pub channel: Option<u8>,
    pub manual: usize,
    /// The keyboard's compass — inclusive MIDI notes. Notes outside it
    /// are not this keyboard's to send, so they are ignored.
    pub keys: (u8, u8),
    /// Semitones this keyboard is currently shifted by.
    pub transpose: i8,
}

/// One MIDI input as the console sees it, with the assignments that
/// name it already resolved against the loaded organ.
pub struct MidiPort {
    pub name: String,
    /// Rebuilt whenever the assignments or the port list change, so the
    /// MIDI callback only ever scans a handful of routes.
    pub routes: Vec<Route>,
    /// The bindings that name this device (see [`Binding`]).
    pub bindings: Vec<Binding>,
}

impl MidiPort {
    /// Every manual a message on this channel reaches. More than one is
    /// legitimate: a keyboard may be assigned to two divisions.
    fn targets(&self, channel: u8) -> Vec<usize> {
        self.matching(channel, None)
    }

    /// The same for a note, which must also be inside the keyboard's
    /// own compass — the width the player taught it is the only thing
    /// that decides which notes exist.
    fn note_targets(&self, channel: u8, key: u8) -> Vec<usize> {
        self.matching(channel, Some(key))
    }

    /// Where a key from this port actually lands, after the shift the
    /// octave buttons have applied to the keyboard that sent it. `None`
    /// when the shift pushes it off the MIDI range entirely.
    fn transpose(&self, channel: u8, key: u8) -> Option<u8> {
        let shift = self
            .routes
            .iter()
            .find(|route| {
                route.channel.is_none_or(|on| on == channel + 1)
                    && (route.keys.0..=route.keys.1).contains(&key)
            })
            .map_or(0, |route| route.transpose);
        u8::try_from(key as i16 + shift as i16).ok().filter(|k| *k < 128)
    }

    fn matching(&self, channel: u8, key: Option<u8>) -> Vec<usize> {
        let mut manuals: Vec<usize> = self
            .routes
            .iter()
            .filter(|route| route.channel.is_none_or(|on| on == channel + 1))
            .filter(|route| key.is_none_or(|key| (route.keys.0..=route.keys.1).contains(&key)))
            .map(|route| route.manual)
            .collect();
        manuals.sort_unstable();
        manuals.dedup();
        manuals
    }
}

/// An assignment being learned: the dialog is waiting to be played.
///
/// Two presses teach it everything — the first names the keyboard (its
/// port and channel) and the bottom of its compass, the second the top.
/// That is one gesture for what would otherwise be four fields, and it
/// is the only way the app can know how wide the player's keyboard
/// actually is.
#[derive(Clone)]
pub struct Learn {
    pub manual: usize,
    /// Which of the manual's inputs to write; past the end appends one.
    pub slot: usize,
    /// Set by the first key: the keyboard being taught, and its bottom.
    pub heard: Option<config::Input>,
    started: Instant,
}

/// Listening forever would leave a live console silently swallowing the
/// notes it was meant to play, so the wait gives up on its own.
const LEARN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(20);

/// The computer keyboard's device name, wherever a device is named: in
/// the config file, in the UI's list, in a binding.
pub const COMPUTER_KEYBOARD: &str = "Computer keyboard";

/// A binding waiting to be taught what presses it.
#[derive(Clone, Copy)]
pub struct ControlLearn {
    pub slot: usize,
    started: Instant,
}

/// The computer keyboard resolved for play: which manual its rows
/// address and by how much they are shifted.
#[derive(Clone, Copy)]
pub struct KeyboardInput {
    pub manual: usize,
    pub transpose: i8,
    pub compass: (u8, u8),
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
    /// Per manual index: the MIDI channel (1-16) the sample set's
    /// sidecar says that manual conventionally speaks on. Used only to
    /// pre-fill the channel when a device is assigned by hand; playing
    /// a key sets the real one.
    pub suggested_channels: Vec<Option<u8>>,
    /// Set while Preferences waits for a key press to bind a manual.
    pub learn: Option<Learn>,
    /// Set while it waits for the *control* to press: which binding row
    /// the next message that isn't a note belongs to.
    pub control_learn: Option<ControlLearn>,
    /// Bindings on the computer keyboard, which no operating system
    /// enumerates but which is otherwise an input like the rest.
    pub key_bindings: Vec<Binding>,
    /// Where the computer keyboard's notes go, and how far they are
    /// shifted. Assigned like any other input, in the config.
    pub keyboard: Option<KeyboardInput>,
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

    /// The config file's manual names resolved to indices in the loaded
    /// organ. A saved name this set hasn't got is dropped with a warning
    /// rather than guessed at: playing the wrong division is worse than
    /// playing nothing.
    fn saved_assignments(&self) -> Vec<(usize, Vec<config::Input>)> {
        let names = self.manual_names();
        let names: Vec<&str> = names.iter().map(String::as_str).collect();
        self.midi_config
            .assignments(&self.organ_key)
            .filter_map(|(manual, inputs)| {
                match aristide_formats::sidecar::match_names(&names, manual).as_slice() {
                    [index] => Some((*index, inputs.to_vec())),
                    _ => {
                        tracing::warn!(
                            "midi: assignments name a manual {manual:?} this organ \
                             hasn't got — ignoring them"
                        );
                        None
                    }
                }
            })
            .collect()
    }

    /// Push the saved assignments into the connected ports. Every edit
    /// goes through the config, so this is the one place routing is
    /// derived and the MIDI callback never has to look at names.
    fn resolve_routes(&mut self) {
        self.resolve_bindings();
        let assignments = self.saved_assignments();
        let native = self.native_compass();
        for port in &mut self.midi_ports {
            port.routes = assignments
                .iter()
                .flat_map(|(manual, inputs)| {
                    inputs
                        .iter()
                        .filter(|input| input.device == port.name)
                        .map(|input| Route {
                            channel: input.channel,
                            manual: *manual,
                            // A keyboard nobody has measured is assumed
                            // to be exactly the organ's own compass.
                            keys: input.compass().unwrap_or(native[*manual]),
                            transpose: input.transpose,
                        })
                })
                .collect();
        }
        // A manual answers to every key any of its keyboards can send:
        // that union is the compass the console plays and the UI draws,
        // and it is where repitching starts filling in. A shifted
        // keyboard reaches shifted pipes, so the shift is part of it —
        // otherwise pressing octave-up would just make the top of the
        // keyboard silent.
        let mut widened: Vec<Option<(u8, u8)>> = vec![None; native.len()];
        for port in &self.midi_ports {
            for route in &port.routes {
                let shift = |key: u8| (key as i16 + route.transpose as i16).clamp(0, 127) as u8;
                let reach = (shift(route.keys.0), shift(route.keys.1));
                let slot = &mut widened[route.manual];
                *slot = Some(match *slot {
                    Some((low, high)) => (low.min(reach.0), high.max(reach.1)),
                    None => reach,
                });
            }
        }
        // The computer keyboard is assigned like any other input, and
        // its span counts towards the compass the same way. Unassigned,
        // it falls back to the principal manual rather than going
        // silent: it is the keyboard of last resort, on a machine that
        // may have no MIDI at all, and it cannot surprise anyone by
        // blasting a division nobody plugged in.
        let default_keyboard = assignments
            .iter()
            .all(|(_, inputs)| !inputs.iter().any(|i| i.device == COMPUTER_KEYBOARD))
            .then(|| self.principal_manual())
            .flatten()
            .map(|manual| (manual, vec![config::Input {
                device: COMPUTER_KEYBOARD.to_string(),
                channel: None,
                low: None,
                high: None,
                transpose: 0,
            }]));
        self.keyboard = assignments.iter().chain(default_keyboard.iter()).find_map(|(manual, inputs)| {
            let input = inputs.iter().find(|i| i.device == COMPUTER_KEYBOARD)?;
            let (low, high) = control::keyboard_compass();
            let shift = |key: u8| (key as i16 + input.transpose as i16).clamp(0, 127) as u8;
            let slot = &mut widened[*manual];
            *slot = Some(match *slot {
                Some((at_low, at_high)) => {
                    (at_low.min(shift(low)), at_high.max(shift(high)))
                }
                None => (shift(low), shift(high)),
            });
            Some(KeyboardInput {
                manual: *manual,
                transpose: input.transpose,
                compass: (low, high),
            })
        });
        if let Control::Organ(console) = &mut self.control {
            for (manual, compass) in widened.into_iter().enumerate() {
                match compass {
                    Some((low, high)) => console.set_compass(manual, low as i16, high as i16),
                    None => console.reset_compass(manual),
                }
            }
        }
    }

    /// A computer key, pressed or released, through exactly the path a
    /// MIDI message takes: a binding first, then a note on whichever
    /// manual the computer keyboard is assigned to.
    ///
    /// The mapping lives here rather than in the console UI so that one
    /// binding table governs both, and so an octave button on a MIDI
    /// console can shift the computer keyboard as readily as `=` can.
    pub fn key(&mut self, code: &str, pressed: bool) {
        let fired: Vec<Binding> = self
            .key_bindings
            .iter()
            .filter(|binding| binding.trigger == control::Trigger::Key(code.to_string()))
            .cloned()
            .collect();
        if !fired.is_empty() {
            // A key that does something does not also play: releasing it
            // must not sound the note its press didn't.
            if pressed {
                for binding in fired {
                    self.run(&binding, COMPUTER_KEYBOARD, 127);
                }
            }
            return;
        }
        let (Some(keyboard), Some(note)) = (self.keyboard, control::key_note(code)) else {
            return;
        };
        let Ok(key) = u8::try_from(note as i16 + keyboard.transpose as i16) else {
            return;
        };
        if key > 127 {
            return;
        }
        let State {
            engine, control, ..
        } = &mut *self;
        let Control::Organ(console) = control else {
            return;
        };
        if pressed {
            let (starts, retriggered) = console.note_on_manual(keyboard.manual, key);
            for handle in retriggered {
                engine.send(Command::StopVoice { handle });
            }
            for start in starts {
                engine.send(start_command(&start));
            }
        } else {
            for handle in console.note_off_manual(keyboard.manual, key) {
                engine.send(Command::StopVoice { handle });
            }
        }
    }

    /// Point the computer keyboard at a manual, moving it off whatever
    /// it was on — one keyboard, one place, however many manuals ask.
    pub fn assign_keyboard(&mut self, manual: usize) -> bool {
        let names = self.manual_names();
        let Some(wanted) = names.get(manual).cloned() else {
            return false;
        };
        let organ = self.organ_key.clone();
        let mut transpose = 0;
        for name in &names {
            while let Some(slot) = self
                .midi_config
                .inputs(&organ, name)
                .iter()
                .position(|input| input.device == COMPUTER_KEYBOARD)
            {
                transpose = self.midi_config.inputs(&organ, name)[slot].transpose;
                self.midi_config.remove_input(&organ, name, slot);
            }
        }
        let slot = self.midi_config.inputs(&organ, &wanted).len();
        self.midi_config.set_input(
            &organ,
            &wanted,
            slot,
            config::Input {
                device: COMPUTER_KEYBOARD.to_string(),
                channel: None,
                low: None,
                high: None,
                transpose,
            },
        );
        tracing::info!("control: computer keyboard plays {wanted}");
        self.resolve_routes();
        self.persist();
        true
    }

    /// Run one action by name, as if a binding had fired it — the menu
    /// and a piston must not be able to mean different things.
    pub fn run_named(&mut self, action: &control::Action, device: &str) -> bool {
        let Some(subject) = self.resolve_subject(action, None) else {
            return false;
        };
        self.run(
            &Binding {
                channel: None,
                trigger: control::Trigger::Note(0),
                action: action.clone(),
                subject,
            },
            device,
            127,
        );
        true
    }

    /// Turn the saved bindings into the form the callbacks match
    /// against, looking every name up once. A binding naming something
    /// this organ hasn't got is reported and dropped from the live
    /// table — but stays in the file, because it is about a different
    /// instrument, not a mistake.
    fn resolve_bindings(&mut self) {
        let organ = self.organ_key.clone();
        let mut by_device: std::collections::HashMap<String, Vec<Binding>> = Default::default();
        for saved in self.midi_config.controls(&organ).to_vec() {
            if saved.trigger.trim().is_empty() {
                continue; // a row the player is still setting up
            }
            let (Some(trigger), Some(action)) = (
                control::Trigger::parse(&saved.trigger),
                control::Action::parse(&saved.action),
            ) else {
                tracing::warn!(
                    "control: {:?} → {:?} is not something Aristide knows how to do",
                    saved.trigger,
                    saved.action
                );
                continue;
            };
            let Some(subject) = self.resolve_subject(&action, saved.manual.as_deref()) else {
                tracing::warn!(
                    "control: {:?} names something this organ hasn't got — ignoring it",
                    saved.action
                );
                continue;
            };
            by_device.entry(saved.device.clone()).or_default().push(Binding {
                channel: saved.channel,
                trigger,
                action,
                subject,
            });
        }
        for port in &mut self.midi_ports {
            port.bindings = by_device.remove(&port.name).unwrap_or_default();
        }
        // The computer keyboard is a device like any other; it just
        // isn't one the operating system enumerates.
        self.key_bindings = by_device.remove(COMPUTER_KEYBOARD).unwrap_or_default();
    }

    /// The thing an action acts on, looked up in the loaded organ.
    fn resolve_subject(&self, action: &control::Action, manual: Option<&str>) -> Option<Subject> {
        let Control::Organ(console) = &self.control else {
            return Some(Subject::None);
        };
        let one = |names: &[&str], pattern: &str| {
            match aristide_formats::sidecar::match_names(names, pattern).as_slice() {
                [index] => Some(*index),
                _ => None,
            }
        };
        Some(match action {
            control::Action::Stop(name) => {
                let stops = console.stop_states();
                let names: Vec<&str> = stops.iter().map(|(_, name, _, _)| *name).collect();
                Subject::Stop(stops[one(&names, name)?].0)
            }
            control::Action::Coupler(name) => {
                let couplers = console.coupler_states();
                let names: Vec<&str> = couplers.iter().map(|(_, name, _)| *name).collect();
                Subject::Coupler(couplers[one(&names, name)?].0)
            }
            control::Action::Enclosure(name) => {
                let boxes = console.enclosure_states();
                let names: Vec<String> = boxes.iter().map(|(_, name, _, _)| name.clone()).collect();
                let names: Vec<&str> = names.iter().map(String::as_str).collect();
                Subject::Enclosure(boxes[one(&names, name)?].0)
            }
            control::Action::Transpose(_) | control::Action::TransposeReset => match manual {
                Some(name) => {
                    let names = self.manual_names();
                    let names: Vec<&str> = names.iter().map(String::as_str).collect();
                    Subject::Manual(one(&names, name)?)
                }
                None => Subject::Device,
            },
            _ => Subject::None,
        })
    }

    /// The manual a keyboard with nothing better to play should play:
    /// the Great by whatever name the set gives it, else the first
    /// manual that isn't the pedalboard.
    fn principal_manual(&self) -> Option<usize> {
        let names = self.manual_names();
        let great = names.iter().position(|name| {
            let name = name.to_lowercase();
            ["great", "haupt", "grand orgue", "grand-orgue", "main", "first"]
                .iter()
                .any(|hint| name.contains(hint))
        });
        great.or_else(|| {
            let Control::Organ(console) = &self.control else {
                return None;
            };
            // GO's convention puts the pedalboard first, so the second
            // manual is the lowest keyboard.
            (console.manual_states().len() > 1).then_some(1).or(Some(0))
        })
    }

    /// Each manual's compass as the sample set declares it.
    fn native_compass(&self) -> Vec<(u8, u8)> {
        match &self.control {
            Control::Organ(console) => (0..console.manual_states().len())
                .map(|manual| {
                    console
                        .native_compass(manual)
                        .map(|(low, high)| (low.clamp(0, 127) as u8, high.clamp(0, 127) as u8))
                        .unwrap_or((0, 127))
                })
                .collect(),
            Control::Tone => Vec::new(),
        }
    }

    /// The saved inputs of one manual, by index — what the UI edits by
    /// slot. Manuals the organ has but the file doesn't mention come
    /// back empty.
    pub fn manual_inputs(&self, manual: usize) -> Vec<config::Input> {
        self.manual_names()
            .get(manual)
            .map(|name| self.midi_config.inputs(&self.organ_key, name).to_vec())
            .unwrap_or_default()
    }

    /// Assign `input` to one manual's slot (past the end appends), then
    /// re-resolve and save. Returns false when the manual doesn't exist.
    pub fn set_input(&mut self, manual: usize, slot: usize, input: config::Input) -> bool {
        let Some(name) = self.manual_names().get(manual).cloned() else {
            return false;
        };
        tracing::info!(
            "midi: {name} ← {} channel {}",
            input.device,
            input
                .channel
                .map_or_else(|| "any".to_string(), |c| c.to_string())
        );
        let organ = self.organ_key.clone();
        self.midi_config.set_input(&organ, &name, slot, input);
        self.resolve_routes();
        self.persist();
        true
    }

    pub fn remove_input(&mut self, manual: usize, slot: usize) -> bool {
        let Some(name) = self.manual_names().get(manual).cloned() else {
            return false;
        };
        let organ = self.organ_key.clone();
        self.midi_config.remove_input(&organ, &name, slot);
        self.resolve_routes();
        self.persist();
        true
    }

    /// Do what a binding says. `value` is the message's own (a
    /// controller's position, a note's velocity); only the continuous
    /// actions read it.
    ///
    /// This is the only place an action becomes an effect, so a
    /// binding, a menu item and a future script all mean exactly the
    /// same thing by "cancel".
    pub fn run(&mut self, binding: &Binding, device: &str, value: u8) {
        let State {
            engine, control, ..
        } = &mut *self;
        let mut send = |command: Command| {
            if !engine.send(command) {
                tracing::warn!("command queue full, dropped {command:?}");
            }
        };
        match (&binding.action, binding.subject) {
            (control::Action::Transpose(by), subject) => {
                self.transpose_inputs(device, subject, |at| at.saturating_add(*by));
            }
            (control::Action::TransposeReset, subject) => {
                self.transpose_inputs(device, subject, |_| 0);
            }
            (control::Action::Stop(_), Subject::Stop(stop)) => {
                let Control::Organ(console) = control else {
                    return;
                };
                let drawn = console.is_drawn(stop);
                let (stopped, starts) = console.set_drawn(stop, !drawn);
                for handle in stopped {
                    send(Command::StopVoice { handle });
                }
                for start in starts {
                    send(start_command(&start));
                }
            }
            (control::Action::Coupler(_), Subject::Coupler(index)) => {
                let Control::Organ(console) = control else {
                    return;
                };
                let engaged = console.coupler_engaged(index);
                let (start, stop) = console.set_coupler(index, !engaged);
                if let Some(start) = start {
                    send(start_command(&start));
                }
                if let Some(handle) = stop {
                    send(Command::StopVoice { handle });
                }
            }
            (control::Action::Tremulant, _) => {
                let engaged = !self.trem_engaged;
                self.set_tremulant(engaged);
            }
            (control::Action::Cancel, _) => {
                let Control::Organ(console) = control else {
                    return;
                };
                for handle in console.cancel() {
                    send(Command::StopVoice { handle });
                }
            }
            (control::Action::Panic, _) => {
                if let Control::Organ(console) = control {
                    console.all_off();
                }
                send(Command::AllNotesOff);
            }
            (control::Action::Enclosure(_), Subject::Enclosure(index)) => {
                let Control::Organ(console) = control else {
                    return;
                };
                let position = value.min(127) as f32 / 127.0;
                if let Some((enclosure, position)) = console.set_enclosure(index, position) {
                    send(Command::SetEnclosurePosition {
                        enclosure,
                        position,
                    });
                }
            }
            _ => {}
        }
    }

    /// Engage or release the tremulant, with its own switch noise.
    pub fn set_tremulant(&mut self, on: bool) {
        let changed = self.trem_engaged != on;
        self.trem_engaged = on;
        for group in self.trem_groups.clone() {
            self.engine.send(Command::SetTremulant { group, engaged: on });
        }
        if !changed {
            return;
        }
        let State {
            engine, control, ..
        } = &mut *self;
        if let Control::Organ(console) = control {
            let (start, stop) = console.tremulant_toggle_noise(on);
            if let Some(start) = start {
                engine.send(start_command(&start));
            }
            if let Some(handle) = stop {
                engine.send(Command::StopVoice { handle });
            }
        }
    }

    /// Shift the keyboards a pitch action applies to: one manual's, or
    /// every one on the device the trigger came from — which is what a
    /// transposer built into a console means by "up".
    fn transpose_inputs(&mut self, device: &str, subject: Subject, to: impl Fn(i8) -> i8) {
        let organ = self.organ_key.clone();
        // The computer keyboard's default assignment is implied, not
        // written; shifting it is the moment it becomes a real choice.
        if device == COMPUTER_KEYBOARD
            && let Some(keyboard) = self.keyboard
            && !self
                .manual_names()
                .iter()
                .any(|name| {
                    self.midi_config
                        .inputs(&organ, name)
                        .iter()
                        .any(|input| input.device == COMPUTER_KEYBOARD)
                })
        {
            self.assign_keyboard(keyboard.manual);
        }
        let names = self.manual_names();
        let mut changed = false;
        for (index, name) in names.iter().enumerate() {
            for input in self.midi_config.inputs_mut(&organ, name) {
                let mine = match subject {
                    Subject::Manual(manual) => manual == index,
                    _ => input.device == device,
                };
                if !mine {
                    continue;
                }
                let shifted = to(input.transpose).clamp(-36, 36);
                if shifted != input.transpose {
                    input.transpose = shifted;
                    changed = true;
                    tracing::info!("control: {} on {name} now plays {shifted:+} semitones", input.device);
                }
            }
        }
        if changed {
            self.resolve_routes();
            self.persist();
        }
    }

    /// The learn target, or `None` once it has waited too long. Checked
    /// wherever the state is observed, so a forgotten dialog doesn't
    /// keep eating notes.
    pub fn learning(&mut self) -> Option<Learn> {
        if self
            .learn
            .as_ref()
            .is_some_and(|l| l.started.elapsed() > LEARN_TIMEOUT)
        {
            tracing::info!("midi: nothing played — stopped listening");
            self.learn = None;
        }
        self.learn.clone()
    }

    /// The binding row waiting for its control, if the wait is still on.
    pub fn control_learning(&mut self) -> Option<ControlLearn> {
        if self
            .control_learn
            .is_some_and(|l| l.started.elapsed() > LEARN_TIMEOUT)
        {
            tracing::info!("control: nothing pressed — stopped listening");
            self.control_learn = None;
        }
        self.control_learn
    }

    pub fn listen_control(&mut self, slot: usize) {
        self.control_learn = Some(ControlLearn {
            slot,
            started: Instant::now(),
        });
    }

    /// One control pressed while listening: the row it was waiting for
    /// takes this device, channel and trigger, and keeps its action.
    fn learn_control(&mut self, device: &str, channel: Option<u8>, trigger: control::Trigger) {
        let Some(learn) = self.control_learning() else {
            return;
        };
        self.control_learn = None;
        let organ = self.organ_key.clone();
        let saved = self.midi_config.controls(&organ).get(learn.slot).cloned();
        let control = config::Control {
            device: device.to_string(),
            channel,
            trigger: trigger.to_string(),
            action: saved
                .as_ref()
                .map_or_else(|| "octave-up".to_string(), |c| c.action.clone()),
            manual: saved.and_then(|c| c.manual),
        };
        tracing::info!(
            "control: {device} {} → {}",
            control.trigger,
            control.action
        );
        self.midi_config.set_control(&organ, learn.slot, control);
        self.resolve_routes();
        self.persist();
    }

    pub fn set_control(&mut self, slot: usize, control: config::Control) {
        let organ = self.organ_key.clone();
        self.midi_config.set_control(&organ, slot, control);
        self.resolve_routes();
        self.persist();
    }

    pub fn remove_control(&mut self, slot: usize) {
        let organ = self.organ_key.clone();
        self.midi_config.remove_control(&organ, slot);
        self.resolve_routes();
        self.persist();
    }

    pub fn controls(&self) -> Vec<config::Control> {
        self.midi_config.controls(&self.organ_key).to_vec()
    }

    pub fn listen(&mut self, manual: usize, slot: usize) {
        self.learn = Some(Learn {
            manual,
            slot,
            heard: None,
            started: Instant::now(),
        });
    }

    /// One key played while listening. The first names the keyboard and
    /// the bottom of its compass; the second fixes the top and writes
    /// the assignment. Pressing the same key twice is a slip, not a
    /// one-key keyboard, so it keeps waiting.
    fn learn_key(&mut self, device: &str, channel: u8, key: u8) {
        let Some(mut learn) = self.learning() else {
            return;
        };
        match learn.heard.take() {
            None => {
                tracing::info!("midi: heard {device} channel {} key {key}", channel + 1);
                learn.heard = Some(config::Input {
                    device: device.to_string(),
                    channel: Some(channel + 1),
                    low: Some(key),
                    high: None,
                    transpose: 0,
                });
                learn.started = Instant::now();
                self.learn = Some(learn);
            }
            Some(input) if input.low == Some(key) => {
                learn.heard = Some(input);
                self.learn = Some(learn);
            }
            Some(mut input) => {
                input.high = Some(key);
                self.learn = None;
                self.set_input(learn.manual, learn.slot, input);
            }
        }
    }

    /// Write the assignments back for this organ. Called after every
    /// change, so quitting never loses one.
    fn persist(&mut self) {
        let Some(path) = self.config_path.clone() else {
            return;
        };
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
    let (sample_bank, control, suggested_channels) = match &args.set {
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
            let suggested = suggested_channels(&organ, &sidecar.midi.channels);
            let mut console = Console::new(organ, loaded.specs, drawn, sample_rate);
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
            (loaded.bank, Control::Organ(console), suggested)
        }
        None => {
            tracing::info!("no sample set given — playing the test tone");
            (Default::default(), Control::Tone, Vec::new())
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
    let state = Arc::new(Mutex::new(State {
        engine: handle,
        control,
        midi_ports: Vec::new(),
        midi_config,
        config_path,
        organ_key,
        suggested_channels,
        learn: None,
        control_learn: None,
        key_bindings: Vec::new(),
        keyboard: None,
        trem_groups,
        trem_engaged: false,
        master_gain: args.master_gain.unwrap_or(0.178),
        reverb_wet: reverb_ir.is_some().then_some(reverb_wet),
        expression_cc,
    }));
    // Assignments exist before any hardware does: the computer
    // keyboard and every binding are live from the first note.
    state.lock().expect("state poisoned").resolve_routes();
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
                ports.push(MidiPort {
                    name: name.clone(),
                    routes: Vec::new(),
                    bindings: Vec::new(),
                });
            }
            Err(err) => tracing::warn!("midi: failed to connect to {name}: {err}"),
        }
    }
    // Routing comes from the config file, keyed by this organ: a device
    // the player has not placed on THIS instrument is silent, however it
    // was set on another. Resolving after the whole list is published
    // keeps a port's id and its routes in step.
    {
        let mut state = state.lock().expect("state poisoned");
        state.midi_ports = ports;
        state.resolve_routes();
        for port in &state.midi_ports {
            match port.routes.len() {
                0 => tracing::info!(
                    "midi: {} plays nothing — assign it in Preferences → MIDI",
                    port.name
                ),
                count => tracing::info!("midi: {} drives {count} manual(s)", port.name),
            }
        }
    }

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

    // Learning swallows the key that teaches it. The player is looking
    // at Preferences, not at the music desk, and a division blurting out
    // mid-assignment reads as a fault.
    if status & 0xF0 == 0x90 && data2 > 0 && state.learning().is_some() {
        let Some(device) = state.midi_ports.get(port).map(|p| p.name.clone()) else {
            return;
        };
        state.learn_key(&device, channel, data1);
        return;
    }

    // Teaching a binding: the first message that could be a control
    // becomes one, and does nothing else on its way past.
    if state.control_learning().is_some() {
        let trigger = match (status & 0xF0, data2) {
            (0x90, velocity) if velocity > 0 => Some(control::Trigger::Note(data1)),
            (0xB0, value) if value >= control::SWITCH_ON => Some(control::Trigger::Control(data1)),
            (0xC0, _) => Some(control::Trigger::Program(data1)),
            _ => None,
        };
        if let Some(trigger) = trigger
            && let Some(device) = state.midi_ports.get(port).map(|p| p.name.clone())
        {
            state.learn_control(&device, Some(channel + 1), trigger);
        }
        return;
    }

    // A bound message is a control, not a note: a piston that also
    // sounded the key it sits under would be unusable. Note-offs of a
    // bound note are swallowed with it.
    let Some(source) = state.midi_ports.get(port) else {
        return;
    };
    let device = source.name.clone();
    if let Some(fired) = matching_bindings(&source.bindings, status, channel, data1) {
        for binding in fired {
            state.run(&binding, &device, data2);
        }
        return;
    }

    // A manual with no input assigned is deaf to everything, including
    // note-offs: it sent no note-ons either, so nothing can hang. The M1
    // test tone has no manuals to assign to and always sounds.
    let source = &state.midi_ports[port];
    // Notes are filtered by the keyboard's compass as well as its
    // channel; everything else (expression, all-notes-off) is not a key
    // and only has to reach the right manuals.
    let targets = match (&state.control, status & 0xF0) {
        (Control::Tone, _) => Vec::new(),
        (_, 0x90 | 0x80) => source.note_targets(channel, data1),
        _ => source.targets(channel),
    };
    // The keyboard's own shift: which pipes its keys reach, exactly as
    // a transposer on a console moves the whole keyboard.
    let key = match (status & 0xF0, source.transpose(channel, data1)) {
        (0x90 | 0x80, Some(shifted)) => shifted,
        (0x90 | 0x80, None) => return,
        _ => data1,
    };
    if matches!(state.control, Control::Organ(_)) && targets.is_empty() {
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
    match (status & 0xF0, key, data2) {
        (0x90, key, velocity) if velocity > 0 => match control {
            Control::Tone => send(Command::NoteOn {
                key,
                freq_hz: midi_note_to_hz(key),
            }),
            Control::Organ(console) => {
                for manual in targets {
                    let (starts, retriggered) = console.note_on_manual(manual, key);
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
            }
        },
        (0x80, key, _) | (0x90, key, 0) => match control {
            Control::Tone => send(Command::NoteOff { key }),
            Control::Organ(console) => {
                for manual in targets {
                    for handle in console.note_off_manual(manual, key) {
                        send(Command::StopVoice { handle });
                    }
                }
            }
        },
        (0xB0, 120..=123, _) => {
            if let Control::Organ(console) = control {
                console.all_off();
            }
            send(Command::AllNotesOff);
        }
        // Expression pedal: drive the swell boxes of whatever manuals
        // this input plays.
        (0xB0, cc, value) if cc == expression_cc => {
            if let Control::Organ(console) = control {
                for manual in targets {
                    for (enclosure, position) in console.expression_manual(manual, value) {
                        send(Command::SetEnclosurePosition {
                            enclosure,
                            position,
                        });
                    }
                }
            }
        }
        _ => {}
    }
}

/// The bindings a message fires, if any. A note-off of a bound note
/// matches too, so it can be swallowed rather than reaching a manual
/// the note-on never did.
fn matching_bindings(
    bindings: &[Binding],
    status: u8,
    channel: u8,
    data1: u8,
) -> Option<Vec<Binding>> {
    if bindings.is_empty() {
        return None;
    }
    let fired: Vec<Binding> = bindings
        .iter()
        .filter(|binding| binding.channel.is_none_or(|on| on == channel + 1))
        .filter(|binding| match (&binding.trigger, status & 0xF0) {
            (control::Trigger::Note(note), 0x90 | 0x80) => *note == data1,
            (control::Trigger::Control(cc), 0xB0) => *cc == data1,
            (control::Trigger::Program(program), 0xC0) => *program == data1,
            _ => false,
        })
        .cloned()
        .collect();
    if fired.is_empty() {
        return None;
    }
    // Switch-like bindings act on the press only; continuous ones
    // follow every message.
    let acting: Vec<Binding> = fired
        .into_iter()
        .filter(|binding| binding.action.is_continuous() || status & 0xF0 != 0x80)
        .collect();
    Some(acting)
}

/// The engine command that starts one console voice.
fn start_command(start: &console::VoiceStart) -> Command {
    Command::StartVoice {
        handle: start.handle,
        sample: start.spec.sample,
        rate: start.spec.rate,
        gain: start.spec.gain,
        group: start.spec.group,
        wind_weight: start.spec.wind_weight,
        brightness: start.spec.brightness,
        enclosure: start.spec.enclosure,
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
        let console = Console::new(organ, loaded.specs, drawn, 48_000.0);
        let (_engine, engine) = Engine::new(48000.0, Arc::new(loaded.bank));
        let state = Arc::new(Mutex::new(State {
            engine,
            control: Control::Organ(console),
            midi_ports: vec![MidiPort {
                name: "Test Keyboard".into(),
                routes: Vec::new(),
                bindings: Vec::new(),
            }],
            midi_config: Default::default(),
            config_path: None,
            organ_key: "test organ".into(),
            suggested_channels: Vec::new(),
            learn: None,
            control_learn: None,
            key_bindings: Vec::new(),
            keyboard: None,
            trem_groups: Vec::new(),
            trem_engaged: false,
            master_gain: 0.178,
            reverb_wet: None,
            expression_cc: 11,
        }));
        // Everything downstream reads the resolved tables, exactly as
        // the server does once before it opens any device.
        state.lock().expect("state").resolve_routes();
        Some((state, manual))
    }

    fn held_on(state: &Mutex<State>, manual: usize) -> Vec<u8> {
        let state = state.lock().expect("state poisoned");
        let Control::Organ(console) = &state.control else {
            panic!("organ expected");
        };
        console.manual_states()[manual].4.clone()
    }

    /// Assign the one test port to a manual, the way the dialog does.
    fn bind(state: &Mutex<State>, manual: usize, channel: Option<u8>) {
        let slot = state.lock().expect("state poisoned").manual_inputs(manual).len();
        bind_compass(state, manual, slot, channel, None, None);
    }

    fn bind_compass(
        state: &Mutex<State>,
        manual: usize,
        slot: usize,
        channel: Option<u8>,
        low: Option<u8>,
        high: Option<u8>,
    ) {
        let mut state = state.lock().expect("state poisoned");
        state.set_input(
            manual,
            slot,
            config::Input {
                device: "Test Keyboard".into(),
                channel,
                low,
                high,
                transpose: 0,
            },
        );
    }

    fn compass_of(state: &Mutex<State>, manual: usize) -> Option<(i16, i16)> {
        let state = state.lock().expect("state poisoned");
        match &state.control {
            Control::Organ(console) => console.compass(manual),
            Control::Tone => None,
        }
    }

    /// The point of assigning by manual: a plain keyboard that only ever
    /// speaks on channel 1 can still be the second manual.
    #[test]
    fn a_keyboard_plays_the_manual_it_is_assigned_to() {
        let Some((state, manual)) = demo_state("Gamba 8'") else {
            return;
        };
        assert_eq!(manual, 2, "the fixture's stop is on the second manual");

        bind(&state, manual, None);
        handle_midi(&[0x90, 60, 100], 0, &state);
        assert_eq!(held_on(&state, manual), vec![60], "any channel plays it");
        handle_midi(&[0x80, 60, 0], 0, &state);
        assert!(held_on(&state, manual).is_empty(), "and releases it again");
    }

    /// One DIN cable, several manuals: the channel is what tells them
    /// apart, and a message on the wrong one reaches nothing.
    #[test]
    fn a_channel_bound_input_hears_only_that_channel() {
        let Some((state, manual)) = demo_state("Gamba 8'") else {
            return;
        };
        bind(&state, manual, Some(2));

        handle_midi(&[0x90, 60, 100], 0, &state); // channel 1
        assert!(held_on(&state, manual).is_empty(), "not this manual's channel");
        handle_midi(&[0x80, 60, 0], 0, &state);

        handle_midi(&[0x91, 60, 100], 0, &state); // channel 2
        assert_eq!(held_on(&state, manual), vec![60]);
        handle_midi(&[0x81, 60, 0], 0, &state);
        assert!(held_on(&state, manual).is_empty());
    }

    /// The default on an organ nobody has configured: an input the
    /// player has not placed sounds nothing at all, rather than guessing
    /// a division from its MIDI channel.
    #[test]
    fn an_unassigned_device_is_silent() {
        let Some((state, manual)) = demo_state("Gamba 8'") else {
            return;
        };
        handle_midi(&[0x90, 60, 100], 0, &state);
        for index in 0..=manual {
            assert!(
                held_on(&state, index).is_empty(),
                "unassigned input plays nothing, manual {index}"
            );
        }
    }

    /// Auto-detect: the dialog waits, the player plays its lowest and
    /// highest key, and neither sounds — a division blurting out
    /// mid-assignment reads as a fault.
    #[test]
    fn listening_takes_the_keyboard_and_its_width_from_two_presses() {
        let Some((state, manual)) = demo_state("Gamba 8'") else {
            return;
        };
        state.lock().expect("state").listen(manual, 0);

        // The first key names the keyboard and the bottom of its range.
        handle_midi(&[0x92, 55, 100], 0, &state); // channel 3
        assert!(
            held_on(&state, manual).is_empty(),
            "the teaching keys do not sound"
        );
        assert!(
            state.lock().expect("state").learn.is_some(),
            "still waiting for the top"
        );
        // A repeat of the same key is a slip, not a one-key keyboard.
        handle_midi(&[0x92, 55, 100], 0, &state);
        assert!(state.lock().expect("state").learn.is_some());

        handle_midi(&[0x92, 96, 100], 0, &state);
        let locked = state.lock().expect("state");
        assert_eq!(
            locked.manual_inputs(manual),
            [config::Input {
                device: "Test Keyboard".into(),
                channel: Some(3),
                low: Some(55),
                high: Some(96),
                transpose: 0,
            }],
            "port, channel and compass all come from the playing"
        );
        assert!(locked.learn.is_none(), "two keys are enough");
        drop(locked);

        // The route is live at once, and the manual now answers to the
        // keyboard's own range rather than the set's.
        assert_eq!(compass_of(&state, manual), Some((55, 96)));
        handle_midi(&[0x92, 60, 100], 0, &state);
        assert_eq!(held_on(&state, manual), vec![60]);
    }

    /// The player's keyboard is the compass: inside it every key plays,
    /// outside it none does, whatever the sample set's own range.
    #[test]
    fn a_keyboards_compass_decides_which_notes_exist() {
        let Some((state, manual)) = demo_state("Gamba 8'") else {
            return;
        };
        let native = compass_of(&state, manual).expect("a compass");
        let (native_low, native_high) = (native.0 as u8, native.1 as u8);
        bind_compass(&state, manual, 0, None, Some(48), Some(60));
        assert_eq!(compass_of(&state, manual), Some((48, 60)));

        handle_midi(&[0x90, 61, 100], 0, &state);
        assert!(held_on(&state, manual).is_empty(), "past this keyboard");
        handle_midi(&[0x90, 60, 100], 0, &state);
        assert_eq!(held_on(&state, manual), vec![60], "its top key plays");
        handle_midi(&[0x80, 60, 0], 0, &state);

        // Widened past the set, the extra keys speak — repitched.
        bind_compass(&state, manual, 0, None, Some(native_low), Some(native_high + 5));
        assert_eq!(compass_of(&state, manual), Some((native.0, native.1 + 5)));
        handle_midi(&[0x90, native_high + 5, 100], 0, &state);
        assert_eq!(
            held_on(&state, manual),
            vec![native_high + 5],
            "five keys past the set, repitched from its top pipe"
        );
        handle_midi(&[0x80, native_high + 5, 0], 0, &state);

        // A second keyboard on the same manual brings its own width,
        // and the manual answers to both.
        bind_compass(&state, manual, 1, None, Some(24), Some(48));
        assert_eq!(compass_of(&state, manual), Some((24, native.1 + 5)));
    }

    fn bind_control(state: &Mutex<State>, trigger: &str, action: &str, device: &str) {
        let mut state = state.lock().expect("state poisoned");
        let slot = state.controls().len();
        state.set_control(
            slot,
            config::Control {
                device: device.into(),
                channel: None,
                trigger: trigger.into(),
                action: action.into(),
                manual: None,
            },
        );
    }

    /// A piston is a control, not a note: it does its job and the key it
    /// sits on stays silent.
    #[test]
    fn a_bound_note_works_the_console_instead_of_playing() {
        let Some((state, manual)) = demo_state("Gamba 8'") else {
            return;
        };
        bind(&state, manual, None);
        let stop = {
            let state = state.lock().expect("state");
            let Control::Organ(console) = &state.control else {
                panic!("organ expected")
            };
            console
                .stop_states()
                .iter()
                .find(|(_, name, _, _)| *name == "Gamba 8'")
                .expect("the fixture's stop")
                .0
        };
        bind_control(&state, "note:36", "stop:Gamba 8'", "Test Keyboard");

        // The fixture draws that stop to make notes audible, so the
        // piston's first press is a retire and its second a draw.
        let drawn = |state: &Mutex<State>| {
            let state = state.lock().expect("state");
            let Control::Organ(console) = &state.control else {
                panic!("organ expected")
            };
            console.is_drawn(stop)
        };
        assert!(drawn(&state), "the fixture starts with it drawn");

        handle_midi(&[0x90, 36, 100], 0, &state);
        assert!(held_on(&state, manual).is_empty(), "a piston is not a key");
        assert!(!drawn(&state), "the piston worked the stop it names");

        // Its note-off is swallowed with it, and pressing again toggles.
        handle_midi(&[0x80, 36, 0], 0, &state);
        handle_midi(&[0x90, 36, 100], 0, &state);
        assert!(drawn(&state), "a second press draws it again");
    }

    /// Octave up moves the *keyboard*, not the division: the pipes its
    /// keys reach change, and the manual's compass follows.
    #[test]
    fn octave_up_shifts_the_keyboard_that_pressed_it() {
        let Some((state, manual)) = demo_state("Gamba 8'") else {
            return;
        };
        bind_compass(&state, manual, 0, None, Some(48), Some(72));
        bind_control(&state, "note:24", "octave-up", "Test Keyboard");

        handle_midi(&[0x90, 60, 100], 0, &state);
        assert_eq!(held_on(&state, manual), vec![60]);
        handle_midi(&[0x80, 60, 0], 0, &state);

        handle_midi(&[0x90, 24, 100], 0, &state);
        assert_eq!(
            state.lock().expect("state").manual_inputs(manual)[0].transpose,
            12,
            "the keyboard is shifted, and the shift is saved"
        );
        handle_midi(&[0x90, 60, 100], 0, &state);
        assert_eq!(
            held_on(&state, manual),
            vec![72],
            "the same key now reaches an octave higher"
        );
        handle_midi(&[0x80, 60, 0], 0, &state);

        // Down twice lands an octave below where it started.
        bind_control(&state, "note:25", "octave-down", "Test Keyboard");
        handle_midi(&[0x90, 25, 100], 0, &state);
        handle_midi(&[0x90, 25, 100], 0, &state);
        assert_eq!(
            state.lock().expect("state").manual_inputs(manual)[0].transpose,
            -12
        );
        handle_midi(&[0x90, 60, 100], 0, &state);
        assert_eq!(held_on(&state, manual), vec![48]);
    }

    /// The computer keyboard is an input like any other: the same
    /// binding vocabulary, the same shift, and it plays without anyone
    /// assigning it first.
    #[test]
    fn computer_keys_play_and_can_be_bound() {
        let Some((state, _)) = demo_state("Montre 8'") else {
            return;
        };
        let keyboard = state
            .lock()
            .expect("state")
            .keyboard
            .expect("assigned by default");
        assert_eq!(keyboard.transpose, 0);

        state.lock().expect("state").key("KeyZ", true);
        assert_eq!(
            held_on(&state, keyboard.manual),
            vec![48],
            "the bottom row starts at C3"
        );
        state.lock().expect("state").key("KeyZ", false);
        assert!(held_on(&state, keyboard.manual).is_empty());

        bind_control(&state, "key:Equal", "octave-up", COMPUTER_KEYBOARD);
        state.lock().expect("state").key("Equal", true);
        state.lock().expect("state").key("KeyZ", true);
        assert_eq!(
            held_on(&state, keyboard.manual),
            vec![60],
            "one octave up, from a computer key"
        );

        state.lock().expect("state").key("KeyZ", false);

        // A key with a job does not also play a note.
        bind_control(&state, "key:KeyX", "cancel", COMPUTER_KEYBOARD);
        state.lock().expect("state").key("KeyX", true);
        assert!(
            held_on(&state, keyboard.manual).is_empty(),
            "a bound key plays nothing"
        );
        let state = state.lock().expect("state");
        let Control::Organ(console) = &state.control else {
            panic!("organ expected")
        };
        assert!(
            console.stop_states().iter().all(|(_, _, _, drawn)| !drawn),
            "and did what it was bound to: cancel"
        );
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
        let input = config::Input {
            device: "Test Keyboard".into(),
            channel: Some(2),
            low: None,
            high: None,
            transpose: 0,
        };
        state
            .midi_config
            .set_input(&organ, "Second Manual", 0, input.clone());
        state.resolve_routes();
        assert_eq!(state.midi_ports[0].routes.len(), 1);
        assert_eq!(state.midi_ports[0].routes[0].manual, manual);
        assert_eq!(state.midi_ports[0].routes[0].channel, Some(2));

        // A manual this organ hasn't got, and an assignment saved under
        // a different organ, both leave the port silent rather than
        // sounding the wrong division.
        state.midi_config.remove_input(&organ, "Second Manual", 0);
        state
            .midi_config
            .set_input(&organ, "Positif de dos", 0, input.clone());
        state
            .midi_config
            .set_input("Another Organ", "Second Manual", 0, input);
        state.resolve_routes();
        assert!(state.midi_ports[0].routes.is_empty());
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

        // First Manual, Second Manual, Pedal — read backwards, the
        // channel each manual conventionally speaks on.
        let suggested = suggested_channels(&organ, &sidecar.midi.channels);
        assert_eq!(suggested, vec![Some(3), Some(1), Some(2)]);
    }
}
