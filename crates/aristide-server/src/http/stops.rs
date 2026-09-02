//! Stops: drawing them, pulling them off a source, retiring them,
//! and everything the console's stop editor changes.

use std::sync::Mutex;

use super::{apply_stop, bad_request, json, param, unescape, Reply};
use super::snapshot::{state_json, state_json_locked};
use crate::State;

pub(super) fn draw(state: &Mutex<State>, query: &str) -> Reply {
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

// Move a stop to another manual — the ranges re-anchor by
// pitch, and the change lands in the organ's file when it has
// one.
pub(super) fn move_to_manual(state: &Mutex<State>, query: &str) -> Reply {
    let mut state = state.lock().expect("state poisoned");
    // Live, but it writes manual NAMES to the file — mid-rebuild
    // the console's names can be stale (a rename just rewrote
    // them), and a stale [[move]] leaves the file unloadable.
    if state.is_loading() {
        return bad_request("an organ is already loading");
    }
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

pub(super) fn pull(state: &Mutex<State>, query: &str) -> Reply {
    let mut state = state.lock().expect("state poisoned");
    if state.is_loading() {
        return bad_request("an organ is already loading");
    }
    match (
        param(query, "from").map(unescape),
        param(query, "manual").map(unescape),
        param(query, "on").map(unescape),
    ) {
        (Some(from), Some(manual), Some(on)) => {
            let stop = param(query, "stop").map(unescape);
            match state.pull_from_source(&from, &manual, stop.as_deref(), &on) {
                Ok(()) => json(state_json_locked(&state)),
                Err(err) => bad_request(&err),
            }
        }
        _ => bad_request("missing from/manual/on"),
    }
}

pub(super) fn unpull(state: &Mutex<State>, query: &str) -> Reply {
    let mut state = state.lock().expect("state poisoned");
    if state.is_loading() {
        return bad_request("an organ is already loading");
    }
    match param(query, "stop").and_then(|v| v.parse::<u32>().ok()) {
        Some(stop) => match state.remove_stop(aristide_model::StopId(stop)) {
            Ok(()) => json(state_json_locked(&state)),
            Err(err) => bad_request(&err),
        },
        None => bad_request("missing stop"),
    }
}

// Rename a stop — a label, so it lands live (no rebuild) and
// is kept in the organ's file. Refused mid-load: the write
// addresses file lines by names a rebuild may be changing.
pub(super) fn rename(state: &Mutex<State>, query: &str) -> Reply {
    let mut state = state.lock().expect("state poisoned");
    if state.is_loading() {
        return bad_request("an organ is already loading");
    }
    match (
        param(query, "stop").and_then(|v| v.parse::<u32>().ok()),
        param(query, "name").map(unescape),
    ) {
        (Some(stop), Some(name)) => {
            match state.rename_stop(aristide_model::StopId(stop), &name) {
                Ok(()) => json(state_json_locked(&state)),
                Err(err) => bad_request(&err),
            }
        }
        _ => bad_request("missing stop/name"),
    }
}

// A stop's own voicing: `footage` (feet — "16", "4", "2 2/3";
// "native" or empty goes back to the samples' own pitch),
// `cents` on top, `gain` in dB, or `reset=1` for all-neutral.
// Fields left out keep their current values. Live — held keys
// re-speak the stop at its new pitch — and written to the
// file's [[voicing.adjust]]; no rebuild.
pub(super) fn voice(state: &Mutex<State>, query: &str) -> Reply {
    let mut state = state.lock().expect("state poisoned");
    if state.is_loading() {
        return bad_request("an organ is already loading");
    }
    let Some(stop) = param(query, "stop").and_then(|v| v.parse::<u32>().ok()) else {
        return bad_request("missing stop");
    };
    let stop = aristide_model::StopId(stop);
    let mut voicing = if param(query, "reset").is_some_and(|v| v != "0") {
        crate::load::StopVoicing::default()
    } else {
        state.stop_voicing.get(&stop).copied().unwrap_or_default()
    };
    if let Some(text) = param(query, "footage").map(unescape) {
        let text = text.trim();
        if text.is_empty() || text.eq_ignore_ascii_case("native") {
            voicing.feet = None;
        } else {
            match aristide_formats::sidecar::parse_footage(text) {
                Some(feet) => voicing.feet = Some(feet),
                None => {
                    return bad_request(&format!("{text:?} names no footage"));
                }
            }
        }
    }
    if let Some(cents) = param(query, "cents") {
        match cents.parse::<f64>() {
            Ok(cents) if cents.is_finite() => {
                voicing.cents = cents.clamp(-2400.0, 2400.0)
            }
            _ => return bad_request("cents must be a number"),
        }
    }
    if let Some(gain) = param(query, "gain") {
        match gain.parse::<f64>() {
            Ok(gain) if gain.is_finite() => {
                voicing.gain_db = gain.clamp(-40.0, 20.0)
            }
            _ => return bad_request("gain must be a number of dB"),
        }
    }
    if let Some(brightness) = param(query, "brightness") {
        match brightness.parse::<f64>() {
            Ok(db) if db.is_finite() => voicing.brightness_db = db.clamp(-12.0, 12.0),
            _ => return bad_request("brightness must be a number of dB"),
        }
    }
    match state.set_stop_voicing(stop, voicing) {
        Ok(()) => json(state_json_locked(&state)),
        Err(err) => bad_request(&err),
    }
}

// Voicing at any scope inside a stop: `stop=<id>` plus, to narrow it,
// `keys=C2..B3` (or `key=F#3`, or raw key numbers on a microtonal
// manual) and/or `rank=<name>`. `gain`/`cents`/`brightness` set a
// field, an empty value unsays it (the pipes then follow the broader
// rule), and `clear=1` removes the whole rule. With no `keys`/`key`/
// `rank` this IS the stop's own rule and behaves exactly like
// /api/organ/stop/voice.
//
// Live: level and tone slide under held keys, pitch glides — a rebuild
// only if a pitch trim re-anchors keys onto other pipes.
pub(super) fn voicing(state: &Mutex<State>, query: &str) -> Reply {
    let mut state = state.lock().expect("state poisoned");
    if state.is_loading() {
        return bad_request("an organ is already loading");
    }
    let Some(stop) = param(query, "stop").and_then(|v| v.parse::<u32>().ok()) else {
        return bad_request("missing stop");
    };
    let stop = aristide_model::StopId(stop);
    let clear = param(query, "clear").is_some_and(|v| v != "0");
    let span = match (param(query, "key").map(unescape), param(query, "keys").map(unescape)) {
        (None, None) => None,
        (Some(_), Some(_)) => return bad_request("key and keys name the same thing"),
        (Some(text), None) | (None, Some(text)) => {
            match aristide_formats::sidecar::parse_key_span(&text) {
                Some(span) => Some(span),
                None => {
                    return bad_request(&format!(
                        "{text:?} is not a key or a key span (\"C2..B3\", \"F#3\", \"48..59\")"
                    ))
                }
            }
        }
    };
    let rank = param(query, "rank").map(unescape).filter(|r| !r.trim().is_empty());
    // No narrowing: this is the stop's own rule, so it goes through the
    // stop editor's own path and keeps its footage.
    if span.is_none() && rank.is_none() {
        let mut voicing = if clear {
            crate::load::StopVoicing::default()
        } else {
            state.stop_voicing.get(&stop).copied().unwrap_or_default()
        };
        for (name, field, floor, ceiling) in [
            ("cents", 0u8, -2400.0, 2400.0),
            ("gain", 1, -40.0, 20.0),
            ("brightness", 2, -12.0, 12.0),
        ] {
            let Some(text) = param(query, name) else { continue };
            let value = match text.parse::<f64>() {
                Ok(value) if value.is_finite() => value.clamp(floor, ceiling),
                // An empty value at stop scope is "no trim" — there is
                // nothing broader for it to fall through to.
                _ if text.is_empty() => 0.0,
                _ => return bad_request(&format!("{name} must be a number")),
            };
            match field {
                0 => voicing.cents = value,
                1 => voicing.gain_db = value,
                _ => voicing.brightness_db = value,
            }
        }
        return match state.set_stop_voicing(stop, voicing) {
            Ok(()) => json(state_json_locked(&state)),
            Err(err) => bad_request(&err),
        };
    }
    let scope = crate::load::VoicingScope { keys: span, rank };
    let mut voicing = if clear {
        crate::load::PipeVoicing::default()
    } else {
        state
            .pipe_voicing
            .get(&(stop, scope.clone()))
            .copied()
            .unwrap_or_default()
    };
    for (name, field, floor, ceiling) in [
        ("cents", 0u8, -2400.0, 2400.0),
        ("gain", 1, -40.0, 20.0),
        ("brightness", 2, -12.0, 12.0),
    ] {
        let Some(text) = param(query, name) else { continue };
        // Empty unsays the field: these pipes go back to following the
        // stop's own rule for it, which is not the same as pinning 0.
        let value = if text.is_empty() {
            None
        } else {
            match text.parse::<f64>() {
                Ok(value) if value.is_finite() => Some(value.clamp(floor, ceiling)),
                _ => return bad_request(&format!("{name} must be a number")),
            }
        };
        match field {
            0 => voicing.cents = value,
            1 => voicing.gain_db = value,
            _ => voicing.brightness_db = value,
        }
    }
    match state.set_pipe_voicing(stop, scope, voicing) {
        Ok(()) => json(state_json_locked(&state)),
        Err(err) => bad_request(&err),
    }
}

// A stop's knob engraving: `label=` is the footage line the
// drawknob face shows (empty = engrave nothing), `auto=1` goes
// back to showing the footage the stop actually speaks at.
// A label, so it lands live — no rebuild.
pub(super) fn label(state: &Mutex<State>, query: &str) -> Reply {
    let mut state = state.lock().expect("state poisoned");
    if state.is_loading() {
        return bad_request("an organ is already loading");
    }
    let Some(stop) = param(query, "stop").and_then(|v| v.parse::<u32>().ok()) else {
        return bad_request("missing stop");
    };
    let label = if param(query, "auto").is_some_and(|v| v != "0") {
        None
    } else {
        match param(query, "label").map(unescape) {
            Some(label) => Some(label),
            None => return bad_request("missing label (or auto=1)"),
        }
    };
    match state.set_stop_pitch_label(aristide_model::StopId(stop), label) {
        Ok(()) => json(state_json_locked(&state)),
        Err(err) => bad_request(&err),
    }
}

// Whether a stop speaks pipes of its own (`own=1` doubles
// pipes other stops already sound) or shares them (`own=0`,
// the default and what a real unit action does). Lands live —
// held keys re-derive — and is kept in the organ file.
pub(super) fn own_pipes(state: &Mutex<State>, query: &str) -> Reply {
    let mut state = state.lock().expect("state poisoned");
    if state.is_loading() {
        return bad_request("an organ is already loading");
    }
    let Some(stop) = param(query, "stop").and_then(|v| v.parse::<u32>().ok()) else {
        return bad_request("missing stop");
    };
    let Some(own) = param(query, "own").map(|v| v != "0") else {
        return bad_request("missing own");
    };
    match state.set_stop_own_pipes(aristide_model::StopId(stop), own) {
        Ok(()) => json(state_json_locked(&state)),
        Err(err) => bad_request(&err),
    }
}

// Point a stop at a different source stop — same drawknob,
// same label, different pipes. Structural (the pull lines are
// rewritten), so the organ rebuilds.
pub(super) fn source(state: &Mutex<State>, query: &str) -> Reply {
    let mut state = state.lock().expect("state poisoned");
    if state.is_loading() {
        return bad_request("an organ is already loading");
    }
    match (
        param(query, "stop").and_then(|v| v.parse::<u32>().ok()),
        param(query, "from").map(unescape),
        param(query, "manual").map(unescape),
        param(query, "source_stop").map(unescape),
    ) {
        (Some(stop), Some(from), Some(manual), Some(source_stop)) => {
            match state.retarget_stop(
                aristide_model::StopId(stop),
                &from,
                &manual,
                &source_stop,
            ) {
                Ok(()) => json(state_json_locked(&state)),
                Err(err) => bad_request(&err),
            }
        }
        _ => bad_request("missing stop/from/manual/source_stop"),
    }
}

// A division's drawknob order, top of the jamb first — display
// only, so it lands live like panel placement: no rebuild, no
// ids moved, just the snapshot dealing the rank out anew.
// `items=` is the full vocabulary (`s<id>` stops, `c<idx>`
// couplers seated in the jamb); `stops=` is the older
// stops-only spelling and still accepted.
pub(super) fn order(state: &Mutex<State>, query: &str) -> Reply {
    let mut state = state.lock().expect("state poisoned");
    if state.is_loading() {
        return bad_request("an organ is already loading");
    }
    let items: Option<Result<Vec<crate::RankItem>, ()>> = match (
        param(query, "items"),
        param(query, "stops"),
    ) {
        (Some(list), _) => Some(
            list.split(',')
                .filter(|part| !part.is_empty())
                .map(|part| match part.split_at(1) {
                    ("s", id) => id
                        .parse::<u32>()
                        .map(|id| crate::RankItem::Stop(aristide_model::StopId(id)))
                        .map_err(|_| ()),
                    ("c", index) => index
                        .parse::<usize>()
                        .map(crate::RankItem::Coupler)
                        .map_err(|_| ()),
                    _ => Err(()),
                })
                .collect(),
        ),
        (None, Some(list)) => Some(
            list.split(',')
                .filter(|part| !part.is_empty())
                .map(|part| {
                    part.parse::<u32>()
                        .map(|id| crate::RankItem::Stop(aristide_model::StopId(id)))
                        .map_err(|_| ())
                })
                .collect(),
        ),
        (None, None) => None,
    };
    match (
        param(query, "manual").and_then(|v| v.parse::<usize>().ok()),
        items,
    ) {
        (Some(manual), Some(Ok(items))) => {
            match state.set_rank_order(manual, &items) {
                Ok(()) => json(state_json_locked(&state)),
                Err(err) => bad_request(&err),
            }
        }
        _ => bad_request("missing manual/items (comma-separated s<id>/c<idx>)"),
    }
}
