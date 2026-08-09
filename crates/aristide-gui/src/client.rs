//! The server client: state snapshots and control commands over the
//! local HTTP API. All I/O lives on a background thread (`spawn`); the
//! UI thread only exchanges messages through channels, so a slow or
//! absent server can never freeze the interface.

use std::sync::mpsc::{Receiver, Sender, TryRecvError};
use std::time::Duration;

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

/// The full console state snapshot (`GET /api/state`).
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct Snapshot {
    #[serde(default)]
    pub stops: Vec<StopState>,
    #[serde(default)]
    pub couplers: Vec<CouplerState>,
    #[serde(default)]
    pub tremulant: bool,
    #[serde(default)]
    pub gain: f32,
    #[serde(default)]
    pub tuning: Option<TuningState>,
}

/// A control action the UI wants performed.
#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    SetStop { id: u32, on: bool },
    SetCoupler { idx: usize, on: bool },
    SetTremulant(bool),
    SetGain(f32),
    SetTuning { temperament: String, a4: f64, transpose: i8 },
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
            Command::SetTuning {
                temperament,
                a4,
                transpose,
            } => format!("/api/tuning?temperament={temperament}&a4={a4}&transpose={transpose}"),
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

/// Spawn the polling/command thread. `repaint` is called whenever a new
/// update is queued so the UI wakes promptly.
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
            loop {
                // Apply pending commands first, in order.
                loop {
                    match commands.try_recv() {
                        Ok(command) => {
                            let url = format!("{base_url}{}", command.to_query());
                            if let Err(err) = agent.post(&url).call() {
                                let _ = updates.send(Update::Error(err.to_string()));
                            }
                        }
                        Err(TryRecvError::Empty) => break,
                        Err(TryRecvError::Disconnected) => return,
                    }
                }
                // Then refresh the state.
                let result = agent
                    .get(&format!("{base_url}/api/state"))
                    .call()
                    .map_err(|e| e.to_string())
                    .and_then(|response| response.into_string().map_err(|e| e.to_string()))
                    .and_then(|body| parse_snapshot(&body));
                let update = match result {
                    Ok(snapshot) => Update::State(snapshot),
                    Err(err) => Update::Error(err),
                };
                if updates.send(update).is_err() {
                    return;
                }
                repaint();
                std::thread::sleep(Duration::from_millis(250));
            }
        })
        .expect("spawn network thread");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_server_state_shape() {
        let body = r#"{"stops":[{"id":1,"name":"Montre 8'","manual":"First Manual","on":true}],
            "couplers":[{"idx":0,"name":"II/I","on":false}],
            "tremulant":false,"gain":0.35,
            "tuning":{"temperament":"meantone4","a4":415.0,"transpose":-2}}"#;
        let snapshot = parse_snapshot(body).expect("parses");
        assert_eq!(snapshot.stops.len(), 1);
        assert_eq!(snapshot.stops[0].name, "Montre 8'");
        assert!(snapshot.stops[0].on);
        assert_eq!(snapshot.couplers[0].name, "II/I");
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
    }
}
