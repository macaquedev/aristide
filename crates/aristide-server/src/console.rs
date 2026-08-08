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
    next_handle: u64,
    /// (manual index, MIDI key) → handles sounding for that key.
    /// `percussive` voices are excluded (they stop themselves).
    sounding: HashMap<(usize, u8), Vec<u64>>,
}

impl Console {
    pub fn new(organ: Organ, specs: HashMap<(RankId, u16), VoiceSpec>, drawn: Vec<StopId>) -> Console {
        Console {
            organ,
            specs,
            drawn,
            next_handle: 0,
            sounding: HashMap::new(),
        }
    }

    /// MIDI channels map onto manuals in model order (pedal first when
    /// present); out-of-range channels wrap so a single-channel console
    /// still reaches something.
    fn manual_index(&self, channel: u8) -> Option<usize> {
        if self.organ.manuals.is_empty() {
            return None;
        }
        Some(channel as usize % self.organ.manuals.len())
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
                    held.push(handle);
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
    }

    /// Forget everything sounding (the engine is told separately).
    pub fn all_off(&mut self) {
        self.sounding.clear();
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
                    },
                );
            }
        }
        Console::new(organ, specs, vec![StopId(1), StopId(2)])
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
