//! Build: one stop's rule — its events, the timestamps they start and
//! end at, and the pipework they can draw on.

use std::sync::Mutex;

use aristide_model::{RankId, StopId};
use serde::Deserialize;
use serde_json::{json, Value};

use super::{bad_request, json, param, unescape, Reply};
use crate::console::Console;
use crate::rule::{Event, Rule, Source, Stamp};
use crate::State;

/// `GET /api/rule?stop=ID`: the stop, its rule and every source.
pub(super) fn get(state: &Mutex<State>, query: &str) -> Reply {
    let Some(stop) = param(query, "stop").and_then(|v| v.parse().ok()).map(StopId) else {
        return bad_request("stop=<id> is required");
    };
    let state = state.lock().expect("state poisoned");
    let Some(console) = state.console() else {
        return bad_request("no organ is loaded");
    };
    match rule_json(console, stop) {
        Some(body) => json(body.to_string()),
        None => bad_request("no such stop"),
    }
}

/// `POST /api/organ/rule?stop=ID&rule=<JSON>`, or `&reset=1` for the
/// conventional stop. Answers with the stop's rule as `GET` does.
pub(super) fn set(state: &Mutex<State>, query: &str) -> Reply {
    let Some(stop) = param(query, "stop").and_then(|v| v.parse().ok()).map(StopId) else {
        return bad_request("stop=<id> is required");
    };
    let next = if param(query, "reset").is_some_and(|v| v == "1") {
        None
    } else {
        let Some(text) = param(query, "rule") else {
            return bad_request("rule=<json> or reset=1 is required");
        };
        match serde_json::from_str::<WireRule>(&unescape(text)) {
            Ok(wire) => Some(wire.into_rule()),
            Err(err) => return bad_request(&format!("rule: {err}")),
        }
    };
    let mut state = state.lock().expect("state poisoned");
    if let Err(why) = state.set_stop_rule(stop, next) {
        return bad_request(&why);
    }
    let Some(console) = state.console() else {
        return bad_request("no organ is loaded");
    };
    match rule_json(console, stop) {
        Some(body) => json(body.to_string()),
        None => bad_request("no such stop"),
    }
}

#[derive(Deserialize)]
struct WireRule {
    stamps: Vec<Stamp>,
    events: Vec<WireEvent>,
}

#[derive(Deserialize)]
struct WireEvent {
    source: WireSource,
    cents: f64,
    level: f64,
    start: String,
    end: Option<String>,
}

#[derive(Deserialize)]
struct WireSource {
    stop: u32,
    rank: Option<u32>,
}

impl WireRule {
    fn into_rule(self) -> Rule {
        Rule {
            stamps: self.stamps,
            events: self
                .events
                .into_iter()
                .map(|event| Event {
                    source: Source {
                        stop: StopId(event.source.stop),
                        rank: event.source.rank.map(RankId),
                    },
                    cents: event.cents,
                    level_db: event.level,
                    start: event.start,
                    end: event.end,
                })
                .collect(),
        }
    }
}

fn rule_json(console: &Console, stop: StopId) -> Option<Value> {
    let stops = console.stop_states();
    let (_, name, manual, midx, _) = stops.iter().find(|(id, ..)| *id == stop)?;
    let rule = console.stop_rule(stop);
    // Every event sounds each rank of its source on every key: what a
    // press on this stop costs in voices against a conventional stop.
    let voices: usize = rule
        .events
        .iter()
        .map(|event| match event.source.rank {
            Some(_) => 1,
            None => console.stop_ranks(event.source.stop).len(),
        })
        .sum();
    let sources: Vec<Value> = stops
        .iter()
        .map(|(id, name, manual, midx, _)| {
            json!({
                "stop": id.0,
                "name": name,
                "manual": manual,
                "midx": midx,
                "ranks": console
                    .stop_ranks(*id)
                    .into_iter()
                    .map(|(rank, name)| json!({ "id": rank.0, "name": name }))
                    .collect::<Vec<_>>(),
            })
        })
        .collect();
    Some(json!({
        "stop": { "id": stop.0, "name": name, "manual": manual, "midx": midx },
        "custom": console.stop_has_rule(stop),
        "voices": voices,
        "stamps": rule.stamps,
        "events": rule
            .events
            .iter()
            .map(|event| json!({
                "source": { "stop": event.source.stop.0, "rank": event.source.rank.map(|r| r.0) },
                "cents": event.cents,
                "level": event.level_db,
                "start": event.start,
                "end": event.end,
            }))
            .collect::<Vec<_>>(),
        "sources": sources,
    }))
}
