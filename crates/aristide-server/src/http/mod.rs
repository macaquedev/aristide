//! A deliberately small local web console: draw/retire stops, toggle
//! the tremulant, set master gain. Serves one embedded page plus a
//! JSON state endpoint on localhost.
//!
//! This is a stopgap until the real IPC control plane + native GUI
//! (M5); it exists so registration changes don't need a restart. It
//! runs on its own thread and talks to the engine exactly like MIDI
//! does: lock the shared state, send commands.
//!
//! This module holds the server loop, the route table, the shared
//! request helpers (`param`, `unescape`, `json`, `bad_request`) and
//! the state snapshot every handler answers with. One handler module
//! per domain holds the routes themselves:
//!
//! - [`organ`] — loading, saving, the library and browser, manuals,
//!   enclosures and panel placement
//! - [`stops`] — drawing, pulling and retiring stops, their order,
//!   voicing and editor
//! - [`couplers`] — engaging, defining and linking couplers
//! - [`tuning`] — every tuning scope, from instrument to rank
//! - [`midi`] — ports, input bindings, learn and control bindings
//! - [`room`] — reverb, noises, tremulants, swell and master gain
//! - [`play`] — the playing surface: keys, pistons, cancel, panic

use std::sync::{Arc, Mutex};

use aristide_engine::Command;
use tiny_http::{Header, Method, Response, Server};

use crate::State;

mod couplers;
mod midi;
mod organ;
mod play;
mod room;
mod snapshot;
mod stops;
mod tuning;

/// Every handler answers with one of these: a body plus a status.
type Reply = Response<std::io::Cursor<Vec<u8>>>;

const PAGE: &str = include_str!("../console.html");

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
    // A sample set's own organ stays the instrument the set defines:
    // one gate, ahead of every handler, so no change to the instrument
    // itself lands live or in the file until the organ is saved under
    // a different name. The player's own settings (wiring, room,
    // whole-instrument pitch, console layout) pass — they are about
    // this player, not the set. 409, not 400 — the request is fine,
    // the organ's state is what refuses — so the console can answer
    // with the save-as dialog rather than an error strip.
    if *method == Method::Post
        && changes_instrument(path, query)
        && state.lock().expect("state poisoned").setup.adopted
    {
        return adopted_refusal();
    }
    match (method, path) {
        (Method::Get, "/") => html(PAGE),
        (Method::Get, "/api/state") => json(snapshot::state_json(state)),
        (Method::Post, "/api/stop") => stops::draw(state, query),
        (Method::Post, "/api/cancel") => play::cancel(state, query),
        (Method::Post, "/api/coupler") => couplers::engage(state, query),
        (Method::Post, "/api/noises") => room::noises(state, query),
        (Method::Post, "/api/bus") => room::bus(state, query),
        (Method::Post, "/api/reverb") => room::reverb(state, query),
        (Method::Post, "/api/tuning") => tuning::set(state, query),
        (Method::Post, "/api/organ/move") => stops::move_to_manual(state, query),
        (Method::Post, "/api/organ/coupler") => couplers::keep(state, query),
        (Method::Post, "/api/organ/compass") => organ::compass(state, query),
        (Method::Post, "/api/organ/manual/add") => organ::manual_add(state, query),
        (Method::Post, "/api/organ/manual/kind") => organ::manual_kind(state, query),
        (Method::Post, "/api/organ/manual/hex") => organ::manual_hex(state, query),
        (Method::Post, "/api/organ/manual/rename") => organ::manual_rename(state, query),
        (Method::Post, "/api/organ/manual/remove") => organ::manual_remove(state, query),
        (Method::Post, "/api/organ/manual/order") => organ::manual_order(state, query),
        (Method::Post, "/api/organ/source/add") => organ::source_add(state, query),
        (Method::Post, "/api/organ/pull") => stops::pull(state, query),
        (Method::Post, "/api/organ/unpull") => stops::unpull(state, query),
        (Method::Post, "/api/organ/stop/rename") => stops::rename(state, query),
        (Method::Post, "/api/organ/stop/voice") => stops::voice(state, query),
        (Method::Post, "/api/organ/stop/label") => stops::label(state, query),
        (Method::Post, "/api/organ/stop/own_pipes") => stops::own_pipes(state, query),
        (Method::Post, "/api/organ/coupler/rename") => couplers::rename(state, query),
        (Method::Post, "/api/organ/coupler/routes") => couplers::routes(state, query),
        (Method::Post, "/api/organ/coupler/add") => couplers::add(state, query),
        (Method::Post, "/api/organ/coupler/remove") => couplers::remove(state, query),
        (Method::Post, "/api/organ/coupler/link") => couplers::link(state, query),
        (Method::Post, "/api/organ/coupler/keys") => couplers::keys(state, query),
        (Method::Post, "/api/organ/coupled_keys") => couplers::coupled_keys(state, query),
        (Method::Post, "/api/organ/stop/source") => stops::source(state, query),
        (Method::Post, "/api/organ/enclosure/add") => organ::enclosure_add(state, query),
        (Method::Post, "/api/organ/enclosure/remove") => organ::enclosure_remove(state, query),
        (Method::Post, "/api/organ/enclosure/assign") => organ::enclosure_assign(state, query),
        (Method::Post, "/api/organ/panel/place") => organ::panel_place(state, query),
        (Method::Post, "/api/organ/stop/order") => stops::order(state, query),
        (Method::Get, "/api/organ/offerings") => organ::offerings(state, query),
        (Method::Post, "/api/organ/load") => organ::load(state, query),
        (Method::Post, "/api/organ/new") => organ::create(state, query),
        (Method::Post, "/api/organ/rename") => organ::rename(state, query),
        (Method::Post, "/api/library/forget") => organ::library_forget(state, query),
        (Method::Get, "/api/browse") => organ::browse(state, query),
        (Method::Post, "/api/organ/save") => organ::save(state, query),
        (Method::Post, "/api/organ/save_as") => organ::save_as(state, query),
        (Method::Post, "/api/midi/bind") => midi::bind(state, query),
        (Method::Post, "/api/midi/unbind") => midi::unbind(state, query),
        (Method::Post, "/api/midi/learn") => midi::learn(state, query),
        (Method::Post, "/api/key") => midi::key(state, query),
        (Method::Post, "/api/action") => midi::action(state, query),
        (Method::Post, "/api/control/bind") => midi::control_bind(state, query),
        (Method::Post, "/api/control/unbind") => midi::control_unbind(state, query),
        (Method::Post, "/api/conflict") => midi::conflict(state, query),
        (Method::Post, "/api/control/learn") => midi::control_learn(state, query),
        (Method::Post, "/api/couplers") => couplers::repitch(state, query),
        (Method::Post, "/api/midi/rescan") => midi::rescan(state, query),
        (Method::Post, "/api/note") => play::note(state, query),
        (Method::Post, "/api/panic") => play::panic_button(state, query),
        (Method::Post, "/api/general") => play::general(state, query),
        (Method::Post, "/api/divisional") => play::divisional(state, query),
        (Method::Post, "/api/stepper") => play::stepper(state, query),
        (Method::Post, "/api/crescendo") => play::crescendo(state, query),
        (Method::Post, "/api/setter") => play::setter(state, query),
        (Method::Post, "/api/trem") => room::trem(state, query),
        (Method::Post, "/api/trem/params") => room::trem_params(state, query),
        (Method::Post, "/api/enclosure") => room::enclosure(state, query),
        (Method::Post, "/api/gain") => room::gain(state, query),
        _ => Response::from_string("not found").with_status_code(404),
    }
}

/// A key press/release from the UI — same path as a MIDI note, but
/// addressed by manual index rather than channel.
fn apply_note(state: &Mutex<State>, manual: usize, key: u16, on: bool) {
    let mut state = state.lock().expect("state poisoned");
    let State {
        engine, control, ..
    } = &mut *state;
    if let Some(console) = control.organ_mut() {
        if on {
            let (starts, retriggered) = console.note_on_manual(manual, key, 127);
            for handle in retriggered {
                engine.send(Command::StopVoice { handle });
            }
            for start in starts {
                send_start(engine, Some(start));
            }
        } else {
            let (stopped, starts) = console.note_off_manual(manual, key);
            for handle in stopped {
                engine.send(Command::StopVoice { handle });
            }
            // A Bass/Melody coupler retargeting onto another held key.
            for start in starts {
                send_start(engine, Some(start));
            }
        }
    }
}

fn apply_stop(state: &Mutex<State>, id: u32, on: bool) {
    let mut state = state.lock().expect("state poisoned");
    let State {
        engine, control, ..
    } = &mut *state;
    if let Some(console) = control.organ_mut() {
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
        engine.send(start.command());
    }
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

/// Percent-decoding, for the one parameter that carries free text: a
/// MIDI port name ("Midiplus AKM320 MIDI 1"). Malformed escapes are
/// left as they are rather than dropped — a name that round-trips
/// wrongly is easier to diagnose than one that silently loses a byte.
fn unescape(value: &str) -> String {
    let mut out = Vec::with_capacity(value.len());
    let bytes = value.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'+' => out.push(b' '),
            b'%' if index + 2 < bytes.len() => {
                match u8::from_str_radix(&value[index + 1..index + 3], 16) {
                    Ok(byte) => {
                        out.push(byte);
                        index += 2;
                    }
                    Err(_) => out.push(b'%'),
                }
            }
            byte => out.push(byte),
        }
        index += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn param<'a>(query: &'a str, key: &str) -> Option<&'a str> {
    query
        .split('&')
        .filter_map(|pair| pair.split_once('='))
        .find(|(k, _)| *k == key)
        .map(|(_, v)| v)
}

/// Every value of a repeated key, in order — `path=a&path=b` is how a
/// multi-set load travels.
fn params<'a>(query: &'a str, key: &'a str) -> impl Iterator<Item = &'a str> {
    query
        .split('&')
        .filter_map(|pair| pair.split_once('='))
        .filter(move |(k, _)| *k == key)
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

/// The refusal every instrument-changing route answers while the
/// loaded organ is a sample set's own (`adopted = true` in its file).
pub const ADOPTED_REFUSAL: &str = "this is the sample set's own organ, kept as the set defines \
     it — save it under a different name to change the instrument itself";

fn adopted_refusal() -> Response<std::io::Cursor<Vec<u8>>> {
    Response::from_string(ADOPTED_REFUSAL).with_status_code(409)
}

/// Whether a POST changes the *instrument* — what a sample set's own
/// organ refuses. The line: anything that alters what the set defines
/// (its keyboards, stops, couplers, enclosures, sources, voicing, a
/// tuning of any scope below the whole instrument, the tremulant's
/// shape, the organ's name) is the instrument. Anything about how
/// this player uses it — MIDI wiring and the learns and conflicts
/// that end in a bind, the room, the whole-instrument pitch, where
/// panels sit, how knobs are ordered, whether coupled keys show — is
/// the player's, and lands in the set's own organ file so the set
/// comes back wired and pitched as they left it. Playing (stops,
/// keys, swell, gain, pistons) is no change at all; neither is
/// loading another organ, nor saving this one. Mirrors the
/// `config::write_composite_*` family: extend both together.
fn changes_instrument(path: &str, query: &str) -> bool {
    match path {
        // The cascade below the instrument — a division, a set, a
        // stop, a rank — is voicing; the instrument's own root is the
        // player's concert pitch.
        "/api/tuning" => ["manual", "source", "stop", "rank"]
            .iter()
            .any(|scope| param(query, scope).is_some()),
        "/api/trem/params" => true,
        "/api/organ/load" | "/api/organ/new" | "/api/organ/save" | "/api/organ/save_as"
        | "/api/organ/panel/place" | "/api/organ/stop/order" | "/api/organ/coupled_keys" => {
            false
        }
        _ => path.starts_with("/api/organ/"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::snapshot::state_json;
    use crate::Control;
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
        // What load::prepare records for a set standing as itself:
        // each stop from its own manual, under the wrapper alias the
        // adoption inventory uses.
        let provenance: std::collections::HashMap<
            aristide_model::StopId,
            aristide_formats::instrument::StopProvenance,
        > = organ
            .stops
            .iter()
            .map(|stop| {
                (
                    stop.id,
                    aristide_formats::instrument::StopProvenance {
                        source: "s1".into(),
                        source_manual: organ
                            .manuals
                            .iter()
                            .find(|m| m.id == stop.manual)
                            .map(|m| m.name.clone())
                            .unwrap_or_default(),
                        source_stop: stop.name.clone(),
                        via_division: false,
                    },
                )
            })
            .collect();
        let loaded = crate::bank::build(&organ, 48000.0, 16, None).expect("bank builds");
        let mut console =
            crate::console::Console::new(organ, loaded.specs, Vec::new(), 48000.0);
        console.set_home(loaded.home.map(std::sync::Arc::new));
        console.set_stop_sources(
            provenance
                .iter()
                .map(|(id, prov)| (*id, prov.source.clone()))
                .collect(),
        );
        if let Some(home) = console.home() {
            let mut tuning = console.tuning();
            tuning.reference = home.reference(69);
            console.set_tuning(tuning);
        }
        let (_engine, handle) =
            aristide_engine::Engine::new(48000.0, std::sync::Arc::new(loaded.bank));
        let state = Arc::new(Mutex::new(State {
            engine: handle,
            control: Control::Organ(console),
            midi_ports: Vec::new(),
            midi_config: Default::default(),
            // Tests must never touch the user's real assignments.
            config_path: None,
            organ_key: "test organ".into(),
            suggested_channels: vec![Some(3), Some(1), Some(2)],
            learn: None,
            control_learn: None,
            pending: None,
            key_bindings: Vec::new(),
            keyboard: Vec::new(),
            live_notes: std::collections::HashMap::new(),
            channel_bend: std::collections::HashMap::new(),
            ltn_cache: std::collections::HashMap::new(),
            trems: vec![crate::TremControl {
                name: "Tremulant".into(),
                wave: false,
                groups: vec![0, 1],
                engaged: false,
                params: Default::default(),
            }],
            setter_armed: false,
            stepper_frame: 0,
            crescendo_stage: 0,
            master_gain: 0.178,
            reverb_wet: Some(0.25),
            expression_cc: 11,
            composite_path: None,
            setup: Default::default(),
            provenance,
            stop_voicing: Default::default(),
            stop_labels: Default::default(),
            stop_order: Default::default(),
            compass_overrides: Vec::new(),
            pending_load: None,
            loading: None,
            load_error: None,
            load_warnings: Vec::new(),
            layout: Default::default(),
            coupled_keys: true,
            coupler_key_modes: Default::default(),
        }));
        // As the server does once before it opens any device: routing,
        // bindings and the computer keyboard all come from this.
        state.lock().expect("state poisoned").resolve_routes();
        Some(state)
    }

    /// The snapshot names what the demo set was recorded in, and the
    /// tuning API speaks `original`: naming it returns the reference
    /// to the organ's own pitch, `home` for either Hz field does the
    /// same on demand, and a target keeps whatever reference it had.
    #[test]
    fn snapshot_names_the_recorded_tuning_and_original_follows_it() {
        let Some(state) = demo_state() else { return };
        let body = state_json(&state);
        let home_a4 = body
            .split("\"home\":{\"a4_hz\":")
            .nth(1)
            .and_then(|rest| rest.split(',').next())
            .and_then(|hz| hz.parse::<f64>().ok())
            .unwrap_or_else(|| panic!("home in the snapshot: {body}"));
        assert!((400.0..480.0).contains(&home_a4), "a′ = {home_a4}");
        assert!(body.contains("\"offsets_cents\":["), "{body}");
        assert!(body.contains("\"temperament\":\"original\""), "the default: {body}");
        let reference_hz = |body: &str| {
            body.split("\"reference\":{\"key\":69,\"hz\":")
                .nth(1)
                .and_then(|rest| rest.split('}').next())
                .and_then(|hz| hz.parse::<f64>().ok())
                .unwrap_or_else(|| panic!("reference in the snapshot: {body}"))
        };
        assert!(
            (reference_hz(&body) - home_a4).abs() < 0.01,
            "as recorded, the reference is the organ's own a′"
        );

        let ok = respond(&state, &Method::Post, "/api/tuning?temperament=equal&a4=440");
        assert_eq!(ok.status_code().0, 200);
        let body = state_json(&state);
        assert!(body.contains("\"temperament\":\"equal\""), "{body}");
        assert_eq!(reference_hz(&body), 440.0);

        respond(&state, &Method::Post, "/api/tuning?temperament=original");
        let body = state_json(&state);
        assert!(body.contains("\"temperament\":\"original\""), "{body}");
        assert!((reference_hz(&body) - home_a4).abs() < 0.01, "back to as recorded");

        respond(&state, &Method::Post, "/api/tuning?a4=430");
        assert_eq!(reference_hz(&state_json(&state)), 430.0, "pulled by hand");
        respond(&state, &Method::Post, "/api/tuning?reference_hz=home");
        assert!((reference_hz(&state_json(&state)) - home_a4).abs() < 0.01, "and released");
        let bad = respond(&state, &Method::Post, "/api/tuning?reference_hz=loud");
        assert_eq!(bad.status_code().0, 400);

        // Pipes keep their drift unless asked to land exactly.
        assert!(state_json(&state).contains("\"pipes\":\"original\""));
        respond(&state, &Method::Post, "/api/tuning?pipes=exact");
        assert!(state_json(&state).contains("\"pipes\":\"exact\""));
        let bad = respond(&state, &Method::Post, "/api/tuning?pipes=sloppy");
        assert_eq!(bad.status_code().0, 400);
    }

    /// The whole setup story over the API: the snapshot describes how
    /// the instrument was put together, a declared compass takes
    /// effect live and survives in the snapshot, and saving writes a
    /// composite file that loads back to the same structure — with the
    /// MIDI wiring inside, since the file owns it from then on.
    #[test]
    fn organ_setup_compass_and_save_round_trip() {
        let Some(state) = demo_state() else { return };
        let demo = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../testsets/grandorgue-demo/demo.organ");
        {
            let mut state = state.lock().expect("state poisoned");
            let names: Vec<String> = state.manual_names();
            state.setup.sources = vec![("Demo".into(), demo.clone())];
            state.setup.pulls = names
                .iter()
                .enumerate()
                .map(|(index, name)| (0, name.clone(), index))
                .collect();
            state.setup.implicit = true;
        }
        let body = state_json(&state);
        assert!(body.contains("\"setup\":{\"implicit\":true,\"file\":null"));
        assert!(body.contains("\"native_low\""));

        // Declare a wider compass on manual 1 and see it live.
        respond(&state, &Method::Post, "/api/organ/compass?manual=1&low=24&high=103");
        let body = state_json(&state);
        assert!(
            body.contains("\"idx\":1,\"low\":24,\"high\":103") && body.contains("\"declared\":true"),
            "declared compass shows: {body}"
        );

        // Save, then load the file back through the instrument layer.
        let path = std::env::temp_dir().join("aristide-organ-save-test.toml");
        let _ = std::fs::remove_file(&path);
        respond(
            &state,
            &Method::Post,
            &format!(
                "/api/organ/save?path={}",
                path.display().to_string().replace('/', "%2F")
            ),
        );
        let body = state_json(&state);
        assert!(body.contains("\"implicit\":false"), "saved: {body}");
        let saved = aristide_formats::instrument::load(&path).expect("saved organ loads");
        // The saved name is the assignments key (== the organ's name
        // in production; the fixture keeps them apart on purpose).
        assert_eq!(saved.organ.name, "test organ");
        let manual = saved.organ.manuals.iter().find(|m| m.first_midi_note == 24);
        assert!(manual.is_some_and(|m| m.key_count == 80), "declared compass survives");
        assert_eq!(
            saved.organ.manuals.len(),
            state_json(&state).matches("\"native_low\"").count()
        );
        assert!(!saved.organ.stops.is_empty());

        // The file now owns the wiring: a binding learned via the API
        // lands in it.
        respond(
            &state,
            &Method::Post,
            "/api/midi/bind?manual=0&slot=0&device=Test%20Keys&ch=4",
        );
        let saved = aristide_formats::instrument::load(&path).expect("still loads");
        assert_eq!(saved.midi.inputs.len(), 1);
        assert_eq!(saved.midi.inputs[0].device, "Test Keys");
        assert_eq!(saved.midi.inputs[0].channel, Some(4));

        // A division tuned apart, a coupler taken off, and a stop
        // moved all show live and land in the file.
        respond(
            &state,
            &Method::Post,
            "/api/tuning?manual=1&temperament=meantone&a4=415",
        );
        respond(&state, &Method::Post, "/api/organ/coupler?idx=0&keep=0");
        let (stop, stop_name, target) = {
            let state = state.lock().expect("state poisoned");
            let Some(console) = state.console() else {
                unreachable!()
            };
            let (id, name, _, from, _) = console.stop_states()[0];
            (id.0, name.to_string(), if from == 0 { 1 } else { 0 })
        };
        respond(
            &state,
            &Method::Post,
            &format!("/api/organ/move?stop={stop}&manual={target}"),
        );
        let body = state_json(&state);
        assert!(
            body.contains(
                "\"manual_tuning\":[{\"idx\":1,\"temperament\":\"meantone4\",\"edo\":12,\"reference\":{\"key\":69,\"hz\":415}"
            ),
            "own tuning shows: {body}"
        );
        assert!(body.contains("\"hidden\":true"), "picked-off coupler shows");
        let saved = aristide_formats::instrument::load(&path).expect("reloads");
        assert_eq!(
            saved.manual_tuning,
            [aristide_formats::instrument::ManualTuningDef {
                manual: 1,
                temperament: Some("meantone4".into()),
                edo: None,
                reference_key: Some(aristide_formats::sidecar::KeySpec::Name("A4".into())),
                reference_hz: Some(415.0),
                transpose: Some(0),
                scale: None,
                keymap: None,
                pipes: None,
            }]
        );
        assert_eq!(saved.sidecar.couplers.drop.len(), 1);
        // Ids renumber on reload; the stop's name is its identity here.
        let _ = stop;
        let moved = saved
            .organ
            .stops
            .iter()
            .find(|s| s.name == stop_name)
            .expect("moved stop reloads");
        assert_eq!(
            moved.manual, saved.organ.manuals[target].id,
            "the move replays from the file"
        );
        let _ = std::fs::remove_file(&path);
    }

    /// Every tuning scope over one endpoint: a set by alias, a stop by
    /// pin or own tuning, a rank within a stop — each live in the
    /// snapshot (the stop reporting what it resolves to) and in the
    /// file, and each undone the same way.
    #[test]
    fn sets_stops_and_ranks_tune_apart_over_the_api() {
        let Some(state) = demo_state() else { return };
        let demo = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../testsets/grandorgue-demo/demo.organ");
        let (stop, rank, other) = {
            let mut state = state.lock().expect("state poisoned");
            let names: Vec<String> = state.manual_names();
            state.setup.sources = vec![("Demo".into(), demo.clone())];
            state.setup.pulls = names
                .iter()
                .enumerate()
                .map(|(index, name)| (0, name.clone(), index))
                .collect();
            state.setup.implicit = true;
            let Some(console) = state.console() else { unreachable!() };
            let states = console.stop_states();
            let (id, ..) = states[0];
            let (other, ..) = states[1];
            (id, console.stop_ranks(id)[0].0, other)
        };
        let path = std::env::temp_dir().join("aristide-scoped-tuning-api-test.toml");
        let _ = std::fs::remove_file(&path);
        respond(
            &state,
            &Method::Post,
            &format!("/api/organ/save?path={}", path.display().to_string().replace('/', "%2F")),
        );
        let stop_entry = |body: &str, id: aristide_model::StopId| -> String {
            let at = body.find(&format!("{{\"id\":{},\"name\":", id.0)).expect("stop listed");
            let end = body[at..].find("\"ranks\":[").expect("ranks follow") + at;
            body[at..end].to_string()
        };

        // The set: every stop from it follows it.
        let ok = respond(&state, &Method::Post, "/api/tuning?source=s1&temperament=meantone");
        assert_eq!(ok.status_code().0, 200);
        let body = state_json(&state);
        assert!(
            body.contains("\"source_tuning\":[{\"source\":\"s1\",\"temperament\":\"meantone4\""),
            "set tuning shows: {body}"
        );
        assert!(
            stop_entry(&body, stop).contains("\"tuning\":{\"scope\":\"source\",\"follow\":\"auto\"}"),
            "a stop resolves to its set: {body}"
        );
        let bad = respond(&state, &Method::Post, "/api/tuning?source=nowhere&temperament=equal");
        assert_eq!(bad.status_code().0, 400);

        // A pin past the set, an own tuning, a rank of its own.
        respond(&state, &Method::Post, &format!("/api/tuning?stop={}&follow=organ", stop.0));
        let body = state_json(&state);
        assert!(
            stop_entry(&body, stop).contains("\"scope\":\"organ\",\"follow\":\"organ\""),
            "pinned: {body}"
        );
        assert!(
            body.contains(&format!("\"stop_tuning\":[{{\"stop\":{},\"follow\":\"organ\"}}]", stop.0)),
            "the pin is listed: {body}"
        );
        respond(
            &state,
            &Method::Post,
            &format!("/api/tuning?stop={}&follow=own&temperament=pythagorean", stop.0),
        );
        let body = state_json(&state);
        assert!(stop_entry(&body, stop).contains("\"scope\":\"stop\",\"follow\":\"own\""), "{body}");
        assert!(
            body.contains(&format!(
                "\"stop_tuning\":[{{\"stop\":{},\"follow\":\"own\",\"temperament\":\"pythagorean\"",
                stop.0
            )),
            "own tuning listed: {body}"
        );
        assert!(
            stop_entry(&body, other).contains("\"scope\":\"source\""),
            "the other stop still follows the set: {body}"
        );
        respond(
            &state,
            &Method::Post,
            &format!("/api/tuning?stop={}&rank={}&temperament=werckmeister", other.0, {
                let state = state.lock().expect("state poisoned");
                let Some(console) = state.console() else { unreachable!() };
                console.stop_ranks(other)[0].0.0
            }),
        );
        let body = state_json(&state);
        assert!(
            body.contains(&format!("\"rank_tuning\":[{{\"stop\":{},\"rank\":", other.0)),
            "rank tuning listed: {body}"
        );
        assert!(body.contains("\"own\":true}"), "the rank is marked: {body}");
        let bad = respond(&state, &Method::Post, &format!("/api/tuning?stop={}&follow=sideways", stop.0));
        assert_eq!(bad.status_code().0, 400);

        // All of it in the file.
        let saved = aristide_formats::instrument::load(&path).expect("reloads");
        assert_eq!(
            saved.source_tuning.get("s1").and_then(|t| t.temperament.as_deref()),
            Some("meantone4")
        );
        assert_eq!(saved.sidecar.tuning.stops.len(), 2, "{:?}", saved.sidecar.tuning.stops);
        assert!(saved.sidecar.tuning.stops.iter().any(|row| row.rank.is_some()));
        let reloaded = crate::load::prepare(std::slice::from_ref(&path), &[], 48_000.0, &|_| {})
            .expect("the scoped file loads");
        assert!(reloaded.console.source_tuning("s1").is_some());
        assert_eq!(reloaded.console.stop_tunings().len(), 1);
        assert_eq!(reloaded.console.rank_tunings().len(), 1);

        // And undone: follow=auto, reset=1 on the rank, follow=organ on
        // the set.
        respond(&state, &Method::Post, &format!("/api/tuning?stop={}&follow=auto", stop.0));
        respond(&state, &Method::Post, &format!("/api/tuning?stop={}&rank={}&reset=1", other.0, rank.0));
        let _ = rank;
        respond(&state, &Method::Post, "/api/tuning?source=s1&follow=organ");
        let body = state_json(&state);
        assert!(!body.contains("\"source_tuning\""), "{body}");
        assert!(!body.contains("\"stop_tuning\""), "{body}");
        assert!(stop_entry(&body, stop).contains("\"scope\":\"organ\",\"follow\":\"auto\""));
        let saved = aristide_formats::instrument::load(&path).expect("reloads");
        assert!(saved.source_tuning.is_empty());
        let _ = std::fs::remove_file(&path);
    }

    /// Three settings the console commits without naming a manual.
    /// Instrument-wide tuning always writes through — those are field
    /// commits, not slider drags; reverb wet and noises only write when
    /// asked with `persist=1`, and are otherwise live-only exactly as
    /// before. Reverb also proves the "no [reverb] table, no write"
    /// rule at the endpoint, not just the writer: the demo set's own
    /// sidecar keeps reverb off, so its saved file never grows one.
    #[test]
    fn tuning_reverb_and_noises_persist_when_the_organ_has_a_file() {
        let Some(state) = demo_state() else { return };
        let path = std::env::temp_dir().join("aristide-instrument-settings-endpoint-test.toml");
        let _ = std::fs::remove_file(&path);
        {
            let mut state = state.lock().expect("state poisoned");
            let names: Vec<String> = state.manual_names();
            state.setup.sources = vec![(
                "Demo".into(),
                Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("../../testsets/grandorgue-demo/demo.organ"),
            )];
            state.setup.pulls = names
                .iter()
                .enumerate()
                .map(|(index, name)| (0, name.clone(), index))
                .collect();
            state.setup.implicit = true;
        }
        respond(
            &state,
            &Method::Post,
            &format!(
                "/api/organ/save?path={}",
                path.display().to_string().replace('/', "%2F")
            ),
        );

        // Whole-instrument tuning: no persist flag exists for it, and
        // none is needed — every successful commit lands in the file.
        respond(&state, &Method::Post, "/api/tuning?temperament=meantone&a4=415");
        assert!(
            state_json(&state).contains(
                "\"tuning\":{\"temperament\":\"meantone4\",\"edo\":12,\"reference\":{\"key\":69,\"hz\":415}"
            ),
            "live tuning shows: {}",
            state_json(&state)
        );
        let saved = aristide_formats::instrument::load(&path).expect("reloads");
        assert_eq!(saved.sidecar.tuning.temperament, "meantone4");
        assert_eq!(saved.sidecar.tuning.reference_hz, Some(415.0));
        assert_eq!(saved.sidecar.tuning.reference_key.midi_note(), Some(69));

        // The anchor may name any key: middle C at 256 Hz lands live
        // and in the file as the pair it is.
        respond(
            &state,
            &Method::Post,
            "/api/tuning?reference_key=C4&reference_hz=256",
        );
        assert!(
            state_json(&state).contains("\"reference\":{\"key\":60,\"hz\":256}"),
            "{}",
            state_json(&state)
        );
        let saved = aristide_formats::instrument::load(&path).expect("reloads");
        assert_eq!(saved.sidecar.tuning.reference_key.midi_note(), Some(60));
        assert_eq!(saved.sidecar.tuning.reference_hz, Some(256.0));
        let text = std::fs::read_to_string(&path).expect("reads");
        assert!(text.contains("reference_key = \"C4\""), "{text}");
        assert!(!text.contains("a4_hz"), "old spelling must not linger: {text}");
        let response = respond(&state, &Method::Post, "/api/tuning?reference_key=H9");
        assert_eq!(response.status_code().0, 400, "a non-key is refused");

        // Reverb without persist=1 stays live-only.
        respond(&state, &Method::Post, "/api/reverb?wet=0.6");
        assert!(state_json(&state).contains("\"reverb\":0.6"));
        let saved = aristide_formats::instrument::load(&path).expect("reloads");
        assert_eq!(saved.sidecar.reverb.wet, 0.25, "unpersisted, the default stands");

        // With persist=1, the demo set's sidecar declares no [reverb]
        // (it is recorded wet already), so the file grows none either —
        // wet has nothing to mean without an impulse response.
        respond(&state, &Method::Post, "/api/reverb?wet=0.7&persist=1");
        let text = std::fs::read_to_string(&path).expect("reads");
        assert!(!text.contains("[reverb]"), "no ir, so persist writes nothing: {text}");

        // Noises without persist=1 stays live-only; with it, the file
        // gains a [noises] table (there was none before — the demo set
        // is silent on the point, so the loader's defaults applied).
        respond(&state, &Method::Post, "/api/noises?on=0&vol=0.3");
        assert!(state_json(&state).contains("\"noises\":{\"on\":false,\"vol\":0.3}"));
        let saved = aristide_formats::instrument::load(&path).expect("reloads");
        assert!(saved.sidecar.noises.enabled, "unpersisted, the default stands");

        respond(&state, &Method::Post, "/api/noises?on=0&vol=0.3&persist=1");
        let saved = aristide_formats::instrument::load(&path).expect("reloads");
        assert!(!saved.sidecar.noises.enabled);
        // The query parses as f32; persisted as f64, only that much
        // precision survives.
        assert_eq!(saved.sidecar.noises.volume, 0.3_f32 as f64);

        // Mid-rebuild, the whole-instrument write refuses rather than
        // race the file the rebuild is about to replace.
        state.lock().expect("state poisoned").loading = Some("rebuilding…".into());
        let response = respond(&state, &Method::Post, "/api/tuning?a4=430");
        assert_eq!(response.status_code().0, 400, "refuses while loading");
        state.lock().expect("state poisoned").loading = None;

        let _ = std::fs::remove_file(&path);
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

    /// The combination action over HTTP, and the shape the piston rail
    /// reads it in: every endpoint is the on-screen twin of a binding
    /// action, and the snapshot says where the stepper and the pedal
    /// stand without the console having to guess.
    #[test]
    fn the_combination_endpoints_move_the_console_and_ride_the_snapshot() {
        let Some(state) = demo_state() else { return };
        let stage = |body: &str, field: &str| -> i64 {
            body.split(&format!("\"{field}\":"))
                .nth(1)
                .and_then(|rest| rest.split(&[',', '}'][..]).next())
                .and_then(|value| value.parse::<i64>().ok())
                .unwrap_or_else(|| panic!("{field} in the snapshot: {body}"))
        };

        // Set arms; the general press that follows stores and disarms.
        respond(&state, &Method::Post, "/api/stop?id=1&on=1");
        respond(&state, &Method::Post, "/api/setter?on=1");
        assert!(state_json(&state).contains("\"setter\":true"));
        respond(&state, &Method::Post, "/api/general?n=2");
        let body = state_json(&state);
        assert!(body.contains("\"setter\":false"), "storing disarmed: {body}");
        assert!(body.contains("\"generals\":[2]"), "slot 2 has something in it: {body}");

        respond(&state, &Method::Post, "/api/cancel");
        respond(&state, &Method::Post, "/api/general?n=2");
        assert!(
            state_json(&state)[..state_json(&state).find("\"manuals\"").expect("manuals")]
                .contains("\"on\":true"),
            "the general recalled its registration"
        );

        // The stepper: a frame is stored, walked to and counted.
        respond(&state, &Method::Post, "/api/stepper?store=1");
        respond(&state, &Method::Post, "/api/stepper?insert=1");
        let body = state_json(&state);
        assert_eq!(stage(&body, "frames"), 2, "two frames: {body}");
        assert_eq!(stage(&body, "frame"), 2, "standing on the new one");
        respond(&state, &Method::Post, "/api/stepper?go=prev");
        assert_eq!(stage(&state_json(&state), "frame"), 1);

        // The crescendo: a stage stored, the pedal moved onto it, the
        // overlay visible as `cres` without the hand having drawn it.
        respond(&state, &Method::Post, "/api/cancel");
        respond(&state, &Method::Post, "/api/stop?id=16&on=1");
        respond(&state, &Method::Post, "/api/crescendo?stage=1&store=1");
        respond(&state, &Method::Post, "/api/cancel");
        let body = state_json(&state);
        assert_eq!(stage(&body, "crescendo"), 0, "the pedal starts at the heel");
        assert_eq!(stage(&body, "crescendo_stages"), 32, "GO's own count");
        respond(&state, &Method::Post, "/api/crescendo?stage=1");
        let body = state_json(&state);
        assert!(body.contains("\"cres\":true"), "the overlay rides the stop: {body}");
        assert!(body.contains("\"hand\":false"), "…as the pedal's doing, not the hand's");
        respond(&state, &Method::Post, "/api/crescendo?stage=0");
        let body = state_json(&state);
        assert!(!body.contains("\"cres\":true"), "back to the heel: {body}");
        assert!(!body.contains("\"hand\":false"), "and no stop is out of step");
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
    fn midi_assignments_survive_the_api() {
        let Some(state) = demo_state() else { return };

        let body = state_json(&state);
        assert!(
            body.contains(
                "\"midi\":{\"ports\":[{\"id\":0,\"name\":\"Computer keyboard\",\"virtual\":true}]"
            ),
            "no MIDI on a test rig, but the computer keyboard is always \
             assignable: {body}"
        );
        // Demo set: First Manual, Second Manual, Pedal — every one of
        // them listed, every one of them empty.
        assert!(
            body.contains("{\"idx\":1,\"name\":\"First Manual\",\"inputs\":[],\"native\":[36,96]}"),
            "manuals listed with nothing assigned: {body}"
        );
        assert!(!body.contains("\"learning\""), "not listening yet");

        respond(
            &state,
            &Method::Post,
            "/api/midi/bind?manual=2&slot=0&device=Test%20Keyboard&ch=4",
        );
        let body = state_json(&state);
        assert!(
            body.contains(
                "{\"slot\":0,\"device\":\"Test Keyboard\",\"channel\":4,\"connected\":false,\"low\":null,\"high\":null,\"transpose\":0,\"bend\":null,\"map\":null}"
            ),
            "assigned by name, honest that it isn't plugged in, and no \
             compass measured yet: {body}"
        );

        // No channel given falls back to what the set suggests for that
        // manual (Second Manual speaks on channel 2 in the demo sidecar).
        respond(
            &state,
            &Method::Post,
            "/api/midi/bind?manual=2&slot=0&device=Test%20Keyboard",
        );
        assert!(state_json(&state).contains("\"channel\":2"));
        respond(
            &state,
            &Method::Post,
            "/api/midi/bind?manual=2&slot=0&device=Test%20Keyboard&ch=any",
        );
        assert!(state_json(&state).contains("\"channel\":null"));

        // A slot past the end adds an input: two keyboards, one manual.
        respond(
            &state,
            &Method::Post,
            "/api/midi/bind?manual=2&slot=9&device=Second%20Keyboard&ch=2",
        );
        assert!(state_json(&state).contains("\"slot\":1,\"device\":\"Second Keyboard\""));

        // A learned keyboard width rides along on the binding.
        respond(
            &state,
            &Method::Post,
            "/api/midi/bind?manual=2&slot=0&device=Test%20Keyboard&low=31&high=101",
        );
        assert!(state_json(&state).contains("\"low\":31,\"high\":101"));

        // A keyboard that should sound below what it sends: C2–C7 keys
        // playing G1–G6 is transpose −5, set right on the binding.
        respond(
            &state,
            &Method::Post,
            "/api/midi/bind?manual=2&slot=0&device=Test%20Keyboard&transpose=-5",
        );
        let body = state_json(&state);
        assert!(
            body.contains("\"low\":31,\"high\":101,\"transpose\":-5"),
            "the shift is set and the learned compass survives it: {body}"
        );
        // Not sending it keeps it, like low/high; out-of-range clamps
        // to the octave actions' own ±36 bound.
        respond(
            &state,
            &Method::Post,
            "/api/midi/bind?manual=2&slot=0&device=Test%20Keyboard&ch=any",
        );
        assert!(state_json(&state).contains("\"transpose\":-5"));
        respond(
            &state,
            &Method::Post,
            "/api/midi/bind?manual=2&slot=0&device=Test%20Keyboard&transpose=99",
        );
        assert!(state_json(&state).contains("\"transpose\":36"));
        respond(
            &state,
            &Method::Post,
            "/api/midi/bind?manual=2&slot=0&device=Test%20Keyboard&transpose=0",
        );
        respond(
            &state,
            &Method::Post,
            "/api/midi/bind?manual=2&slot=0&device=Test%20Keyboard&low=set&high=set",
        );
        assert!(
            state_json(&state).contains("\"low\":null,\"high\":null"),
            "\"set\" gives the manual back the compass its set declares"
        );

        respond(&state, &Method::Post, "/api/midi/unbind?manual=2&slot=0");
        let body = state_json(&state);
        assert!(
            body.contains("{\"slot\":0,\"device\":\"Second Keyboard\",\"channel\":2"),
            "removing the first input renumbers the rest: {body}"
        );
        respond(&state, &Method::Post, "/api/midi/unbind?manual=2&slot=0");
        assert!(state_json(&state).contains("\"name\":\"Second Manual\",\"inputs\":[],\"native\":"));

        // A manual this organ hasn't got is refused, not clamped.
        respond(
            &state,
            &Method::Post,
            "/api/midi/bind?manual=9&slot=0&device=Test%20Keyboard",
        );
        respond(&state, &Method::Post, "/api/midi/unbind?manual=9&slot=0");

        // Auto-detect is a mode the snapshot reports, so the dialog can
        // show which row is waiting.
        respond(&state, &Method::Post, "/api/midi/learn?manual=1&slot=0");
        assert!(state_json(&state).contains(
            "\"learning\":{\"manual\":1,\"slot\":0,\"step\":\"low\"}"
        ));
        respond(&state, &Method::Post, "/api/midi/learn");
        assert!(!state_json(&state).contains("\"learning\""), "cancelled");
        respond(&state, &Method::Post, "/api/midi/learn?manual=9&slot=0");
        assert!(!state_json(&state).contains("\"learning\""), "no such manual");
    }

    /// Every field a console reads sits where it says it does, and the
    /// whole thing is valid JSON — the snapshot is hand-written, so
    /// nothing else checks that.
    #[test]
    fn the_snapshot_is_well_formed() {
        let Some(state) = demo_state() else { return };
        respond(&state, &Method::Post, "/api/control/bind?slot=0&action=panic");
        let body = state_json(&state);
        let value: serde_json::Value = serde_json::from_str(&body).expect("valid JSON");
        for field in [
            "stops", "couplers", "manuals", "midi", "controls", "actions",
            "tuning", "enclosures", "noises", "coupler_repitch",
        ] {
            assert!(value.get(field).is_some(), "{field} missing: {body}");
        }
        // The keyboard object appears only once the player assigns it —
        // through the same bind call as any MIDI device.
        assert!(
            value.get("keyboard").is_none(),
            "an unassigned computer keyboard is absent, not defaulted: {body}"
        );
        respond(
            &state,
            &Method::Post,
            "/api/midi/bind?manual=1&slot=0&device=Computer%20keyboard",
        );
        let assigned: serde_json::Value =
            serde_json::from_str(&state_json(&state)).expect("valid JSON");
        assert_eq!(assigned["keyboard"]["manual"], 1);
        assert!(value["midi"]["ports"].is_array());
        assert_eq!(value["controls"][0]["action"], "panic");
    }

    #[test]
    fn bindings_survive_the_api() {
        let Some(state) = demo_state() else { return };

        let body = state_json(&state);
        assert!(body.contains("\"controls\":[]"), "nothing bound yet: {body}");
        assert!(
            body.contains("\"actions\":[\"octave-up\""),
            "the vocabulary a UI offers is published: {body}"
        );
        assert!(
            !body.contains("\"keyboard\":{"),
            "the computer keyboard plays nothing until assigned: {body}"
        );

        // Adding a binding, then teaching it what presses it.
        respond(
            &state,
            &Method::Post,
            "/api/control/bind?slot=0&action=octave-up&device=Test%20Keyboard",
        );
        assert!(state_json(&state).contains("\"trigger\":\"\""), "not taught yet");

        respond(&state, &Method::Post, "/api/control/learn?slot=0");
        assert!(state_json(&state).contains("\"control_learning\":0"));
        respond(&state, &Method::Post, "/api/key?code=Equal&on=1");
        let body = state_json(&state);
        assert!(
            body.contains("\"trigger\":\"key:Equal\"")
                && body.contains("\"device\":\"Computer keyboard\""),
            "the key that was pressed became the trigger: {body}"
        );
        assert!(!body.contains("\"control_learning\""), "and the wait ended");

        // Changing the action leaves the taught trigger alone.
        respond(
            &state,
            &Method::Post,
            "/api/control/bind?slot=0&action=stop%3AMontre%208%27",
        );
        let body = state_json(&state);
        assert!(body.contains("\"action\":\"stop:Montre 8'\""), "{body}");
        assert!(body.contains("\"trigger\":\"key:Equal\""), "{body}");

        // And the binding does what it says, from a computer key.
        respond(&state, &Method::Post, "/api/key?code=Equal&on=1");
        let value: serde_json::Value =
            serde_json::from_str(&state_json(&state)).expect("valid JSON");
        let montre = value["stops"]
            .as_array()
            .expect("stops")
            .iter()
            .find(|stop| stop["id"] == 16)
            .expect("stop 16 exists");
        assert_eq!(montre["name"], "Montre 8'");
        assert_eq!(montre["manual"], "First Manual");
        assert_eq!(montre["on"], true, "the bound key drew the stop it names");

        respond(&state, &Method::Post, "/api/control/unbind?slot=0");
        assert!(state_json(&state).contains("\"controls\":[]"));

        respond(&state, &Method::Post, "/api/control/bind?slot=0&action=nonsense");
        assert!(state_json(&state).contains("\"controls\":[]"), "refused");
    }

    /// The tremulant shape edits live and lands in the snapshot in the
    /// file's vocabulary (Hz, cents); a wave tremulant refuses — its
    /// undulation is recorded in the samples.
    #[test]
    fn tremulant_shape_edits_live() {
        let Some(state) = demo_state() else { return };
        let shaped = respond(
            &state,
            &Method::Post,
            "/api/trem/params?rate=3.5&depth=15&ramp=1.2&wobble=6",
        );
        assert_eq!(shaped.status_code().0, 200);
        let body = state_json(&state);
        assert!(
            body.contains("\"rate\":3.5") && body.contains("\"depth\":15.0"),
            "snapshot carries the new shape: {body}"
        );
        {
            let mut state = state.lock().expect("state poisoned");
            assert!(
                (state.trems[0].params.rate_hz - 3.5).abs() < 1e-6,
                "the live control changed"
            );
            state.trems[0].wave = true;
        }
        let refused = respond(&state, &Method::Post, "/api/trem/params?rate=5");
        assert_eq!(refused.status_code().0, 400, "wave tremulants have no shape");
    }

    /// A sample set's own organ takes the player's settings — wiring,
    /// room, whole-instrument pitch — into its own file, and refuses
    /// every change to the instrument itself with 409: nothing touches
    /// the engine or the file. Saving it under a different name copies
    /// the file, switches to the copy, and from then on instrument
    /// edits land there; the original keeps its mark and its bytes.
    #[test]
    fn an_adopted_organ_takes_settings_and_refuses_instrument_edits() {
        let Some(state) = demo_state() else { return };
        let demo = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../testsets/grandorgue-demo/demo.organ");
        let dir = std::env::temp_dir().join("aristide-adopted-guard-test");
        let _ = std::fs::remove_dir_all(&dir);
        let organ = aristide_formats::grandorgue::load(&demo).expect("demo parses").organ;
        let canonical = demo.canonicalize().expect("canonicalizes");
        let file = crate::config::create_wrapper_organ(&dir, "Demo", &canonical, &organ, None)
            .expect("inventory written");
        {
            let mut state = state.lock().expect("state");
            state.composite_path = Some(file.clone());
            state.setup.adopted = true;
            state.setup.sources = vec![("Demo".into(), file.clone())];
            state.organ_key = "Demo".into();
        }
        assert!(state_json(&state).contains("\"adopted\":true"));

        // The player's own settings land — live and in the set's file.
        let ok = respond(&state, &Method::Post, "/api/tuning?reference_hz=415");
        assert_eq!(ok.status_code().0, 200, "the whole-instrument pitch is the player's");
        let ok = respond(
            &state,
            &Method::Post,
            "/api/midi/bind?manual=0&slot=0&device=Computer%20keyboard",
        );
        assert_eq!(ok.status_code().0, 200, "so is the wiring");
        let ok = respond(&state, &Method::Post, "/api/reverb?wet=0.5&persist=1");
        assert_eq!(ok.status_code().0, 200, "and the room");
        let body = state_json(&state);
        assert!(body.contains("\"adopted\":true"), "still the set's own organ: {body}");
        assert!(body.contains("\"hz\":415"), "the pitch changed live: {body}");
        assert!(body.contains("Computer keyboard"), "the binding is live: {body}");
        let text = std::fs::read_to_string(&file).expect("reads");
        assert!(text.contains("415"), "the pitch is written into the set's own file: {text}");
        assert!(text.contains("Computer keyboard"), "so is the binding: {text}");
        assert!(text.contains("0.5"), "and the room: {text}");

        // The instrument itself is refused, whole and untouched.
        let before = text;
        let json_before = state_json(&state);
        let refused = respond(&state, &Method::Post, "/api/tuning?manual=0&reference_hz=430");
        assert_eq!(refused.status_code().0, 409, "a division's own tuning is voicing");
        let refused = respond(&state, &Method::Post, "/api/tuning?stop=0&reference_hz=430");
        assert_eq!(refused.status_code().0, 409, "so is a stop's");
        let refused = respond(&state, &Method::Post, "/api/organ/manual/add?name=Solo");
        assert_eq!(refused.status_code().0, 409, "a structural edit is refused");
        let refused = respond(&state, &Method::Post, "/api/organ/rename?name=Other");
        assert_eq!(refused.status_code().0, 409, "so is renaming the set's own organ");
        let refused = respond(&state, &Method::Post, "/api/trem/params?rate=5");
        assert_eq!(refused.status_code().0, 409, "and reshaping its tremulant");
        assert_eq!(state_json(&state), json_before, "nothing changed live");
        assert_eq!(std::fs::read_to_string(&file).expect("reads"), before, "nor on disk");
        assert!(state.lock().expect("state").pending_load.is_none(), "no rebuild queued");

        let played = respond(&state, &Method::Post, "/api/stop?id=0&on=1");
        assert_eq!(played.status_code().0, 200, "playing is not a change");
        let live = respond(&state, &Method::Post, "/api/reverb?wet=0.6");
        assert_eq!(live.status_code().0, 200, "a slider mid-drag is not a change either");

        let same = respond(&state, &Method::Post, "/api/organ/save_as?name=Demo");
        assert_eq!(same.status_code().0, 400, "the copy needs a name of its own");
        let saved = respond(&state, &Method::Post, "/api/organ/save_as?name=My%20Demo");
        assert_eq!(saved.status_code().0, 200);
        let body = state_json(&state);
        assert!(body.contains("\"adopted\":false"), "the copy takes edits: {body}");
        assert!(body.contains("\"organ\":\"My Demo\""), "and carries the new name: {body}");
        let copy = state.lock().expect("state").composite_path.clone().expect("a file");
        assert_ne!(copy, file);
        assert_eq!(copy.parent(), file.parent(), "the copy sits beside the original");
        assert_eq!(std::fs::read_to_string(&file).expect("reads"), before, "original untouched");
        {
            let state = state.lock().expect("state");
            assert_eq!(state.organ_key, "My Demo", "wiring is keyed by the new name");
            assert_eq!(state.midi_config.library[0].name, "My Demo", "the library learned it");
            assert!(!state.setup.adopted);
        }

        let ok = respond(&state, &Method::Post, "/api/tuning?manual=0&reference_hz=430");
        assert_eq!(ok.status_code().0, 200, "instrument edits land on the copy");
        let text = std::fs::read_to_string(&copy).expect("reads");
        assert!(text.contains("430"), "written into the copy: {text}");
        assert_eq!(std::fs::read_to_string(&file).expect("reads"), before, "never the original");
    }

    /// The organ-pane editor endpoints against a real inventory file:
    /// each edit answers 200, writes its line, and queues a rebuild;
    /// bad edits answer 400 with the reason and leave the file alone.
    #[test]
    fn the_editor_endpoints_write_the_file_and_queue_a_reload() {
        let Some(state) = demo_state() else { return };
        let demo = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../testsets/grandorgue-demo/demo.organ");
        let dir = std::env::temp_dir().join("aristide-editor-endpoints-test");
        let _ = std::fs::remove_dir_all(&dir);
        let organ = aristide_formats::grandorgue::load(&demo).expect("demo parses").organ;
        let canonical = demo.canonicalize().expect("canonicalizes");
        let file =
            crate::config::create_wrapper_organ(&dir, "Editor Test", &canonical, &organ, None)
                .expect("inventory written");
        state.lock().expect("state").composite_path = Some(file.clone());
        // The tests run no main loop; each edit queues a rebuild that
        // must be cleared before the next edit is allowed.
        let settle = |state: &Arc<Mutex<State>>| {
            let mut state = state.lock().expect("state");
            assert!(state.pending_load.is_some(), "a rebuild was queued");
            state.pending_load = None;
            state.loading = None;
        };

        let ok = respond(
            &state,
            &Method::Post,
            "/api/organ/manual/add?name=Solo&low=48&high=84",
        );
        assert_eq!(ok.status_code().0, 200);
        settle(&state);
        let text = std::fs::read_to_string(&file).expect("reads");
        assert!(text.contains("name = \"Solo\""), "the manual was declared: {text}");

        let dup = respond(&state, &Method::Post, "/api/organ/manual/add?name=solo");
        assert_eq!(dup.status_code().0, 400, "duplicate names are refused");

        // A hex layout belongs to microtonal manuals only, and unlike
        // the kind it is a live edit: a reset answers 200 with no
        // rebuild queued — the console redraws from the next snapshot.
        let hex = respond(&state, &Method::Post, "/api/organ/manual/hex?manual=1&right=2");
        assert_eq!(hex.status_code().0, 400, "hand keyboards have no hex field");
        let hex = respond(&state, &Method::Post, "/api/organ/manual/hex?manual=1&reset=1");
        assert_eq!(hex.status_code().0, 200);
        assert!(
            state.lock().expect("state").pending_load.is_none(),
            "a hex edit is live — it must not queue a rebuild"
        );
        let hex = respond(&state, &Method::Post, "/api/organ/manual/hex?manual=99&reset=1");
        assert_eq!(hex.status_code().0, 400, "unknown manuals are refused");

        let renamed = respond(
            &state,
            &Method::Post,
            "/api/organ/manual/rename?manual=1&name=Grand",
        );
        assert_eq!(renamed.status_code().0, 200);
        settle(&state);
        let text = std::fs::read_to_string(&file).expect("reads");
        assert!(text.contains("name = \"Grand\""), "{text}");
        assert!(text.contains("on = \"Grand\""), "its pulls followed: {text}");

        let boxed = respond(&state, &Method::Post, "/api/organ/enclosure/add?name=Box");
        assert_eq!(boxed.status_code().0, 200);
        settle(&state);
        let assigned = respond(
            &state,
            &Method::Post,
            "/api/organ/enclosure/assign?enclosure=Box&stop=16&in=1",
        );
        assert_eq!(assigned.status_code().0, 200);
        settle(&state);
        let text = std::fs::read_to_string(&file).expect("reads");
        assert!(text.contains("[[enclosure]]"), "{text}");
        assert!(text.contains("Montre 8'"), "the stop joined the box by name: {text}");

        let unpulled = respond(&state, &Method::Post, "/api/organ/unpull?stop=16");
        assert_eq!(unpulled.status_code().0, 200);
        settle(&state);
        let def: aristide_formats::instrument::Definition =
            toml::from_str(&std::fs::read_to_string(&file).expect("reads")).expect("parses");
        assert!(
            !def.stops.iter().any(|pull| pull.stop == "Montre 8'"),
            "the pull line is gone: {}",
            std::fs::read_to_string(&file).expect("reads")
        );

        let offerings = respond(&state, &Method::Get, "/api/organ/offerings");
        assert_eq!(offerings.status_code().0, 200);
        use std::io::Read;
        let mut body = String::new();
        offerings
            .into_reader()
            .read_to_string(&mut body)
            .expect("reads");
        assert!(body.contains("\"alias\":\"s1\""), "{body}");
        assert!(
            body.contains("\"name\":\"Montre 8'\",\"pulled\":false"),
            "the unpulled stop is offered again: {body}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The per-stop editors: rename and voicing land live (no rebuild
    /// queued — the label changes now, held keys re-speak now) and in
    /// the file; re-sourcing rewrites the pull and rebuilds. The
    /// snapshot carries provenance and pitch for every stop, which is
    /// what the console's right-click popover shows.
    #[test]
    fn per_stop_endpoints_edit_live_and_structurally() {
        let Some(state) = demo_state() else { return };
        let demo = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../testsets/grandorgue-demo/demo.organ");
        let dir = std::env::temp_dir().join("aristide-per-stop-endpoints-test");
        let _ = std::fs::remove_dir_all(&dir);
        let organ = aristide_formats::grandorgue::load(&demo).expect("demo parses").organ;
        let canonical = demo.canonicalize().expect("canonicalizes");
        let file =
            crate::config::create_wrapper_organ(&dir, "Stop Editor", &canonical, &organ, None)
                .expect("inventory written");
        state.lock().expect("state").composite_path = Some(file.clone());

        // The snapshot says where every stop came from and how it
        // speaks — Montre 8' is s1's own, an 8' with no trim.
        let value: serde_json::Value =
            serde_json::from_str(&state_json(&state)).expect("valid JSON");
        let montre = value["stops"]
            .as_array()
            .expect("stops")
            .iter()
            .find(|stop| stop["id"] == 16)
            .expect("stop 16 exists")
            .clone();
        assert_eq!(montre["src"]["from"], "s1");
        assert_eq!(montre["src"]["manual"], "First Manual");
        assert_eq!(montre["src"]["stop"], "Montre 8'");
        assert_eq!(montre["pitch"]["own"], false);
        let native = montre["pitch"]["native"].as_f64().expect("a footage");
        assert!((native - 8.0).abs() < 0.5, "an 8' reads as one: {native}");

        // Rename: live — the snapshot changes NOW, nothing rebuilds —
        // and the pull line carries the new label.
        let renamed = respond(
            &state,
            &Method::Post,
            "/api/organ/stop/rename?stop=16&name=Diapason%208",
        );
        assert_eq!(renamed.status_code().0, 200);
        assert!(
            state.lock().expect("state").pending_load.is_none(),
            "a rename is a label — no rebuild"
        );
        assert!(state_json(&state).contains("\"name\":\"Diapason 8\""));
        let text = std::fs::read_to_string(&file).expect("reads");
        assert!(text.contains("rename = \"Diapason 8\""), "{text}");

        // Voicing: live too, echoed in the snapshot, written as the
        // stop's own [[voicing.adjust]] rule under its console name.
        let voiced = respond(
            &state,
            &Method::Post,
            "/api/organ/stop/voice?stop=16&footage=4&cents=2.5&gain=-3",
        );
        assert_eq!(voiced.status_code().0, 200);
        assert!(state.lock().expect("state").pending_load.is_none(), "voicing is live");
        let value: serde_json::Value =
            serde_json::from_str(&state_json(&state)).expect("valid JSON");
        let montre = value["stops"]
            .as_array()
            .expect("stops")
            .iter()
            .find(|stop| stop["id"] == 16)
            .expect("still there")
            .clone();
        assert_eq!(montre["pitch"]["footage"], 4.0);
        assert_eq!(montre["pitch"]["cents"], 2.5);
        assert_eq!(montre["pitch"]["gain"], -3.0);
        assert_eq!(montre["pitch"]["own"], true);
        let text = std::fs::read_to_string(&file).expect("reads");
        assert!(text.contains("[[voicing.adjust]]"), "{text}");
        assert!(text.contains("stops = [\"Diapason 8\"]"), "{text}");
        assert!(text.contains("pitch = 4"), "{text}");

        // A mixture has no single footage to override; cents still do.
        let plein_jeu = value["stops"]
            .as_array()
            .expect("stops")
            .iter()
            .find(|stop| stop["name"] == "Plein jeu III")
            .expect("the mixture")["id"]
            .as_u64()
            .expect("an id");
        assert!(value["stops"]
            .as_array()
            .expect("stops")
            .iter()
            .any(|stop| stop["name"] == "Plein jeu III" && stop["pitch"]["native"].is_null()));
        let refused = respond(
            &state,
            &Method::Post,
            &format!("/api/organ/stop/voice?stop={plein_jeu}&footage=4"),
        );
        assert_eq!(refused.status_code().0, 400, "a mixture refuses footage");
        let ok = respond(
            &state,
            &Method::Post,
            &format!("/api/organ/stop/voice?stop={plein_jeu}&cents=-4"),
        );
        assert_eq!(ok.status_code().0, 200, "cents still tune a mixture");

        // Neutral values take the rule out of the file again.
        let reset = respond(&state, &Method::Post, "/api/organ/stop/voice?stop=16&reset=1");
        assert_eq!(reset.status_code().0, 200);
        let text = std::fs::read_to_string(&file).expect("reads");
        assert!(!text.contains("stops = [\"Diapason 8\"]"), "{text}");

        // The knob engraving: live like the rename, a value in the
        // snapshot and a line in the file; auto takes both away again.
        let labeled = respond(
            &state,
            &Method::Post,
            "/api/organ/stop/label?stop=16&label=8%20Fuss",
        );
        assert_eq!(labeled.status_code().0, 200);
        assert!(
            state.lock().expect("state").pending_load.is_none(),
            "an engraving is a label — no rebuild"
        );
        assert!(state_json(&state).contains("\"label\":\"8 Fuss\""));
        let text = std::fs::read_to_string(&file).expect("reads");
        assert!(text.contains("pitch_label = \"8 Fuss\""), "{text}");
        let hidden = respond(&state, &Method::Post, "/api/organ/stop/label?stop=16&label=");
        assert_eq!(hidden.status_code().0, 200);
        assert!(
            state_json(&state).contains("\"label\":\"\""),
            "the empty engraving is a value, not an absence"
        );
        let auto = respond(&state, &Method::Post, "/api/organ/stop/label?stop=16&auto=1");
        assert_eq!(auto.status_code().0, 200);
        assert!(!state_json(&state).contains("\"label\""));
        assert!(!std::fs::read_to_string(&file).expect("reads").contains("pitch_label"));

        // Re-sourcing is structural: the pull line now names the other
        // stop, keeps the drawknob's label, and the organ rebuilds.
        let retargeted = respond(
            &state,
            &Method::Post,
            "/api/organ/stop/source?stop=16&from=s1&manual=Second%20Manual&source_stop=Trompette%208%27",
        );
        assert_eq!(retargeted.status_code().0, 200);
        {
            let mut state = state.lock().expect("state");
            assert!(state.pending_load.is_some(), "re-sourcing rebuilds");
            state.pending_load = None;
            state.loading = None;
        }
        let def: aristide_formats::instrument::Definition =
            toml::from_str(&std::fs::read_to_string(&file).expect("reads")).expect("parses");
        let pull = def
            .stops
            .iter()
            .find(|pull| pull.rename.as_deref() == Some("Diapason 8"))
            .expect("the retargeted pull keeps the label");
        assert_eq!(pull.stop, "Trompette 8'");
        assert_eq!(pull.manual.as_deref(), Some("Second Manual"));
        assert!(
            !def.stops.iter().any(|pull| pull.stop == "Montre 8'"),
            "the old pull is gone"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Drawknob order and panel size are display facts with the panel-
    /// placement contract: live (no rebuild), written to the file's
    /// [console] section, and name-keyed order follows a stop rename.
    #[test]
    fn stop_order_and_panel_size_are_live_layout_facts() {
        let Some(state) = demo_state() else { return };
        let demo = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../testsets/grandorgue-demo/demo.organ");
        let dir = std::env::temp_dir().join("aristide-order-endpoints-test");
        let _ = std::fs::remove_dir_all(&dir);
        let organ = aristide_formats::grandorgue::load(&demo).expect("demo parses").organ;
        let canonical = demo.canonicalize().expect("canonicalizes");
        let file =
            crate::config::create_wrapper_organ(&dir, "Order Test", &canonical, &organ, None)
                .expect("inventory written");
        state.lock().expect("state").composite_path = Some(file.clone());

        // First Manual's stops in snapshot order; reorder them reversed.
        let ids = |body: &str| -> Vec<u64> {
            let value: serde_json::Value = serde_json::from_str(body).expect("valid JSON");
            value["stops"]
                .as_array()
                .expect("stops")
                .iter()
                .filter(|stop| stop["midx"] == 1)
                .map(|stop| stop["id"].as_u64().expect("id"))
                .collect()
        };
        let before = ids(&state_json(&state));
        let reversed: Vec<String> = before.iter().rev().map(u64::to_string).collect();
        let ordered = respond(
            &state,
            &Method::Post,
            &format!("/api/organ/stop/order?manual=1&stops={}", reversed.join(",")),
        );
        assert_eq!(ordered.status_code().0, 200);
        assert!(
            state.lock().expect("state").pending_load.is_none(),
            "an order is display only — no rebuild"
        );
        let after = ids(&state_json(&state));
        assert_eq!(
            after,
            before.iter().rev().copied().collect::<Vec<_>>(),
            "the snapshot deals the stops out in the new order"
        );
        let def: aristide_formats::instrument::Definition =
            toml::from_str(&std::fs::read_to_string(&file).expect("reads")).expect("parses");
        let listed = def.console.order.get("First Manual").expect("order written");
        assert_eq!(listed.len(), before.len());

        // The order is name-keyed; a rename must carry its entry.
        assert_eq!(listed[0], "Cornett III", "reversed: the last stop leads");
        let montre = respond(
            &state,
            &Method::Post,
            "/api/organ/stop/rename?stop=16&name=Diapason%208",
        );
        assert_eq!(montre.status_code().0, 200);
        let def: aristide_formats::instrument::Definition =
            toml::from_str(&std::fs::read_to_string(&file).expect("reads")).expect("parses");
        let listed = def.console.order.get("First Manual").expect("still ordered");
        assert!(listed.iter().any(|name| name == "Diapason 8"), "{listed:?}");
        assert!(listed.iter().all(|name| name != "Montre 8'"));
        let after_rename = ids(&state_json(&state));
        assert_eq!(after, after_rename, "the renamed knob kept its place");

        // A stray id refuses — the reorder raced an edit.
        let stray = respond(
            &state,
            &Method::Post,
            "/api/organ/stop/order?manual=1&stops=0",
        );
        assert_eq!(stray.status_code().0, 400, "stop 0 is the Pedal's");

        // Sizing a jamb rides panel placement: w/h in the layout, the
        // file, and later plain moves keep the size.
        let sized = respond(
            &state,
            &Method::Post,
            "/api/organ/panel/place?panel=jamb%3AFirst%20Manual&x=0.1&y=0.2&w=0.25&h=0.5",
        );
        assert_eq!(sized.status_code().0, 200);
        let value: serde_json::Value =
            serde_json::from_str(&state_json(&state)).expect("valid JSON");
        assert_eq!(value["layout"]["jamb:First Manual"]["w"], 0.25);
        assert_eq!(value["layout"]["jamb:First Manual"]["h"], 0.5);
        let moved = respond(
            &state,
            &Method::Post,
            "/api/organ/panel/place?panel=jamb%3AFirst%20Manual&x=0.3&y=0.2",
        );
        assert_eq!(moved.status_code().0, 200);
        let value: serde_json::Value =
            serde_json::from_str(&state_json(&state)).expect("valid JSON");
        assert_eq!(value["layout"]["jamb:First Manual"]["x"], 0.3);
        assert_eq!(
            value["layout"]["jamb:First Manual"]["h"], 0.5,
            "a plain move never un-sizes"
        );
        let text = std::fs::read_to_string(&file).expect("reads");
        assert!(text.contains("w = 0.25"), "{text}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The per-coupler editors: the snapshot carries every coupler's
    /// routes; a rename lands live (the map, since the demo's couplers
    /// are the set's own) with control bindings following; a routes
    /// edit materializes the coupler as this organ's define and
    /// rebuilds; adding defines a new one and duplicate names refuse.
    #[test]
    fn coupler_endpoints_edit_live_and_structurally() {
        let Some(state) = demo_state() else { return };
        let demo = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../testsets/grandorgue-demo/demo.organ");
        let dir = std::env::temp_dir().join("aristide-coupler-endpoints-test");
        let _ = std::fs::remove_dir_all(&dir);
        let organ = aristide_formats::grandorgue::load(&demo).expect("demo parses").organ;
        let canonical = demo.canonicalize().expect("canonicalizes");
        let file =
            crate::config::create_wrapper_organ(&dir, "Coupler Editor", &canonical, &organ, None)
                .expect("inventory written");
        state.lock().expect("state").composite_path = Some(file.clone());

        // Every coupler's routes ride the snapshot, manuals as indexes.
        let value: serde_json::Value =
            serde_json::from_str(&state_json(&state)).expect("valid JSON");
        let couplers = value["couplers"].as_array().expect("couplers");
        let (idx, ii_i) = couplers
            .iter()
            .enumerate()
            .find(|(_, c)| c["name"] == "II/I")
            .expect("the demo's II/I");
        let route = &ii_i["routes"][0];
        assert_eq!(route["from"], 1, "listens on the first manual's keys");
        assert_eq!(route["to"], 2, "sounds the second manual's stops");
        assert_eq!(route["shift"], 0);

        // A control binding speaks the coupler's name; the rename must
        // carry it along or the button would silently unwire.
        respond(
            &state,
            &Method::Post,
            "/api/control/bind?slot=0&action=coupler%3AII%2FI&device=BCF2000&trigger=cc%2016",
        );
        let renamed = respond(
            &state,
            &Method::Post,
            &format!("/api/organ/coupler/rename?idx={idx}&name=R%C3%A9cit%2FG.O."),
        );
        assert_eq!(renamed.status_code().0, 200);
        assert!(
            state.lock().expect("state").pending_load.is_none(),
            "a coupler rename is a label — no rebuild"
        );
        assert!(state_json(&state).contains("\"name\":\"Récit/G.O.\""));
        // Adoption inventoried the set's couplers as defines, so the
        // rename lands on the define's own name line — no map needed.
        let def: aristide_formats::instrument::Definition =
            toml::from_str(&std::fs::read_to_string(&file).expect("reads")).expect("parses");
        assert!(def.couplers.define.iter().any(|d| d.name == "Récit/G.O."), "renamed in place");
        assert!(def.couplers.rename.is_empty());
        {
            let state = state.lock().expect("state");
            let organ = state.midi_config.organs.get("test organ").expect("wiring");
            assert_eq!(organ.controls[0].action, "coupler:Récit/G.O.");
        }

        // Editing its routes rewrites the define in place — and a
        // rebuild (the materialize path for carried couplers is the
        // config tests' business; an adopted organ's couplers are all
        // defines already).
        let routes = "%5B%7B%22from%22%3A1%2C%22to%22%3A2%2C%22shift%22%3A-12%7D%5D"; // [{"from":1,"to":2,"shift":-12}]
        let edited = respond(
            &state,
            &Method::Post,
            &format!("/api/organ/coupler/routes?idx={idx}&routes={routes}"),
        );
        assert_eq!(edited.status_code().0, 200);
        {
            let mut state = state.lock().expect("state");
            assert!(state.pending_load.is_some(), "a route edit rebuilds");
            state.pending_load = None;
            state.loading = None;
        }
        let def: aristide_formats::instrument::Definition =
            toml::from_str(&std::fs::read_to_string(&file).expect("reads")).expect("parses");
        assert!(def.couplers.drop.is_empty(), "its own define, nothing to drop");
        let own = def
            .couplers
            .define
            .iter()
            .find(|d| d.name == "Récit/G.O.")
            .expect("the edited define");
        assert_eq!(own.routes[0].shift, -12);
        assert_eq!(own.routes.len(), 1);

        // A brand-new coupler appends a define; a taken name refuses.
        let added = respond(
            &state,
            &Method::Post,
            &format!("/api/organ/coupler/add?name=Great%20sub&routes={routes}"),
        );
        assert_eq!(added.status_code().0, 200);
        {
            let mut state = state.lock().expect("state");
            state.pending_load = None;
            state.loading = None;
        }
        let dup = respond(
            &state,
            &Method::Post,
            &format!("/api/organ/coupler/add?name=II%2FP&routes={routes}"),
        );
        assert_eq!(dup.status_code().0, 400, "a console coupler already owns that name");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Couplers seat in jambs (a `c` token in the rank order), link
    /// into one action that moves together, and pull coupled keys down
    /// for the display — all live console facts, no rebuild; deleting
    /// a define rewrites the file and rebuilds.
    #[test]
    fn couplers_seat_link_and_pull_keys() {
        let Some(state) = demo_state() else { return };
        let demo = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../testsets/grandorgue-demo/demo.organ");
        let dir = std::env::temp_dir().join("aristide-coupler-console-test");
        let _ = std::fs::remove_dir_all(&dir);
        let organ = aristide_formats::grandorgue::load(&demo).expect("demo parses").organ;
        let canonical = demo.canonicalize().expect("canonicalizes");
        let file =
            crate::config::create_wrapper_organ(&dir, "Coupler Console", &canonical, &organ, None)
                .expect("inventory written");
        state.lock().expect("state").composite_path = Some(file.clone());
        let snapshot = || -> serde_json::Value {
            serde_json::from_str(&state_json(&state)).expect("valid JSON")
        };
        let coupler_idx = |value: &serde_json::Value, name: &str| -> u64 {
            value["couplers"]
                .as_array()
                .expect("couplers")
                .iter()
                .find(|c| c["name"] == name)
                .unwrap_or_else(|| panic!("coupler {name}"))["idx"]
                .as_u64()
                .expect("idx")
        };

        // Seat II/I in First Manual's jamb, between its stops: the
        // rank order takes the `c` token, the snapshot deals the
        // manual's rank with the coupler in place and stamps the
        // coupler's seat — live, no rebuild.
        let value = snapshot();
        let ii_i = coupler_idx(&value, "II/I");
        let stops: Vec<String> = value["stops"]
            .as_array()
            .expect("stops")
            .iter()
            .filter(|stop| stop["midx"] == 1)
            .map(|stop| format!("s{}", stop["id"].as_u64().expect("id")))
            .collect();
        let mut items = stops.clone();
        items.insert(1, format!("c{ii_i}"));
        let seated = respond(
            &state,
            &Method::Post,
            &format!("/api/organ/stop/order?manual=1&items={}", items.join(",")),
        );
        assert_eq!(seated.status_code().0, 200);
        assert!(
            state.lock().expect("state").pending_load.is_none(),
            "a seat is display only — no rebuild"
        );
        let value = snapshot();
        let manual = &value["manuals"].as_array().expect("manuals")[1];
        let rank: Vec<&str> =
            manual["rank"].as_array().expect("rank").iter().map(|t| t.as_str().unwrap()).collect();
        assert_eq!(rank[1], format!("c{ii_i}"), "the coupler sits second: {rank:?}");
        assert_eq!(
            value["couplers"].as_array().expect("couplers")[ii_i as usize]["midx"], 1,
            "the coupler knows its jamb"
        );
        let text = std::fs::read_to_string(&file).expect("reads");
        assert!(text.contains("coupler:II/I"), "{text}");

        // A coupler has one seat: listing it in another division's
        // rank unseats it from the first.
        let moved = respond(
            &state,
            &Method::Post,
            &format!("/api/organ/stop/order?manual=2&items=c{ii_i}"),
        );
        assert_eq!(moved.status_code().0, 200);
        let value = snapshot();
        assert_eq!(value["couplers"].as_array().expect("couplers")[ii_i as usize]["midx"], 2);
        let first: Vec<&str> = value["manuals"].as_array().expect("manuals")[1]["rank"]
            .as_array()
            .expect("rank")
            .iter()
            .map(|t| t.as_str().unwrap())
            .collect();
        assert!(!first.contains(&format!("c{ii_i}").as_str()), "unseated: {first:?}");

        // Linking two couplers makes them one action: engaging either
        // engages both, releasing either releases both — live, in the
        // snapshot's `linked`, and in the file's [couplers] link.
        let ii_p = coupler_idx(&value, "II/P");
        let linked = respond(
            &state,
            &Method::Post,
            &format!("/api/organ/coupler/link?idx={ii_i}&with={ii_p}&on=1"),
        );
        assert_eq!(linked.status_code().0, 200);
        assert!(state.lock().expect("state").pending_load.is_none(), "a link is live");
        let on = |value: &serde_json::Value, idx: u64| -> bool {
            value["couplers"].as_array().expect("couplers")[idx as usize]["on"]
                .as_bool()
                .expect("on")
        };
        respond(&state, &Method::Post, &format!("/api/coupler?idx={ii_i}&on=1"));
        let value = snapshot();
        assert!(on(&value, ii_i) && on(&value, ii_p), "linked couplers move together");
        assert_eq!(value["couplers"].as_array().expect("couplers")[ii_i as usize]["linked"][0], ii_p);
        respond(&state, &Method::Post, &format!("/api/coupler?idx={ii_p}&on=0"));
        let value = snapshot();
        assert!(!on(&value, ii_i) && !on(&value, ii_p), "either rocker releases both");
        let def: aristide_formats::instrument::Definition =
            toml::from_str(&std::fs::read_to_string(&file).expect("reads")).expect("parses");
        assert_eq!(def.couplers.link, [["II/I", "II/P"]]);
        let unlinked = respond(
            &state,
            &Method::Post,
            &format!("/api/organ/coupler/link?idx={ii_i}&with={ii_p}&on=0"),
        );
        assert_eq!(unlinked.status_code().0, 200);
        respond(&state, &Method::Post, &format!("/api/coupler?idx={ii_i}&on=1"));
        let value = snapshot();
        assert!(on(&value, ii_i) && !on(&value, ii_p), "unlinked couplers part ways");

        // With II/I engaged, a First Manual key pulls the coupled key
        // down on the Second Manual — display only, `coupled` beside
        // `held`, and never a note the sound path didn't already play.
        respond(&state, &Method::Post, "/api/note?manual=1&key=60&on=1");
        let value = snapshot();
        let manuals = value["manuals"].as_array().expect("manuals");
        assert_eq!(manuals[1]["held"][0], 60);
        assert_eq!(manuals[2]["coupled"][0], 60, "the coupled key goes down too");
        assert!(manuals[2]["held"].as_array().is_none_or(|held| held.is_empty()));

        // The organ default turns the display off; a per-coupler
        // "always" override brings this coupler's back.
        let off = respond(&state, &Method::Post, "/api/organ/coupled_keys?on=0");
        assert_eq!(off.status_code().0, 200);
        let value = snapshot();
        assert_eq!(value["coupled_keys"], false);
        assert!(value["manuals"][2]["coupled"].as_array().is_none_or(|c| c.is_empty()));
        let always = respond(
            &state,
            &Method::Post,
            &format!("/api/organ/coupler/keys?idx={ii_i}&mode=always"),
        );
        assert_eq!(always.status_code().0, 200);
        let value = snapshot();
        assert_eq!(value["manuals"][2]["coupled"][0], 60);
        assert_eq!(value["couplers"].as_array().expect("couplers")[ii_i as usize]["keys"], "always");
        let def: aristide_formats::instrument::Definition =
            toml::from_str(&std::fs::read_to_string(&file).expect("reads")).expect("parses");
        assert_eq!(def.console.coupled_keys, Some(false));
        assert_eq!(def.console.coupler_keys.get("II/I").map(String::as_str), Some("always"));
        respond(&state, &Method::Post, "/api/note?manual=1&key=60&on=0");

        // Deleting a define rewrites the file — every reference goes
        // with it — and rebuilds.
        let removed = respond(
            &state,
            &Method::Post,
            &format!("/api/organ/coupler/remove?idx={ii_i}"),
        );
        assert_eq!(removed.status_code().0, 200);
        {
            let mut state = state.lock().expect("state");
            assert!(state.pending_load.is_some(), "deleting a define rebuilds");
            state.pending_load = None;
            state.loading = None;
        }
        let def: aristide_formats::instrument::Definition =
            toml::from_str(&std::fs::read_to_string(&file).expect("reads")).expect("parses");
        assert!(!def.couplers.define.iter().any(|d| d.name == "II/I"), "the define is gone");
        assert!(def.console.coupler_keys.is_empty(), "its override went with it");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Panel placement (`/api/organ/panel/place`) is cosmetic console
    /// geometry, not organ structure: it writes `[console.layout]` and
    /// updates the snapshot, but — unlike every editor above — never
    /// queues a rebuild. Manual renames/removals carry a placed panel's
    /// key along or drop it.
    #[test]
    fn panel_placement_persists_without_reloading() {
        let Some(state) = demo_state() else { return };

        // No organ at all: refused outright.
        let none = tone_state();
        let no_organ = respond(
            &none,
            &Method::Post,
            "/api/organ/panel/place?panel=shoes&x=0&y=0",
        );
        assert_eq!(no_organ.status_code().0, 400);

        // An organ that isn't saved as a file yet (the fixture's
        // `demo_state` never sets `composite_path`): refused the same
        // way every other structural editor is.
        let implicit = respond(
            &state,
            &Method::Post,
            "/api/organ/panel/place?panel=shoes&x=0&y=0",
        );
        assert_eq!(implicit.status_code().0, 400);

        let demo = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../testsets/grandorgue-demo/demo.organ");
        let dir = std::env::temp_dir().join("aristide-panel-place-test");
        let _ = std::fs::remove_dir_all(&dir);
        let organ = aristide_formats::grandorgue::load(&demo).expect("demo parses").organ;
        let canonical = demo.canonicalize().expect("canonicalizes");
        let file =
            crate::config::create_wrapper_organ(&dir, "Panel Test", &canonical, &organ, None)
                .expect("inventory written");
        state.lock().expect("state").composite_path = Some(file.clone());

        // Place a keyboard panel: 200, the snapshot carries it, the
        // file on disk has the line — and nothing was queued for
        // reload, unlike every structural edit.
        let ok = respond(
            &state,
            &Method::Post,
            "/api/organ/panel/place?panel=keyboard%3AFirst%20Manual&x=0.4375&y=0.3125",
        );
        assert_eq!(ok.status_code().0, 200);
        assert!(
            state.lock().expect("state").pending_load.is_none(),
            "a cosmetic edit never queues a rebuild"
        );
        let value: serde_json::Value =
            serde_json::from_str(&state_json(&state)).expect("valid JSON");
        assert_eq!(value["layout"]["keyboard:First Manual"]["x"], 0.4375);
        assert_eq!(value["layout"]["keyboard:First Manual"]["y"], 0.3125);
        let text = std::fs::read_to_string(&file).expect("reads");
        assert!(text.contains("[console.layout]"), "{text}");
        assert!(text.contains("\"keyboard:First Manual\""), "{text}");
        assert!(text.contains("0.4375") && text.contains("0.3125"), "{text}");

        // Out-of-range coordinates are clamped, not refused.
        let clamped = respond(
            &state,
            &Method::Post,
            "/api/organ/panel/place?panel=shoes&x=-0.5&y=1.75",
        );
        assert_eq!(clamped.status_code().0, 200);
        let value: serde_json::Value =
            serde_json::from_str(&state_json(&state)).expect("valid JSON");
        assert_eq!(value["layout"]["shoes"]["x"], 0.0);
        assert_eq!(value["layout"]["shoes"]["y"], 1.0);

        // Invalid panel ids are refused outright, nothing written.
        let no_such_manual = respond(
            &state,
            &Method::Post,
            "/api/organ/panel/place?panel=keyboard%3ANope&x=0&y=0",
        );
        assert_eq!(no_such_manual.status_code().0, 400);
        let garbage = respond(
            &state,
            &Method::Post,
            "/api/organ/panel/place?panel=garbage&x=0&y=0",
        );
        assert_eq!(garbage.status_code().0, 400);
        assert!(
            !std::fs::read_to_string(&file).expect("reads").contains("Nope"),
            "the refused edit never touched the file"
        );

        // Placing on a manual's jamb too, then renaming that manual:
        // both its panel keys — keyboard and jamb — follow the rename,
        // in the snapshot and in the file.
        respond(
            &state,
            &Method::Post,
            "/api/organ/panel/place?panel=jamb%3AFirst%20Manual&x=0.1&y=0.2",
        );
        let renamed = respond(
            &state,
            &Method::Post,
            "/api/organ/manual/rename?manual=1&name=Grand",
        );
        assert_eq!(renamed.status_code().0, 200);
        {
            // No main loop runs in tests: a structural edit still
            // queues a rebuild (unlike panel placement), so it must be
            // cleared before the next editor call is allowed.
            let mut state = state.lock().expect("state");
            assert!(state.pending_load.is_some(), "the rename is structural");
            state.pending_load = None;
            state.loading = None;
        }
        let value: serde_json::Value =
            serde_json::from_str(&state_json(&state)).expect("valid JSON");
        let layout = value["layout"].as_object().expect("layout object");
        assert!(layout.contains_key("keyboard:Grand"), "{layout:?}");
        assert!(layout.contains_key("jamb:Grand"), "{layout:?}");
        assert!(
            !layout.contains_key("keyboard:First Manual") && !layout.contains_key("jamb:First Manual"),
            "the old keys are gone: {layout:?}"
        );
        let text = std::fs::read_to_string(&file).expect("reads");
        assert!(text.contains("\"keyboard:Grand\""), "{text}");
        assert!(text.contains("\"jamb:Grand\""), "{text}");

        // Placing on, then removing, an unrelated manual drops just
        // its own panel key.
        respond(
            &state,
            &Method::Post,
            "/api/organ/panel/place?panel=keyboard%3ASecond%20Manual&x=0.6&y=0.6",
        );
        let removed = respond(&state, &Method::Post, "/api/organ/manual/remove?manual=2");
        assert_eq!(removed.status_code().0, 200);
        {
            let mut state = state.lock().expect("state");
            assert!(state.pending_load.is_some());
            state.pending_load = None;
            state.loading = None;
        }
        let value: serde_json::Value =
            serde_json::from_str(&state_json(&state)).expect("valid JSON");
        let layout = value["layout"].as_object().expect("layout object");
        assert!(
            !layout.contains_key("keyboard:Second Manual"),
            "the removed manual's panel is gone: {layout:?}"
        );
        assert!(layout.contains_key("keyboard:Grand"), "the untouched manual survives");
        assert!(layout.contains_key("shoes"));

        // Reloading the file straight through the format layer (the
        // test runs no main loop to reload the live console) carries
        // exactly what survived: shoes, and Grand's two panels.
        let reloaded = aristide_formats::instrument::load(&file).expect("reloads");
        let mut keys: Vec<&str> = reloaded.console_layout.keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(keys, ["jamb:Grand", "keyboard:Grand", "shoes"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Couplers reaching for pipes a division hasn't got is off, and
    /// switchable without editing the set.
    #[test]
    fn coupler_repitch_is_off_and_switchable() {
        let Some(state) = demo_state() else { return };
        assert!(state_json(&state).contains("\"coupler_repitch\":false"));
        respond(&state, &Method::Post, "/api/couplers?repitch=1");
        assert!(state_json(&state).contains("\"coupler_repitch\":true"));
        respond(&state, &Method::Post, "/api/couplers");
        assert!(state_json(&state).contains("\"coupler_repitch\":false"));
    }

    /// The computer keyboard is assigned through the same bind and
    /// unbind calls as any MIDI device — there is no separate path.
    #[test]
    fn the_computer_keyboard_binds_like_any_device() {
        let Some(state) = demo_state() else { return };
        respond(
            &state,
            &Method::Post,
            "/api/midi/bind?manual=1&slot=0&device=Computer%20keyboard",
        );
        assert!(state_json(&state).contains("\"keyboard\":{\"manual\":1,\"transpose\":0"));

        // The action endpoint shifts it the way a binding would.
        respond(
            &state,
            &Method::Post,
            "/api/action?do=octave-up&device=Computer%20keyboard",
        );
        assert!(
            state_json(&state).contains("\"keyboard\":{\"manual\":1,\"transpose\":12"),
            "the keyboard is shifted, not the division"
        );

        // Picking it in another manual's dropdown is a second job for
        // the same keyboard: nothing moves until the player answers
        // the keep-both / replace / cancel dialog.
        respond(
            &state,
            &Method::Post,
            "/api/midi/bind?manual=2&slot=0&device=Computer%20keyboard",
        );
        let body = state_json(&state);
        assert!(
            body.contains("\"conflict\":{\"kind\":\"input\",\"device\":\"Computer keyboard\""),
            "the second manual is asked about, not assumed: {body}"
        );
        assert!(
            body.contains("\"keyboard\":{\"manual\":1,\"transpose\":12"),
            "and until then nothing has changed: {body}"
        );

        // Replace moves it, the shift carried along.
        respond(&state, &Method::Post, "/api/conflict?choice=replace");
        let body = state_json(&state);
        assert!(!body.contains("\"conflict\""), "the question is answered");
        assert!(
            body.contains("\"keyboard\":{\"manual\":2,\"transpose\":12"),
            "replaced means moved, shift and all: {body}"
        );
        assert!(
            body.matches("\"device\":\"Computer keyboard\"").count() == 1,
            "one manual, not two: {body}"
        );

        // Asked again and told to keep both, it plays both manuals.
        respond(
            &state,
            &Method::Post,
            "/api/midi/bind?manual=1&slot=0&device=Computer%20keyboard",
        );
        respond(&state, &Method::Post, "/api/conflict?choice=keep");
        let body = state_json(&state);
        assert!(
            body.matches("\"device\":\"Computer keyboard\"").count() == 2,
            "kept means both rows stand: {body}"
        );

        // Removing the rows detaches it, like unplugging any device.
        respond(&state, &Method::Post, "/api/midi/unbind?manual=2&slot=0");
        respond(&state, &Method::Post, "/api/midi/unbind?manual=1&slot=0");
        let body = state_json(&state);
        assert!(
            !body.contains("\"keyboard\":{"),
            "detached, the keyboard plays nothing: {body}"
        );
    }

    #[test]
    fn port_names_survive_the_query_string() {
        assert_eq!(unescape("Midiplus%20AKM320%20MIDI%201"), "Midiplus AKM320 MIDI 1");
        assert_eq!(unescape("R%C3%A9cit+din"), "Récit din");
        assert_eq!(unescape("100%"), "100%", "a stray percent is kept");
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
            .find(|(_, name, _, _, _)| *name == "Montre 8'")
            .map(|(id, _, _, _, _)| *id)
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
        let (_, clacks) = console.set_coupler(0, true);
        assert_eq!(clacks.len(), 1, "coupler noise mapped");
        let (unclack, _) = console.set_coupler(0, false);
        assert_eq!(unclack, vec![clacks[0].handle]);

        // Disabling kills open noise voices and mutes future toggles.
        console.set_drawn(montre, true);
        let kills = console.set_noises(false, 0.7);
        assert!(!kills.is_empty(), "open noise voices killed on disable");
        let (_, silent) = console.set_drawn(montre, false);
        assert!(silent.is_empty());
    }

    /// A state with no organ loaded — what the picker talks to.
    fn tone_state() -> Arc<Mutex<State>> {
        let (_engine, handle) =
            aristide_engine::Engine::new(48000.0, std::sync::Arc::new(Default::default()));
        Arc::new(Mutex::new(State {
            engine: handle,
            control: Control::Tone,
            midi_ports: Vec::new(),
            midi_config: Default::default(),
            config_path: None,
            organ_key: String::new(),
            suggested_channels: Vec::new(),
            learn: None,
            control_learn: None,
            pending: None,
            key_bindings: Vec::new(),
            keyboard: Vec::new(),
            live_notes: std::collections::HashMap::new(),
            channel_bend: std::collections::HashMap::new(),
            ltn_cache: std::collections::HashMap::new(),
            trems: Vec::new(),
            setter_armed: false,
            stepper_frame: 0,
            crescendo_stage: 0,
            master_gain: 0.178,
            reverb_wet: None,
            expression_cc: 11,
            composite_path: None,
            setup: Default::default(),
            provenance: Default::default(),
            stop_voicing: Default::default(),
            stop_labels: Default::default(),
            stop_order: Default::default(),
            compass_overrides: Vec::new(),
            pending_load: None,
            loading: None,
            load_error: None,
            load_warnings: Vec::new(),
            layout: Default::default(),
            coupled_keys: true,
            coupler_key_modes: Default::default(),
        }))
    }

    /// Loading over the API only queues: the main thread owns the
    /// stream, so the endpoint hands it the request and the snapshot
    /// says "loading" until it lands.
    #[test]
    fn loading_an_organ_is_queued_for_the_main_thread() {
        let state = tone_state();
        let file = std::env::temp_dir().join("aristide-load-queue-test.organ");
        std::fs::write(&file, "[Organ]").expect("fixture written");
        let other = std::env::temp_dir().join("aristide-load-queue-other.organ");
        std::fs::write(&other, "[Organ]").expect("fixture written");

        let missing = respond(&state, &Method::Post, "/api/organ/load");
        assert_eq!(missing.status_code().0, 400, "a path is required");
        let absent = respond(
            &state,
            &Method::Post,
            "/api/organ/load?path=/no/such/place.organ",
        );
        assert_eq!(absent.status_code().0, 400, "the path must exist");

        let url = format!("/api/organ/load?path={}", file.display());
        let queued = respond(&state, &Method::Post, &url);
        assert_eq!(queued.status_code().0, 200);
        {
            let state = state.lock().expect("state poisoned");
            let pending = state.pending_load.as_ref().expect("request queued");
            assert_eq!(pending.paths, vec![file.clone()]);
            assert!(!pending.initial, "a picker load must not exit on failure");
            assert!(state.loading.is_some(), "snapshot narrates the load");
        }
        assert!(state_json(&state).contains("\"loading\":"));

        // Last pick wins: a second pick replaces the queued one — the
        // player changing their mind mid-load must never be refused
        // into a silent no-op.
        let replaced = respond(
            &state,
            &Method::Post,
            &format!("/api/organ/load?path={}", other.display()),
        );
        assert_eq!(replaced.status_code().0, 200);
        {
            let state = state.lock().expect("state poisoned");
            let pending = state.pending_load.as_ref().expect("request queued");
            assert_eq!(pending.paths, vec![other.clone()]);
        }
        let _ = std::fs::remove_file(&file);
        let _ = std::fs::remove_file(&other);
    }

    /// The blank-organ endpoint needs a real name. (Creation itself is
    /// proven in `config::tests` — here would write into the user's
    /// config; a load already running no longer refuses, it is
    /// replaced — last pick wins, same as `/api/organ/load`.)
    #[test]
    fn a_blank_organ_needs_a_name() {
        let state = tone_state();
        let missing = respond(&state, &Method::Post, "/api/organ/new");
        assert_eq!(missing.status_code().0, 400, "a name is required");
        let blank = respond(&state, &Method::Post, "/api/organ/new?name=%20%20");
        assert_eq!(blank.status_code().0, 400, "whitespace is not a name");
    }

    /// The whole blank-organ story at the file level: a fresh blank
    /// file takes a sample set as a source, and the offerings endpoint
    /// then lists that source's manuals and stops for the drawer.
    #[test]
    fn a_blank_organ_offers_an_added_sources_stops() {
        let demo = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../testsets/grandorgue-demo/demo.organ");
        if !demo.is_file() {
            eprintln!("skipping: demo set not present");
            return;
        }
        let dir = std::env::temp_dir().join("aristide-blank-offerings-test");
        let _ = std::fs::remove_dir_all(&dir);
        let path = crate::config::create_blank_organ(&dir, "Fresh").expect("blank organ");

        let alias = crate::config::append_composite_source(&path, &demo).expect("source added");
        let body = organ::offerings_json(&path).expect("offerings read");
        assert!(body.contains(&format!("\"alias\":\"{alias}\"")), "offered: {body}");
        assert!(body.contains("\"stops\":[{\"name\":"), "stops listed: {body}");
        assert!(!body.contains("\"error\""), "no source error: {body}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_snapshot_offers_the_library_and_forget_removes() {
        let dir = std::env::temp_dir().join("aristide-library-forget-test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("fixture dir");
        let set = dir.join("demo.organ");
        std::fs::write(&set, "[Organ]").expect("fixture set");
        let state = tone_state();
        state
            .lock()
            .expect("state poisoned")
            .midi_config
            .remember("Demo", &set);
        let body = state_json(&state);
        assert!(
            body.contains(&format!(
                "\"library\":[{{\"name\":\"Demo\",\"path\":{}}}]",
                json_string(&set.display().to_string())
            )),
            "library present: {body}"
        );
        assert!(!body.contains("\"organ\":"), "no organ is loaded");

        respond(
            &state,
            &Method::Post,
            &format!("/api/library/forget?path={}", set.display()),
        );
        assert!(state_json(&state).contains("\"library\":[]"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Recent lists only organs whose files exist. A deleted set drops
    /// off the list without being forgotten, so an organ on an
    /// unplugged drive comes back when the drive does.
    #[test]
    fn recent_hides_a_missing_file_until_it_returns() {
        let dir = std::env::temp_dir().join("aristide-library-missing-test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("fixture dir");
        let kept = dir.join("kept.organ");
        let gone = dir.join("gone.organ");
        std::fs::write(&kept, "[Organ]").expect("fixture set");
        std::fs::write(&gone, "[Organ]").expect("fixture set");
        let state = tone_state();
        {
            let mut state = state.lock().expect("state poisoned");
            state.midi_config.remember("Kept", &kept);
            state.midi_config.remember("Gone", &gone);
        }
        assert!(state_json(&state).contains("\"name\":\"Gone\""), "listed while present");

        std::fs::remove_file(&gone).expect("delete fixture");
        let body = state_json(&state);
        assert!(!body.contains("\"name\":\"Gone\""), "hidden once deleted: {body}");
        assert!(body.contains("\"name\":\"Kept\""), "the others stay: {body}");
        assert_eq!(
            state.lock().expect("state poisoned").midi_config.library.len(),
            2,
            "hidden, not forgotten"
        );

        std::fs::write(&gone, "[Organ]").expect("restore fixture");
        assert!(
            state_json(&state).contains("\"name\":\"Gone\""),
            "back when the file is"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Renaming a sample-set organ: the name lands in the set's
    /// sidecar (the set itself is untouched), the assignments keyed by
    /// the old name move to the new one, and the library entry keeps
    /// its path while showing the new name.
    #[test]
    fn renaming_a_set_organ_follows_everywhere() {
        let Some(state) = demo_state() else { return };
        let dir = std::env::temp_dir().join("aristide-rename-set-test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("fixture dir");
        let set = dir.join("village.organ");
        std::fs::write(&set, "[Organ]").expect("fixture set");
        {
            let mut state = state.lock().expect("state poisoned");
            state.setup.sources = vec![("test organ".into(), set.clone())];
            state.midi_config.remember("test organ", &set);
            state.midi_config.set_input(
                "test organ",
                "First Manual",
                0,
                crate::config::Input {
                    device: "Test Keys".into(),
                    channel: Some(1),
                    low: None,
                    high: None,
                    transpose: 0,
                    bend: None,
                    map: None,
                },
            );
        }

        let missing = respond(&state, &Method::Post, "/api/organ/rename");
        assert_eq!(missing.status_code().0, 400, "a name is required");
        let blank = respond(&state, &Method::Post, "/api/organ/rename?name=%20");
        assert_eq!(blank.status_code().0, 400, "whitespace is not a name");

        let renamed = respond(
            &state,
            &Method::Post,
            "/api/organ/rename?name=Chapel%20Royal",
        );
        assert_eq!(renamed.status_code().0, 200);
        let body = state_json(&state);
        assert!(body.contains("\"organ\":\"Chapel Royal\""), "live: {body}");
        assert!(
            body.contains("{\"name\":\"Chapel Royal\",\"path\":"),
            "library shows the new name: {body}"
        );
        {
            let state = state.lock().expect("state poisoned");
            assert_eq!(state.organ_key, "Chapel Royal");
            assert!(
                state.midi_config.organ("test organ").is_none(),
                "nothing left under the old key"
            );
            assert_eq!(
                state.midi_config.inputs("Chapel Royal", "First Manual")[0].device,
                "Test Keys",
                "the wiring moved with the name"
            );
            assert_eq!(
                state.midi_config.library[0].path, set,
                "the library still points at the same file"
            );
            assert_eq!(state.setup.sources[0].0, "Chapel Royal");
        }
        let sidecar = aristide_formats::sidecar::load_for(&set)
            .expect("sidecar parses")
            .expect("sidecar written");
        assert_eq!(sidecar.name, "Chapel Royal", "the rename is durable");
        assert_eq!(
            std::fs::read_to_string(&set).expect("set readable"),
            "[Organ]",
            "the set itself is never touched"
        );

        // Renaming to the current name is a no-op, not an error.
        let same = respond(
            &state,
            &Method::Post,
            "/api/organ/rename?name=Chapel%20Royal",
        );
        assert_eq!(same.status_code().0, 200);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A combination assembled ad hoc has no file to carry a name, so
    /// renaming it is refused with the way out; and with no organ at
    /// all there is nothing to rename.
    #[test]
    fn renaming_needs_an_organ_that_lives_in_a_file() {
        let state = tone_state();
        let refused = respond(&state, &Method::Post, "/api/organ/rename?name=Ghost");
        assert_eq!(refused.status_code().0, 400, "no organ, no rename");

        let Some(state) = demo_state() else { return };
        {
            let mut state = state.lock().expect("state poisoned");
            state.setup.sources = vec![
                ("A".into(), "/sets/a.organ".into()),
                ("B".into(), "/sets/b.organ".into()),
            ];
            state.setup.implicit = true;
        }
        let implicit = respond(&state, &Method::Post, "/api/organ/rename?name=Duo");
        assert_eq!(implicit.status_code().0, 400);
        assert_eq!(
            state.lock().expect("state poisoned").organ_key,
            "test organ",
            "a refused rename changes nothing"
        );
    }

    /// Renaming a composite organ rewrites the `name` in its own file —
    /// which keeps its path, so the library entry and the wiring the
    /// file owns keep working — and the file loads back under the new
    /// name.
    #[test]
    fn renaming_a_composite_edits_its_file_and_keeps_references() {
        let Some(state) = demo_state() else { return };
        let demo = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../testsets/grandorgue-demo/demo.organ");
        let path = std::env::temp_dir().join("aristide-rename-composite-test.toml");
        let _ = std::fs::remove_file(&path);
        {
            let mut state = state.lock().expect("state poisoned");
            let names: Vec<String> = state.manual_names();
            state.setup.sources = vec![("Demo".into(), demo.clone())];
            state.setup.pulls = names
                .iter()
                .enumerate()
                .map(|(index, name)| (0, name.clone(), index))
                .collect();
            state.setup.implicit = true;
            state.midi_config.remember("Demo", &demo);
        }
        respond(
            &state,
            &Method::Post,
            &format!(
                "/api/organ/save?path={}",
                path.display().to_string().replace('/', "%2F")
            ),
        );
        // Wiring learned before the rename, stored under the old name.
        respond(
            &state,
            &Method::Post,
            "/api/midi/bind?manual=0&slot=0&device=Test%20Keys&ch=4",
        );

        let renamed = respond(
            &state,
            &Method::Post,
            "/api/organ/rename?name=St%20Fictive",
        );
        assert_eq!(renamed.status_code().0, 200);

        let saved = aristide_formats::instrument::load(&path).expect("still loads");
        assert_eq!(saved.organ.name, "St Fictive", "the file carries the new name");
        assert_eq!(
            saved.midi.inputs[0].device, "Test Keys",
            "the wiring the file owns survives the rename"
        );
        {
            let state = state.lock().expect("state poisoned");
            assert_eq!(state.organ_key, "St Fictive");
            let canonical = path.canonicalize().expect("saved file exists");
            let entry = state
                .midi_config
                .library
                .iter()
                .find(|entry| entry.path == canonical)
                .expect("the composite is in the library");
            assert_eq!(entry.name, "St Fictive");
            let demo_entry = state
                .midi_config
                .library
                .iter()
                .find(|entry| entry.path != canonical)
                .expect("the source set stays in the library");
            assert_eq!(demo_entry.name, "Demo", "the source set keeps its name");
            let (_, inputs) = state
                .midi_config
                .assignments("St Fictive")
                .next()
                .expect("the wiring moved with the name");
            assert_eq!(inputs[0].device, "Test Keys");
        }
        // Wiring changed after the rename lands under the new key and
        // in the file.
        respond(
            &state,
            &Method::Post,
            "/api/midi/bind?manual=0&slot=0&device=Test%20Keys&ch=7",
        );
        let saved = aristide_formats::instrument::load(&path).expect("reloads");
        assert_eq!(saved.midi.inputs[0].channel, Some(7));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn browse_lists_directories_and_loadable_files_only() {
        let dir = std::env::temp_dir().join("aristide-browse-test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("sub")).expect("fixture dir");
        for name in [
            "set.organ",
            "combo.toml",
            "hw.Organ_Hauptwerk_xml",
            "19edo.scl",
            "white.kbm",
            "readme.txt",
            ".hidden.organ",
        ] {
            std::fs::write(dir.join(name), "").expect("fixture file");
        }
        let body = organ::browse_json(&dir).expect("browses");
        assert!(body.contains("\"sub\""), "subdirectory listed: {body}");
        assert!(body.contains("set.organ"), "sample set listed");
        assert!(body.contains("combo.toml"), "composite listed");
        assert!(body.contains("hw.Organ_Hauptwerk_xml"), "Hauptwerk listed");
        assert!(body.contains("19edo.scl"), "Scala scale listed");
        assert!(body.contains("white.kbm"), "keyboard mapping listed");
        assert!(!body.contains("readme.txt"), "other files are noise");
        assert!(!body.contains(".hidden.organ"), "dotfiles skipped");
        assert!(
            body.find("\"sub\"").unwrap() < body.find("combo.toml").unwrap(),
            "directories come first"
        );
        assert!(organ::browse_json(&dir.join("nowhere")).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Every console-editor write against a real inventory leaves the
    /// file loadable — including a stop-move racing a rename's rebuild,
    /// which once wrote the stale manual name into a [[move]] and left
    /// the organ permanently unloadable (hit in the field, 2026-08-21).
    #[test]
    fn every_edit_leaves_the_file_loadable() {
        let Some(state) = demo_state() else { return };
        let demo = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../testsets/grandorgue-demo/demo.organ");
        let dir = std::env::temp_dir().join("aristide-repro-night-edits");
        let _ = std::fs::remove_dir_all(&dir);
        let organ = aristide_formats::grandorgue::load(&demo).expect("demo parses").organ;
        let canonical = demo.canonicalize().expect("canonicalizes");
        let file = crate::config::create_wrapper_organ(&dir, "Repro", &canonical, &organ, None)
            .expect("inventory written");
        state.lock().expect("state").composite_path = Some(file.clone());

        let clear = |state: &Mutex<State>| {
            let mut state = state.lock().expect("state");
            state.pending_load = None;
            state.loading = None;
        };
        let check = |step: &str| {
            if let Err(err) = aristide_formats::instrument::load(&file) {
                let text = std::fs::read_to_string(&file).unwrap_or_default();
                panic!("unloadable after {step}: {err}\n---\n{text}");
            }
        };

        let steps: &[(&str, &str)] = &[
            ("place keyboard", "/api/organ/panel/place?panel=keyboard%3AFirst%20Manual&x=0.4&y=0.3"),
            ("place jamb", "/api/organ/panel/place?panel=jamb%3AFirst%20Manual&x=0.1&y=0.3"),
            ("move a stop", "/api/organ/move?stop=16&manual=0"),
            ("enclose: add box named like the manual", "/api/organ/enclosure/add?name=First%20Manual"),
            ("enclose: assign a stop", "/api/organ/enclosure/assign?enclosure=First%20Manual&stop=17&in=1"),
            ("rename the manual", "/api/organ/manual/rename?manual=1&name=Grand"),
        ];
        for (step, url) in steps {
            let response = respond(&state, &Method::Post, url);
            assert_eq!(response.status_code().0, 200, "{step} refused");
            clear(&state);
            check(step);
        }

        // The race: the rename above rewrote the file ("Grand") and its
        // rebuild is still in flight — the live console still says
        // "First Manual". A stop-move now must be refused, or it would
        // write the stale name into a [[move]].
        {
            let mut state = state.lock().expect("state");
            state.pending_load = Some(crate::LoadRequest {
                paths: vec![file.clone()],
                stops: Vec::new(),
                initial: false,
            });
        }
        let raced = respond(&state, &Method::Post, "/api/organ/move?stop=18&manual=0");
        assert_eq!(raced.status_code().0, 400, "a mid-rebuild move is refused");
        clear(&state);
        check("move raced against a rename's rebuild");

        // A file that already carries a stale move (written before the
        // guard existed) still loads: the move is skipped with a
        // warning, never a failure.
        let mut text = std::fs::read_to_string(&file).expect("reads");
        text.push_str("\n[[move]]\nstop = \"Montre 8'\"\nfrom = \"First Manual\"\nto = \"Pedal\"\n");
        std::fs::write(&file, text).expect("writes");
        let healed = aristide_formats::instrument::load(&file).expect("a stale move never bricks the organ");
        assert!(
            healed.warnings.iter().any(|w| w.contains("skipped")),
            "the stale move is called out: {:?}",
            healed.warnings
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
