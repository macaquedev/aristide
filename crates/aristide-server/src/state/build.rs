//! Build edits: one stop's rule, live from the next note (held keys
//! re-speak the stop) and in the organ's file.

use aristide_engine::Command;
use aristide_model::StopId;

use super::{Control, State};
use crate::config;
use crate::rule::{self, Rule};

impl State {
    /// Replace one stop's rule; `None` makes it conventional again.
    pub fn set_stop_rule(&mut self, stop: StopId, next: Option<Rule>) -> Result<(), String> {
        let State {
            engine, control, ..
        } = &mut *self;
        let Control::Organ(console) = control else {
            return Err("no organ is loaded".into());
        };
        let (stopped, starts) = console.set_stop_rule(stop, next)?;
        for handle in stopped {
            engine.send(Command::StopVoice { handle });
        }
        for start in starts {
            engine.send(start.command());
        }
        let current = console.stop_has_rule(stop).then(|| console.stop_rule(stop));
        let Some((manual, name)) = console
            .stop_states()
            .iter()
            .find(|(id, ..)| *id == stop)
            .map(|(_, name, manual, _, _)| (manual.to_string(), name.to_string()))
        else {
            return Ok(());
        };
        let def = current.and_then(|rule| rule::to_def(console, stop, &rule));
        match self.composite_path.clone() {
            Some(path) => {
                if let Err(err) = config::write_composite_rule(&path, &manual, &name, def.as_ref()) {
                    tracing::warn!("stop rule not saved: {err}");
                }
            }
            None => tracing::warn!("stop rule not saved: this organ has no file yet"),
        }
        Ok(())
    }
}
