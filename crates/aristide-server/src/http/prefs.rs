//! The player's own settings — the user config, never an organ file.
//! Preferences is the one console surface that talks to the server
//! without touching the instrument: what lands here is a fact about
//! this machine.

use std::sync::Mutex;

use super::snapshot::state_json_locked;
use super::{bad_request, json, param, unescape, Reply};
use crate::config::{SamplePrefs, Streaming};
use crate::State;

/// `[samples]`: how this machine holds a set's audio. Every parameter
/// is optional and the rest keep their value; `ram_budget_mb=` (empty)
/// returns the budget to half of physical RAM. Saved at once, applied
/// on the next load.
pub(super) fn samples(state: &Mutex<State>, query: &str) -> Reply {
    let mut state = state.lock().expect("state poisoned");
    let mut prefs: SamplePrefs = state.midi_config.samples.clone();
    if let Some(mode) = param(query, "streaming") {
        match Streaming::parse(&unescape(mode)) {
            Some(mode) => prefs.streaming = mode,
            None => return bad_request("streaming must be auto, on or off"),
        }
    }
    if let Some(budget) = param(query, "ram_budget_mb") {
        let budget = unescape(budget);
        if budget.trim().is_empty() {
            prefs.ram_budget_mb = None;
        } else {
            match budget.trim().parse::<u64>() {
                Ok(mb) if mb > 0 => prefs.ram_budget_mb = Some(mb),
                _ => return bad_request("ram_budget_mb must be a whole number of MiB"),
            }
        }
    }
    if let Some(bits) = param(query, "bits") {
        match bits.trim().parse::<u32>() {
            Ok(bits @ (16 | 32)) => prefs.bits = bits,
            _ => return bad_request("bits must be 16 or 32"),
        }
    }
    if let Some(cache) = param(query, "cache") {
        match cache.trim() {
            "1" | "true" | "on" => prefs.cache = true,
            "0" | "false" | "off" => prefs.cache = false,
            _ => return bad_request("cache must be 0 or 1"),
        }
    }
    state.set_sample_prefs(prefs);
    json(state_json_locked(&state))
}
