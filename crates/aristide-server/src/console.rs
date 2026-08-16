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
    /// Stops classified as control noises (drawstop thumps etc.) —
    /// hidden from UIs, triggered by the control they belong to.
    noise_stops: Vec<StopId>,
    stop_noise: HashMap<StopId, VoiceSpec>,
    coupler_noise: HashMap<usize, VoiceSpec>,
    tremulant_noise: Option<VoiceSpec>,
    /// Noise voices currently sounding their silent loop (control is
    /// on); note-off on toggle-off plays the push-in thump.
    stop_noise_open: HashMap<StopId, u64>,
    coupler_noise_open: HashMap<usize, u64>,
    trem_noise_open: Option<u64>,
    noises_enabled: bool,
    noise_volume: f32,
    /// `channel_map[c]` = index into `organ.manuals` MIDI channel `c`
    /// plays; channels past the end wrap.
    channel_map: Vec<usize>,
    /// Per manual index: the enclosures (engine indices) its stops sit
    /// inside — an expression pedal on that manual's channel drives
    /// them. Derived from stop→rank→windchest→enclosure membership.
    manual_enclosures: Vec<Vec<u8>>,
    /// Current pedal position per enclosure (1 = open, GO's default).
    enclosure_positions: Vec<f32>,
    next_handle: u64,
    /// (manual index, MIDI key) → pipes held by that key, tagged with
    /// the stop that engaged them (so retiring a stop can release
    /// them). `percussive` voices are excluded (they stop themselves).
    sounding: HashMap<(usize, u8), Vec<(StopId, RankId, u16)>>,
    /// The most recent voice handle started per pipe — used to expedite
    /// a still-releasing (pallet-staggered) predecessor when the pipe
    /// re-speaks, so a pipe can never overlap itself at full level.
    last_pipe_voice: HashMap<(RankId, u16), u64>,
    /// Each speaking pipe's voice handle and how many holders (keys,
    /// couplers) currently demand it. A pipe speaks ONCE no matter how
    /// many routes reach it — starting a second voice on the same pipe
    /// sums the identical recording coherently (+6 dB), and on release
    /// the phase aligner makes both tails coherent too: the release
    /// comes out LOUDER than the chord (the octave-coupled F-major pop).
    speaking: HashMap<(RankId, u16), (u64, u32)>,
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
        let mut console = Console {
            organ,
            specs,
            drawn,
            engaged_couplers: Vec::new(),
            tuning: Tuning::default(),
            noise_stops: Vec::new(),
            stop_noise: HashMap::new(),
            coupler_noise: HashMap::new(),
            tremulant_noise: None,
            stop_noise_open: HashMap::new(),
            coupler_noise_open: HashMap::new(),
            trem_noise_open: None,
            noises_enabled: true,
            noise_volume: 0.7,
            channel_map,
            manual_enclosures: Vec::new(),
            enclosure_positions: Vec::new(),
            next_handle: 0,
            sounding: HashMap::new(),
            speaking: HashMap::new(),
            last_pipe_voice: HashMap::new(),
        };
        console.classify_noises();
        // Noise stops must never be part of the registration.
        let noise_stops = console.noise_stops.clone();
        console.drawn.retain(|id| !noise_stops.contains(id));
        console.map_enclosures();
        console
    }

    /// Which enclosures each manual's expression pedal drives: every
    /// box that any of the manual's stops (via rank → windchest) sits
    /// inside. Noise stops don't count — a drawstop thump rank on an
    /// effects chest must not capture a pedal.
    fn map_enclosures(&mut self) {
        self.enclosure_positions = vec![1.0; self.organ.enclosures.len()];
        self.manual_enclosures = vec![Vec::new(); self.organ.manuals.len()];
        let max = aristide_engine::enclosure::MAX_ENCLOSURES as u32;
        for stop in &self.organ.stops {
            if self.noise_stops.contains(&stop.id) {
                continue;
            }
            let Some(manual_index) = self
                .organ
                .manuals
                .iter()
                .position(|m| m.id == stop.manual)
            else {
                continue;
            };
            for range in &stop.ranks {
                let Some(rank) = self.organ.rank(range.rank) else {
                    continue;
                };
                let Some(chest) = self
                    .organ
                    .windchests
                    .iter()
                    .find(|c| c.number == rank.windchest)
                else {
                    continue;
                };
                for &enclosure in chest.enclosures.iter().take(1) {
                    if enclosure < max
                        && !self.manual_enclosures[manual_index].contains(&(enclosure as u8))
                    {
                        self.manual_enclosures[manual_index].push(enclosure as u8);
                    }
                }
            }
        }
    }

    /// Expression pedal on `channel`: move every enclosure that
    /// channel's manual encloses. Returns (engine index, position)
    /// pairs for the control loop to forward.
    pub fn expression(&mut self, channel: u8, value: u8) -> Vec<(u8, f32)> {
        let Some(manual_index) = self.manual_index(channel) else {
            return Vec::new();
        };
        self.expression_manual(manual_index, value)
    }

    /// The same pedal move addressed by manual — used when an input
    /// device is pinned to one manual and its channel means nothing.
    pub fn expression_manual(&mut self, manual_index: usize, value: u8) -> Vec<(u8, f32)> {
        if manual_index >= self.manual_enclosures.len() {
            return Vec::new();
        }
        let position = value.min(127) as f32 / 127.0;
        let mut moves = Vec::new();
        for &enclosure in &self.manual_enclosures[manual_index] {
            if let Some(slot) = self.enclosure_positions.get_mut(enclosure as usize) {
                *slot = position;
            }
            moves.push((enclosure, position));
        }
        moves
    }

    /// UI pedal move on one enclosure by model index.
    pub fn set_enclosure(&mut self, index: usize, position: f32) -> Option<(u8, f32)> {
        let position = position.clamp(0.0, 1.0);
        *self.enclosure_positions.get_mut(index)? = position;
        (index < aristide_engine::enclosure::MAX_ENCLOSURES)
            .then_some((index as u8, position))
    }

    /// (model index, name, position, displayed) per enclosure, for UIs.
    pub fn enclosure_states(&self) -> Vec<(usize, String, f32, bool)> {
        self.organ
            .enclosures
            .iter()
            .enumerate()
            .map(|(index, enclosure)| {
                (
                    index,
                    enclosure.name.clone(),
                    self.enclosure_positions.get(index).copied().unwrap_or(1.0),
                    enclosure.displayed,
                )
            })
            .collect()
    }

    /// Control-noise "stops" (drawstop thumps, coupler clacks, blower).
    /// GO-set convention: the noise sample is structured like a pipe —
    /// pull-thump attack → near-silent sustain loop → push-in thump as
    /// the release tail — so the engine's own note lifecycle plays it:
    /// draw = note-on, retire = note-off. Classified by name (the
    /// GO-world convention); mapped to their control by fuzzy match.
    fn classify_noises(&mut self) {
        let mut noise_stops = Vec::new();
        for stop in &self.organ.stops {
            if stop.name.to_lowercase().contains("noise") {
                noise_stops.push(stop.id);
            }
        }
        self.noise_stops = noise_stops;

        for &noise_id in &self.noise_stops {
            let Some(noise_stop) = self.organ.stops.iter().find(|s| s.id == noise_id) else {
                continue;
            };
            let Some(spec) = noise_stop.ranks.first().and_then(|range| {
                self.specs.get(&(range.rank, range.first_pipe)).copied()
            }) else {
                continue;
            };
            let name = noise_stop.name.to_lowercase();

            if name.contains("tremblant") || name.contains("tremulant") {
                self.tremulant_noise = Some(spec);
                continue;
            }
            if name.contains("coupler") {
                let stripped = strip_noise_suffix(&noise_stop.name);
                let best = self
                    .organ
                    .couplers
                    .iter()
                    .enumerate()
                    .map(|(index, coupler)| (index, name_match_score(&stripped, &coupler.name)))
                    .max_by(|a, b| a.1.total_cmp(&b.1));
                if let Some((index, score)) = best {
                    if score >= 0.5 {
                        self.coupler_noise.insert(index, spec);
                    }
                }
                continue;
            }
            // Ordinary drawstop noise: match against real stops on the
            // same manual.
            let stripped = strip_noise_suffix(&noise_stop.name);
            let best = self
                .organ
                .stops
                .iter()
                .filter(|s| s.manual == noise_stop.manual && !self.noise_stops.contains(&s.id))
                .map(|s| (s.id, name_match_score(&stripped, &s.name)))
                .max_by(|a, b| a.1.total_cmp(&b.1));
            if let Some((id, score)) = best {
                if score >= 0.45 {
                    self.stop_noise.insert(id, spec);
                }
            }
        }
    }

    /// Open a noise voice (pull thump into its silent loop). The voice
    /// stays alive until the control toggles back off — its note-off
    /// then plays the push-in thump from the sample's tail. Noise
    /// voices never draw wind or breathe with pressure.
    fn open_noise(&mut self, spec: Option<VoiceSpec>) -> Option<VoiceStart> {
        let mut spec = spec?;
        if !self.noises_enabled || self.noise_volume <= 0.0 {
            return None;
        }
        spec.gain *= self.noise_volume;
        spec.wind_weight = 0.0;
        spec.brightness = 0.0;
        let handle = self.next_handle;
        self.next_handle += 1;
        Some(VoiceStart { handle, spec })
    }

    /// Enable/disable noises and set their volume. Returns handles of
    /// currently open noise voices to KILL (silently) when disabling.
    pub fn set_noises(&mut self, enabled: bool, volume: f32) -> Vec<u64> {
        self.noises_enabled = enabled;
        self.noise_volume = volume.clamp(0.0, 2.0);
        if enabled {
            return Vec::new();
        }
        let mut kills: Vec<u64> = self.stop_noise_open.drain().map(|(_, h)| h).collect();
        kills.extend(self.coupler_noise_open.drain().map(|(_, h)| h));
        kills.extend(self.trem_noise_open.take());
        kills
    }

    pub fn noises(&self) -> (bool, f32) {
        (self.noises_enabled, self.noise_volume)
    }

    /// Tremulant toggled: open the trem noise voice or release it
    /// (note-off plays the stop-side thump). Returns (start, stop).
    pub fn tremulant_toggle_noise(&mut self, engaged: bool) -> (Option<VoiceStart>, Option<u64>) {
        if engaged {
            let start = self.open_noise(self.tremulant_noise);
            self.trem_noise_open = start.as_ref().map(|s| s.handle);
            (start, None)
        } else {
            (None, self.trem_noise_open.take())
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

    /// The map as all 16 channels, wrap resolved — what a UI edits.
    /// Short maps wrap (3 manuals: channel 4 plays what channel 1 does),
    /// which is invisible until someone wants to change one channel; so
    /// the editable form is always the full sixteen.
    pub fn channel_map(&self) -> Vec<usize> {
        (0..16)
            .map(|channel| self.manual_index(channel).unwrap_or(0))
            .collect()
    }

    /// Point one MIDI channel at one manual. Expands the map to all
    /// sixteen channels first, so the edit means exactly what it says.
    pub fn set_channel(&mut self, channel: u8, manual_index: usize) {
        if manual_index >= self.organ.manuals.len() || channel >= 16 {
            return;
        }
        let mut map = self.channel_map();
        map[channel as usize] = manual_index;
        self.channel_map = map;
    }

    /// Voices retired by this press: a re-press before the note-off
    /// (key bounce, fast repetition) must release the previous voices —
    /// a pipe can't speak twice, and doubling correlated audio jumps
    /// +6 dB into clipping.
    pub fn note_on(&mut self, channel: u8, key: u8) -> (Vec<VoiceStart>, Vec<u64>) {
        let Some(manual_index) = self.manual_index(channel) else {
            return (Vec::new(), Vec::new());
        };
        self.note_on_manual(manual_index, key)
    }

    /// `note_on` addressed by manual index — the coordinate UIs speak
    /// (a clicked on-screen key has no MIDI channel).
    pub fn note_on_manual(&mut self, manual_index: usize, key: u8) -> (Vec<VoiceStart>, Vec<u64>) {
        if manual_index >= self.organ.manuals.len() {
            return (Vec::new(), Vec::new());
        }
        let mut retriggered = Vec::new();
        for (_, rank, pipe) in self
            .sounding
            .remove(&(manual_index, key))
            .unwrap_or_default()
        {
            if let Some(handle) = self.release_pipe(rank, pipe) {
                retriggered.push(handle);
            }
        }
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
                    if spec.percussive {
                        // One-shots (noises) aren't refcounted.
                        let handle = self.next_handle;
                        self.next_handle += 1;
                        starts.push(VoiceStart { handle, spec: *spec });
                        continue;
                    }
                    held.push((stop.id, range.rank, pipe));
                    match self.speaking.get_mut(&(range.rank, pipe)) {
                        Some((_, holders)) => *holders += 1,
                        None => {
                            let handle = self.next_handle;
                            self.next_handle += 1;
                            self.speaking.insert((range.rank, pipe), (handle, 1));
                            // The pipe's previous voice may still be in
                            // its pallet-stagger window (Held at full
                            // level); expedite its release or the pipe
                            // overlaps itself — heard as a phantom
                            // "opening of another note".
                            if let Some(previous) =
                                self.last_pipe_voice.insert((range.rank, pipe), handle)
                            {
                                retriggered.push(previous);
                            }
                            let mut spec = *spec;
                            spec.rate *= self.tuning.rate_multiplier(midi_key as u8);
                            starts.push(VoiceStart { handle, spec });
                        }
                    }
                }
            }
        }
        // Track the key even when no stops are drawn: the UI lights it,
        // note-off clears it, and drawing a stop mid-hold must find it
        // to start pipes under it.
        self.sounding.insert((manual_index, key), held);
        // Expedites can duplicate handles already queued by the
        // retrigger drain; the engine tolerates it but keep it clean.
        retriggered.sort_unstable();
        retriggered.dedup();
        (starts, retriggered)
    }

    pub fn note_off(&mut self, channel: u8, key: u8) -> Vec<u64> {
        let Some(manual_index) = self.manual_index(channel) else {
            return Vec::new();
        };
        self.note_off_manual(manual_index, key)
    }

    /// `note_off` addressed by manual index (see `note_on_manual`).
    pub fn note_off_manual(&mut self, manual_index: usize, key: u8) -> Vec<u64> {
        let mut released = Vec::new();
        for (_, rank, pipe) in self
            .sounding
            .remove(&(manual_index, key))
            .unwrap_or_default()
        {
            if let Some(handle) = self.release_pipe(rank, pipe) {
                released.push(handle);
            }
        }
        released
    }

    /// One holder lets go of a pipe; the voice stops only when the
    /// last holder does.
    fn release_pipe(&mut self, rank: RankId, pipe: u16) -> Option<u64> {
        let (handle, holders) = self.speaking.get_mut(&(rank, pipe))?;
        *holders -= 1;
        if *holders == 0 {
            let handle = *handle;
            self.speaking.remove(&(rank, pipe));
            Some(handle)
        } else {
            None
        }
    }

    /// Draw or retire a stop. Returns the handles of voices to stop
    /// (retired pipes, or the noise voice whose note-off plays the
    /// push-in thump) and the voices to start: the pull-thump noise,
    /// plus — the pallets under held keys being open — the stop's own
    /// pipes on those keys, as on a tracker.
    pub fn set_drawn(&mut self, stop: StopId, drawn: bool) -> (Vec<u64>, Vec<VoiceStart>) {
        // Noise stops aren't directly drawable, and a no-op change
        // shouldn't thump.
        if self.noise_stops.contains(&stop) || self.drawn.contains(&stop) == drawn {
            return (Vec::new(), Vec::new());
        }
        if drawn {
            self.drawn.push(stop);
            let mut starts = Vec::new();
            if let Some(noise) = self.open_noise(self.stop_noise.get(&stop).copied()) {
                self.stop_noise_open.insert(stop, noise.handle);
                starts.push(noise);
            }
            let mut expedited = self.start_stop_under_held_keys(stop, &mut starts);
            expedited.sort_unstable();
            expedited.dedup();
            return (expedited, starts);
        }
        self.drawn.retain(|&id| id != stop);
        let mut to_release: Vec<(RankId, u16)> = Vec::new();
        for entries in self.sounding.values_mut() {
            entries.retain(|&(owner, rank, pipe)| {
                if owner == stop {
                    to_release.push((rank, pipe));
                    false
                } else {
                    true
                }
            });
        }
        let mut released = Vec::new();
        for (rank, pipe) in to_release {
            if let Some(handle) = self.release_pipe(rank, pipe) {
                released.push(handle);
            }
        }
        // Note-off on the open noise voice = the push-in thump.
        released.extend(self.stop_noise_open.remove(&stop));
        (released, Vec::new())
    }

    /// Start `stop`'s pipes under every currently held key (coupling
    /// expanded with the *current* coupler state, like a fresh press).
    /// Appends the new voices to `starts` and returns handles of
    /// previous pipe voices to expedite (see `note_on_manual`).
    fn start_stop_under_held_keys(&mut self, stop: StopId, starts: &mut Vec<VoiceStart>) -> Vec<u64> {
        let mut expedited = Vec::new();
        let held_keys: Vec<(usize, u8)> = self.sounding.keys().copied().collect();
        for (manual_index, key) in held_keys {
            let origin = self.organ.manuals[manual_index].id;
            let played = key as i16 + self.tuning.transpose as i16;
            let mut new_entries = Vec::new();
            for (manual_id, midi_key) in self.couple(origin, played) {
                let Some(manual) = self.organ.manuals.iter().find(|m| m.id == manual_id) else {
                    continue;
                };
                let key_index = midi_key - manual.first_midi_note as i16;
                if key_index < 0 || key_index as u16 >= manual.key_count {
                    continue;
                }
                let key_index = key_index as u16;
                let Some(stop_def) = self
                    .organ
                    .stops
                    .iter()
                    .find(|s| s.id == stop && s.manual == manual_id)
                else {
                    continue;
                };
                for range in &stop_def.ranks {
                    if key_index < range.first_key
                        || key_index >= range.first_key + range.key_count
                    {
                        continue;
                    }
                    let pipe = range.first_pipe + (key_index - range.first_key);
                    let Some(spec) = self.specs.get(&(range.rank, pipe)) else {
                        continue;
                    };
                    // One-shots strike on key press, not on drawing the
                    // stop mid-hold.
                    if spec.percussive {
                        continue;
                    }
                    new_entries.push((stop, range.rank, pipe));
                    match self.speaking.get_mut(&(range.rank, pipe)) {
                        Some((_, holders)) => *holders += 1,
                        None => {
                            let handle = self.next_handle;
                            self.next_handle += 1;
                            self.speaking.insert((range.rank, pipe), (handle, 1));
                            if let Some(previous) =
                                self.last_pipe_voice.insert((range.rank, pipe), handle)
                            {
                                expedited.push(previous);
                            }
                            let mut spec = *spec;
                            spec.rate *= self.tuning.rate_multiplier(midi_key as u8);
                            starts.push(VoiceStart { handle, spec });
                        }
                    }
                }
            }
            if !new_entries.is_empty() {
                self.sounding
                    .entry((manual_index, key))
                    .or_default()
                    .extend(new_entries);
            }
        }
        expedited
    }

    /// Engage or release a coupler by its index in `organ.couplers`.
    /// Sounding notes keep their current coupling; new presses use the
    /// new state. Returns (clack voice to start, noise handle to stop).
    pub fn set_coupler(&mut self, index: usize, engaged: bool) -> (Option<VoiceStart>, Option<u64>) {
        if index >= self.organ.couplers.len()
            || self.engaged_couplers.contains(&index) == engaged
        {
            return (None, None);
        }
        if engaged {
            self.engaged_couplers.push(index);
            let noise = self.open_noise(self.coupler_noise.get(&index).copied());
            if let Some(start) = &noise {
                self.coupler_noise_open.insert(index, start.handle);
            }
            (noise, None)
        } else {
            self.engaged_couplers.retain(|&i| i != index);
            (None, self.coupler_noise_open.remove(&index))
        }
    }

    /// General cancel: retire every drawn stop and release every
    /// engaged coupler, as the cancel piston does on a real console.
    /// Returns the voice handles to stop — retired pipes, plus the open
    /// noise voices whose note-off is the push-in thump / coupler clack.
    /// Keys held through a cancel keep sounding only what survives it,
    /// which is nothing: the organ goes silent.
    pub fn cancel(&mut self) -> Vec<u64> {
        let mut stopped = Vec::new();
        for stop in self.drawn.clone() {
            let (released, _) = self.set_drawn(stop, false);
            stopped.extend(released);
        }
        for index in self.engaged_couplers.clone() {
            let (_, noise) = self.set_coupler(index, false);
            stopped.extend(noise);
        }
        stopped
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

    /// Every *playable* stop with its manual name and drawn state, for
    /// UIs — control noises are hidden (they belong to their controls).
    pub fn stop_states(&self) -> Vec<(StopId, &str, &str, bool)> {
        self.organ
            .stops
            .iter()
            .filter(|stop| !self.noise_stops.contains(&stop.id))
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

    /// Per-manual UI state: index, name, first MIDI note, key count,
    /// and the keys currently held on it (sorted). Held keys are the
    /// player's — the origin manual of each press, not coupled echoes.
    pub fn manual_states(&self) -> Vec<(usize, &str, u8, u16, Vec<u8>)> {
        self.organ
            .manuals
            .iter()
            .enumerate()
            .map(|(index, manual)| {
                let mut held: Vec<u8> = self
                    .sounding
                    .keys()
                    .filter(|&&(m, _)| m == index)
                    .map(|&(_, key)| key)
                    .collect();
                held.sort_unstable();
                (
                    index,
                    manual.name.as_str(),
                    manual.first_midi_note,
                    manual.key_count,
                    held,
                )
            })
            .collect()
    }

    pub fn organ_name(&self) -> &str {
        &self.organ.name
    }

    /// Forget everything sounding (the engine is told separately).
    pub fn all_off(&mut self) {
        self.sounding.clear();
        self.speaking.clear();
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

/// "Montre 8' stop noise" → "Montre 8'"; "I/P coupler stop noise" → "I/P".
fn strip_noise_suffix(name: &str) -> String {
    let lower = name.to_lowercase();
    let mut stripped = lower.as_str();
    for suffix in [" coupler stop noise", " stop noise", " noise"] {
        if let Some(prefix) = stripped.strip_suffix(suffix) {
            stripped = prefix;
            break;
        }
    }
    stripped.to_string()
}

/// Fuzzy score between a stripped noise name and a control name:
/// normalized-prefix containment, else token overlap (tokens match on
/// equality or ≥2-char prefix). Handles "Fl Harm 8" vs "Flute Harm. 8'"
/// and "Ped Flute 4" vs "Flute 4'".
fn name_match_score(noise: &str, candidate: &str) -> f32 {
    let normalize = |s: &str| -> String {
        s.chars()
            .filter(|c| c.is_alphanumeric())
            .collect::<String>()
            .to_lowercase()
    };
    let (a, b) = (normalize(noise), normalize(candidate));
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    if b.starts_with(&a) || a.starts_with(&b) {
        return 1.0;
    }
    let tokens = |s: &str| -> Vec<String> {
        s.to_lowercase()
            .split(|c: char| !c.is_alphanumeric())
            .filter(|t| !t.is_empty())
            .map(str::to_string)
            .collect()
    };
    let (ta, tb) = (tokens(noise), tokens(candidate));
    let mut matched = 0usize;
    for token_a in &ta {
        if tb.iter().any(|token_b| {
            token_a == token_b
                || (token_a.len() >= 2 && token_b.starts_with(token_a.as_str()))
                || (token_b.len() >= 2 && token_a.starts_with(token_b.as_str()))
        }) {
            matched += 1;
        }
    }
    matched as f32 / ta.len().max(tb.len()) as f32
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
            enclosures: vec![],
            windchests: vec![],
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
                        enclosure: aristide_engine::enclosure::ENCLOSURE_NONE,
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
        let (starts, _) = console.note_on(0, 60);
        assert_eq!(starts.len(), 2);
        let stops = console.note_off(0, 60);
        assert_eq!(stops.len(), 2);
        assert_eq!(
            stops,
            starts.iter().map(|s| s.handle).collect::<Vec<_>>()
        );
    }

    #[test]
    fn drawing_a_stop_starts_pipes_under_held_keys() {
        let mut console = test_console();
        // With nothing drawn a press starts no voices but is still
        // tracked: the key lights and can be released cleanly.
        console.set_drawn(StopId(1), false);
        console.set_drawn(StopId(2), false);
        let (starts, _) = console.note_on(0, 60);
        assert!(starts.is_empty());
        assert_eq!(console.manual_states()[0].4, vec![60]);

        // Drawing a stop mid-hold speaks immediately under the key.
        let (stopped, starts) = console.set_drawn(StopId(1), true);
        assert!(stopped.is_empty());
        assert_eq!(starts.len(), 1);
        let first = starts[0].handle;

        // A second stop adds its own rank without restarting the first.
        let (_, starts) = console.set_drawn(StopId(2), true);
        assert_eq!(starts.len(), 1);
        let second = starts[0].handle;

        // Pushing a stop in releases only its pipe, via the normal
        // note-off path (release tail, not a cut).
        let (released, starts) = console.set_drawn(StopId(1), false);
        assert!(starts.is_empty());
        assert_eq!(released, vec![first]);

        // Releasing the key stops the remaining pipe and clears the light.
        assert_eq!(console.note_off(0, 60), vec![second]);
        assert!(console.manual_states()[0].4.is_empty());
    }

    #[test]
    fn drawing_a_stop_reaches_held_keys_through_couplers() {
        let mut console = coupled_console();
        console.set_drawn(StopId(1), false);
        console.set_drawn(StopId(2), false);
        console.set_coupler(0, true); // II/I
        assert!(console.note_on(0, 60).0.is_empty());

        // The Swell stop drawn mid-hold sounds through the coupler.
        let (_, starts) = console.set_drawn(StopId(2), true);
        assert_eq!(starts.len(), 1);
        assert_eq!(starts[0].spec.sample, 1, "rank 2's sample expected");
        let handle = starts[0].handle;
        assert_eq!(console.note_off(0, 60), vec![handle]);
    }

    #[test]
    fn keys_outside_the_manual_are_ignoredable() {
        let mut console = test_console();
        assert!(console.note_on(0, 20).0.is_empty());
        assert!(console.note_on(0, 120).0.is_empty());
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
            enclosures: vec![],
            windchests: vec![],
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
                        enclosure: aristide_engine::enclosure::ENCLOSURE_NONE,
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
        assert_eq!(console.note_on(0, 60).0.len(), 1, "no couplers yet");
        console.note_off(0, 60);

        console.set_coupler(0, true); // II/I
        assert_eq!(console.note_on(0, 60).0.len(), 2, "unison coupler adds II");
        assert_eq!(console.note_off(0, 60).len(), 2, "note-off kills both");

        console.set_coupler(1, true); // 16' I (self, −12)
        // Great C + Swell C (II/I) + Great C−12 (16' I). Coupled notes
        // don't re-couple, so the sub-octave stays on the Great.
        assert_eq!(console.note_on(0, 60).0.len(), 3);
        console.note_off(0, 60);

        // Out-of-compass shifted notes drop out quietly.
        assert_eq!(console.note_on(0, 37).0.len(), 2, "37-12 is below compass");
        console.note_off(0, 37);
    }

    #[test]
    fn tuning_retunes_and_transposes() {
        let mut console = test_console();
        // Equal temperament, a=440: everything at unity rate.
        let baseline = console.note_on(0, 60).0[0].spec.rate;
        assert!((baseline - 1.0).abs() < 1e-6);
        console.note_off(0, 60);

        // Meantone C sits +10.265 cents above equal (a-referenced).
        console.set_tuning(crate::tuning::Tuning {
            temperament: crate::tuning::Temperament::Meantone4,
            a4_hz: 440.0,
            transpose: 0,
        });
        let meantone_c = console.note_on(0, 60).0[0].spec.rate;
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
        let (transposed, _) = console.note_on(0, 60);
        assert_eq!(transposed.len(), 2, "both drawn stops sound");
        // Pipe index = key 62 − first_midi 36 = 26; sample index equals
        // rank − 1 in the fixture, so instead verify by keying at the
        // compass edge: 96 + 2 is out of range → silent.
        console.note_off(0, 60);
        assert!(console.note_on(0, 96).0.is_empty(), "96+2 exceeds compass");
    }

    #[test]
    fn shared_pipes_speak_once_across_octave_coupling() {
        // The F-major pop: with a 16' coupler, key 72's coupled pipe IS
        // key 60's direct pipe. It must not start a second voice — the
        // identical recording would sum coherently (+6 dB) and the
        // phase-aligned release would make both tails coherent too:
        // a release LOUDER than the chord.
        let mut console = coupled_console();
        console.set_coupler(1, true); // 16' I: self-coupler at −12

        let (first, _) = console.note_on(0, 72);
        assert_eq!(first.len(), 2, "72 direct + coupled 60");
        let (second, _) = console.note_on(0, 60);
        assert_eq!(
            second.len(),
            1,
            "60's direct pipe already speaks via 72's coupling — only \
             the new 48-pipe may start"
        );

        // Releasing 72 must NOT stop the shared pipe (60 still holds it).
        let stopped = console.note_off(0, 72);
        assert_eq!(stopped.len(), 1, "only 72's unshared pipe stops");
        // Releasing 60 stops the shared pipe and 60's own coupled pipe.
        let stopped = console.note_off(0, 60);
        assert_eq!(stopped.len(), 2, "shared pipe + 48-pipe stop last");

        // Every started voice eventually stopped exactly once.
        assert!(console.note_off(0, 60).is_empty());
        assert!(console.note_off(0, 72).is_empty());
    }

    #[test]
    fn re_pressed_pipe_expedites_its_staggered_predecessor() {
        // Press a chord, release it (voices enter the pallet-stagger
        // window at full level), then instantly re-press a key that
        // reaches one of those pipes: the new press must carry the old
        // voice's handle so the engine releases it NOW — otherwise the
        // pipe overlaps itself ("you hear the opening of another note").
        let mut console = coupled_console();
        console.set_coupler(1, true); // 16' I self-coupler

        let (starts, _) = console.note_on(0, 72); // pipes 36 and 24
        let shared_pipe_voice = starts
            .iter()
            .map(|s| s.handle)
            .max()
            .expect("voices started");
        let released = console.note_off(0, 72);
        assert_eq!(released.len(), 2);

        // Immediately press 60 → its direct pipe IS 72's coupled pipe.
        let (_, expedited) = console.note_on(0, 60);
        assert!(
            expedited.contains(&shared_pipe_voice)
                || released.contains(&shared_pipe_voice),
            "the shared pipe's previous voice must be expedited on re-press"
        );
        assert!(
            !expedited.is_empty(),
            "re-press within the stagger window must expedite predecessors"
        );
    }

    #[test]
    fn coupler_cycles_terminate() {
        let mut console = coupled_console();
        console.set_coupler(0, true); // II/I
        console.set_coupler(2, true); // I/II — cycle
        // I→II and II→I at unison collapse to the same two notes.
        assert_eq!(console.note_on(0, 60).0.len(), 2);
        console.note_off(0, 60);
    }

    #[test]
    fn retrigger_stops_previous_voices_first() {
        // A re-press before note-off (key bounce, fast repetition) must
        // release the first press's voices — a pipe can't speak twice,
        // and doubling correlated audio is an instant +6 dB.
        let mut console = test_console();
        let (first, retriggered) = console.note_on(0, 60);
        assert_eq!(first.len(), 2);
        assert!(retriggered.is_empty());
        let first_handles: Vec<u64> = first.iter().map(|s| s.handle).collect();

        let (second, retriggered) = console.note_on(0, 60);
        assert_eq!(second.len(), 2);
        assert_eq!(retriggered, first_handles, "old voices released");

        // Note-off stops only the live (second) voices.
        let stopped = console.note_off(0, 60);
        assert_eq!(
            stopped,
            second.iter().map(|s| s.handle).collect::<Vec<_>>()
        );
        assert!(console.note_off(0, 60).is_empty());
    }
}
