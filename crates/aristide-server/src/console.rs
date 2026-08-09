//! Control-side console state: which stops are drawn, and which voices
//! each key press starts. This is deliberately outside the RT engine —
//! registration, couplers, and (later) microtonal key mappings all live
//! at this layer, where allocation and locking are fine.

use std::collections::HashMap;

use aristide_model::{ManualId, Organ, RankId, StopId};

use crate::bank::VoiceSpec;
use crate::tuning::Tuning;

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
    /// Engaged couplers, as indices into `organ.couplers`.
    engaged_couplers: Vec<usize>,
    tuning: Tuning,
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
            engaged_couplers: Vec::new(),
            tuning: Tuning::default(),
            channel_map,
            next_handle: 0,
            sounding: HashMap::new(),
        }
    }

    pub fn set_tuning(&mut self, tuning: Tuning) {
        self.tuning = tuning;
    }

    pub fn tuning(&self) -> Tuning {
        self.tuning
    }

    /// Expand one played key through the engaged couplers into every
    /// (manual, MIDI key) that should sound. Couplers act on *played*
    /// keys only — coupler-produced notes don't re-couple (matching
    /// default organ behaviour, and making self-couplers like a 16' II
    /// trivially finite; GO's opt-in propagation flags can come later).
    fn couple(&self, manual: ManualId, midi_key: i16) -> Vec<(ManualId, i16)> {
        let mut targets = vec![(manual, midi_key)];
        for &engaged in &self.engaged_couplers {
            let Some(coupler) = self.organ.couplers.get(engaged) else {
                continue;
            };
            if coupler.from_manual != manual {
                continue;
            }
            let target = (
                coupler.to_manual,
                midi_key.saturating_add(coupler.key_shift),
            );
            if !targets.contains(&target) {
                targets.push(target);
            }
        }
        targets
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
        let origin = self.organ.manuals[manual_index].id;
        // The transposer shifts which pipes sound, like the console
        // gadget; temperament + concert pitch then retune each pipe.
        let played = key as i16 + self.tuning.transpose as i16;

        let mut starts = Vec::new();
        let mut held = Vec::new();
        for (manual_id, midi_key) in self.couple(origin, played) {
            let Some(manual) = self.organ.manuals.iter().find(|m| m.id == manual_id) else {
                continue;
            };
            let key_index = midi_key - manual.first_midi_note as i16;
            if key_index < 0 || key_index as u16 >= manual.key_count {
                continue;
            }
            let key_index = key_index as u16;
            for stop in &self.organ.stops {
                if stop.manual != manual_id || !self.drawn.contains(&stop.id) {
                    continue;
                }
                for range in &stop.ranks {
                    if key_index < range.first_key
                        || key_index >= range.first_key + range.key_count
                    {
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
                    let mut spec = *spec;
                    if !spec.percussive {
                        spec.rate *= self.tuning.rate_multiplier(midi_key as u8);
                    }
                    starts.push(VoiceStart { handle, spec });
                }
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

    /// Engage or release a coupler by its index in `organ.couplers`.
    /// Sounding notes keep their current coupling; new presses use the
    /// new state.
    pub fn set_coupler(&mut self, index: usize, engaged: bool) {
        if index >= self.organ.couplers.len() {
            return;
        }
        if engaged {
            if !self.engaged_couplers.contains(&index) {
                self.engaged_couplers.push(index);
            }
        } else {
            self.engaged_couplers.retain(|&i| i != index);
        }
    }

    /// Every coupler with its engaged state, for UIs.
    pub fn coupler_states(&self) -> Vec<(usize, &str, bool)> {
        self.organ
            .couplers
            .iter()
            .enumerate()
            .map(|(index, coupler)| {
                (
                    index,
                    coupler.name.as_str(),
                    self.engaged_couplers.contains(&index),
                )
            })
            .collect()
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

    /// Two manuals with one stop each, plus II/I unison, 16' I (self,
    /// −12), and a deliberate I→II / II→I cycle pair.
    fn coupled_console() -> Console {
        let manual = |id: u32, name: &str| Manual {
            id: ManualId(id),
            name: name.into(),
            first_midi_note: 36,
            key_count: 61,
        };
        let stop = |id: u32, manual: u32, rank: u32| Stop {
            id: StopId(id),
            name: format!("stop {id}"),
            manual: ManualId(manual),
            ranks: vec![RankRange {
                rank: RankId(rank),
                first_key: 0,
                key_count: 61,
                first_pipe: 0,
            }],
        };
        let rank = |id: u32| Rank {
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
        };
        let coupler = |name: &str, from: u32, to: u32, shift: i16| aristide_model::Coupler {
            name: name.into(),
            from_manual: ManualId(from),
            to_manual: ManualId(to),
            key_shift: shift,
        };
        let organ = Organ {
            name: "C".into(),
            base_path: Default::default(),
            manuals: vec![manual(1, "Great"), manual(2, "Swell")],
            stops: vec![stop(1, 1, 1), stop(2, 2, 2)],
            ranks: vec![rank(1), rank(2)],
            couplers: vec![
                coupler("II/I", 1, 2, 0),
                coupler("16' I", 1, 1, -12),
                coupler("I/II", 2, 1, 0),
            ],
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
                        brightness: 0.0,
                    },
                );
            }
        }
        Console::new(organ, specs, vec![StopId(1), StopId(2)], Vec::new())
    }

    #[test]
    fn couplers_route_between_manuals_and_octaves() {
        let mut console = coupled_console();
        // Channel 0 → Great (no pedal in this organ → identity map).
        assert_eq!(console.note_on(0, 60).len(), 1, "no couplers yet");
        console.note_off(0, 60);

        console.set_coupler(0, true); // II/I
        assert_eq!(console.note_on(0, 60).len(), 2, "unison coupler adds II");
        assert_eq!(console.note_off(0, 60).len(), 2, "note-off kills both");

        console.set_coupler(1, true); // 16' I (self, −12)
        // Great C + Swell C (II/I) + Great C−12 (16' I). Coupled notes
        // don't re-couple, so the sub-octave stays on the Great.
        assert_eq!(console.note_on(0, 60).len(), 3);
        console.note_off(0, 60);

        // Out-of-compass shifted notes drop out quietly.
        assert_eq!(console.note_on(0, 37).len(), 2, "37-12 is below compass");
        console.note_off(0, 37);
    }

    #[test]
    fn tuning_retunes_and_transposes() {
        let mut console = test_console();
        // Equal temperament, a=440: everything at unity rate.
        let baseline = console.note_on(0, 60)[0].spec.rate;
        assert!((baseline - 1.0).abs() < 1e-6);
        console.note_off(0, 60);

        // Meantone C sits +10.265 cents above equal (a-referenced).
        console.set_tuning(crate::tuning::Tuning {
            temperament: crate::tuning::Temperament::Meantone4,
            a4_hz: 440.0,
            transpose: 0,
        });
        let meantone_c = console.note_on(0, 60)[0].spec.rate;
        let expected = (10.265f32 / 1200.0).exp2();
        assert!(
            (meantone_c - expected).abs() < 1e-4,
            "meantone C rate {meantone_c} vs {expected}"
        );
        console.note_off(0, 60);

        // Transpose +2: key 60 routes to pipe 62 (rate reflects D's
        // offset, and the sounding pipe index shifts).
        console.set_tuning(crate::tuning::Tuning {
            temperament: crate::tuning::Temperament::Equal,
            a4_hz: 440.0,
            transpose: 2,
        });
        let transposed = console.note_on(0, 60);
        assert_eq!(transposed.len(), 2, "both drawn stops sound");
        // Pipe index = key 62 − first_midi 36 = 26; sample index equals
        // rank − 1 in the fixture, so instead verify by keying at the
        // compass edge: 96 + 2 is out of range → silent.
        console.note_off(0, 60);
        assert!(console.note_on(0, 96).is_empty(), "96+2 exceeds compass");
    }

    #[test]
    fn coupler_cycles_terminate() {
        let mut console = coupled_console();
        console.set_coupler(0, true); // II/I
        console.set_coupler(2, true); // I/II — cycle
        // I→II and II→I at unison collapse to the same two notes.
        assert_eq!(console.note_on(0, 60).len(), 2);
        console.note_off(0, 60);
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
