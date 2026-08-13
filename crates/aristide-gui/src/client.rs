//! The server client: state snapshots and control commands over the
//! local HTTP API. All I/O lives on a background thread (`spawn`); the
//! UI thread only exchanges messages through channels, so a slow or
//! absent server can never freeze the interface.

use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, TryRecvError};
use std::time::{Duration, Instant};

use serde::Deserialize;

/// One stop as the server reports it.
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct StopState {
    pub id: u32,
    pub name: String,
    pub manual: String,
    pub on: bool,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct CouplerState {
    pub idx: usize,
    pub name: String,
    pub on: bool,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct TuningState {
    pub temperament: String,
    pub a4: f64,
    pub transpose: i8,
}

/// One keyboard (manual or pedalboard) with its currently held keys.
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct ManualState {
    pub idx: usize,
    pub name: String,
    /// MIDI number of the lowest key.
    pub first_key: u8,
    pub key_count: u8,
    #[serde(default)]
    pub held: Vec<u8>,
}

/// A swell box; only `displayed` ones get a shoe in the UI.
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct EnclosureState {
    pub idx: usize,
    pub name: String,
    /// 0 = closed, 1 = open.
    pub value: f32,
    pub displayed: bool,
}

/// The full console state snapshot (`GET /api/state`).
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct Snapshot {
    #[serde(default)]
    pub organ: Option<String>,
    #[serde(default)]
    pub stops: Vec<StopState>,
    #[serde(default)]
    pub couplers: Vec<CouplerState>,
    #[serde(default)]
    pub manuals: Vec<ManualState>,
    #[serde(default)]
    pub enclosures: Vec<EnclosureState>,
    #[serde(default)]
    pub tremulant: bool,
    #[serde(default)]
    pub gain: f32,
    #[serde(default)]
    pub tuning: Option<TuningState>,
    /// Reverb wet level; absent when no impulse response is loaded.
    #[serde(default)]
    pub reverb: Option<f32>,
    #[serde(default)]
    pub noises: Option<NoiseState>,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct NoiseState {
    pub on: bool,
    pub vol: f32,
}

/// A control action the UI wants performed.
#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    SetStop { id: u32, on: bool },
    SetCoupler { idx: usize, on: bool },
    SetTremulant(bool),
    SetGain(f32),
    SetReverb(f32),
    SetNoises { on: bool, vol: f32 },
    SetTuning { temperament: String, a4: f64, transpose: i8 },
    SetEnclosure { idx: usize, value: f32 },
    Note { manual: usize, key: u8, on: bool },
    Panic,
}

impl Command {
    /// The POST path+query implementing this command.
    pub fn to_query(&self) -> String {
        match self {
            Command::SetStop { id, on } => {
                format!("/api/stop?id={id}&on={}", *on as u8)
            }
            Command::SetCoupler { idx, on } => {
                format!("/api/coupler?idx={idx}&on={}", *on as u8)
            }
            Command::SetTremulant(on) => format!("/api/trem?on={}", *on as u8),
            Command::SetGain(v) => format!("/api/gain?v={v}"),
            Command::SetReverb(wet) => format!("/api/reverb?wet={wet}"),
            Command::SetNoises { on, vol } => {
                format!("/api/noises?on={}&vol={vol}", *on as u8)
            }
            Command::SetTuning {
                temperament,
                a4,
                transpose,
            } => format!("/api/tuning?temperament={temperament}&a4={a4}&transpose={transpose}"),
            Command::SetEnclosure { idx, value } => {
                format!("/api/enclosure?idx={idx}&v={value}")
            }
            Command::Note { manual, key, on } => {
                format!("/api/note?manual={manual}&key={key}&on={}", *on as u8)
            }
            Command::Panic => "/api/panic".into(),
        }
    }
}

/// What the network thread reports back.
#[derive(Debug)]
pub enum Update {
    State(Snapshot),
    Error(String),
}

pub fn parse_snapshot(body: &str) -> Result<Snapshot, String> {
    serde_json::from_str(body).map_err(|e| e.to_string())
}

/// How often the state is re-polled when nothing else happens.
const POLL_INTERVAL: Duration = Duration::from_millis(250);

/// Spawn the polling/command thread. `repaint` is called whenever a new
/// update is queued so the UI wakes promptly.
///
/// Commands are dispatched the moment they arrive (`recv_timeout`, not a
/// fixed sleep) — key presses must not wait out the poll interval — and
/// every command batch is followed by an immediate state refresh so the
/// UI confirms quickly.
pub fn spawn(
    base_url: String,
    commands: Receiver<Command>,
    updates: Sender<Update>,
    repaint: impl Fn() + Send + 'static,
) {
    std::thread::Builder::new()
        .name("aristide-gui-net".into())
        .spawn(move || {
            let agent = ureq::AgentBuilder::new()
                .timeout(Duration::from_secs(2))
                .build();
            let mut next_poll = Instant::now();
            loop {
                if Instant::now() >= next_poll {
                    let result = agent
                        .get(&format!("{base_url}/api/state"))
                        .call()
                        .map_err(|e| e.to_string())
                        .and_then(|response| {
                            response.into_string().map_err(|e| e.to_string())
                        })
                        .and_then(|body| parse_snapshot(&body));
                    let update = match result {
                        Ok(snapshot) => Update::State(snapshot),
                        Err(err) => Update::Error(err),
                    };
                    if updates.send(update).is_err() {
                        return;
                    }
                    repaint();
                    next_poll = Instant::now() + POLL_INTERVAL;
                }

                let wait = next_poll.saturating_duration_since(Instant::now());
                match commands.recv_timeout(wait) {
                    Ok(first) => {
                        let mut batch = vec![first];
                        loop {
                            match commands.try_recv() {
                                Ok(command) => batch.push(command),
                                Err(TryRecvError::Empty) => break,
                                Err(TryRecvError::Disconnected) => return,
                            }
                        }
                        for command in batch {
                            let url = format!("{base_url}{}", command.to_query());
                            if let Err(err) = agent.post(&url).call() {
                                let _ = updates.send(Update::Error(err.to_string()));
                                repaint();
                            }
                        }
                        // Confirm the effect right away.
                        next_poll = Instant::now();
                    }
                    Err(RecvTimeoutError::Timeout) => {}
                    Err(RecvTimeoutError::Disconnected) => return,
                }
            }
        })
        .expect("spawn network thread");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_server_state_shape() {
        let body = r#"{"organ":"GrandOrgue demo V1",
            "stops":[{"id":1,"name":"Montre 8'","manual":"First Manual","on":true}],
            "couplers":[{"idx":0,"name":"II/I","on":false}],
            "manuals":[{"idx":0,"name":"Pedal","first_key":36,"key_count":32,"held":[38]},
                       {"idx":1,"name":"First Manual","first_key":36,"key_count":61,"held":[]}],
            "enclosures":[{"idx":0,"name":"Recit","value":1,"displayed":true},
                          {"idx":1,"name":"Grandorgue","value":0.5,"displayed":false}],
            "tremulant":false,"gain":0.35,
            "tuning":{"temperament":"meantone4","a4":415.0,"transpose":-2}}"#;
        let snapshot = parse_snapshot(body).expect("parses");
        assert_eq!(snapshot.organ.as_deref(), Some("GrandOrgue demo V1"));
        assert_eq!(snapshot.stops.len(), 1);
        assert_eq!(snapshot.stops[0].name, "Montre 8'");
        assert!(snapshot.stops[0].on);
        assert_eq!(snapshot.couplers[0].name, "II/I");
        assert_eq!(snapshot.manuals.len(), 2);
        assert_eq!(snapshot.manuals[0].key_count, 32);
        assert_eq!(snapshot.manuals[0].held, vec![38]);
        // Integer-valued swell positions (0/1) must parse as floats.
        assert_eq!(snapshot.enclosures[0].value, 1.0);
        assert!(snapshot.enclosures[0].displayed);
        assert!(!snapshot.enclosures[1].displayed);
        let tuning = snapshot.tuning.expect("tuning present");
        assert_eq!(tuning.temperament, "meantone4");
        assert_eq!(tuning.transpose, -2);
    }

    #[test]
    fn tone_mode_state_parses_too() {
        // No organ loaded: no tuning object, empty lists.
        let snapshot =
            parse_snapshot(r#"{"stops":[],"couplers":[],"tremulant":false,"gain":0.35}"#)
                .expect("parses");
        assert!(snapshot.stops.is_empty());
        assert!(snapshot.tuning.is_none());
        assert!(snapshot.manuals.is_empty());
        assert!(snapshot.enclosures.is_empty());
    }

    #[test]
    fn commands_build_the_right_queries() {
        assert_eq!(
            Command::SetStop { id: 7, on: true }.to_query(),
            "/api/stop?id=7&on=1"
        );
        assert_eq!(
            Command::SetCoupler { idx: 2, on: false }.to_query(),
            "/api/coupler?idx=2&on=0"
        );
        assert_eq!(Command::SetTremulant(true).to_query(), "/api/trem?on=1");
        assert_eq!(
            Command::SetTuning {
                temperament: "kirnberger3".into(),
                a4: 440.0,
                transpose: 3
            }
            .to_query(),
            "/api/tuning?temperament=kirnberger3&a4=440&transpose=3"
        );
        assert_eq!(
            Command::SetEnclosure { idx: 0, value: 0.25 }.to_query(),
            "/api/enclosure?idx=0&v=0.25"
        );
        assert_eq!(
            Command::Note { manual: 1, key: 60, on: true }.to_query(),
            "/api/note?manual=1&key=60&on=1"
        );
        assert_eq!(
            Command::Note { manual: 0, key: 36, on: false }.to_query(),
            "/api/note?manual=0&key=36&on=0"
        );
        assert_eq!(Command::Panic.to_query(), "/api/panic");
    }
}
