//! The room and the wind: reverb, control noises, bus delays,
//! tremulants, swell shoes and the master gain.

use std::sync::Mutex;

use aristide_engine::Command;
use aristide_model::units::cents_to_ratio;

use super::{bad_request, json, param, Reply};
use super::snapshot::{state_json, state_json_locked};
use crate::State;

pub(super) fn noises(state: &Mutex<State>, query: &str) -> Reply {
    {
        let mut state = state.lock().expect("state poisoned");
        let persist = param(query, "persist") == Some("1");
        // A persist writes the organ's file; mid-rebuild that
        // file is about to be replaced out from under the
        // write, so refuse like every file-writing edit does.
        // A live-only change stays welcome throughout.
        if persist && state.is_loading() {
            return bad_request("an organ is already loading");
        }
        let composite_path = state.composite_path.clone();
        let State {
            engine, control, ..
        } = &mut *state;
        if let Some(console) = control.organ_mut() {
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
            // The clamp lives in set_noises; read it back so the
            // file never disagrees with what is actually sounding.
            let (enabled, volume) = console.noises();
            if persist
                && let Some(path) = composite_path
                && let Err(err) =
                    crate::config::write_composite_noises(&path, enabled, volume as f64)
            {
                tracing::warn!("noises not saved: {err}");
            }
        }
    }
    json(state_json(state))
}

// Live bus control, stateless pass-through to the engine: the
// whole delay node re-configures at once (ms required; feedback
// 0, mix 1, dry 1 by default), and/or the output pair moves.
// The sidecar's [routing] is the durable home; this is the
// performance knob.
pub(super) fn bus(state: &Mutex<State>, query: &str) -> Reply {
    let Some(bus) = param(query, "bus")
        .and_then(|v| v.parse::<u8>().ok())
        .filter(|b| (*b as usize) < aristide_engine::routing::MAX_BUSES)
    else {
        return bad_request("bus must be 0-7");
    };
    let number = |name: &str, default: f32| {
        param(query, name)
            .and_then(|v| v.parse::<f32>().ok())
            .unwrap_or(default)
    };
    {
        let mut state = state.lock().expect("state poisoned");
        if let Some(ms) = param(query, "ms").and_then(|v| v.parse::<f32>().ok()) {
            state.engine.send(Command::SetBusDelay {
                bus,
                params: aristide_engine::routing::DelayParams {
                    seconds: (ms / 1000.0).max(0.0),
                    feedback: number("feedback", 0.0),
                    mix: number("mix", 1.0),
                    dry: number("dry", 1.0),
                },
            });
        }
        if let (Some(left), Some(right)) = (
            param(query, "left").and_then(|v| v.parse::<u8>().ok()),
            param(query, "right").and_then(|v| v.parse::<u8>().ok()),
        ) {
            state.engine.send(Command::SetBusOutput {
                bus,
                left: left.saturating_sub(1),
                right: right.saturating_sub(1),
                gain: number("gain", 1.0),
            });
        }
    }
    json(state_json(state))
}

pub(super) fn reverb(state: &Mutex<State>, query: &str) -> Reply {
    match param(query, "wet").and_then(|v| v.parse::<f32>().ok()) {
        Some(wet) if (0.0..=2.0).contains(&wet) => {
            {
                let mut state = state.lock().expect("state poisoned");
                // Same file-writing guard as /api/noises: a
                // persist mid-rebuild is refused, a live-only
                // change is not.
                if param(query, "persist") == Some("1")
                    && state.is_loading()
                {
                    return bad_request("an organ is already loading");
                }
                if state.reverb_wet.is_some() {
                    state.reverb_wet = Some(wet);
                    state.engine.send(Command::SetReverbWet { wet });
                    if param(query, "persist") == Some("1")
                        && let Some(path) = state.composite_path.clone()
                        && let Err(err) =
                            crate::config::write_composite_reverb_wet(&path, wet as f64)
                    {
                        tracing::warn!("reverb wet not saved: {err}");
                    }
                }
            }
            json(state_json(state))
        }
        _ => bad_request("bad wet"),
    }
}

pub(super) fn trem(state: &Mutex<State>, query: &str) -> Reply {
    let on = param(query, "on") == Some("1");
    let index = param(query, "idx").and_then(|v| v.parse::<usize>().ok());
    let mut state = state.lock().expect("state poisoned");
    match index {
        Some(index) => state.set_tremulant_at(index, on),
        None => state.set_tremulant(on),
    }
    json(state_json_locked(&state))
}

// Reshape a synth tremulant, live: rate in Hz, depth in pitch
// cents, ramp in seconds, wobble (irregularity) in percent.
// Fields given override the current shape; `idx` defaults to
// the first shapeable (non-wave) tremulant.
pub(super) fn trem_params(state: &Mutex<State>, query: &str) -> Reply {
    let mut state = state.lock().expect("state poisoned");
    let index = match param(query, "idx").and_then(|v| v.parse::<usize>().ok()) {
        Some(index) => index,
        None => match state.trems.iter().position(|t| !t.wave) {
            Some(index) => index,
            None => return bad_request("this organ has no shapeable tremulant"),
        },
    };
    let Some(trem) = state.trems.get(index) else {
        return bad_request("no such tremulant");
    };
    let mut params = trem.params;
    if let Some(rate) = param(query, "rate").and_then(|v| v.parse::<f32>().ok()) {
        params.rate_hz = rate.clamp(0.5, 12.0);
    }
    if let Some(cents) = param(query, "depth").and_then(|v| v.parse::<f64>().ok()) {
        let kp =
            aristide_engine::wind::WindParams::default().pitch_exponent as f64;
        params.depth =
            (cents_to_ratio(cents.clamp(0.0, 30.0) / kp) - 1.0) as f32;
    }
    if let Some(ramp) = param(query, "ramp").and_then(|v| v.parse::<f32>().ok()) {
        params.ramp_seconds = ramp.clamp(0.05, 3.0);
    }
    if let Some(pct) = param(query, "wobble").and_then(|v| v.parse::<f32>().ok()) {
        params.wobble = (pct / 100.0).clamp(0.0, 0.25);
    }
    match state.set_tremulant_shape(index, params) {
        Ok(()) => json(state_json_locked(&state)),
        Err(err) => bad_request(&err),
    }
}

pub(super) fn enclosure(state: &Mutex<State>, query: &str) -> Reply {
    let index = param(query, "idx").and_then(|v| v.parse::<usize>().ok());
    let value = param(query, "v").and_then(|v| v.parse::<f32>().ok());
    match (index, value) {
        (Some(index), Some(value)) if (0.0..=1.0).contains(&value) => {
            {
                let mut state = state.lock().expect("state poisoned");
                let State {
                    engine, control, ..
                } = &mut *state;
                if let Some(console) = control.organ_mut()
                    && let Some((enclosure, position)) = console.set_enclosure(index, value)
                {
                    engine.send(Command::SetEnclosurePosition {
                        enclosure,
                        position,
                    });
                }
            }
            json(state_json(state))
        }
        _ => bad_request("bad enclosure move"),
    }
}

pub(super) fn gain(state: &Mutex<State>, query: &str) -> Reply {
    match param(query, "v").and_then(|v| v.parse::<f32>().ok()) {
        Some(v) if (0.0..=2.0).contains(&v) => {
            let mut state = state.lock().expect("state poisoned");
            state.master_gain = v;
            state.engine.send(Command::SetMasterGain { linear: v });
            json(state_json_locked(&state))
        }
        _ => bad_request("bad gain"),
    }
}
