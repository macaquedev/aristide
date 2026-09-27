//! The player's console: its keyboards, taught once for every organ,
//! and which of them plays each of the loaded organ's manuals.

use std::sync::Mutex;

use super::snapshot::state_json_locked;
use super::{Reply, bad_request, json, param, unescape};
use crate::config::KeyboardChoice;
use crate::{KeyboardEdit, State};

fn index(query: &str, name: &str) -> Option<usize> {
    param(query, name).and_then(|v| v.parse::<usize>().ok())
}

// Add a keyboard with no device yet: `pedal=1` for a pedalboard.
pub(super) fn add(state: &Mutex<State>, query: &str) -> Reply {
    let mut state = state.lock().expect("state poisoned");
    state.add_console_keyboard(param(query, "pedal") == Some("1"));
    json(state_json_locked(&state))
}

// Change a keyboard: any of name, pedal (0/1), device (empty for none),
// ch (1-16 or any), and low+high (MIDI notes) or range=organ to forget
// a learned compass.
pub(super) fn set(state: &Mutex<State>, query: &str) -> Reply {
    let Some(keyboard) = index(query, "keyboard") else {
        return bad_request("missing keyboard");
    };
    let channel = match param(query, "ch") {
        None => None,
        Some("any") => Some(None),
        Some(value) => match value.parse::<u8>() {
            Ok(ch) if (1..=16).contains(&ch) => Some(Some(ch)),
            _ => return bad_request("ch must be 1-16 or any"),
        },
    };
    let note = |name| {
        param(query, name)
            .and_then(|v| v.parse::<u8>().ok())
            .filter(|k| *k < 128)
    };
    let compass = match (param(query, "range"), note("low"), note("high")) {
        (Some("organ"), ..) => Some(None),
        (_, Some(low), Some(high)) => Some(Some((low.min(high), low.max(high)))),
        _ => None,
    };
    let edit = KeyboardEdit {
        name: param(query, "name").map(unescape),
        pedal: param(query, "pedal").map(|v| v == "1"),
        device: param(query, "device").map(unescape),
        channel,
        compass,
    };
    let mut state = state.lock().expect("state poisoned");
    match state.edit_console_keyboard(keyboard, edit) {
        Ok(()) => json(state_json_locked(&state)),
        Err(reason) => bad_request(reason),
    }
}

pub(super) fn remove(state: &Mutex<State>, query: &str) -> Reply {
    let Some(keyboard) = index(query, "keyboard") else {
        return bad_request("missing keyboard");
    };
    let mut state = state.lock().expect("state poisoned");
    if !state.remove_console_keyboard(keyboard) {
        return bad_request("no such keyboard");
    }
    json(state_json_locked(&state))
}

// Detection: wait for a key on `keyboard` (past the end adds one on the
// first press, a pedalboard with pedal=1). range=1 waits for the lowest
// key and then the highest. No keyboard stops listening.
pub(super) fn learn(state: &Mutex<State>, query: &str) -> Reply {
    let mut state = state.lock().expect("state poisoned");
    match index(query, "keyboard") {
        Some(keyboard) => {
            tracing::info!("console: listening for keyboard {keyboard}");
            state.listen_console(
                keyboard,
                param(query, "pedal") == Some("1"),
                param(query, "range") == Some("1"),
            );
        }
        None => state.learn = None,
    }
    json(state_json_locked(&state))
}

// Which console keyboard plays one of the loaded organ's manuals:
// keyboard=<name>, none, or auto for the console's order.
pub(super) fn map(state: &Mutex<State>, query: &str) -> Reply {
    let Some(manual) = index(query, "manual") else {
        return bad_request("missing manual");
    };
    let choice = match param(query, "keyboard").map(unescape).as_deref() {
        Some("auto") => None,
        Some("none") => Some(KeyboardChoice::Nothing),
        Some(name) if !name.is_empty() => Some(KeyboardChoice::Console(name.to_string())),
        _ => return bad_request("keyboard must be a name, none or auto"),
    };
    let mut state = state.lock().expect("state poisoned");
    match state.map_manual(manual, choice) {
        Ok(()) => json(state_json_locked(&state)),
        Err(reason) => bad_request(reason),
    }
}
