mod bank;
mod config;
mod console;
mod control;
mod http;
mod load;
mod tuning;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use anyhow::{Context, Result};
use aristide_engine::{Command, Engine, EngineHandle};
use aristide_formats::instrument;
use aristide_model::StopId;
use console::Console;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use midir::{Ignore, MidiInput, MidiInputConnection};

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
#[derive(Clone)]
pub struct Route {
    /// MIDI channel 1-16; `None` accepts any.
    pub channel: Option<u8>,
    pub manual: usize,
    /// The keyboard's compass — inclusive MIDI notes. Notes outside it
    /// are not this keyboard's to send, so they are ignored.
    pub keys: (u8, u8),
    /// Semitones this keyboard is currently shifted by.
    pub transpose: i8,
    /// Pitch-bend range in semitones; `None` = this keyboard's bends
    /// are ignored (see [`config::Input::bend`]).
    pub bend: Option<f32>,
    /// Lumatone key map for a generalized keyboard. When present, the
    /// map replaces the channel/compass fields: it alone decides which
    /// (channel, note) pairs play and which manual key each addresses.
    /// Keys land in extended-note numbering — the map's Nth used
    /// channel contributes keys N×128..N×128+127 — so a layout that
    /// continues note numbers across channels (the Lumatone Editor's
    /// convention for >128-key layouts) reads as one contiguous pitch
    /// ladder for the tuning layer.
    pub map: Option<std::sync::Arc<aristide_model::lumatone::LumatoneMap>>,
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

    /// Where a note from this port lands, as (manual, shifted key)
    /// pairs: it must be inside a keyboard's own compass — the width
    /// the player taught it is the only thing that decides which notes
    /// exist — and each route applies its own shift, since two manuals
    /// on one device may sit at different octaves. A shift that pushes
    /// a key off the MIDI range drops that landing alone.
    /// The widest bend range any of this port's routes grants notes on
    /// `channel`, if any grants one at all. Per channel because that is
    /// how MPE addresses notes; per port because the range is a fact
    /// about the controller.
    fn bend_range(&self, channel: u8) -> Option<f32> {
        self.routes
            .iter()
            .filter(|route| route.channel.is_none_or(|on| on == channel + 1))
            .filter_map(|route| route.bend)
            .max_by(f32::total_cmp)
    }

    fn note_lands(&self, channel: u8, key: u8) -> Vec<(usize, u16)> {
        let mut lands: Vec<(usize, u16)> = self
            .routes
            .iter()
            .filter_map(|route| {
                if let Some(map) = &route.map {
                    // A mapped keyboard: the map is the whole story —
                    // membership, channel handling, and the landing key
                    // in extended-note numbering.
                    map.key_for(channel, key)?;
                    let rank = map.channels().position(|used| used == channel)? as i32;
                    let extended = rank * 128 + key as i32 + route.transpose as i32;
                    return u16::try_from(extended).ok().map(|key| (route.manual, key));
                }
                if !route.channel.is_none_or(|on| on == channel + 1)
                    || !(route.keys.0..=route.keys.1).contains(&key)
                {
                    return None;
                }
                u8::try_from(key as i16 + route.transpose as i16)
                    .ok()
                    .filter(|shifted| *shifted < 128)
                    .map(|shifted| (route.manual, u16::from(shifted)))
            })
            .collect();
        lands.sort_unstable();
        lands.dedup();
        lands
    }

    /// The extended-note span a mapped route can land on (before its
    /// transpose): what compass widening uses in place of the learned
    /// `keys` range.
    fn map_reach(map: &aristide_model::lumatone::LumatoneMap) -> Option<(u16, u16)> {
        let mut low = u16::MAX;
        let mut high = 0u16;
        let mut any = false;
        for (rank, channel) in map.channels().enumerate() {
            for note in 0..128u16 {
                if map.key_for(channel, note as u8).is_some() {
                    let extended = rank as u16 * 128 + note;
                    low = low.min(extended);
                    high = high.max(extended);
                    any = true;
                }
            }
        }
        any.then_some((low, high))
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

/// A bind parked mid-air: what it proposes already has a job, and the
/// console is asking whether it now has two. One device driving two
/// manuals, or one message doing two things, are both legitimate wants
/// — but never ones to create silently.
#[derive(Clone)]
pub enum Pending {
    /// `input` wants this manual's slot, but the same device (on an
    /// overlapping channel) already plays the rows in `existing`,
    /// as (manual index, slot) pairs.
    Input {
        manual: usize,
        slot: usize,
        input: config::Input,
        existing: Vec<(usize, usize)>,
    },
    /// `control` wants this slot, but the same message from the same
    /// device is already bound at the slots in `existing`.
    Control {
        slot: usize,
        control: config::Control,
        existing: Vec<usize>,
    },
}

/// The player's answer to a parked bind.
#[derive(Clone, Copy, PartialEq)]
pub enum Resolution {
    /// Both jobs stand: the device drives the old rows and the new one.
    KeepBoth,
    /// The old rows go; the new one takes over what they knew about the
    /// hardware (a learned compass, a shift) unless it states its own.
    Replace,
    Cancel,
}

/// Whether two channel filters can hear the same message: `None` is
/// "any channel", so it overlaps everything.
fn channels_overlap(a: Option<u8>, b: Option<u8>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => a == b,
        _ => true,
    }
}

/// Channels and a learned compass mean nothing to the computer
/// keyboard — its width is the two letter rows, always.
fn normalize_input(input: &mut config::Input) {
    if input.device == COMPUTER_KEYBOARD {
        input.channel = None;
        input.low = None;
        input.high = None;
    }
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
    /// A bind waiting on the player: it would give a device (or one of
    /// its messages) a second job, and the console is showing the
    /// keep-both / replace / cancel dialog.
    pub pending: Option<Pending>,
    /// Bindings on the computer keyboard, which no operating system
    /// enumerates but which is otherwise an input like the rest.
    pub key_bindings: Vec<Binding>,
    /// Where the computer keyboard's notes go, and how far each
    /// assignment is shifted. Assigned like any other input, in the
    /// config — and like any device it may drive more than one manual,
    /// once the player has confirmed that is what they meant.
    pub keyboard: Vec<KeyboardInput>,
    /// Live notes per (port, channel, incoming note): the (manual, key)
    /// landings each produced, so a later per-channel pitch bend can
    /// find them. Populated only for bend-enabled inputs.
    pub live_notes: HashMap<(usize, u8, u8), Vec<(usize, u16)>>,
    /// The current bend per (port, channel), in cents — an MPE member's
    /// bend routinely arrives before its note-on, and the note must
    /// start already bent.
    pub channel_bend: HashMap<(usize, u8), f64>,
    /// Lumatone maps by the path the config spelled, loaded once;
    /// `None` records a failed load so it isn't retried every rescan.
    pub ltn_cache: HashMap<String, Option<std::sync::Arc<aristide_model::lumatone::LumatoneMap>>>,
    /// Wind groups the tremulant acts on (from the sidecar; empty when
    /// no set is loaded).
    pub trem_groups: Vec<u8>,
    pub trem_engaged: bool,
    pub master_gain: f32,
    /// Reverb wet level; `None` = no IR loaded.
    pub reverb_wet: Option<f32>,
    /// MIDI controller number driving swell boxes (sidecar `[enclosures] cc`).
    pub expression_cc: u8,
    /// Set when the loaded organ is a composite definition file loaded
    /// on its own: that file owns the rig's MIDI wiring, so every
    /// assignment change is written back into its `[midi]` section.
    pub composite_path: Option<PathBuf>,
    /// How this instrument was put together — what the setup dialog
    /// asks about and what `/api/organ/save` writes to a file.
    pub setup: Setup,
    /// Per composite manual: a player-declared compass overriding the
    /// set's own. Asked when sets are combined; editable later in
    /// Preferences; saved into the composite file.
    pub compass_overrides: Vec<Option<(u8, u8)>>,
    /// Sources waiting to be loaded, queued by the picker (or the CLI at
    /// startup). The main thread owns the audio stream, so it is the one
    /// that performs the load and swaps the result in.
    pub pending_load: Option<LoadRequest>,
    /// What the load in progress is doing right now, for a UI watching.
    pub loading: Option<String>,
    /// Why the last load failed, kept until the next one starts.
    pub load_error: Option<String>,
    /// What the last load skipped or papered over (dangling organ-file
    /// references, ignored sidecar lines), kept until the next one
    /// starts — an organ that loads emptier than its file intends must
    /// say so where the player is looking, not only in the log.
    pub load_warnings: Vec<String>,
    /// Where the console's movable panels sit, by panel id
    /// (`"keyboard:<manual>"`, `"jamb:<manual>"`, `"couplers"`,
    /// `"shoes"`) — only the ones a player has explicitly placed.
    /// Loaded from the organ file's `[console.layout]` and kept in
    /// step with it; purely cosmetic, so editing it never rebuilds the
    /// engine.
    pub layout: std::collections::BTreeMap<String, instrument::PanelPos>,
}

/// One request to load an instrument, from the picker or the CLI.
pub struct LoadRequest {
    pub paths: Vec<PathBuf>,
    /// CLI `--stops` registration patterns; empty means each source's
    /// sidecar default. The picker never sets these.
    pub stops: Vec<String>,
    /// Queued by the command line: a failure should exit the process,
    /// as a bad CLI path always has, not leave a silent server running.
    pub initial: bool,
}

/// The provenance of the loaded instrument.
#[derive(Default)]
pub struct Setup {
    /// Label and path of each source set, in load order.
    pub sources: Vec<(String, PathBuf)>,
    /// Every whole-division pull: (source index, source manual name,
    /// composite manual index). Replaying these rebuilds the organ.
    pub pulls: Vec<(usize, String, usize)>,
    /// Stops moved between manuals this session, as (stop, from, to)
    /// names in order — the form `[[move]]` takes in a saved file.
    pub moves: Vec<(String, String, String)>,
    /// Combined ad hoc on the CLI: nothing on disk holds this
    /// instrument yet, so the console should ask how it goes together
    /// and offer to save it.
    pub implicit: bool,
}

impl State {
    pub fn manual_names(&self) -> Vec<String> {
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

    /// The compass a manual answers to before any keyboard widens it:
    /// the player's declared override, else the set's own.
    fn compass_override(&self, manual: usize) -> Option<(u8, u8)> {
        self.compass_overrides.get(manual).copied().flatten()
    }

    /// Push the saved assignments into the connected ports. Every edit
    /// goes through the config, so this is the one place routing is
    /// derived and the MIDI callback never has to look at names.
    fn resolve_routes(&mut self) {
        self.resolve_bindings();
        let assignments = self.saved_assignments();
        // Lumatone maps load once per path, against the organ file's
        // directory like scale files. A file that fails to load warns
        // and leaves its input deaf — never bricking the organ.
        let base = self
            .composite_path
            .as_deref()
            .and_then(std::path::Path::parent)
            .map(std::path::Path::to_path_buf);
        for (_, inputs) in &assignments {
            for input in inputs {
                let Some(path) = &input.map else { continue };
                if self.ltn_cache.contains_key(path) {
                    continue;
                }
                let file = std::path::Path::new(path);
                let resolved = match (file.is_relative(), &base) {
                    (true, Some(base)) => base.join(file),
                    _ => file.to_path_buf(),
                };
                let loaded = std::fs::read_to_string(&resolved)
                    .map_err(|err| format!("{}: {err}", resolved.display()))
                    .and_then(|text| aristide_model::lumatone::LumatoneMap::parse(&text));
                let entry = match loaded {
                    Ok(map) => {
                        for warning in &map.warnings {
                            tracing::warn!("lumatone map {path}: {warning}");
                        }
                        tracing::info!(
                            "lumatone map {path}: {} keys over {} channels",
                            map.key_count(),
                            map.channels().count()
                        );
                        Some(std::sync::Arc::new(map))
                    }
                    Err(err) => {
                        tracing::warn!(
                            "lumatone map {path} not loaded: {err} — the input stays silent"
                        );
                        None
                    }
                };
                self.ltn_cache.insert(path.clone(), entry);
            }
        }
        let native: Vec<(u8, u8)> = self
            .native_compass()
            .iter()
            .enumerate()
            .map(|(manual, own)| self.compass_override(manual).unwrap_or(*own))
            .collect();
        let ltn_cache = &self.ltn_cache;
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
                            bend: input.bend,
                            map: input
                                .map
                                .as_ref()
                                .and_then(|path| ltn_cache.get(path).cloned().flatten()),
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
        // A declared compass is the floor of the union: a narrow
        // learned keyboard must not shrink the manual below it.
        let mut widened: Vec<Option<(i16, i16)>> = (0..native.len())
            .map(|manual| {
                self.compass_override(manual)
                    .map(|(low, high)| (low as i16, high as i16))
            })
            .collect();
        for port in &self.midi_ports {
            for route in &port.routes {
                // A mapped keyboard reaches the map's extended-note
                // span; a plain one its (learned) MIDI compass.
                let reach: (i16, i16) = if let Some(map) = &route.map {
                    let Some((low, high)) = MidiPort::map_reach(map) else {
                        continue;
                    };
                    let shift = |key: u16| {
                        (key as i32 + route.transpose as i32).clamp(0, i16::MAX as i32) as i16
                    };
                    (shift(low), shift(high))
                } else {
                    let shift =
                        |key: u8| (key as i16 + route.transpose as i16).clamp(0, 127);
                    (shift(route.keys.0), shift(route.keys.1))
                };
                let slot = &mut widened[route.manual];
                *slot = Some(match *slot {
                    Some((low, high)) => (low.min(reach.0), high.max(reach.1)),
                    None => reach,
                });
            }
        }
        // The computer keyboard is assigned like any other input — or
        // not at all: unassigned, its keys play nothing. Unlike a MIDI
        // keyboard it never counts towards a manual's compass. Widening
        // serves real hardware (a 61-note keyboard on a 56-note set);
        // two QWERTY rows are not a console, and letting them reshape
        // the instrument would rescale a manual nobody asked to change.
        // Keys past the manual's end simply stay silent, and the legend
        // already draws them as unavailable.
        self.keyboard = assignments
            .iter()
            .flat_map(|(manual, inputs)| {
                inputs
                    .iter()
                    .filter(|input| input.device == COMPUTER_KEYBOARD)
                    .map(|input| KeyboardInput {
                        manual: *manual,
                        transpose: input.transpose,
                        compass: control::keyboard_compass(),
                    })
            })
            .collect();
        if let Control::Organ(console) = &mut self.control {
            for (manual, compass) in widened.into_iter().enumerate() {
                match compass {
                    Some((low, high)) => console.set_compass(manual, low, high),
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
        let Some(note) = control::key_note(code) else {
            return;
        };
        // The keyboard may drive more than one manual — a confirmed
        // "keep both" — and each assignment carries its own shift.
        for keyboard in self.keyboard.clone() {
            let Ok(key) = u8::try_from(note as i16 + keyboard.transpose as i16) else {
                continue;
            };
            if key > 127 {
                continue;
            }
            let State {
                engine, control, ..
            } = &mut *self;
            let Control::Organ(console) = control else {
                return;
            };
            if pressed {
                // The compass rule, exactly as MIDI routing applies it: a
                // key landing outside the manual says nothing, and is not
                // tracked as held either. The keyboard never widens the
                // manual to reach it — the legend draws it as unavailable.
                let within = console
                    .compass(keyboard.manual)
                    .is_some_and(|(low, high)| (low..=high).contains(&(key as i16)));
                if !within {
                    continue;
                }
                let (starts, retriggered) =
                    console.note_on_manual(keyboard.manual, u16::from(key));
                for handle in retriggered {
                    engine.send(Command::StopVoice { handle });
                }
                for start in starts {
                    engine.send(start_command(&start));
                }
            } else {
                for handle in console.note_off_manual(keyboard.manual, u16::from(key)) {
                    engine.send(Command::StopVoice { handle });
                }
            }
        }
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
                let names: Vec<&str> = stops.iter().map(|(_, name, _, _, _)| *name).collect();
                Subject::Stop(stops[one(&names, name)?].0)
            }
            control::Action::Coupler(name) => {
                let couplers = console.coupler_states();
                let names: Vec<&str> = couplers.iter().map(|(_, name, _, _)| *name).collect();
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

    /// The bind path every UI edit takes: commit `input`, unless the
    /// same device on an overlapping channel already plays another row
    /// — then park it and ask whether the device now drives both. A
    /// row's identity is its device and channel: an edit that keeps
    /// them (a shift, a compass) never asks, so answering "keep both"
    /// once is answered for good. Returns false when the manual
    /// doesn't exist.
    pub fn propose_input(&mut self, manual: usize, slot: usize, mut input: config::Input) -> bool {
        self.pending = None;
        let names = self.manual_names();
        let Some(name) = names.get(manual) else {
            return false;
        };
        normalize_input(&mut input);
        let organ = self.organ_key.clone();
        let saved = self.midi_config.inputs(&organ, name).get(slot);
        let identity_kept =
            saved.is_some_and(|s| s.device == input.device && s.channel == input.channel);
        if !identity_kept {
            let mut existing = Vec::new();
            for (other_manual, other_name) in names.iter().enumerate() {
                for (other_slot, other) in
                    self.midi_config.inputs(&organ, other_name).iter().enumerate()
                {
                    if other_manual == manual && other_slot == slot {
                        continue; // the row being rewritten
                    }
                    if other.device == input.device
                        && channels_overlap(other.channel, input.channel)
                    {
                        existing.push((other_manual, other_slot));
                    }
                }
            }
            if !existing.is_empty() {
                tracing::info!("midi: {} already plays elsewhere — asking", input.device);
                self.pending = Some(Pending::Input {
                    manual,
                    slot,
                    input,
                    existing,
                });
                return true;
            }
        }
        self.set_input(manual, slot, input)
    }

    /// Act on the player's answer to a parked bind. `false` when there
    /// was nothing pending — the dialog raced an organ load or another
    /// edit, and there is nothing left to act on.
    pub fn resolve_pending(&mut self, resolution: Resolution) -> bool {
        let Some(pending) = self.pending.take() else {
            return false;
        };
        match (resolution, pending) {
            (Resolution::Cancel, _) => {}
            (
                Resolution::KeepBoth,
                Pending::Input {
                    manual,
                    slot,
                    input,
                    ..
                },
            ) => {
                self.set_input(manual, slot, input);
            }
            (Resolution::KeepBoth, Pending::Control { slot, control, .. }) => {
                self.set_control(slot, control);
            }
            (
                Resolution::Replace,
                Pending::Input {
                    manual,
                    mut slot,
                    mut input,
                    mut existing,
                },
            ) => {
                let organ = self.organ_key.clone();
                let names = self.manual_names();
                // The rows being replaced knew things about the hardware
                // itself — a learned compass, a shift. Replacing means
                // "this keyboard now plays here instead", so those facts
                // move with it unless the new row states its own.
                if let Some(&(other_manual, other_slot)) = existing.first()
                    && let Some(other_name) = names.get(other_manual)
                    && let Some(old) = self.midi_config.inputs(&organ, other_name).get(other_slot)
                {
                    if input.transpose == 0 {
                        input.transpose = old.transpose;
                    }
                    if input.low.is_none() && input.high.is_none() {
                        input.low = old.low;
                        input.high = old.high;
                    }
                    normalize_input(&mut input);
                }
                // Remove bottom-up so earlier removals never shift the
                // later slots; the target slides down past any removed
                // row beneath it on its own manual.
                existing.sort_unstable();
                for &(other_manual, other_slot) in existing.iter().rev() {
                    if let Some(other_name) = names.get(other_manual) {
                        self.midi_config.remove_input(&organ, other_name, other_slot);
                    }
                    if other_manual == manual && other_slot < slot {
                        slot -= 1;
                    }
                }
                self.set_input(manual, slot, input);
            }
            (
                Resolution::Replace,
                Pending::Control {
                    mut slot,
                    control,
                    mut existing,
                },
            ) => {
                let organ = self.organ_key.clone();
                existing.sort_unstable();
                for &other in existing.iter().rev() {
                    self.midi_config.remove_control(&organ, other);
                    if other < slot {
                        slot -= 1;
                    }
                }
                self.set_control(slot, control);
            }
        }
        true
    }

    /// Assign `input` to one manual's slot (past the end appends), then
    /// re-resolve and save. Returns false when the manual doesn't exist.
    pub fn set_input(&mut self, manual: usize, slot: usize, mut input: config::Input) -> bool {
        let Some(name) = self.manual_names().get(manual).cloned() else {
            return false;
        };
        normalize_input(&mut input);
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
        self.pending = None;
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
                let (stopped, starts) = console.set_coupler(index, !engaged);
                for handle in stopped {
                    send(Command::StopVoice { handle });
                }
                for start in starts {
                    send(start_command(&start));
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
        self.pending = None;
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
        self.propose_control(learn.slot, control);
    }

    /// The bind path every UI edit takes: commit `control`, unless the
    /// same message from the same device is already bound elsewhere —
    /// then park it and ask. A row's identity is its device, channel
    /// and trigger: an edit that keeps them (choosing another action,
    /// naming a manual) never asks, so answering "keep both" once is
    /// answered for good.
    pub fn propose_control(&mut self, slot: usize, control: config::Control) {
        self.pending = None;
        let organ = self.organ_key.clone();
        let saved = self.midi_config.controls(&organ).get(slot);
        let identity_kept = saved.is_some_and(|s| {
            s.device == control.device
                && s.channel == control.channel
                && s.trigger == control.trigger
        });
        if !identity_kept && !control.trigger.trim().is_empty() {
            let existing: Vec<usize> = self
                .midi_config
                .controls(&organ)
                .iter()
                .enumerate()
                .filter(|(other, saved)| {
                    *other != slot
                        && saved.device == control.device
                        && saved.trigger == control.trigger
                        && channels_overlap(saved.channel, control.channel)
                })
                .map(|(other, _)| other)
                .collect();
            if !existing.is_empty() {
                tracing::info!(
                    "control: {} on {} is already bound — asking",
                    control.trigger,
                    control.device
                );
                self.pending = Some(Pending::Control {
                    slot,
                    control,
                    existing,
                });
                return;
            }
        }
        self.set_control(slot, control);
    }

    pub fn set_control(&mut self, slot: usize, control: config::Control) {
        let organ = self.organ_key.clone();
        self.midi_config.set_control(&organ, slot, control);
        self.resolve_routes();
        self.persist();
    }

    pub fn remove_control(&mut self, slot: usize) {
        self.pending = None;
        let organ = self.organ_key.clone();
        self.midi_config.remove_control(&organ, slot);
        self.resolve_routes();
        self.persist();
    }

    pub fn controls(&self) -> Vec<config::Control> {
        self.midi_config.controls(&self.organ_key).to_vec()
    }

    pub fn listen(&mut self, manual: usize, slot: usize) {
        self.pending = None;
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
                    bend: None,
                    map: None,
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
                self.propose_input(learn.manual, learn.slot, input);
            }
        }
    }

    /// Declare (or with `None` retract) a manual's compass, live and —
    /// when the organ lives in a file that declares the manual — in
    /// that file. Returns false for a manual the organ hasn't got.
    pub fn set_compass_override(&mut self, manual: usize, compass: Option<(u8, u8)>) -> bool {
        let names = self.manual_names();
        if manual >= names.len() {
            return false;
        }
        self.compass_overrides.resize(names.len().max(self.compass_overrides.len()), None);
        self.compass_overrides[manual] = compass;
        self.resolve_routes();
        if let Some(path) = self.composite_path.clone() {
            match config::write_composite_compass(&path, &names[manual], compass) {
                Ok(true) => {}
                Ok(false) => tracing::warn!(
                    "compass not saved: {} has no [[manual]] named {:?} — declare it \
                     to keep this compass",
                    path.display(),
                    names[manual]
                ),
                Err(err) => tracing::warn!("compass not saved: {err}"),
            }
        }
        true
    }

    /// Move a stop to another manual — live under held keys, kept for
    /// saving, and appended to the organ's file when it has one.
    pub fn move_stop(&mut self, stop: StopId, manual: usize) -> bool {
        let names = self.manual_names();
        let Some(to_name) = names.get(manual).cloned() else {
            return false;
        };
        let State {
            engine, control, ..
        } = &mut *self;
        let Control::Organ(console) = control else {
            return false;
        };
        let Some((stop_name, from_name)) = console
            .stop_states()
            .iter()
            .find(|(id, ..)| *id == stop)
            .map(|(_, name, from, _, _)| (name.to_string(), from.to_string()))
        else {
            return false;
        };
        if from_name == to_name {
            return true;
        }
        let (stopped, starts) = console.move_stop(stop, manual);
        for handle in stopped {
            engine.send(Command::StopVoice { handle });
        }
        for start in starts {
            engine.send(start_command(&start));
        }
        self.setup
            .moves
            .push((stop_name.clone(), from_name.clone(), to_name.clone()));
        if let Some(path) = &self.composite_path
            && let Err(err) = config::append_composite_move(path, &stop_name, &from_name, &to_name)
        {
            tracing::warn!("move not saved: {err}");
        }
        true
    }

    /// Keep a coupler on the console or take it off — live, and in the
    /// organ's file when it has one. Off is not gone: the routes stay,
    /// so the Organ preferences can put it back.
    pub fn set_coupler_pick(&mut self, index: usize, keep: bool) -> bool {
        let State {
            engine, control, ..
        } = &mut *self;
        let Control::Organ(console) = control else {
            return false;
        };
        if index >= console.coupler_states().len() {
            return false;
        }
        let (stopped, starts) = console.set_coupler_available(index, keep);
        for handle in stopped {
            engine.send(Command::StopVoice { handle });
        }
        for start in starts {
            engine.send(start_command(&start));
        }
        let dropped: Vec<String> = console
            .coupler_states()
            .iter()
            .filter(|(_, _, _, available)| !available)
            .map(|(_, name, _, _)| name.to_string())
            .collect();
        if let Some(path) = &self.composite_path
            && let Err(err) = config::write_composite_drops(path, &dropped)
        {
            tracing::warn!("coupler pick not saved: {err}");
        }
        true
    }

    /// Tune one division apart from the instrument, or with `None`
    /// return it to the shared tuning — live from the next note, and
    /// in the organ's file when it declares the manual.
    /// A live tuning as the organ file spells it.
    fn tuning_fields_of(tuning: &tuning::Tuning) -> config::ManualTuningFields {
        config::ManualTuningFields {
            temperament: tuning.temperament.name().to_string(),
            a4_hz: tuning.a4_hz,
            transpose: tuning.transpose,
            scale: tuning.scale.as_ref().map(|scale| scale.scl.clone()),
            keymap: tuning.scale.as_ref().and_then(|scale| scale.kbm.clone()),
        }
    }

    pub fn tune_manual(&mut self, manual: usize, tuning: Option<tuning::Tuning>) -> bool {
        let names = self.manual_names();
        if manual >= names.len() {
            return false;
        }
        let Control::Organ(console) = &mut self.control else {
            return false;
        };
        let fields = tuning.as_ref().map(Self::tuning_fields_of);
        console.set_manual_tuning(manual, tuning);
        if let Some(path) = self.composite_path.clone() {
            match config::write_composite_manual_tuning(&path, &names[manual], fields) {
                Ok(true) => {}
                Ok(false) => tracing::warn!(
                    "manual tuning not saved: {} has no [[manual]] named {:?} — declare \
                     it to keep this tuning",
                    path.display(),
                    names[manual]
                ),
                Err(err) => tracing::warn!("manual tuning not saved: {err}"),
            }
        }
        true
    }

    /// Write this instrument — sources, manuals with their effective
    /// compasses, division pulls, and the current MIDI wiring — as a
    /// composite organ file, which from now on is where it lives.
    pub fn save_composite(&mut self, path: PathBuf) -> Result<(), String> {
        if self.composite_path.is_some() {
            return Err("this organ already lives in a file".into());
        }
        if self.setup.sources.is_empty() {
            return Err("no sample set loaded".into());
        }
        let names = self.manual_names();
        let native = self.native_compass();
        let manuals: Vec<config::SavedManual> = names
            .iter()
            .enumerate()
            .map(|(manual, name)| {
                let (low, high) = self.compass_override(manual).unwrap_or(native[manual]);
                let tuning = match &self.control {
                    Control::Organ(console) => console.manual_tuning(manual),
                    Control::Tone => None,
                };
                let tuning = tuning.as_ref().map(Self::tuning_fields_of);
                config::SavedManual {
                    name: name.clone(),
                    kind: match &self.control {
                        Control::Organ(console) => console.manual_kind(manual),
                        Control::Tone => Default::default(),
                    },
                    low,
                    high,
                    tuning,
                }
            })
            .collect();
        let dropped: Vec<String> = match &self.control {
            Control::Organ(console) => console
                .coupler_states()
                .iter()
                .filter(|(_, _, _, available)| !available)
                .map(|(_, name, _, _)| name.to_string())
                .collect(),
            Control::Tone => Vec::new(),
        };
        config::save_composite(
            &path,
            &self.organ_key,
            &self.setup.sources,
            &manuals,
            &self.setup.pulls,
            &self.setup.moves,
            &dropped,
        )?;
        tracing::info!("organ saved: {}", path.display());
        // The file is now the way to load this organ again.
        let canonical = path.canonicalize().unwrap_or_else(|_| path.clone());
        self.midi_config.remember(&self.organ_key, &canonical);
        self.composite_path = Some(path);
        self.setup.implicit = false;
        // The new file owns the wiring from here on; write it in.
        self.persist();
        Ok(())
    }

    /// Rename the loaded organ, everywhere the name is load-bearing at
    /// once: the file that owns it (a composite's `name`, or a sample
    /// set's sidecar), the assignments in `midi_config` — they are
    /// keyed by name, so they must move or the rename would silently
    /// unwire the organ — the library's display name (its path key is
    /// untouched), and the live console. Nothing is written under the
    /// new name until the file write has succeeded.
    pub fn rename_organ(&mut self, name: &str) -> Result<(), String> {
        let name = name.trim();
        if name.is_empty() {
            return Err("the organ needs a name".into());
        }
        let Control::Organ(console) = &mut self.control else {
            return Err("no organ is loaded".into());
        };
        if console.organ_name() == name {
            return Ok(());
        }
        // Where the name lives on disk. An implicit combination has no
        // file yet, so there is nowhere durable to put its name.
        let file = if let Some(path) = &self.composite_path {
            config::write_composite_name(path, name)?;
            path.canonicalize().unwrap_or_else(|_| path.clone())
        } else if self.setup.implicit {
            return Err(
                "this combination isn't saved as a file yet — save it in \
                 Preferences → Organ first"
                    .into(),
            );
        } else if let Some((_, path)) = self.setup.sources.first() {
            config::write_sidecar_name(path, name)?;
            path.clone()
        } else {
            return Err("no organ is loaded".into());
        };
        let old_key = std::mem::replace(&mut self.organ_key, name.to_string());
        if let Some(wiring) = self.midi_config.organs.remove(&old_key) {
            self.midi_config.organs.insert(name.to_string(), wiring);
        }
        console.set_organ_name(name.to_string());
        for entry in &mut self.midi_config.library {
            if entry.path == file {
                entry.name = name.to_string();
            }
        }
        // The setup pane labels the organ's own file by its name; the
        // sets inside a saved combination keep their labels.
        for (label, path) in &mut self.setup.sources {
            if *path == file {
                *label = name.to_string();
            }
        }
        tracing::info!("organ renamed: {old_key:?} → {name:?}");
        self.persist();
        Ok(())
    }

    // ---- organ-pane editor ---------------------------------------------
    //
    // Structural edits go through the organ's file: write the line,
    // then reload the file so the console plays exactly what it says.
    // The running organ keeps sounding until the rebuilt one swaps in.

    /// The file every organ-pane edit writes to. Blank and adopted
    /// organs always have one; only unsaved CLI combinations don't.
    fn organ_file(&self) -> Result<PathBuf, String> {
        self.composite_path.clone().ok_or_else(|| {
            "this combination isn't saved as a file yet — save it in \
             Preferences → Organ first"
                .to_string()
        })
    }

    /// Queue reloading the organ's own file after a structural edit.
    fn reload_organ_file(&mut self, path: PathBuf) {
        self.loading = Some("rebuilding the organ…".to_string());
        self.load_error = None;
        self.load_warnings.clear();
        self.pending_load = Some(LoadRequest {
            paths: vec![path],
            stops: Vec::new(),
            initial: false,
        });
    }

    pub fn add_manual(
        &mut self,
        name: &str,
        low: u8,
        high: u8,
        kind: aristide_model::ManualKind,
    ) -> Result<(), String> {
        if self
            .manual_names()
            .iter()
            .any(|existing| existing.eq_ignore_ascii_case(name.trim()))
        {
            return Err(format!("this organ already has a manual named {:?}", name.trim()));
        }
        let path = self.organ_file()?;
        config::append_composite_manual(&path, name, low, high, kind)?;
        self.reload_organ_file(path);
        Ok(())
    }

    /// Redeclare a manual's kind — pedalboard, hand keyboard or a
    /// generalized (microtonal) key field. Structural: the console
    /// redraws the keyboard, so the file line is followed by a reload.
    pub fn set_manual_kind(
        &mut self,
        manual: usize,
        kind: aristide_model::ManualKind,
    ) -> Result<(), String> {
        let names = self.manual_names();
        let Some(name) = names.get(manual) else {
            return Err("no such manual".into());
        };
        let path = self.organ_file()?;
        if !config::write_composite_manual_kind(&path, name, kind)? {
            return Err(format!(
                "{} has no [[manual]] named {name:?} — it wasn't declared by this file",
                path.display()
            ));
        }
        self.reload_organ_file(path);
        Ok(())
    }

    pub fn rename_manual(&mut self, manual: usize, name: &str) -> Result<(), String> {
        let names = self.manual_names();
        let Some(old) = names.get(manual) else {
            return Err("no such manual".into());
        };
        let name = name.trim();
        if name.is_empty() {
            return Err("the manual needs a name".into());
        }
        if old == name {
            return Ok(());
        }
        if names.iter().any(|existing| existing.eq_ignore_ascii_case(name)) {
            return Err(format!("this organ already has a manual named {name:?}"));
        }
        let path = self.organ_file()?;
        if !config::rename_composite_manual(&path, old, name)? {
            return Err(format!(
                "{} has no [[manual]] named {old:?} — it wasn't declared by this file",
                path.display()
            ));
        }
        // Assignments are keyed by manual name; they follow the rename
        // or the manual would silently unwire.
        if let Some(organ) = self.midi_config.organs.get_mut(&self.organ_key) {
            if let Some(inputs) = organ.manuals.remove(old.as_str()) {
                organ.manuals.insert(name.to_string(), inputs);
            }
            for control in &mut organ.controls {
                if control
                    .manual
                    .as_deref()
                    .is_some_and(|m| m.eq_ignore_ascii_case(old))
                {
                    control.manual = Some(name.to_string());
                }
            }
        }
        for prefix in ["keyboard", "jamb"] {
            if let Some(pos) = self.layout.remove(&format!("{prefix}:{old}")) {
                self.layout.insert(format!("{prefix}:{name}"), pos);
            }
        }
        self.persist();
        self.reload_organ_file(path);
        Ok(())
    }

    pub fn remove_manual(&mut self, manual: usize) -> Result<(), String> {
        let names = self.manual_names();
        let Some(name) = names.get(manual) else {
            return Err("no such manual".into());
        };
        let path = self.organ_file()?;
        if !config::remove_composite_manual(&path, name)? {
            return Err(format!(
                "{} has no [[manual]] named {name:?} — it wasn't declared by this file",
                path.display()
            ));
        }
        if let Some(organ) = self.midi_config.organs.get_mut(&self.organ_key) {
            organ.manuals.remove(name.as_str());
            organ.controls.retain(|control| {
                !control
                    .manual
                    .as_deref()
                    .is_some_and(|m| m.eq_ignore_ascii_case(name))
            });
        }
        for prefix in ["keyboard", "jamb"] {
            self.layout.remove(&format!("{prefix}:{name}"));
        }
        self.persist();
        self.reload_organ_file(path);
        Ok(())
    }

    pub fn reorder_manual(&mut self, manual: usize, to: usize) -> Result<(), String> {
        let names = self.manual_names();
        let Some(name) = names.get(manual) else {
            return Err("no such manual".into());
        };
        let path = self.organ_file()?;
        if !config::reorder_composite_manual(&path, name, to)? {
            return Err(format!(
                "{} has no [[manual]] named {name:?} — it wasn't declared by this file",
                path.display()
            ));
        }
        self.reload_organ_file(path);
        Ok(())
    }

    /// Add a sample set to the organ's `[sources]`. Nothing is pulled
    /// yet — the pane's source browser offers its material from here.
    pub fn add_organ_source(&mut self, set: &std::path::Path) -> Result<String, String> {
        if !set.is_file() {
            return Err(format!("{}: not a file", set.display()));
        }
        if instrument::is_definition(set) {
            return Err("a source must be a sample set, not another organ file".into());
        }
        let canonical = set.canonicalize().unwrap_or_else(|_| set.to_path_buf());
        let path = self.organ_file()?;
        config::append_composite_source(&path, &canonical)
    }

    /// Pull one stop (or with `stop` absent a whole division) from a
    /// source onto a manual of this organ.
    pub fn pull_from_source(
        &mut self,
        from: &str,
        source_manual: &str,
        stop: Option<&str>,
        on: &str,
    ) -> Result<(), String> {
        if !self
            .manual_names()
            .iter()
            .any(|name| name.eq_ignore_ascii_case(on))
        {
            return Err(format!("this organ has no manual named {on:?}"));
        }
        let path = self.organ_file()?;
        config::append_composite_pull(&path, from, source_manual, stop, on)?;
        self.reload_organ_file(path);
        Ok(())
    }

    /// Delete a stop: remove the `[[stop]]` line that pulled it in.
    /// The source still offers it, so the pane can pull it back.
    pub fn remove_stop(&mut self, stop: StopId) -> Result<(), String> {
        let Control::Organ(console) = &self.control else {
            return Err("no organ is loaded".into());
        };
        let Some((name, manual_name)) = console
            .stop_states()
            .iter()
            .find(|(id, ..)| *id == stop)
            .map(|(_, name, manual, _, _)| (name.to_string(), manual.to_string()))
        else {
            return Err("no such stop".into());
        };
        let path = self.organ_file()?;
        if !config::remove_composite_stop_pull(&path, &name, &manual_name)? {
            return Err(format!(
                "{name:?} came in as part of a whole division — edit its \
                 [[division]] line in {}",
                path.display()
            ));
        }
        self.reload_organ_file(path);
        Ok(())
    }

    /// Define a new (empty) swell box; the pane drags stops in after.
    pub fn add_enclosure(&mut self, name: &str) -> Result<(), String> {
        let path = self.organ_file()?;
        config::append_composite_enclosure(&path, name)?;
        self.reload_organ_file(path);
        Ok(())
    }

    /// Remove a file-defined swell box. Boxes a source carries in have
    /// no line here to remove, and the error says so.
    pub fn remove_enclosure(&mut self, name: &str) -> Result<(), String> {
        let path = self.organ_file()?;
        if !config::remove_composite_enclosure(&path, name)? {
            return Err(format!(
                "{name:?} is the sample set's own box, not one this file defines"
            ));
        }
        self.reload_organ_file(path);
        Ok(())
    }

    /// Put a stop into (or take it out of) a file-defined swell box.
    pub fn assign_enclosure(
        &mut self,
        enclosure: &str,
        stop: StopId,
        inside: bool,
    ) -> Result<(), String> {
        let Control::Organ(console) = &self.control else {
            return Err("no organ is loaded".into());
        };
        let Some(name) = console
            .stop_states()
            .iter()
            .find(|(id, ..)| *id == stop)
            .map(|(_, name, ..)| name.to_string())
        else {
            return Err("no such stop".into());
        };
        let path = self.organ_file()?;
        if !config::assign_composite_enclosure_stop(&path, enclosure, &name, inside)? {
            return Err(format!(
                "{enclosure:?} is the sample set's own box — only boxes this \
                 file defines hold stops by name"
            ));
        }
        self.reload_organ_file(path);
        Ok(())
    }

    /// Move a console panel to a spot on the canvas: `x`/`y` are
    /// normalized fractions, clamped into `[0, 1]` and rounded to four
    /// decimals before they're written. Cosmetic geometry only — unlike
    /// every structural edit above, this never queues a rebuild; the
    /// in-memory layout is updated directly instead.
    pub fn place_panel(&mut self, panel: &str, x: f32, y: f32) -> Result<(), String> {
        if !matches!(self.control, Control::Organ(_)) {
            return Err("no organ is loaded".into());
        }
        let manual_names = self.manual_names();
        let valid = matches!(panel, "couplers" | "shoes")
            || ["keyboard:", "jamb:"].iter().any(|prefix| {
                panel
                    .strip_prefix(prefix)
                    .is_some_and(|name| manual_names.iter().any(|existing| existing == name))
            });
        if !valid {
            return Err(format!("{panel:?} is not a panel of this organ"));
        }
        let path = self.organ_file()?;
        let round4 = |v: f32| (v.clamp(0.0, 1.0) * 10_000.0).round() / 10_000.0;
        let (x, y) = (round4(x), round4(y));
        config::write_composite_panel(&path, panel, x, y)?;
        self.layout
            .insert(panel.to_string(), instrument::PanelPos { x, y });
        Ok(())
    }

    /// Drop an organ from the picker's library and save the change.
    pub fn forget_organ(&mut self, path: &std::path::Path) -> bool {
        let removed = self.midi_config.forget(path);
        if removed {
            self.persist();
        }
        removed
    }

    /// Write the assignments back for this organ. Called after every
    /// change, so quitting never loses one. A composite organ's file
    /// owns its wiring, so the change lands there too.
    fn persist(&mut self) {
        if let Some(path) = &self.composite_path {
            let organ = self.midi_config.organ(&self.organ_key);
            if let Err(err) = config::write_composite_midi(path, organ) {
                tracing::warn!("midi wiring not saved to {}: {err}", path.display());
            }
        }
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
        anyhow::ensure!(!args.sets.is_empty(), "--list-stops needs a set path");
        for path in &args.sets {
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
    let state = Arc::new(Mutex::new(State {
        engine: handle,
        control: Control::Tone,
        midi_ports: Vec::new(),
        midi_config,
        config_path,
        organ_key: String::new(),
        suggested_channels: Vec::new(),
        learn: None,
        control_learn: None,
        pending: None,
        key_bindings: Vec::new(),
        keyboard: Vec::new(),
        live_notes: HashMap::new(),
        channel_bend: HashMap::new(),
        ltn_cache: HashMap::new(),
        trem_groups: Vec::new(),
        trem_engaged: false,
        master_gain: args.master_gain.unwrap_or(0.178),
        reverb_wet: None,
        expression_cc: 11,
        composite_path: None,
        setup: Setup::default(),
        compass_overrides: Vec::new(),
        loading: pending_load.as_ref().map(|_| "loading…".to_string()),
        pending_load,
        load_error: None,
        load_warnings: Vec::new(),
        layout: Default::default(),
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

/// The audio device and the stream playing into it, owned by the main
/// thread (a cpal stream is not `Send`). The engine's sample bank is
/// fixed at its construction — the RT path never swaps pointers — so
/// loading an organ means a new engine, and a new engine means a new
/// stream; [`AudioOutput::start`] does both.
struct AudioOutput {
    device: cpal::Device,
    config: cpal::StreamConfig,
    channels: usize,
    sample_rate: f32,
    buffer_frames: u32,
    safe: bool,
    overruns: Arc<std::sync::atomic::AtomicU32>,
    dsp_peak_ns: Arc<std::sync::atomic::AtomicU64>,
    dsp_over_budget: Arc<std::sync::atomic::AtomicU32>,
    dsp_budget_ns: Arc<std::sync::atomic::AtomicU64>,
    /// Where each new engine's recording tap goes; the recorder thread
    /// drains them all into one WAV.
    record: Option<std::sync::mpsc::Sender<rtrb::Consumer<f32>>>,
    stream: Option<cpal::Stream>,
}

impl AudioOutput {
    /// Widen the stream to at least `wanted` channels for a routed
    /// organ, keeping the sample rate (the bank was decoded against
    /// it). No such layout is a warning, not an error: the engine
    /// folds unreachable output pairs back onto the main pair.
    fn ensure_channels(&mut self, wanted: usize) {
        if wanted <= self.channels {
            return;
        }
        let rate = self.config.sample_rate.0;
        let found = self
            .device
            .supported_output_configs()
            .ok()
            .and_then(|configs| {
                configs
                    .filter(|range| range.sample_format() == cpal::SampleFormat::F32)
                    .filter(|range| range.channels() as usize >= wanted)
                    .filter(|range| {
                        (range.min_sample_rate().0..=range.max_sample_rate().0).contains(&rate)
                    })
                    .min_by_key(|range| range.channels())
                    .map(|range| range.with_sample_rate(cpal::SampleRate(rate)))
            });
        match found {
            Some(config) => {
                let config: cpal::StreamConfig = config.into();
                tracing::info!(
                    "audio: widening the stream to {} channels for routing",
                    config.channels
                );
                self.channels = config.channels as usize;
                self.config = config;
            }
            None => tracing::warn!(
                "audio: routing wants {wanted} channels but the device offers no such \
                 f32 layout at {rate} Hz — routed buses fold to the main pair"
            ),
        }
    }

    /// Replace the running engine (and its stream) with a fresh one on
    /// `bank`. Falls back to the backend's default buffer size if the
    /// requested one is refused.
    fn start(
        &mut self,
        bank: Arc<aristide_engine::bank::SampleBank>,
        reverb: Option<(Arc<aristide_engine::reverb::PreparedIr>, f32)>,
    ) -> Result<EngineHandle> {
        // Drop the old stream first: exclusive backends refuse a second
        // stream on the device, and the old engine (with its bank) dies
        // with it here on the control side, never on the audio thread.
        self.stream = None;
        #[cfg(target_os = "linux")]
        AUDIO_TID.store(0, std::sync::atomic::Ordering::Release);
        let (stream, handle) =
            match self.build(cpal::BufferSize::Fixed(self.buffer_frames), &bank, &reverb) {
                Ok(pair) => pair,
                Err(err) => {
                    tracing::warn!(
                        "device refused a {}-frame buffer ({err}); using its default \
                         (expect higher latency — try another --buffer value)",
                        self.buffer_frames
                    );
                    self.build(cpal::BufferSize::Default, &bank, &reverb)?
                }
            };
        stream.play()?;
        self.stream = Some(stream);
        // Each stream gets a fresh callback thread; promote it again.
        #[cfg(target_os = "linux")]
        std::thread::spawn(promote_audio_thread_via_rtkit);
        Ok(handle)
    }

    fn build(
        &self,
        buffer_size: cpal::BufferSize,
        bank: &Arc<aristide_engine::bank::SampleBank>,
        reverb: &Option<(Arc<aristide_engine::reverb::PreparedIr>, f32)>,
    ) -> Result<(cpal::Stream, EngineHandle)> {
        let (mut engine, handle) = Engine::new(self.sample_rate, Arc::clone(bank));
        engine.set_reverb(
            reverb.as_ref().map(|(ir, _)| Arc::clone(ir)),
            reverb.as_ref().map_or(0.0, |(_, wet)| *wet),
        );
        if self.safe {
            engine.set_lite(true);
        }
        let mut tap = None;
        if self.record.is_some() {
            // ~90 s of stereo headroom; the writer drains far faster.
            let (producer, consumer) = rtrb::RingBuffer::new(1 << 23);
            engine.set_tap(producer);
            tap = Some(consumer);
        }
        let mut stream_config = self.config.clone();
        stream_config.buffer_size = buffer_size;
        let mut rt_ready = false;
        let mut last_callback: Option<std::time::Instant> = None;
        let overruns = Arc::clone(&self.overruns);
        let dsp_peak_ns = Arc::clone(&self.dsp_peak_ns);
        let dsp_over_budget = Arc::clone(&self.dsp_over_budget);
        let dsp_budget_ns = Arc::clone(&self.dsp_budget_ns);
        let channels = self.channels;
        let sample_rate = self.sample_rate;
        let buffer_hint = self.buffer_frames;
        let mut engine = engine;
        let stream = self.device.build_output_stream(
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
        // Only a real stream's tap reaches the recorder: a consumer
        // whose build failed would drain nothing forever.
        if let (Some(sender), Some(consumer)) = (&self.record, tap) {
            let _ = sender.send(consumer);
        }
        Ok((stream, handle))
    }
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
        tremulant,
        enclosures,
        expression_cc,
        reverb,
        composite,
        suggested_channels,
        setup,
        layout,
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
    let trem_groups = match &tremulant {
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

    let mut state = state.lock().expect("state poisoned");
    // Assignments are per organ, so the loaded set's own name is the
    // key its wiring is stored under.
    state.organ_key = console.organ_name().to_string();
    // A composite file owns its MIDI wiring: whatever it says replaces
    // anything the user config remembers under this organ's name, and
    // every later change is written back into the file.
    if let Some((_, midi)) = &composite {
        let organ_key = state.organ_key.clone();
        state
            .midi_config
            .organs
            .insert(organ_key, config::organ_config_from_file(midi));
    }
    // Every source lands in the library, so the picker can offer it
    // next time without the command line.
    for (label, path) in &setup.sources {
        state.midi_config.remember(label, path);
    }
    state.engine = handle;
    state.control = Control::Organ(console);
    state.suggested_channels = suggested_channels;
    state.trem_groups = trem_groups;
    state.trem_engaged = false;
    state.reverb_wet = reverb.map(|(_, wet)| wet);
    state.expression_cc = expression_cc;
    state.composite_path = composite.map(|(path, _)| path);
    state.setup = setup;
    state.compass_overrides = Vec::new();
    state.layout = layout;
    state.learn = None;
    state.control_learn = None;
    state.pending = None;
    // A pick queued while this one loaded is already the next load;
    // its narration stays up until that one lands too.
    if state.pending_load.is_none() {
        state.loading = None;
    }
    state.load_error = None;
    state.load_warnings = warnings;
    state.resolve_routes();
    state.persist();
    tracing::info!("organ ready: {}", state.organ_key);
    Ok(())
}

/// The recording tap's writer: drains every engine's tap ring into one
/// WAV (16-bit PCM). Loading an organ replaces the engine, so taps
/// arrive over a channel and each engine's output is appended in turn.
#[allow(clippy::type_complexity)]
fn spawn_recorder(
    path: PathBuf,
    rate: u32,
) -> Result<(
    std::sync::mpsc::Sender<rtrb::Consumer<f32>>,
    std::thread::JoinHandle<std::io::Result<()>>,
)> {
    tracing::info!("recording engine output to {}", path.display());
    let (sender, receiver) = std::sync::mpsc::channel::<rtrb::Consumer<f32>>();
    let worker = std::thread::Builder::new()
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
            let mut taps: Vec<rtrb::Consumer<f32>> = Vec::new();
            while !SHUTDOWN.load(std::sync::atomic::Ordering::Relaxed) {
                while let Ok(tap) = receiver.try_recv() {
                    taps.push(tap);
                }
                for tap in &mut taps {
                    while let Ok(value) = tap.pop() {
                        let clamped = (value.clamp(-1.0, 1.0) * 32767.0) as i16;
                        file.write_all(&clamped.to_le_bytes())?;
                        written = written.saturating_add(2);
                    }
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
        })?;
    Ok((sender, worker))
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

/// Glide for one pitch-bend step: about the interval between an MPE
/// controller's bend messages, so a stream of them reads as one
/// continuous motion instead of a staircase.
const BEND_GLIDE_MS: f32 = 12.0;

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
    // Notes are filtered by each keyboard's compass as well as its
    // channel, and land already shifted — per route, because one device
    // may drive two manuals whose keyboards sit at different octaves.
    // Everything else (expression, all-notes-off) is not a key and only
    // has to reach the right manuals.
    let is_note = matches!(status & 0xF0, 0x90 | 0x80);
    let (lands, targets) = match (&state.control, is_note) {
        (Control::Tone, _) => (Vec::new(), Vec::new()),
        (_, true) => (source.note_lands(channel, data1), Vec::new()),
        (_, false) => (Vec::new(), source.targets(channel)),
    };
    let bend_range = source.bend_range(channel);
    let key = data1;
    if matches!(state.control, Control::Organ(_)) && lands.is_empty() && targets.is_empty() {
        return;
    }
    let State {
        engine,
        control,
        live_notes,
        channel_bend,
        ..
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
                for &(manual, key) in &lands {
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
                            bus: start.spec.bus,
                            delay_frames: start.spec.delay_frames,
                        });
                    }
                }
                if bend_range.is_some() {
                    // A note on an already-bent MPE channel starts at
                    // the bend, not at centre — the retune snaps (the
                    // voice is a frame old, nothing to glide from).
                    let cents = channel_bend
                        .get(&(port, channel))
                        .copied()
                        .unwrap_or(0.0);
                    if cents != 0.0 {
                        for &(manual, key) in &lands {
                            for (handle, rate) in console.bend_key(manual, key, cents) {
                                send(Command::SetVoiceRate {
                                    handle,
                                    rate,
                                    glide_ms: 0.0,
                                });
                            }
                        }
                    }
                    live_notes.insert((port, channel, data1), lands);
                }
            }
        },
        (0x80, key, _) | (0x90, key, 0) => match control {
            Control::Tone => send(Command::NoteOff { key }),
            Control::Organ(console) => {
                live_notes.remove(&(port, channel, data1));
                for (manual, key) in lands {
                    for handle in console.note_off_manual(manual, key) {
                        send(Command::StopVoice { handle });
                    }
                }
            }
        },
        // Per-channel pitch bend: MPE's per-note pitch, since a member
        // channel holds one note. 14-bit centre 8192; the input's bend
        // range says what full deflection means. Inputs with no range
        // configured ignore bends entirely, as organ consoles do.
        (0xE0, lsb, msb) => {
            if let (Control::Organ(console), Some(range)) = (control, bend_range) {
                let value = (lsb as i32) | ((msb as i32) << 7);
                let cents = (value - 8192) as f64 / 8192.0 * range as f64 * 100.0;
                channel_bend.insert((port, channel), cents);
                for (_, landings) in live_notes
                    .iter()
                    .filter(|((p, c, _), _)| *p == port && *c == channel)
                {
                    for &(manual, key) in landings {
                        for (handle, rate) in console.bend_key(manual, key, cents) {
                            send(Command::SetVoiceRate {
                                handle,
                                rate,
                                glide_ms: BEND_GLIDE_MS,
                            });
                        }
                    }
                }
            }
        }
        (0xB0, 120..=123, _) => {
            if let Control::Organ(console) = control {
                console.all_off();
            }
            live_notes.clear();
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
        bus: start.spec.bus,
        delay_frames: start.spec.delay_frames,
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
            ltn_cache: HashMap::new(),
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
            pending: None,
            key_bindings: Vec::new(),
            keyboard: Vec::new(),
            live_notes: HashMap::new(),
            channel_bend: HashMap::new(),
            trem_groups: Vec::new(),
            trem_engaged: false,
            master_gain: 0.178,
            reverb_wet: None,
            expression_cc: 11,
            composite_path: None,
            setup: Default::default(),
            compass_overrides: Vec::new(),
            pending_load: None,
            loading: None,
            load_error: None,
            load_warnings: Vec::new(),
            layout: Default::default(),
        }));
        // Everything downstream reads the resolved tables, exactly as
        // the server does once before it opens any device.
        state.lock().expect("state").resolve_routes();
        Some((state, manual))
    }

    fn held_on(state: &Mutex<State>, manual: usize) -> Vec<u16> {
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
                bend: None,
                map: None,
            },
        );
    }

    /// Pick the computer keyboard for a manual, exactly as the MIDI
    /// tab's device dropdown does — there is no other assignment path.
    fn bind_computer(state: &Mutex<State>, manual: usize) -> bool {
        let mut state = state.lock().expect("state poisoned");
        let slot = state.manual_inputs(manual).len();
        state.set_input(
            manual,
            slot,
            config::Input {
                device: COMPUTER_KEYBOARD.into(),
                channel: None,
                low: None,
                high: None,
                transpose: 0,
                bend: None,
                map: None,
            },
        )
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

    /// A Lumatone map replaces channel/compass routing outright: only
    /// mapped (channel, note) pairs play, and each lands in extended-
    /// note numbering — the map's Nth used channel owns keys N×128 up.
    #[test]
    fn a_lumatone_map_routes_by_channel_and_note() {
        let ltn = "[Board0]\n\
                   Key_0=60\nChan_0=1\n\
                   Key_1=62\nChan_1=1\n\
                   Key_2=4\nChan_2=2\n\
                   Key_3=70\nChan_3=1\nKTyp_3=2\n";
        let map = aristide_model::lumatone::LumatoneMap::parse(ltn).expect("parses");
        let port = MidiPort {
            name: "Lumatone".into(),
            routes: vec![Route {
                channel: None,
                manual: 1,
                keys: (0, 127),
                transpose: 0,
                bend: None,
                map: Some(std::sync::Arc::new(map)),
            }],
            bindings: Vec::new(),
        };
        assert_eq!(port.note_lands(0, 60), vec![(1, 60)]);
        assert_eq!(port.note_lands(0, 62), vec![(1, 62)]);
        assert_eq!(
            port.note_lands(1, 4),
            vec![(1, 132)],
            "the second used channel continues 128 keys up"
        );
        assert!(port.note_lands(0, 61).is_empty(), "unmapped pair");
        assert!(port.note_lands(0, 70).is_empty(), "a CC key is not a note");
        assert!(port.note_lands(2, 60).is_empty(), "unused channel");
        let map = port.routes[0].map.as_ref().expect("map");
        assert_eq!(
            MidiPort::map_reach(map),
            Some((60, 132)),
            "compass widening sees the extended span"
        );
    }

    /// MPE per-note pitch: a member channel's bend reaches the notes
    /// that channel is holding — and only on inputs that declared a
    /// bend range, because organ consoles ignore bend wheels.
    #[test]
    fn pitch_bend_follows_the_notes_of_its_channel() {
        let Some((state, manual)) = demo_state("Gamba 8'") else {
            return;
        };
        {
            let mut locked = state.lock().expect("state poisoned");
            locked.set_input(
                manual,
                0,
                config::Input {
                    device: "Test Keyboard".into(),
                    channel: None,
                    low: None,
                    high: None,
                    transpose: 0,
                    bend: Some(48.0),
                    map: None,
                },
            );
        }
        handle_midi(&[0x90, 60, 100], 0, &state);
        handle_midi(&[0xE0, 0x7F, 0x7F], 0, &state); // full deflection up
        {
            let locked = state.lock().expect("state poisoned");
            let cents = locked.channel_bend[&(0, 0)];
            let expected = (16383.0 - 8192.0) / 8192.0 * 4800.0;
            assert!(
                (cents - expected).abs() < 1e-6,
                "48-semitone range at full deflection: {cents} vs {expected}"
            );
            assert_eq!(
                locked.live_notes[&(0, 0, 60)],
                vec![(manual, 60)],
                "the bend knows which landings its channel holds"
            );
        }
        // The bend outlives the note (MPE sends it before the next
        // note-on too), but the note's tracking ends with the note.
        handle_midi(&[0x80, 60, 0], 0, &state);
        {
            let locked = state.lock().expect("state poisoned");
            assert!(locked.live_notes.is_empty());
            assert!(locked.channel_bend.contains_key(&(0, 0)));
        }
        // An input with no bend range stays deaf to the wheel.
        {
            let mut locked = state.lock().expect("state poisoned");
            locked.channel_bend.clear();
            locked.set_input(
                manual,
                0,
                config::Input {
                    device: "Test Keyboard".into(),
                    channel: None,
                    low: None,
                    high: None,
                    transpose: 0,
                    bend: None,
                    map: None,
                },
            );
        }
        handle_midi(&[0x90, 60, 100], 0, &state);
        handle_midi(&[0xE0, 0x7F, 0x7F], 0, &state);
        let locked = state.lock().expect("state poisoned");
        assert!(locked.channel_bend.is_empty(), "no range, no bend");
        assert!(locked.live_notes.is_empty(), "and no tracking");
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
                bend: None,
                map: None,
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
            vec![u16::from(native_high + 5)],
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
                .find(|(_, name, _, _, _)| *name == "Gamba 8'")
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

    /// The computer keyboard is an input like any other: unassigned it
    /// plays nothing, and once the player gives it a manual it speaks
    /// the same binding vocabulary and shift as a MIDI console.
    #[test]
    fn computer_keys_play_and_can_be_bound() {
        let Some((state, manual)) = demo_state("Montre 8'") else {
            return;
        };
        // Mappable, never mandatory: until the player points it at a
        // manual, the letter rows are just letters.
        assert!(
            state.lock().expect("state").keyboard.is_empty(),
            "unassigned until the player says so"
        );
        state.lock().expect("state").key("KeyZ", true);
        assert!(held_on(&state, manual).is_empty(), "unassigned keys are silent");
        state.lock().expect("state").key("KeyZ", false);

        assert!(bind_computer(&state, manual));
        let keyboard = *state
            .lock()
            .expect("state")
            .keyboard
            .first()
            .expect("assigned now");
        assert_eq!(keyboard.manual, manual);
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
            console.stop_states().iter().all(|(_, _, _, _, drawn)| !drawn),
            "and did what it was bound to: cancel"
        );
    }

    /// A MIDI keyboard's width becomes the manual's compass; the
    /// computer keyboard's never does. Two QWERTY rows are not a
    /// console: however it is assigned or shifted, the manual keeps the
    /// compass it had, and keys pushed past it stay silent.
    #[test]
    fn the_computer_keyboard_never_rescales_a_manual() {
        let Some((state, manual)) = demo_state("Gamba 8'") else {
            return;
        };
        let native = compass_of(&state, manual).expect("a compass");
        assert!(bind_computer(&state, manual));
        assert_eq!(
            compass_of(&state, manual),
            Some(native),
            "assignment leaves the compass alone"
        );

        // Shift it far above the manual's top: still no widening, and
        // the keys that now land outside the compass say nothing.
        bind_control(&state, "key:Equal", "transpose:36", COMPUTER_KEYBOARD);
        state.lock().expect("state").key("Equal", true);
        assert_eq!(
            state.lock().expect("state").keyboard.first().expect("still assigned").transpose,
            36
        );
        assert_eq!(compass_of(&state, manual), Some(native), "shifted, not widened");
        state.lock().expect("state").key("KeyP", true); // 76 + 36 = 112, past the set
        assert!(
            held_on(&state, manual).is_empty(),
            "a key past the compass is silent, not repitched"
        );

        // Picking it for another manual is a second job for the same
        // keyboard: the bind parks and asks. Replace moves it — shift
        // and all — rather than duplicating it.
        let other = manual - 1;
        {
            let mut state = state.lock().expect("state");
            let slot = state.manual_inputs(other).len();
            assert!(state.propose_input(
                other,
                slot,
                config::Input {
                    device: COMPUTER_KEYBOARD.into(),
                    channel: None,
                    low: None,
                    high: None,
                    transpose: 0,
                    bend: None,
                    map: None,
                },
            ));
            assert!(
                matches!(state.pending, Some(Pending::Input { .. })),
                "a second manual for one keyboard is asked about, not assumed"
            );
            assert!(state.resolve_pending(Resolution::Replace));
            let keyboard = *state.keyboard.first().expect("still assigned");
            assert_eq!(keyboard.manual, other, "replaced means moved");
            assert_eq!(keyboard.transpose, 36, "the shift moved with it");
            assert!(
                state.manual_inputs(manual).is_empty(),
                "the old manual lost its row"
            );
        }

        // Detached — the row removed like any device's — the letter
        // rows go back to being letters.
        assert!(state.lock().expect("state").remove_input(other, 0));
        assert!(state.lock().expect("state").keyboard.is_empty());
        assert_eq!(compass_of(&state, manual), Some(native));
    }

    /// One keyboard, two divisions — a confirmed "keep both": the bind
    /// parks and asks first, and once kept, the same note-on sounds
    /// both manuals, each through its own shift.
    #[test]
    fn a_device_kept_on_two_manuals_plays_both() {
        let Some((state, manual)) = demo_state("Gamba 8'") else {
            return;
        };
        let other = manual - 1;
        bind(&state, manual, None);
        {
            let mut state = state.lock().expect("state");
            assert!(state.propose_input(
                other,
                0,
                config::Input {
                    device: "Test Keyboard".into(),
                    channel: None,
                    low: None,
                    high: None,
                    transpose: 12,
                    bend: None,
                    map: None,
                },
            ));
            assert!(
                matches!(state.pending, Some(Pending::Input { .. })),
                "a second manual for one device is asked about"
            );
            assert!(
                state.manual_inputs(other).is_empty(),
                "and nothing commits until the answer"
            );
            assert!(state.resolve_pending(Resolution::KeepBoth));
            assert_eq!(state.manual_inputs(other).len(), 1);
        }
        handle_midi(&[0x90, 60, 100], 0, &state);
        assert_eq!(held_on(&state, manual), vec![60]);
        assert_eq!(
            held_on(&state, other),
            vec![72],
            "each route lands through its own shift"
        );
        handle_midi(&[0x80, 60, 0], 0, &state);
        assert!(held_on(&state, manual).is_empty());
        assert!(held_on(&state, other).is_empty());

        // Cancel really is a no-op: propose again, walk away.
        {
            let mut state = state.lock().expect("state");
            assert!(state.propose_input(
                other,
                1,
                config::Input {
                    device: "Test Keyboard".into(),
                    channel: Some(5),
                    low: None,
                    high: None,
                    transpose: 0,
                    bend: None,
                    map: None,
                },
            ));
            assert!(state.pending.is_some());
            assert!(state.resolve_pending(Resolution::Cancel));
            assert_eq!(state.manual_inputs(other).len(), 1, "unchanged");
            assert!(!state.resolve_pending(Resolution::Cancel), "nothing left");
        }
    }

    /// The same message bound twice is asked about — for any trigger, a
    /// note as much as a control change — and "keep both" means both:
    /// one press fires both actions. Editing what a kept row *does*
    /// never re-asks; its identity (device, channel, trigger) stands.
    #[test]
    fn a_message_bound_twice_asks_then_fires_both() {
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
                .find(|(_, name, _, _, _)| *name == "Gamba 8'")
                .expect("the fixture's stop")
                .0
        };
        bind_control(&state, "note:36", "stop:Gamba 8'", "Test Keyboard");
        {
            let mut state = state.lock().expect("state");
            state.propose_control(
                1,
                config::Control {
                    device: "Test Keyboard".into(),
                    channel: None,
                    trigger: "note:36".into(),
                    action: "tremulant".into(),
                    manual: None,
                },
            );
            assert!(
                matches!(state.pending, Some(Pending::Control { .. })),
                "the same message twice is asked about"
            );
            assert_eq!(state.controls().len(), 1, "parked, not committed");
            assert!(state.resolve_pending(Resolution::KeepBoth));
            assert_eq!(state.controls().len(), 2);
        }
        handle_midi(&[0x90, 36, 100], 0, &state);
        {
            let state = state.lock().expect("state");
            let Control::Organ(console) = &state.control else {
                panic!("organ expected")
            };
            assert!(
                !console.is_drawn(stop),
                "the press worked the stop it names"
            );
            assert!(state.trem_engaged, "and the tremulant, both from one press");
        }

        // Re-pointing the kept row at another action keeps its
        // identity, so nothing asks again.
        {
            let mut state = state.lock().expect("state");
            state.propose_control(
                1,
                config::Control {
                    device: "Test Keyboard".into(),
                    channel: None,
                    trigger: "note:36".into(),
                    action: "cancel".into(),
                    manual: None,
                },
            );
            assert!(state.pending.is_none(), "same identity, no question");
            assert_eq!(state.controls()[1].action, "cancel");
        }

        // Replace retires the old rows in the new one's favour — and
        // the target slot slides down past the removals beneath it.
        {
            let mut state = state.lock().expect("state");
            state.propose_control(
                2,
                config::Control {
                    device: "Test Keyboard".into(),
                    channel: None,
                    trigger: "note:36".into(),
                    action: "panic".into(),
                    manual: None,
                },
            );
            assert!(state.pending.is_some());
            assert!(state.resolve_pending(Resolution::Replace));
            let controls = state.controls();
            assert_eq!(controls.len(), 1, "the old rows are gone");
            assert_eq!(controls[0].action, "panic");
            assert_eq!(controls[0].trigger, "note:36");
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
        let input = config::Input {
            device: "Test Keyboard".into(),
            channel: Some(2),
            low: None,
            high: None,
            transpose: 0,
            bend: None,
            map: None,
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

        let drawn = load::choose_registration(&organ, &sidecar.registration.default);
        // The sidecar names no stops: the organ starts cancelled.
        assert!(drawn.is_empty(), "no stop drawn at startup");
        // "*" is still available as an explicit full-organ pattern.
        let full = load::choose_registration(&organ, &["*".into()]);
        assert_eq!(full.len(), organ.stops.len(), "\"*\" draws every stop");

        // A named pattern still narrows to exactly what it says.
        let plein = load::choose_registration(&organ, &["plein jeu".into()]);
        let names: Vec<&str> = organ
            .stops
            .iter()
            .filter(|s| plein.contains(&s.id))
            .map(|s| s.name.as_str())
            .collect();
        assert_eq!(names, ["Plein jeu III"], "no drawstop noises");

        // First Manual, Second Manual, Pedal — read backwards, the
        // channel each manual conventionally speaks on.
        let suggested = load::suggested_channels(&organ, &sidecar.midi.channels);
        assert_eq!(suggested, vec![Some(3), Some(1), Some(2)]);
    }
}
