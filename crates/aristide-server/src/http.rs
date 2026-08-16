//! A deliberately small local web console: draw/retire stops, toggle
//! the tremulant, set master gain. Serves one embedded page plus a
//! JSON state endpoint on localhost.
//!
//! This is a stopgap until the real IPC control plane + native GUI
//! (M5); it exists so registration changes don't need a restart. It
//! runs on its own thread and talks to the engine exactly like MIDI
//! does: lock the shared state, send commands.

use std::sync::{Arc, Mutex};

use aristide_engine::Command;
use tiny_http::{Header, Method, Response, Server};

use crate::{Control, State};

const PAGE: &str = include_str!("console.html");

pub fn spawn(state: Arc<Mutex<State>>, port: u16) -> std::io::Result<()> {
    let server = Server::http(("127.0.0.1", port))
        .map_err(|e| std::io::Error::other(format!("http bind: {e}")))?;
    tracing::info!("console ui: http://127.0.0.1:{port}/");
    std::thread::Builder::new()
        .name("aristide-http".into())
        .spawn(move || {
            for request in server.incoming_requests() {
                // Log every non-poll request so phantom traffic (e.g. a
                // client sending note-ons nobody asked for) shows up in
                // the server log with a timestamp. /api/state is the UI's
                // steady poll and would drown everything else out.
                if request.url() != "/api/state" {
                    tracing::info!("http {} {}", request.method(), request.url());
                }
                // Consoles are web pages (Tauri webview, plain browser);
                // they fetch this API cross-origin, so every response
                // carries the permissive CORS header. The bind stays
                // localhost-only, which is the actual access control.
                let response = respond(&state, request.method(), request.url())
                    .with_header(
                        Header::from_bytes("Access-Control-Allow-Origin", "*")
                            .expect("valid header"),
                    );
                let _ = request.respond(response);
            }
        })?;
    Ok(())
}

fn respond(
    state: &Mutex<State>,
    method: &Method,
    url: &str,
) -> Response<std::io::Cursor<Vec<u8>>> {
    let (path, query) = url.split_once('?').unwrap_or((url, ""));
    match (method, path) {
        (Method::Get, "/") => html(PAGE),
        (Method::Get, "/api/state") => json(state_json(state)),
        (Method::Post, "/api/stop") => {
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
        (Method::Post, "/api/cancel") => {
            {
                let mut state = state.lock().expect("state poisoned");
                let State {
                    engine, control, ..
                } = &mut *state;
                if let Control::Organ(console) = control {
                    for handle in console.cancel() {
                        engine.send(Command::StopVoice { handle });
                    }
                }
            }
            json(state_json(state))
        }
        (Method::Post, "/api/coupler") => {
            let index = param(query, "idx").and_then(|v| v.parse::<usize>().ok());
            let on = param(query, "on") == Some("1");
            match index {
                Some(index) => {
                    {
                        let mut state = state.lock().expect("state poisoned");
                        let State {
                            engine, control, ..
                        } = &mut *state;
                        if let Control::Organ(console) = control {
                            let (start, stop) = console.set_coupler(index, on);
                            send_start(engine, start);
                            if let Some(handle) = stop {
                                engine.send(Command::StopVoice { handle });
                            }
                        }
                    }
                    json(state_json(state))
                }
                None => bad_request("missing idx"),
            }
        }
        (Method::Post, "/api/noises") => {
            {
                let mut state = state.lock().expect("state poisoned");
                let State {
                    engine, control, ..
                } = &mut *state;
                if let Control::Organ(console) = control {
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
                }
            }
            json(state_json(state))
        }
        (Method::Post, "/api/reverb") => {
            match param(query, "wet").and_then(|v| v.parse::<f32>().ok()) {
                Some(wet) if (0.0..=2.0).contains(&wet) => {
                    {
                        let mut state = state.lock().expect("state poisoned");
                        if state.reverb_wet.is_some() {
                            state.reverb_wet = Some(wet);
                            state.engine.send(Command::SetReverbWet { wet });
                        }
                    }
                    json(state_json(state))
                }
                _ => bad_request("bad wet"),
            }
        }
        (Method::Post, "/api/tuning") => {
            {
                let mut state = state.lock().expect("state poisoned");
                if let Control::Organ(console) = &mut state.control {
                    let mut tuning = console.tuning();
                    if let Some(t) =
                        param(query, "temperament").and_then(crate::tuning::Temperament::parse)
                    {
                        tuning.temperament = t;
                    }
                    if let Some(a4) = param(query, "a4").and_then(|v| v.parse::<f64>().ok()) {
                        tuning.a4_hz = a4.clamp(300.0, 500.0);
                    }
                    if let Some(t) = param(query, "transpose").and_then(|v| v.parse::<i8>().ok())
                    {
                        tuning.transpose = t.clamp(-12, 12);
                    }
                    console.set_tuning(tuning);
                }
            }
            json(state_json(state))
        }
        (Method::Post, "/api/note") => {
            let manual = param(query, "manual").and_then(|v| v.parse::<usize>().ok());
            let key = param(query, "key").and_then(|v| v.parse::<u8>().ok());
            let on = param(query, "on") == Some("1");
            match (manual, key) {
                (Some(manual), Some(key)) if key < 128 => {
                    apply_note(state, manual, key, on);
                    json(state_json(state))
                }
                _ => bad_request("missing manual/key"),
            }
        }
        (Method::Post, "/api/panic") => {
            let mut state = state.lock().expect("state poisoned");
            let State {
                engine, control, ..
            } = &mut *state;
            if let Control::Organ(console) = control {
                console.all_off();
            }
            engine.send(Command::AllNotesOff);
            json(state_json_locked(&state))
        }
        (Method::Post, "/api/trem") => {
            let on = param(query, "on") == Some("1");
            apply_trem(state, on);
            json(state_json(state))
        }
        (Method::Post, "/api/enclosure") => {
            let index = param(query, "idx").and_then(|v| v.parse::<usize>().ok());
            let value = param(query, "v").and_then(|v| v.parse::<f32>().ok());
            match (index, value) {
                (Some(index), Some(value)) if (0.0..=1.0).contains(&value) => {
                    {
                        let mut state = state.lock().expect("state poisoned");
                        let State {
                            engine, control, ..
                        } = &mut *state;
                        if let Control::Organ(console) = control {
                            if let Some((enclosure, position)) =
                                console.set_enclosure(index, value)
                            {
                                engine.send(Command::SetEnclosurePosition {
                                    enclosure,
                                    position,
                                });
                            }
                        }
                    }
                    json(state_json(state))
                }
                _ => bad_request("bad enclosure move"),
            }
        }
        (Method::Post, "/api/gain") => match param(query, "v").and_then(|v| v.parse::<f32>().ok())
        {
            Some(v) if (0.0..=2.0).contains(&v) => {
                let mut state = state.lock().expect("state poisoned");
                state.master_gain = v;
                state.engine.send(Command::SetMasterGain { linear: v });
                json(state_json_locked(&state))
            }
            _ => bad_request("bad gain"),
        },
        _ => Response::from_string("not found").with_status_code(404),
    }
}

/// A key press/release from the UI — same path as a MIDI note, but
/// addressed by manual index rather than channel.
fn apply_note(state: &Mutex<State>, manual: usize, key: u8, on: bool) {
    let mut state = state.lock().expect("state poisoned");
    let State {
        engine, control, ..
    } = &mut *state;
    if let Control::Organ(console) = control {
        if on {
            let (starts, retriggered) = console.note_on_manual(manual, key);
            for handle in retriggered {
                engine.send(Command::StopVoice { handle });
            }
            for start in starts {
                send_start(engine, Some(start));
            }
        } else {
            for handle in console.note_off_manual(manual, key) {
                engine.send(Command::StopVoice { handle });
            }
        }
    }
}

fn apply_stop(state: &Mutex<State>, id: u32, on: bool) {
    let mut state = state.lock().expect("state poisoned");
    let State {
        engine, control, ..
    } = &mut *state;
    if let Control::Organ(console) = control {
        let (stopped, starts) = console.set_drawn(aristide_model::StopId(id), on);
        for handle in stopped {
            engine.send(Command::StopVoice { handle });
        }
        for start in starts {
            send_start(engine, Some(start));
        }
    }
}

/// Start a control-noise one-shot (drawstop thump, coupler clack).
fn send_start(engine: &mut aristide_engine::EngineHandle, noise: Option<crate::console::VoiceStart>) {
    if let Some(start) = noise {
        engine.send(Command::StartVoice {
            handle: start.handle,
            sample: start.spec.sample,
            rate: start.spec.rate,
            gain: start.spec.gain,
            group: start.spec.group,
            wind_weight: start.spec.wind_weight,
            brightness: start.spec.brightness,
            enclosure: start.spec.enclosure,
        });
    }
}

fn apply_trem(state: &Mutex<State>, on: bool) {
    let mut state = state.lock().expect("state poisoned");
    let changed = state.trem_engaged != on;
    state.trem_engaged = on;
    let groups = state.trem_groups.clone();
    for group in groups {
        state.engine.send(Command::SetTremulant { group, engaged: on });
    }
    if changed {
        let State {
            engine, control, ..
        } = &mut *state;
        if let Control::Organ(console) = control {
            let (start, stop) = console.tremulant_toggle_noise(on);
            send_start(engine, start);
            if let Some(handle) = stop {
                engine.send(Command::StopVoice { handle });
            }
        }
    }
}

fn state_json(state: &Mutex<State>) -> String {
    state_json_locked(&state.lock().expect("state poisoned"))
}

fn state_json_locked(state: &State) -> String {
    let mut out = String::from("{\"stops\":[");
    if let Control::Organ(console) = &state.control {
        let mut first = true;
        for (id, name, manual, drawn) in console.stop_states() {
            if !first {
                out.push(',');
            }
            first = false;
            out.push_str(&format!(
                "{{\"id\":{},\"name\":{},\"manual\":{},\"on\":{}}}",
                id.0,
                json_string(name),
                json_string(manual),
                drawn
            ));
        }
    }
    out.push_str("],\"couplers\":[");
    if let Control::Organ(console) = &state.control {
        let mut first = true;
        for (index, name, engaged) in console.coupler_states() {
            if !first {
                out.push(',');
            }
            first = false;
            out.push_str(&format!(
                "{{\"idx\":{index},\"name\":{},\"on\":{engaged}}}",
                json_string(name)
            ));
        }
    }
    out.push_str("],\"manuals\":[");
    if let Control::Organ(console) = &state.control {
        let mut first = true;
        for (idx, name, first_key, key_count, held) in console.manual_states() {
            if !first {
                out.push(',');
            }
            first = false;
            let held: Vec<String> = held.iter().map(|k| k.to_string()).collect();
            out.push_str(&format!(
                "{{\"idx\":{idx},\"name\":{},\"first_key\":{first_key},\"key_count\":{key_count},\"held\":[{}]}}",
                json_string(name),
                held.join(",")
            ));
        }
    }
    out.push_str(&format!(
        "],\"tremulant\":{},\"gain\":{}",
        state.trem_engaged, state.master_gain
    ));
    if let Control::Organ(console) = &state.control {
        out.push_str(&format!(",\"organ\":{}", json_string(console.organ_name())));
    }
    if let Control::Organ(console) = &state.control {
        let tuning = console.tuning();
        out.push_str(&format!(
            ",\"tuning\":{{\"temperament\":{},\"a4\":{},\"transpose\":{}}}",
            json_string(tuning.temperament.name()),
            tuning.a4_hz,
            tuning.transpose
        ));
    }
    if let Some(wet) = state.reverb_wet {
        out.push_str(&format!(",\"reverb\":{wet}"));
    }
    if let Control::Organ(console) = &state.control {
        let (enabled, volume) = console.noises();
        out.push_str(&format!(
            ",\"noises\":{{\"on\":{enabled},\"vol\":{volume}}}"
        ));
        out.push_str(",\"enclosures\":[");
        let mut first = true;
        for (index, name, position, displayed) in console.enclosure_states() {
            if !first {
                out.push(',');
            }
            first = false;
            out.push_str(&format!(
                "{{\"idx\":{index},\"name\":{},\"value\":{position},\"displayed\":{displayed}}}",
                json_string(&name)
            ));
        }
        out.push(']');
    }
    out.push('}');
    out
}

fn json_string(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for c in value.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn param<'a>(query: &'a str, key: &str) -> Option<&'a str> {
    query
        .split('&')
        .filter_map(|pair| pair.split_once('='))
        .find(|(k, _)| *k == key)
        .map(|(_, v)| v)
}

fn html(body: &str) -> Response<std::io::Cursor<Vec<u8>>> {
    Response::from_string(body).with_header(
        Header::from_bytes("Content-Type", "text/html; charset=utf-8").expect("valid header"),
    )
}

fn json(body: String) -> Response<std::io::Cursor<Vec<u8>>> {
    Response::from_string(body).with_header(
        Header::from_bytes("Content-Type", "application/json").expect("valid header"),
    )
}

fn bad_request(reason: &str) -> Response<std::io::Cursor<Vec<u8>>> {
    Response::from_string(reason).with_status_code(400)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn demo_state() -> Option<Arc<Mutex<State>>> {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../testsets/grandorgue-demo/demo.organ");
        if !path.is_file() {
            eprintln!("skipping: demo set not present");
            return None;
        }
        let organ = aristide_formats::grandorgue::load(&path)
            .expect("demo set loads")
            .organ;
        let loaded = crate::bank::build(&organ, 48000.0).expect("bank builds");
        let console =
            crate::console::Console::new(organ, loaded.specs, Vec::new(), Vec::new());
        let (_engine, handle) =
            aristide_engine::Engine::new(48000.0, std::sync::Arc::new(loaded.bank));
        Some(Arc::new(Mutex::new(State {
            engine: handle,
            control: Control::Organ(console),
            trem_groups: vec![0, 1],
            trem_engaged: false,
            master_gain: 0.178,
            reverb_wet: Some(0.25),
            expression_cc: 11,
        })))
    }

    #[test]
    fn api_toggles_stops_and_tremulant() {
        let Some(state) = demo_state() else { return };

        let body = state_json(&state);
        assert!(body.contains("Montre 8'"), "state lists stops: {body}");
        assert!(body.contains("\"tremulant\":false"));

        // Draw stop 1, verify, retire it again.
        let object_for_id_1 = |body: &str| -> String {
            let start = body.find("{\"id\":1,").expect("stop 1 present");
            body[start..start + body[start..].find('}').expect("object closes")].to_string()
        };
        respond(&state, &Method::Post, "/api/stop?id=1&on=1");
        assert!(object_for_id_1(&state_json(&state)).contains("\"on\":true"));
        respond(&state, &Method::Post, "/api/stop?id=1&on=0");
        assert!(object_for_id_1(&state_json(&state)).contains("\"on\":false"));

        respond(&state, &Method::Post, "/api/trem?on=1");
        assert!(state_json(&state).contains("\"tremulant\":true"));

        respond(&state, &Method::Post, "/api/gain?v=0.5");
        assert!(state_json(&state).contains("\"gain\":0.5"));
    }

    #[test]
    fn general_cancel_clears_stops_and_couplers() {
        let Some(state) = demo_state() else { return };

        // Registration = the stops and couplers arrays, which come
        // first in the snapshot; "noises" carries its own "on" flag.
        let registration = |body: &str| -> String {
            body[..body.find("\"manuals\"").expect("manuals follow")].to_string()
        };

        respond(&state, &Method::Post, "/api/stop?id=1&on=1");
        respond(&state, &Method::Post, "/api/stop?id=16&on=1");
        respond(&state, &Method::Post, "/api/coupler?idx=0&on=1");
        let drawn = registration(&state_json(&state));
        assert_eq!(drawn.matches("\"on\":true").count(), 3, "two stops + a coupler");

        respond(&state, &Method::Post, "/api/cancel");
        let cancelled = registration(&state_json(&state));
        assert!(
            !cancelled.contains("\"on\":true"),
            "cancel left something drawn: {cancelled}"
        );
    }

    #[test]
    fn ui_notes_play_and_state_reports_held_keys() {
        let Some(state) = demo_state() else { return };

        let body = state_json(&state);
        assert!(body.contains("\"manuals\":["), "manuals listed: {body}");
        assert!(body.contains("\"organ\":"), "organ name present: {body}");
        assert!(body.contains("\"first_key\":"), "keyboard compass present");

        // Draw Montre 8' (id 16, First Manual) and press middle C
        // through the API — the state must show the key held.
        respond(&state, &Method::Post, "/api/stop?id=16&on=1");
        respond(&state, &Method::Post, "/api/note?manual=1&key=60&on=1");
        assert!(
            state_json(&state).contains("\"held\":[60]"),
            "pressed key reported held: {}",
            state_json(&state)
        );
        respond(&state, &Method::Post, "/api/note?manual=1&key=60&on=0");
        assert!(!state_json(&state).contains("\"held\":[60]"));

        // Panic silences everything sounding.
        respond(&state, &Method::Post, "/api/note?manual=1&key=62&on=1");
        assert!(state_json(&state).contains("\"held\":[62]"));
        respond(&state, &Method::Post, "/api/panic");
        assert!(!state_json(&state).contains("\"held\":[62]"));

        respond(&state, &Method::Post, "/api/note?manual=9&key=60&on=1");
        assert!(!state_json(&state).contains("\"held\":[60]"), "bad manual ignored");
    }

    #[test]
    fn stop_noises_are_hidden_and_fire_on_toggles() {
        let Some(state) = demo_state() else { return };

        // No control noise masquerades as a drawable stop.
        let body = state_json(&state);
        assert!(
            !body.contains("stop noise") && !body.contains("Motor noise"),
            "noise stops leaked into the stop list"
        );
        assert!(body.contains("\"noises\":{\"on\":true"));

        let mut guard = state.lock().expect("state");
        let Control::Organ(console) = &mut guard.control else {
            panic!("organ expected");
        };
        // Drawing Montre 8' produces its drawknob thump (a percussive
        // one-shot at noise volume).
        let montre = console
            .stop_states()
            .iter()
            .find(|(_, name, _, _)| *name == "Montre 8'")
            .map(|(id, _, _, _)| *id)
            .expect("Montre 8' visible");
        let (stopped, mut noise) = console.set_drawn(montre, true);
        assert!(stopped.is_empty());
        assert_eq!(noise.len(), 1, "drawstop noise mapped and produced");
        let noise = noise.remove(0);
        assert_eq!(noise.spec.wind_weight, 0.0, "noises draw no wind");

        // Redundant draw: no second thump.
        let (_, again) = console.set_drawn(montre, true);
        assert!(again.is_empty());

        // Retiring releases the noise voice (its note-off = the
        // push-in thump).
        let (stopped, on_retire) = console.set_drawn(montre, false);
        assert!(on_retire.is_empty());
        assert!(
            stopped.contains(&noise.handle),
            "retire must note-off the open noise voice"
        );

        // Coupler clack (demo couplers have mapped noises).
        let (clack, _) = console.set_coupler(0, true);
        let clack = clack.expect("coupler noise mapped");
        let (_, unclack) = console.set_coupler(0, false);
        assert_eq!(unclack, Some(clack.handle));

        // Disabling kills open noise voices and mutes future toggles.
        console.set_drawn(montre, true);
        let kills = console.set_noises(false, 0.7);
        assert!(!kills.is_empty(), "open noise voices killed on disable");
        let (_, silent) = console.set_drawn(montre, false);
        assert!(silent.is_empty());
    }
}
