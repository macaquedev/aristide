//! Route: which speaker groups each division and stop sounds through,
//! and Setup's speaker groups themselves.

use std::sync::Mutex;

use aristide_model::StopId;
use serde_json::{json, Value};

use super::{bad_request, json, param, unescape, Reply};
use crate::routing::{Resolved, MAIN};
use crate::state::RouteChange;
use crate::State;

/// `GET /api/routing`: the speaker groups as columns, then every
/// division and stop with the sends in effect and whether it owns them.
pub(super) fn matrix(state: &Mutex<State>, _query: &str) -> Reply {
    json(routing_json(&state.lock().expect("state poisoned")))
}

fn routing_json(state: &State) -> String {
    let available = |left: u8, right: u8| (left.max(right) as usize) < state.output_channels;
    let mut speakers = vec![json!({
        "name": MAIN, "output": [1, 2], "defined": true, "available": true,
    })];
    for speaker in state.speakers() {
        speakers.push(json!({
            "name": speaker.name,
            "output": [speaker.left + 1, speaker.right + 1],
            "defined": true,
            "available": available(speaker.left, speaker.right),
        }));
    }
    // Groups this organ sends to that Setup doesn't define: shown, so
    // the player sees why those sources play through Main.
    let defined = state.speakers();
    for group in state.routing.groups_in_use() {
        if crate::routing::speaker_pair(&defined, group).is_none() {
            speakers.push(json!({ "name": group, "output": null, "defined": false, "available": false }));
        }
    }
    let routing = &state.routing;
    let (divisions, stops): (Vec<Value>, Vec<Value>) = match state.console() {
        Some(console) => (
            console
                .manual_states()
                .iter()
                .enumerate()
                .map(|(idx, (_, name, ..))| {
                    json!({
                        "idx": idx,
                        "name": name,
                        "own": routing.divisions.get(idx).is_some_and(Option::is_some),
                        "sends": routing.division(idx),
                    })
                })
                .collect(),
            console
                .stop_states()
                .iter()
                .map(|(id, name, _, midx, _)| {
                    let (sends, file) = match routing.stop(*id, *midx) {
                        Resolved::Sends(sends) => (json!(sends), Value::Null),
                        Resolved::File(table) => (Value::Null, json!(routing.file_buses[table])),
                    };
                    json!({
                        "id": id.0,
                        "name": name,
                        "midx": midx,
                        "own": routing.stops.contains_key(id),
                        "file": file,
                        "sends": sends,
                    })
                })
                .collect(),
        ),
        None => (Vec::new(), Vec::new()),
    };
    json!({
        "channels": state.output_channels,
        "speakers": speakers,
        "divisions": divisions,
        "stops": stops,
    })
    .to_string()
}

/// `POST /api/routing?manual=<idx>[&stop=<id>]` with `speakers=<group>`
/// and `level_db=<dB>` to send (or re-level), `speakers=<group>&off=1`
/// to stop sending, `sends=Main:0,Rear:-6` to set them all at once (empty
/// for nowhere), or `follow=1` to drop the source's own sends.
pub(super) fn set(state: &Mutex<State>, query: &str) -> Reply {
    let mut state = state.lock().expect("state poisoned");
    if state.is_loading() {
        return bad_request("an organ is already loading");
    }
    let stop = param(query, "stop").and_then(|v| v.parse::<u32>().ok()).map(StopId);
    let manual = param(query, "manual").and_then(|v| v.parse::<usize>().ok()).or_else(|| {
        let stop = stop?;
        let console = state.console()?;
        console.stop_states().iter().find(|(id, ..)| *id == stop).map(|(.., m, _)| *m)
    });
    let Some(manual) = manual else {
        return bad_request("name a division (manual=) or a stop (stop=)");
    };
    let group = param(query, "speakers").map(unescape);
    let change = if param(query, "follow") == Some("1") {
        RouteChange::Follow
    } else if let Some(list) = param(query, "sends").map(unescape) {
        let mut sends = crate::routing::Sends::new();
        for item in list.split(',').filter(|item| !item.trim().is_empty()) {
            match item.rsplit_once(':').map(|(group, db)| (group.trim(), db.trim().parse::<f64>())) {
                Some((group, Ok(db))) if !group.is_empty() && db.is_finite() => {
                    sends.insert(group.to_string(), db);
                }
                _ => return bad_request("sends must read group:dB,group:dB"),
            }
        }
        RouteChange::Replace(sends)
    } else {
        match (group, param(query, "off") == Some("1")) {
            (Some(group), true) => RouteChange::Unsend(group),
            (Some(group), false) => match param(query, "level_db").map(|v| v.parse::<f64>()) {
                None => RouteChange::Send(group, 0.0),
                Some(Ok(db)) if db.is_finite() => RouteChange::Send(group, db),
                Some(_) => return bad_request("level_db must be a number of dB"),
            },
            (None, _) => return bad_request("name a speaker group (speakers=) or follow=1"),
        }
    };
    match state.route(manual, stop, change) {
        Ok(()) => json(routing_json(&state)),
        Err(err) => bad_request(&err),
    }
}

/// `POST /api/speakers?name=<group>&left=<ch>&right=<ch>` defines or
/// moves a speaker group (1-based channels); `&remove=1` removes it.
pub(super) fn speakers(state: &Mutex<State>, query: &str) -> Reply {
    let Some(name) = param(query, "name").map(unescape) else {
        return bad_request("missing name");
    };
    let output = if param(query, "remove") == Some("1") {
        None
    } else {
        let channel = |key: &str| param(query, key).and_then(|v| v.parse::<u8>().ok());
        match (channel("left"), channel("right")) {
            (Some(left), Some(right)) => Some([left, right]),
            _ => return bad_request("left and right must be channel numbers"),
        }
    };
    let mut state = state.lock().expect("state poisoned");
    match state.set_speaker(&name, output) {
        Ok(()) => json(routing_json(&state)),
        Err(err) => bad_request(&err),
    }
}
