//! Control-side console state: which stops are drawn, and which voices
//! each key press starts. This is deliberately outside the RT engine —
//! registration, couplers, and (later) microtonal key mappings all live
//! at this layer, where allocation and locking are fine.

use std::collections::HashMap;

use aristide_model::{ManualId, Organ, RankId, RankRange, StopId};

use crate::bank::VoiceSpec;
use crate::tuning::Tuning;

/// A voice the console wants started, tagged with the handle it will
/// later be stopped by.
pub struct VoiceStart {
    pub handle: u64,
    pub spec: VoiceSpec,
}

/// One place a played key lands after coupling: the manual and key
/// that should sound, and the policies the route it travelled grants.
struct Landing {
    manual: ManualId,
    midi_key: i16,
    /// May pipes the division hasn't got be filled in by repitching a
    /// neighbour? True for the played key itself (repitching serves
    /// the player's keyboard) and for routes that opt in.
    fill: bool,
    /// Is the landing bounded by the destination's compass? The played
    /// key always is — the compass *is* the instrument. A repitching
    /// route is explicitly asked to synthesize what the instrument
    /// hasn't got (16' tone off the bottom of an 8' rank), so it
    /// reaches past the compass; a normal route stays inside it.
    bounded: bool,
}

pub struct Console {
    organ: Organ,
    specs: HashMap<(RankId, u16), VoiceSpec>,
    drawn: Vec<StopId>,
    /// Engaged couplers, as indices into `organ.couplers`.
    engaged_couplers: Vec<usize>,
    /// Per coupler: still on the console? Picked in the Organ setup —
    /// an unavailable coupler is disengaged and hidden, never deleted,
    /// so it can come back without reloading the set.
    available_couplers: Vec<bool>,
    tuning: Tuning,
    /// Per manual index: a tuning of that division's own, overriding
    /// the console's — a 415 Hz meantone Positif against a 440 equal
    /// Great in one instrument.
    manual_tuning: Vec<Option<Tuning>>,
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
    ///
    /// Pipes here (and in `last_pipe_voice` / `speaking`) are *nominal*
    /// indices — the rank-ladder position a key demands, which is the
    /// physical pipe whenever the rank has it and a position past its
    /// ends (or in a hole) when a neighbour is repitched to stand in.
    /// Identity by nominal position is what lets two keys borrow the
    /// same physical pipe at two pitches simultaneously, while the same
    /// pitch reached by several routes still merges into one voice.
    sounding: HashMap<(usize, u8), Vec<(StopId, RankId, i32)>>,
    /// The most recent voice handle started per pipe — used to expedite
    /// a still-releasing (pallet-staggered) predecessor when the pipe
    /// re-speaks, so a pipe can never overlap itself at full level.
    /// (Repitched borrowings at other pitches don't count as "itself":
    /// different rates aren't phase-coherent, so they may overlap.)
    last_pipe_voice: HashMap<(RankId, i32), u64>,
    /// Each speaking pipe's voice handle and how many holders (keys,
    /// couplers) currently demand it. A pipe speaks ONCE no matter how
    /// many routes reach it — starting a second voice on the same pipe
    /// sums the identical recording coherently (+6 dB), and on release
    /// the phase aligner makes both tails coherent too: the release
    /// comes out LOUDER than the chord (the octave-coupled F-major pop).
    speaking: HashMap<(RankId, i32), (u64, u32)>,
    /// Engine output rate; frequency-derived voice parameters have to be
    /// recomputed against it when a pipe is repitched.
    device_rate: f32,
    /// Whether a coupled voice may be repitched from a neighbouring
    /// pipe. False, and deliberately so: see `voices_for_key`.
    couplers_repitch: bool,
    /// Per manual: the inclusive MIDI note range that manual answers to.
    /// Starts as the sample set's own compass and is widened to the
    /// player's keyboard (see `set_compass`) — a key outside it is
    /// silent, which is the locked compass rule with the player's
    /// hardware supplying the number.
    compass: Vec<(i16, i16)>,
}

impl Console {
    pub fn new(
        organ: Organ,
        specs: HashMap<(RankId, u16), VoiceSpec>,
        drawn: Vec<StopId>,
        device_rate: f32,
    ) -> Console {
        let compass = organ
            .manuals
            .iter()
            .map(|manual| {
                let first = manual.first_midi_note as i16;
                (first, first + manual.key_count as i16 - 1)
            })
            .collect();
        let mut console = Console {
            organ,
            specs,
            drawn,
            device_rate,
            couplers_repitch: false,
            compass,
            engaged_couplers: Vec::new(),
            available_couplers: Vec::new(),
            tuning: Tuning::default(),
            manual_tuning: Vec::new(),
            noise_stops: Vec::new(),
            stop_noise: HashMap::new(),
            coupler_noise: HashMap::new(),
            tremulant_noise: None,
            stop_noise_open: HashMap::new(),
            coupler_noise_open: HashMap::new(),
            trem_noise_open: None,
            noises_enabled: true,
            noise_volume: 0.7,
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
        // Re-mapping (a stop moved) must not slam every box open.
        if self.enclosure_positions.len() != self.organ.enclosures.len() {
            self.enclosure_positions = vec![1.0; self.organ.enclosures.len()];
        }
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

    /// Expression pedal: move every enclosure this manual's stops sit
    /// inside. Returns (engine index, position) pairs for the control
    /// loop to forward.
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

    /// Give one division a tuning of its own, or with `None` return it
    /// to the console's. Applies from the next note on; held voices
    /// keep the pitch they started at, like pipes mid-speech.
    pub fn set_manual_tuning(&mut self, manual_index: usize, tuning: Option<Tuning>) {
        if manual_index >= self.organ.manuals.len() {
            return;
        }
        if self.manual_tuning.len() < self.organ.manuals.len() {
            self.manual_tuning.resize(self.organ.manuals.len(), None);
        }
        self.manual_tuning[manual_index] = tuning;
    }

    pub fn manual_tuning(&self, manual_index: usize) -> Option<Tuning> {
        self.manual_tuning.get(manual_index).copied().flatten()
    }

    /// The tuning a division actually plays under: its own, else the
    /// console's. Couplers make this physical — a coupled copy sounds
    /// the destination's pipes, and pipes are tuned where they stand,
    /// so the copy speaks in the destination's temperament.
    fn effective_tuning(&self, manual_index: usize) -> Tuning {
        self.manual_tuning
            .get(manual_index)
            .copied()
            .flatten()
            .unwrap_or(self.tuning)
    }

    /// Expand one played key through the engaged couplers' routes into
    /// every (manual, MIDI key) that should sound, each landing tagged
    /// with the policies the route it travelled grants it. Couplers act
    /// on *played* keys only — coupler-produced notes don't re-couple
    /// (matching default organ behaviour, and making self-couplers like
    /// a 16' II trivially finite; GO's opt-in propagation flags can
    /// come later).
    ///
    /// The played key itself lands first — unless an engaged route with
    /// `unison_off` covers it, in which case its own division is silent
    /// and only the coupled copies speak: the note moves rather than
    /// doubles.
    fn couple(&self, manual: ManualId, midi_key: i16) -> Vec<Landing> {
        let mut unison_off = false;
        let mut copies: Vec<Landing> = Vec::new();
        for &engaged in &self.engaged_couplers {
            let Some(coupler) = self.organ.couplers.get(engaged) else {
                continue;
            };
            for route in &coupler.routes {
                if route.from_manual != manual || !route.covers(midi_key) {
                    continue;
                }
                unison_off |= route.unison_off;
                let Some(target) = &route.target else {
                    continue;
                };
                let repitch = target.repitch.unwrap_or(self.couplers_repitch);
                let landing = Landing {
                    manual: target.manual,
                    midi_key: midi_key.saturating_add(target.key_shift),
                    fill: repitch,
                    bounded: !repitch,
                };
                match copies
                    .iter_mut()
                    .find(|c| (c.manual, c.midi_key) == (landing.manual, landing.midi_key))
                {
                    // Two routes onto the same note speak one pipe; if
                    // either may fill or escape the compass, the
                    // landing may.
                    Some(existing) => {
                        existing.fill |= landing.fill;
                        existing.bounded &= landing.bounded;
                    }
                    None => copies.push(landing),
                }
            }
        }
        let mut landings = Vec::with_capacity(copies.len() + 1);
        if !unison_off {
            landings.push(Landing {
                manual,
                midi_key,
                fill: true,
                bounded: true,
            });
        }
        // A copy that lands exactly on the played key adds nothing.
        landings.extend(
            copies
                .into_iter()
                .filter(|c| unison_off || (c.manual, c.midi_key) != (manual, midi_key)),
        );
        landings
    }

    /// The inclusive MIDI range a manual answers to, and the widening
    /// of it to the player's keyboard. Notes outside are silent: the
    /// compass is the instrument, and the player's hardware now says
    /// how wide it is.
    pub fn compass(&self, manual_index: usize) -> Option<(i16, i16)> {
        self.compass.get(manual_index).copied()
    }

    /// The compass the sample set itself declares — what the player's
    /// keyboard is measured against, and what `reset_compass` restores.
    pub fn native_compass(&self, manual_index: usize) -> Option<(i16, i16)> {
        self.organ.manuals.get(manual_index).map(|manual| {
            let first = manual.first_midi_note as i16;
            (first, first + manual.key_count as i16 - 1)
        })
    }

    pub fn set_compass(&mut self, manual_index: usize, low: i16, high: i16) {
        let Some(manual) = self.organ.manuals.get(manual_index) else {
            return;
        };
        let (low, high) = (low.min(high).max(0), high.max(low).min(127));
        let native = (
            manual.first_midi_note as i16,
            manual.first_midi_note as i16 + manual.key_count as i16 - 1,
        );
        if (low, high) != native {
            tracing::info!(
                "compass: {} plays {}..{} (the set gives {}..{})",
                manual.name,
                low,
                high,
                native.0,
                native.1
            );
        }
        self.compass[manual_index] = (low, high);
    }

    /// Restore a manual to the compass its sample set declares.
    pub fn reset_compass(&mut self, manual_index: usize) {
        let Some(manual) = self.organ.manuals.get(manual_index) else {
            return;
        };
        let first = manual.first_midi_note as i16;
        self.compass[manual_index] = (first, first + manual.key_count as i16 - 1);
    }

    /// Let couplers reach pipes their division hasn't got by repitching
    /// a neighbour. Off by default (`[couplers] repitch` in the
    /// sidecar); see `voices_for_key` for why.
    pub fn set_coupler_repitch(&mut self, repitch: bool) {
        self.couplers_repitch = repitch;
    }

    pub fn coupler_repitch(&self) -> bool {
        self.couplers_repitch
    }

    /// Which pipe of `range` speaks for a key, the *nominal* pipe index
    /// it stands in for (the rank-ladder position the key demands, equal
    /// to the pipe itself when nothing is borrowed), and the ratio its
    /// playback rate must be scaled by. The nominal index is the voice's
    /// identity for refcounting: two keys borrowing the same physical
    /// pipe at different pitches are two voices, not one.
    ///
    /// The pipe a rank *nominally* holds for a key may be missing: the
    /// set's compass may be narrower than the keyboard the player has
    /// widened this manual to, the range may stop short of it, or the
    /// set may simply have a hole where a sample failed to load. In
    /// every one of those cases the honest answer is not silence — it
    /// is the nearest pipe the rank does have, played at the pitch the
    /// missing one would have sounded. That is what a real organ
    /// builder's borrowing does, and what makes a 56-note set playable
    /// from a 61-note keyboard.
    ///
    /// The ratio is exactly 1 (before tuning) whenever the nominal pipe
    /// exists, so nothing in the ordinary compass is touched by this.
    fn pipe_for(&self, range: &RankRange, key_index: i16, fill: bool) -> Option<(u16, i32, f32)> {
        let first = range.first_pipe as i32;
        let last = first + range.key_count as i32 - 1;
        if last < first {
            return None;
        }
        // Where the key would fall in the rank if the rank ran forever.
        // Outside the range this is the pipe the organ doesn't have.
        let wanted = first + (key_index as i32 - range.first_key as i32);
        if !fill {
            // Nothing stands in for this one: it speaks if the rank has
            // it, and is silent if it hasn't.
            let exact = u16::try_from(wanted).ok()?;
            let present = (first..=last).contains(&wanted)
                && self.specs.contains_key(&(range.rank, exact));
            return present.then_some((exact, wanted, 1.0));
        }
        // The rank may run past this stop's window into it — a unit
        // rank drawn at 16' on the pedal and 8' on a manual overlaps
        // both stops' windows. The stop's *coverage* is the range's
        // business (settled in `range_covers` before we get here); the
        // pipe that stands in is the rank's, so the search spans the
        // whole rank. A real pipe at the wanted ladder position beats
        // any repitched neighbour.
        let rank_last = self
            .organ
            .rank(range.rank)
            .map_or(last, |rank| rank.pipes.len() as i32 - 1);
        let mut source = wanted.clamp(0, rank_last);
        // A hole at the wanted position is a defect, not a decision:
        // step outwards until a pipe that actually loaded turns up.
        if !self.specs.contains_key(&(range.rank, source as u16)) {
            let mut found = None;
            for distance in 1..=rank_last {
                for candidate in [source - distance, source + distance] {
                    if (0..=rank_last).contains(&candidate)
                        && self.specs.contains_key(&(range.rank, candidate as u16))
                    {
                        found = Some(candidate);
                        break;
                    }
                }
                if found.is_some() {
                    break;
                }
            }
            source = found?;
        }
        // Ranks are semitone ladders, so the pitch the missing pipe
        // would have sounded is its distance from the one we found.
        let semitones = (wanted - source) as f32;
        let ratio = if semitones == 0.0 {
            1.0
        } else {
            (semitones / 12.0).exp2()
        };
        Some((source as u16, wanted, ratio))
    }

    fn within_compass(&self, manual_index: usize, midi_key: i16) -> bool {
        self.compass
            .get(manual_index)
            .is_some_and(|&(low, high)| (low..=high).contains(&midi_key))
    }

    /// Whether this rank range answers for a key.
    ///
    /// Inside the set's own compass the range's limits are respected to
    /// the key: a stop that covers half the keyboard (divided registers,
    /// treble-only mixtures) is a musical decision, and filling it in
    /// would be inventing an instrument. Beyond the set's compass —
    /// where the player's keyboard is wider than the organ — there is no
    /// such decision to respect, so a range that reaches the edge
    /// carries on past it.
    fn range_covers(
        &self,
        range: &RankRange,
        key_index: i16,
        manual_index: usize,
        fill: bool,
    ) -> bool {
        let first = range.first_key as i16;
        let last = first + range.key_count as i16 - 1;
        if (first..=last).contains(&key_index) {
            return true;
        }
        if !fill {
            return false;
        }
        let native_last = self.organ.manuals[manual_index].key_count as i16 - 1;
        if key_index < 0 {
            first == 0
        } else if key_index > native_last {
            last == native_last
        } else {
            false
        }
    }

    /// The pipes one key press sounds, with each voice's parameters
    /// settled — couplers expanded, compass enforced, missing pipes
    /// filled by repitching. Both the key press and drawing a stop
    /// under a held key come through here, so they cannot disagree.
    ///
    /// `only` restricts the walk to one stop (drawing it mid-hold).
    ///
    /// The `i32` in each entry is the voice's identity for refcounting:
    /// the nominal pipe index of `pipe_for`, not the physical pipe that
    /// happens to sound it.
    fn voices_for_key(
        &self,
        manual_index: usize,
        key: u8,
        only: Option<StopId>,
    ) -> Vec<(StopId, RankId, i32, VoiceSpec)> {
        let Some(origin) = self.organ.manuals.get(manual_index).map(|m| m.id) else {
            return Vec::new();
        };
        // The transposer shifts which pipes sound, like the console
        // gadget — the *played* division's transpose, since it is the
        // keyboard that shifts; temperament + concert pitch retune each
        // pipe where it lands.
        let played = key as i16 + self.effective_tuning(manual_index).transpose as i16;
        let mut voices = Vec::new();
        // Each landing carries its own policies: the played key fills
        // (repitching serves the player's keyboard) inside the compass;
        // a coupled copy fills, and escapes the compass, only if its
        // route asked to. A 16' running off the bottom of a rank, or a
        // coupler into a division with a shorter compass, sounds
        // nothing there unless its route opts into repitching.
        for landing in self.couple(origin, played) {
            let Landing {
                manual: manual_id,
                midi_key,
                fill,
                bounded,
            } = landing;
            let Some(target) = self
                .organ
                .manuals
                .iter()
                .position(|m| m.id == manual_id)
                .filter(|&index| !bounded || self.within_compass(index, midi_key))
            else {
                continue;
            };
            // Negative below the set's own bottom key: the rank ladder
            // is extrapolated in both directions, so keep the sign.
            let key_index = midi_key - self.organ.manuals[target].first_midi_note as i16;
            for stop in &self.organ.stops {
                if stop.manual != manual_id
                    || !self.drawn.contains(&stop.id)
                    || only.is_some_and(|only| only != stop.id)
                {
                    continue;
                }
                for range in &stop.ranks {
                    if !self.range_covers(range, key_index, target, fill) {
                        continue;
                    }
                    let Some((pipe, nominal, ratio)) = self.pipe_for(range, key_index, fill)
                    else {
                        continue;
                    };
                    let Some(spec) = self.specs.get(&(range.rank, pipe)) else {
                        continue;
                    };
                    voices.push((
                        stop.id,
                        range.rank,
                        nominal,
                        self.voiced(*spec, ratio, midi_key, target),
                    ));
                }
            }
        }
        voices
    }

    /// A pipe's spec as it must sound for one key: the scale's own
    /// deviation for that key, times the repitching ratio when the pipe
    /// is standing in for one the rank hasn't got. The scale is the
    /// *sounding* division's — its pipes, its temperament.
    ///
    /// Everything downstream of pitch has to move with it. Wind draw and
    /// the pressure→brightness hinge are properties of the *sounding*
    /// pitch, not of the recording, so a pipe pressed into service five
    /// semitones up draws wind like the pipe it is imitating.
    fn voiced(&self, mut spec: VoiceSpec, ratio: f32, midi_key: i16, manual_index: usize) -> VoiceSpec {
        spec.rate *= ratio
            * self
                .effective_tuning(manual_index)
                .rate_multiplier(midi_key.clamp(0, 127) as u8);
        if ratio != 1.0 {
            let sounding_hz = (spec.nominal_hz * ratio) as f64;
            spec.wind_weight = crate::bank::wind_weight(sounding_hz, spec.percussive);
            spec.brightness =
                crate::bank::brightness_coefficient(sounding_hz, self.device_rate, spec.percussive);
        }
        spec
    }

    /// Voices retired by this press: a re-press before the note-off
    /// (key bounce, fast repetition) must release the previous voices —
    /// a pipe can't speak twice, and doubling correlated audio jumps
    /// +6 dB into clipping.
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
        let mut starts = Vec::new();
        let mut held = Vec::new();
        for (stop, rank, pipe, spec) in self.voices_for_key(manual_index, key, None) {
            if spec.percussive {
                // One-shots (noises) aren't refcounted.
                let handle = self.next_handle;
                self.next_handle += 1;
                starts.push(VoiceStart { handle, spec });
                continue;
            }
            held.push((stop, rank, pipe));
            self.hold_pipe(rank, pipe, spec, &mut starts, &mut retriggered);
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

    /// One more holder demands a pipe. A pipe speaks ONCE no matter how
    /// many routes reach it, so this either bumps the refcount or
    /// starts a new voice — expediting the pipe's still-releasing
    /// (pallet-staggered) predecessor so it never overlaps itself.
    fn hold_pipe(
        &mut self,
        rank: RankId,
        pipe: i32,
        spec: VoiceSpec,
        starts: &mut Vec<VoiceStart>,
        expedited: &mut Vec<u64>,
    ) {
        match self.speaking.get_mut(&(rank, pipe)) {
            Some((_, holders)) => *holders += 1,
            None => {
                let handle = self.next_handle;
                self.next_handle += 1;
                self.speaking.insert((rank, pipe), (handle, 1));
                if let Some(previous) = self.last_pipe_voice.insert((rank, pipe), handle) {
                    expedited.push(previous);
                }
                starts.push(VoiceStart { handle, spec });
            }
        }
    }

    /// One holder lets go of a pipe; the voice stops only when the
    /// last holder does.
    fn release_pipe(&mut self, rank: RankId, pipe: i32) -> Option<u64> {
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
        let mut to_release: Vec<(RankId, i32)> = Vec::new();
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
            let mut new_entries = Vec::new();
            for (stop, rank, pipe, spec) in self.voices_for_key(manual_index, key, Some(stop)) {
                // One-shots strike on key press, not on drawing the
                // stop mid-hold.
                if spec.percussive {
                    continue;
                }
                new_entries.push((stop, rank, pipe));
                self.hold_pipe(rank, pipe, spec, starts, &mut expedited);
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

    pub fn is_drawn(&self, stop: StopId) -> bool {
        self.drawn.contains(&stop)
    }

    pub fn coupler_engaged(&self, index: usize) -> bool {
        self.engaged_couplers.contains(&index)
    }

    pub fn coupler_available(&self, index: usize) -> bool {
        self.available_couplers.get(index).copied().unwrap_or(true)
    }

    /// Keep a coupler on the console or take it off it. Taking it off
    /// releases it first (with its clack, like any release); the routes
    /// stay in the organ, so putting it back needs no reload.
    pub fn set_coupler_available(
        &mut self,
        index: usize,
        available: bool,
    ) -> (Vec<u64>, Vec<VoiceStart>) {
        if index >= self.organ.couplers.len() {
            return (Vec::new(), Vec::new());
        }
        if self.available_couplers.len() < self.organ.couplers.len() {
            self.available_couplers.resize(self.organ.couplers.len(), true);
        }
        let released = if !available && self.coupler_engaged(index) {
            self.set_coupler(index, false)
        } else {
            (Vec::new(), Vec::new())
        };
        self.available_couplers[index] = available;
        released
    }

    /// Move a stop to another manual, re-anchoring its key windows by
    /// pitch — the key that meant tenor C keeps meaning tenor C — and
    /// trimming to the destination's key count. A drawn stop is
    /// retired and redrawn across the move, so held keys release its
    /// old pipes and (where the destination holds them too) sound its
    /// new ones.
    pub fn move_stop(
        &mut self,
        stop: StopId,
        manual_index: usize,
    ) -> (Vec<u64>, Vec<VoiceStart>) {
        let Some(target) = self.organ.manuals.get(manual_index) else {
            return (Vec::new(), Vec::new());
        };
        let (target_id, target_first, target_count) = (
            target.id,
            target.first_midi_note as i32,
            target.key_count as i32,
        );
        let Some(entry) = self.organ.stops.iter().position(|s| s.id == stop) else {
            return (Vec::new(), Vec::new());
        };
        if self.organ.stops[entry].manual == target_id || self.noise_stops.contains(&stop) {
            return (Vec::new(), Vec::new());
        }
        let was_drawn = self.is_drawn(stop);
        let (mut stopped, _) = if was_drawn {
            self.set_drawn(stop, false)
        } else {
            (Vec::new(), Vec::new())
        };
        let source_first = self
            .organ
            .manuals
            .iter()
            .find(|m| m.id == self.organ.stops[entry].manual)
            .map(|m| m.first_midi_note as i32)
            .unwrap_or(target_first);
        let shift = source_first - target_first;
        let moved = &mut self.organ.stops[entry];
        moved.manual = target_id;
        moved.ranks = moved
            .ranks
            .iter()
            .filter_map(|range| {
                let mut first_key = range.first_key as i32 + shift;
                let mut key_count = range.key_count as i32;
                let mut first_pipe = range.first_pipe as i32;
                if first_key < 0 {
                    first_pipe -= first_key;
                    key_count += first_key;
                    first_key = 0;
                }
                key_count = key_count.min(target_count - first_key);
                (key_count > 0).then_some(aristide_model::RankRange {
                    rank: range.rank,
                    first_key: first_key as u16,
                    key_count: key_count as u16,
                    first_pipe: first_pipe as u16,
                })
            })
            .collect();
        if self.organ.stops[entry].ranks.is_empty() {
            tracing::warn!(
                "moved stop {:?} lies entirely outside its new manual's compass",
                self.organ.stops[entry].name
            );
        }
        tracing::info!(
            "stop {:?} moved to {:?}",
            self.organ.stops[entry].name,
            self.organ.manuals[manual_index].name
        );
        // Expression routing follows the stop to its new division.
        self.map_enclosures();
        let starts = if was_drawn {
            let (also_stopped, starts) = self.set_drawn(stop, true);
            stopped.extend(also_stopped);
            starts
        } else {
            Vec::new()
        };
        (stopped, starts)
    }

    /// Engage or release a coupler by its index in `organ.couplers`.
    /// Takes effect under held notes immediately, as an electric-action
    /// console does (and as drawing a stop mid-hold already did):
    /// engaging starts the coupled pipes under the held keys, releasing
    /// lets go of them, and a unison-off coupler moves the held notes.
    /// Returns (voice handles to stop, voices to start) like
    /// `set_drawn` — the clack noise rides along in them.
    pub fn set_coupler(&mut self, index: usize, engaged: bool) -> (Vec<u64>, Vec<VoiceStart>) {
        if index >= self.organ.couplers.len()
            || self.engaged_couplers.contains(&index) == engaged
            || (engaged && !self.coupler_available(index))
        {
            return (Vec::new(), Vec::new());
        }
        let mut stops = Vec::new();
        let mut starts = Vec::new();
        if engaged {
            self.engaged_couplers.push(index);
            if let Some(noise) = self.open_noise(self.coupler_noise.get(&index).copied()) {
                self.coupler_noise_open.insert(index, noise.handle);
                starts.push(noise);
            }
        } else {
            self.engaged_couplers.retain(|&i| i != index);
            // Note-off on the open noise voice = the release clack.
            stops.extend(self.coupler_noise_open.remove(&index));
        }
        self.recouple_held_keys(&mut stops, &mut starts);
        stops.sort_unstable();
        stops.dedup();
        (stops, starts)
    }

    /// Re-derive what every held key should sound under the current
    /// coupler state and diff it against what it does sound: pipes no
    /// longer demanded are released, newly demanded ones started. This
    /// is what makes a coupler change land on held notes instead of
    /// waiting for the next press.
    fn recouple_held_keys(&mut self, stops: &mut Vec<u64>, starts: &mut Vec<VoiceStart>) {
        let held: Vec<(usize, u8)> = self.sounding.keys().copied().collect();
        for (manual_index, key) in held {
            // One-shots strike on key press, not on a coupler change —
            // same rule as drawing a stop mid-hold.
            let desired: Vec<(StopId, RankId, i32, VoiceSpec)> = self
                .voices_for_key(manual_index, key, None)
                .into_iter()
                .filter(|(_, _, _, spec)| !spec.percussive)
                .collect();
            let mut remaining = self
                .sounding
                .remove(&(manual_index, key))
                .unwrap_or_default();
            let mut entries = Vec::with_capacity(desired.len());
            let mut to_start = Vec::new();
            for (stop, rank, pipe, spec) in desired {
                // Each sounding entry holds one refcount, so match
                // multiset-style: a demand already held is kept, not
                // restarted.
                match remaining.iter().position(|&e| e == (stop, rank, pipe)) {
                    Some(at) => {
                        remaining.swap_remove(at);
                        entries.push((stop, rank, pipe));
                    }
                    None => to_start.push((stop, rank, pipe, spec)),
                }
            }
            for &(_, rank, pipe) in &remaining {
                stops.extend(self.release_pipe(rank, pipe));
            }
            for (stop, rank, pipe, spec) in to_start {
                entries.push((stop, rank, pipe));
                self.hold_pipe(rank, pipe, spec, starts, stops);
            }
            self.sounding.insert((manual_index, key), entries);
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
            // Stops are all retired by now, so releasing couplers can
            // only stop voices (the clack noise), never start any —
            // even a unison-off coupler has nothing to give back.
            let (released, _) = self.set_coupler(index, false);
            stopped.extend(released);
        }
        stopped
    }

    /// Every coupler with its engaged and on-console states, for UIs.
    pub fn coupler_states(&self) -> Vec<(usize, &str, bool, bool)> {
        self.organ
            .couplers
            .iter()
            .enumerate()
            .map(|(index, coupler)| {
                (
                    index,
                    coupler.name.as_str(),
                    self.engaged_couplers.contains(&index),
                    self.coupler_available(index),
                )
            })
            .collect()
    }

    /// Every *playable* stop with its manual (name and index) and drawn
    /// state, for UIs — control noises are hidden (they belong to
    /// their controls).
    pub fn stop_states(&self) -> Vec<(StopId, &str, &str, usize, bool)> {
        self.organ
            .stops
            .iter()
            .filter(|stop| !self.noise_stops.contains(&stop.id))
            .map(|stop| {
                let manual = self
                    .organ
                    .manuals
                    .iter()
                    .position(|m| m.id == stop.manual);
                (
                    stop.id,
                    stop.name.as_str(),
                    manual
                        .map(|index| self.organ.manuals[index].name.as_str())
                        .unwrap_or("?"),
                    manual.unwrap_or(usize::MAX),
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
                // The compass, not the set's own key count: a manual
                // widened to the player's keyboard has more keys to
                // draw, and they play.
                let (low, high) = self.compass[index];
                (
                    index,
                    manual.name.as_str(),
                    low.clamp(0, 127) as u8,
                    (high - low + 1).max(0) as u16,
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
                        nominal_hz: 440.0,
                        enclosure: aristide_engine::enclosure::ENCLOSURE_NONE,
                    },
                );
            }
        }
        Console::new(organ, specs, vec![StopId(1), StopId(2)], 48_000.0)
    }

    /// The fixture manual is MIDI 36..96. Widening it to a keyboard
    /// that runs past both ends is the case the whole feature exists
    /// for: a 56-note set under a 61-note keyboard.
    #[test]
    fn keys_past_the_set_are_repitched_from_the_pipes_that_exist() {
        let mut console = test_console();
        console.set_compass(0, 31, 101);

        let (top, _) = console.note_on_manual(0, 101);
        assert_eq!(top.len(), 2, "both stops speak five keys above the set");
        for start in &top {
            let semitones = start.spec.rate.log2() * 12.0;
            assert!(
                (semitones - 5.0).abs() < 1e-3,
                "top pipe stretched a fourth up, got {semitones} semitones"
            );
        }
        console.note_off_manual(0, 101);

        // Downward is the same mechanism and sounds better: a longer,
        // slower pipe is what the bottom of a keyboard wants anyway.
        let (bottom, _) = console.note_on_manual(0, 31);
        assert_eq!(bottom.len(), 2);
        for start in &bottom {
            let semitones = start.spec.rate.log2() * 12.0;
            assert!((semitones + 5.0).abs() < 1e-3, "got {semitones} semitones");
        }
        console.note_off_manual(0, 31);

        // Inside the set's own compass nothing is repitched at all.
        let (native, _) = console.note_on_manual(0, 60);
        assert!(native.iter().all(|s| (s.spec.rate - 1.0).abs() < 1e-6));
    }

    /// Two keys past the compass edge borrow the *same* physical pipe
    /// at two different pitches. They are two notes, and both must
    /// sound at once — a voice's identity is the key it stands in for,
    /// not the sample that happens to feed it.
    #[test]
    fn two_repitched_keys_off_the_same_pipe_sound_together() {
        let mut console = test_console();
        console.set_compass(0, 31, 101);

        let (first, _) = console.note_on_manual(0, 101);
        assert_eq!(first.len(), 2);
        let (second, _) = console.note_on_manual(0, 100);
        assert_eq!(
            second.len(),
            2,
            "the second repitched key must start its own voices"
        );
        for start in &second {
            let semitones = start.spec.rate.log2() * 12.0;
            assert!((semitones - 4.0).abs() < 1e-3, "got {semitones} semitones");
        }

        // Each key releases its own voices, not the other's.
        let released = console.note_off_manual(0, 101);
        assert_eq!(
            released,
            first.iter().map(|s| s.handle).collect::<Vec<_>>(),
            "the first key's voices stop while the second still holds"
        );
        let released = console.note_off_manual(0, 100);
        assert_eq!(released, second.iter().map(|s| s.handle).collect::<Vec<_>>());
    }

    #[test]
    fn keys_outside_the_compass_stay_silent() {
        let mut console = test_console();
        assert!(
            console.note_on_manual(0, 97).0.is_empty(),
            "the set's compass is the default, and 97 is past it"
        );
        console.set_compass(0, 36, 97);
        assert_eq!(console.note_on_manual(0, 97).0.len(), 2, "now it plays");
        console.note_off_manual(0, 97);
        assert!(
            console.note_on_manual(0, 98).0.is_empty(),
            "one key past the keyboard is still nothing"
        );
    }

    /// A missing sample mid-compass is a defect in the set, not a
    /// musical decision, so its neighbour stands in.
    #[test]
    fn a_hole_in_a_rank_is_filled_by_its_neighbour() {
        let mut console = test_console();
        console.specs.remove(&(RankId(1), 24));

        let (starts, _) = console.note_on_manual(0, 60);
        assert_eq!(starts.len(), 2, "the hole is filled, not skipped");
        let stretched: Vec<f32> = starts
            .iter()
            .map(|s| s.spec.rate.log2() * 12.0)
            .filter(|semitones| semitones.abs() > 1e-3)
            .collect();
        assert_eq!(stretched.len(), 1, "only the rank with the hole moves");
        assert!(
            stretched[0].abs() - 1.0 < 1e-3,
            "filled from a pipe one semitone away, got {}",
            stretched[0]
        );
    }

    /// Half-compass stops are real (divided registers, treble mixtures).
    /// Widening the keyboard must not invent the other half.
    #[test]
    fn a_stop_that_ends_early_is_not_extended() {
        let mut console = test_console();
        for stop in &mut console.organ.stops {
            if stop.id == StopId(2) {
                stop.ranks[0].key_count = 31;
            }
        }
        console.set_compass(0, 36, 101);

        assert_eq!(
            console.note_on_manual(0, 80).0.len(),
            1,
            "inside the set's compass the short stop simply doesn't cover it"
        );
        console.note_off_manual(0, 80);
        assert_eq!(
            console.note_on_manual(0, 101).0.len(),
            1,
            "and it is not carried past the set's compass either"
        );
    }

    /// A unit rank drawn at two pitches: Bourdon 16' on the Pedal, the
    /// same pipes again as a Bourdon 8' on the Swell. Each stop sees a
    /// window into one 73-pipe rank — the 16' the bottom 32, the 8'
    /// pipes 12..72.
    fn unit_rank_console() -> Console {
        let organ = Organ {
            name: "U".into(),
            base_path: Default::default(),
            manuals: vec![
                Manual {
                    id: ManualId(1),
                    name: "Pedal".into(),
                    first_midi_note: 36,
                    key_count: 32,
                },
                Manual {
                    id: ManualId(2),
                    name: "Swell".into(),
                    first_midi_note: 36,
                    key_count: 61,
                },
            ],
            stops: vec![
                Stop {
                    id: StopId(1),
                    name: "Bourdon 16".into(),
                    manual: ManualId(1),
                    ranks: vec![RankRange {
                        rank: RankId(1),
                        first_key: 0,
                        key_count: 32,
                        first_pipe: 0,
                    }],
                },
                Stop {
                    id: StopId(2),
                    name: "Bourdon 8".into(),
                    manual: ManualId(2),
                    ranks: vec![RankRange {
                        rank: RankId(1),
                        first_key: 0,
                        key_count: 61,
                        first_pipe: 12,
                    }],
                },
            ],
            ranks: vec![Rank {
                id: RankId(1),
                name: "Bourdon unit".into(),
                windchest: 1,
                pipes: (0..73)
                    .map(|_| Pipe {
                        nominal_frequency_hz: 440.0,
                        pitch_tuning_cents: 0.0,
                        gain_db: 0.0,
                        midi_key_number: None,
                        source: PipeSource::Silent,
                    })
                    .collect(),
            }],
            couplers: vec![],
            enclosures: vec![],
            windchests: vec![],
        };
        let mut specs = HashMap::new();
        for pipe in 0..73u16 {
            specs.insert(
                (RankId(1), pipe),
                VoiceSpec {
                    sample: 0,
                    rate: 1.0,
                    gain: 1.0,
                    percussive: false,
                    group: 0,
                    wind_weight: 1.0,
                    brightness: 0.02,
                    nominal_hz: 440.0,
                    enclosure: aristide_engine::enclosure::ENCLOSURE_NONE,
                },
            );
        }
        Console::new(organ, specs, vec![StopId(1), StopId(2)], 48_000.0)
    }

    /// Widening a manual reaches keys the *stop* never had — but when
    /// the stop's rank is a unit rank, the pipes those keys want are
    /// often real (the other stop's window covers them). A real pipe at
    /// true pitch always beats the window's edge pipe stretched to
    /// imitate it.
    #[test]
    fn extended_keys_use_the_real_pipes_of_a_shared_rank() {
        let mut console = unit_rank_console();

        // Pedal native 36..67, widened a fourth up. Key 72 wants pipe
        // 36 — past the 16' window, squarely inside the 8' treble.
        console.set_compass(0, 36, 72);
        let (starts, _) = console.note_on_manual(0, 72);
        assert_eq!(starts.len(), 1, "the extended pedal key speaks");
        assert!(
            (starts[0].spec.rate - 1.0).abs() < 1e-6,
            "pipe 36 exists in the unit rank; nothing may be repitched"
        );
        console.note_off_manual(0, 72);

        // Downward off the Swell 8': pipes 7..11 are the 16' bottom
        // the 8' window never reached, and they are equally real.
        console.set_compass(1, 31, 96);
        let (starts, _) = console.note_on_manual(1, 31);
        assert_eq!(starts.len(), 1);
        assert!((starts[0].spec.rate - 1.0).abs() < 1e-6);
        console.note_off_manual(1, 31);

        // Past the rank's REAL end the old rule still holds: swell key
        // 99 wants pipe 75 of 73, so the last pipe stretches to serve.
        console.set_compass(1, 31, 101);
        let (starts, _) = console.note_on_manual(1, 99);
        assert_eq!(starts.len(), 1);
        let semitones = starts[0].spec.rate.log2() * 12.0;
        assert!((semitones - 3.0).abs() < 1e-3, "got {semitones}");
    }

    /// Repitching fills in what the *player's keyboard* can reach. A
    /// coupler is not a keyboard: it may only sound pipes the division
    /// it points at actually has. A 16' coupler running off the bottom
    /// of a rank, or a coupler into a shorter division, sounds nothing
    /// there — inventing those pipes would be inventing an organ.
    #[test]
    fn couplers_never_repitch_by_default() {
        let mut console = coupled_console();
        console.set_compass(0, 24, 96); // a keyboard wider than the set
        console.set_coupler(1, true); // 16' I: −12 onto its own manual

        // The played key speaks at pitch, and its 16' copy speaks only
        // because that pipe exists.
        let (starts, _) = console.note_on_manual(0, 60);
        assert_eq!(starts.len(), 2, "the key and its 16' copy");
        assert!(
            starts.iter().all(|s| (s.spec.rate - 1.0).abs() < 1e-6),
            "nothing is repitched inside the rank"
        );
        console.note_off_manual(0, 60);

        // Twelve keys from the bottom, the 16' copy runs off the end of
        // the rank. The key itself still speaks; the copy does not.
        let (starts, _) = console.note_on_manual(0, 40);
        assert_eq!(
            starts.len(),
            1,
            "the 16' coupler must not repitch a pipe to reach below the rank"
        );
        assert!((starts[0].spec.rate - 1.0).abs() < 1e-6);
        console.note_off_manual(0, 40);

        // The same key played five below the set's own compass is
        // repitched — that is the player's keyboard, not a coupler.
        let (starts, _) = console.note_on_manual(0, 31);
        assert_eq!(starts.len(), 1, "only the direct voice, repitched");
        let semitones = starts[0].spec.rate.log2() * 12.0;
        assert!((semitones + 5.0).abs() < 1e-3, "got {semitones}");
    }

    /// Coupling into a division whose rank stops short: the keys past
    /// its end are silent there, however wide the keyboard is.
    #[test]
    fn coupling_into_a_shorter_division_stops_where_it_stops() {
        let mut console = coupled_console();
        for stop in &mut console.organ.stops {
            if stop.id == StopId(2) {
                stop.ranks[0].key_count = 49; // the Swell is a short rank
            }
        }
        console.set_coupler(0, true); // II/I, unison

        let (starts, _) = console.note_on_manual(0, 60);
        assert_eq!(starts.len(), 2, "both divisions have this key");
        console.note_off_manual(0, 60);

        let (starts, _) = console.note_on_manual(0, 90);
        assert_eq!(
            starts.len(),
            1,
            "past the Swell's last pipe only the Great speaks"
        );
    }

    /// Sets built for it can ask for the other behaviour. The compass
    /// still bounds the coupler either way — that rule is older.
    #[test]
    fn a_set_can_ask_couplers_to_repitch() {
        let mut console = coupled_console();
        console.set_coupler_repitch(true);
        console.set_compass(0, 24, 96);
        console.set_coupler(1, true); // 16' I

        let (starts, _) = console.note_on_manual(0, 40);
        assert_eq!(starts.len(), 2, "now the 16' copy is filled in");
        let stretched: Vec<f32> = starts
            .iter()
            .map(|s| s.spec.rate.log2() * 12.0)
            .filter(|semitones| semitones.abs() > 1e-3)
            .collect();
        assert_eq!(stretched.len(), 1);
        assert!((stretched[0] + 8.0).abs() < 1e-3, "got {}", stretched[0]);
    }

    #[test]
    fn note_on_starts_one_voice_per_drawn_stop() {
        let mut console = test_console();
        let (starts, _) = console.note_on_manual(0, 60);
        assert_eq!(starts.len(), 2);
        let stops = console.note_off_manual(0, 60);
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
        let (starts, _) = console.note_on_manual(0, 60);
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
        assert_eq!(console.note_off_manual(0, 60), vec![second]);
        assert!(console.manual_states()[0].4.is_empty());
    }

    #[test]
    fn drawing_a_stop_reaches_held_keys_through_couplers() {
        let mut console = coupled_console();
        console.set_drawn(StopId(1), false);
        console.set_drawn(StopId(2), false);
        console.set_coupler(0, true); // II/I
        assert!(console.note_on_manual(0, 60).0.is_empty());

        // The Swell stop drawn mid-hold sounds through the coupler.
        let (_, starts) = console.set_drawn(StopId(2), true);
        assert_eq!(starts.len(), 1);
        assert_eq!(starts[0].spec.sample, 1, "rank 2's sample expected");
        let handle = starts[0].handle;
        assert_eq!(console.note_off_manual(0, 60), vec![handle]);
    }

    #[test]
    fn keys_outside_the_manual_are_ignoredable() {
        let mut console = test_console();
        assert!(console.note_on_manual(0, 20).0.is_empty());
        assert!(console.note_on_manual(0, 120).0.is_empty());
        assert!(console.note_off_manual(0, 20).is_empty());
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
        let coupler = |name: &str, from: u32, to: u32, shift: i16| {
            aristide_model::Coupler::simple(name, ManualId(from), ManualId(to), shift)
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
                        nominal_hz: 440.0,
                        enclosure: aristide_engine::enclosure::ENCLOSURE_NONE,
                    },
                );
            }
        }
        Console::new(organ, specs, vec![StopId(1), StopId(2)], 48_000.0)
    }

    /// "A fourths coupler that plays a fourth down, but only from
    /// tenor C": a route with a source-key range.
    #[test]
    fn a_coupler_can_act_on_a_key_range_only() {
        let mut console = coupled_console();
        console.organ.couplers.push(aristide_model::Coupler {
            name: "Fourths II/I from tenor C".into(),
            routes: vec![aristide_model::CouplerRoute {
                from_manual: ManualId(1),
                low_key: Some(48),
                high_key: None,
                unison_off: false,
                target: Some(aristide_model::CouplerTarget {
                    manual: ManualId(2),
                    key_shift: -5,
                    repitch: None,
                }),
            }],
        });
        let index = console.organ.couplers.len() - 1;
        console.set_coupler(index, true);

        // Below tenor C the coupler simply isn't there.
        assert_eq!(console.note_on_manual(0, 47).0.len(), 1);
        console.note_off_manual(0, 47);

        // From tenor C up: the played key plus its fourth-down copy on
        // II — a real pipe, nothing repitched.
        let (starts, _) = console.note_on_manual(0, 48);
        assert_eq!(starts.len(), 2);
        let copy = starts.iter().find(|s| s.spec.sample == 1).expect("II speaks");
        assert!((copy.spec.rate - 1.0).abs() < 1e-6);
        console.note_off_manual(0, 48);
    }

    /// "A 16' coupler which transposes the bottom octave down instead
    /// of leaving it on": two routes — a classic doubling above the
    /// break, and below it a unison-off + repitch route that *moves*
    /// the note down, inventing the pipes the rank hasn't got.
    #[test]
    fn a_sixteen_foot_that_transposes_the_bottom_octave() {
        let mut console = coupled_console();
        let split = 36 + 12; // an octave above the fixture's bottom key
        let target = |repitch| {
            Some(aristide_model::CouplerTarget {
                manual: ManualId(1),
                key_shift: -12,
                repitch,
            })
        };
        console.organ.couplers.push(aristide_model::Coupler {
            name: "16' I".into(),
            routes: vec![
                aristide_model::CouplerRoute {
                    from_manual: ManualId(1),
                    low_key: Some(split),
                    high_key: None,
                    unison_off: false,
                    target: target(None),
                },
                aristide_model::CouplerRoute {
                    from_manual: ManualId(1),
                    low_key: None,
                    high_key: Some(split - 1),
                    unison_off: true,
                    target: target(Some(true)),
                },
            ],
        });
        let index = console.organ.couplers.len() - 1;
        console.set_coupler(index, true);

        // Above the break: the classic doubling, real pipes only.
        let (starts, _) = console.note_on_manual(0, 60);
        assert_eq!(starts.len(), 2, "the key and its 16' copy");
        assert!(starts.iter().all(|s| (s.spec.rate - 1.0).abs() < 1e-6));
        console.note_off_manual(0, 60);

        // In the bottom octave the note moves: one voice, sounding an
        // octave below the played key, bent down from the deepest pipe
        // the rank has — past the compass, because this route asked to.
        let (starts, _) = console.note_on_manual(0, 40);
        assert_eq!(starts.len(), 1, "unison off: the played key itself is silent");
        let semitones = starts[0].spec.rate.log2() * 12.0;
        assert!(
            (semitones + 8.0).abs() < 1e-3,
            "pipe 0 bent down to sound an octave below key 40, got {semitones}"
        );
        console.note_off_manual(0, 40);
    }

    /// Engaging or releasing a coupler lands on held notes at once,
    /// as an electric-action console does — the same way drawing a
    /// stop mid-hold already behaves.
    #[test]
    fn coupler_changes_land_on_held_notes() {
        let mut console = coupled_console();
        let (starts, _) = console.note_on_manual(0, 60);
        assert_eq!(starts.len(), 1, "only the Great before coupling");

        // Engaging II/I under the held key speaks the Swell at once.
        let (stopped, starts) = console.set_coupler(0, true);
        assert!(stopped.is_empty());
        assert_eq!(starts.len(), 1);
        assert_eq!(starts[0].spec.sample, 1, "rank 2's sample");
        let swell_voice = starts[0].handle;

        // Releasing it lets go of exactly that voice.
        let (stopped, starts) = console.set_coupler(0, false);
        assert!(starts.is_empty());
        assert_eq!(stopped, vec![swell_voice]);

        // The key still sounds its own pipe, and note-off finds it.
        assert_eq!(console.note_off_manual(0, 60).len(), 1);
    }

    /// A pure unison-off coupler (GO's `UnisonOff=Y`): the manual's own
    /// sound is silenced, held notes included, and given back on
    /// release.
    #[test]
    fn a_unison_off_coupler_moves_held_notes() {
        let mut console = coupled_console();
        console.organ.couplers.push(aristide_model::Coupler {
            name: "Unison Off I".into(),
            routes: vec![aristide_model::CouplerRoute {
                from_manual: ManualId(1),
                low_key: None,
                high_key: None,
                unison_off: true,
                target: None,
            }],
        });
        let index = console.organ.couplers.len() - 1;

        let (starts, _) = console.note_on_manual(0, 60);
        let direct = starts[0].handle;

        // Engaging unison-off silences the held key's own division…
        let (stopped, starts) = console.set_coupler(index, true);
        assert_eq!(stopped, vec![direct]);
        assert!(starts.is_empty());

        // …and releasing it gives the note back. The silenced voice's
        // release tail is expedited so the pipe can't overlap itself.
        let (stopped, starts) = console.set_coupler(index, false);
        assert_eq!(stopped, vec![direct]);
        assert_eq!(starts.len(), 1);
        assert_eq!(console.note_off_manual(0, 60).len(), 1);
    }

    #[test]
    fn couplers_route_between_manuals_and_octaves() {
        let mut console = coupled_console();
        // Channel 0 → Great (no pedal in this organ → identity map).
        assert_eq!(console.note_on_manual(0, 60).0.len(), 1, "no couplers yet");
        console.note_off_manual(0, 60);

        console.set_coupler(0, true); // II/I
        assert_eq!(console.note_on_manual(0, 60).0.len(), 2, "unison coupler adds II");
        assert_eq!(console.note_off_manual(0, 60).len(), 2, "note-off kills both");

        console.set_coupler(1, true); // 16' I (self, −12)
        // Great C + Swell C (II/I) + Great C−12 (16' I). Coupled notes
        // don't re-couple, so the sub-octave stays on the Great.
        assert_eq!(console.note_on_manual(0, 60).0.len(), 3);
        console.note_off_manual(0, 60);

        // Out-of-compass shifted notes drop out quietly.
        assert_eq!(console.note_on_manual(0, 37).0.len(), 2, "37-12 is below compass");
        console.note_off_manual(0, 37);
    }

    #[test]
    fn tuning_retunes_and_transposes() {
        let mut console = test_console();
        // Equal temperament, a=440: everything at unity rate.
        let baseline = console.note_on_manual(0, 60).0[0].spec.rate;
        assert!((baseline - 1.0).abs() < 1e-6);
        console.note_off_manual(0, 60);

        // Meantone C sits +10.265 cents above equal (a-referenced).
        console.set_tuning(crate::tuning::Tuning {
            temperament: crate::tuning::Temperament::Meantone4,
            a4_hz: 440.0,
            transpose: 0,
        });
        let meantone_c = console.note_on_manual(0, 60).0[0].spec.rate;
        let expected = (10.265f32 / 1200.0).exp2();
        assert!(
            (meantone_c - expected).abs() < 1e-4,
            "meantone C rate {meantone_c} vs {expected}"
        );
        console.note_off_manual(0, 60);

        // Transpose +2: key 60 routes to pipe 62 (rate reflects D's
        // offset, and the sounding pipe index shifts).
        console.set_tuning(crate::tuning::Tuning {
            temperament: crate::tuning::Temperament::Equal,
            a4_hz: 440.0,
            transpose: 2,
        });
        let (transposed, _) = console.note_on_manual(0, 60);
        assert_eq!(transposed.len(), 2, "both drawn stops sound");
        // Pipe index = key 62 − first_midi 36 = 26; sample index equals
        // rank − 1 in the fixture, so instead verify by keying at the
        // compass edge: 96 + 2 is out of range → silent.
        console.note_off_manual(0, 60);
        assert!(console.note_on_manual(0, 96).0.is_empty(), "96+2 exceeds compass");
    }

    /// One instrument, two pitches: the Swell tuned apart speaks its
    /// own temperament even when a coupler reaches it from the Great —
    /// pipes are tuned where they stand. And a per-division transpose
    /// moves only its own keyboard.
    #[test]
    fn a_division_can_be_tuned_apart() {
        let mut console = coupled_console();
        console.set_manual_tuning(
            1,
            Some(crate::tuning::Tuning {
                temperament: crate::tuning::Temperament::Meantone4,
                a4_hz: 440.0,
                transpose: 0,
            }),
        );
        console.set_coupler(0, true); // II/I: playing the Great adds the Swell
        let (starts, _) = console.note_on_manual(0, 60);
        assert_eq!(starts.len(), 2);
        let mut rates: Vec<f32> = starts.iter().map(|s| s.spec.rate).collect();
        rates.sort_by(f32::total_cmp);
        let meantone_c = (10.265f32 / 1200.0).exp2();
        assert!((rates[0] - 1.0).abs() < 1e-6, "Great stays equal: {rates:?}");
        assert!(
            (rates[1] - meantone_c).abs() < 1e-4,
            "coupled copy speaks the Swell's meantone: {rates:?}"
        );
        console.note_off_manual(0, 60);
        console.set_coupler(0, false);

        console.set_manual_tuning(
            0,
            Some(crate::tuning::Tuning {
                temperament: crate::tuning::Temperament::Equal,
                a4_hz: 440.0,
                transpose: 2,
            }),
        );
        assert!(console.note_on_manual(0, 96).0.is_empty(), "96+2 runs off the Great");
        assert_eq!(console.note_on_manual(1, 96).0.len(), 1, "the Swell is unmoved");
        // Back on the shared tuning, the Great answers again.
        console.set_manual_tuning(0, None);
        assert_eq!(console.note_on_manual(0, 96).0.len(), 1);
    }

    /// Moving a stop re-homes it mid-hold: the key holding its new
    /// manual picks it up, its old manual gives it up.
    #[test]
    fn moving_a_stop_rehomes_it_under_held_keys() {
        let mut console = coupled_console();
        let (starts, _) = console.note_on_manual(0, 60);
        assert_eq!(starts.len(), 1, "only the Great's own stop");
        let (stopped, starts) = console.move_stop(StopId(2), 0);
        assert!(stopped.is_empty(), "nothing sounded on the Swell");
        assert_eq!(starts.len(), 1, "the moved stop speaks under the held key");
        assert!(console.note_on_manual(1, 62).0.is_empty(), "the Swell gave it up");
        assert_eq!(console.note_off_manual(0, 60).len(), 2);
        assert_eq!(console.stop_states()[1].3, 0, "stop 2 reports the Great");
    }

    /// A coupler taken off the console releases, hides, and refuses
    /// engagement — and comes back whole when restored.
    #[test]
    fn a_coupler_off_the_console_stays_restorable() {
        let mut console = coupled_console();
        console.set_coupler(0, true);
        assert_eq!(console.note_on_manual(0, 60).0.len(), 2);
        console.note_off_manual(0, 60);

        console.set_coupler_available(0, false);
        assert!(!console.coupler_engaged(0));
        assert!(!console.coupler_states()[0].3);
        assert_eq!(console.note_on_manual(0, 60).0.len(), 1);
        console.note_off_manual(0, 60);
        console.set_coupler(0, true);
        assert!(!console.coupler_engaged(0), "off the console means unpullable");

        console.set_coupler_available(0, true);
        console.set_coupler(0, true);
        assert_eq!(console.note_on_manual(0, 60).0.len(), 2);
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

        let (first, _) = console.note_on_manual(0, 72);
        assert_eq!(first.len(), 2, "72 direct + coupled 60");
        let (second, _) = console.note_on_manual(0, 60);
        assert_eq!(
            second.len(),
            1,
            "60's direct pipe already speaks via 72's coupling — only \
             the new 48-pipe may start"
        );

        // Releasing 72 must NOT stop the shared pipe (60 still holds it).
        let stopped = console.note_off_manual(0, 72);
        assert_eq!(stopped.len(), 1, "only 72's unshared pipe stops");
        // Releasing 60 stops the shared pipe and 60's own coupled pipe.
        let stopped = console.note_off_manual(0, 60);
        assert_eq!(stopped.len(), 2, "shared pipe + 48-pipe stop last");

        // Every started voice eventually stopped exactly once.
        assert!(console.note_off_manual(0, 60).is_empty());
        assert!(console.note_off_manual(0, 72).is_empty());
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

        let (starts, _) = console.note_on_manual(0, 72); // pipes 36 and 24
        let shared_pipe_voice = starts
            .iter()
            .map(|s| s.handle)
            .max()
            .expect("voices started");
        let released = console.note_off_manual(0, 72);
        assert_eq!(released.len(), 2);

        // Immediately press 60 → its direct pipe IS 72's coupled pipe.
        let (_, expedited) = console.note_on_manual(0, 60);
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
        assert_eq!(console.note_on_manual(0, 60).0.len(), 2);
        console.note_off_manual(0, 60);
    }

    #[test]
    fn retrigger_stops_previous_voices_first() {
        // A re-press before note-off (key bounce, fast repetition) must
        // release the first press's voices — a pipe can't speak twice,
        // and doubling correlated audio is an instant +6 dB.
        let mut console = test_console();
        let (first, retriggered) = console.note_on_manual(0, 60);
        assert_eq!(first.len(), 2);
        assert!(retriggered.is_empty());
        let first_handles: Vec<u64> = first.iter().map(|s| s.handle).collect();

        let (second, retriggered) = console.note_on_manual(0, 60);
        assert_eq!(second.len(), 2);
        assert_eq!(retriggered, first_handles, "old voices released");

        // Note-off stops only the live (second) voices.
        let stopped = console.note_off_manual(0, 60);
        assert_eq!(
            stopped,
            second.iter().map(|s| s.handle).collect::<Vec<_>>()
        );
        assert!(console.note_off_manual(0, 60).is_empty());
    }
}
