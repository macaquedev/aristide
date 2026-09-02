//! Loading and saving instruments, the library and file browser,
//! manuals, enclosures and where panels sit on the canvas.

use std::sync::Mutex;

use super::{bad_request, json, json_string, param, params, unescape, Reply};
use super::snapshot::state_json_locked;
use crate::State;

// Declare a manual's compass (both low and high, MIDI notes),
// or with neither given go back to the set's own. Live at
// once, and saved into the organ's file when it has one.
pub(super) fn compass(state: &Mutex<State>, query: &str) -> Reply {
    let mut state = state.lock().expect("state poisoned");
    let Some(manual) = param(query, "manual").and_then(|v| v.parse::<usize>().ok())
    else {
        return bad_request("missing manual");
    };
    let compass = match (
        param(query, "low").map(|v| v.parse::<u8>()),
        param(query, "high").map(|v| v.parse::<u8>()),
    ) {
        (Some(Ok(low)), Some(Ok(high))) if low <= high && high < 128 => {
            Some((low, high))
        }
        (None, None) => None,
        _ => return bad_request("low and high must be MIDI notes, low first"),
    };
    if state.set_compass_override(manual, compass) {
        json(state_json_locked(&state))
    } else {
        bad_request("no such manual")
    }
}

// ---- organ-pane editor --------------------------------------
//
// Structural edits: each writes its line into the organ's own
// file, then reloads the file. Edits that trigger a rebuild are
// refused while a load is already in flight.
pub(super) fn manual_add(state: &Mutex<State>, query: &str) -> Reply {
    let Some(name) = param(query, "name").map(unescape) else {
        return bad_request("missing name");
    };
    // `kind` names the keyboard type; `pedal=1` stays as the
    // older spelling of `kind=pedal`.
    let kind = match param(query, "kind") {
        Some(text) => match aristide_model::ManualKind::parse(text) {
            Some(kind) => kind,
            None => return bad_request("kind must be manual, pedal or microtonal"),
        },
        None if param(query, "pedal").is_some_and(|v| v != "0") => {
            aristide_model::ManualKind::Pedal
        }
        None => aristide_model::ManualKind::Manual,
    };
    let low = match param(query, "low").map(|v| v.parse::<u8>()) {
        Some(Ok(low)) if low < 128 => low,
        None => 36,
        _ => return bad_request("low must be a MIDI note"),
    };
    let high = match param(query, "high").map(|v| v.parse::<u8>()) {
        Some(Ok(high)) if high < 128 => high,
        None => {
            if kind == aristide_model::ManualKind::Pedal {
                67
            } else {
                96
            }
        }
        _ => return bad_request("high must be a MIDI note"),
    };
    if low > high {
        return bad_request("low is above high");
    }
    let mut state = state.lock().expect("state poisoned");
    if state.is_loading() {
        return bad_request("an organ is already loading");
    }
    match state.add_manual(&name, low, high, kind) {
        Ok(()) => json(state_json_locked(&state)),
        Err(err) => bad_request(&err),
    }
}

pub(super) fn manual_kind(state: &Mutex<State>, query: &str) -> Reply {
    let mut state = state.lock().expect("state poisoned");
    if state.is_loading() {
        return bad_request("an organ is already loading");
    }
    match (
        param(query, "manual").and_then(|v| v.parse::<usize>().ok()),
        param(query, "kind").and_then(aristide_model::ManualKind::parse),
    ) {
        (Some(manual), Some(kind)) => match state.set_manual_kind(manual, kind) {
            Ok(()) => json(state_json_locked(&state)),
            Err(err) => bad_request(&err),
        },
        _ => bad_request("missing manual/kind (manual, pedal or microtonal)"),
    }
}

// A microtonal manual's hex-field layout. Fields given override
// the current effective layout; `preset=` fills the two
// step-vectors from a named classic layout, derived against the
// manual's own steps-per-octave; `reset=1` removes the
// declaration so the derived default returns.
pub(super) fn manual_hex(state: &Mutex<State>, query: &str) -> Reply {
    let mut state = state.lock().expect("state poisoned");
    if state.is_loading() {
        return bad_request("an organ is already loading");
    }
    let Some(manual) = param(query, "manual").and_then(|v| v.parse::<usize>().ok())
    else {
        return bad_request("missing manual");
    };
    if param(query, "reset").is_some_and(|v| v != "0") {
        return match state.set_manual_hex(manual, None) {
            Ok(()) => json(state_json_locked(&state)),
            Err(err) => bad_request(&err),
        };
    }
    let Some(console) = state.console() else {
        return bad_request("no organ");
    };
    let Some(mut layout) = console.manual_hex(manual) else {
        return bad_request("not a microtonal manual — no hex field to lay out");
    };
    let compass_top = console
        .manual_states()
        .get(manual)
        .map(|(_, _, first, count, _)| *first as i32 + (*count).max(1) as i32 - 1);
    let mut refit_cols = false;
    if let Some(preset) = param(query, "preset") {
        let steps = console
            .manual_tuning(manual)
            .unwrap_or(console.tuning())
            .steps_per_octave();
        let Some((right, upright)) =
            aristide_model::HexLayout::preset_steps(preset, steps)
        else {
            return bad_request(
                "preset must be bosanquet, wicki-hayden or harmonic-table",
            );
        };
        layout.right = right;
        layout.upright = upright;
        // New step-vectors change how far the board reaches, so
        // its width is refitted unless this call pins it.
        refit_cols = true;
    }
    for (name, slot) in [("right", &mut layout.right), ("upright", &mut layout.upright)]
    {
        if let Some(text) = param(query, name) {
            match text.parse::<i16>() {
                Ok(value) => {
                    *slot = value;
                    refit_cols = true;
                }
                Err(_) => return bad_request(&format!("{name} must be a whole number")),
            }
        }
    }
    if let Some(text) = param(query, "anchor") {
        match text.parse::<u16>() {
            Ok(value) => layout.anchor = value,
            Err(_) => return bad_request("anchor must be a key number"),
        }
    }
    if let Some(text) = param(query, "rows") {
        match text.parse::<i64>() {
            Ok(value) if (1..=aristide_model::HexLayout::MAX_ROWS as i64).contains(&value) => {
                layout.rows = value as u8;
            }
            _ => {
                return bad_request(&format!(
                    "rows must be 1..{}",
                    aristide_model::HexLayout::MAX_ROWS
                ));
            }
        }
    }
    match param(query, "cols") {
        Some(text) => match text.parse::<i64>() {
            Ok(value)
                if (1..=aristide_model::HexLayout::MAX_COLS as i64).contains(&value) =>
            {
                layout.cols = value as u8;
            }
            _ => {
                return bad_request(&format!(
                    "cols must be 1..{}",
                    aristide_model::HexLayout::MAX_COLS
                ));
            }
        },
        None => {
            if refit_cols && let Some(top) = compass_top {
                layout.cols = 1;
                layout.fit_cols(top);
            }
        }
    }
    match state.set_manual_hex(manual, Some(layout)) {
        Ok(()) => json(state_json_locked(&state)),
        Err(err) => bad_request(&err),
    }
}

pub(super) fn manual_rename(state: &Mutex<State>, query: &str) -> Reply {
    let mut state = state.lock().expect("state poisoned");
    if state.is_loading() {
        return bad_request("an organ is already loading");
    }
    match (
        param(query, "manual").and_then(|v| v.parse::<usize>().ok()),
        param(query, "name").map(unescape),
    ) {
        (Some(manual), Some(name)) => match state.rename_manual(manual, &name) {
            Ok(()) => json(state_json_locked(&state)),
            Err(err) => bad_request(&err),
        },
        _ => bad_request("missing manual/name"),
    }
}

pub(super) fn manual_remove(state: &Mutex<State>, query: &str) -> Reply {
    let mut state = state.lock().expect("state poisoned");
    if state.is_loading() {
        return bad_request("an organ is already loading");
    }
    match param(query, "manual").and_then(|v| v.parse::<usize>().ok()) {
        Some(manual) => match state.remove_manual(manual) {
            Ok(()) => json(state_json_locked(&state)),
            Err(err) => bad_request(&err),
        },
        None => bad_request("missing manual"),
    }
}

pub(super) fn manual_order(state: &Mutex<State>, query: &str) -> Reply {
    let mut state = state.lock().expect("state poisoned");
    if state.is_loading() {
        return bad_request("an organ is already loading");
    }
    match (
        param(query, "manual").and_then(|v| v.parse::<usize>().ok()),
        param(query, "to").and_then(|v| v.parse::<usize>().ok()),
    ) {
        (Some(manual), Some(to)) => match state.reorder_manual(manual, to) {
            Ok(()) => json(state_json_locked(&state)),
            Err(err) => bad_request(&err),
        },
        _ => bad_request("missing manual/to"),
    }
}

pub(super) fn source_add(state: &Mutex<State>, query: &str) -> Reply {
    let Some(path) = param(query, "path").map(unescape) else {
        return bad_request("missing path");
    };
    let mut state = state.lock().expect("state poisoned");
    match state.add_organ_source(std::path::Path::new(&path)) {
        Ok(_) => json(state_json_locked(&state)),
        Err(err) => bad_request(&err),
    }
}

pub(super) fn enclosure_add(state: &Mutex<State>, query: &str) -> Reply {
    let Some(name) = param(query, "name").map(unescape) else {
        return bad_request("missing name");
    };
    let mut state = state.lock().expect("state poisoned");
    if state.is_loading() {
        return bad_request("an organ is already loading");
    }
    match state.add_enclosure(&name) {
        Ok(()) => json(state_json_locked(&state)),
        Err(err) => bad_request(&err),
    }
}

pub(super) fn enclosure_remove(state: &Mutex<State>, query: &str) -> Reply {
    let Some(name) = param(query, "name").map(unescape) else {
        return bad_request("missing name");
    };
    let mut state = state.lock().expect("state poisoned");
    if state.is_loading() {
        return bad_request("an organ is already loading");
    }
    match state.remove_enclosure(&name) {
        Ok(()) => json(state_json_locked(&state)),
        Err(err) => bad_request(&err),
    }
}

// Put a stop in a swell box (`in=1`) or take it out (`in=0`).
pub(super) fn enclosure_assign(state: &Mutex<State>, query: &str) -> Reply {
    let mut state = state.lock().expect("state poisoned");
    if state.is_loading() {
        return bad_request("an organ is already loading");
    }
    match (
        param(query, "enclosure").map(unescape),
        param(query, "stop").and_then(|v| v.parse::<u32>().ok()),
        param(query, "in").map(|v| v != "0"),
    ) {
        (Some(enclosure), Some(stop), Some(inside)) => {
            match state.assign_enclosure(&enclosure, aristide_model::StopId(stop), inside)
            {
                Ok(()) => json(state_json_locked(&state)),
                Err(err) => bad_request(&err),
            }
        }
        _ => bad_request("missing enclosure/stop/in"),
    }
}

// Move — and with `w`/`h`, size — a console panel on the
// canvas: all four are normalized fractions, clamped rather
// than refused (a sized jamb wraps its stops into columns).
// Size left out keeps whatever the panel has on record.
// Cosmetic — this writes the file but, unlike the edits above,
// never queues a rebuild.
pub(super) fn panel_place(state: &Mutex<State>, query: &str) -> Reply {
    let mut state = state.lock().expect("state poisoned");
    if state.is_loading() {
        return bad_request("an organ is already loading");
    }
    let size = match (
        param(query, "w").map(|v| v.parse::<f32>()),
        param(query, "h").map(|v| v.parse::<f32>()),
    ) {
        (Some(Ok(w)), Some(Ok(h))) if w.is_finite() && h.is_finite() => {
            Some((w, h))
        }
        (None, None) => None,
        _ => return bad_request("w and h must be fractions, both or neither"),
    };
    match (
        param(query, "panel").map(unescape),
        param(query, "x").and_then(|v| v.parse::<f32>().ok()),
        param(query, "y").and_then(|v| v.parse::<f32>().ok()),
    ) {
        (Some(panel), Some(x), Some(y)) => {
            match state.place_panel(&panel, x, y, size) {
                Ok(()) => json(state_json_locked(&state)),
                Err(err) => bad_request(&err),
            }
        }
        _ => bad_request("missing panel/x/y"),
    }
}

// What every source of this organ offers, for the pane's
// source browser: manuals, stops, and what is already pulled.
// Sources are parsed on demand (an ODF parse, no samples).
pub(super) fn offerings(state: &Mutex<State>, _query: &str) -> Reply {
    let path = {
        let state = state.lock().expect("state poisoned");
        state.composite_path.clone()
    };
    let Some(path) = path else {
        return bad_request("this organ has no file yet");
    };
    match offerings_json(&path) {
        Ok(body) => json(body),
        Err(err) => bad_request(&err),
    }
}

// Load an instrument: one or more paths to `.organ` sets or
// composite `.toml` files. The load itself happens on the main
// thread (it owns the audio stream); this only queues it, and
// the state snapshots narrate progress until the organ appears.
pub(super) fn load(state: &Mutex<State>, query: &str) -> Reply {
    let paths: Vec<std::path::PathBuf> = params(query, "path")
        .map(|value| std::path::PathBuf::from(unescape(value)))
        .collect();
    if paths.is_empty() {
        return bad_request("missing path");
    }
    for path in &paths {
        if !path.is_file() {
            return bad_request(&format!("{}: not a file", path.display()));
        }
    }
    let mut state = state.lock().expect("state poisoned");
    // Last pick wins: while one organ decodes, picking another
    // replaces the queued request instead of being refused — a
    // refusal here surfaces nowhere the player is looking, and
    // "clicked an organ, nothing happened" must not exist.
    state.loading = Some("loading…".to_string());
    state.load_error = None;
    state.load_warnings.clear();
    state.pending_load = Some(crate::LoadRequest {
        paths,
        stops: Vec::new(),
        initial: false,
    });
    json(state_json_locked(&state))
}

// Create a blank composite — nothing but a name — under the
// config directory's organs/, and queue loading it. The player
// grows it from there; the file is theirs to edit or move.
pub(super) fn create(state: &Mutex<State>, query: &str) -> Reply {
    let Some(name) = param(query, "name").map(unescape) else {
        return bad_request("missing name");
    };
    let mut state = state.lock().expect("state poisoned");
    let Some(dir) = crate::config::organs_dir() else {
        return bad_request("no config directory to keep organs in");
    };
    match crate::config::create_blank_organ(&dir, &name) {
        Ok(path) => {
            state.loading = Some("loading…".to_string());
            state.load_error = None;
            state.load_warnings.clear();
            state.pending_load = Some(crate::LoadRequest {
                paths: vec![path],
                stops: Vec::new(),
                initial: false,
            });
            json(state_json_locked(&state))
        }
        Err(err) => bad_request(&err),
    }
}

// Rename the loaded organ in place: the name changes in the
// file that owns it, and everything keyed by it (assignments,
// the library's label) follows; no path changes, so nothing
// that refers to the organ's file breaks.
pub(super) fn rename(state: &Mutex<State>, query: &str) -> Reply {
    match param(query, "name").map(unescape) {
        Some(name) => {
            let mut state = state.lock().expect("state poisoned");
            match state.rename_organ(&name) {
                Ok(()) => json(state_json_locked(&state)),
                Err(err) => bad_request(&err),
            }
        }
        None => bad_request("missing name"),
    }
}

// Take an organ off the picker's Recent list. Nothing else is
// touched: the organ file stays where it is (Browse's organs
// shortcut still reaches it, and loading its set finds it
// again), and its assignments are kept.
pub(super) fn library_forget(state: &Mutex<State>, query: &str) -> Reply {
    match param(query, "path").map(unescape) {
        Some(path) => {
            let mut state = state.lock().expect("state poisoned");
            state.forget_organ(std::path::Path::new(&path));
            json(state_json_locked(&state))
        }
        None => bad_request("missing path"),
    }
}

// The picker's file browser: subdirectories and loadable organ
// files under `dir` (the home directory when absent). The bind
// is localhost-only, which is the access control here as for
// every other endpoint.
pub(super) fn browse(_state: &Mutex<State>, query: &str) -> Reply {
    let dir = param(query, "dir")
        .map(unescape)
        .filter(|dir| !dir.is_empty())
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(std::path::PathBuf::from))
        .unwrap_or_else(|| std::path::PathBuf::from("/"));
    match browse_json(&dir) {
        Ok(body) => json(body),
        Err(err) => bad_request(&err),
    }
}

// Write the loaded combination to a composite organ file —
// from then on that file is the organ, and it owns the wiring.
pub(super) fn save(state: &Mutex<State>, query: &str) -> Reply {
    let mut state = state.lock().expect("state poisoned");
    match param(query, "path").map(unescape) {
        Some(path) if path.ends_with(".toml") => {
            match state.save_composite(std::path::PathBuf::from(path)) {
                Ok(()) => json(state_json_locked(&state)),
                Err(err) => bad_request(&err),
            }
        }
        Some(_) => bad_request("path must end in .toml"),
        None => bad_request("missing path"),
    }
}

// Copy the loaded organ's file under a new name and switch to
// the copy — the way past an adopted organ's refusal to change the
// instrument itself.
pub(super) fn save_as(state: &Mutex<State>, query: &str) -> Reply {
    match param(query, "name").map(unescape) {
        Some(name) => {
            let mut state = state.lock().expect("state poisoned");
            match state.save_organ_as(&name) {
                Ok(()) => json(state_json_locked(&state)),
                Err(err) => bad_request(&err),
            }
        }
        None => bad_request("missing name"),
    }
}

/// One directory as the picker's browser shows it: subdirectories and
/// loadable organ files (`.organ` sample sets, `.toml` composites,
/// unencrypted Hauptwerk definitions), dotfiles skipped, directories
/// first.
pub(super) fn browse_json(dir: &std::path::Path) -> Result<String, String> {
    let dir = dir
        .canonicalize()
        .map_err(|err| format!("{}: {err}", dir.display()))?;
    let entries = std::fs::read_dir(&dir).map_err(|err| format!("{}: {err}", dir.display()))?;
    let mut dirs: Vec<String> = Vec::new();
    let mut files: Vec<String> = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        if entry.path().is_dir() {
            dirs.push(name);
        } else {
            let lower = name.to_lowercase();
            // Loadable organs plus Scala tuning files; each picker
            // filters client-side to the extensions it means.
            if lower.ends_with(".organ")
                || lower.ends_with(".toml")
                || lower.ends_with(".organ_hauptwerk_xml")
                || lower.ends_with(".scl")
                || lower.ends_with(".kbm")
            {
                files.push(name);
            }
        }
    }
    let key = |name: &String| name.to_lowercase();
    dirs.sort_by_key(key);
    files.sort_by_key(key);
    // Where the console's own organ files live, so the browser can
    // offer a jump there: the config directory is a dotfile, which
    // this listing (rightly) hides, so without the shortcut an organ
    // taken off Recent would be unreachable.
    let organs = crate::config::organs_dir()
        .filter(|dir| dir.is_dir())
        .map_or_else(
            || "null".to_string(),
            |dir| json_string(&dir.display().to_string()),
        );
    let mut out = format!(
        "{{\"dir\":{},\"parent\":{},\"organs\":{organs},\"entries\":[",
        json_string(&dir.display().to_string()),
        dir.parent().map_or_else(
            || "null".to_string(),
            |parent| json_string(&parent.display().to_string())
        )
    );
    let mut first = true;
    for (name, is_dir) in dirs
        .iter()
        .map(|name| (name, true))
        .chain(files.iter().map(|name| (name, false)))
    {
        if !first {
            out.push(',');
        }
        first = false;
        out.push_str(&format!(
            "{{\"name\":{},\"path\":{},\"dir\":{is_dir}}}",
            json_string(name),
            json_string(&dir.join(name).display().to_string())
        ));
    }
    out.push_str("]}");
    Ok(out)
}

/// What each source of a composite offers, with what the file already
/// pulls marked. Parsing a source is an ODF read, no samples; an
/// unreadable source reports its error instead of hiding the rest.
pub(super) fn offerings_json(path: &std::path::Path) -> Result<String, String> {
    let text =
        std::fs::read_to_string(path).map_err(|err| format!("{}: {err}", path.display()))?;
    let def: aristide_formats::instrument::Definition =
        toml::from_str(&text).map_err(|err| format!("{}: {err}", path.display()))?;
    let dir = path.parent().unwrap_or(std::path::Path::new(""));

    // What the file already pulls, per source alias: whole divisions
    // by source-manual name, single stops by (source manual, name) —
    // the shapes the pane itself writes. Hand-written patterns that
    // aren't exact names may not be recognized; that only marks a stop
    // as still offered, never hides one.
    let division_pulled = |alias: &str, manual: &str| {
        def.divisions
            .iter()
            .any(|pull| pull.from == alias && manual.eq_ignore_ascii_case(&pull.manual))
    };
    let stop_pulled = |alias: &str, manual: &str, stop: &str| {
        def.stops.iter().any(|pull| {
            pull.from == alias
                && stop.eq_ignore_ascii_case(&pull.stop)
                && pull
                    .manual
                    .as_deref()
                    .is_none_or(|pattern| manual.eq_ignore_ascii_case(pattern))
        })
    };

    let mut out = String::from("{\"sources\":[");
    let mut first_source = true;
    for (alias, source) in &def.sources {
        if !first_source {
            out.push(',');
        }
        first_source = false;
        let source_path = source.path();
        let resolved = if source_path.is_absolute() {
            source_path.to_path_buf()
        } else {
            dir.join(source_path)
        };
        out.push_str(&format!(
            "{{\"alias\":{},\"path\":{}",
            json_string(alias),
            json_string(&resolved.display().to_string())
        ));
        match aristide_formats::load_set(&resolved) {
            Ok(loaded) => {
                let organ = loaded.organ;
                out.push_str(&format!(",\"name\":{},\"manuals\":[", json_string(&organ.name)));
                let mut first_manual = true;
                for manual in &organ.manuals {
                    if !first_manual {
                        out.push(',');
                    }
                    first_manual = false;
                    let whole = division_pulled(alias, &manual.name);
                    out.push_str(&format!(
                        "{{\"name\":{},\"pedal\":{},\"pulled\":{whole},\"stops\":[",
                        json_string(&manual.name),
                        manual.pedal()
                    ));
                    let mut first_stop = true;
                    for stop in organ.stops.iter().filter(|stop| stop.manual == manual.id) {
                        if !first_stop {
                            out.push(',');
                        }
                        first_stop = false;
                        let pulled = whole || stop_pulled(alias, &manual.name, &stop.name);
                        out.push_str(&format!(
                            "{{\"name\":{},\"pulled\":{pulled}}}",
                            json_string(&stop.name)
                        ));
                    }
                    out.push_str("]}");
                }
                out.push(']');
            }
            Err(err) => {
                out.push_str(&format!(",\"error\":{}", json_string(&err.to_string())));
            }
        }
        out.push('}');
    }
    out.push_str("]}");
    Ok(out)
}
