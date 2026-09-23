//! Speaker routing, as the Route panel sees it: each division and stop
//! sends to named speaker groups at a level, a stop following its
//! division unless it has sends of its own.
//!
//! The engine renders every voice onto one bus and a bus feeds any
//! number of output pairs, so each distinct set of sends gets a bus of
//! its own and every source with those sends shares it. The default —
//! the main group at 0 dB — is bus 0, untouched, so an organ that never
//! opens Route renders exactly as before. Buses the organ file's older
//! `[[routing.bus]]` tables claim stay theirs.

use std::collections::{BTreeMap, HashMap};

use aristide_engine::routing::{Send, MAX_BUSES, MAX_SENDS};
use aristide_model::StopId;

/// The group every organ has: the interface's first pair.
pub const MAIN: &str = "Main";
/// Levels a send may take, in dB.
pub const LEVEL_RANGE: std::ops::RangeInclusive<f64> = -60.0..=12.0;

/// Speaker group → level in dB.
pub type Sends = BTreeMap<String, f64>;

pub fn main_only() -> Sends {
    Sends::from([(MAIN.to_string(), 0.0)])
}

/// A speaker group as this machine's Setup defines it: a name and a
/// 0-based interface channel pair.
#[derive(Debug, Clone, PartialEq)]
pub struct Speaker {
    pub name: String,
    pub left: u8,
    pub right: u8,
}

/// Sends compared at 0.01 dB, so a bus can be found by its sends.
type Key = Vec<(String, i64)>;

fn key(sends: &Sends) -> Key {
    sends
        .iter()
        .map(|(group, db)| (group.to_ascii_lowercase(), (db * 100.0).round() as i64))
        .collect()
}

/// How one stop reaches the speakers.
#[derive(Debug, Clone, PartialEq)]
pub enum Resolved<'a> {
    /// Through the organ file's `[[routing.bus]]` table of this index.
    File(usize),
    Sends(&'a Sends),
}

#[derive(Debug, Clone, Default)]
pub struct Routing {
    /// Names of the organ file's `[[routing.bus]]` tables, on buses
    /// 1..=len.
    pub file_buses: Vec<String>,
    /// Stops those tables claim, by table index.
    pub file_members: HashMap<StopId, usize>,
    /// Speaking delays from `[[voicing.delay]]`, in frames.
    pub delays: HashMap<StopId, u32>,
    /// Per manual index: the division's own sends.
    pub divisions: Vec<Option<Sends>>,
    pub stops: HashMap<StopId, Sends>,
    /// Which bus each distinct set of sends plays on now.
    assigned: Vec<(Key, u8, Sends)>,
}

static MAIN_ONLY: std::sync::LazyLock<Sends> = std::sync::LazyLock::new(main_only);

impl Routing {
    pub fn division(&self, manual: usize) -> &Sends {
        self.divisions.get(manual).and_then(Option::as_ref).unwrap_or(&MAIN_ONLY)
    }

    /// A stop's own sends win, then the organ file's bus tables, then
    /// its division.
    pub fn stop(&self, stop: StopId, manual: usize) -> Resolved<'_> {
        if let Some(sends) = self.stops.get(&stop) {
            Resolved::Sends(sends)
        } else if let Some(&table) = self.file_members.get(&stop) {
            Resolved::File(table)
        } else {
            Resolved::Sends(self.division(manual))
        }
    }

    /// Give every distinct set of sends a bus, keeping the ones already
    /// placed where they are so held notes keep their speakers; a set
    /// that replaced another takes the bus it left, so a level drag
    /// moves the held notes with it. Returns how many sets found no
    /// bus — those play through the main pair until some are merged.
    pub fn allocate(&mut self, stops: &[(StopId, usize)]) -> usize {
        let main = key(&MAIN_ONLY);
        let mut wanted: Vec<(Key, Sends)> = Vec::new();
        for &(stop, manual) in stops {
            if let Resolved::Sends(sends) = self.stop(stop, manual) {
                let k = key(sends);
                if k != main && !wanted.iter().any(|(w, _)| *w == k) {
                    wanted.push((k, sends.clone()));
                }
            }
        }
        let previous = std::mem::take(&mut self.assigned);
        let (kept, left): (Vec<_>, Vec<_>) = previous
            .into_iter()
            .partition(|(k, ..)| wanted.iter().any(|(w, _)| w == k));
        let first = self.file_buses.len() as u8 + 1;
        let freed: Vec<u8> = left.iter().map(|(_, bus, _)| *bus).collect();
        let untouched = (first..MAX_BUSES as u8)
            .filter(|bus| !kept.iter().any(|(_, b, _)| b == bus) && !freed.contains(bus));
        let mut free: Vec<u8> = freed.iter().copied().chain(untouched).collect();
        free.reverse();
        self.assigned = kept;
        let mut unplaced = 0;
        for (k, sends) in wanted {
            if self.assigned.iter().any(|(a, ..)| *a == k) {
                continue;
            }
            match free.pop() {
                Some(bus) => self.assigned.push((k, bus, sends)),
                None => unplaced += 1,
            }
        }
        unplaced
    }

    fn bus_of(&self, sends: &Sends) -> u8 {
        let k = key(sends);
        self.assigned
            .iter()
            .find(|(a, ..)| *a == k)
            .map_or(0, |(_, bus, _)| *bus)
    }

    /// The console's per-stop table: bus and speaking delay.
    pub fn stop_table(&self, stops: &[(StopId, usize)]) -> HashMap<StopId, (u8, u32)> {
        stops
            .iter()
            .filter_map(|&(stop, manual)| {
                let bus = match self.stop(stop, manual) {
                    Resolved::File(table) => table as u8 + 1,
                    Resolved::Sends(sends) => self.bus_of(sends),
                };
                let delay = self.delays.get(&stop).copied().unwrap_or(0);
                (bus != 0 || delay != 0).then_some((stop, (bus, delay)))
            })
            .collect()
    }

    /// What each placed bus should feed, with this machine's speakers.
    /// A group Setup doesn't define plays through the main pair: a rig
    /// missing a speaker should sound wrong, never silent.
    pub fn bus_sends(&self, speakers: &[Speaker]) -> Vec<(u8, [Send; MAX_SENDS], u8)> {
        self.assigned
            .iter()
            .map(|(_, bus, sends)| {
                let mut out = [Send { left: 0, right: 1, gain: 0.0 }; MAX_SENDS];
                let mut count = 0;
                for (group, db) in sends.iter().take(MAX_SENDS) {
                    let (left, right) = speaker_pair(speakers, group).unwrap_or((0, 1));
                    out[count] = Send {
                        left,
                        right,
                        gain: 10f32.powf(*db as f32 / 20.0),
                    };
                    count += 1;
                }
                (*bus, out, count as u8)
            })
            .collect()
    }

    /// Every group some source sends to.
    pub fn groups_in_use(&self) -> Vec<&str> {
        let mut groups: Vec<&str> = self
            .divisions
            .iter()
            .flatten()
            .chain(self.stops.values())
            .flat_map(|sends| sends.keys().map(String::as_str))
            .collect();
        groups.sort_by_key(|g| g.to_ascii_lowercase());
        groups.dedup_by(|a, b| a.eq_ignore_ascii_case(b));
        groups
    }
}

pub fn speaker_pair(speakers: &[Speaker], group: &str) -> Option<(u8, u8)> {
    if group.eq_ignore_ascii_case(MAIN) {
        return Some((0, 1));
    }
    speakers
        .iter()
        .find(|s| s.name.eq_ignore_ascii_case(group))
        .map(|s| (s.left, s.right))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sends(pairs: &[(&str, f64)]) -> Sends {
        pairs.iter().map(|(g, db)| (g.to_string(), *db)).collect()
    }

    const STOPS: [(StopId, usize); 3] = [(StopId(1), 0), (StopId(2), 0), (StopId(3), 1)];

    #[test]
    fn untouched_organ_stays_on_the_main_bus() {
        let mut routing = Routing::default();
        assert_eq!(routing.allocate(&STOPS), 0);
        assert!(routing.stop_table(&STOPS).is_empty());
        assert!(routing.bus_sends(&[]).is_empty());
    }

    #[test]
    fn stops_follow_their_division_unless_they_own_sends() {
        let mut routing = Routing {
            divisions: vec![Some(sends(&[("Rear", -6.0)])), None],
            ..Default::default()
        };
        routing.stops.insert(StopId(2), sends(&[("Main", 0.0), ("Rear", 0.0)]));
        routing.allocate(&STOPS);
        let table = routing.stop_table(&STOPS);
        assert_eq!(table.len(), 2, "the second division plays through Main");
        assert_ne!(table[&StopId(1)].0, table[&StopId(2)].0);
        let rear = [Speaker { name: "Rear".into(), left: 2, right: 3 }];
        let fed = routing.bus_sends(&rear);
        let (_, sent, count) = fed.iter().find(|(bus, ..)| *bus == table[&StopId(1)].0).unwrap();
        assert_eq!(*count, 1);
        assert_eq!((sent[0].left, sent[0].right), (2, 3));
        assert!((sent[0].gain - 0.501).abs() < 0.001);
    }

    #[test]
    fn a_changed_level_keeps_its_bus() {
        let mut routing = Routing {
            divisions: vec![Some(sends(&[("Rear", 0.0)])), None],
            ..Default::default()
        };
        routing.allocate(&STOPS);
        let before = routing.stop_table(&STOPS)[&StopId(1)].0;
        routing.divisions[0] = Some(sends(&[("Rear", -3.0)]));
        routing.allocate(&STOPS);
        assert_eq!(routing.stop_table(&STOPS)[&StopId(1)].0, before, "held notes follow the drag");
    }

    #[test]
    fn file_buses_keep_their_numbers_and_unknown_groups_fold_to_main() {
        let mut routing = Routing {
            file_buses: vec!["chamade".into()],
            file_members: HashMap::from([(StopId(3), 0)]),
            divisions: vec![Some(sends(&[("Gallery", 0.0)])), None],
            ..Default::default()
        };
        routing.allocate(&STOPS);
        let table = routing.stop_table(&STOPS);
        assert_eq!(table[&StopId(3)].0, 1);
        assert_eq!(table[&StopId(1)].0, 2);
        let (_, sent, count) = routing.bus_sends(&[])[0];
        assert_eq!((count, sent[0].left, sent[0].right), (1, 0, 1));
    }

    #[test]
    fn too_many_distinct_routings_are_counted() {
        let stops: Vec<(StopId, usize)> = (0..20).map(|i| (StopId(i), 0)).collect();
        let mut routing = Routing::default();
        for (i, (stop, _)) in stops.iter().enumerate() {
            routing.stops.insert(*stop, sends(&[("Main", -(i as f64))]));
        }
        // Stop 0 at 0 dB is the main bus itself.
        assert_eq!(routing.allocate(&stops), 19 - (MAX_BUSES - 1));
    }
}
