//! Couplers: engaging them, keeping them on the console, defining
//! their routes, linking them and how they show coupled keys.

use std::sync::Mutex;

use aristide_engine::Command;

use super::{bad_request, json, param, send_start, unescape, Reply};
use super::snapshot::{state_json, state_json_locked};
use crate::State;

pub(super) fn engage(state: &Mutex<State>, query: &str) -> Reply {
    let index = param(query, "idx").and_then(|v| v.parse::<usize>().ok());
    let on = param(query, "on") == Some("1");
    match index {
        Some(index) => {
            {
                let mut state = state.lock().expect("state poisoned");
                let State {
                    engine, control, ..
                } = &mut *state;
                if let Some(console) = control.organ_mut() {
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

// Keep a coupler on the console (`keep=1`) or take it off
// (`keep=0`). Off is hidden and disengaged, not deleted.
pub(super) fn keep(state: &Mutex<State>, query: &str) -> Reply {
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

// Rename a coupler — a rocker's engraving, live like a stop
// rename; the file keeps it and name-keyed references follow.
pub(super) fn rename(state: &Mutex<State>, query: &str) -> Reply {
    let mut state = state.lock().expect("state poisoned");
    if state.is_loading() {
        return bad_request("an organ is already loading");
    }
    match (
        param(query, "idx").and_then(|v| v.parse::<usize>().ok()),
        param(query, "name").map(unescape),
    ) {
        (Some(index), Some(name)) => match state.rename_coupler(index, &name) {
            Ok(()) => json(state_json_locked(&state)),
            Err(err) => bad_request(&err),
        },
        _ => bad_request("missing idx/name"),
    }
}

// Replace a coupler's routes (`routes=` a JSON array of
// {from, to, shift, low, high, unison_off, scope, repitch} —
// manuals as console indexes). Structural: a source's coupler
// materializes as this organ's own define, and the organ
// rebuilds.
pub(super) fn routes(state: &Mutex<State>, query: &str) -> Reply {
    let mut state = state.lock().expect("state poisoned");
    if state.is_loading() {
        return bad_request("an organ is already loading");
    }
    let Some(index) = param(query, "idx").and_then(|v| v.parse::<usize>().ok()) else {
        return bad_request("missing idx");
    };
    let routes: Vec<crate::CouplerRouteEdit> =
        match param(query, "routes").map(unescape).as_deref().map(serde_json::from_str)
        {
            Some(Ok(routes)) => routes,
            Some(Err(err)) => return bad_request(&format!("routes: {err}")),
            None => return bad_request("missing routes"),
        };
    match state.set_coupler_routes(index, &routes) {
        Ok(()) => json(state_json_locked(&state)),
        Err(err) => bad_request(&err),
    }
}

// Define a brand-new coupler — same route vocabulary, same
// structural contract.
pub(super) fn add(state: &Mutex<State>, query: &str) -> Reply {
    let mut state = state.lock().expect("state poisoned");
    if state.is_loading() {
        return bad_request("an organ is already loading");
    }
    let Some(name) = param(query, "name").map(unescape) else {
        return bad_request("missing name");
    };
    let routes: Vec<crate::CouplerRouteEdit> =
        match param(query, "routes").map(unescape).as_deref().map(serde_json::from_str)
        {
            Some(Ok(routes)) => routes,
            Some(Err(err)) => return bad_request(&format!("routes: {err}")),
            None => return bad_request("missing routes"),
        };
    match state.add_coupler(&name, &routes) {
        Ok(()) => json(state_json_locked(&state)),
        Err(err) => bad_request(&err),
    }
}

// Delete a coupler outright: a define is removed from the file
// (rebuild); a source's coupler is taken off the console
// instead, restorable from the Organ preferences.
pub(super) fn remove(state: &Mutex<State>, query: &str) -> Reply {
    let mut state = state.lock().expect("state poisoned");
    if state.is_loading() {
        return bad_request("an organ is already loading");
    }
    match param(query, "idx").and_then(|v| v.parse::<usize>().ok()) {
        Some(index) => match state.remove_coupler(index) {
            Ok(()) => json(state_json_locked(&state)),
            Err(err) => bad_request(&err),
        },
        None => bad_request("missing idx"),
    }
}

// Link (`on=1`) or unlink two couplers so they move together —
// live and in the file's [couplers] link, no rebuild.
pub(super) fn link(state: &Mutex<State>, query: &str) -> Reply {
    let mut state = state.lock().expect("state poisoned");
    if state.is_loading() {
        return bad_request("an organ is already loading");
    }
    match (
        param(query, "idx").and_then(|v| v.parse::<usize>().ok()),
        param(query, "with").and_then(|v| v.parse::<usize>().ok()),
        param(query, "on").map(|v| v != "0"),
    ) {
        (Some(index), Some(with), Some(on)) => {
            match state.link_coupler(index, with, on) {
                Ok(()) => json(state_json_locked(&state)),
                Err(err) => bad_request(&err),
            }
        }
        _ => bad_request("missing idx/with/on"),
    }
}

// One coupler's coupled-keys override: auto (follow the organ
// default), never, or always. Display only — live, no rebuild.
pub(super) fn keys(state: &Mutex<State>, query: &str) -> Reply {
    let mut state = state.lock().expect("state poisoned");
    if state.is_loading() {
        return bad_request("an organ is already loading");
    }
    let mode = match param(query, "mode") {
        Some("auto") => None,
        Some(mode @ ("never" | "always")) => Some(mode),
        _ => return bad_request("mode must be auto, never or always"),
    };
    match param(query, "idx").and_then(|v| v.parse::<usize>().ok()) {
        Some(index) => match state.set_coupler_key_mode(index, mode) {
            Ok(()) => json(state_json_locked(&state)),
            Err(err) => bad_request(&err),
        },
        None => bad_request("missing idx"),
    }
}

// The organ-wide coupled-keys default: whether engaged couplers
// pull the coupled keys down on screen. Display only — live.
pub(super) fn coupled_keys(state: &Mutex<State>, query: &str) -> Reply {
    let mut state = state.lock().expect("state poisoned");
    if state.is_loading() {
        return bad_request("an organ is already loading");
    }
    match param(query, "on").map(|v| v != "0") {
        Some(on) => match state.set_coupled_keys(on) {
            Ok(()) => json(state_json_locked(&state)),
            Err(err) => bad_request(&err),
        },
        None => bad_request("missing on"),
    }
}

// Whether couplers may repitch to reach pipes a division hasn't
// got. Off is the default and the musically honest answer; a
// piece that wants the other can turn it on without editing the
// set's sidecar.
pub(super) fn repitch(state: &Mutex<State>, query: &str) -> Reply {
    let on = param(query, "repitch") == Some("1");
    {
        let mut state = state.lock().expect("state poisoned");
        if let Some(console) = state.console_mut() {
            console.set_coupler_repitch(on);
            tracing::info!("couplers: repitch {}", if on { "on" } else { "off" });
        }
    }
    json(state_json(state))
}
