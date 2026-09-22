//! Tuning, at every scope the cascade knows: the instrument, a set,
//! a division, a stop, a rank.

use std::sync::Mutex;

use aristide_engine::Command;

use super::{bad_request, json, param, unescape, Reply};
use super::snapshot::{state_json, tuning_scopes_json};
use crate::console::Console;
use crate::State;

/// `GET /api/tuning`: every scope the Tuning panel edits, resolved.
pub(super) fn scopes(state: &Mutex<State>, _query: &str) -> Reply {
    let state = state.lock().expect("state poisoned");
    match tuning_scopes_json(&state) {
        Some(body) => json(body),
        None => bad_request("no organ is loaded"),
    }
}

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
                // Naming a temperament is leaving the scale and
                // any steps collection, and temperaments are
                // twelve-class vocabulary.
                tuning.scale = None;
                tuning.steps = None;
                tuning.edo = 12;
                tuning.period = 1200.0;
                if !tuning.corrects_pipes() && !anchor_given {
                    tuning.reference = tuning.home_reference(tuning.reference.key);
                }
            } else if let Some(name) = param(query, "temperament") {
                return Err(format!("temperament {name:?} names no known temperament"));
            }
            // The root a named temperament is centred on: a pitch-class
            // name ("D", "F#", "Bb") or a bare 0..11 number.
            if let Some(spec) = param(query, "root").map(unescape) {
                tuning.temperament_root = parse_pitch_class(&spec)
                    .ok_or_else(|| format!("root {spec:?} names no pitch class"))?;
            }
            // `offsets=` names 12 comma-separated cent deviations and
            // implies a custom temperament, replacing whichever table
            // (or scale) was playing.
            if let Some(spec) = param(query, "offsets") {
                let values: Result<Vec<f32>, _> =
                    spec.split(',').map(|v| v.trim().parse::<f32>()).collect();
                let values = values.map_err(|_| format!("offsets {spec:?} is not 12 numbers"))?;
                let &[a, b, c, d, e, f, g, h, i, j, k, l] = values.as_slice() else {
                    return Err(format!(
                        "offsets needs exactly 12 comma-separated cents, got {}",
                        values.len()
                    ));
                };
                tuning.temperament = crate::tuning::Temperament::Custom([
                    a, b, c, d, e, f, g, h, i, j, k, l,
                ]);
                tuning.scale = None;
                tuning.steps = None;
                tuning.edo = 12;
                tuning.period = 1200.0;
            }
            if let Some(offset) = param(query, "offset_cents") {
                tuning.offset_cents = offset
                    .parse::<f64>()
                    .map_err(|_| format!("offset_cents {offset:?} is not a number"))?;
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
                // scale and any steps collection, the same way
                // naming a temperament is.
                tuning.edo = edo;
                tuning.scale = None;
                tuning.steps = None;
            }
            // `period=<cents|none>`: under equal division, the
            // repeat interval (a plain number, default 1200); under
            // a steps collection, its own repeat interval (`none`
            // clears it — no repetition).
            // A request naming `steps=` gives its period to the new
            // collection below instead.
            if let Some(spec) = param(query, "period").filter(|_| param(query, "steps").is_none()) {
                if let Some(steps) = &tuning.steps {
                    let mut updated = (**steps).clone();
                    updated.period = match spec {
                        "" | "none" | "off" => None,
                        _ => Some(spec.parse::<f64>().map_err(|_| {
                            format!("period {spec:?} is not a number of cents or \"none\"")
                        })?),
                    };
                    tuning.steps = Some(std::sync::Arc::new(updated));
                } else {
                    let period = spec
                        .parse::<f64>()
                        .map_err(|_| format!("period {spec:?} is not a number of cents"))?;
                    if !(period.is_finite() && period > 0.0) {
                        return Err("period must be a positive number of cents".into());
                    }
                    tuning.period = period;
                }
            }
            // `steps=<comma-separated cents>`: an inline pitch
            // collection, replacing the temperament/division and the
            // scale. Its repeat interval and start key keep whatever
            // this tuning already had (or the defaults) unless this
            // same request also names `period=`/`start_key=`.
            if let Some(spec) = param(query, "steps") {
                let values: Result<Vec<f64>, _> =
                    spec.split(',').map(|v| v.trim().parse::<f64>()).collect();
                let steps =
                    values.map_err(|_| format!("steps {spec:?} is not comma-separated cents"))?;
                if steps.is_empty() {
                    return Err("steps needs at least one value".into());
                }
                // A `period=` in the same request is this collection's;
                // otherwise it keeps the repeat it had (none, if new).
                let period = match param(query, "period") {
                    Some("" | "none" | "off") => None,
                    Some(spec) => Some(spec.parse::<f64>().map_err(|_| {
                        format!("period {spec:?} is not a number of cents or \"none\"")
                    })?),
                    None => tuning.steps.as_ref().and_then(|s| s.period),
                };
                let start_key = tuning.steps.as_ref().map_or(60, |s| s.start_key);
                tuning.steps = Some(std::sync::Arc::new(crate::tuning::StepsTuning {
                    steps,
                    period,
                    start_key,
                }));
                tuning.scale = None;
            }
            if let Some(spec) = param(query, "start_key").map(unescape) {
                let key = parse_reference_key(&spec)
                    .ok_or_else(|| format!("start_key {spec:?} names no key"))?;
                if let Some(steps) = &tuning.steps {
                    let mut updated = (**steps).clone();
                    updated.start_key = key;
                    tuning.steps = Some(std::sync::Arc::new(updated));
                } else if !tuning.set_scale_start(key) {
                    return Err("start_key only applies to steps or a scale without a keymap".into());
                }
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
                    // Naming a Scala file is leaving any steps
                    // collection, same as it leaves the temperament.
                    tuning.steps = None;
                }
                None => {}
            }
            // An a′ change re-anchors a linear-mapped scale.
            tuning.refresh_scale_reference();
            // A non-repeating steps collection has no pitch to give a
            // reference key outside its range — reject rather than
            // silently play nothing.
            if let Some(steps) = &tuning.steps
                && steps.period.is_none()
                && steps.cents_for(tuning.reference.key as u16).is_none()
            {
                return Err(format!(
                    "reference_key {} falls outside the non-repeating steps collection",
                    aristide_formats::sidecar::note_name(tuning.reference.key)
                ));
            }
            Ok(tuning)
        };
        // Which halves the request edits: anchor fields make the scope
        // own its pitch, scale fields its intervals. `own=anchor|scale|
        // both` adopts a half as it stands (audibly a no-op) and
        // `follow=anchor|scale` returns one to the scopes above. A bare
        // request, as before, takes the whole tuning as the scope's own.
        let touches = |fields: &[&str]| fields.iter().any(|field| param(query, field).is_some());
        let anchor_touched = touches(&["a4", "reference_key", "reference_hz", "offset_cents"]);
        let scale_touched = touches(&[
            "temperament", "root", "offsets", "edo", "period", "steps", "start_key", "scale",
            "keymap", "pipes",
        ]);
        let own_param = param(query, "own");
        let part_follow = follow.as_deref().filter(|f| matches!(*f, "anchor" | "scale"));
        let owns_after = |before: crate::tuning::Owns| -> Result<crate::tuning::Owns, String> {
            let mut owns = before;
            owns.anchor |= anchor_touched;
            owns.scale |= scale_touched;
            match own_param {
                Some("anchor") => owns.anchor = true,
                Some("scale") => owns.scale = true,
                Some("both") => owns = crate::tuning::Owns::BOTH,
                Some(other) => return Err(format!("own must be anchor, scale or both, not {other:?}")),
                None => {}
            }
            match part_follow {
                Some("anchor") => owns.anchor = false,
                Some("scale") => owns.scale = false,
                _ => {}
            }
            if !anchor_touched && !scale_touched && own_param.is_none() && part_follow.is_none() {
                owns = crate::tuning::Owns::BOTH;
            }
            Ok(owns)
        };
        // A scope's new own tuning, patched from what it resolves to
        // now, or `None` when it ends up owning neither half.
        let own_tuning = |resolved: crate::tuning::Tuning,
                          before: Option<crate::tuning::Owns>|
         -> Result<Option<crate::tuning::Tuning>, String> {
            let owns = owns_after(before.unwrap_or(crate::tuning::Owns::NONE))?;
            let mut tuning = patched(resolved)?;
            tuning.owns = owns;
            Ok(owns.any().then_some(tuning))
        };
        match (stop, source, manual) {
            (Some(stop), _, _) => {
                let Some(console) = state.console() else {
                    return bad_request("no organ is loaded");
                };
                if let Some(rank) = rank {
                    let back = reset || follow.as_deref() == Some("stop");
                    let before = console.rank_tuning(stop, rank).map(|t| t.owns);
                    let resolved = console.rank_tuning_resolved(stop, rank);
                    let tuning = match (!back).then(|| own_tuning(resolved, before)).transpose() {
                        Ok(tuning) => tuning.flatten(),
                        Err(err) => return bad_request(&err),
                    };
                    if let Err(err) = state.tune_rank(stop, rank, tuning) {
                        return bad_request(&err);
                    }
                } else {
                    let whole_follow = follow.as_deref().filter(|f| !matches!(*f, "anchor" | "scale" | "own"));
                    let change = match whole_follow {
                        None if !reset => {
                            let before = console.stop_own_tuning(stop).map(|t| t.owns);
                            let resolved = console.stop_tuning_resolved(stop).0.into_owned();
                            let pinned = console.stop_follow(stop);
                            match own_tuning(resolved, before) {
                                Ok(Some(tuning)) => Err(tuning),
                                Ok(None) => Ok(pinned),
                                Err(err) => return bad_request(&err),
                            }
                        }
                        None => Ok(crate::tuning::Follow::Auto),
                        Some(name) => match crate::tuning::Follow::parse(name) {
                            Some(follow) => Ok(follow),
                            None => {
                                return bad_request(
                                    "follow must be auto, division, source, organ, own, anchor or scale",
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
                let before = console.source_tuning(&alias).map(|t| t.owns);
                let resolved = console.source_tuning_resolved(&alias);
                let tuning = match (!back).then(|| own_tuning(resolved, before)).transpose() {
                    Ok(tuning) => tuning.flatten(),
                    Err(err) => return bad_request(&err),
                };
                if let Err(err) = state.tune_source(&alias, tuning) {
                    return bad_request(&err);
                }
            }
            (None, None, Some(manual)) => {
                let reset = reset || follow.as_deref() == Some("organ");
                let current = state.console().map(|console| {
                    (console.manual_tuning(manual), console.manual_tuning_resolved(manual))
                });
                if let Some((own, mut resolved)) = current {
                    let before = own.as_ref().map(|t| t.owns);
                    if let Some(own) = &own {
                        resolved.transpose = own.transpose;
                    }
                    let tuning = if reset {
                        None
                    } else {
                        // A division's transposer lives with its tuning,
                        // so a division owning neither half still keeps
                        // one while it transposes.
                        let owns = match owns_after(before.unwrap_or(crate::tuning::Owns::NONE)) {
                            Ok(owns) => owns,
                            Err(err) => return bad_request(&err),
                        };
                        let mut tuning = match patched(resolved) {
                            Ok(tuning) => tuning,
                            Err(err) => return bad_request(&err),
                        };
                        tuning.owns = owns;
                        (owns.any() || tuning.transpose != 0).then_some(tuning)
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

/// A temperament root as the API takes it: a pitch-class name ("D",
/// "F#", "Bb") or a bare 0..11 number.
fn parse_pitch_class(spec: &str) -> Option<u8> {
    let spec = spec.trim();
    spec.parse::<u8>()
        .ok()
        .filter(|&pc| pc <= 11)
        .or_else(|| aristide_formats::sidecar::parse_pitch_class(spec))
}
