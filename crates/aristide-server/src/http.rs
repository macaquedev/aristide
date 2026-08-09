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
                let response = respond(&state, request.method(), request.url());
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
                            send_noise(engine, start);
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
        (Method::Post, "/api/trem") => {
            let on = param(query, "on") == Some("1");
            apply_trem(state, on);
            json(state_json(state))
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

fn apply_stop(state: &Mutex<State>, id: u32, on: bool) {
    let mut state = state.lock().expect("state poisoned");
    let State {
        engine, control, ..
    } = &mut *state;
    if let Control::Organ(console) = control {
        let (stopped, noise) = console.set_drawn(aristide_model::StopId(id), on);
        for handle in stopped {
            engine.send(Command::StopVoice { handle });
        }
        send_noise(engine, noise);
    }
}

/// Start a control-noise one-shot (drawstop thump, coupler clack).
fn send_noise(engine: &mut aristide_engine::EngineHandle, noise: Option<crate::console::VoiceStart>) {
    if let Some(start) = noise {
        engine.send(Command::StartVoice {
            handle: start.handle,
            sample: start.spec.sample,
            rate: start.spec.rate,
            gain: start.spec.gain,
            group: start.spec.group,
            wind_weight: start.spec.wind_weight,
            brightness: start.spec.brightness,
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
            send_noise(engine, start);
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
    out.push_str(&format!(
        "],\"tremulant\":{},\"gain\":{}",
        state.trem_engaged, state.master_gain
    ));
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
            master_gain: 0.35,
            reverb_wet: Some(0.25),
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
        let (stopped, noise) = console.set_drawn(montre, true);
        assert!(stopped.is_empty());
        let noise = noise.expect("drawstop noise mapped and produced");
        assert_eq!(noise.spec.wind_weight, 0.0, "noises draw no wind");

        // Redundant draw: no second thump.
        let (_, again) = console.set_drawn(montre, true);
        assert!(again.is_none());

        // Retiring releases the noise voice (its note-off = the
        // push-in thump).
        let (stopped, on_retire) = console.set_drawn(montre, false);
        assert!(on_retire.is_none());
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
        assert!(silent.is_none());
    }
}
