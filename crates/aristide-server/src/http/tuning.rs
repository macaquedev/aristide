//! Tuning, at every scope the cascade knows: the instrument, a set,
//! a division, a stop, a rank.

use std::sync::Mutex;

use aristide_engine::Command;

use super::{bad_request, json, param, state_json, unescape, Reply};
use crate::console::Console;
use crate::State;

pub(super) fn set(state: &Mutex<State>, query: &str) -> Reply {
    {
        let mut state = state.lock().expect("state poisoned");
        // A whole-instrument commit writes the file's top-level
        // [tuning]; mid-rebuild the file is about to be replaced
        // out from under that write, so refuse exactly as the
        // organ-pane editor's own file-writing edits do.
        if state.is_loading() {
            return bad_request("an organ is already loading");
        }
        // The scope: `stop` (+ `rank` for one rank within it),
        // `source` (a set, by alias), `manual` (a division), or
        // none for the instrument. A scope other than the
        // instrument starts from what it effectively plays now
        // and takes a tuning of its own; `follow=` instead
        // names what a stop follows (auto | division | source |
        // organ), `follow=own` its own tuning, and `reset=1`
        // (or `follow=organ` for a set, `follow=stop` for a
        // rank) returns a scope to what it would follow.
        let manual = param(query, "manual").and_then(|v| v.parse::<usize>().ok());
        let source = param(query, "source").map(unescape);
        let stop = param(query, "stop")
            .and_then(|v| v.parse::<u32>().ok())
            .map(aristide_model::StopId);
        let rank = param(query, "rank")
            .and_then(|v| v.parse::<u32>().ok())
            .map(aristide_model::RankId);
        let follow = param(query, "follow").map(unescape);
        let reset = param(query, "reset") == Some("1");
        // Scale files load now, against the organ's own
        // directory, so a bad path answers this request instead
        // of warning into the void.
        let scale_base = state
            .composite_path
            .as_deref()
            .and_then(std::path::Path::parent)
            .map(std::path::Path::to_path_buf);
        let patched = |mut tuning: crate::tuning::Tuning| -> Result<_, String> {
            // Naming `original` is asking for the organ as
            // recorded: the reference returns to the organ's
            // own pitch on its key unless this same request
            // pins it. A target keeps whatever reference the
            // player had — changing the temperament never
            // jumps the pitch a semitone on its own.
            let anchor_given = ["a4", "reference_key", "reference_hz"]
                .iter()
                .any(|field| param(query, field).is_some());
            if let Some(t) =
                param(query, "temperament").and_then(crate::tuning::Temperament::parse)
            {
                tuning.temperament = t;
                // Naming a temperament is leaving the scale,
                // and temperaments are twelve-class vocabulary.
                tuning.scale = None;
                tuning.edo = 12;
                if !tuning.corrects_pipes() && !anchor_given {
                    tuning.reference = tuning.home_reference(tuning.reference.key);
                }
            }
            if let Some(edo) = param(query, "edo").and_then(|v| v.parse::<u16>().ok()) {
                if !crate::tuning::EDO_RANGE.contains(&edo) {
                    return Err(format!(
                        "edo must be {}..{}",
                        crate::tuning::EDO_RANGE.start(),
                        crate::tuning::EDO_RANGE.end()
                    ));
                }
                // Choosing a division count is leaving the
                // scale, the same way naming a temperament is.
                tuning.edo = edo;
                tuning.scale = None;
            }
            // The anchor: `reference_key` (a note name or MIDI
            // number) and `reference_hz`, either alone keeping
            // the other; `a4=` is the older single-field form
            // and means an A4 anchor.
            // `home` for either Hz field puts the key back on
            // what the recording sounds there.
            if let Some(a4) = param(query, "a4") {
                tuning.reference = match a4.parse::<f64>() {
                    Ok(hz) => crate::tuning::PitchReference { key: 69, hz },
                    Err(_) if a4 == "home" => tuning.home_reference(69),
                    Err(_) => return Err(format!("a4 {a4:?} is not a pitch")),
                };
            }
            if let Some(spec) = param(query, "reference_key").map(unescape) {
                tuning.reference.key = parse_reference_key(&spec)
                    .ok_or_else(|| format!("reference_key {spec:?} names no key"))?;
            }
            if let Some(hz) = param(query, "reference_hz") {
                tuning.reference.hz = match hz.parse::<f64>() {
                    Ok(hz) => hz,
                    Err(_) if hz == "home" => {
                        tuning.home_reference(tuning.reference.key).hz
                    }
                    Err(_) => return Err(format!("reference_hz {hz:?} is not a pitch")),
                };
            }
            tuning.reference = tuning.reference.clamped();
            if let Some(t) = param(query, "transpose").and_then(|v| v.parse::<i8>().ok())
            {
                tuning.transpose = t.clamp(-12, 12);
            }
            if let Some(mode) = param(query, "pipes") {
                tuning.pipes = crate::tuning::PipeRetune::parse(mode)
                    .ok_or_else(|| format!("pipes {mode:?} is neither original nor exact"))?;
            }
            match param(query, "scale").map(unescape) {
                Some(scl) if scl.is_empty() || scl == "off" => tuning.scale = None,
                Some(scl) => {
                    let kbm = param(query, "keymap").map(unescape);
                    let scale = crate::tuning::ScaleTuning::load(
                        &scl,
                        kbm.as_deref().filter(|kbm| !kbm.is_empty()),
                        tuning.reference,
                        scale_base.as_deref(),
                    )?;
                    tuning.scale = Some(std::sync::Arc::new(scale));
                }
                None => {}
            }
            // An a′ change re-anchors a linear-mapped scale.
            tuning.refresh_scale_reference();
            Ok(tuning)
        };
        match (stop, source, manual) {
            (Some(stop), _, _) => {
                let Some(console) = state.console() else {
                    return bad_request("no organ is loaded");
                };
                if let Some(rank) = rank {
                    let back = reset || follow.as_deref() == Some("stop");
                    let current = console
                        .rank_tuning(stop, rank)
                        .unwrap_or_else(|| console.stop_tuning_resolved(stop).0.clone());
                    let tuning = match (!back).then(|| patched(current)).transpose() {
                        Ok(tuning) => tuning,
                        Err(err) => return bad_request(&err),
                    };
                    if let Err(err) = state.tune_rank(stop, rank, tuning) {
                        return bad_request(&err);
                    }
                } else {
                    let change = match follow.as_deref() {
                        Some("own") | None if !reset => {
                            let current = console
                                .stop_own_tuning(stop)
                                .unwrap_or_else(|| console.stop_tuning_resolved(stop).0.clone());
                            match patched(current) {
                                Ok(tuning) => Err(tuning),
                                Err(err) => return bad_request(&err),
                            }
                        }
                        None => Ok(crate::tuning::Follow::Auto),
                        Some(name) => match crate::tuning::Follow::parse(name) {
                            Some(follow) => Ok(follow),
                            None => {
                                return bad_request(
                                    "follow must be auto, division, source, organ or own",
                                )
                            }
                        },
                    };
                    if let Err(err) = state.tune_stop(stop, change) {
                        return bad_request(&err);
                    }
                }
            }
            (None, Some(alias), _) => {
                let Some(console) = state.console() else {
                    return bad_request("no organ is loaded");
                };
                let back = reset || follow.as_deref() == Some("organ");
                let current = console.source_tuning(&alias).unwrap_or(console.tuning());
                let tuning = match (!back).then(|| patched(current)).transpose() {
                    Ok(tuning) => tuning,
                    Err(err) => return bad_request(&err),
                };
                if let Err(err) = state.tune_source(&alias, tuning) {
                    return bad_request(&err);
                }
            }
            (None, None, Some(manual)) => {
                let reset = reset || follow.as_deref() == Some("organ");
                let current = state
                    .console()
                    .map(|console| console.manual_tuning(manual).unwrap_or(console.tuning()));
                if let Some(current) = current {
                    let tuning = match (!reset).then(|| patched(current)).transpose() {
                        Ok(tuning) => tuning,
                        Err(err) => return bad_request(&err),
                    };
                    state.tune_manual(manual, tuning);
                }
            }
            (None, None, None) => {
                if let Some(console) = state.console_mut() {
                    match patched(console.tuning()) {
                        Ok(tuning) => console.set_tuning(tuning),
                        Err(err) => return bad_request(&err),
                    }
                }
                // Discrete field commits, not slider drags —
                // every successful whole-instrument change is
                // worth a write, no persist flag needed.
                state.persist_tuning();
            }
        }
        // Live drift: the change lands on sounding voices as a
        // glide (`glide` in ms — 150 is a discreet slide, tens
        // of seconds a performed drift), not just future notes.
        let glide_ms = param(query, "glide")
            .and_then(|v| v.parse::<f32>().ok())
            .unwrap_or(150.0)
            .clamp(0.0, 60_000.0);
        let retuned = state.console_mut().map(Console::retune_held).unwrap_or_default();
        for (handle, rate) in retuned {
            state.engine.send(Command::SetVoiceRate {
                handle,
                rate,
                glide_ms,
            });
        }
    }
    json(state_json(state))
}

/// A tuning anchor's key as the API takes it: a scientific-pitch name
/// ("C4", "F#3") or a bare MIDI number.
fn parse_reference_key(spec: &str) -> Option<u8> {
    let spec = spec.trim();
    spec.parse::<u8>()
        .ok()
        .filter(|&key| key <= 127)
        .or_else(|| aristide_formats::sidecar::parse_note_name(spec))
}
