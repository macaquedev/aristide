//! The playing surface: keys pressed on screen, general pistons,
//! cancel and panic.

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
            state.recall_general(slot);
        }
    }
    json(state_json_locked(&state))
}
