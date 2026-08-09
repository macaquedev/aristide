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
                        if let Control::Organ(console) = &mut state.control {
                            console.set_coupler(index, on);
                        }
                    }
                    json(state_json(state))
                }
                None => bad_request("missing idx"),
            }
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
        for handle in console.set_drawn(aristide_model::StopId(id), on) {
            engine.send(Command::StopVoice { handle });
        }
    }
}

fn apply_trem(state: &Mutex<State>, on: bool) {
    let mut state = state.lock().expect("state poisoned");
    state.trem_engaged = on;
    let groups = state.trem_groups.clone();
    for group in groups {
        state.engine.send(Command::SetTremulant { group, engaged: on });
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
        "],\"tremulant\":{},\"gain\":{}}}",
        state.trem_engaged, state.master_gain
    ));
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
}
