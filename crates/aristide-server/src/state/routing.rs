//! Route panel edits and Setup's speaker groups: each lands live from
//! the next note (held notes on a re-levelled bus follow the ramp) and
//! in the organ's file or the user config.

use aristide_engine::Command;
use aristide_model::StopId;

use super::{Control, State};
use crate::config;
use crate::routing::{self, Resolved, Sends, MAIN};

/// One cell or row of the Route matrix.
pub enum RouteChange {
    /// Send to a speaker group at a level in dB.
    Send(String, f64),
    /// Stop sending to a speaker group.
    Unsend(String),
    /// Drop the source's own sends and follow what is above it.
    Follow,
}

impl State {
    /// Every stop with its manual index, as routing resolves them.
    fn stop_places(&self) -> Vec<(StopId, usize)> {
        self.console()
            .map(|console| {
                console
                    .stop_states()
                    .iter()
                    .map(|(id, _, _, manual, _)| (*id, *manual))
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn speakers(&self) -> Vec<routing::Speaker> {
        self.midi_config.speakers.iter().map(config::SpeakerDef::speaker).collect()
    }

    /// Tell the engine what every placed bus feeds.
    pub(super) fn send_bus_sends(&mut self) {
        let speakers = self.speakers();
        for (bus, sends, count) in self.routing.bus_sends(&speakers) {
            self.engine.send(Command::SetBusSends { bus, sends, count });
        }
    }

    /// Change one division's (`stop: None`) or stop's routing.
    pub fn route(
        &mut self,
        manual: usize,
        stop: Option<StopId>,
        change: RouteChange,
    ) -> Result<(), String> {
        let Control::Organ(console) = &self.control else {
            return Err("no organ is loaded".into());
        };
        let manual_name = console
            .manual_states()
            .get(manual)
            .map(|(_, name, ..)| name.to_string())
            .ok_or("no such division")?;
        let stop_name = match stop {
            Some(stop) => Some(
                console
                    .stop_states()
                    .iter()
                    .find(|(id, _, _, m, _)| *id == stop && *m == manual)
                    .map(|(_, name, ..)| name.to_string())
                    .ok_or("no such stop on that division")?,
            ),
            None => None,
        };
        let mut next = self.routing.clone();
        next.divisions.resize(console.manual_states().len(), None);
        // A source starts from what it plays now; one routed by an
        // organ file's bus table starts from nothing.
        let current: Sends = match stop {
            Some(stop) => match next.stop(stop, manual) {
                Resolved::Sends(sends) => sends.clone(),
                Resolved::File(_) => Sends::new(),
            },
            None => next.division(manual).clone(),
        };
        let own = match change {
            RouteChange::Follow => None,
            RouteChange::Send(group, db) => {
                let group = group.trim();
                if group.is_empty() {
                    return Err("name a speaker group".into());
                }
                let mut sends = current;
                sends.retain(|g, _| !g.eq_ignore_ascii_case(group));
                let name = if group.eq_ignore_ascii_case(MAIN) { MAIN } else { group };
                sends.insert(
                    name.to_string(),
                    db.clamp(*routing::LEVEL_RANGE.start(), *routing::LEVEL_RANGE.end()),
                );
                Some(sends)
            }
            RouteChange::Unsend(group) => {
                let mut sends = current;
                sends.retain(|g, _| !g.eq_ignore_ascii_case(&group));
                Some(sends)
            }
        };
        match (stop, own.clone()) {
            (Some(stop), Some(sends)) => {
                next.stops.insert(stop, sends);
            }
            (Some(stop), None) => {
                next.stops.remove(&stop);
            }
            (None, sends) => next.divisions[manual] = sends,
        }
        let places = self.stop_places();
        if next.allocate(&places) > 0 {
            return Err(format!(
                "Route holds at most {} different routings at once",
                aristide_engine::routing::MAX_BUSES - 1 - next.file_buses.len()
            ));
        }
        let table = next.stop_table(&places);
        self.routing = next;
        if let Control::Organ(console) = &mut self.control {
            console.set_stop_routing(table);
        }
        self.send_bus_sends();
        match self.composite_path.clone() {
            Some(path) => {
                if let Err(err) = config::write_composite_route(
                    &path,
                    &manual_name,
                    stop_name.as_deref(),
                    own.as_ref(),
                ) {
                    tracing::warn!("routing not saved: {err}");
                }
            }
            None => tracing::warn!("routing not saved: this organ has no file yet"),
        }
        Ok(())
    }

    /// Define, move (same name) or with `None` remove a speaker group.
    pub fn set_speaker(&mut self, name: &str, output: Option<[u8; 2]>) -> Result<(), String> {
        let name = name.trim();
        if name.is_empty() {
            return Err("the speaker group needs a name".into());
        }
        if name.eq_ignore_ascii_case(MAIN) {
            return Err("Main is always the first output pair".into());
        }
        let speakers = &mut self.midi_config.speakers;
        let at = speakers.iter().position(|s| s.name.eq_ignore_ascii_case(name));
        match (output, at) {
            (Some(output), _) if output.iter().any(|&c| c == 0 || c > 64) => {
                return Err("channels run from 1 to 64".into());
            }
            (Some(output), Some(at)) => speakers[at].output = output,
            (Some(output), None) => speakers.push(config::SpeakerDef {
                name: name.to_string(),
                output,
            }),
            (None, Some(at)) => {
                speakers.remove(at);
            }
            (None, None) => return Err("no such speaker group".into()),
        }
        self.persist();
        self.send_bus_sends();
        Ok(())
    }
}
