//! The playing surface: keys pressed on screen, the combination action
//! (generals, divisionals, the stepper, the crescendo), cancel and
//! panic.
//!
//! Every endpoint here is the on-screen twin of a binding action, and
//! both land on the same `State` method — a piston under a thumb and a
//! piston under a mouse must never come to mean different things.

use std::sync::Mutex;

use aristide_engine::Command;

use super::{apply_note, bad_request, json, param, Reply};
use super::snapshot::{state_json, state_json_locked};
use crate::State;

pub(super) fn cancel(state: &Mutex<State>, _query: &str) -> Reply {
    {
        let mut state = state.lock().expect("state poisoned");
        let State {
            engine, control, ..
        } = &mut *state;
        if let Some(console) = control.organ_mut() {
            for handle in console.cancel() {
                engine.send(Command::StopVoice { handle });
            }
        }
    }
    json(state_json(state))
}

pub(super) fn note(state: &Mutex<State>, query: &str) -> Reply {
    let manual = param(query, "manual").and_then(|v| v.parse::<usize>().ok());
    let key = param(query, "key").and_then(|v| v.parse::<u16>().ok());
    let on = param(query, "on") == Some("1");
    match (manual, key) {
        (Some(manual), Some(key)) if key < 4096 => {
            apply_note(state, manual, key, on);
            json(state_json(state))
        }
        _ => bad_request("missing manual/key"),
    }
}

pub(super) fn panic_button(state: &Mutex<State>, _query: &str) -> Reply {
    let mut state = state.lock().expect("state poisoned");
    let State {
        engine, control, ..
    } = &mut *state;
    if let Some(console) = control.organ_mut() {
        console.all_off();
    }
    engine.send(Command::AllNotesOff);
    json(state_json_locked(&state))
}

pub(super) fn general(state: &Mutex<State>, query: &str) -> Reply {
    let slot = param(query, "n").and_then(|v| v.parse::<u8>().ok());
    let mut state = state.lock().expect("state poisoned");
    if let Some(slot) = slot {
        if param(query, "store") == Some("1") {
            state.store_general(slot);
        } else {
            // Not `recall_general`: a press on screen must mean what a
            // press under a thumb means, and with the setter armed
            // that is "store".
            state.general(slot);
        }
    }
    json(state_json_locked(&state))
}

/// `?manual=<index>&n=<slot>` recalls; `&store=1` stores. The manual
/// is an index, as everywhere else on this API — a binding names it,
/// the console points at it.
pub(super) fn divisional(state: &Mutex<State>, query: &str) -> Reply {
    let manual = param(query, "manual").and_then(|v| v.parse::<usize>().ok());
    let slot = param(query, "n").and_then(|v| v.parse::<u8>().ok());
    let (Some(manual), Some(slot)) = (manual, slot) else {
        return bad_request("missing manual/n");
    };
    let mut state = state.lock().expect("state poisoned");
    if param(query, "store") == Some("1") {
        state.store_divisional(manual, slot);
    } else {
        state.divisional(manual, slot);
    }
    json(state_json_locked(&state))
}

/// `?go=next|prev|<frame>` walks the sequence; `?store=1`, `?insert=1`
/// and `?delete=1` are the editing gestures the piston rail offers.
pub(super) fn stepper(state: &Mutex<State>, query: &str) -> Reply {
    let mut state = state.lock().expect("state poisoned");
    match param(query, "go") {
        Some("next") => state.stepper_next(),
        Some("prev") => state.stepper_prev(),
        Some(frame) => match frame.parse::<u16>() {
            Ok(frame) => state.stepper_goto(frame),
            Err(_) => return bad_request("go = next | prev | <frame>"),
        },
        None => {}
    }
    if param(query, "store") == Some("1") {
        state.stepper_store();
    }
    if param(query, "insert") == Some("1") {
        state.stepper_insert();
    }
    if param(query, "delete") == Some("1") {
        state.stepper_delete();
    }
    json(state_json_locked(&state))
}

/// `?stage=<n>` moves the pedal (0 = heel); `&store=1` writes the hand
/// registration into that stage instead, which is what Set + the
/// stage's own piston does.
pub(super) fn crescendo(state: &Mutex<State>, query: &str) -> Reply {
    let Some(stage) = param(query, "stage").and_then(|v| v.parse::<u8>().ok()) else {
        return bad_request("missing stage");
    };
    let mut state = state.lock().expect("state poisoned");
    if param(query, "store") == Some("1") {
        state.store_crescendo(stage);
    } else {
        state.set_crescendo(stage);
    }
    json(state_json_locked(&state))
}

/// Arm or disarm the setter — the console's Set piston, which decides
/// what the *next* combination press means.
pub(super) fn setter(state: &Mutex<State>, query: &str) -> Reply {
    let mut state = state.lock().expect("state poisoned");
    state.setter_armed = match param(query, "on") {
        Some("1") => true,
        Some("0") => false,
        _ => !state.setter_armed,
    };
    json(state_json_locked(&state))
}
