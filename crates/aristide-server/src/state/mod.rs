//! The control-plane state machine: everything MIDI, HTTP, and the
//! load path read and mutate about the running instrument. `State`
//! itself, its small supporting types, and the core lifecycle methods
//! (accessors, organ-file save/rename, persistence) live here; the
//! bulkier method groups are split into sibling modules —
//! [`learn`] for binding resolution and MIDI-learn/performance
//! controls, [`edit`] for organ-structure mutators, and [`tuning`]
//! for manual/rank/source tuning edits.

use std::collections::HashMap;
use std::path::PathBuf;

use aristide_engine::EngineHandle;
use aristide_formats::instrument;
use aristide_model::StopId;

use crate::bindings::{Binding, ControlLearn, Learn, MidiPort};
use crate::console::Console;
use crate::{config, load};

mod edit;
mod learn;
mod tuning;

/// What MIDI input drives: the sampled organ console, or the M1 tone.
pub enum Control {
    Tone,
    Organ(Console),
}

impl Control {
    pub fn organ(&self) -> Option<&Console> {
        match self {
            Control::Organ(console) => Some(console),
            Control::Tone => None,
        }
    }

    pub fn organ_mut(&mut self) -> Option<&mut Console> {
        match self {
            Control::Organ(console) => Some(console),
            Control::Tone => None,
        }
    }
}

/// One engageable tremulant on the loaded organ, as the control plane
/// tracks it: which engine wind groups it drives and whether it is a
/// wave (sample-switching) or synth (pressure-modulating) tremulant.
pub struct TremControl {
    pub name: String,
    pub wave: bool,
    pub groups: Vec<u8>,
    pub engaged: bool,
    /// The synth tremulant's live shape (meaningless for wave trems —
    /// their undulation is recorded in the samples). Kept here so the
    /// console can show and edit it; every change goes straight to the
    /// engine and into the organ file's `[tremulant]` section.
    pub params: aristide_engine::wind::TremulantParams,
}

/// A bind parked mid-air: what it proposes already has a job, and the
/// console is asking whether it now has two. One device driving two
/// manuals, or one message doing two things, are both legitimate wants
/// — but never ones to create silently.
#[derive(Clone)]
pub enum Pending {
    /// `input` wants this manual's slot, but the same device (on an
    /// overlapping channel) already plays the rows in `existing`,
    /// as (manual index, slot) pairs.
    Input {
        manual: usize,
        slot: usize,
        input: config::Input,
        existing: Vec<(usize, usize)>,
    },
    /// `control` wants this slot, but the same message from the same
    /// device is already bound at the slots in `existing`.
    Control {
        slot: usize,
        control: config::Control,
        existing: Vec<usize>,
    },
}

/// The player's answer to a parked bind.
#[derive(Clone, Copy, PartialEq)]
pub enum Resolution {
    /// Both jobs stand: the device drives the old rows and the new one.
    KeepBoth,
    /// The old rows go; the new one takes over what they knew about the
    /// hardware (a learned compass, a shift) unless it states its own.
    Replace,
    Cancel,
}

/// The computer keyboard resolved for play: which manual its rows
/// address and by how much they are shifted.
#[derive(Clone, Copy)]
pub struct KeyboardInput {
    pub manual: usize,
    pub transpose: i8,
    pub compass: (u8, u8),
}

pub struct State {
    pub engine: EngineHandle,
    pub control: Control,
    /// MIDI inputs in connection order; the index is the port id the
    /// HTTP API and the input callbacks both use.
    pub midi_ports: Vec<MidiPort>,
    /// Saved assignments for every organ this machine has played.
    pub midi_config: config::MidiConfig,
    /// Where to write them back. `None` in tests and when the user has
    /// no config directory: assignments then last only for this run.
    pub config_path: Option<PathBuf>,
    /// The loaded organ's name — the key its assignments are stored
    /// under, so one rig can drive many instruments differently.
    pub organ_key: String,
    /// Per manual index: the MIDI channel (1-16) the sample set's
    /// sidecar says that manual conventionally speaks on. Used only to
    /// pre-fill the channel when a device is assigned by hand; playing
    /// a key sets the real one.
    pub suggested_channels: Vec<Option<u8>>,
    /// Set while Preferences waits for a key press to bind a manual.
    pub learn: Option<Learn>,
    /// Set while it waits for the *control* to press: which binding row
    /// the next message that isn't a note belongs to.
    pub control_learn: Option<ControlLearn>,
    /// A bind waiting on the player: it would give a device (or one of
    /// its messages) a second job, and the console is showing the
    /// keep-both / replace / cancel dialog.
    pub pending: Option<Pending>,
    /// Bindings on the computer keyboard, which no operating system
    /// enumerates but which is otherwise an input like the rest.
    pub key_bindings: Vec<Binding>,
    /// Where the computer keyboard's notes go, and how far each
    /// assignment is shifted. Assigned like any other input, in the
    /// config — and like any device it may drive more than one manual,
    /// once the player has confirmed that is what they meant.
    pub keyboard: Vec<KeyboardInput>,
    /// Live notes per (port, channel, incoming note): the (manual, key)
    /// landings each produced, so a later per-channel pitch bend can
    /// find them. Populated only for bend-enabled inputs.
    pub live_notes: HashMap<(usize, u8, u8), Vec<(usize, u16)>>,
    /// The current bend per (port, channel), in cents — an MPE member's
    /// bend routinely arrives before its note-on, and the note must
    /// start already bent.
    pub channel_bend: HashMap<(usize, u8), f64>,
    /// Lumatone maps by the path the config spelled, loaded once;
    /// `None` records a failed load so it isn't retried every rescan.
    pub ltn_cache: HashMap<String, Option<std::sync::Arc<aristide_model::lumatone::LumatoneMap>>>,
    /// The loaded organ's engageable tremulants (empty when no set is
    /// loaded). The plain `tremulant` action/endpoint toggles them all;
    /// named actions (`tremulant:Tremblant`) reach one.
    pub trems: Vec<TremControl>,
    /// The combination setter: while armed, the next general press
    /// stores the current registration instead of recalling.
    pub setter_armed: bool,
    pub master_gain: f32,
    /// Reverb wet level; `None` = no IR loaded.
    pub reverb_wet: Option<f32>,
    /// MIDI controller number driving swell boxes (sidecar `[enclosures] cc`).
    pub expression_cc: u8,
    /// Set when the loaded organ is a composite definition file loaded
    /// on its own: that file owns the rig's MIDI wiring, so every
    /// assignment change is written back into its `[midi]` section.
    pub composite_path: Option<PathBuf>,
    /// How this instrument was put together — what the setup dialog
    /// asks about and what `/api/organ/save` writes to a file.
    pub setup: Setup,
    /// Per stop: where it came from. The coordinates every per-stop
    /// file edit — rename, re-source, delete — addresses its file
    /// lines by.
    pub provenance: std::collections::HashMap<StopId, instrument::StopProvenance>,
    /// Per stop: its own `[[voicing.adjust]]` rule (footage, cents,
    /// gain), mirrored here so the console editor can show and edit
    /// exactly what the file says.
    pub stop_voicing: std::collections::HashMap<StopId, load::StopVoicing>,
    /// Per stop: its declared knob engraving (`""` = engrave nothing);
    /// stops absent here engrave the footage they actually speak at.
    pub stop_labels: std::collections::HashMap<StopId, String>,
    /// Per manual name: its declared drawknob order (console stop
    /// names, top of the jamb first) — the file's `[console.order]`.
    /// Display only; the snapshot deals stops out in this order and
    /// names that no longer resolve simply have no effect.
    pub stop_order: std::collections::BTreeMap<String, Vec<String>>,
    /// Per composite manual: a player-declared compass overriding the
    /// set's own. Asked when sets are combined; editable later in
    /// Preferences; saved into the composite file.
    pub compass_overrides: Vec<Option<(u8, u8)>>,
    /// Sources waiting to be loaded, queued by the picker (or the CLI at
    /// startup). The main thread owns the audio stream, so it is the one
    /// that performs the load and swaps the result in.
    pub pending_load: Option<LoadRequest>,
    /// What the load in progress is doing right now, for a UI watching.
    pub loading: Option<String>,
    /// Why the last load failed, kept until the next one starts.
    pub load_error: Option<String>,
    /// What the last load skipped or papered over (dangling organ-file
    /// references, ignored sidecar lines), kept until the next one
    /// starts — an organ that loads emptier than its file intends must
    /// say so where the player is looking, not only in the log.
    pub load_warnings: Vec<String>,
    /// Where the console's movable panels sit, by panel id
    /// (`"keyboard:<manual>"`, `"jamb:<manual>"`, `"couplers"`,
    /// `"shoes"`) — only the ones a player has explicitly placed.
    /// Loaded from the organ file's `[console.layout]` and kept in
    /// step with it; purely cosmetic, so editing it never rebuilds the
    /// engine.
    pub layout: std::collections::BTreeMap<String, instrument::PanelPos>,
    /// Whether engaged couplers pull the coupled keys down on the
    /// on-screen keyboards — the organ file's `[console] coupled_keys`.
    /// Display only, so editing it never rebuilds; true by default.
    pub coupled_keys: bool,
    /// Per-coupler `"never"` / `"always"` overrides of `coupled_keys`,
    /// by console name — the file's `[console.coupler_keys]`.
    pub coupler_key_modes: std::collections::BTreeMap<String, String>,
}

/// One request to load an instrument, from the picker or the CLI.
pub struct LoadRequest {
    pub paths: Vec<PathBuf>,
    /// CLI `--stops` registration patterns; empty means each source's
    /// sidecar default. The picker never sets these.
    pub stops: Vec<String>,
    /// Queued by the command line: a failure should exit the process,
    /// as a bad CLI path always has, not leave a silent server running.
    pub initial: bool,
}

/// One coupler route as the console editor sends it (the JSON the
/// /api/organ/coupler/routes endpoint carries): manuals as console
/// indexes, defaults by absence — the wire twin of the snapshot's
/// route objects.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CouplerRouteEdit {
    pub from: Option<usize>,
    #[serde(default)]
    pub to: Option<usize>,
    #[serde(default)]
    pub shift: i16,
    #[serde(default)]
    pub low: Option<u8>,
    #[serde(default)]
    pub high: Option<u8>,
    #[serde(default)]
    pub unison_off: bool,
    #[serde(default)]
    pub scope: Option<String>,
    #[serde(default)]
    pub repitch: Option<bool>,
    #[serde(default)]
    pub own_pipes: bool,
}

/// One entry of a division's display rank, as the order endpoint
/// speaks it: a stop by id (`s12`), or a coupler by console index
/// (`c3`) — a coupler listed in a division's rank is seated in that
/// jamb, a drawknob among the stops, instead of on the rail.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RankItem {
    Stop(StopId),
    Coupler(usize),
}

/// The provenance of the loaded instrument.
#[derive(Default)]
pub struct Setup {
    /// Label and path of each source set, in load order.
    pub sources: Vec<(String, PathBuf)>,
    /// Every whole-division pull: (source index, source manual name,
    /// composite manual index). Replaying these rebuilds the organ.
    pub pulls: Vec<(usize, String, usize)>,
    /// Stops moved between manuals this session, as (stop, from, to)
    /// names in order — the form `[[move]]` takes in a saved file.
    pub moves: Vec<(String, String, String)>,
    /// Combined ad hoc on the CLI: nothing on disk holds this
    /// instrument yet, so the console should ask how it goes together
    /// and offer to save it.
    pub implicit: bool,
    /// The sample set's own organ (its file carries `adopted = true`):
    /// kept exactly as the set defines it, so every edit is refused
    /// until the organ is saved under a different name — the copy is
    /// the player's and takes edits.
    pub adopted: bool,
}

impl State {
    /// The loaded organ's console, if an organ (not the tone
    /// generator) is what input drives.
    pub fn console(&self) -> Option<&Console> {
        self.control.organ()
    }

    pub fn console_mut(&mut self) -> Option<&mut Console> {
        self.control.organ_mut()
    }

    /// An organ is loading or queued to load: the file-writing edits
    /// refuse while this holds, since the file is about to be replaced.
    pub fn is_loading(&self) -> bool {
        self.loading.is_some() || self.pending_load.is_some()
    }

    pub fn manual_names(&self) -> Vec<String> {
        match &self.control {
            Control::Organ(console) => console
                .manual_states()
                .iter()
                .map(|(_, name, _, _, _)| name.to_string())
                .collect(),
            Control::Tone => Vec::new(),
        }
    }

    /// The config file's manual names resolved to indices in the loaded
    /// organ. A saved name this set hasn't got is dropped with a warning
    /// rather than guessed at: playing the wrong division is worse than
    /// playing nothing.
    fn saved_assignments(&self) -> Vec<(usize, Vec<config::Input>)> {
        let names = self.manual_names();
        let names: Vec<&str> = names.iter().map(String::as_str).collect();
        self.midi_config
            .assignments(&self.organ_key)
            .filter_map(|(manual, inputs)| {
                match aristide_formats::sidecar::match_names(&names, manual).as_slice() {
                    [index] => Some((*index, inputs.to_vec())),
                    _ => {
                        tracing::warn!(
                            "midi: assignments name a manual {manual:?} this organ \
                             hasn't got — ignoring them"
                        );
                        None
                    }
                }
            })
            .collect()
    }

    /// The compass a manual answers to before any keyboard widens it:
    /// the player's declared override, else the set's own.
    fn compass_override(&self, manual: usize) -> Option<(u8, u8)> {
        self.compass_overrides.get(manual).copied().flatten()
    }

    /// Write this instrument — sources, manuals with their effective
    /// compasses, division pulls, and the current MIDI wiring — as a
    /// composite organ file, which from now on is where it lives.
    pub fn save_composite(&mut self, path: PathBuf) -> Result<(), String> {
        if self.composite_path.is_some() {
            return Err("this organ already lives in a file".into());
        }
        if self.setup.sources.is_empty() {
            return Err("no sample set loaded".into());
        }
        let names = self.manual_names();
        let native = self.native_compass();
        let manuals: Vec<config::SavedManual> = names
            .iter()
            .enumerate()
            .map(|(manual, name)| {
                let (low, high) = self.compass_override(manual).unwrap_or(native[manual]);
                let tuning = match &self.control {
                    Control::Organ(console) => console.manual_tuning(manual),
                    Control::Tone => None,
                };
                let tuning = tuning.as_ref().map(Self::tuning_fields_of);
                config::SavedManual {
                    name: name.clone(),
                    kind: match &self.control {
                        Control::Organ(console) => console.manual_kind(manual),
                        Control::Tone => Default::default(),
                    },
                    low,
                    high,
                    tuning,
                }
            })
            .collect();
        let dropped: Vec<String> = match &self.control {
            Control::Organ(console) => console
                .coupler_states()
                .iter()
                .filter(|(_, _, _, available)| !available)
                .map(|(_, name, _, _)| name.to_string())
                .collect(),
            Control::Tone => Vec::new(),
        };
        config::save_composite(
            &path,
            &self.organ_key,
            &self.setup.sources,
            &manuals,
            &self.setup.pulls,
            &self.setup.moves,
            &dropped,
        )?;
        tracing::info!("organ saved: {}", path.display());
        // The file is now the way to load this organ again.
        let canonical = path.canonicalize().unwrap_or_else(|_| path.clone());
        self.midi_config.remember(&self.organ_key, &canonical);
        self.composite_path = Some(path);
        self.setup.implicit = false;
        // The new file owns the wiring from here on; write it in.
        self.persist();
        Ok(())
    }

    /// Rename the loaded organ, everywhere the name is load-bearing at
    /// once: the file that owns it (a composite's `name`, or a sample
    /// set's sidecar), the assignments in `midi_config` — they are
    /// keyed by name, so they must move or the rename would silently
    /// unwire the organ — the library's display name (its path key is
    /// untouched), and the live console. Nothing is written under the
    /// new name until the file write has succeeded.
    pub fn rename_organ(&mut self, name: &str) -> Result<(), String> {
        let name = name.trim();
        if name.is_empty() {
            return Err("the organ needs a name".into());
        }
        let Control::Organ(console) = &mut self.control else {
            return Err("no organ is loaded".into());
        };
        if console.organ_name() == name {
            return Ok(());
        }
        // Where the name lives on disk. An implicit combination has no
        // file yet, so there is nowhere durable to put its name.
        let file = if let Some(path) = &self.composite_path {
            config::write_composite_name(path, name)?;
            path.canonicalize().unwrap_or_else(|_| path.clone())
        } else if self.setup.implicit {
            return Err(
                "this combination isn't saved as a file yet — save it in \
                 Preferences → Organ first"
                    .into(),
            );
        } else if let Some((_, path)) = self.setup.sources.first() {
            config::write_sidecar_name(path, name)?;
            path.clone()
        } else {
            return Err("no organ is loaded".into());
        };
        let old_key = std::mem::replace(&mut self.organ_key, name.to_string());
        if let Some(wiring) = self.midi_config.organs.remove(&old_key) {
            self.midi_config.organs.insert(name.to_string(), wiring);
        }
        console.set_organ_name(name.to_string());
        for entry in &mut self.midi_config.library {
            if entry.path == file {
                entry.name = name.to_string();
            }
        }
        // The setup pane labels the organ's own file by its name; the
        // sets inside a saved combination keep their labels.
        for (label, path) in &mut self.setup.sources {
            if *path == file {
                *label = name.to_string();
            }
        }
        tracing::info!("organ renamed: {old_key:?} → {name:?}");
        self.persist();
        Ok(())
    }

    /// Save the loaded organ as a copy under `name` and switch to it:
    /// the file is copied line for line beside the original (an
    /// adopted set's organ loses its `adopted` flag on the way), the
    /// wiring is copied under the new name, the library learns the
    /// copy, and the console carries on playing the very same
    /// instrument — nothing needs rebuilding, only the name and the
    /// file behind it change. The original file is left untouched.
    /// This is the way past an adopted organ's refusal to be edited.
    pub fn save_organ_as(&mut self, name: &str) -> Result<(), String> {
        let name = name.trim();
        if name.is_empty() {
            return Err("the organ needs a name".into());
        }
        let Some(current) = self.composite_path.clone() else {
            return Err(
                "this combination isn't saved as a file yet — save it as an organ file first"
                    .into(),
            );
        };
        // The copy sits beside the original — the organs directory
        // for an adopted set, or wherever the player keeps the file.
        let dir = current
            .parent()
            .filter(|dir| !dir.as_os_str().is_empty())
            .map(std::path::Path::to_path_buf)
            .or_else(config::organs_dir)
            .ok_or_else(|| "no directory to keep the copy in".to_string())?;
        let Control::Organ(console) = &mut self.control else {
            return Err("no organ is loaded".into());
        };
        let copy = config::copy_composite_as(&current, &dir, name)?;
        let old_file = current.canonicalize().unwrap_or(current);
        let file = copy.canonicalize().unwrap_or_else(|_| copy.clone());
        let old_key = std::mem::replace(&mut self.organ_key, name.to_string());
        if let Some(wiring) = self.midi_config.organs.get(&old_key).cloned() {
            self.midi_config.organs.insert(name.to_string(), wiring);
        }
        console.set_organ_name(name.to_string());
        self.midi_config.remember(name, &file);
        for (label, path) in &mut self.setup.sources {
            if *path == old_file {
                *label = name.to_string();
                *path = file.clone();
            }
        }
        self.composite_path = Some(copy);
        self.setup.adopted = false;
        self.setup.implicit = false;
        tracing::info!("organ saved as {name:?}: {}", file.display());
        self.persist();
        Ok(())
    }

    // ---- organ-pane editor ---------------------------------------------
    //
    // Structural edits go through the organ's file: write the line,
    // then reload the file so the console plays exactly what it says.
    // The running organ keeps sounding until the rebuilt one swaps in.

    /// The file every organ-pane edit writes to. Blank and adopted
    /// organs always have one; only unsaved CLI combinations don't.
    fn organ_file(&self) -> Result<PathBuf, String> {
        self.composite_path.clone().ok_or_else(|| {
            "this combination isn't saved as a file yet — save it in \
             Preferences → Organ first"
                .to_string()
        })
    }

    /// Queue reloading the organ's own file after a structural edit.
    fn reload_organ_file(&mut self, path: PathBuf) {
        self.loading = Some("rebuilding the organ…".to_string());
        self.load_error = None;
        self.load_warnings.clear();
        self.pending_load = Some(LoadRequest {
            paths: vec![path],
            stops: Vec::new(),
            initial: false,
        });
    }

    /// Drop an organ from the picker's library and save the change.
    pub fn forget_organ(&mut self, path: &std::path::Path) -> bool {
        let removed = self.midi_config.forget(path);
        if removed {
            self.persist();
        }
        removed
    }

    /// Write the assignments back for this organ. Called after every
    /// change, so quitting never loses one. A composite organ's file
    /// owns its wiring, so the change lands there too.
    pub(crate) fn persist(&mut self) {
        if let Some(path) = &self.composite_path {
            let organ = self.midi_config.organ(&self.organ_key);
            if let Err(err) = config::write_composite_midi(path, organ) {
                tracing::warn!("midi wiring not saved to {}: {err}", path.display());
            }
        }
        let Some(path) = self.config_path.clone() else {
            return;
        };
        if let Err(err) = config::save(&path, &self.midi_config) {
            tracing::warn!("midi assignments not saved: {err}");
        }
    }
}
