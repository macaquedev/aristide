//! Stop rules: what one key press on a stop sounds. A conventional
//! stop is the one-event case — its own pipes, at pitch, from key-down
//! until release. A rule adds events: each takes pipes from a stop of
//! this organ (all its ranks or one of them), at a pitch offset and a
//! level, starting at a timestamp after key-down or key-up and ending
//! at release or at a later timestamp.
//!
//! Timestamps are shared: every onset and finite ending names one, so
//! moving a timestamp moves everything attached to it. `down` and `up`
//! are the fixed 0 ms timestamps of the two timelines.

use aristide_formats::sidecar::{RuleDef, RuleEventDef, RuleStampDef};
use aristide_model::{RankId, StopId};
use serde::{Deserialize, Serialize};

use crate::console::Console;

pub const DOWN: &str = "down";
pub const UP: &str = "up";
/// Longest offset a timestamp may take — any musical canon trick.
pub const MAX_MS: f64 = 30_000.0;
/// Pitch offsets span eight octaves either way.
pub const MAX_CENTS: f64 = 9_600.0;
pub const LEVEL_DB: std::ops::RangeInclusive<f64> = -60.0..=12.0;
/// More events than this is a mistake, not an instrument: each one is
/// a voice per key on every press.
pub const MAX_EVENTS: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Anchor {
    Down,
    Up,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Stamp {
    pub id: String,
    pub anchor: Anchor,
    pub ms: f64,
}

/// Where an event's pipes come from: one stop's pipework as that stop
/// lays it out on its keyboard, every rank of it or only `rank`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Source {
    pub stop: StopId,
    pub rank: Option<RankId>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Event {
    pub source: Source,
    pub cents: f64,
    pub level_db: f64,
    pub start: String,
    /// `None` = until release.
    pub end: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Rule {
    pub stamps: Vec<Stamp>,
    pub events: Vec<Event>,
}

/// When an event sounds, relative to the key that triggers it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Timing {
    /// Starts `delay_ms` after key-down. `end_ms` (from key-down) ends
    /// it early; `hold_ms` keeps it speaking that long past key-up.
    Down {
        delay_ms: f64,
        end_ms: Option<f64>,
        hold_ms: Option<f64>,
    },
    /// Starts `delay_ms` after key-up and ends at `end_ms` after it.
    Up { delay_ms: f64, end_ms: f64 },
}

impl Rule {
    /// The conventional stop: its own pipes, from key-down until release.
    pub fn plain(stop: StopId) -> Rule {
        Rule {
            stamps: fixed_stamps(),
            events: vec![Event {
                source: Source { stop, rank: None },
                cents: 0.0,
                level_db: 0.0,
                start: DOWN.into(),
                end: None,
            }],
        }
    }

    /// Whether this rule sounds exactly what the stop does without one.
    pub fn is_plain(&self, stop: StopId) -> bool {
        matches!(self.events.as_slice(), [event] if *event == Rule::plain(stop).events[0])
    }

    pub fn stamp(&self, id: &str) -> Option<&Stamp> {
        self.stamps.iter().find(|stamp| stamp.id == id)
    }

    /// Check the rule's shape; sources are the console's to check.
    pub fn validate(&self) -> Result<(), String> {
        for fixed in fixed_stamps() {
            if self.stamp(&fixed.id) != Some(&fixed) {
                return Err(format!("the {} timestamp is fixed at 0 ms", fixed.id));
            }
        }
        for (index, stamp) in self.stamps.iter().enumerate() {
            if stamp.id.trim().is_empty() {
                return Err("a timestamp needs an id".into());
            }
            if !(0.0..=MAX_MS).contains(&stamp.ms) {
                return Err(format!("timestamps run from 0 to {MAX_MS} ms"));
            }
            for other in &self.stamps[..index] {
                if other.id == stamp.id {
                    return Err(format!("two timestamps are called {:?}", stamp.id));
                }
                if other.anchor == stamp.anchor && (other.ms - stamp.ms).abs() < 0.5 {
                    return Err(format!("two timestamps share {} ms", stamp.ms.round()));
                }
            }
        }
        if self.events.len() > MAX_EVENTS {
            return Err(format!("a stop holds at most {MAX_EVENTS} events"));
        }
        for event in &self.events {
            if !event.cents.is_finite() || event.cents.abs() > MAX_CENTS {
                return Err(format!("pitch runs ±{MAX_CENTS} cents"));
            }
            if !LEVEL_DB.contains(&event.level_db) {
                return Err("level runs from −60 to +12 dB".into());
            }
            self.timing(event)?;
        }
        Ok(())
    }

    /// An event's timing, or why it has none.
    pub fn timing(&self, event: &Event) -> Result<Timing, String> {
        let start = self
            .stamp(&event.start)
            .ok_or_else(|| format!("no timestamp {:?}", event.start))?;
        let end = match &event.end {
            Some(id) => Some(self.stamp(id).ok_or_else(|| format!("no timestamp {id:?}"))?),
            None => None,
        };
        match (start.anchor, end) {
            (Anchor::Down, None) => Ok(Timing::Down {
                delay_ms: start.ms,
                end_ms: None,
                hold_ms: None,
            }),
            (Anchor::Down, Some(end)) if end.anchor == Anchor::Up => Ok(Timing::Down {
                delay_ms: start.ms,
                end_ms: None,
                hold_ms: (end.ms > 0.0).then_some(end.ms),
            }),
            (_, Some(end)) if end.anchor == start.anchor && end.ms > start.ms => match start.anchor {
                Anchor::Down => Ok(Timing::Down {
                    delay_ms: start.ms,
                    end_ms: Some(end.ms),
                    hold_ms: None,
                }),
                Anchor::Up => Ok(Timing::Up {
                    delay_ms: start.ms,
                    end_ms: end.ms,
                }),
            },
            (Anchor::Up, None) => Err("an event after key-up needs an ending".into()),
            _ => Err("an event must end after it starts".into()),
        }
    }
}

/// A stop by the names the console shows: its manual, then its own.
fn stop_named(console: &Console, manual: &str, stop: &str) -> Option<StopId> {
    console
        .stop_states()
        .iter()
        .find(|(_, name, on_manual, _, _)| name.eq_ignore_ascii_case(stop) && on_manual.eq_ignore_ascii_case(manual))
        .map(|(id, ..)| *id)
}

fn names_of(console: &Console, stop: StopId) -> Option<(String, String)> {
    console
        .stop_states()
        .iter()
        .find(|(id, ..)| *id == stop)
        .map(|(_, name, manual, _, _)| (manual.to_string(), name.to_string()))
}

/// The file's spelling of a stop's rule: every stop and rank by name.
pub fn to_def(console: &Console, stop: StopId, rule: &Rule) -> Option<RuleDef> {
    let (manual, name) = names_of(console, stop)?;
    let mut events = Vec::with_capacity(rule.events.len());
    for event in &rule.events {
        let (source_manual, source_stop) = names_of(console, event.source.stop)?;
        let rank = match event.source.rank {
            Some(rank) => Some(
                console
                    .stop_ranks(event.source.stop)
                    .into_iter()
                    .find(|(id, _)| *id == rank)?
                    .1
                    .to_string(),
            ),
            None => None,
        };
        events.push(RuleEventDef {
            manual: source_manual,
            stop: source_stop,
            rank,
            cents: event.cents,
            level_db: event.level_db,
            start: event.start.clone(),
            end: event.end.clone(),
        });
    }
    Some(RuleDef {
        manual,
        stop: name,
        stamps: rule
            .stamps
            .iter()
            .filter(|stamp| stamp.id != DOWN && stamp.id != UP)
            .map(|stamp| RuleStampDef {
                id: stamp.id.clone(),
                anchor: match stamp.anchor {
                    Anchor::Down => DOWN.into(),
                    Anchor::Up => UP.into(),
                },
                ms: stamp.ms,
            })
            .collect(),
        events,
    })
}

/// A file rule resolved against the loaded organ, or why it can't be.
pub fn from_def(console: &Console, def: &RuleDef) -> Result<(StopId, Rule), String> {
    let stop = stop_named(console, &def.manual, &def.stop)
        .ok_or_else(|| format!("stop {:?} on {:?} matches nothing", def.stop, def.manual))?;
    let mut stamps = fixed_stamps();
    for stamp in &def.stamps {
        let anchor = match stamp.anchor.to_ascii_lowercase().as_str() {
            DOWN => Anchor::Down,
            UP => Anchor::Up,
            other => return Err(format!("timestamp anchor {other:?} is neither down nor up")),
        };
        stamps.push(Stamp { id: stamp.id.clone(), anchor, ms: stamp.ms });
    }
    let mut events = Vec::with_capacity(def.events.len());
    for event in &def.events {
        let source = stop_named(console, &event.manual, &event.stop).ok_or_else(|| {
            format!("source stop {:?} on {:?} matches nothing", event.stop, event.manual)
        })?;
        let rank = match &event.rank {
            Some(name) => Some(
                console
                    .stop_ranks(source)
                    .into_iter()
                    .find(|(_, rank)| rank.eq_ignore_ascii_case(name))
                    .map(|(id, _)| id)
                    .ok_or_else(|| format!("rank {name:?} is not in {:?}", event.stop))?,
            ),
            None => None,
        };
        events.push(Event {
            source: Source { stop: source, rank },
            cents: event.cents,
            level_db: event.level_db,
            start: event.start.clone(),
            end: event.end.clone(),
        });
    }
    let rule = Rule { stamps, events };
    rule.validate()?;
    Ok((stop, rule))
}

fn fixed_stamps() -> Vec<Stamp> {
    vec![
        Stamp { id: DOWN.into(), anchor: Anchor::Down, ms: 0.0 },
        Stamp { id: UP.into(), anchor: Anchor::Up, ms: 0.0 },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with(stamps: &[(&str, Anchor, f64)], start: &str, end: Option<&str>) -> Rule {
        let mut rule = Rule::plain(StopId(0));
        for &(id, anchor, ms) in stamps {
            rule.stamps.push(Stamp { id: id.into(), anchor, ms });
        }
        rule.events[0].start = start.into();
        rule.events[0].end = end.map(Into::into);
        rule
    }

    #[test]
    fn a_plain_rule_is_one_event_until_release() {
        let rule = Rule::plain(StopId(3));
        assert!(rule.is_plain(StopId(3)));
        assert_eq!(
            rule.timing(&rule.events[0]),
            Ok(Timing::Down { delay_ms: 0.0, end_ms: None, hold_ms: None })
        );
    }

    #[test]
    fn timings_follow_their_timestamps() {
        let rule = with(&[("t1", Anchor::Down, 50.0)], DOWN, Some("t1"));
        assert_eq!(
            rule.timing(&rule.events[0]),
            Ok(Timing::Down { delay_ms: 0.0, end_ms: Some(50.0), hold_ms: None })
        );
        let rule = with(&[("u1", Anchor::Up, 50.0)], DOWN, Some("u1"));
        assert_eq!(
            rule.timing(&rule.events[0]),
            Ok(Timing::Down { delay_ms: 0.0, end_ms: None, hold_ms: Some(50.0) })
        );
        let rule = with(&[("u1", Anchor::Up, 50.0), ("u2", Anchor::Up, 150.0)], "u1", Some("u2"));
        assert_eq!(rule.timing(&rule.events[0]), Ok(Timing::Up { delay_ms: 50.0, end_ms: 150.0 }));
    }

    #[test]
    fn invalid_rules_are_refused() {
        assert!(with(&[], UP, None).validate().is_err(), "key-up needs an ending");
        assert!(with(&[("t1", Anchor::Down, 50.0)], "t1", Some(DOWN)).validate().is_err());
        assert!(with(&[("t1", Anchor::Down, 0.2)], DOWN, None).validate().is_err(), "duplicate time");
        assert!(with(&[("t1", Anchor::Down, 50.0)], DOWN, Some("zz")).validate().is_err());
        let mut rule = Rule::plain(StopId(0));
        rule.stamps[0].ms = 10.0;
        assert!(rule.validate().is_err(), "down is fixed");
    }
}
