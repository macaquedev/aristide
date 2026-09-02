//! MIDI input: the small resolved types the control plane matches
//! messages against (`Binding`, `Route`, `MidiPort`, the MIDI-learn
//! gestures), the supervisor thread that owns every open port and
//! reconnects on hot-plug, and `handle_midi` — the callback every
//! note, controller and pitch-bend message passes through on its way
//! from a port into the engine.

use std::sync::{Arc, Mutex};
use std::time::Instant;

use anyhow::Result;
use aristide_engine::Command;
use aristide_model::units::equal_ladder_hz;
use aristide_model::StopId;
use midir::{Ignore, MidiInput, MidiInputConnection};

use crate::state::{Control, State};
use crate::{config, control, SHUTDOWN};

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
    /// Index into [`State::trems`], for a named tremulant action.
    Tremulant(usize),
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
    pub(crate) fn map_reach(map: &aristide_model::lumatone::LumatoneMap) -> Option<(u16, u16)> {
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

    /// The colour each extended manual key inherits from this port's
    /// Lumatone maps targeting `manual`, as `(key, 0xRRGGBB)`. The
    /// `.ltn` colours physical keys; this walks the same channel-rank
    /// extended numbering as [`note_lands`](Self::note_lands), so a
    /// colour lands exactly where its key's notes do. Ports without
    /// maps yield nothing.
    pub fn map_colors(&self, manual: usize) -> Vec<(u16, u32)> {
        let mut colors = Vec::new();
        for route in &self.routes {
            let Some(map) = &route.map else { continue };
            if route.manual != manual {
                continue;
            }
            for (rank, channel) in map.channels().enumerate() {
                for note in 0..128u8 {
                    let Some(physical) = map.key_for(channel, note) else { continue };
                    let Some(colour) = map.colour(physical) else { continue };
                    let extended = rank as i32 * 128 + note as i32 + route.transpose as i32;
                    if let Ok(key) = u16::try_from(extended) {
                        colors.push((key, colour));
                    }
                }
            }
        }
        colors.sort_unstable();
        colors.dedup_by_key(|(key, _)| *key);
        colors
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
    pub(crate) started: Instant,
}

/// Listening forever would leave a live console silently swallowing the
/// notes it was meant to play, so the wait gives up on its own.
pub(crate) const LEARN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(20);

/// The computer keyboard's device name, wherever a device is named: in
/// the config file, in the UI's list, in a binding.
pub const COMPUTER_KEYBOARD: &str = "Computer keyboard";

/// A binding waiting to be taught what presses it.
#[derive(Clone, Copy)]
pub struct ControlLearn {
    pub slot: usize,
    pub(crate) started: Instant,
}
/// Whether two channel filters can hear the same message: `None` is
/// "any channel", so it overlaps everything.
pub(crate) fn channels_overlap(a: Option<u8>, b: Option<u8>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => a == b,
        _ => true,
    }
}

/// Channels and a learned compass mean nothing to the computer
/// keyboard — its width is the two letter rows, always.
pub(crate) fn normalize_input(input: &mut config::Input) {
    if input.device == COMPUTER_KEYBOARD {
        input.channel = None;
        input.low = None;
        input.high = None;
    }
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
pub(crate) fn spawn_midi_supervisor(state: Arc<Mutex<State>>) {
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
        handle_learn_key_mode(&mut state, port, channel, data1);
        return;
    }

    // Teaching a binding: the first message that could be a control
    // becomes one, and does nothing else on its way past.
    if state.control_learning().is_some() {
        handle_learn_control_mode(&mut state, port, channel, status, data1, data2);
        return;
    }

    // A bound message is a control, not a note: a piston that also
    // sounded the key it sits under would be unusable. Note-offs of a
    // bound note are swallowed with it.
    match dispatch_binding(&mut state, port, status, channel, data1, data2) {
        Dispatch::NoSuchPort | Dispatch::Bound => return,
        Dispatch::Fallthrough => {}
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

    // Note-ons are the one message class that makes sound out of nowhere,
    // so each gets a timestamped log line — that's what lets a user tell
    // "phantom notes came in over MIDI" apart from every other suspect.
    if status & 0xF0 == 0x90 && data2 > 0 {
        tracing::info!("midi: note-on ch={channel} key={data1} vel={data2}");
    }
    match (status & 0xF0, key, data2) {
        (0x90, _, velocity) if velocity > 0 => {
            handle_note_on(&mut state, port, channel, key, velocity, lands, bend_range);
        }
        (0x80, _, _) | (0x90, _, 0) => {
            handle_note_off(&mut state, port, channel, key, lands);
        }
        // Per-channel pitch bend: MPE's per-note pitch, since a member
        // channel holds one note. 14-bit centre 8192; the input's bend
        // range says what full deflection means. Inputs with no range
        // configured ignore bends entirely, as organ consoles do.
        (0xE0, lsb, msb) => {
            handle_pitch_bend(&mut state, port, channel, lsb, msb, bend_range);
        }
        (0xB0, 120..=123, _) => {
            handle_all_notes_off(&mut state);
        }
        // Expression pedal: drive the swell boxes of whatever manuals
        // this input plays.
        (0xB0, cc, value) if cc == expression_cc => {
            handle_expression(&mut state, targets, value);
        }
        _ => {}
    }
}

/// The first press of a learn gesture names the keyboard (its port and
/// channel) and the bottom of its compass; the second fixes the top
/// and commits the assignment. A port that has vanished mid-gesture
/// (unplugged between the two presses) simply leaves the dialog
/// waiting rather than crashing on it.
fn handle_learn_key_mode(state: &mut State, port: usize, channel: u8, data1: u8) {
    let Some(device) = state.midi_ports.get(port).map(|p| p.name.clone()) else {
        return;
    };
    state.learn_key(&device, channel, data1);
}

/// The first message that could plausibly be a control (a note, a
/// switch-like CC, a program change) while a binding row is listening
/// becomes that binding; anything else passes through unclaimed.
fn handle_learn_control_mode(
    state: &mut State,
    port: usize,
    channel: u8,
    status: u8,
    data1: u8,
    data2: u8,
) {
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
}

/// What became of a message once bindings had first refusal on it.
enum Dispatch {
    /// `port` names no connected input; the message has nowhere to land.
    NoSuchPort,
    /// A binding fired and ran; the message is spent.
    Bound,
    /// Nothing on this device names it — free to reach a manual instead.
    Fallthrough,
}

/// Bindings get first refusal on every message: a piston that also
/// sounded the key it sits under would be unusable, and a note-off of
/// a bound note is swallowed the same way so nothing hangs.
fn dispatch_binding(
    state: &mut State,
    port: usize,
    status: u8,
    channel: u8,
    data1: u8,
    data2: u8,
) -> Dispatch {
    let Some(source) = state.midi_ports.get(port) else {
        return Dispatch::NoSuchPort;
    };
    let device = source.name.clone();
    let Some(fired) = matching_bindings(&source.bindings, status, channel, data1) else {
        return Dispatch::Fallthrough;
    };
    for binding in fired {
        state.run(&binding, &device, data2);
    }
    Dispatch::Bound
}

/// A key struck: a tone in Tone mode, or every landing this port's
/// routes give it on the loaded organ, each already shifted to its
/// route's manual. A note on an already-bent MPE channel starts at
/// the bend, not at centre — the retune snaps, since the voice is a
/// frame old and there is nothing to glide from.
fn handle_note_on(
    state: &mut State,
    port: usize,
    channel: u8,
    raw_key: u8,
    velocity: u8,
    lands: Vec<(usize, u16)>,
    bend_range: Option<f32>,
) {
    let State {
        engine,
        control,
        live_notes,
        channel_bend,
        ..
    } = state;
    let mut send = |command: Command| {
        if !engine.send(command) {
            tracing::warn!("command queue full, dropped {command:?}");
        }
    };
    match control {
        Control::Tone => send(Command::NoteOn {
            key: raw_key,
            freq_hz: midi_note_to_hz(raw_key),
        }),
        Control::Organ(console) => {
            for &(manual, key) in &lands {
                let (starts, retriggered) = console.note_on_manual(manual, key, velocity);
                for handle in retriggered {
                    send(Command::StopVoice { handle });
                }
                for start in starts {
                    send(start.command());
                }
            }
            if bend_range.is_some() {
                let cents = channel_bend.get(&(port, channel)).copied().unwrap_or(0.0);
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
                live_notes.insert((port, channel, raw_key), lands);
            }
        }
    }
}

/// A key released — or, in the Tone-mode case, just released. Landings
/// stopped on the organ may hand back `starts` of their own: a
/// Bass/Melody coupler retargeting the release onto another held key.
fn handle_note_off(
    state: &mut State,
    port: usize,
    channel: u8,
    raw_key: u8,
    lands: Vec<(usize, u16)>,
) {
    let State {
        engine,
        control,
        live_notes,
        ..
    } = state;
    let mut send = |command: Command| {
        if !engine.send(command) {
            tracing::warn!("command queue full, dropped {command:?}");
        }
    };
    match control {
        Control::Tone => send(Command::NoteOff { key: raw_key }),
        Control::Organ(console) => {
            live_notes.remove(&(port, channel, raw_key));
            for (manual, key) in lands {
                let (stopped, starts) = console.note_off_manual(manual, key);
                for handle in stopped {
                    send(Command::StopVoice { handle });
                }
                for start in starts {
                    send(start.command());
                }
            }
        }
    }
}

/// MPE per-note pitch bend: a member channel holds one note, so the
/// bend applies to every landing currently live on this port's
/// channel. 14-bit centre 8192; the input's own bend range says what
/// full deflection means.
fn handle_pitch_bend(
    state: &mut State,
    port: usize,
    channel: u8,
    lsb: u8,
    msb: u8,
    bend_range: Option<f32>,
) {
    let State {
        engine,
        control,
        live_notes,
        channel_bend,
        ..
    } = state;
    let mut send = |command: Command| {
        if !engine.send(command) {
            tracing::warn!("command queue full, dropped {command:?}");
        }
    };
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

/// The all-notes-off / all-sound-off CC range (120-123): silence the
/// organ (or the tone) outright and forget every live note.
fn handle_all_notes_off(state: &mut State) {
    let State {
        engine,
        control,
        live_notes,
        ..
    } = state;
    let mut send = |command: Command| {
        if !engine.send(command) {
            tracing::warn!("command queue full, dropped {command:?}");
        }
    };
    if let Control::Organ(console) = control {
        console.all_off();
    }
    live_notes.clear();
    send(Command::AllNotesOff);
}

/// The expression pedal's CC: drive the swell boxes of whatever
/// manuals this input plays.
fn handle_expression(state: &mut State, targets: Vec<usize>, value: u8) {
    let State { engine, control, .. } = state;
    let mut send = |command: Command| {
        if !engine.send(command) {
            tracing::warn!("command queue full, dropped {command:?}");
        }
    };
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

/// Tone mode's key→pitch: the plain equal ladder, control-side, so
/// the RT engine only ever sees frequencies. (Sampled pipes carry
/// their own pitch; the tuning layer decides theirs.)
fn midi_note_to_hz(key: u8) -> f32 {
    equal_ladder_hz(key as f64) as f32
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use aristide_engine::Engine;

    use crate::console::Console;
    use crate::state::{Pending, Resolution, TremControl};
    use crate::{bank, load};

    /// A general captures the console and brings it back: store with
    /// the setter armed, wipe everything, recall — the same stops,
    /// coupler and tremulant return.
    #[test]
    fn generals_store_and_recall() {
        let Some((state, _)) = demo_state("Montre 8'") else {
            return;
        };
        let mut state = state.lock().expect("state");
        if let Control::Organ(console) = &mut state.control {
            console.set_coupler(0, true);
        }
        state.set_tremulant(true);
        state.setter_armed = true;
        state.general(3);
        assert!(!state.setter_armed, "storing disarms the setter");

        if let Control::Organ(console) = &mut state.control {
            console.cancel();
            console.set_coupler(0, false);
        }
        state.set_tremulant(false);
        if let Control::Organ(console) = &state.control {
            assert!(
                console.stop_states().iter().all(|(_, _, _, _, drawn)| !drawn),
                "cancel wiped the registration"
            );
        }

        state.general(3);
        if let Control::Organ(console) = &state.control {
            assert!(
                console
                    .stop_states()
                    .iter()
                    .any(|(_, name, _, _, drawn)| *drawn && *name == "Montre 8'"),
                "the stored stop returns"
            );
            assert!(console.coupler_states()[0].2, "the stored coupler returns");
        }
        assert!(state.trems[0].engaged, "the stored tremulant returns");
    }

    /// Every stop on one manual, with its drawn state — what the
    /// divisional tests read the console back with.
    fn stops_on(state: &State, manual: usize) -> Vec<(String, bool)> {
        let Control::Organ(console) = &state.control else {
            panic!("organ expected");
        };
        console
            .stop_states()
            .iter()
            .filter(|(_, _, _, midx, _)| *midx == manual)
            .map(|(_, name, _, _, drawn)| (name.to_string(), *drawn))
            .collect()
    }

    fn draw(state: &mut State, name: &str, on: bool) {
        let Control::Organ(console) = &mut state.control else {
            panic!("organ expected");
        };
        let id = console
            .stop_states()
            .iter()
            .find(|(_, stop, ..)| *stop == name)
            .map(|(id, ..)| *id)
            .unwrap_or_else(|| panic!("no stop named {name:?}"));
        console.set_drawn(id, on);
    }

    fn coupler_on(state: &State, name: &str) -> bool {
        let Control::Organ(console) = &state.control else {
            panic!("organ expected");
        };
        console
            .coupler_states()
            .iter()
            .find(|(_, coupler, ..)| *coupler == name)
            .map(|(_, _, engaged, _)| *engaged)
            .unwrap_or_else(|| panic!("no coupler named {name:?}"))
    }

    fn set_coupler_by_name(state: &mut State, name: &str, on: bool) {
        let Control::Organ(console) = &mut state.control else {
            panic!("organ expected");
        };
        let index = console
            .coupler_states()
            .iter()
            .find(|(_, coupler, ..)| *coupler == name)
            .map(|(index, ..)| *index)
            .unwrap_or_else(|| panic!("no coupler named {name:?}"));
        console.set_coupler(index, on);
    }

    /// A divisional reaches its own division and nothing else: the
    /// other manuals' stops are exactly where the hand left them.
    #[test]
    fn divisionals_stay_inside_their_division() {
        let Some((state, _)) = demo_state("Montre 8'") else {
            return;
        };
        let mut state = state.lock().expect("state");
        // Montre 8' is on the First Manual; give the Second one a stop
        // that must survive the divisional untouched.
        draw(&mut state, "Hautbois 8'", true);
        state.setter_armed = true;
        state.divisional(FIRST, 1);
        assert!(!state.setter_armed, "storing disarms the setter");

        draw(&mut state, "Montre 8'", false);
        draw(&mut state, "Prestant 4'", true);
        draw(&mut state, "Hautbois 8'", false);

        state.divisional(FIRST, 1);
        let first = stops_on(&state, FIRST);
        assert!(
            first.iter().any(|(name, on)| name == "Montre 8'" && *on),
            "the division's stored stop returns"
        );
        assert!(
            first.iter().any(|(name, on)| name == "Prestant 4'" && !*on),
            "a stop the division did not store is retired"
        );
        assert!(
            stops_on(&state, SECOND)
                .iter()
                .any(|(name, on)| name == "Hautbois 8'" && !*on),
            "another division is not the divisional's business"
        );
    }

    /// GO's `DivisionalsStore*` flags, honoured. The demo ODF says
    /// couplers yes (both kinds), tremulants no — so a First Manual
    /// divisional carries II/I and leaves the Tremblant alone.
    #[test]
    fn divisionals_follow_the_odf_store_flags() {
        let Some((state, _)) = demo_state("Montre 8'") else {
            return;
        };
        let mut state = state.lock().expect("state");
        assert!(
            matches!(&state.control, Control::Organ(console)
                if console.combination_scope().divisional_intermanual_couplers
                    && console.combination_scope().divisional_intramanual_couplers
                    && !console.combination_scope().divisional_tremulants),
            "the demo ODF's own answers reached the console"
        );
        set_coupler_by_name(&mut state, "II/I", true);
        state.set_tremulant(true);
        state.setter_armed = true;
        state.divisional(FIRST, 2);

        set_coupler_by_name(&mut state, "II/I", false);
        state.set_tremulant(false);
        state.divisional(FIRST, 2);
        assert!(
            coupler_on(&state, "II/I"),
            "an intermanual coupler of this division is stored and recalled"
        );
        assert!(
            !state.trems[0].engaged,
            "tremulants are out of scope on this set, so the divisional leaves it"
        );

        // A coupler belonging to another division is never touched.
        set_coupler_by_name(&mut state, "I/P", true);
        state.divisional(FIRST, 2);
        assert!(coupler_on(&state, "I/P"), "the Pedal's coupler is the Pedal's");
    }

    /// With `DivisionalsStoreTremulants` on, the division's own
    /// tremulant comes along — "its own" being the one blowing on the
    /// wind its pipes stand on.
    #[test]
    fn divisionals_carry_tremulants_when_the_console_says_so() {
        let scope = aristide_model::CombinationScope {
            divisional_intermanual_couplers: false,
            divisional_intramanual_couplers: false,
            divisional_tremulants: true,
        };
        let Some((state, _)) = demo_state_scoped("Hautbois 8'", Some(scope)) else {
            return;
        };
        let mut state = state.lock().expect("state");
        // The demo's Tremblant blows on the Récit chest, which is the
        // Second Manual's — group 2 (chest 3, 0-based in the engine).
        state.trems[0].groups = match &state.control {
            Control::Organ(console) => console.manual_wind_groups(SECOND),
            _ => panic!("organ expected"),
        };
        state.set_tremulant(true);
        set_coupler_by_name(&mut state, "16' II", true);
        state.setter_armed = true;
        state.divisional(SECOND, 1);

        state.set_tremulant(false);
        set_coupler_by_name(&mut state, "16' II", false);
        state.divisional(SECOND, 1);
        assert!(state.trems[0].engaged, "the division's tremulant returns");
        assert!(
            !coupler_on(&state, "16' II"),
            "with the coupler flags off, the division's own coupler was never stored"
        );
    }

    /// The stepper: store, insert, walk. Frames are positions, and the
    /// ends are walls.
    #[test]
    fn the_stepper_walks_and_stores_frames() {
        let Some((state, _)) = demo_state("Montre 8'") else {
            return;
        };
        let mut state = state.lock().expect("state");
        state.stepper_store(); // frame 1: Montre 8'
        draw(&mut state, "Montre 8'", false);
        draw(&mut state, "Prestant 4'", true);
        state.stepper_insert(); // frame 2: Prestant 4'
        assert_eq!(state.stepper_frames(), 2);
        assert_eq!(state.stepper_frame, 1);

        state.stepper_prev();
        assert_eq!(state.stepper_frame, 0);
        let drawn = stops_on(&state, FIRST);
        assert!(drawn.iter().any(|(name, on)| name == "Montre 8'" && *on));
        assert!(drawn.iter().any(|(name, on)| name == "Prestant 4'" && !*on));

        state.stepper_next();
        assert_eq!(state.stepper_frame, 1);
        assert!(stops_on(&state, FIRST)
            .iter()
            .any(|(name, on)| name == "Prestant 4'" && *on));
        // The end is a wall: pressing on stays put rather than wrapping
        // round to the beginning mid-piece.
        state.stepper_next();
        assert_eq!(state.stepper_frame, 1, "the sequence stops at its end");
        state.stepper_goto(1);
        assert_eq!(state.stepper_frame, 0);
        state.stepper_goto(99);
        assert_eq!(state.stepper_frame, 1, "past the end clamps, never grows");
    }

    /// The crescendo is an overlay, not a registration: hand ∪ pedal
    /// sounds, and rolling back takes away only what the pedal added.
    #[test]
    fn the_crescendo_adds_over_the_hand_and_takes_back_only_its_own() {
        let Some((state, _)) = demo_state("Montre 8'") else {
            return;
        };
        let mut state = state.lock().expect("state");
        // Stage 1 adds the stop the hand already has plus one it hasn't.
        state.midi_config.organs.entry("test organ".into()).or_default().crescendo =
            [(1u8, vec!["Montre 8'".to_string(), "Prestant 4'".to_string()])]
                .into_iter()
                .collect();

        state.set_crescendo(1);
        let drawn = stops_on(&state, FIRST);
        assert!(drawn.iter().any(|(name, on)| name == "Montre 8'" && *on));
        assert!(
            drawn.iter().any(|(name, on)| name == "Prestant 4'" && *on),
            "the pedal adds a stop the hand hasn't drawn"
        );
        if let Control::Organ(console) = &state.control {
            let prestant = console
                .stop_states()
                .iter()
                .find(|(_, name, ..)| *name == "Prestant 4'")
                .map(|(id, ..)| *id)
                .expect("Prestant exists");
            assert!(!console.is_hand_drawn(prestant), "the knob itself stays in");
        }

        state.set_crescendo(0);
        let drawn = stops_on(&state, FIRST);
        assert!(
            drawn.iter().any(|(name, on)| name == "Montre 8'" && *on),
            "a stop the hand also drew survives the pedal coming back"
        );
        assert!(
            drawn.iter().any(|(name, on)| name == "Prestant 4'" && !*on),
            "what the pedal added, the pedal takes away"
        );
    }

    /// Cancel is a thumb on the jamb: it clears the hand and cannot
    /// move the pedal, so the crescendo keeps what it holds.
    #[test]
    fn cancel_clears_the_hand_but_not_the_crescendo() {
        let Some((state, _)) = demo_state("Montre 8'") else {
            return;
        };
        let mut state = state.lock().expect("state");
        state.midi_config.organs.entry("test organ".into()).or_default().crescendo =
            [(1u8, vec!["Prestant 4'".to_string()])].into_iter().collect();
        state.set_crescendo(1);
        if let Control::Organ(console) = &mut state.control {
            console.cancel();
        }
        let drawn = stops_on(&state, FIRST);
        assert!(drawn.iter().any(|(name, on)| name == "Montre 8'" && !*on));
        assert!(
            drawn.iter().any(|(name, on)| name == "Prestant 4'" && *on),
            "the pedal is still where the foot left it"
        );
        state.set_crescendo(0);
        assert!(stops_on(&state, FIRST).iter().all(|(_, on)| !*on), "now silent");
    }

    /// Set + a crescendo stage's piston stores the *hand*, never the
    /// sounding set — or every store would fold the overlay into
    /// itself and ratchet the stage upwards.
    #[test]
    fn storing_a_crescendo_stage_captures_the_drawknobs() {
        let Some((state, _)) = demo_state("Montre 8'") else {
            return;
        };
        let mut state = state.lock().expect("state");
        state.midi_config.organs.entry("test organ".into()).or_default().crescendo =
            [(1u8, vec!["Prestant 4'".to_string()])].into_iter().collect();
        state.set_crescendo(1);
        state.store_crescendo(2);
        let stored = state.midi_config.organs["test organ"].crescendo[&2].clone();
        assert_eq!(
            stored,
            vec!["Montre 8'".to_string()],
            "only what the hand has drawn, not what the pedal is holding"
        );
    }

    /// A stored name this organ hasn't got is reported and skipped —
    /// never fatal, and never dropped from the file.
    #[test]
    fn a_stored_name_the_organ_lacks_is_skipped() {
        let Some((state, _)) = demo_state("Montre 8'") else {
            return;
        };
        let mut state = state.lock().expect("state");
        state.midi_config.organs.entry("test organ".into()).or_default().generals.insert(
            4,
            config::Registration {
                stops: vec!["Prestant 4'".into(), "Vox Humana of Another Organ 8'".into()],
                couplers: vec!["V/IV".into()],
                tremulants: vec!["Tremolo That Isn't".into()],
            },
        );
        state.general(4);
        assert!(
            stops_on(&state, FIRST)
                .iter()
                .any(|(name, on)| name == "Prestant 4'" && *on),
            "the names that do resolve still land"
        );
        assert_eq!(
            state.midi_config.organs["test organ"].generals[&4].stops.len(),
            2,
            "the unresolved name stays in the file"
        );
    }

    /// The pedal's travel maps onto the stages end to end: heel adds
    /// nothing, toe stands on the last stage.
    #[test]
    fn a_controller_sweeps_the_whole_crescendo() {
        let Some((state, _)) = demo_state("Montre 8'") else {
            return;
        };
        let mut state = state.lock().expect("state");
        let binding = Binding {
            channel: None,
            trigger: control::Trigger::Control(4),
            action: control::Action::Crescendo,
            subject: Subject::None,
        };
        for (value, expected) in [(0u8, 0u8), (64, 16), (127, crate::state::CRESCENDO_STAGES)] {
            state.run(&binding, "Test Keyboard", value);
            assert_eq!(state.crescendo_stage, expected, "cc {value}");
        }
    }

    /// The demo set's manuals, in console order: 0 Pedal, 1 First
    /// Manual, 2 Second Manual. Named here so the combination tests
    /// below read as organ rather than as indices.
    const FIRST: usize = 1;
    const SECOND: usize = 2;

    /// A live state on the demo set, with `stop` drawn so notes make
    /// voices (held keys are only recorded for pipes that speak).
    fn demo_state(stop: &str) -> Option<(Arc<Mutex<State>>, usize)> {
        demo_state_scoped(stop, None)
    }

    /// The same, with the console's divisional reach overridden — the
    /// demo ODF says inter- and intramanual couplers yes, tremulants
    /// no, and the tests want both sides of each answer.
    fn demo_state_scoped(
        stop: &str,
        scope: Option<aristide_model::CombinationScope>,
    ) -> Option<(Arc<Mutex<State>>, usize)> {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../testsets/grandorgue-demo/demo.organ");
        if !path.is_file() {
            eprintln!("skipping: demo set not present");
            return None;
        }
        let mut organ = aristide_formats::grandorgue::load(&path)
            .expect("demo set loads")
            .organ;
        if let Some(scope) = scope {
            organ.combinations = scope;
        }
        let organ = organ;
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
        let loaded = bank::build(&organ, 48000.0, 16, None).expect("bank builds");
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
            trems: vec![TremControl {
                name: "Tremulant".into(),
                wave: false,
                groups: vec![0],
                engaged: false,
                params: Default::default(),
            }],
            setter_armed: false,
            stepper_frame: 0,
            crescendo_stage: 0,
            master_gain: 0.178,
            reverb_wet: None,
            expression_cc: 11,
            composite_path: None,
            setup: Default::default(),
            provenance: Default::default(),
            stop_voicing: Default::default(),
            stop_labels: Default::default(),
            stop_order: Default::default(),
            compass_overrides: Vec::new(),
            pending_load: None,
            loading: None,
            load_error: None,
            load_warnings: Vec::new(),
            layout: Default::default(),
            coupled_keys: true,
            coupler_key_modes: Default::default(),
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

    /// A map's key colours reach the console keyed by the extended
    /// manual key its notes land on — physical key numbering stays
    /// the map's private business. Colours on CC keys never surface
    /// (no note lands there), and other manuals see nothing.
    #[test]
    fn lumatone_colours_land_on_extended_keys() {
        let ltn = "[Board0]\n\
                   Key_0=60\nChan_0=1\nCol_0=FF0000\n\
                   Key_1=62\nChan_1=1\n\
                   Key_2=4\nChan_2=2\nCol_2=64C8DC\n\
                   Key_3=70\nChan_3=1\nKTyp_3=2\nCol_3=00FF00\n";
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
        assert_eq!(
            port.map_colors(1),
            vec![(60, 0xFF0000), (132, 0x64C8DC)],
            "coloured note keys in extended numbering; uncoloured and CC keys absent"
        );
        assert!(port.map_colors(0).is_empty(), "other manuals see nothing");
    }

    /// The whole live path from a saved assignment to key colours:
    /// set_input names an .ltn, resolve_routes loads and attaches it,
    /// and the port then answers map_colors with extended-key colours
    /// — what the snapshot serves the hex field.
    #[test]
    fn resolved_routes_carry_map_colours() {
        let Some((state, manual)) = demo_state("Gamba 8'") else {
            return;
        };
        let dir = std::env::temp_dir().join("aristide-ltn-colours-test");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("colours.ltn");
        std::fs::write(&path, "[Board0]\nKey_0=60\nChan_0=1\nCol_0=FF0000\n").expect("writes");
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
                    bend: None,
                    map: Some(path.to_string_lossy().into_owned()),
                },
            );
        }
        let locked = state.lock().expect("state poisoned");
        assert_eq!(
            locked.midi_ports[0].map_colors(manual),
            vec![(60, 0xFF0000)],
            "the resolved route serves its map's colours"
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

    /// On a microtonal manual the computer keyboard is a hex surface,
    /// not a piano: the four rows read as the manual's own layout in
    /// the slanted, physically-true stagger — S (up-right of Z) is
    /// +upright, A (up-left) steps down, and under Bosanquet W
    /// (straight above Z) duplicates it.
    #[test]
    fn the_computer_keyboard_plays_a_hex_manual_isomorphically() {
        let Some((state, manual)) = demo_state("Gamba 8'") else {
            return;
        };
        {
            let mut locked = state.lock().expect("state poisoned");
            let Control::Organ(console) = &mut locked.control else {
                panic!("an organ is loaded");
            };
            console.force_manual_kind(manual, aristide_model::ManualKind::Microtonal);
        }
        assert!(bind_computer(&state, manual));
        let layout = {
            let locked = state.lock().expect("state poisoned");
            let Control::Organ(console) = &locked.control else {
                panic!("an organ is loaded");
            };
            console.manual_hex(manual).expect("a microtonal manual has a layout")
        };
        let anchor = layout.anchor;
        assert_eq!((layout.right, layout.upright), (2, 1), "the derived Bosanquet default");

        state.lock().expect("state").key("KeyZ", true);
        assert_eq!(held_on(&state, manual), vec![anchor], "Z is the board's bottom left");
        state.lock().expect("state").key("KeyS", true);
        assert_eq!(
            held_on(&state, manual),
            vec![anchor, anchor + 1],
            "S, physically up-right of Z, is one up-right step"
        );
        state.lock().expect("state").key("KeyA", true);
        assert_eq!(
            held_on(&state, manual),
            vec![anchor, anchor + 1],
            "A, up-left of Z, would step below the compass — silent, not wrapped"
        );
        for code in ["KeyZ", "KeyA", "KeyS"] {
            state.lock().expect("state").key(code, false);
        }
        assert!(held_on(&state, manual).is_empty(), "and they all release");

        state.lock().expect("state").key("KeyW", true);
        assert_eq!(
            held_on(&state, manual),
            vec![anchor],
            "W, straight above Z, duplicates it — up-left plus up-right is zero"
        );
        state.lock().expect("state").key("KeyW", false);

        // The letter rows' piano mapping stays what it is on a hand
        // keyboard: nothing here changed the other manuals' vocabulary.
        let hand = manual - 1;
        assert!(bind_computer(&state, hand));
        state.lock().expect("state").key("Comma", true);
        assert!(
            held_on(&state, hand).contains(&60),
            "comma is still middle C on the hand keyboard"
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
            assert!(
                state.trems.iter().any(|t| t.engaged),
                "and the tremulant, both from one press"
            );
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
