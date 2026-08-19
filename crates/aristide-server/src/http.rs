//! A deliberately small local web console: draw/retire stops, toggle
//! the tremulant, set master gain. Serves one embedded page plus a
//! JSON state endpoint on localhost.
//!
//! This is a stopgap until the real IPC control plane + native GUI
//! (M5); it exists so registration changes don't need a restart. It
//! runs on its own thread and talks to the engine exactly like MIDI
//! does: lock the shared state, send commands.

use std::sync::{Arc, Mutex};

use aristide_engine::Command;
use tiny_http::{Header, Method, Response, Server};

use crate::{Control, State};

const PAGE: &str = include_str!("console.html");

pub fn spawn(state: Arc<Mutex<State>>, port: u16) -> std::io::Result<()> {
    let server = Server::http(("127.0.0.1", port))
        .map_err(|e| std::io::Error::other(format!("http bind: {e}")))?;
    tracing::info!("console ui: http://127.0.0.1:{port}/");
    std::thread::Builder::new()
        .name("aristide-http".into())
        .spawn(move || {
            for request in server.incoming_requests() {
                // Log every non-poll request so phantom traffic (e.g. a
                // client sending note-ons nobody asked for) shows up in
                // the server log with a timestamp. /api/state is the UI's
                // steady poll and would drown everything else out.
                if request.url() != "/api/state" {
                    tracing::info!("http {} {}", request.method(), request.url());
                }
                // Consoles are web pages (Tauri webview, plain browser);
                // they fetch this API cross-origin, so every response
                // carries the permissive CORS header. The bind stays
                // localhost-only, which is the actual access control.
                let response = respond(&state, request.method(), request.url())
                    .with_header(
                        Header::from_bytes("Access-Control-Allow-Origin", "*")
                            .expect("valid header"),
                    );
                let _ = request.respond(response);
            }
        })?;
    Ok(())
}

fn respond(
    state: &Mutex<State>,
    method: &Method,
    url: &str,
) -> Response<std::io::Cursor<Vec<u8>>> {
    let (path, query) = url.split_once('?').unwrap_or((url, ""));
    match (method, path) {
        (Method::Get, "/") => html(PAGE),
        (Method::Get, "/api/state") => json(state_json(state)),
        (Method::Post, "/api/stop") => {
            let id = param(query, "id").and_then(|v| v.parse::<u32>().ok());
            let on = param(query, "on") == Some("1");
            match id {
                Some(id) => {
                    apply_stop(state, id, on);
                    json(state_json(state))
                }
                None => bad_request("missing id"),
            }
        }
        (Method::Post, "/api/cancel") => {
            {
                let mut state = state.lock().expect("state poisoned");
                let State {
                    engine, control, ..
                } = &mut *state;
                if let Control::Organ(console) = control {
                    for handle in console.cancel() {
                        engine.send(Command::StopVoice { handle });
                    }
                }
            }
            json(state_json(state))
        }
        (Method::Post, "/api/coupler") => {
            let index = param(query, "idx").and_then(|v| v.parse::<usize>().ok());
            let on = param(query, "on") == Some("1");
            match index {
                Some(index) => {
                    {
                        let mut state = state.lock().expect("state poisoned");
                        let State {
                            engine, control, ..
                        } = &mut *state;
                        if let Control::Organ(console) = control {
                            let (stopped, starts) = console.set_coupler(index, on);
                            for handle in stopped {
                                engine.send(Command::StopVoice { handle });
                            }
                            for start in starts {
                                send_start(engine, Some(start));
                            }
                        }
                    }
                    json(state_json(state))
                }
                None => bad_request("missing idx"),
            }
        }
        (Method::Post, "/api/noises") => {
            {
                let mut state = state.lock().expect("state poisoned");
                let State {
                    engine, control, ..
                } = &mut *state;
                if let Control::Organ(console) = control {
                    let (mut enabled, mut volume) = console.noises();
                    if let Some(on) = param(query, "on") {
                        enabled = on == "1";
                    }
                    if let Some(v) = param(query, "vol").and_then(|v| v.parse::<f32>().ok()) {
                        volume = v;
                    }
                    for handle in console.set_noises(enabled, volume) {
                        engine.send(Command::KillVoice { handle });
                    }
                }
            }
            json(state_json(state))
        }
        (Method::Post, "/api/reverb") => {
            match param(query, "wet").and_then(|v| v.parse::<f32>().ok()) {
                Some(wet) if (0.0..=2.0).contains(&wet) => {
                    {
                        let mut state = state.lock().expect("state poisoned");
                        if state.reverb_wet.is_some() {
                            state.reverb_wet = Some(wet);
                            state.engine.send(Command::SetReverbWet { wet });
                        }
                    }
                    json(state_json(state))
                }
                _ => bad_request("bad wet"),
            }
        }
        (Method::Post, "/api/tuning") => {
            {
                let mut state = state.lock().expect("state poisoned");
                // With `manual`, the update tunes that one division
                // apart from the instrument (starting from what it
                // effectively plays now); `reset=1` returns it to the
                // shared tuning. Without, it tunes the instrument.
                let manual = param(query, "manual").and_then(|v| v.parse::<usize>().ok());
                let patched = |mut tuning: crate::tuning::Tuning| {
                    if let Some(t) =
                        param(query, "temperament").and_then(crate::tuning::Temperament::parse)
                    {
                        tuning.temperament = t;
                    }
                    if let Some(a4) = param(query, "a4").and_then(|v| v.parse::<f64>().ok()) {
                        tuning.a4_hz = a4.clamp(300.0, 500.0);
                    }
                    if let Some(t) = param(query, "transpose").and_then(|v| v.parse::<i8>().ok())
                    {
                        tuning.transpose = t.clamp(-12, 12);
                    }
                    tuning
                };
                match manual {
                    Some(manual) => {
                        let reset = param(query, "reset") == Some("1");
                        let current = match &state.control {
                            Control::Organ(console) => Some(
                                console.manual_tuning(manual).unwrap_or(console.tuning()),
                            ),
                            Control::Tone => None,
                        };
                        if let Some(current) = current {
                            let tuning = (!reset).then(|| patched(current));
                            state.tune_manual(manual, tuning);
                        }
                    }
                    None => {
                        if let Control::Organ(console) = &mut state.control {
                            let tuning = patched(console.tuning());
                            console.set_tuning(tuning);
                        }
                    }
                }
            }
            json(state_json(state))
        }
        // Move a stop to another manual — the ranges re-anchor by
        // pitch, and the change lands in the organ's file when it has
        // one.
        (Method::Post, "/api/organ/move") => {
            let mut state = state.lock().expect("state poisoned");
            match (
                param(query, "stop").and_then(|v| v.parse::<u32>().ok()),
                param(query, "manual").and_then(|v| v.parse::<usize>().ok()),
            ) {
                (Some(stop), Some(manual)) => {
                    if state.move_stop(aristide_model::StopId(stop), manual) {
                        json(state_json_locked(&state))
                    } else {
                        bad_request("no such stop or manual")
                    }
                }
                _ => bad_request("missing stop/manual"),
            }
        }
        // Keep a coupler on the console (`keep=1`) or take it off
        // (`keep=0`). Off is hidden and disengaged, not deleted.
        (Method::Post, "/api/organ/coupler") => {
            let mut state = state.lock().expect("state poisoned");
            match (
                param(query, "idx").and_then(|v| v.parse::<usize>().ok()),
                param(query, "keep").map(|v| v != "0"),
            ) {
                (Some(index), Some(keep)) => {
                    if state.set_coupler_pick(index, keep) {
                        json(state_json_locked(&state))
                    } else {
                        bad_request("no such coupler")
                    }
                }
                _ => bad_request("missing idx/keep"),
            }
        }
        // Declare a manual's compass (both low and high, MIDI notes),
        // or with neither given go back to the set's own. Live at
        // once, and saved into the organ's file when it has one.
        (Method::Post, "/api/organ/compass") => {
            let mut state = state.lock().expect("state poisoned");
            let Some(manual) = param(query, "manual").and_then(|v| v.parse::<usize>().ok())
            else {
                return bad_request("missing manual");
            };
            let compass = match (
                param(query, "low").map(|v| v.parse::<u8>()),
                param(query, "high").map(|v| v.parse::<u8>()),
            ) {
                (Some(Ok(low)), Some(Ok(high))) if low <= high && high < 128 => {
                    Some((low, high))
                }
                (None, None) => None,
                _ => return bad_request("low and high must be MIDI notes, low first"),
            };
            if state.set_compass_override(manual, compass) {
                json(state_json_locked(&state))
            } else {
                bad_request("no such manual")
            }
        }
        // Load an instrument: one or more paths to `.organ` sets or
        // composite `.toml` files. The load itself happens on the main
        // thread (it owns the audio stream); this only queues it, and
        // the state snapshots narrate progress until the organ appears.
        (Method::Post, "/api/organ/load") => {
            let paths: Vec<std::path::PathBuf> = params(query, "path")
                .map(|value| std::path::PathBuf::from(unescape(value)))
                .collect();
            if paths.is_empty() {
                return bad_request("missing path");
            }
            for path in &paths {
                if !path.is_file() {
                    return bad_request(&format!("{}: not a file", path.display()));
                }
            }
            let mut state = state.lock().expect("state poisoned");
            if state.loading.is_some() || state.pending_load.is_some() {
                return bad_request("an organ is already loading");
            }
            state.loading = Some("loading…".to_string());
            state.load_error = None;
            state.pending_load = Some(crate::LoadRequest {
                paths,
                stops: Vec::new(),
                initial: false,
            });
            json(state_json_locked(&state))
        }
        // Create a blank composite — nothing but a name — under the
        // config directory's organs/, and queue loading it. The player
        // grows it from there; the file is theirs to edit or move.
        (Method::Post, "/api/organ/new") => {
            let Some(name) = param(query, "name").map(unescape) else {
                return bad_request("missing name");
            };
            let mut state = state.lock().expect("state poisoned");
            if state.loading.is_some() || state.pending_load.is_some() {
                return bad_request("an organ is already loading");
            }
            let Some(dir) = crate::config::organs_dir() else {
                return bad_request("no config directory to keep organs in");
            };
            match crate::config::create_blank_organ(&dir, &name) {
                Ok(path) => {
                    state.loading = Some("loading…".to_string());
                    state.load_error = None;
                    state.pending_load = Some(crate::LoadRequest {
                        paths: vec![path],
                        stops: Vec::new(),
                        initial: false,
                    });
                    json(state_json_locked(&state))
                }
                Err(err) => bad_request(&err),
            }
        }
        // Rename the loaded organ in place: the name changes in the
        // file that owns it, and everything keyed by it (assignments,
        // the library's label) follows; no path changes, so nothing
        // that refers to the organ's file breaks.
        (Method::Post, "/api/organ/rename") => match param(query, "name").map(unescape) {
            Some(name) => {
                let mut state = state.lock().expect("state poisoned");
                match state.rename_organ(&name) {
                    Ok(()) => json(state_json_locked(&state)),
                    Err(err) => bad_request(&err),
                }
            }
            None => bad_request("missing name"),
        },
        // Take an organ off the picker. Its assignments are kept.
        (Method::Post, "/api/library/forget") => match param(query, "path").map(unescape) {
            Some(path) => {
                let mut state = state.lock().expect("state poisoned");
                state.forget_organ(std::path::Path::new(&path));
                json(state_json_locked(&state))
            }
            None => bad_request("missing path"),
        },
        // The picker's file browser: subdirectories and loadable organ
        // files under `dir` (the home directory when absent). The bind
        // is localhost-only, which is the access control here as for
        // every other endpoint.
        (Method::Get, "/api/browse") => {
            let dir = param(query, "dir")
                .map(unescape)
                .filter(|dir| !dir.is_empty())
                .map(std::path::PathBuf::from)
                .or_else(|| std::env::var_os("HOME").map(std::path::PathBuf::from))
                .unwrap_or_else(|| std::path::PathBuf::from("/"));
            match browse_json(&dir) {
                Ok(body) => json(body),
                Err(err) => bad_request(&err),
            }
        }
        // Write the loaded combination to a composite organ file —
        // from then on that file is the organ, and it owns the wiring.
        (Method::Post, "/api/organ/save") => {
            let mut state = state.lock().expect("state poisoned");
            match param(query, "path").map(unescape) {
                Some(path) if path.ends_with(".toml") => {
                    match state.save_composite(std::path::PathBuf::from(path)) {
                        Ok(()) => json(state_json_locked(&state)),
                        Err(err) => bad_request(&err),
                    }
                }
                Some(_) => bad_request("path must end in .toml"),
                None => bad_request("missing path"),
            }
        }
        // Input routing, addressed the way the player thinks about it:
        // *this manual* listens to *that device*. `slot` numbers a
        // manual's inputs (a manual may have several); a slot past the
        // end adds one. `ch` is 1-16, or absent for any channel.
        (Method::Post, "/api/midi/bind") => {
            let manual = param(query, "manual").and_then(|v| v.parse::<usize>().ok());
            let slot = param(query, "slot").and_then(|v| v.parse::<usize>().ok());
            // Devices travel by name, not by port number: a manual may
            // be pointed at a keyboard that is currently unplugged, and
            // port numbers shift under a rescan anyway.
            let device = param(query, "device").map(unescape);
            match (manual, slot, device) {
                (Some(manual), Some(slot), Some(device)) if !device.is_empty() => {
                    let mut state = state.lock().expect("state poisoned");
                    // No channel given means "whatever the set suggests
                    // for this manual, else every channel" — the sidecar
                    // knows how the real console was wired.
                    let channel = match param(query, "ch") {
                        Some("any") => None,
                        Some(value) => value.parse::<u8>().ok().filter(|c| (1..=16).contains(c)),
                        None => state.suggested_channels.get(manual).copied().flatten(),
                    };
                    // The keyboard's own compass. "set" means the sample
                    // set's, i.e. forget what was learned.
                    let key = |name| match param(query, name) {
                        Some("set") => None,
                        Some(value) => value.parse::<u8>().ok().filter(|k| *k < 128),
                        None => state
                            .manual_inputs(manual)
                            .get(slot)
                            .and_then(|input| if name == "low" { input.low } else { input.high }),
                    };
                    let (low, high) = (key("low"), key("high"));
                    // The keyboard's shift in semitones: a controller
                    // whose keys should sound below (or above) what
                    // they send. Same ±36 bound as the octave actions;
                    // absent, rebinding keeps whatever the octave
                    // buttons have done to it.
                    let transpose = match param(query, "transpose") {
                        Some(value) => match value.parse::<i8>() {
                            Ok(semitones) => semitones.clamp(-36, 36),
                            Err(_) => return bad_request("transpose must be semitones"),
                        },
                        None => state
                            .manual_inputs(manual)
                            .get(slot)
                            .map_or(0, |input| input.transpose),
                    };
                    state.learn = None;
                    if !state.set_input(
                        manual,
                        slot,
                        crate::config::Input {
                            device,
                            channel,
                            low,
                            high,
                            transpose,
                        },
                    ) {
                        return bad_request("no such manual");
                    }
                    json(state_json_locked(&state))
                }
                _ => bad_request("missing manual/slot/device"),
            }
        }
        (Method::Post, "/api/midi/unbind") => {
            let manual = param(query, "manual").and_then(|v| v.parse::<usize>().ok());
            let slot = param(query, "slot").and_then(|v| v.parse::<usize>().ok());
            match (manual, slot) {
                (Some(manual), Some(slot)) => {
                    let mut state = state.lock().expect("state poisoned");
                    state.learn = None;
                    if !state.remove_input(manual, slot) {
                        return bad_request("no such manual");
                    }
                    json(state_json_locked(&state))
                }
                _ => bad_request("missing manual/slot"),
            }
        }
        // Auto-detect: wait for a key press and bind whatever port and
        // channel it arrives on. Assigning by hand is still there for a
        // keyboard that isn't plugged in yet.
        (Method::Post, "/api/midi/learn") => {
            let mut state = state.lock().expect("state poisoned");
            let manual = param(query, "manual").and_then(|v| v.parse::<usize>().ok());
            let slot = param(query, "slot").and_then(|v| v.parse::<usize>().ok());
            match (manual, slot) {
                (Some(manual), Some(slot)) => {
                    let manuals = match &state.control {
                        Control::Organ(console) => console.manual_states().len(),
                        Control::Tone => 0,
                    };
                    if manual >= manuals {
                        return bad_request("no such manual");
                    }
                    tracing::info!("midi: listening for a key to assign to manual {manual}");
                    state.listen(manual, slot);
                }
                // No target = stop listening.
                _ => state.learn = None,
            }
            json(state_json_locked(&state))
        }
        // A computer key, treated as any other input: a binding first,
        // then a note on whatever manual the keyboard is assigned to.
        (Method::Post, "/api/key") => {
            let on = param(query, "on") == Some("1");
            match param(query, "code").map(unescape) {
                Some(code) if !code.is_empty() => {
                    let mut state = state.lock().expect("state poisoned");
                    // Teaching a binding: a key press is a control like
                    // any piston, and must not also play.
                    if state.control_learning().is_some() {
                        if on {
                            state.learn_control(
                                crate::COMPUTER_KEYBOARD,
                                None,
                                crate::control::Trigger::Key(code),
                            );
                        }
                        return json(state_json_locked(&state));
                    }
                    state.key(&code, on);
                    json(state_json_locked(&state))
                }
                _ => bad_request("missing code"),
            }
        }
        // Run an action outright — the same verbs a binding uses, so a
        // menu item and a piston cannot drift apart.
        (Method::Post, "/api/action") => match param(query, "do").map(unescape) {
            Some(text) => match crate::control::Action::parse(&text) {
                Some(action) => {
                    let device = param(query, "device").map(unescape).unwrap_or_default();
                    let mut state = state.lock().expect("state poisoned");
                    state.run_named(&action, &device);
                    json(state_json_locked(&state))
                }
                None => bad_request("no such action"),
            },
            None => bad_request("missing action"),
        },
        // Bindings: what an input does when it isn't playing a note.
        (Method::Post, "/api/control/bind") => {
            let slot = param(query, "slot").and_then(|v| v.parse::<usize>().ok());
            let action = param(query, "action").map(unescape);
            match (slot, action) {
                (Some(slot), Some(action)) if crate::control::Action::parse(&action).is_some() => {
                    let mut state = state.lock().expect("state poisoned");
                    let saved = state.controls().get(slot).cloned();
                    // Only what the request names is changed: setting an
                    // action leaves the trigger it was taught alone.
                    let control = crate::config::Control {
                        device: param(query, "device")
                            .map(unescape)
                            .or_else(|| saved.as_ref().map(|c| c.device.clone()))
                            .unwrap_or_default(),
                        channel: match param(query, "ch") {
                            Some("any") => None,
                            Some(value) => value.parse().ok().filter(|c| (1..=16).contains(c)),
                            None => saved.as_ref().and_then(|c| c.channel),
                        },
                        trigger: param(query, "trigger")
                            .map(unescape)
                            .or_else(|| saved.as_ref().map(|c| c.trigger.clone()))
                            .unwrap_or_default(),
                        action,
                        manual: match param(query, "manual") {
                            Some("any") => None,
                            Some(value) => Some(unescape(value)),
                            None => saved.and_then(|c| c.manual),
                        },
                    };
                    state.control_learn = None;
                    state.set_control(slot, control);
                    json(state_json_locked(&state))
                }
                (Some(_), Some(_)) => bad_request("no such action"),
                _ => bad_request("missing slot/action"),
            }
        }
        (Method::Post, "/api/control/unbind") => {
            match param(query, "slot").and_then(|v| v.parse::<usize>().ok()) {
                Some(slot) => {
                    let mut state = state.lock().expect("state poisoned");
                    state.control_learn = None;
                    state.remove_control(slot);
                    json(state_json_locked(&state))
                }
                None => bad_request("missing slot"),
            }
        }
        // Auto-detect for controls: press the piston, pedal or key.
        (Method::Post, "/api/control/learn") => {
            let mut state = state.lock().expect("state poisoned");
            match param(query, "slot").and_then(|v| v.parse::<usize>().ok()) {
                Some(slot) => {
                    tracing::info!("control: listening for the control of binding {slot}");
                    state.listen_control(slot);
                }
                None => state.control_learn = None,
            }
            json(state_json_locked(&state))
        }
        // Whether couplers may repitch to reach pipes a division hasn't
        // got. Off is the default and the musically honest answer; a
        // piece that wants the other can turn it on without editing the
        // set's sidecar.
        (Method::Post, "/api/couplers") => {
            let on = param(query, "repitch") == Some("1");
            {
                let mut state = state.lock().expect("state poisoned");
                if let Control::Organ(console) = &mut state.control {
                    console.set_coupler_repitch(on);
                    tracing::info!("couplers: repitch {}", if on { "on" } else { "off" });
                }
            }
            json(state_json(state))
        }
        (Method::Post, "/api/midi/rescan") => {
            crate::request_midi_rescan();
            json(state_json(state))
        }
        (Method::Post, "/api/note") => {
            let manual = param(query, "manual").and_then(|v| v.parse::<usize>().ok());
            let key = param(query, "key").and_then(|v| v.parse::<u8>().ok());
            let on = param(query, "on") == Some("1");
            match (manual, key) {
                (Some(manual), Some(key)) if key < 128 => {
                    apply_note(state, manual, key, on);
                    json(state_json(state))
                }
                _ => bad_request("missing manual/key"),
            }
        }
        (Method::Post, "/api/panic") => {
            let mut state = state.lock().expect("state poisoned");
            let State {
                engine, control, ..
            } = &mut *state;
            if let Control::Organ(console) = control {
                console.all_off();
            }
            engine.send(Command::AllNotesOff);
            json(state_json_locked(&state))
        }
        (Method::Post, "/api/trem") => {
            let on = param(query, "on") == Some("1");
            apply_trem(state, on);
            json(state_json(state))
        }
        (Method::Post, "/api/enclosure") => {
            let index = param(query, "idx").and_then(|v| v.parse::<usize>().ok());
            let value = param(query, "v").and_then(|v| v.parse::<f32>().ok());
            match (index, value) {
                (Some(index), Some(value)) if (0.0..=1.0).contains(&value) => {
                    {
                        let mut state = state.lock().expect("state poisoned");
                        let State {
                            engine, control, ..
                        } = &mut *state;
                        if let Control::Organ(console) = control {
                            if let Some((enclosure, position)) =
                                console.set_enclosure(index, value)
                            {
                                engine.send(Command::SetEnclosurePosition {
                                    enclosure,
                                    position,
                                });
                            }
                        }
                    }
                    json(state_json(state))
                }
                _ => bad_request("bad enclosure move"),
            }
        }
        (Method::Post, "/api/gain") => match param(query, "v").and_then(|v| v.parse::<f32>().ok())
        {
            Some(v) if (0.0..=2.0).contains(&v) => {
                let mut state = state.lock().expect("state poisoned");
                state.master_gain = v;
                state.engine.send(Command::SetMasterGain { linear: v });
                json(state_json_locked(&state))
            }
            _ => bad_request("bad gain"),
        },
        _ => Response::from_string("not found").with_status_code(404),
    }
}

/// A key press/release from the UI — same path as a MIDI note, but
/// addressed by manual index rather than channel.
fn apply_note(state: &Mutex<State>, manual: usize, key: u8, on: bool) {
    let mut state = state.lock().expect("state poisoned");
    let State {
        engine, control, ..
    } = &mut *state;
    if let Control::Organ(console) = control {
        if on {
            let (starts, retriggered) = console.note_on_manual(manual, key);
            for handle in retriggered {
                engine.send(Command::StopVoice { handle });
            }
            for start in starts {
                send_start(engine, Some(start));
            }
        } else {
            for handle in console.note_off_manual(manual, key) {
                engine.send(Command::StopVoice { handle });
            }
        }
    }
}

fn apply_stop(state: &Mutex<State>, id: u32, on: bool) {
    let mut state = state.lock().expect("state poisoned");
    let State {
        engine, control, ..
    } = &mut *state;
    if let Control::Organ(console) = control {
        let (stopped, starts) = console.set_drawn(aristide_model::StopId(id), on);
        for handle in stopped {
            engine.send(Command::StopVoice { handle });
        }
        for start in starts {
            send_start(engine, Some(start));
        }
    }
}

/// Start a control-noise one-shot (drawstop thump, coupler clack).
fn send_start(engine: &mut aristide_engine::EngineHandle, noise: Option<crate::console::VoiceStart>) {
    if let Some(start) = noise {
        engine.send(Command::StartVoice {
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

fn apply_trem(state: &Mutex<State>, on: bool) {
    let mut state = state.lock().expect("state poisoned");
    let changed = state.trem_engaged != on;
    state.trem_engaged = on;
    let groups = state.trem_groups.clone();
    for group in groups {
        state.engine.send(Command::SetTremulant { group, engaged: on });
    }
    if changed {
        let State {
            engine, control, ..
        } = &mut *state;
        if let Control::Organ(console) = control {
            let (start, stop) = console.tremulant_toggle_noise(on);
            send_start(engine, start);
            if let Some(handle) = stop {
                engine.send(Command::StopVoice { handle });
            }
        }
    }
}

fn state_json(state: &Mutex<State>) -> String {
    state_json_locked(&state.lock().expect("state poisoned"))
}

fn state_json_locked(state: &State) -> String {
    let mut out = String::from("{\"stops\":[");
    if let Control::Organ(console) = &state.control {
        let mut first = true;
        for (id, name, manual, manual_index, drawn) in console.stop_states() {
            if !first {
                out.push(',');
            }
            first = false;
            out.push_str(&format!(
                "{{\"id\":{},\"name\":{},\"manual\":{},\"midx\":{},\"on\":{}}}",
                id.0,
                json_string(name),
                json_string(manual),
                // usize::MAX marks a stop on a manual the set hasn't
                // got — loaders prevent it, but JSON must stay finite.
                manual_index.min(u32::MAX as usize),
                drawn
            ));
        }
    }
    out.push_str("],\"couplers\":[");
    if let Control::Organ(console) = &state.control {
        let mut first = true;
        for (index, name, engaged, available) in console.coupler_states() {
            if !first {
                out.push(',');
            }
            first = false;
            out.push_str(&format!(
                "{{\"idx\":{index},\"name\":{},\"on\":{engaged}{}}}",
                json_string(name),
                // Present only when off the console, so the common
                // snapshot stays small and old clients stay right.
                if available { "" } else { ",\"hidden\":true" }
            ));
        }
    }
    out.push_str("],\"manuals\":[");
    if let Control::Organ(console) = &state.control {
        let mut first = true;
        for (idx, name, first_key, key_count, held) in console.manual_states() {
            if !first {
                out.push(',');
            }
            first = false;
            let held: Vec<String> = held.iter().map(|k| k.to_string()).collect();
            out.push_str(&format!(
                "{{\"idx\":{idx},\"name\":{},\"first_key\":{first_key},\"key_count\":{key_count},\"held\":[{}]}}",
                json_string(name),
                held.join(",")
            ));
        }
    }
    out.push_str(&format!(
        "],\"tremulant\":{},\"gain\":{}",
        state.trem_engaged, state.master_gain
    ));
    if let Control::Organ(console) = &state.control {
        out.push_str(&format!(",\"organ\":{}", json_string(console.organ_name())));
    }
    // The picker's world: what could be loaded, what is loading now,
    // and why the last attempt failed.
    if let Some(phase) = &state.loading {
        out.push_str(&format!(",\"loading\":{}", json_string(phase)));
    }
    if let Some(error) = &state.load_error {
        out.push_str(&format!(",\"load_error\":{}", json_string(error)));
    }
    out.push_str(",\"library\":[");
    for (index, entry) in state.midi_config.library.iter().enumerate() {
        if index > 0 {
            out.push(',');
        }
        out.push_str(&format!(
            "{{\"name\":{},\"path\":{}}}",
            json_string(&entry.name),
            json_string(&entry.path.display().to_string())
        ));
    }
    out.push(']');
    if let Control::Organ(console) = &state.control {
        let tuning = console.tuning();
        out.push_str(&format!(
            ",\"tuning\":{{\"temperament\":{},\"a4\":{},\"transpose\":{}}}",
            json_string(tuning.temperament.name()),
            tuning.a4_hz,
            tuning.transpose
        ));
        // Divisions tuned apart from the instrument, by manual index —
        // absent manuals follow the shared tuning above.
        let own: Vec<String> = (0..console.manual_states().len())
            .filter_map(|manual| {
                console.manual_tuning(manual).map(|tuning| {
                    format!(
                        "{{\"idx\":{manual},\"temperament\":{},\"a4\":{},\"transpose\":{}}}",
                        json_string(tuning.temperament.name()),
                        tuning.a4_hz,
                        tuning.transpose
                    )
                })
            })
            .collect();
        if !own.is_empty() {
            out.push_str(&format!(",\"manual_tuning\":[{}]", own.join(",")));
        }
    }
    if let Some(wet) = state.reverb_wet {
        out.push_str(&format!(",\"reverb\":{wet}"));
    }
    // MIDI, as the dialog reads it: the inputs this machine has, and
    // what each of the organ's manuals listens to. Bindings name a
    // device even while it is unplugged, so `connected` says whether
    // this one is actually there.
    out.push_str(",\"midi\":{\"ports\":[");
    for (id, port) in state.midi_ports.iter().enumerate() {
        if id > 0 {
            out.push(',');
        }
        out.push_str(&format!(
            "{{\"id\":{id},\"name\":{}}}",
            json_string(&port.name)
        ));
    }
    // The computer keyboard is assignable like any device, though no
    // operating system will ever list it.
    if !state.midi_ports.is_empty() {
        out.push(',');
    }
    out.push_str(&format!(
        "{{\"id\":{},\"name\":{},\"virtual\":true}}",
        state.midi_ports.len(),
        json_string(crate::COMPUTER_KEYBOARD)
    ));
    out.push_str("],\"manuals\":[");
    if let Control::Organ(console) = &state.control {
        for (position, (idx, name, _, _, _)) in console.manual_states().iter().enumerate() {
            if position > 0 {
                out.push(',');
            }
            out.push_str(&format!(
                "{{\"idx\":{idx},\"name\":{},\"inputs\":[",
                json_string(name)
            ));
            for (slot, input) in state.manual_inputs(*idx).iter().enumerate() {
                if slot > 0 {
                    out.push(',');
                }
                let connected = input.device == crate::COMPUTER_KEYBOARD
                    || state.midi_ports.iter().any(|p| p.name == input.device);
                let number = |value: Option<u8>| {
                    value.map_or_else(|| "null".to_string(), |v| v.to_string())
                };
                out.push_str(&format!(
                    "{{\"slot\":{slot},\"device\":{},\"channel\":{},\"connected\":{connected},\"low\":{},\"high\":{},\"transpose\":{}}}",
                    json_string(&input.device),
                    number(input.channel),
                    number(input.low),
                    number(input.high),
                    input.transpose
                ));
            }
            // What the set itself declares, so the dialog can say how
            // far a widened keyboard is reaching past it.
            let native = console
                .native_compass(*idx)
                .map_or_else(|| "null".to_string(), |(low, high)| format!("[{low},{high}]"));
            out.push_str(&format!("],\"native\":{native}}}"));
        }
    }
    out.push(']');
    if let Some(learn) = &state.learn {
        // Which key the dialog is still waiting for.
        let step = if learn.heard.is_some() { "high" } else { "low" };
        out.push_str(&format!(
            ",\"learning\":{{\"manual\":{},\"slot\":{},\"step\":\"{step}\"}}",
            learn.manual, learn.slot
        ));
    }
    out.push('}');
    // Bindings, the computer keyboard, and the vocabulary a UI can
    // offer — everything a Controls pane needs to draw itself.
    out.push_str(",\"controls\":[");
    for (slot, control) in state.controls().iter().enumerate() {
        if slot > 0 {
            out.push(',');
        }
        let optional = |value: &Option<String>| {
            value
                .as_deref()
                .map_or_else(|| "null".to_string(), json_string)
        };
        out.push_str(&format!(
            "{{\"slot\":{slot},\"device\":{},\"channel\":{},\"trigger\":{},\"action\":{},\"manual\":{}}}",
            json_string(&control.device),
            control
                .channel
                .map_or_else(|| "null".to_string(), |c| c.to_string()),
            json_string(&control.trigger),
            json_string(&control.action),
            optional(&control.manual)
        ));
    }
    out.push(']');
    if let Some(learn) = state.control_learn {
        out.push_str(&format!(",\"control_learning\":{}", learn.slot));
    }
    out.push_str(",\"actions\":[");
    for (index, action) in crate::control::CATALOGUE.iter().enumerate() {
        if index > 0 {
            out.push(',');
        }
        out.push_str(&json_string(action));
    }
    out.push(']');
    if let Some(keyboard) = state.keyboard {
        out.push_str(&format!(
            ",\"keyboard\":{{\"manual\":{},\"transpose\":{},\"low\":{},\"high\":{}}}",
            keyboard.manual, keyboard.transpose, keyboard.compass.0, keyboard.compass.1
        ));
    }
    if let Control::Organ(console) = &state.control {
        out.push_str(&format!(
            ",\"coupler_repitch\":{}",
            console.coupler_repitch()
        ));
        let (enabled, volume) = console.noises();
        out.push_str(&format!(
            ",\"noises\":{{\"on\":{enabled},\"vol\":{volume}}}"
        ));
        out.push_str(",\"enclosures\":[");
        let mut first = true;
        for (index, name, position, displayed) in console.enclosure_states() {
            if !first {
                out.push(',');
            }
            first = false;
            out.push_str(&format!(
                "{{\"idx\":{index},\"name\":{},\"value\":{position},\"displayed\":{displayed}}}",
                json_string(&name)
            ));
        }
        out.push(']');
    }
    // How this instrument was put together: the setup dialog opens on
    // `implicit` (combined on the CLI, nothing on disk yet), the Organ
    // preferences edit compasses, and saving writes it all to a file.
    if !state.setup.sources.is_empty() {
        out.push_str(&format!(
            ",\"setup\":{{\"implicit\":{},\"file\":{},\"sources\":[",
            state.setup.implicit,
            state
                .composite_path
                .as_ref()
                .map_or_else(|| "null".to_string(), |p| json_string(&p.display().to_string()))
        ));
        for (index, (label, path)) in state.setup.sources.iter().enumerate() {
            if index > 0 {
                out.push(',');
            }
            out.push_str(&format!(
                "{{\"name\":{},\"path\":{}}}",
                json_string(label),
                json_string(&path.display().to_string())
            ));
        }
        out.push_str("],\"compass\":[");
        for (manual, own) in state.native_compass().iter().enumerate() {
            if manual > 0 {
                out.push(',');
            }
            let (low, high) = state
                .compass_overrides
                .get(manual)
                .copied()
                .flatten()
                .unwrap_or(*own);
            out.push_str(&format!(
                "{{\"idx\":{manual},\"low\":{low},\"high\":{high},\"native_low\":{},\"native_high\":{},\"declared\":{}}}",
                own.0,
                own.1,
                state.compass_overrides.get(manual).copied().flatten().is_some()
            ));
        }
        out.push_str("]}");
    }
    out.push('}');
    out
}

fn json_string(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for c in value.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Percent-decoding, for the one parameter that carries free text: a
/// MIDI port name ("Midiplus AKM320 MIDI 1"). Malformed escapes are
/// left as they are rather than dropped — a name that round-trips
/// wrongly is easier to diagnose than one that silently loses a byte.
fn unescape(value: &str) -> String {
    let mut out = Vec::with_capacity(value.len());
    let bytes = value.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'+' => out.push(b' '),
            b'%' if index + 2 < bytes.len() => {
                match u8::from_str_radix(&value[index + 1..index + 3], 16) {
                    Ok(byte) => {
                        out.push(byte);
                        index += 2;
                    }
                    Err(_) => out.push(b'%'),
                }
            }
            byte => out.push(byte),
        }
        index += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn param<'a>(query: &'a str, key: &str) -> Option<&'a str> {
    query
        .split('&')
        .filter_map(|pair| pair.split_once('='))
        .find(|(k, _)| *k == key)
        .map(|(_, v)| v)
}

/// Every value of a repeated key, in order — `path=a&path=b` is how a
/// multi-set load travels.
fn params<'a>(query: &'a str, key: &'a str) -> impl Iterator<Item = &'a str> {
    query
        .split('&')
        .filter_map(|pair| pair.split_once('='))
        .filter(move |(k, _)| *k == key)
        .map(|(_, v)| v)
}

/// One directory as the picker's browser shows it: subdirectories and
/// loadable organ files (`.organ` sample sets, `.toml` composites,
/// unencrypted Hauptwerk definitions), dotfiles skipped, directories
/// first.
fn browse_json(dir: &std::path::Path) -> Result<String, String> {
    let dir = dir
        .canonicalize()
        .map_err(|err| format!("{}: {err}", dir.display()))?;
    let entries = std::fs::read_dir(&dir).map_err(|err| format!("{}: {err}", dir.display()))?;
    let mut dirs: Vec<String> = Vec::new();
    let mut files: Vec<String> = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        if entry.path().is_dir() {
            dirs.push(name);
        } else {
            let lower = name.to_lowercase();
            if lower.ends_with(".organ")
                || lower.ends_with(".toml")
                || lower.ends_with(".organ_hauptwerk_xml")
            {
                files.push(name);
            }
        }
    }
    let key = |name: &String| name.to_lowercase();
    dirs.sort_by_key(key);
    files.sort_by_key(key);
    let mut out = format!(
        "{{\"dir\":{},\"parent\":{},\"entries\":[",
        json_string(&dir.display().to_string()),
        dir.parent().map_or_else(
            || "null".to_string(),
            |parent| json_string(&parent.display().to_string())
        )
    );
    let mut first = true;
    for (name, is_dir) in dirs
        .iter()
        .map(|name| (name, true))
        .chain(files.iter().map(|name| (name, false)))
    {
        if !first {
            out.push(',');
        }
        first = false;
        out.push_str(&format!(
            "{{\"name\":{},\"path\":{},\"dir\":{is_dir}}}",
            json_string(name),
            json_string(&dir.join(name).display().to_string())
        ));
    }
    out.push_str("]}");
    Ok(out)
}

fn html(body: &str) -> Response<std::io::Cursor<Vec<u8>>> {
    Response::from_string(body).with_header(
        Header::from_bytes("Content-Type", "text/html; charset=utf-8").expect("valid header"),
    )
}

fn json(body: String) -> Response<std::io::Cursor<Vec<u8>>> {
    Response::from_string(body).with_header(
        Header::from_bytes("Content-Type", "application/json").expect("valid header"),
    )
}

fn bad_request(reason: &str) -> Response<std::io::Cursor<Vec<u8>>> {
    Response::from_string(reason).with_status_code(400)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn demo_state() -> Option<Arc<Mutex<State>>> {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../testsets/grandorgue-demo/demo.organ");
        if !path.is_file() {
            eprintln!("skipping: demo set not present");
            return None;
        }
        let organ = aristide_formats::grandorgue::load(&path)
            .expect("demo set loads")
            .organ;
        let loaded = crate::bank::build(&organ, 48000.0).expect("bank builds");
        let console = crate::console::Console::new(organ, loaded.specs, Vec::new(), 48000.0);
        let (_engine, handle) =
            aristide_engine::Engine::new(48000.0, std::sync::Arc::new(loaded.bank));
        let state = Arc::new(Mutex::new(State {
            engine: handle,
            control: Control::Organ(console),
            midi_ports: Vec::new(),
            midi_config: Default::default(),
            // Tests must never touch the user's real assignments.
            config_path: None,
            organ_key: "test organ".into(),
            suggested_channels: vec![Some(3), Some(1), Some(2)],
            learn: None,
            control_learn: None,
            key_bindings: Vec::new(),
            keyboard: None,
            trem_groups: vec![0, 1],
            trem_engaged: false,
            master_gain: 0.178,
            reverb_wet: Some(0.25),
            expression_cc: 11,
            composite_path: None,
            setup: Default::default(),
            compass_overrides: Vec::new(),
            pending_load: None,
            loading: None,
            load_error: None,
        }));
        // As the server does once before it opens any device: routing,
        // bindings and the computer keyboard all come from this.
        state.lock().expect("state poisoned").resolve_routes();
        Some(state)
    }

    /// The whole setup story over the API: the snapshot describes how
    /// the instrument was put together, a declared compass takes
    /// effect live and survives in the snapshot, and saving writes a
    /// composite file that loads back to the same structure — with the
    /// MIDI wiring inside, since the file owns it from then on.
    #[test]
    fn organ_setup_compass_and_save_round_trip() {
        let Some(state) = demo_state() else { return };
        let demo = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../testsets/grandorgue-demo/demo.organ");
        {
            let mut state = state.lock().expect("state poisoned");
            let names: Vec<String> = state.manual_names();
            state.setup.sources = vec![("Demo".into(), demo.clone())];
            state.setup.pulls = names
                .iter()
                .enumerate()
                .map(|(index, name)| (0, name.clone(), index))
                .collect();
            state.setup.implicit = true;
        }
        let body = state_json(&state);
        assert!(body.contains("\"setup\":{\"implicit\":true,\"file\":null"));
        assert!(body.contains("\"native_low\""));

        // Declare a wider compass on manual 1 and see it live.
        respond(&state, &Method::Post, "/api/organ/compass?manual=1&low=24&high=103");
        let body = state_json(&state);
        assert!(
            body.contains("\"idx\":1,\"low\":24,\"high\":103") && body.contains("\"declared\":true"),
            "declared compass shows: {body}"
        );

        // Save, then load the file back through the instrument layer.
        let path = std::env::temp_dir().join("aristide-organ-save-test.toml");
        let _ = std::fs::remove_file(&path);
        respond(
            &state,
            &Method::Post,
            &format!(
                "/api/organ/save?path={}",
                path.display().to_string().replace('/', "%2F")
            ),
        );
        let body = state_json(&state);
        assert!(body.contains("\"implicit\":false"), "saved: {body}");
        let saved = aristide_formats::instrument::load(&path).expect("saved organ loads");
        // The saved name is the assignments key (== the organ's name
        // in production; the fixture keeps them apart on purpose).
        assert_eq!(saved.organ.name, "test organ");
        let manual = saved.organ.manuals.iter().find(|m| m.first_midi_note == 24);
        assert!(manual.is_some_and(|m| m.key_count == 80), "declared compass survives");
        assert_eq!(
            saved.organ.manuals.len(),
            state_json(&state).matches("\"native_low\"").count()
        );
        assert!(!saved.organ.stops.is_empty());

        // The file now owns the wiring: a binding learned via the API
        // lands in it.
        respond(
            &state,
            &Method::Post,
            "/api/midi/bind?manual=0&slot=0&device=Test%20Keys&ch=4",
        );
        let saved = aristide_formats::instrument::load(&path).expect("still loads");
        assert_eq!(saved.midi.inputs.len(), 1);
        assert_eq!(saved.midi.inputs[0].device, "Test Keys");
        assert_eq!(saved.midi.inputs[0].channel, Some(4));

        // A division tuned apart, a coupler taken off, and a stop
        // moved all show live and land in the file.
        respond(
            &state,
            &Method::Post,
            "/api/tuning?manual=1&temperament=meantone&a4=415",
        );
        respond(&state, &Method::Post, "/api/organ/coupler?idx=0&keep=0");
        let (stop, stop_name, target) = {
            let state = state.lock().expect("state poisoned");
            let Control::Organ(console) = &state.control else {
                unreachable!()
            };
            let (id, name, _, from, _) = console.stop_states()[0];
            (id.0, name.to_string(), if from == 0 { 1 } else { 0 })
        };
        respond(
            &state,
            &Method::Post,
            &format!("/api/organ/move?stop={stop}&manual={target}"),
        );
        let body = state_json(&state);
        assert!(
            body.contains("\"manual_tuning\":[{\"idx\":1,\"temperament\":\"meantone4\",\"a4\":415"),
            "own tuning shows: {body}"
        );
        assert!(body.contains("\"hidden\":true"), "picked-off coupler shows");
        let saved = aristide_formats::instrument::load(&path).expect("reloads");
        assert_eq!(
            saved.manual_tuning,
            [(1, Some("meantone4".into()), Some(415.0), Some(0))]
        );
        assert_eq!(saved.sidecar.couplers.drop.len(), 1);
        // Ids renumber on reload; the stop's name is its identity here.
        let _ = stop;
        let moved = saved
            .organ
            .stops
            .iter()
            .find(|s| s.name == stop_name)
            .expect("moved stop reloads");
        assert_eq!(
            moved.manual, saved.organ.manuals[target].id,
            "the move replays from the file"
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn api_toggles_stops_and_tremulant() {
        let Some(state) = demo_state() else { return };

        let body = state_json(&state);
        assert!(body.contains("Montre 8'"), "state lists stops: {body}");
        assert!(body.contains("\"tremulant\":false"));

        // Draw stop 1, verify, retire it again.
        let object_for_id_1 = |body: &str| -> String {
            let start = body.find("{\"id\":1,").expect("stop 1 present");
            body[start..start + body[start..].find('}').expect("object closes")].to_string()
        };
        respond(&state, &Method::Post, "/api/stop?id=1&on=1");
        assert!(object_for_id_1(&state_json(&state)).contains("\"on\":true"));
        respond(&state, &Method::Post, "/api/stop?id=1&on=0");
        assert!(object_for_id_1(&state_json(&state)).contains("\"on\":false"));

        respond(&state, &Method::Post, "/api/trem?on=1");
        assert!(state_json(&state).contains("\"tremulant\":true"));

        respond(&state, &Method::Post, "/api/gain?v=0.5");
        assert!(state_json(&state).contains("\"gain\":0.5"));
    }

    #[test]
    fn general_cancel_clears_stops_and_couplers() {
        let Some(state) = demo_state() else { return };

        // Registration = the stops and couplers arrays, which come
        // first in the snapshot; "noises" carries its own "on" flag.
        let registration = |body: &str| -> String {
            body[..body.find("\"manuals\"").expect("manuals follow")].to_string()
        };

        respond(&state, &Method::Post, "/api/stop?id=1&on=1");
        respond(&state, &Method::Post, "/api/stop?id=16&on=1");
        respond(&state, &Method::Post, "/api/coupler?idx=0&on=1");
        let drawn = registration(&state_json(&state));
        assert_eq!(drawn.matches("\"on\":true").count(), 3, "two stops + a coupler");

        respond(&state, &Method::Post, "/api/cancel");
        let cancelled = registration(&state_json(&state));
        assert!(
            !cancelled.contains("\"on\":true"),
            "cancel left something drawn: {cancelled}"
        );
    }

    #[test]
    fn ui_notes_play_and_state_reports_held_keys() {
        let Some(state) = demo_state() else { return };

        let body = state_json(&state);
        assert!(body.contains("\"manuals\":["), "manuals listed: {body}");
        assert!(body.contains("\"organ\":"), "organ name present: {body}");
        assert!(body.contains("\"first_key\":"), "keyboard compass present");

        // Draw Montre 8' (id 16, First Manual) and press middle C
        // through the API — the state must show the key held.
        respond(&state, &Method::Post, "/api/stop?id=16&on=1");
        respond(&state, &Method::Post, "/api/note?manual=1&key=60&on=1");
        assert!(
            state_json(&state).contains("\"held\":[60]"),
            "pressed key reported held: {}",
            state_json(&state)
        );
        respond(&state, &Method::Post, "/api/note?manual=1&key=60&on=0");
        assert!(!state_json(&state).contains("\"held\":[60]"));

        // Panic silences everything sounding.
        respond(&state, &Method::Post, "/api/note?manual=1&key=62&on=1");
        assert!(state_json(&state).contains("\"held\":[62]"));
        respond(&state, &Method::Post, "/api/panic");
        assert!(!state_json(&state).contains("\"held\":[62]"));

        respond(&state, &Method::Post, "/api/note?manual=9&key=60&on=1");
        assert!(!state_json(&state).contains("\"held\":[60]"), "bad manual ignored");
    }

    #[test]
    fn midi_assignments_survive_the_api() {
        let Some(state) = demo_state() else { return };

        let body = state_json(&state);
        assert!(
            body.contains(
                "\"midi\":{\"ports\":[{\"id\":0,\"name\":\"Computer keyboard\",\"virtual\":true}]"
            ),
            "no MIDI on a test rig, but the computer keyboard is always \
             assignable: {body}"
        );
        // Demo set: First Manual, Second Manual, Pedal — every one of
        // them listed, every one of them empty.
        assert!(
            body.contains("{\"idx\":1,\"name\":\"First Manual\",\"inputs\":[],\"native\":[36,96]}"),
            "manuals listed with nothing assigned: {body}"
        );
        assert!(!body.contains("\"learning\""), "not listening yet");

        respond(
            &state,
            &Method::Post,
            "/api/midi/bind?manual=2&slot=0&device=Test%20Keyboard&ch=4",
        );
        let body = state_json(&state);
        assert!(
            body.contains(
                "{\"slot\":0,\"device\":\"Test Keyboard\",\"channel\":4,\"connected\":false,\"low\":null,\"high\":null,\"transpose\":0}"
            ),
            "assigned by name, honest that it isn't plugged in, and no \
             compass measured yet: {body}"
        );

        // No channel given falls back to what the set suggests for that
        // manual (Second Manual speaks on channel 2 in the demo sidecar).
        respond(
            &state,
            &Method::Post,
            "/api/midi/bind?manual=2&slot=0&device=Test%20Keyboard",
        );
        assert!(state_json(&state).contains("\"channel\":2"));
        respond(
            &state,
            &Method::Post,
            "/api/midi/bind?manual=2&slot=0&device=Test%20Keyboard&ch=any",
        );
        assert!(state_json(&state).contains("\"channel\":null"));

        // A slot past the end adds an input: two keyboards, one manual.
        respond(
            &state,
            &Method::Post,
            "/api/midi/bind?manual=2&slot=9&device=Second%20Keyboard&ch=2",
        );
        assert!(state_json(&state).contains("\"slot\":1,\"device\":\"Second Keyboard\""));

        // A learned keyboard width rides along on the binding.
        respond(
            &state,
            &Method::Post,
            "/api/midi/bind?manual=2&slot=0&device=Test%20Keyboard&low=31&high=101",
        );
        assert!(state_json(&state).contains("\"low\":31,\"high\":101"));

        // A keyboard that should sound below what it sends: C2–C7 keys
        // playing G1–G6 is transpose −5, set right on the binding.
        respond(
            &state,
            &Method::Post,
            "/api/midi/bind?manual=2&slot=0&device=Test%20Keyboard&transpose=-5",
        );
        let body = state_json(&state);
        assert!(
            body.contains("\"low\":31,\"high\":101,\"transpose\":-5"),
            "the shift is set and the learned compass survives it: {body}"
        );
        // Not sending it keeps it, like low/high; out-of-range clamps
        // to the octave actions' own ±36 bound.
        respond(
            &state,
            &Method::Post,
            "/api/midi/bind?manual=2&slot=0&device=Test%20Keyboard&ch=any",
        );
        assert!(state_json(&state).contains("\"transpose\":-5"));
        respond(
            &state,
            &Method::Post,
            "/api/midi/bind?manual=2&slot=0&device=Test%20Keyboard&transpose=99",
        );
        assert!(state_json(&state).contains("\"transpose\":36"));
        respond(
            &state,
            &Method::Post,
            "/api/midi/bind?manual=2&slot=0&device=Test%20Keyboard&transpose=0",
        );
        respond(
            &state,
            &Method::Post,
            "/api/midi/bind?manual=2&slot=0&device=Test%20Keyboard&low=set&high=set",
        );
        assert!(
            state_json(&state).contains("\"low\":null,\"high\":null"),
            "\"set\" gives the manual back the compass its set declares"
        );

        respond(&state, &Method::Post, "/api/midi/unbind?manual=2&slot=0");
        let body = state_json(&state);
        assert!(
            body.contains("{\"slot\":0,\"device\":\"Second Keyboard\",\"channel\":2"),
            "removing the first input renumbers the rest: {body}"
        );
        respond(&state, &Method::Post, "/api/midi/unbind?manual=2&slot=0");
        assert!(state_json(&state).contains("\"name\":\"Second Manual\",\"inputs\":[],\"native\":"));

        // A manual this organ hasn't got is refused, not clamped.
        respond(
            &state,
            &Method::Post,
            "/api/midi/bind?manual=9&slot=0&device=Test%20Keyboard",
        );
        respond(&state, &Method::Post, "/api/midi/unbind?manual=9&slot=0");

        // Auto-detect is a mode the snapshot reports, so the dialog can
        // show which row is waiting.
        respond(&state, &Method::Post, "/api/midi/learn?manual=1&slot=0");
        assert!(state_json(&state).contains(
            "\"learning\":{\"manual\":1,\"slot\":0,\"step\":\"low\"}"
        ));
        respond(&state, &Method::Post, "/api/midi/learn");
        assert!(!state_json(&state).contains("\"learning\""), "cancelled");
        respond(&state, &Method::Post, "/api/midi/learn?manual=9&slot=0");
        assert!(!state_json(&state).contains("\"learning\""), "no such manual");
    }

    /// Every field a console reads sits where it says it does, and the
    /// whole thing is valid JSON — the snapshot is hand-written, so
    /// nothing else checks that.
    #[test]
    fn the_snapshot_is_well_formed() {
        let Some(state) = demo_state() else { return };
        respond(&state, &Method::Post, "/api/control/bind?slot=0&action=panic");
        let body = state_json(&state);
        let value: serde_json::Value = serde_json::from_str(&body).expect("valid JSON");
        for field in [
            "stops", "couplers", "manuals", "midi", "controls", "actions",
            "tuning", "enclosures", "noises", "coupler_repitch",
        ] {
            assert!(value.get(field).is_some(), "{field} missing: {body}");
        }
        // The keyboard object appears only once the player assigns it —
        // through the same bind call as any MIDI device.
        assert!(
            value.get("keyboard").is_none(),
            "an unassigned computer keyboard is absent, not defaulted: {body}"
        );
        respond(
            &state,
            &Method::Post,
            "/api/midi/bind?manual=1&slot=0&device=Computer%20keyboard",
        );
        let assigned: serde_json::Value =
            serde_json::from_str(&state_json(&state)).expect("valid JSON");
        assert_eq!(assigned["keyboard"]["manual"], 1);
        assert!(value["midi"]["ports"].is_array());
        assert_eq!(value["controls"][0]["action"], "panic");
    }

    #[test]
    fn bindings_survive_the_api() {
        let Some(state) = demo_state() else { return };

        let body = state_json(&state);
        assert!(body.contains("\"controls\":[]"), "nothing bound yet: {body}");
        assert!(
            body.contains("\"actions\":[\"octave-up\""),
            "the vocabulary a UI offers is published: {body}"
        );
        assert!(
            !body.contains("\"keyboard\":{"),
            "the computer keyboard plays nothing until assigned: {body}"
        );

        // Adding a binding, then teaching it what presses it.
        respond(
            &state,
            &Method::Post,
            "/api/control/bind?slot=0&action=octave-up&device=Test%20Keyboard",
        );
        assert!(state_json(&state).contains("\"trigger\":\"\""), "not taught yet");

        respond(&state, &Method::Post, "/api/control/learn?slot=0");
        assert!(state_json(&state).contains("\"control_learning\":0"));
        respond(&state, &Method::Post, "/api/key?code=Equal&on=1");
        let body = state_json(&state);
        assert!(
            body.contains("\"trigger\":\"key:Equal\"")
                && body.contains("\"device\":\"Computer keyboard\""),
            "the key that was pressed became the trigger: {body}"
        );
        assert!(!body.contains("\"control_learning\""), "and the wait ended");

        // Changing the action leaves the taught trigger alone.
        respond(
            &state,
            &Method::Post,
            "/api/control/bind?slot=0&action=stop%3AMontre%208%27",
        );
        let body = state_json(&state);
        assert!(body.contains("\"action\":\"stop:Montre 8'\""), "{body}");
        assert!(body.contains("\"trigger\":\"key:Equal\""), "{body}");

        // And the binding does what it says, from a computer key.
        respond(&state, &Method::Post, "/api/key?code=Equal&on=1");
        assert!(
            state_json(&state).contains("{\"id\":16,\"name\":\"Montre 8'\",\"manual\":\"First Manual\",\"midx\":1,\"on\":true}"),
            "the bound key drew the stop it names"
        );

        respond(&state, &Method::Post, "/api/control/unbind?slot=0");
        assert!(state_json(&state).contains("\"controls\":[]"));

        respond(&state, &Method::Post, "/api/control/bind?slot=0&action=nonsense");
        assert!(state_json(&state).contains("\"controls\":[]"), "refused");
    }

    /// Couplers reaching for pipes a division hasn't got is off, and
    /// switchable without editing the set.
    #[test]
    fn coupler_repitch_is_off_and_switchable() {
        let Some(state) = demo_state() else { return };
        assert!(state_json(&state).contains("\"coupler_repitch\":false"));
        respond(&state, &Method::Post, "/api/couplers?repitch=1");
        assert!(state_json(&state).contains("\"coupler_repitch\":true"));
        respond(&state, &Method::Post, "/api/couplers");
        assert!(state_json(&state).contains("\"coupler_repitch\":false"));
    }

    /// The computer keyboard is assigned through the same bind and
    /// unbind calls as any MIDI device — there is no separate path.
    #[test]
    fn the_computer_keyboard_binds_like_any_device() {
        let Some(state) = demo_state() else { return };
        respond(
            &state,
            &Method::Post,
            "/api/midi/bind?manual=1&slot=0&device=Computer%20keyboard",
        );
        assert!(state_json(&state).contains("\"keyboard\":{\"manual\":1,\"transpose\":0"));

        // The action endpoint shifts it the way a binding would.
        respond(
            &state,
            &Method::Post,
            "/api/action?do=octave-up&device=Computer%20keyboard",
        );
        assert!(
            state_json(&state).contains("\"keyboard\":{\"manual\":1,\"transpose\":12"),
            "the keyboard is shifted, not the division"
        );

        // Picking it in another manual's dropdown moves it: one
        // keyboard, one place, the shift carried along.
        respond(
            &state,
            &Method::Post,
            "/api/midi/bind?manual=2&slot=0&device=Computer%20keyboard",
        );
        let body = state_json(&state);
        assert!(
            body.contains("\"keyboard\":{\"manual\":2,\"transpose\":12"),
            "moving it keeps the shift: {body}"
        );
        assert!(
            body.matches("\"device\":\"Computer keyboard\"").count() == 1,
            "and it plays one manual, not two: {body}"
        );

        // Removing the row detaches it, like unplugging any device.
        respond(&state, &Method::Post, "/api/midi/unbind?manual=2&slot=0");
        let body = state_json(&state);
        assert!(
            !body.contains("\"keyboard\":{"),
            "detached, the keyboard plays nothing: {body}"
        );
    }

    #[test]
    fn port_names_survive_the_query_string() {
        assert_eq!(unescape("Midiplus%20AKM320%20MIDI%201"), "Midiplus AKM320 MIDI 1");
        assert_eq!(unescape("R%C3%A9cit+din"), "Récit din");
        assert_eq!(unescape("100%"), "100%", "a stray percent is kept");
    }

    #[test]
    fn stop_noises_are_hidden_and_fire_on_toggles() {
        let Some(state) = demo_state() else { return };

        // No control noise masquerades as a drawable stop.
        let body = state_json(&state);
        assert!(
            !body.contains("stop noise") && !body.contains("Motor noise"),
            "noise stops leaked into the stop list"
        );
        assert!(body.contains("\"noises\":{\"on\":true"));

        let mut guard = state.lock().expect("state");
        let Control::Organ(console) = &mut guard.control else {
            panic!("organ expected");
        };
        // Drawing Montre 8' produces its drawknob thump (a percussive
        // one-shot at noise volume).
        let montre = console
            .stop_states()
            .iter()
            .find(|(_, name, _, _, _)| *name == "Montre 8'")
            .map(|(id, _, _, _, _)| *id)
            .expect("Montre 8' visible");
        let (stopped, mut noise) = console.set_drawn(montre, true);
        assert!(stopped.is_empty());
        assert_eq!(noise.len(), 1, "drawstop noise mapped and produced");
        let noise = noise.remove(0);
        assert_eq!(noise.spec.wind_weight, 0.0, "noises draw no wind");

        // Redundant draw: no second thump.
        let (_, again) = console.set_drawn(montre, true);
        assert!(again.is_empty());

        // Retiring releases the noise voice (its note-off = the
        // push-in thump).
        let (stopped, on_retire) = console.set_drawn(montre, false);
        assert!(on_retire.is_empty());
        assert!(
            stopped.contains(&noise.handle),
            "retire must note-off the open noise voice"
        );

        // Coupler clack (demo couplers have mapped noises).
        let (_, clacks) = console.set_coupler(0, true);
        assert_eq!(clacks.len(), 1, "coupler noise mapped");
        let (unclack, _) = console.set_coupler(0, false);
        assert_eq!(unclack, vec![clacks[0].handle]);

        // Disabling kills open noise voices and mutes future toggles.
        console.set_drawn(montre, true);
        let kills = console.set_noises(false, 0.7);
        assert!(!kills.is_empty(), "open noise voices killed on disable");
        let (_, silent) = console.set_drawn(montre, false);
        assert!(silent.is_empty());
    }

    /// A state with no organ loaded — what the picker talks to.
    fn tone_state() -> Arc<Mutex<State>> {
        let (_engine, handle) =
            aristide_engine::Engine::new(48000.0, std::sync::Arc::new(Default::default()));
        Arc::new(Mutex::new(State {
            engine: handle,
            control: Control::Tone,
            midi_ports: Vec::new(),
            midi_config: Default::default(),
            config_path: None,
            organ_key: String::new(),
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
            composite_path: None,
            setup: Default::default(),
            compass_overrides: Vec::new(),
            pending_load: None,
            loading: None,
            load_error: None,
        }))
    }

    /// Loading over the API only queues: the main thread owns the
    /// stream, so the endpoint hands it the request and the snapshot
    /// says "loading" until it lands.
    #[test]
    fn loading_an_organ_is_queued_for_the_main_thread() {
        let state = tone_state();
        let file = std::env::temp_dir().join("aristide-load-queue-test.organ");
        std::fs::write(&file, "[Organ]").expect("fixture written");

        let missing = respond(&state, &Method::Post, "/api/organ/load");
        assert_eq!(missing.status_code().0, 400, "a path is required");
        let absent = respond(
            &state,
            &Method::Post,
            "/api/organ/load?path=/no/such/place.organ",
        );
        assert_eq!(absent.status_code().0, 400, "the path must exist");

        let url = format!("/api/organ/load?path={}", file.display());
        let queued = respond(&state, &Method::Post, &url);
        assert_eq!(queued.status_code().0, 200);
        {
            let state = state.lock().expect("state poisoned");
            let pending = state.pending_load.as_ref().expect("request queued");
            assert_eq!(pending.paths, vec![file.clone()]);
            assert!(!pending.initial, "a picker load must not exit on failure");
            assert!(state.loading.is_some(), "snapshot narrates the load");
        }
        assert!(state_json(&state).contains("\"loading\":"));

        // One at a time: a second request is refused, not stacked.
        let refused = respond(&state, &Method::Post, &url);
        assert_eq!(refused.status_code().0, 400);
        let _ = std::fs::remove_file(&file);
    }

    /// The blank-organ endpoint enforces the same rules as loading:
    /// a real name, one load at a time. (Creation itself is proven in
    /// `config::tests` — here would write into the user's config.)
    #[test]
    fn a_blank_organ_needs_a_name_and_a_free_load_slot() {
        let state = tone_state();
        let missing = respond(&state, &Method::Post, "/api/organ/new");
        assert_eq!(missing.status_code().0, 400, "a name is required");
        let blank = respond(&state, &Method::Post, "/api/organ/new?name=%20%20");
        assert_eq!(blank.status_code().0, 400, "whitespace is not a name");

        state.lock().expect("state poisoned").loading = Some("loading…".into());
        let busy = respond(&state, &Method::Post, "/api/organ/new?name=Chapel");
        assert_eq!(busy.status_code().0, 400, "one load at a time");
    }

    #[test]
    fn the_snapshot_offers_the_library_and_forget_removes() {
        let state = tone_state();
        state
            .lock()
            .expect("state poisoned")
            .midi_config
            .remember("Demo", Path::new("/sets/demo.organ"));
        let body = state_json(&state);
        assert!(
            body.contains("\"library\":[{\"name\":\"Demo\",\"path\":\"/sets/demo.organ\"}]"),
            "library present: {body}"
        );
        assert!(!body.contains("\"organ\":"), "no organ is loaded");

        respond(
            &state,
            &Method::Post,
            "/api/library/forget?path=/sets/demo.organ",
        );
        assert!(state_json(&state).contains("\"library\":[]"));
    }

    /// Renaming a sample-set organ: the name lands in the set's
    /// sidecar (the set itself is untouched), the assignments keyed by
    /// the old name move to the new one, and the library entry keeps
    /// its path while showing the new name.
    #[test]
    fn renaming_a_set_organ_follows_everywhere() {
        let Some(state) = demo_state() else { return };
        let dir = std::env::temp_dir().join("aristide-rename-set-test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("fixture dir");
        let set = dir.join("village.organ");
        std::fs::write(&set, "[Organ]").expect("fixture set");
        {
            let mut state = state.lock().expect("state poisoned");
            state.setup.sources = vec![("test organ".into(), set.clone())];
            state.midi_config.remember("test organ", &set);
            state.midi_config.set_input(
                "test organ",
                "First Manual",
                0,
                crate::config::Input {
                    device: "Test Keys".into(),
                    channel: Some(1),
                    low: None,
                    high: None,
                    transpose: 0,
                },
            );
        }

        let missing = respond(&state, &Method::Post, "/api/organ/rename");
        assert_eq!(missing.status_code().0, 400, "a name is required");
        let blank = respond(&state, &Method::Post, "/api/organ/rename?name=%20");
        assert_eq!(blank.status_code().0, 400, "whitespace is not a name");

        let renamed = respond(
            &state,
            &Method::Post,
            "/api/organ/rename?name=Chapel%20Royal",
        );
        assert_eq!(renamed.status_code().0, 200);
        let body = state_json(&state);
        assert!(body.contains("\"organ\":\"Chapel Royal\""), "live: {body}");
        assert!(
            body.contains("{\"name\":\"Chapel Royal\",\"path\":"),
            "library shows the new name: {body}"
        );
        {
            let state = state.lock().expect("state poisoned");
            assert_eq!(state.organ_key, "Chapel Royal");
            assert!(
                state.midi_config.organ("test organ").is_none(),
                "nothing left under the old key"
            );
            assert_eq!(
                state.midi_config.inputs("Chapel Royal", "First Manual")[0].device,
                "Test Keys",
                "the wiring moved with the name"
            );
            assert_eq!(
                state.midi_config.library[0].path, set,
                "the library still points at the same file"
            );
            assert_eq!(state.setup.sources[0].0, "Chapel Royal");
        }
        let sidecar = aristide_formats::sidecar::load_for(&set)
            .expect("sidecar parses")
            .expect("sidecar written");
        assert_eq!(sidecar.name, "Chapel Royal", "the rename is durable");
        assert_eq!(
            std::fs::read_to_string(&set).expect("set readable"),
            "[Organ]",
            "the set itself is never touched"
        );

        // Renaming to the current name is a no-op, not an error.
        let same = respond(
            &state,
            &Method::Post,
            "/api/organ/rename?name=Chapel%20Royal",
        );
        assert_eq!(same.status_code().0, 200);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A combination assembled ad hoc has no file to carry a name, so
    /// renaming it is refused with the way out; and with no organ at
    /// all there is nothing to rename.
    #[test]
    fn renaming_needs_an_organ_that_lives_in_a_file() {
        let state = tone_state();
        let refused = respond(&state, &Method::Post, "/api/organ/rename?name=Ghost");
        assert_eq!(refused.status_code().0, 400, "no organ, no rename");

        let Some(state) = demo_state() else { return };
        {
            let mut state = state.lock().expect("state poisoned");
            state.setup.sources = vec![
                ("A".into(), "/sets/a.organ".into()),
                ("B".into(), "/sets/b.organ".into()),
            ];
            state.setup.implicit = true;
        }
        let implicit = respond(&state, &Method::Post, "/api/organ/rename?name=Duo");
        assert_eq!(implicit.status_code().0, 400);
        assert_eq!(
            state.lock().expect("state poisoned").organ_key,
            "test organ",
            "a refused rename changes nothing"
        );
    }

    /// Renaming a composite organ rewrites the `name` in its own file —
    /// which keeps its path, so the library entry and the wiring the
    /// file owns keep working — and the file loads back under the new
    /// name.
    #[test]
    fn renaming_a_composite_edits_its_file_and_keeps_references() {
        let Some(state) = demo_state() else { return };
        let demo = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../testsets/grandorgue-demo/demo.organ");
        let path = std::env::temp_dir().join("aristide-rename-composite-test.toml");
        let _ = std::fs::remove_file(&path);
        {
            let mut state = state.lock().expect("state poisoned");
            let names: Vec<String> = state.manual_names();
            state.setup.sources = vec![("Demo".into(), demo.clone())];
            state.setup.pulls = names
                .iter()
                .enumerate()
                .map(|(index, name)| (0, name.clone(), index))
                .collect();
            state.setup.implicit = true;
            state.midi_config.remember("Demo", &demo);
        }
        respond(
            &state,
            &Method::Post,
            &format!(
                "/api/organ/save?path={}",
                path.display().to_string().replace('/', "%2F")
            ),
        );
        // Wiring learned before the rename, stored under the old name.
        respond(
            &state,
            &Method::Post,
            "/api/midi/bind?manual=0&slot=0&device=Test%20Keys&ch=4",
        );

        let renamed = respond(
            &state,
            &Method::Post,
            "/api/organ/rename?name=St%20Fictive",
        );
        assert_eq!(renamed.status_code().0, 200);

        let saved = aristide_formats::instrument::load(&path).expect("still loads");
        assert_eq!(saved.organ.name, "St Fictive", "the file carries the new name");
        assert_eq!(
            saved.midi.inputs[0].device, "Test Keys",
            "the wiring the file owns survives the rename"
        );
        {
            let state = state.lock().expect("state poisoned");
            assert_eq!(state.organ_key, "St Fictive");
            let canonical = path.canonicalize().expect("saved file exists");
            let entry = state
                .midi_config
                .library
                .iter()
                .find(|entry| entry.path == canonical)
                .expect("the composite is in the library");
            assert_eq!(entry.name, "St Fictive");
            let demo_entry = state
                .midi_config
                .library
                .iter()
                .find(|entry| entry.path != canonical)
                .expect("the source set stays in the library");
            assert_eq!(demo_entry.name, "Demo", "the source set keeps its name");
            let (_, inputs) = state
                .midi_config
                .assignments("St Fictive")
                .next()
                .expect("the wiring moved with the name");
            assert_eq!(inputs[0].device, "Test Keys");
        }
        // Wiring changed after the rename lands under the new key and
        // in the file.
        respond(
            &state,
            &Method::Post,
            "/api/midi/bind?manual=0&slot=0&device=Test%20Keys&ch=7",
        );
        let saved = aristide_formats::instrument::load(&path).expect("reloads");
        assert_eq!(saved.midi.inputs[0].channel, Some(7));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn browse_lists_directories_and_loadable_files_only() {
        let dir = std::env::temp_dir().join("aristide-browse-test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("sub")).expect("fixture dir");
        for name in [
            "set.organ",
            "combo.toml",
            "hw.Organ_Hauptwerk_xml",
            "readme.txt",
            ".hidden.organ",
        ] {
            std::fs::write(dir.join(name), "").expect("fixture file");
        }
        let body = browse_json(&dir).expect("browses");
        assert!(body.contains("\"sub\""), "subdirectory listed: {body}");
        assert!(body.contains("set.organ"), "sample set listed");
        assert!(body.contains("combo.toml"), "composite listed");
        assert!(body.contains("hw.Organ_Hauptwerk_xml"), "Hauptwerk listed");
        assert!(!body.contains("readme.txt"), "other files are noise");
        assert!(!body.contains(".hidden.organ"), "dotfiles skipped");
        assert!(
            body.find("\"sub\"").unwrap() < body.find("combo.toml").unwrap(),
            "directories come first"
        );
        assert!(browse_json(&dir.join("nowhere")).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
