//! MIDI and the control plane: which device feeds which manual, the
//! computer keyboard, learn, and what an input does when it isn't
//! playing a note.

use std::sync::Mutex;

use super::{bad_request, json, param, state_json, state_json_locked, unescape, Reply};
use crate::State;

// Input routing, addressed the way the player thinks about it:
// *this manual* listens to *that device*. `slot` numbers a
// manual's inputs (a manual may have several); a slot past the
// end adds one. `ch` is 1-16, or absent for any channel.
pub(super) fn bind(state: &Mutex<State>, query: &str) -> Reply {
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
            // Pitch-bend range in semitones; "off" (or 0)
            // disables. Absent, rebinding keeps what the slot
            // already had — like transpose.
            let bend = match param(query, "bend") {
                Some("off") => None,
                Some(value) => match value.parse::<f32>() {
                    Ok(semitones) if (0.0..=96.0).contains(&semitones) => {
                        (semitones > 0.0).then_some(semitones)
                    }
                    _ => return bad_request("bend must be 0-96 semitones"),
                },
                None => state
                    .manual_inputs(manual)
                    .get(slot)
                    .and_then(|input| input.bend),
            };
            // A Lumatone .ltn key map, resolved at route time;
            // "off" clears it. Absent, rebinding keeps it.
            let map = match param(query, "map").map(unescape) {
                Some(path) if path.is_empty() || path == "off" => None,
                Some(path) => Some(path),
                None => state
                    .manual_inputs(manual)
                    .get(slot)
                    .and_then(|input| input.map.clone()),
            };
            state.learn = None;
            if !state.propose_input(
                manual,
                slot,
                crate::config::Input {
                    device,
                    channel,
                    low,
                    high,
                    transpose,
                    bend,
                    map,
                },
            ) {
                return bad_request("no such manual");
            }
            json(state_json_locked(&state))
        }
        _ => bad_request("missing manual/slot/device"),
    }
}

pub(super) fn unbind(state: &Mutex<State>, query: &str) -> Reply {
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
pub(super) fn learn(state: &Mutex<State>, query: &str) -> Reply {
    let mut state = state.lock().expect("state poisoned");
    let manual = param(query, "manual").and_then(|v| v.parse::<usize>().ok());
    let slot = param(query, "slot").and_then(|v| v.parse::<usize>().ok());
    match (manual, slot) {
        (Some(manual), Some(slot)) => {
            let manuals = state.console().map_or(0, |console| console.manual_states().len());
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
pub(super) fn key(state: &Mutex<State>, query: &str) -> Reply {
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
pub(super) fn action(state: &Mutex<State>, query: &str) -> Reply {
    match param(query, "do").map(unescape) {
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
    }
}

// Bindings: what an input does when it isn't playing a note.
pub(super) fn control_bind(state: &Mutex<State>, query: &str) -> Reply {
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
            state.propose_control(slot, control);
            json(state_json_locked(&state))
        }
        (Some(_), Some(_)) => bad_request("no such action"),
        _ => bad_request("missing slot/action"),
    }
}

pub(super) fn control_unbind(state: &Mutex<State>, query: &str) -> Reply {
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

// The player's answer to a parked bind — one that would give a
// device (or one of its messages) a second job. "keep" commits
// it alongside the old rows, "replace" retires them in its
// favour, "cancel" drops it.
pub(super) fn conflict(state: &Mutex<State>, query: &str) -> Reply {
    let resolution = match param(query, "choice") {
        Some("keep") => crate::Resolution::KeepBoth,
        Some("replace") => crate::Resolution::Replace,
        Some("cancel") => crate::Resolution::Cancel,
        _ => return bad_request("choice must be keep, replace or cancel"),
    };
    let mut state = state.lock().expect("state poisoned");
    // Nothing pending is not an error: the dialog may have
    // raced an organ load, and there is nothing left to do.
    state.resolve_pending(resolution);
    json(state_json_locked(&state))
}

// Auto-detect for controls: press the piston, pedal or key.
pub(super) fn control_learn(state: &Mutex<State>, query: &str) -> Reply {
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

pub(super) fn rescan(state: &Mutex<State>, _query: &str) -> Reply {
    crate::request_midi_rescan();
    json(state_json(state))
}
