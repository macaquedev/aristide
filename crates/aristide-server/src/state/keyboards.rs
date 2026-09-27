//! The player's console, taught once for every organ: its keyboards,
//! how each organ's manuals are played from them, and the one-press
//! detection that finds them. This is the Hauptwerk arrangement —
//! console keyboards on the machine, a keyboard map on the organ — so a
//! newly loaded organ plays at once from the manuals it is given in
//! order, and nothing about the hardware is repeated per organ.

use std::time::Instant;

use super::State;
use crate::bindings::{COMPUTER_KEYBOARD, Learn, LearnTarget, channels_overlap, normalize_input};
use crate::config::{self, ConsoleKeyboard, KeyboardChoice};

/// Which console keyboard plays a manual, and whether that came from
/// the console's order rather than the organ's own map.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct PlayedFrom {
    pub keyboard: Option<usize>,
    pub automatic: bool,
}

/// A console keyboard's fields as an edit names them; `None` keeps
/// what the keyboard has.
#[derive(Default)]
pub struct KeyboardEdit {
    pub name: Option<String>,
    pub pedal: Option<bool>,
    pub device: Option<String>,
    pub channel: Option<Option<u8>>,
    pub compass: Option<Option<(u8, u8)>>,
}

impl State {
    pub fn console_keyboards(&self) -> &[ConsoleKeyboard] {
        &self.midi_config.console
    }

    /// The console keyboard playing `manual` of the loaded organ. An
    /// organ's map wins; a manual with inputs of its own (wired before
    /// there was a console) is left to them; anything else takes the
    /// console in order, hand manuals and pedals counted apart.
    pub fn played_from(&self, manual: usize) -> PlayedFrom {
        let names = self.manual_names();
        let Some(name) = names.get(manual) else {
            return PlayedFrom {
                keyboard: None,
                automatic: false,
            };
        };
        let organ = self.midi_config.organ(&self.organ_key);
        let console = &self.midi_config.console;
        match organ.and_then(|organ| organ.keyboards.get(name)) {
            Some(KeyboardChoice::Nothing) => {
                return PlayedFrom {
                    keyboard: None,
                    automatic: false,
                };
            }
            Some(KeyboardChoice::Console(wanted)) => {
                if let Some(index) = console.iter().position(|k| &k.name == wanted) {
                    return PlayedFrom {
                        keyboard: Some(index),
                        automatic: false,
                    };
                }
            }
            None => {}
        }
        if !self.manual_inputs(manual).is_empty() {
            return PlayedFrom {
                keyboard: None,
                automatic: true,
            };
        }
        let pedal = self.manual_is_pedal(manual);
        let rank = (0..manual)
            .filter(|&m| self.manual_is_pedal(m) == pedal)
            .count();
        PlayedFrom {
            keyboard: console
                .iter()
                .enumerate()
                .filter(|(_, k)| k.pedal == pedal)
                .nth(rank)
                .map(|(index, _)| index),
            automatic: true,
        }
    }

    fn manual_is_pedal(&self, manual: usize) -> bool {
        match &self.control {
            super::Control::Organ(console) => console.manual_pedal(manual),
            super::Control::Tone => false,
        }
    }

    /// The loaded organ's manuals each console keyboard plays.
    pub fn console_inputs(&self) -> Vec<(usize, config::Input)> {
        (0..self.manual_names().len())
            .filter_map(|manual| {
                let keyboard = self
                    .midi_config
                    .console
                    .get(self.played_from(manual).keyboard?)?;
                (!keyboard.input.device.is_empty()).then(|| (manual, keyboard.input.clone()))
            })
            .collect()
    }

    /// A fresh name for a keyboard added to the console.
    fn next_keyboard_name(&self, pedal: bool) -> String {
        let taken = |name: &str| self.midi_config.console.iter().any(|k| k.name == name);
        let stem = if pedal { "Pedalboard" } else { "Manual" };
        let count = self
            .midi_config
            .console
            .iter()
            .filter(|k| k.pedal == pedal)
            .count();
        let mut n = count + 1;
        loop {
            let name = match (pedal, n) {
                (true, 1) => stem.to_string(),
                _ => format!("{stem} {n}"),
            };
            if !taken(&name) {
                return name;
            }
            n += 1;
        }
    }

    /// Add a keyboard with no device yet; returns its index.
    pub fn add_console_keyboard(&mut self, pedal: bool) -> usize {
        let name = self.next_keyboard_name(pedal);
        self.midi_config.console.push(ConsoleKeyboard {
            name,
            pedal,
            input: config::Input {
                device: String::new(),
                channel: None,
                low: None,
                high: None,
                transpose: 0,
                bend: None,
                map: None,
            },
        });
        self.resolve_routes();
        self.persist();
        self.midi_config.console.len() - 1
    }

    /// Change one console keyboard. A device and channel another console
    /// keyboard already uses move here: one physical keyboard is one
    /// console keyboard, so the other is left without a device rather
    /// than both sounding. Errors name what the edit got wrong.
    pub fn edit_console_keyboard(
        &mut self,
        index: usize,
        edit: KeyboardEdit,
    ) -> Result<(), &'static str> {
        let Some(saved) = self.midi_config.console.get(index).cloned() else {
            return Err("no such keyboard");
        };
        let mut keyboard = saved.clone();
        if let Some(name) = edit.name {
            let name = name.trim().to_string();
            if name.is_empty() {
                return Err("a keyboard needs a name");
            }
            if self
                .midi_config
                .console
                .iter()
                .enumerate()
                .any(|(other, k)| other != index && k.name == name)
            {
                return Err("another keyboard has that name");
            }
            keyboard.name = name;
        }
        if let Some(pedal) = edit.pedal {
            keyboard.pedal = pedal;
        }
        if let Some(device) = edit.device {
            if device != keyboard.input.device {
                // A compass learned on other hardware says nothing
                // about this one.
                keyboard.input.low = None;
                keyboard.input.high = None;
            }
            keyboard.input.device = device;
        }
        if let Some(channel) = edit.channel {
            keyboard.input.channel = channel;
        }
        if let Some(compass) = edit.compass {
            keyboard.input.low = compass.map(|c| c.0);
            keyboard.input.high = compass.map(|c| c.1);
        }
        normalize_input(&mut keyboard.input);
        if keyboard.name != saved.name {
            for organ in self.midi_config.organs.values_mut() {
                for choice in organ.keyboards.values_mut() {
                    if *choice == KeyboardChoice::Console(saved.name.clone()) {
                        *choice = KeyboardChoice::Console(keyboard.name.clone());
                    }
                }
            }
        }
        if !keyboard.input.device.is_empty() {
            for (other, k) in self.midi_config.console.iter_mut().enumerate() {
                if other != index
                    && k.input.device == keyboard.input.device
                    && channels_overlap(k.input.channel, keyboard.input.channel)
                {
                    tracing::info!(
                        "console: {} moves from {} to {}",
                        k.input.device,
                        k.name,
                        keyboard.name
                    );
                    k.input.device.clear();
                    k.input.low = None;
                    k.input.high = None;
                }
            }
        }
        self.midi_config.console[index] = keyboard;
        self.resolve_routes();
        self.persist();
        Ok(())
    }

    /// Remove a console keyboard; organs that named it take the console
    /// in order again.
    pub fn remove_console_keyboard(&mut self, index: usize) -> bool {
        if index >= self.midi_config.console.len() {
            return false;
        }
        if self
            .learning()
            .is_some_and(|l| matches!(l.target, LearnTarget::Console { .. }))
        {
            self.learn = None;
        }
        let removed = self.midi_config.console.remove(index);
        for organ in self.midi_config.organs.values_mut() {
            organ
                .keyboards
                .retain(|_, choice| *choice != KeyboardChoice::Console(removed.name.clone()));
        }
        self.resolve_routes();
        self.persist();
        true
    }

    /// Set which console keyboard plays one of the loaded organ's
    /// manuals; `None` returns it to the console's order.
    pub fn map_manual(
        &mut self,
        manual: usize,
        choice: Option<KeyboardChoice>,
    ) -> Result<(), &'static str> {
        let Some(name) = self.manual_names().get(manual).cloned() else {
            return Err("no such manual");
        };
        if let Some(KeyboardChoice::Console(keyboard)) = &choice
            && !self.midi_config.console.iter().any(|k| &k.name == keyboard)
        {
            return Err("no such keyboard");
        }
        let organ = self
            .midi_config
            .organs
            .entry(self.organ_key.clone())
            .or_default();
        match choice {
            Some(choice) => {
                organ.keyboards.insert(name, choice);
            }
            None => {
                organ.keyboards.remove(&name);
            }
        }
        self.resolve_routes();
        self.persist();
        Ok(())
    }

    /// Wait for a key on a console keyboard; `keyboard` past the end
    /// adds one on the first press.
    pub fn listen_console(&mut self, keyboard: usize, pedal: bool, range: bool) {
        self.pending = None;
        self.learn = Some(Learn {
            target: LearnTarget::Console {
                keyboard,
                pedal,
                range,
            },
            heard: None,
            repeat: None,
            started: Instant::now(),
        });
    }

    /// One key heard while a console keyboard listens. A key from a
    /// keyboard the console already has is a slip — pressing the same
    /// manual twice — and the wait goes on with that keyboard named.
    pub(super) fn learn_console_key(
        &mut self,
        mut learn: Learn,
        device: &str,
        channel: Option<u8>,
        key: u8,
    ) {
        let LearnTarget::Console {
            keyboard,
            pedal,
            range,
        } = learn.target
        else {
            return;
        };
        let channel = if device == COMPUTER_KEYBOARD {
            None
        } else {
            channel
        };
        let elsewhere = self
            .midi_config
            .console
            .iter()
            .enumerate()
            .find(|(other, k)| {
                *other != keyboard
                    && k.input.device == device
                    && channels_overlap(k.input.channel, channel)
            })
            .map(|(_, k)| k.name.clone());
        if elsewhere.is_some() && learn.heard.is_none() {
            learn.repeat = elsewhere;
            learn.started = Instant::now();
            self.learn = Some(learn);
            return;
        }
        let compass = match learn.heard.take() {
            Some(first) if range && first.low != Some(key) => Some((first.low.unwrap_or(key), key)),
            Some(first) => {
                learn.heard = Some(first);
                self.learn = Some(learn);
                return;
            }
            None if range && device != COMPUTER_KEYBOARD => {
                learn.heard = Some(config::Input {
                    device: device.to_string(),
                    channel,
                    low: Some(key),
                    high: None,
                    transpose: 0,
                    bend: None,
                    map: None,
                });
                learn.repeat = None;
                learn.started = Instant::now();
                self.learn = Some(learn);
                return;
            }
            None => None,
        };
        self.learn = None;
        self.console_heard += 1;
        let index = if keyboard < self.midi_config.console.len() {
            keyboard
        } else {
            self.add_console_keyboard(pedal)
        };
        tracing::info!(
            "console: {} is {device}",
            self.midi_config.console[index].name
        );
        let _ = self.edit_console_keyboard(
            index,
            KeyboardEdit {
                device: Some(device.to_string()),
                channel: Some(channel),
                compass: range.then_some(compass),
                ..Default::default()
            },
        );
    }
}
