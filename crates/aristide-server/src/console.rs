//! Control-side console state: which stops are drawn, and which voices
//! each key press starts. This is deliberately outside the RT engine —
//! registration, couplers, and (later) microtonal key mappings all live
//! at this layer, where allocation and locking are fine.

use std::collections::HashMap;

use aristide_model::{Organ, RankId, StopId};

use crate::bank::VoiceSpec;

/// A voice the console wants started, tagged with the handle it will
/// later be stopped by.
pub struct VoiceStart {
    pub handle: u64,
    pub spec: VoiceSpec,
}

pub struct Console {
    organ: Organ,
    specs: HashMap<(RankId, u16), VoiceSpec>,
    drawn: Vec<StopId>,
    /// `channel_map[c]` = index into `organ.manuals` MIDI channel `c`
    /// plays; channels past the end wrap.
    channel_map: Vec<usize>,
    next_handle: u64,
    /// (manual index, MIDI key) → voices sounding for that key, tagged
    /// with the stop that started them (so retiring a stop can silence
    /// them). `percussive` voices are excluded (they stop themselves).
    sounding: HashMap<(usize, u8), Vec<(StopId, u64)>>,
}

impl Console {
    pub fn new(
        organ: Organ,
        specs: HashMap<(RankId, u16), VoiceSpec>,
        drawn: Vec<StopId>,
        channel_map: Vec<usize>,
    ) -> Console {
        let channel_map = if channel_map.is_empty() {
            default_channel_map(&organ)
        } else {
            channel_map
        };
        Console {
            organ,
            specs,
            drawn,
            channel_map,
            next_handle: 0,
            sounding: HashMap::new(),
        }
    }

    fn manual_index(&self, channel: u8) -> Option<usize> {
        if self.channel_map.is_empty() {
            return None;
        }
        let mapped = self.channel_map[channel as usize % self.channel_map.len()];
        (mapped < self.organ.manuals.len()).then_some(mapped)
    }

    pub fn note_on(&mut self, channel: u8, key: u8) -> Vec<VoiceStart> {
        let Some(manual_index) = self.manual_index(channel) else {
            return Vec::new();
        };
        let manual = &self.organ.manuals[manual_index];
        let Some(key_index) = key.checked_sub(manual.first_midi_note) else {
            return Vec::new();
        };
        let key_index = key_index as u16;
        if key_index >= manual.key_count {
            return Vec::new();
        }

        let mut starts = Vec::new();
        let mut held = Vec::new();
        for stop in &self.organ.stops {
            if stop.manual != manual.id || !self.drawn.contains(&stop.id) {
                continue;
            }
            for range in &stop.ranks {
                if key_index < range.first_key || key_index >= range.first_key + range.key_count {
                    continue;
                }
                let pipe = range.first_pipe + (key_index - range.first_key);
                let Some(spec) = self.specs.get(&(range.rank, pipe)) else {
                    continue;
                };
                let handle = self.next_handle;
                self.next_handle += 1;
                if !spec.percussive {
                    held.push((stop.id, handle));
                }
                starts.push(VoiceStart {
                    handle,
                    spec: *spec,
                });
            }
        }
        if !held.is_empty() {
            // Retrigger before release: stop the previous voices for
            // this key so they don't ring forever.
            self.sounding
                .entry((manual_index, key))
                .or_default()
                .extend(held);
        }
        starts
    }

    pub fn note_off(&mut self, channel: u8, key: u8) -> Vec<u64> {
        let Some(manual_index) = self.manual_index(channel) else {
            return Vec::new();
        };
        self.sounding
            .remove(&(manual_index, key))
            .unwrap_or_default()
            .into_iter()
            .map(|(_, handle)| handle)
            .collect()
    }

    /// Draw or retire a stop. Retiring returns the handles of its
    /// currently sounding voices so the caller can stop them.
    pub fn set_drawn(&mut self, stop: StopId, drawn: bool) -> Vec<u64> {
        if drawn {
            if !self.drawn.contains(&stop) {
                self.drawn.push(stop);
            }
            return Vec::new();
        }
        self.drawn.retain(|&id| id != stop);
        let mut released = Vec::new();
        for handles in self.sounding.values_mut() {
            handles.retain(|&(owner, handle)| {
                if owner == stop {
                    released.push(handle);
                    false
                } else {
                    true
                }
            });
        }
        released
    }

    /// Every stop with its manual name and drawn state, for UIs.
    pub fn stop_states(&self) -> Vec<(StopId, &str, &str, bool)> {
        self.organ
            .stops
            .iter()
            .map(|stop| {
                let manual = self
                    .organ
                    .manuals
                    .iter()
                    .find(|m| m.id == stop.manual)
                    .map(|m| m.name.as_str())
                    .unwrap_or("?");
                (
                    stop.id,
                    stop.name.as_str(),
                    manual,
                    self.drawn.contains(&stop.id),
                )
            })
            .collect()
    }

    /// Forget everything sounding (the engine is told separately).
    pub fn all_off(&mut self) {
        self.sounding.clear();
    }

    /// The manual each MIDI channel plays, for logging.
    pub fn channel_names(&self) -> Vec<(usize, &str)> {
        self.channel_map
            .iter()
            .enumerate()
            .filter_map(|(channel, &index)| {
                self.organ
                    .manuals
                    .get(index)
                    .map(|m| (channel, m.name.as_str()))
            })
            .collect()
    }
}

/// Keyboards first (in model order), pedal last: channel 0 lands on the
/// Great rather than the pedalboard, which is what a single keyboard
/// plugged into a fresh setup almost always wants.
pub fn default_channel_map(organ: &Organ) -> Vec<usize> {
    let manual_count = organ.manuals.len();
    if manual_count == 0 {
        return Vec::new();
    }
    // The GO convention: a pedalboard, when present, is manuals[0].
    let has_pedal = organ.manuals[0].id == aristide_model::ManualId(0);
    if has_pedal && manual_count > 1 {
        let mut map: Vec<usize> = (1..manual_count).collect();
        map.push(0);
        map
    } else {
        (0..manual_count).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aristide_model::{Manual, ManualId, Pipe, PipeSource, Rank, RankRange, Stop};

    fn test_console() -> Console {
        let organ = Organ {
            name: "T".into(),
            base_path: Default::default(),
            manuals: vec![Manual {
                id: ManualId(1),
                name: "Great".into(),
                first_midi_note: 36,
                key_count: 61,
            }],
            stops: vec![
                Stop {
                    id: StopId(1),
                    name: "Principal 8".into(),
                    manual: ManualId(1),
                    ranks: vec![RankRange {
                        rank: RankId(1),
                        first_key: 0,
                        key_count: 61,
                        first_pipe: 0,
                    }],
                },
                Stop {
                    id: StopId(2),
                    name: "Octave 4".into(),
                    manual: ManualId(1),
                    ranks: vec![RankRange {
                        rank: RankId(2),
                        first_key: 0,
                        key_count: 61,
                        first_pipe: 0,
                    }],
                },
            ],
            ranks: (1..=2)
                .map(|id| Rank {
                    id: RankId(id),
                    name: format!("rank {id}"),
                    windchest: 1,
                    pipes: (0..61)
                        .map(|_| Pipe {
                            nominal_frequency_hz: 440.0,
                            pitch_tuning_cents: 0.0,
                            gain_db: 0.0,
                            midi_key_number: None,
                            source: PipeSource::Silent,
                        })
                        .collect(),
                })
                .collect(),
            couplers: vec![],
        };
        let mut specs = HashMap::new();
        for rank in 1..=2u32 {
            for pipe in 0..61u16 {
                specs.insert(
                    (RankId(rank), pipe),
                    VoiceSpec {
                        sample: rank - 1,
                        rate: 1.0,
                        gain: 1.0,
                        percussive: false,
                        group: 0,
                        wind_weight: 1.0,
                        brightness: 0.02,
                    },
                );
            }
        }
        Console::new(organ, specs, vec![StopId(1), StopId(2)], Vec::new())
    }

    #[test]
    fn default_channel_map_puts_keyboards_before_pedal() {
        let manual = |id: u32, name: &str| Manual {
            id: ManualId(id),
            name: name.into(),
            first_midi_note: 36,
            key_count: 32,
        };
        let organ = Organ {
            manuals: vec![
                manual(0, "Pedal"),
                manual(1, "Great"),
                manual(2, "Swell"),
            ],
            ..Organ::default()
        };
        assert_eq!(default_channel_map(&organ), vec![1, 2, 0]);

        let no_pedal = Organ {
            manuals: vec![manual(1, "Great")],
            ..Organ::default()
        };
        assert_eq!(default_channel_map(&no_pedal), vec![0]);
    }

    #[test]
    fn note_on_starts_one_voice_per_drawn_stop() {
        let mut console = test_console();
        let starts = console.note_on(0, 60);
        assert_eq!(starts.len(), 2);
        let stops = console.note_off(0, 60);
        assert_eq!(stops.len(), 2);
        assert_eq!(
            stops,
            starts.iter().map(|s| s.handle).collect::<Vec<_>>()
        );
    }

    #[test]
    fn keys_outside_the_manual_are_ignoredable() {
        let mut console = test_console();
        assert!(console.note_on(0, 20).is_empty());
        assert!(console.note_on(0, 120).is_empty());
        assert!(console.note_off(0, 20).is_empty());
    }

    #[test]
    fn retrigger_accumulates_then_clears() {
        let mut console = test_console();
        console.note_on(0, 60);
        console.note_on(0, 60);
        assert_eq!(console.note_off(0, 60).len(), 4);
        assert!(console.note_off(0, 60).is_empty());
    }
}
