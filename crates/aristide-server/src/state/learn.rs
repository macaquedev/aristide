//! Binding resolution and performance controls: turning saved MIDI/key
//! assignments into the live route table, dispatching bindings and
//! computer-keyboard keys, the whole combination action (generals,
//! divisionals, the stepper, the crescendo), tremulant engagement, and
//! the MIDI-learn gestures that teach a new keyboard or a new control.

use std::time::Instant;

use aristide_engine::Command;
use aristide_model::units::ratio_to_cents;
use aristide_model::StopId;

use super::{Control, KeyboardInput, Pending, Resolution, State};
use crate::bindings::{
    channels_overlap, normalize_input, Binding, ControlLearn, Learn, MidiPort, Route, Subject,
    COMPUTER_KEYBOARD, LEARN_TIMEOUT,
};
use crate::{config, control};

/// How far a stored registration reaches. A general covers the whole
/// console; a divisional covers one division — its stops always, its
/// couplers and tremulants only where the console is wired that way
/// (see [`aristide_model::CombinationScope`]).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Scope {
    Whole,
    Division(usize),
}

/// The controls one combination reaches, each as `(id, name, on)`.
/// Storing reads the `on`s; recalling diffs against them.
struct Members {
    stops: Vec<(StopId, String, bool)>,
    couplers: Vec<(usize, String, bool)>,
    trems: Vec<(usize, String, bool)>,
}

/// The names of the members that are on — what a store writes down.
fn engaged_names<T>(members: &[(T, String, bool)]) -> Vec<String> {
    members
        .iter()
        .filter(|(_, _, on)| *on)
        .map(|(_, name, _)| name.clone())
        .collect()
}

/// Stored names this console cannot answer for, described for the log.
/// They stay in the file: a name that means nothing today means
/// something again the moment its stop comes back.
fn absent_names<T>(kind: &str, wanted: &[String], members: &[(T, String, bool)]) -> Vec<String> {
    wanted
        .iter()
        .filter(|name| !members.iter().any(|(_, known, _)| known == *name))
        .map(|name| format!("{kind} {name:?}"))
        .collect()
}

fn report_missing(what: &str, missing: &[String]) {
    if !missing.is_empty() {
        tracing::warn!("{what}: not on this console: {}", missing.join(", "));
    }
}

impl State {
    /// Push the saved assignments into the connected ports. Every edit
    /// goes through the config, so this is the one place routing is
    /// derived and the MIDI callback never has to look at names.
    pub(crate) fn resolve_routes(&mut self) {
        self.resolve_bindings();
        let assignments = self.saved_assignments();
        // Lumatone maps load once per path, against the organ file's
        // directory like scale files. A file that fails to load warns
        // and leaves its input deaf — never bricking the organ.
        let base = self
            .composite_path
            .as_deref()
            .and_then(std::path::Path::parent)
            .map(std::path::Path::to_path_buf);
        for (_, inputs) in &assignments {
            for input in inputs {
                let Some(path) = &input.map else { continue };
                if self.ltn_cache.contains_key(path) {
                    continue;
                }
                let file = std::path::Path::new(path);
                let resolved = match (file.is_relative(), &base) {
                    (true, Some(base)) => base.join(file),
                    _ => file.to_path_buf(),
                };
                let loaded = std::fs::read_to_string(&resolved)
                    .map_err(|err| format!("{}: {err}", resolved.display()))
                    .and_then(|text| aristide_model::lumatone::LumatoneMap::parse(&text));
                let entry = match loaded {
                    Ok(map) => {
                        for warning in &map.warnings {
                            tracing::warn!("lumatone map {path}: {warning}");
                        }
                        tracing::info!(
                            "lumatone map {path}: {} keys over {} channels",
                            map.key_count(),
                            map.channels().count()
                        );
                        Some(std::sync::Arc::new(map))
                    }
                    Err(err) => {
                        tracing::warn!(
                            "lumatone map {path} not loaded: {err} — the input stays silent"
                        );
                        None
                    }
                };
                self.ltn_cache.insert(path.clone(), entry);
            }
        }
        let native: Vec<(u8, u8)> = self
            .native_compass()
            .iter()
            .enumerate()
            .map(|(manual, own)| self.compass_override(manual).unwrap_or(*own))
            .collect();
        let ltn_cache = &self.ltn_cache;
        for port in &mut self.midi_ports {
            port.routes = assignments
                .iter()
                .flat_map(|(manual, inputs)| {
                    inputs
                        .iter()
                        .filter(|input| input.device == port.name)
                        .map(|input| Route {
                            channel: input.channel,
                            manual: *manual,
                            // A keyboard nobody has measured is assumed
                            // to be exactly the organ's own compass.
                            keys: input.compass().unwrap_or(native[*manual]),
                            transpose: input.transpose,
                            bend: input.bend,
                            map: input
                                .map
                                .as_ref()
                                .and_then(|path| ltn_cache.get(path).cloned().flatten()),
                        })
                })
                .collect();
        }
        // A manual answers to every key any of its keyboards can send:
        // that union is the compass the console plays and the UI draws,
        // and it is where repitching starts filling in. A shifted
        // keyboard reaches shifted pipes, so the shift is part of it —
        // otherwise pressing octave-up would just make the top of the
        // keyboard silent.
        // A declared compass is the floor of the union: a narrow
        // learned keyboard must not shrink the manual below it.
        let mut widened: Vec<Option<(i16, i16)>> = (0..native.len())
            .map(|manual| {
                self.compass_override(manual)
                    .map(|(low, high)| (low as i16, high as i16))
            })
            .collect();
        for port in &self.midi_ports {
            for route in &port.routes {
                // A mapped keyboard reaches the map's extended-note
                // span; a plain one its (learned) MIDI compass.
                let reach: (i16, i16) = if let Some(map) = &route.map {
                    let Some((low, high)) = MidiPort::map_reach(map) else {
                        continue;
                    };
                    let shift = |key: u16| {
                        (key as i32 + route.transpose as i32).clamp(0, i16::MAX as i32) as i16
                    };
                    (shift(low), shift(high))
                } else {
                    let shift =
                        |key: u8| (key as i16 + route.transpose as i16).clamp(0, 127);
                    (shift(route.keys.0), shift(route.keys.1))
                };
                let slot = &mut widened[route.manual];
                *slot = Some(match *slot {
                    Some((low, high)) => (low.min(reach.0), high.max(reach.1)),
                    None => reach,
                });
            }
        }
        // The computer keyboard is assigned like any other input — or
        // not at all: unassigned, its keys play nothing. Unlike a MIDI
        // keyboard it never counts towards a manual's compass. Widening
        // serves real hardware (a 61-note keyboard on a 56-note set);
        // two QWERTY rows are not a console, and letting them reshape
        // the instrument would rescale a manual nobody asked to change.
        // Keys past the manual's end simply stay silent, and the legend
        // already draws them as unavailable.
        self.keyboard = assignments
            .iter()
            .flat_map(|(manual, inputs)| {
                inputs
                    .iter()
                    .filter(|input| input.device == COMPUTER_KEYBOARD)
                    .map(|input| KeyboardInput {
                        manual: *manual,
                        transpose: input.transpose,
                        compass: control::keyboard_compass(),
                    })
            })
            .collect();
        if let Control::Organ(console) = &mut self.control {
            for (manual, compass) in widened.into_iter().enumerate() {
                match compass {
                    Some((low, high)) => console.set_compass(manual, low, high),
                    None => console.reset_compass(manual),
                }
            }
        }
    }

    /// A computer key, pressed or released, through exactly the path a
    /// MIDI message takes: a binding first, then a note on whichever
    /// manual the computer keyboard is assigned to.
    ///
    /// The mapping lives here rather than in the console UI so that one
    /// binding table governs both, and so an octave button on a MIDI
    /// console can shift the computer keyboard as readily as `=` can.
    pub fn key(&mut self, code: &str, pressed: bool) {
        let fired: Vec<Binding> = self
            .key_bindings
            .iter()
            .filter(|binding| binding.trigger == control::Trigger::Key(code.to_string()))
            .cloned()
            .collect();
        if !fired.is_empty() {
            // A key that does something does not also play: releasing it
            // must not sound the note its press didn't.
            if pressed {
                for binding in fired {
                    self.run(&binding, COMPUTER_KEYBOARD, 127);
                }
            }
            return;
        }
        // What a code means depends on the manual it addresses: a hand
        // keyboard reads the two letter rows as a piano; a microtonal
        // manual reads all four rows as a window onto its own hex grid
        // (see control::KEYBOARD_GRID). Both resolved up front — the
        // keyboard may drive one of each.
        let piano = control::key_note(code);
        let grid = control::key_grid(code);
        if piano.is_none() && grid.is_none() {
            return;
        }
        // The keyboard may drive more than one manual — a confirmed
        // "keep both" — and each assignment carries its own shift.
        for keyboard in self.keyboard.clone() {
            let State {
                engine, control, ..
            } = &mut *self;
            let Control::Organ(console) = control else {
                return;
            };
            let landed: Option<i32> = match console.manual_hex(keyboard.manual) {
                // The grid position asked of the manual's own layout,
                // in the slanted reading that matches how QWERTY rows
                // physically sit (see control::KEYBOARD_GRID) — so the
                // board's shapes lie under the fingers unskewed.
                Some(layout) => grid
                    .map(|(col, row)| layout.key_at_slanted(col, row) + keyboard.transpose as i32),
                None => piano.map(|note| note as i32 + keyboard.transpose as i32)
                    .filter(|key| (0..=127).contains(key)),
            };
            let Some(key) = landed.and_then(|key| u16::try_from(key).ok()) else {
                continue;
            };
            if pressed {
                // The compass rule, exactly as MIDI routing applies it: a
                // key landing outside the manual says nothing, and is not
                // tracked as held either. The keyboard never widens the
                // manual to reach it — the legend draws it as unavailable.
                let within = console
                    .compass(keyboard.manual)
                    .is_some_and(|(low, high)| (low..=high).contains(&(key as i16)));
                if !within {
                    continue;
                }
                // A clicked key has no velocity; full, as GO's
                // on-screen console sends.
                let (starts, retriggered) =
                    console.note_on_manual(keyboard.manual, key, 127);
                for handle in retriggered {
                    engine.send(Command::StopVoice { handle });
                }
                for start in starts {
                    engine.send(start.command());
                }
            } else {
                let (stopped, starts) =
                    console.note_off_manual(keyboard.manual, key);
                for handle in stopped {
                    engine.send(Command::StopVoice { handle });
                }
                // A Bass/Melody coupler retargeting onto another held key.
                for start in starts {
                    engine.send(start.command());
                }
            }
        }
    }

    /// Run one action by name, as if a binding had fired it — the menu
    /// and a piston must not be able to mean different things.
    pub fn run_named(&mut self, action: &control::Action, device: &str) -> bool {
        let Some(subject) = self.resolve_subject(action, None) else {
            return false;
        };
        self.run(
            &Binding {
                channel: None,
                trigger: control::Trigger::Note(0),
                action: action.clone(),
                subject,
            },
            device,
            127,
        );
        true
    }

    /// Turn the saved bindings into the form the callbacks match
    /// against, looking every name up once. A binding naming something
    /// this organ hasn't got is reported and dropped from the live
    /// table — but stays in the file, because it is about a different
    /// instrument, not a mistake.
    fn resolve_bindings(&mut self) {
        let organ = self.organ_key.clone();
        let mut by_device: std::collections::HashMap<String, Vec<Binding>> = Default::default();
        for saved in self.midi_config.controls(&organ).to_vec() {
            if saved.trigger.trim().is_empty() {
                continue; // a row the player is still setting up
            }
            let (Some(trigger), Some(action)) = (
                control::Trigger::parse(&saved.trigger),
                control::Action::parse(&saved.action),
            ) else {
                tracing::warn!(
                    "control: {:?} → {:?} is not something Aristide knows how to do",
                    saved.trigger,
                    saved.action
                );
                continue;
            };
            let Some(subject) = self.resolve_subject(&action, saved.manual.as_deref()) else {
                tracing::warn!(
                    "control: {:?} names something this organ hasn't got — ignoring it",
                    saved.action
                );
                continue;
            };
            by_device.entry(saved.device.clone()).or_default().push(Binding {
                channel: saved.channel,
                trigger,
                action,
                subject,
            });
        }
        for port in &mut self.midi_ports {
            port.bindings = by_device.remove(&port.name).unwrap_or_default();
        }
        // The computer keyboard is a device like any other; it just
        // isn't one the operating system enumerates.
        self.key_bindings = by_device.remove(COMPUTER_KEYBOARD).unwrap_or_default();
    }

    /// The thing an action acts on, looked up in the loaded organ.
    fn resolve_subject(&self, action: &control::Action, manual: Option<&str>) -> Option<Subject> {
        let Control::Organ(console) = &self.control else {
            return Some(Subject::None);
        };
        let one = |names: &[&str], pattern: &str| {
            match aristide_formats::sidecar::match_names(names, pattern).as_slice() {
                [index] => Some(*index),
                _ => None,
            }
        };
        Some(match action {
            control::Action::Stop(name) => {
                let stops = console.stop_states();
                let names: Vec<&str> = stops.iter().map(|(_, name, _, _, _)| *name).collect();
                Subject::Stop(stops[one(&names, name)?].0)
            }
            control::Action::Coupler(name) => {
                let couplers = console.coupler_states();
                let names: Vec<&str> = couplers.iter().map(|(_, name, _, _)| *name).collect();
                Subject::Coupler(couplers[one(&names, name)?].0)
            }
            control::Action::Enclosure(name) => {
                let boxes = console.enclosure_states();
                let names: Vec<String> = boxes.iter().map(|(_, name, _, _)| name.clone()).collect();
                let names: Vec<&str> = names.iter().map(String::as_str).collect();
                Subject::Enclosure(boxes[one(&names, name)?].0)
            }
            control::Action::Tremulant(Some(name)) => {
                let names: Vec<&str> = self.trems.iter().map(|t| t.name.as_str()).collect();
                Subject::Tremulant(one(&names, name)?)
            }
            control::Action::Transpose(_) | control::Action::TransposeReset => match manual {
                Some(name) => {
                    let names = self.manual_names();
                    let names: Vec<&str> = names.iter().map(String::as_str).collect();
                    Subject::Manual(one(&names, name)?)
                }
                None => Subject::Device,
            },
            // A divisional names its manual in the action itself — the
            // piston is physically under that keyboard, so the binding
            // carries the division, not the device's default.
            control::Action::Divisional(name, _) => {
                let names = self.manual_names();
                let names: Vec<&str> = names.iter().map(String::as_str).collect();
                Subject::Manual(one(&names, name)?)
            }
            _ => Subject::None,
        })
    }

    /// Each manual's compass as the sample set declares it.
    pub(crate) fn native_compass(&self) -> Vec<(u8, u8)> {
        match &self.control {
            Control::Organ(console) => (0..console.manual_states().len())
                .map(|manual| {
                    console
                        .native_compass(manual)
                        .map(|(low, high)| (low.clamp(0, 127) as u8, high.clamp(0, 127) as u8))
                        .unwrap_or((0, 127))
                })
                .collect(),
            Control::Tone => Vec::new(),
        }
    }

    /// The saved inputs of one manual, by index — what the UI edits by
    /// slot. Manuals the organ has but the file doesn't mention come
    /// back empty.
    pub fn manual_inputs(&self, manual: usize) -> Vec<config::Input> {
        self.manual_names()
            .get(manual)
            .map(|name| self.midi_config.inputs(&self.organ_key, name).to_vec())
            .unwrap_or_default()
    }

    /// The bind path every UI edit takes: commit `input`, unless the
    /// same device on an overlapping channel already plays another row
    /// — then park it and ask whether the device now drives both. A
    /// row's identity is its device and channel: an edit that keeps
    /// them (a shift, a compass) never asks, so answering "keep both"
    /// once is answered for good. Returns false when the manual
    /// doesn't exist.
    pub fn propose_input(&mut self, manual: usize, slot: usize, mut input: config::Input) -> bool {
        self.pending = None;
        let names = self.manual_names();
        let Some(name) = names.get(manual) else {
            return false;
        };
        normalize_input(&mut input);
        let organ = self.organ_key.clone();
        let saved = self.midi_config.inputs(&organ, name).get(slot);
        let identity_kept =
            saved.is_some_and(|s| s.device == input.device && s.channel == input.channel);
        if !identity_kept {
            let mut existing = Vec::new();
            for (other_manual, other_name) in names.iter().enumerate() {
                for (other_slot, other) in
                    self.midi_config.inputs(&organ, other_name).iter().enumerate()
                {
                    if other_manual == manual && other_slot == slot {
                        continue; // the row being rewritten
                    }
                    if other.device == input.device
                        && channels_overlap(other.channel, input.channel)
                    {
                        existing.push((other_manual, other_slot));
                    }
                }
            }
            if !existing.is_empty() {
                tracing::info!("midi: {} already plays elsewhere — asking", input.device);
                self.pending = Some(Pending::Input {
                    manual,
                    slot,
                    input,
                    existing,
                });
                return true;
            }
        }
        self.set_input(manual, slot, input)
    }

    /// Act on the player's answer to a parked bind. `false` when there
    /// was nothing pending — the dialog raced an organ load or another
    /// edit, and there is nothing left to act on.
    pub fn resolve_pending(&mut self, resolution: Resolution) -> bool {
        let Some(pending) = self.pending.take() else {
            return false;
        };
        match (resolution, pending) {
            (Resolution::Cancel, _) => {}
            (
                Resolution::KeepBoth,
                Pending::Input {
                    manual,
                    slot,
                    input,
                    ..
                },
            ) => {
                self.set_input(manual, slot, input);
            }
            (Resolution::KeepBoth, Pending::Control { slot, control, .. }) => {
                self.set_control(slot, control);
            }
            (
                Resolution::Replace,
                Pending::Input {
                    manual,
                    mut slot,
                    mut input,
                    mut existing,
                },
            ) => {
                let organ = self.organ_key.clone();
                let names = self.manual_names();
                // The rows being replaced knew things about the hardware
                // itself — a learned compass, a shift. Replacing means
                // "this keyboard now plays here instead", so those facts
                // move with it unless the new row states its own.
                if let Some(&(other_manual, other_slot)) = existing.first()
                    && let Some(other_name) = names.get(other_manual)
                    && let Some(old) = self.midi_config.inputs(&organ, other_name).get(other_slot)
                {
                    if input.transpose == 0 {
                        input.transpose = old.transpose;
                    }
                    if input.low.is_none() && input.high.is_none() {
                        input.low = old.low;
                        input.high = old.high;
                    }
                    normalize_input(&mut input);
                }
                // Remove bottom-up so earlier removals never shift the
                // later slots; the target slides down past any removed
                // row beneath it on its own manual.
                existing.sort_unstable();
                for &(other_manual, other_slot) in existing.iter().rev() {
                    if let Some(other_name) = names.get(other_manual) {
                        self.midi_config.remove_input(&organ, other_name, other_slot);
                    }
                    if other_manual == manual && other_slot < slot {
                        slot -= 1;
                    }
                }
                self.set_input(manual, slot, input);
            }
            (
                Resolution::Replace,
                Pending::Control {
                    mut slot,
                    control,
                    mut existing,
                },
            ) => {
                let organ = self.organ_key.clone();
                existing.sort_unstable();
                for &other in existing.iter().rev() {
                    self.midi_config.remove_control(&organ, other);
                    if other < slot {
                        slot -= 1;
                    }
                }
                self.set_control(slot, control);
            }
        }
        true
    }

    /// Assign `input` to one manual's slot (past the end appends), then
    /// re-resolve and save. Returns false when the manual doesn't exist.
    pub fn set_input(&mut self, manual: usize, slot: usize, mut input: config::Input) -> bool {
        let Some(name) = self.manual_names().get(manual).cloned() else {
            return false;
        };
        normalize_input(&mut input);
        tracing::info!(
            "midi: {name} ← {} channel {}",
            input.device,
            input
                .channel
                .map_or_else(|| "any".to_string(), |c| c.to_string())
        );
        let organ = self.organ_key.clone();
        self.midi_config.set_input(&organ, &name, slot, input);
        self.resolve_routes();
        self.persist();
        true
    }

    pub fn remove_input(&mut self, manual: usize, slot: usize) -> bool {
        self.pending = None;
        let Some(name) = self.manual_names().get(manual).cloned() else {
            return false;
        };
        let organ = self.organ_key.clone();
        self.midi_config.remove_input(&organ, &name, slot);
        self.resolve_routes();
        self.persist();
        true
    }

    /// Do what a binding says. `value` is the message's own (a
    /// controller's position, a note's velocity); only the continuous
    /// actions read it.
    ///
    /// This is the only place an action becomes an effect, so a
    /// binding, a menu item and a future script all mean exactly the
    /// same thing by "cancel".
    pub fn run(&mut self, binding: &Binding, device: &str, value: u8) {
        let State {
            engine, control, ..
        } = &mut *self;
        let mut send = |command: Command| {
            if !engine.send(command) {
                tracing::warn!("command queue full, dropped {command:?}");
            }
        };
        match (&binding.action, binding.subject) {
            (control::Action::Transpose(by), subject) => {
                self.transpose_inputs(device, subject, |at| at.saturating_add(*by));
            }
            (control::Action::TransposeReset, subject) => {
                self.transpose_inputs(device, subject, |_| 0);
            }
            (control::Action::Stop(_), Subject::Stop(stop)) => {
                let Control::Organ(console) = control else {
                    return;
                };
                let drawn = console.is_drawn(stop);
                let (stopped, starts) = console.set_drawn(stop, !drawn);
                for handle in stopped {
                    send(Command::StopVoice { handle });
                }
                for start in starts {
                    send(start.command());
                }
            }
            (control::Action::Coupler(_), Subject::Coupler(index)) => {
                let Control::Organ(console) = control else {
                    return;
                };
                let engaged = console.coupler_engaged(index);
                let (stopped, starts) = console.set_coupler(index, !engaged);
                for handle in stopped {
                    send(Command::StopVoice { handle });
                }
                for start in starts {
                    send(start.command());
                }
            }
            (control::Action::Tremulant(_), Subject::Tremulant(index)) => {
                let engaged = self.trems.get(index).is_some_and(|t| t.engaged);
                self.set_tremulant_at(index, !engaged);
            }
            (control::Action::Tremulant(None), _) => {
                let engaged = !self.trems.iter().any(|t| t.engaged);
                self.set_tremulant(engaged);
            }
            (control::Action::General(slot), _) => {
                let slot = *slot;
                self.general(slot);
            }
            (control::Action::Divisional(_, slot), Subject::Manual(manual)) => {
                let (manual, slot) = (manual, *slot);
                self.divisional(manual, slot);
            }
            (control::Action::StepperNext, _) => self.stepper_next(),
            (control::Action::StepperPrev, _) => self.stepper_prev(),
            (control::Action::StepperGoto(frame), _) => {
                let frame = *frame;
                self.stepper_goto(frame);
            }
            (control::Action::StepperStore, _) => self.stepper_store(),
            (control::Action::StepperInsert, _) => self.stepper_insert(),
            (control::Action::Crescendo, _) => {
                // A shoe's travel read as stages: heel (0) adds
                // nothing, toe stands on the last stage, and the
                // rounding puts every stage on an equal slice of the
                // pedal rather than hiding the ends.
                let stages = crate::state::CRESCENDO_STAGES as f32;
                let stage = (value.min(127) as f32 / 127.0 * stages).round() as u8;
                self.set_crescendo(stage);
            }
            (control::Action::CrescendoStage(stage), _) => {
                let stage = *stage;
                if self.take_setter() {
                    self.store_crescendo(stage);
                } else {
                    self.set_crescendo(stage);
                }
            }
            (control::Action::Set, _) => {
                self.setter_armed = !self.setter_armed;
                tracing::info!(
                    "setter {}",
                    if self.setter_armed {
                        "armed — the next general, divisional or crescendo stage stores"
                    } else {
                        "disarmed"
                    }
                );
            }
            (control::Action::Cancel, _) => {
                let Control::Organ(console) = control else {
                    return;
                };
                for handle in console.cancel() {
                    send(Command::StopVoice { handle });
                }
            }
            (control::Action::Panic, _) => {
                if let Control::Organ(console) = control {
                    console.all_off();
                }
                send(Command::AllNotesOff);
            }
            (control::Action::Enclosure(_), Subject::Enclosure(index)) => {
                let Control::Organ(console) = control else {
                    return;
                };
                let position = value.min(127) as f32 / 127.0;
                if let Some((enclosure, position)) = console.set_enclosure(index, position) {
                    send(Command::SetEnclosurePosition {
                        enclosure,
                        position,
                    });
                }
            }
            _ => {}
        }
    }

    /// A general piston: recall the stored registration — or, with the
    /// setter armed, store the current one (and disarm, as a console's
    /// Set does).
    pub fn general(&mut self, slot: u8) {
        if self.take_setter() {
            self.store_general(slot);
        } else {
            self.recall_general(slot);
        }
    }

    /// Consume the armed setter. Every combination piston asks this
    /// exactly once before deciding what it means, so Set says the same
    /// thing wherever the next thumb lands — and disarms afterwards,
    /// as one press of Set on a console arms exactly one store.
    fn take_setter(&mut self) -> bool {
        std::mem::take(&mut self.setter_armed)
    }

    /// Capture the console as it stands into a general, by name — the
    /// text vocabulary bindings use, so the file stays honest across
    /// renames — and persist it with the organ's other per-organ state.
    pub fn store_general(&mut self, slot: u8) {
        let Some(registration) = self.capture(Scope::Whole) else {
            return;
        };
        tracing::info!(
            "general {slot} stored: {} stop(s), {} coupler(s), {} tremulant(s)",
            registration.stops.len(),
            registration.couplers.len(),
            registration.tremulants.len()
        );
        let organ = self.organ_key.clone();
        self.midi_config
            .organs
            .entry(organ)
            .or_default()
            .generals
            .insert(slot, registration);
        self.persist();
    }

    /// Bring a stored general back: every stop, coupler and tremulant
    /// diffs to the stored state — landing on held keys immediately,
    /// as pistons on an electric action do. Stored names the loaded
    /// organ hasn't got are reported and skipped, never fatal.
    pub fn recall_general(&mut self, slot: u8) {
        let Some(general) = self
            .midi_config
            .organs
            .get(&self.organ_key)
            .and_then(|organ| organ.generals.get(&slot))
            .cloned()
        else {
            tracing::info!("general {slot}: nothing stored");
            return;
        };
        let missing = self.recall(&general, Scope::Whole);
        report_missing(&format!("general {slot}"), &missing);
    }

    // ---- divisionals ---------------------------------------------------

    /// A divisional piston: the same gesture as a general, over one
    /// division's own controls. `manual` is an index into the console's
    /// manuals; bindings resolve the name they carry to one of these
    /// before the piston is ever pressed.
    pub fn divisional(&mut self, manual: usize, slot: u8) {
        if self.take_setter() {
            self.store_divisional(manual, slot);
        } else {
            self.recall_divisional(manual, slot);
        }
    }

    pub fn store_divisional(&mut self, manual: usize, slot: u8) {
        let Some(name) = self.manual_names().get(manual).cloned() else {
            return;
        };
        let Some(registration) = self.capture(Scope::Division(manual)) else {
            return;
        };
        tracing::info!(
            "{name} divisional {slot} stored: {} stop(s), {} coupler(s), {} tremulant(s)",
            registration.stops.len(),
            registration.couplers.len(),
            registration.tremulants.len()
        );
        let organ = self.organ_key.clone();
        self.midi_config
            .organs
            .entry(organ)
            .or_default()
            .divisionals
            .entry(name)
            .or_default()
            .insert(slot, registration);
        self.persist();
    }

    pub fn recall_divisional(&mut self, manual: usize, slot: u8) {
        let Some(name) = self.manual_names().get(manual).cloned() else {
            return;
        };
        let Some(divisional) = self
            .midi_config
            .organs
            .get(&self.organ_key)
            .and_then(|organ| organ.divisionals.get(&name))
            .and_then(|slots| slots.get(&slot))
            .cloned()
        else {
            tracing::info!("{name} divisional {slot}: nothing stored");
            return;
        };
        let missing = self.recall(&divisional, Scope::Division(manual));
        report_missing(&format!("{name} divisional {slot}"), &missing);
    }

    // ---- the stepper ---------------------------------------------------

    /// How many frames the sequence holds.
    pub fn stepper_frames(&self) -> usize {
        self.midi_config
            .organs
            .get(&self.organ_key)
            .map(|organ| organ.frames.len())
            .unwrap_or(0)
    }

    /// One thumb, one registration per press: forward through the
    /// sequence. The ends are walls, not wraps — a sequencer that
    /// looped round to the first frame in the middle of a piece would
    /// be a trap, and every console that has one stops at the end.
    pub fn stepper_next(&mut self) {
        let frames = self.stepper_frames();
        if frames == 0 {
            tracing::info!("stepper: no frames yet");
            return;
        }
        self.stepper_to((self.stepper_frame + 1).min(frames - 1));
    }

    pub fn stepper_prev(&mut self) {
        self.stepper_to(self.stepper_frame.saturating_sub(1));
    }

    /// Jump to a frame by its 1-based number, as the file numbers them.
    /// Past the end it clamps: the sequence grows by `stepper_insert`
    /// (or the console's + button), never by walking off the edge.
    pub fn stepper_goto(&mut self, frame: u16) {
        let frames = self.stepper_frames();
        if frames == 0 || frame == 0 {
            return;
        }
        self.stepper_to((frame as usize - 1).min(frames - 1));
    }

    fn stepper_to(&mut self, index: usize) {
        self.stepper_frame = index;
        let Some(frame) = self
            .midi_config
            .organs
            .get(&self.organ_key)
            .and_then(|organ| organ.frames.get(index))
            .cloned()
        else {
            return;
        };
        let missing = self.recall(&frame, Scope::Whole);
        report_missing(&format!("stepper frame {}", index + 1), &missing);
    }

    /// Write the console into the frame the stepper stands on — an
    /// empty sequence gains its first frame here.
    ///
    /// This is its own action rather than "Set + the stepper's own
    /// piston" because the stepper's piston is pressed constantly
    /// while playing: giving it a second, destructive meaning under an
    /// armed setter would overwrite a frame every time a player armed
    /// Set and then reached for the next registration.
    pub fn stepper_store(&mut self) {
        let Some(registration) = self.capture(Scope::Whole) else {
            return;
        };
        let organ = self.organ_key.clone();
        let frames = &mut self
            .midi_config
            .organs
            .entry(organ)
            .or_default()
            .frames;
        if frames.is_empty() {
            frames.push(registration);
            self.stepper_frame = 0;
        } else {
            let index = self.stepper_frame.min(frames.len() - 1);
            frames[index] = registration;
            self.stepper_frame = index;
        }
        tracing::info!("stepper frame {} stored", self.stepper_frame + 1);
        self.persist();
    }

    /// Insert a fresh frame after the current one, store the console
    /// into it and land there — how a sequence is built forwards, one
    /// registration at a time, without renumbering anything by hand.
    pub fn stepper_insert(&mut self) {
        let Some(registration) = self.capture(Scope::Whole) else {
            return;
        };
        let organ = self.organ_key.clone();
        let frames = &mut self
            .midi_config
            .organs
            .entry(organ)
            .or_default()
            .frames;
        let at = if frames.is_empty() {
            0
        } else {
            (self.stepper_frame + 1).min(frames.len())
        };
        frames.insert(at, registration);
        self.stepper_frame = at;
        tracing::info!("stepper frame {} inserted", at + 1);
        self.persist();
    }

    /// Drop the frame the stepper stands on and land on the one that
    /// takes its place (the last frame, if it was the end).
    pub fn stepper_delete(&mut self) {
        let organ = self.organ_key.clone();
        let frames = &mut self
            .midi_config
            .organs
            .entry(organ)
            .or_default()
            .frames;
        if frames.is_empty() || self.stepper_frame >= frames.len() {
            return;
        }
        frames.remove(self.stepper_frame);
        let last = frames.len().saturating_sub(1);
        self.stepper_frame = self.stepper_frame.min(last);
        self.persist();
    }

    // ---- the crescendo -------------------------------------------------

    /// Where the pedal stands and what it is holding, for the console:
    /// `(stage, stops it adds)`.
    pub fn crescendo_overlay(&self) -> Vec<String> {
        if self.crescendo_stage == 0 {
            return Vec::new(); // the heel: the pedal adds nothing
        }
        let Some(organ) = self.midi_config.organs.get(&self.organ_key) else {
            return Vec::new();
        };
        let mut names: Vec<String> = Vec::new();
        for (_, stops) in organ.crescendo.range(1..=self.crescendo_stage) {
            for name in stops {
                if !names.contains(name) {
                    names.push(name.clone());
                }
            }
        }
        names
    }

    /// Move the crescendo pedal to `stage` (0 = heel, nothing added).
    ///
    /// The crescendo is an *overlay*, not a registration: the hand
    /// keeps whatever the drawknobs say, and what sounds is hand ∪
    /// crescendo. Rolling the pedal back therefore takes away only
    /// what the pedal itself put there — a stop the hand also drew
    /// stays. (GrandOrgue reaches the same result by giving every
    /// drawstop an OR over named internal states, the crescendo owning
    /// one of them: `GODrawstop::SetInternalState`.)
    pub fn set_crescendo(&mut self, stage: u8) {
        let stage = stage.min(crate::state::CRESCENDO_STAGES);
        if stage == self.crescendo_stage {
            return;
        }
        self.crescendo_stage = stage;
        let wanted = self.crescendo_overlay();
        let State {
            engine, control, ..
        } = &mut *self;
        let Control::Organ(console) = control else {
            return;
        };
        let by_name: Vec<(StopId, String)> = console
            .stop_states()
            .iter()
            .map(|(id, name, _, _, _)| (*id, name.to_string()))
            .collect();
        let mut missing = Vec::new();
        let mut ids = Vec::new();
        for name in &wanted {
            match by_name.iter().find(|(_, known)| known == name) {
                Some((id, _)) => ids.push(*id),
                None => missing.push(format!("stop {name:?}")),
            }
        }
        let (stopped, starts) = console.set_crescendo(ids);
        for handle in stopped {
            engine.send(Command::StopVoice { handle });
        }
        for start in starts {
            engine.send(start.command());
        }
        report_missing(&format!("crescendo stage {stage}"), &missing);
    }

    /// Store the *hand* registration into one crescendo stage.
    ///
    /// The hand, not what is sounding: storing the sounding set while
    /// the pedal stands on that very stage would fold the overlay into
    /// itself, and every store would ratchet the stage upwards. What
    /// the player means by "this stage adds these stops" is exactly
    /// what their drawknobs say.
    ///
    /// The gesture is Set + `crescendo:<stage>` — the same
    /// arm-then-press idiom as a general, so there is one thing to
    /// learn — and deliberately *not* the pedal itself: a foot sweeping
    /// through the stages must never write anything.
    pub fn store_crescendo(&mut self, stage: u8) {
        if stage == 0 || stage > crate::state::CRESCENDO_STAGES {
            tracing::warn!("crescendo: stage {stage} is not one of 1..{}", crate::state::CRESCENDO_STAGES);
            return;
        }
        let Control::Organ(console) = &self.control else {
            return;
        };
        let stops: Vec<String> = console
            .stop_states()
            .iter()
            .filter(|(id, _, _, _, _)| console.is_hand_drawn(*id))
            .map(|(_, name, _, _, _)| name.to_string())
            .collect();
        tracing::info!("crescendo stage {stage} stored: {} stop(s)", stops.len());
        let organ = self.organ_key.clone();
        self.midi_config
            .organs
            .entry(organ)
            .or_default()
            .crescendo
            .insert(stage, stops);
        self.persist();
    }

    // ---- what a combination reaches ------------------------------------

    /// The controls one combination covers, each with its live state:
    /// the scope question answered in exactly one place, so a store and
    /// the recall that follows it can never disagree about what was in
    /// and what was out.
    fn members(&self, scope: Scope) -> Option<Members> {
        let console = self.control.organ()?;
        let manual = match scope {
            Scope::Whole => None,
            Scope::Division(index) => Some(index),
        };
        let stops = console
            .stop_states()
            .iter()
            .filter(|(_, _, _, midx, _)| manual.is_none_or(|want| *midx == want))
            .map(|(id, name, _, _, drawn)| (*id, name.to_string(), *drawn))
            .collect();

        let flags = console.combination_scope();
        let states: Vec<(usize, String, bool, bool)> = console
            .coupler_states()
            .iter()
            .map(|(index, name, engaged, available)| {
                (*index, name.to_string(), *engaged, *available)
            })
            .collect();
        let couplers: Vec<(usize, String, bool)> = match manual {
            None => states
                .iter()
                .filter(|(_, _, _, available)| *available)
                .map(|(index, name, engaged, _)| (*index, name.clone(), *engaged))
                .collect(),
            Some(index) => {
                let mine = console.manual_couplers(index);
                states
                    .iter()
                    .filter(|(coupler, _, _, available)| {
                        *available
                            && mine.iter().any(|(index, intermanual)| {
                                index == coupler
                                    && if *intermanual {
                                        flags.divisional_intermanual_couplers
                                    } else {
                                        flags.divisional_intramanual_couplers
                                    }
                            })
                    })
                    .map(|(index, name, engaged, _)| (*index, name.clone(), *engaged))
                    .collect()
            }
        };

        let trems: Vec<(usize, String, bool)> = match manual {
            None => self
                .trems
                .iter()
                .enumerate()
                .map(|(index, trem)| (index, trem.name.clone(), trem.engaged))
                .collect(),
            Some(index) if flags.divisional_tremulants => {
                // A tremulant is this division's when it blows on wind
                // the division's own pipes stand on — the only thing
                // that makes a tremulant "the Récit's" rather than the
                // console's.
                let groups = console.manual_wind_groups(index);
                self.trems
                    .iter()
                    .enumerate()
                    .filter(|(_, trem)| trem.groups.iter().any(|group| groups.contains(group)))
                    .map(|(index, trem)| (index, trem.name.clone(), trem.engaged))
                    .collect()
            }
            Some(_) => Vec::new(),
        };
        Some(Members {
            stops,
            couplers,
            trems,
        })
    }

    /// The console as it stands, within `scope`, as names.
    ///
    /// What is *sounding* is what gets stored, crescendo included —
    /// GrandOrgue captures a drawstop's engaged state the same way
    /// (`GOCombination::FillWithCurrent` reads `GetCombinationState()`,
    /// which is `IsEngaged()`), and it is what the player hears when
    /// they press Set: this, please, again.
    fn capture(&self, scope: Scope) -> Option<config::Registration> {
        let members = self.members(scope)?;
        Some(config::Registration {
            stops: engaged_names(&members.stops),
            couplers: engaged_names(&members.couplers),
            tremulants: engaged_names(&members.trems),
        })
    }

    /// Bring `registration` back over `scope`: everything in scope
    /// diffs to the stored state — landing on held keys immediately,
    /// as pistons on an electric action do. Returns the stored names
    /// this console hasn't got, for reporting; they are never dropped
    /// from the file, because the stop they name may well come back.
    fn recall(&mut self, registration: &config::Registration, scope: Scope) -> Vec<String> {
        let Some(members) = self.members(scope) else {
            return Vec::new();
        };
        let mut missing = absent_names("stop", &registration.stops, &members.stops);
        missing.extend(absent_names(
            "coupler",
            &registration.couplers,
            &members.couplers,
        ));
        missing.extend(absent_names(
            "tremulant",
            &registration.tremulants,
            &members.trems,
        ));
        // Tremulant targets first: toggling one needs `&mut self` after
        // the console borrow below has ended.
        let trem_targets: Vec<(usize, bool)> = members
            .trems
            .iter()
            .map(|(index, name, _)| (*index, registration.tremulants.contains(name)))
            .collect();
        {
            let State {
                engine, control, ..
            } = &mut *self;
            let Control::Organ(console) = control else {
                return missing;
            };
            for (id, name, drawn) in &members.stops {
                let wanted = registration.stops.contains(name);
                if wanted != *drawn {
                    let (stopped, starts) = console.set_drawn(*id, wanted);
                    for handle in stopped {
                        engine.send(Command::StopVoice { handle });
                    }
                    for start in starts {
                        engine.send(start.command());
                    }
                }
            }
            for (index, name, engaged) in &members.couplers {
                let wanted = registration.couplers.contains(name);
                if wanted != *engaged {
                    let (stopped, starts) = console.set_coupler(*index, wanted);
                    for handle in stopped {
                        engine.send(Command::StopVoice { handle });
                    }
                    for start in starts {
                        engine.send(start.command());
                    }
                }
            }
        }
        for (index, wanted) in trem_targets {
            self.set_tremulant_at(index, wanted);
        }
        missing
    }

    /// Engage or release every tremulant at once — what the plain
    /// `tremulant` binding and the console's single knob mean.
    pub fn set_tremulant(&mut self, on: bool) {
        for index in 0..self.trems.len() {
            self.set_tremulant_at(index, on);
        }
    }

    /// Reshape one synth tremulant, live — the tuning contract: the
    /// engine gets the new valve immediately (engaged or not), and the
    /// change lands in the organ file's `[tremulant]` section when the
    /// organ has a file, in the file's own vocabulary (rate in Hz,
    /// depth in pitch cents). A wave tremulant's undulation is
    /// recorded in its samples — nothing to shape.
    pub fn set_tremulant_shape(
        &mut self,
        index: usize,
        params: aristide_engine::wind::TremulantParams,
    ) -> Result<(), String> {
        let Some(trem) = self.trems.get_mut(index) else {
            return Err("no such tremulant".into());
        };
        if trem.wave {
            return Err(
                "this tremulant is recorded in the samples (wave) — nothing to shape".into(),
            );
        }
        trem.params = params;
        for &group in &trem.groups.clone() {
            self.engine.send(Command::SetTremulantParams { group, params });
        }
        if let Some(path) = self.composite_path.clone() {
            let kp = aristide_engine::wind::WindParams::default().pitch_exponent as f64;
            let depth_cents = kp * ratio_to_cents(1.0 + params.depth as f64);
            match config::write_composite_tremulant(
                &path,
                params.rate_hz as f64,
                depth_cents,
                params.ramp_seconds as f64,
                params.wobble as f64 * 100.0,
            ) {
                Ok(()) => {}
                Err(err) => tracing::warn!("tremulant shape not saved: {err}"),
            }
        }
        Ok(())
    }

    /// Engage or release one tremulant, with the switch noise.
    pub fn set_tremulant_at(&mut self, index: usize, on: bool) {
        let Some(trem) = self.trems.get_mut(index) else {
            return;
        };
        let changed = trem.engaged != on;
        trem.engaged = on;
        let wave = trem.wave;
        let groups = trem.groups.clone();
        if wave {
            // Sample-switching tremulant: new notes on these chests
            // pick their `wave_tremulant` attack variants, and the
            // engine selects matching releases at note-off.
            if let Control::Organ(console) = &mut self.control {
                for &group in &groups {
                    console.set_wave_tremulant(group, on);
                }
            }
            for group in groups {
                self.engine.send(Command::SetWaveTremulant { group, engaged: on });
            }
        } else {
            for group in groups {
                self.engine.send(Command::SetTremulant { group, engaged: on });
            }
        }
        if !changed {
            return;
        }
        let State {
            engine, control, ..
        } = &mut *self;
        if let Control::Organ(console) = control {
            let (start, stop) = console.tremulant_toggle_noise(on);
            if let Some(start) = start {
                engine.send(start.command());
            }
            if let Some(handle) = stop {
                engine.send(Command::StopVoice { handle });
            }
        }
    }

    /// Shift the keyboards a pitch action applies to: one manual's, or
    /// every one on the device the trigger came from — which is what a
    /// transposer built into a console means by "up".
    fn transpose_inputs(&mut self, device: &str, subject: Subject, to: impl Fn(i8) -> i8) {
        let organ = self.organ_key.clone();
        let names = self.manual_names();
        let mut changed = false;
        for (index, name) in names.iter().enumerate() {
            for input in self.midi_config.inputs_mut(&organ, name) {
                let mine = match subject {
                    Subject::Manual(manual) => manual == index,
                    _ => input.device == device,
                };
                if !mine {
                    continue;
                }
                let shifted = to(input.transpose).clamp(-36, 36);
                if shifted != input.transpose {
                    input.transpose = shifted;
                    changed = true;
                    tracing::info!("control: {} on {name} now plays {shifted:+} semitones", input.device);
                }
            }
        }
        if changed {
            self.resolve_routes();
            self.persist();
        }
    }

    /// The learn target, or `None` once it has waited too long. Checked
    /// wherever the state is observed, so a forgotten dialog doesn't
    /// keep eating notes.
    pub fn learning(&mut self) -> Option<Learn> {
        if self
            .learn
            .as_ref()
            .is_some_and(|l| l.started.elapsed() > LEARN_TIMEOUT)
        {
            tracing::info!("midi: nothing played — stopped listening");
            self.learn = None;
        }
        self.learn.clone()
    }

    /// The binding row waiting for its control, if the wait is still on.
    pub fn control_learning(&mut self) -> Option<ControlLearn> {
        if self
            .control_learn
            .is_some_and(|l| l.started.elapsed() > LEARN_TIMEOUT)
        {
            tracing::info!("control: nothing pressed — stopped listening");
            self.control_learn = None;
        }
        self.control_learn
    }

    pub fn listen_control(&mut self, slot: usize) {
        self.pending = None;
        self.control_learn = Some(ControlLearn {
            slot,
            started: Instant::now(),
        });
    }

    /// One control pressed while listening: the row it was waiting for
    /// takes this device, channel and trigger, and keeps its action.
    pub(crate) fn learn_control(
        &mut self,
        device: &str,
        channel: Option<u8>,
        trigger: control::Trigger,
    ) {
        let Some(learn) = self.control_learning() else {
            return;
        };
        self.control_learn = None;
        let organ = self.organ_key.clone();
        let saved = self.midi_config.controls(&organ).get(learn.slot).cloned();
        let control = config::Control {
            device: device.to_string(),
            channel,
            trigger: trigger.to_string(),
            action: saved
                .as_ref()
                .map_or_else(|| "octave-up".to_string(), |c| c.action.clone()),
            manual: saved.and_then(|c| c.manual),
        };
        tracing::info!(
            "control: {device} {} → {}",
            control.trigger,
            control.action
        );
        self.propose_control(learn.slot, control);
    }

    /// The bind path every UI edit takes: commit `control`, unless the
    /// same message from the same device is already bound elsewhere —
    /// then park it and ask. A row's identity is its device, channel
    /// and trigger: an edit that keeps them (choosing another action,
    /// naming a manual) never asks, so answering "keep both" once is
    /// answered for good.
    pub fn propose_control(&mut self, slot: usize, control: config::Control) {
        self.pending = None;
        let organ = self.organ_key.clone();
        let saved = self.midi_config.controls(&organ).get(slot);
        let identity_kept = saved.is_some_and(|s| {
            s.device == control.device
                && s.channel == control.channel
                && s.trigger == control.trigger
        });
        if !identity_kept && !control.trigger.trim().is_empty() {
            let existing: Vec<usize> = self
                .midi_config
                .controls(&organ)
                .iter()
                .enumerate()
                .filter(|(other, saved)| {
                    *other != slot
                        && saved.device == control.device
                        && saved.trigger == control.trigger
                        && channels_overlap(saved.channel, control.channel)
                })
                .map(|(other, _)| other)
                .collect();
            if !existing.is_empty() {
                tracing::info!(
                    "control: {} on {} is already bound — asking",
                    control.trigger,
                    control.device
                );
                self.pending = Some(Pending::Control {
                    slot,
                    control,
                    existing,
                });
                return;
            }
        }
        self.set_control(slot, control);
    }

    pub fn set_control(&mut self, slot: usize, control: config::Control) {
        let organ = self.organ_key.clone();
        self.midi_config.set_control(&organ, slot, control);
        self.resolve_routes();
        self.persist();
    }

    pub fn remove_control(&mut self, slot: usize) {
        self.pending = None;
        let organ = self.organ_key.clone();
        self.midi_config.remove_control(&organ, slot);
        self.resolve_routes();
        self.persist();
    }

    pub fn controls(&self) -> Vec<config::Control> {
        self.midi_config.controls(&self.organ_key).to_vec()
    }

    pub fn listen(&mut self, manual: usize, slot: usize) {
        self.pending = None;
        self.learn = Some(Learn {
            manual,
            slot,
            heard: None,
            started: Instant::now(),
        });
    }

    /// One key played while listening. The first names the keyboard and
    /// the bottom of its compass; the second fixes the top and writes
    /// the assignment. Pressing the same key twice is a slip, not a
    /// one-key keyboard, so it keeps waiting.
    pub(crate) fn learn_key(&mut self, device: &str, channel: u8, key: u8) {
        let Some(mut learn) = self.learning() else {
            return;
        };
        match learn.heard.take() {
            None => {
                tracing::info!("midi: heard {device} channel {} key {key}", channel + 1);
                learn.heard = Some(config::Input {
                    device: device.to_string(),
                    channel: Some(channel + 1),
                    low: Some(key),
                    high: None,
                    transpose: 0,
                    bend: None,
                    map: None,
                });
                learn.started = Instant::now();
                self.learn = Some(learn);
            }
            Some(input) if input.low == Some(key) => {
                learn.heard = Some(input);
                self.learn = Some(learn);
            }
            Some(mut input) => {
                input.high = Some(key);
                self.learn = None;
                self.propose_input(learn.manual, learn.slot, input);
            }
        }
    }
}
