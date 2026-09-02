//! Organ-structure mutators: every edit the console's Organ/Preferences
//! panes make to the loaded instrument — manuals, stops, couplers,
//! enclosures, panel placement and rank order. Structural edits write
//! the organ's file and queue a reload; cosmetic ones (panel placement,
//! rank order, coupler-pick) land live with no rebuild.

use aristide_engine::Command;
use aristide_formats::instrument;
use aristide_model::units::cents_between;
use aristide_model::StopId;

use super::{Control, CouplerRouteEdit, RankItem, State};
use crate::{config, load};

impl State {
    /// Declare (or with `None` retract) a manual's compass, live and —
    /// when the organ lives in a file that declares the manual — in
    /// that file. Returns false for a manual the organ hasn't got.
    pub fn set_compass_override(&mut self, manual: usize, compass: Option<(u8, u8)>) -> bool {
        let names = self.manual_names();
        if manual >= names.len() {
            return false;
        }
        self.compass_overrides.resize(names.len().max(self.compass_overrides.len()), None);
        self.compass_overrides[manual] = compass;
        self.resolve_routes();
        if let Some(path) = self.composite_path.clone() {
            match config::write_composite_compass(&path, &names[manual], compass) {
                Ok(true) => {}
                Ok(false) => tracing::warn!(
                    "compass not saved: {} has no [[manual]] named {:?} — declare it \
                     to keep this compass",
                    path.display(),
                    names[manual]
                ),
                Err(err) => tracing::warn!("compass not saved: {err}"),
            }
        }
        true
    }

    /// Move a stop to another manual — live under held keys, kept for
    /// saving, and appended to the organ's file when it has one.
    pub fn move_stop(&mut self, stop: StopId, manual: usize) -> bool {
        let names = self.manual_names();
        let Some(to_name) = names.get(manual).cloned() else {
            return false;
        };
        let State {
            engine, control, ..
        } = &mut *self;
        let Control::Organ(console) = control else {
            return false;
        };
        let Some((stop_name, from_name)) = console
            .stop_states()
            .iter()
            .find(|(id, ..)| *id == stop)
            .map(|(_, name, from, _, _)| (name.to_string(), from.to_string()))
        else {
            return false;
        };
        if from_name == to_name {
            return true;
        }
        let (stopped, starts) = console.move_stop(stop, manual);
        for handle in stopped {
            engine.send(Command::StopVoice { handle });
        }
        for start in starts {
            engine.send(start.command());
        }
        self.setup
            .moves
            .push((stop_name.clone(), from_name.clone(), to_name.clone()));
        if let Some(path) = &self.composite_path
            && let Err(err) = config::append_composite_move(path, &stop_name, &from_name, &to_name)
        {
            tracing::warn!("move not saved: {err}");
        }
        true
    }

    /// Keep a coupler on the console or take it off — live, and in the
    /// organ's file when it has one. Off is not gone: the routes stay,
    /// so the Organ preferences can put it back.
    pub fn set_coupler_pick(&mut self, index: usize, keep: bool) -> bool {
        let State {
            engine, control, ..
        } = &mut *self;
        let Control::Organ(console) = control else {
            return false;
        };
        if index >= console.coupler_states().len() {
            return false;
        }
        let (stopped, starts) = console.set_coupler_available(index, keep);
        for handle in stopped {
            engine.send(Command::StopVoice { handle });
        }
        for start in starts {
            engine.send(start.command());
        }
        let dropped: Vec<String> = console
            .coupler_states()
            .iter()
            .filter(|(_, _, _, available)| !available)
            .map(|(_, name, _, _)| name.to_string())
            .collect();
        if let Some(path) = &self.composite_path
            && let Err(err) = config::write_composite_drops(path, &dropped)
        {
            tracing::warn!("coupler pick not saved: {err}");
        }
        true
    }

    pub fn add_manual(
        &mut self,
        name: &str,
        low: u8,
        high: u8,
        kind: aristide_model::ManualKind,
    ) -> Result<(), String> {
        if self
            .manual_names()
            .iter()
            .any(|existing| existing.eq_ignore_ascii_case(name.trim()))
        {
            return Err(format!("this organ already has a manual named {:?}", name.trim()));
        }
        let path = self.organ_file()?;
        config::append_composite_manual(&path, name, low, high, kind)?;
        self.reload_organ_file(path);
        Ok(())
    }

    /// Redeclare a manual's kind — pedalboard, hand keyboard or a
    /// generalized (microtonal) key field. Structural: the console
    /// redraws the keyboard, so the file line is followed by a reload.
    pub fn set_manual_kind(
        &mut self,
        manual: usize,
        kind: aristide_model::ManualKind,
    ) -> Result<(), String> {
        let names = self.manual_names();
        let Some(name) = names.get(manual) else {
            return Err("no such manual".into());
        };
        let path = self.organ_file()?;
        if !config::write_composite_manual_kind(&path, name, kind)? {
            return Err(format!(
                "{} has no [[manual]] named {name:?} — it wasn't declared by this file",
                path.display()
            ));
        }
        self.reload_organ_file(path);
        Ok(())
    }

    /// Redeclare a microtonal manual's hex-field layout (`None` resets
    /// to the derived default). Unlike the kind this is NOT structural:
    /// the layout is a console-drawing fact, no rank or engine state
    /// moves, so it applies to the live organ and the next snapshot
    /// redraws — no reload, no sound interruption, and a player
    /// fiddling with the numbers sees every edit land immediately.
    /// The file line is still written so the layout survives the next
    /// load; a manual the file doesn't declare still applies live,
    /// with a warning that it won't stick (the tuning contract).
    pub fn set_manual_hex(
        &mut self,
        manual: usize,
        layout: Option<aristide_model::HexLayout>,
    ) -> Result<(), String> {
        let names = self.manual_names();
        if manual >= names.len() {
            return Err("no such manual".into());
        }
        let Control::Organ(console) = &mut self.control else {
            return Err("no organ".into());
        };
        console.set_manual_hex(manual, layout);
        if let Some(path) = self.composite_path.clone() {
            match config::write_composite_manual_hex(&path, &names[manual], layout) {
                Ok(true) => {}
                Ok(false) => tracing::warn!(
                    "hex layout not saved: {} has no [[manual]] named {:?} — declare it \
                     to keep this layout",
                    path.display(),
                    names[manual]
                ),
                Err(err) => tracing::warn!("hex layout not saved: {err}"),
            }
        }
        Ok(())
    }

    pub fn rename_manual(&mut self, manual: usize, name: &str) -> Result<(), String> {
        let names = self.manual_names();
        let Some(old) = names.get(manual) else {
            return Err("no such manual".into());
        };
        let name = name.trim();
        if name.is_empty() {
            return Err("the manual needs a name".into());
        }
        if old == name {
            return Ok(());
        }
        if names.iter().any(|existing| existing.eq_ignore_ascii_case(name)) {
            return Err(format!("this organ already has a manual named {name:?}"));
        }
        let path = self.organ_file()?;
        if !config::rename_composite_manual(&path, old, name)? {
            return Err(format!(
                "{} has no [[manual]] named {old:?} — it wasn't declared by this file",
                path.display()
            ));
        }
        // Assignments are keyed by manual name; they follow the rename
        // or the manual would silently unwire.
        if let Some(organ) = self.midi_config.organs.get_mut(&self.organ_key) {
            if let Some(inputs) = organ.manuals.remove(old.as_str()) {
                organ.manuals.insert(name.to_string(), inputs);
            }
            for control in &mut organ.controls {
                if control
                    .manual
                    .as_deref()
                    .is_some_and(|m| m.eq_ignore_ascii_case(old))
                {
                    control.manual = Some(name.to_string());
                }
                // A `divisional:<manual>:<n>` names its manual inside
                // the action text rather than in the row's `manual`
                // field, so it is rewritten here rather than above.
                let renamed = control
                    .action
                    .strip_prefix("divisional:")
                    .and_then(|rest| rest.rsplit_once(':'))
                    .filter(|(named, _)| named.eq_ignore_ascii_case(old));
                if let Some((_, slot)) = renamed {
                    control.action = format!("divisional:{name}:{slot}");
                }
            }
            // Divisionals are keyed by manual name for the same reason
            // the inputs are, and follow for the same reason: a rename
            // must not orphan the division's own pistons.
            if let Some(slots) = organ.divisionals.remove(old.as_str()) {
                organ.divisionals.insert(name.to_string(), slots);
            }
        }
        for prefix in ["keyboard", "jamb"] {
            if let Some(pos) = self.layout.remove(&format!("{prefix}:{old}")) {
                self.layout.insert(format!("{prefix}:{name}"), pos);
            }
        }
        self.persist();
        self.reload_organ_file(path);
        Ok(())
    }

    pub fn remove_manual(&mut self, manual: usize) -> Result<(), String> {
        let names = self.manual_names();
        let Some(name) = names.get(manual) else {
            return Err("no such manual".into());
        };
        let path = self.organ_file()?;
        if !config::remove_composite_manual(&path, name)? {
            return Err(format!(
                "{} has no [[manual]] named {name:?} — it wasn't declared by this file",
                path.display()
            ));
        }
        if let Some(organ) = self.midi_config.organs.get_mut(&self.organ_key) {
            organ.manuals.remove(name.as_str());
            organ.controls.retain(|control| {
                !control
                    .manual
                    .as_deref()
                    .is_some_and(|m| m.eq_ignore_ascii_case(name))
            });
        }
        for prefix in ["keyboard", "jamb"] {
            self.layout.remove(&format!("{prefix}:{name}"));
        }
        self.persist();
        self.reload_organ_file(path);
        Ok(())
    }

    pub fn reorder_manual(&mut self, manual: usize, to: usize) -> Result<(), String> {
        let names = self.manual_names();
        let Some(name) = names.get(manual) else {
            return Err("no such manual".into());
        };
        let path = self.organ_file()?;
        if !config::reorder_composite_manual(&path, name, to)? {
            return Err(format!(
                "{} has no [[manual]] named {name:?} — it wasn't declared by this file",
                path.display()
            ));
        }
        self.reload_organ_file(path);
        Ok(())
    }

    /// Add a sample set to the organ's `[sources]`. Nothing is pulled
    /// yet — the pane's source browser offers its material from here.
    pub fn add_organ_source(&mut self, set: &std::path::Path) -> Result<String, String> {
        if !set.is_file() {
            return Err(format!("{}: not a file", set.display()));
        }
        if instrument::is_definition(set) {
            return Err("a source must be a sample set, not another organ file".into());
        }
        let canonical = set.canonicalize().unwrap_or_else(|_| set.to_path_buf());
        let path = self.organ_file()?;
        config::append_composite_source(&path, &canonical)
    }

    /// Pull one stop (or with `stop` absent a whole division) from a
    /// source onto a manual of this organ.
    pub fn pull_from_source(
        &mut self,
        from: &str,
        source_manual: &str,
        stop: Option<&str>,
        on: &str,
    ) -> Result<(), String> {
        if !self
            .manual_names()
            .iter()
            .any(|name| name.eq_ignore_ascii_case(on))
        {
            return Err(format!("this organ has no manual named {on:?}"));
        }
        let path = self.organ_file()?;
        config::append_composite_pull(&path, from, source_manual, stop, on)?;
        self.reload_organ_file(path);
        Ok(())
    }

    /// A stop's console name, current manual name, and provenance —
    /// what every per-stop file edit needs to find its lines.
    pub(super) fn stop_coordinates(
        &self,
        stop: StopId,
    ) -> Result<(String, String, instrument::StopProvenance), String> {
        let Control::Organ(console) = &self.control else {
            return Err("no organ is loaded".into());
        };
        let Some((name, manual_name)) = console
            .stop_states()
            .iter()
            .find(|(id, ..)| *id == stop)
            .map(|(_, name, manual, _, _)| (name.to_string(), manual.to_string()))
        else {
            return Err("no such stop".into());
        };
        let Some(prov) = self.provenance.get(&stop).cloned() else {
            return Err(format!(
                "where {name:?} came from isn't on record — reload the organ"
            ));
        };
        Ok((name, manual_name, prov))
    }

    /// Delete a stop: remove the `[[stop]]` line that pulled it in, or
    /// except it from its `[[division]]` pull. The source still offers
    /// it, so the pane can pull it back.
    pub fn remove_stop(&mut self, stop: StopId) -> Result<(), String> {
        let (name, manual_name, prov) = self.stop_coordinates(stop)?;
        let path = self.organ_file()?;
        if !config::remove_composite_stop(&path, &prov, &name, &manual_name)? {
            return Err(format!(
                "the pull that brought {name:?} in isn't in {} — it was \
                 hand-edited; edit it there",
                path.display()
            ));
        }
        self.reload_organ_file(path);
        Ok(())
    }

    /// Rename a stop — a label, so it lands live (no rebuild): the
    /// console name changes now, the file keeps it, and every exact
    /// file reference to the old name follows.
    pub fn rename_stop(&mut self, stop: StopId, new: &str) -> Result<(), String> {
        let new = new.trim();
        if new.is_empty() {
            return Err("the stop needs a name".into());
        }
        let (old, manual_name, prov) = self.stop_coordinates(stop)?;
        if old == new {
            return Ok(());
        }
        let Control::Organ(console) = &self.control else {
            return Err("no organ is loaded".into());
        };
        // Two stops of one name on one manual would leave the file's
        // name-keyed lines ([[move]], enclosure members) ambiguous.
        if console
            .stop_states()
            .iter()
            .any(|(id, name, manual, _, _)| {
                *id != stop && name.eq_ignore_ascii_case(new) && *manual == manual_name
            })
        {
            return Err(format!("{manual_name} already has a stop named {new:?}"));
        }
        let path = self.organ_file()?;
        if !config::rename_composite_stop(&path, &prov, &manual_name, &old, new)? {
            return Err(format!(
                "the pull that brought {old:?} in isn't in {} — it was \
                 hand-edited; edit it there",
                path.display()
            ));
        }
        let Control::Organ(console) = &mut self.control else {
            return Err("no organ is loaded".into());
        };
        console.rename_stop(stop, new);
        // Session-side name references follow too, or saving an
        // implicit combination later would write stale move names —
        // and the display order is name-keyed, so it follows or the
        // renamed knob would fall to the bottom of its jamb.
        for (moved, ..) in &mut self.setup.moves {
            if moved.eq_ignore_ascii_case(&old) {
                *moved = new.to_string();
            }
        }
        for names in self.stop_order.values_mut() {
            for name in names.iter_mut() {
                if name.eq_ignore_ascii_case(&old) {
                    *name = new.to_string();
                }
            }
        }
        Ok(())
    }

    /// A stop's own voicing — footage, cents, gain — live and in the
    /// file. Live means now: held keys re-speak the stop at its new
    /// pitch; nothing rebuilds.
    pub fn set_stop_voicing(
        &mut self,
        stop: StopId,
        voicing: load::StopVoicing,
    ) -> Result<(), String> {
        let (name, _, _) = self.stop_coordinates(stop)?;
        let Control::Organ(console) = &mut self.control else {
            return Err("no organ is loaded".into());
        };
        let footage_cents = match voicing.feet {
            None => 0.0,
            Some(feet) => {
                if !(feet > 0.0 && feet.is_finite()) {
                    return Err("footage must be a positive number of feet".into());
                }
                let Some(native) = console.stop_native_footage(stop) else {
                    return Err(format!(
                        "{name:?} speaks no single footage (a mixture) — \
                         tune it in cents instead"
                    ));
                };
                cents_between(feet, native)
            }
        };
        let gain = 10f32.powf((voicing.gain_db.clamp(-40.0, 20.0) as f32) / 20.0);
        let cents = voicing.cents.clamp(-2400.0, 2400.0) + footage_cents;
        console.set_stop_adjust_one(stop, gain, cents);
        let (stopped, starts) = console.reprice_stop(stop);
        for handle in stopped {
            self.engine.send(Command::StopVoice { handle });
        }
        for start in starts {
            self.engine.send(start.command());
        }
        if voicing.is_neutral() {
            self.stop_voicing.remove(&stop);
        } else {
            self.stop_voicing.insert(stop, voicing);
        }
        // The file write is best-effort like the tuning contract: an
        // organ without a file still voices live, with a warning that
        // it won't stick.
        if let Some(path) = self.composite_path.clone() {
            config::write_composite_stop_voicing(
                &path,
                &name,
                voicing.feet,
                voicing.cents,
                voicing.gain_db,
            )?;
        } else {
            tracing::warn!(
                "voicing for {name:?} not saved: this organ has no file yet"
            );
        }
        Ok(())
    }

    /// A stop's knob engraving — the footage line on the drawknob
    /// face. `None` goes back to showing the footage the stop actually
    /// speaks at; `Some("")` engraves nothing; anything else is the
    /// text itself. A label, so it lands live: map now, file line now,
    /// no rebuild.
    pub fn set_stop_pitch_label(
        &mut self,
        stop: StopId,
        label: Option<String>,
    ) -> Result<(), String> {
        let (name, manual_name, prov) = self.stop_coordinates(stop)?;
        let path = self.organ_file()?;
        if !config::write_composite_stop_pitch_label(&path, &prov, &manual_name, label.as_deref())?
        {
            return Err(format!(
                "the pull that brought {name:?} in isn't in {} — it was \
                 hand-edited; edit it there",
                path.display()
            ));
        }
        match label {
            Some(label) => {
                self.stop_labels.insert(stop, label);
            }
            None => {
                self.stop_labels.remove(&stop);
            }
        }
        Ok(())
    }

    /// Declare whether a stop speaks pipes of its own (doubling pipes
    /// other stops sound) or shares them — the default, and what a
    /// real unit action does. Lands live (held keys re-derive, no
    /// rebuild) and is kept in the organ file.
    pub fn set_stop_own_pipes(&mut self, stop: StopId, own: bool) -> Result<(), String> {
        let (name, manual_name, prov) = self.stop_coordinates(stop)?;
        let path = self.organ_file()?;
        if !config::write_composite_stop_own_pipes(&path, &prov, &manual_name, own)? {
            return Err(format!(
                "the pull that brought {name:?} in isn't in {} — it was \
                 hand-edited; edit it there",
                path.display()
            ));
        }
        let State {
            engine, control, ..
        } = &mut *self;
        let Control::Organ(console) = control else {
            return Err("no organ is loaded".into());
        };
        let (stopped, starts) = console.set_stop_own_pipes(stop, own);
        for handle in stopped {
            engine.send(Command::StopVoice { handle });
        }
        for start in starts {
            engine.send(start.command());
        }
        Ok(())
    }

    /// Rename a coupler — a rocker's engraving, so it lands live: the
    /// console name changes now, the file keeps it (a define's own
    /// name line, or the [couplers.rename] map for one a source
    /// carries in), and name-keyed references — drop entries, control
    /// bindings — follow.
    pub fn rename_coupler(&mut self, index: usize, new: &str) -> Result<(), String> {
        let new = new.trim();
        if new.is_empty() {
            return Err("the coupler needs a name".into());
        }
        let Control::Organ(console) = &self.control else {
            return Err("no organ is loaded".into());
        };
        let Some(old) = console
            .coupler_states()
            .get(index)
            .map(|(_, name, _, _)| name.to_string())
        else {
            return Err("no such coupler".into());
        };
        if old == new {
            return Ok(());
        }
        // Couplers are addressed by name everywhere the file speaks of
        // them; two rockers with one engraving would be unaddressable.
        if console
            .coupler_states()
            .iter()
            .enumerate()
            .any(|(at, (_, name, _, _))| at != index && name.eq_ignore_ascii_case(new))
        {
            return Err(format!("this organ already has a coupler named {new:?}"));
        }
        let path = self.organ_file()?;
        config::rename_composite_coupler(&path, &old, new)?;
        let Control::Organ(console) = &mut self.control else {
            return Err("no organ is loaded".into());
        };
        console.rename_coupler(index, new);
        // Control bindings speak coupler names ("coupler:II/I"); they
        // follow the rename or the button would silently unwire.
        let action = format!("coupler:{old}");
        if let Some(organ) = self.midi_config.organs.get_mut(&self.organ_key) {
            for control in &mut organ.controls {
                if control.action.eq_ignore_ascii_case(&action) {
                    control.action = format!("coupler:{new}");
                }
            }
        }
        self.persist();
        Ok(())
    }

    /// Replace a coupler's routes — structural, so the file's lines
    /// are rewritten (a source's coupler materializes as a define of
    /// this organ's own) and the organ rebuilds. Routes arrive with
    /// manuals as console indexes; the file speaks names.
    pub fn set_coupler_routes(
        &mut self,
        index: usize,
        routes: &[CouplerRouteEdit],
    ) -> Result<(), String> {
        let Control::Organ(console) = &self.control else {
            return Err("no organ is loaded".into());
        };
        let Some(name) = console
            .coupler_states()
            .get(index)
            .map(|(_, name, _, _)| name.to_string())
        else {
            return Err("no such coupler".into());
        };
        let lines = self.coupler_route_lines(routes)?;
        let path = self.organ_file()?;
        config::write_composite_coupler_routes(&path, &name, &lines)?;
        self.reload_organ_file(path);
        Ok(())
    }

    /// Define a brand-new coupler and rebuild. Same route vocabulary
    /// as `set_coupler_routes`.
    pub fn add_coupler(&mut self, name: &str, routes: &[CouplerRouteEdit]) -> Result<(), String> {
        let name = name.trim();
        let Control::Organ(console) = &self.control else {
            return Err("no organ is loaded".into());
        };
        if console
            .coupler_states()
            .iter()
            .any(|(_, existing, _, _)| existing.eq_ignore_ascii_case(name))
        {
            return Err(format!("this organ already has a coupler named {name:?}"));
        }
        let lines = self.coupler_route_lines(routes)?;
        let path = self.organ_file()?;
        config::append_composite_coupler(&path, name, &lines)?;
        self.reload_organ_file(path);
        Ok(())
    }

    /// Delete a coupler outright. One this organ's file defines is
    /// removed from it (links, overrides and jamb seats go too) and
    /// the organ rebuilds; a source's coupler is taken off the console
    /// instead — as deleted as a set's own coupler can get, and still
    /// restorable from the Organ preferences.
    pub fn remove_coupler(&mut self, index: usize) -> Result<(), String> {
        let Control::Organ(console) = &self.control else {
            return Err("no organ is loaded".into());
        };
        let Some(name) = console
            .coupler_states()
            .get(index)
            .map(|(_, name, _, _)| name.to_string())
        else {
            return Err("no such coupler".into());
        };
        let path = self.organ_file()?;
        if config::remove_composite_coupler(&path, &name)? {
            self.coupler_key_modes.remove(&name);
            for names in self.stop_order.values_mut() {
                names.retain(|n| !n.eq_ignore_ascii_case(&format!("coupler:{name}")));
            }
            self.reload_organ_file(path);
        } else if !self.set_coupler_pick(index, false) {
            return Err("no such coupler".into());
        }
        Ok(())
    }

    /// Link or unlink two couplers so they move together — live (the
    /// console reconciles their engaged states at once) and in the
    /// file's `[couplers] link`. No rebuild: a link changes what a
    /// rocker does, not what the organ is.
    pub fn link_coupler(&mut self, index: usize, with: usize, on: bool) -> Result<(), String> {
        if index == with {
            return Err("a coupler cannot link to itself".into());
        }
        // The file first: an organ that can't keep the link (not saved
        // yet) must refuse before the live state moves.
        let path = self.organ_file()?;
        let State {
            engine, control, ..
        } = &mut *self;
        let Control::Organ(console) = control else {
            return Err("no organ is loaded".into());
        };
        let states = console.coupler_states();
        let (Some((_, a, _, _)), Some((_, b, _, _))) = (states.get(index), states.get(with))
        else {
            return Err("no such coupler".into());
        };
        let (a, b) = (a.to_string(), b.to_string());
        let (stopped, starts) = console.link_couplers(index, with, on);
        for handle in stopped {
            engine.send(Command::StopVoice { handle });
        }
        for start in starts {
            engine.send(start.command());
        }
        config::write_composite_coupler_link(&path, &a, &b, on)
    }

    /// The organ-wide coupled-keys default — display only, live, and
    /// in the file's `[console] coupled_keys`.
    pub fn set_coupled_keys(&mut self, on: bool) -> Result<(), String> {
        let path = self.organ_file()?;
        config::write_composite_coupled_keys(&path, on)?;
        self.coupled_keys = on;
        Ok(())
    }

    /// One coupler's coupled-keys override: `"never"`, `"always"`, or
    /// None for auto (follow the organ default). Display only, live,
    /// and in the file's `[console.coupler_keys]`.
    pub fn set_coupler_key_mode(&mut self, index: usize, mode: Option<&str>) -> Result<(), String> {
        let Control::Organ(console) = &self.control else {
            return Err("no organ is loaded".into());
        };
        let Some(name) = console
            .coupler_states()
            .get(index)
            .map(|(_, name, _, _)| name.to_string())
        else {
            return Err("no such coupler".into());
        };
        let path = self.organ_file()?;
        config::write_composite_coupler_key_mode(&path, &name, mode)?;
        match mode {
            Some(mode) => {
                self.coupler_key_modes.insert(name, mode.to_string());
            }
            None => {
                self.coupler_key_modes.remove(&name);
            }
        }
        Ok(())
    }

    /// Route tuples from the API (from, to, shift, low, high,
    /// unison_off, scope, repitch — manuals as console indexes) into
    /// the file's vocabulary. A route must listen somewhere, and must
    /// either couple somewhere or silence (a route doing neither is a
    /// dead line the editor shouldn't write).
    fn coupler_route_lines(
        &self,
        routes: &[CouplerRouteEdit],
    ) -> Result<Vec<config::CouplerRouteLine>, String> {
        let names = self.manual_names();
        let manual = |index: Option<usize>| -> Result<Option<String>, String> {
            match index {
                None => Ok(None),
                Some(index) => names
                    .get(index)
                    .cloned()
                    .map(Some)
                    .ok_or_else(|| "no such manual".to_string()),
            }
        };
        routes
            .iter()
            .map(|route| {
                let Some(from) = manual(route.from)? else {
                    return Err("a route needs a manual to listen on".into());
                };
                let to = manual(route.to)?;
                if to.is_none() && !route.unison_off {
                    return Err(
                        "a route must couple somewhere or silence (unison off)".into()
                    );
                }
                let scope = match route.scope.as_deref().unwrap_or("all-keys") {
                    "all-keys" => aristide_model::CouplerScope::AllKeys,
                    "bass" => aristide_model::CouplerScope::Bass,
                    "melody" => aristide_model::CouplerScope::Melody,
                    other => return Err(format!("{other:?} is not a coupler scope")),
                };
                Ok(config::CouplerRouteLine {
                    from,
                    to,
                    shift: route.shift,
                    low: route.low,
                    high: route.high,
                    unison_off: route.unison_off,
                    scope,
                    repitch: route.repitch,
                    own_pipes: route.own_pipes,
                })
            })
            .collect()
    }

    /// Point a stop at a different source stop — same drawknob, same
    /// label, different pipes. Structural: the file's pull lines are
    /// rewritten and the organ rebuilds.
    pub fn retarget_stop(
        &mut self,
        stop: StopId,
        from: &str,
        source_manual: &str,
        source_stop: &str,
    ) -> Result<(), String> {
        let (name, manual_name, prov) = self.stop_coordinates(stop)?;
        let path = self.organ_file()?;
        if !config::retarget_composite_stop(
            &path,
            &prov,
            &name,
            &manual_name,
            from,
            source_manual,
            source_stop,
        )? {
            return Err(format!(
                "the pull that brought {name:?} in isn't in {} — it was \
                 hand-edited; edit it there",
                path.display()
            ));
        }
        self.reload_organ_file(path);
        Ok(())
    }

    /// Define a new (empty) swell box; the pane drags stops in after.
    pub fn add_enclosure(&mut self, name: &str) -> Result<(), String> {
        let path = self.organ_file()?;
        config::append_composite_enclosure(&path, name)?;
        self.reload_organ_file(path);
        Ok(())
    }

    /// Remove a file-defined swell box. Boxes a source carries in have
    /// no line here to remove, and the error says so.
    pub fn remove_enclosure(&mut self, name: &str) -> Result<(), String> {
        let path = self.organ_file()?;
        if !config::remove_composite_enclosure(&path, name)? {
            return Err(format!(
                "{name:?} is the sample set's own box, not one this file defines"
            ));
        }
        self.reload_organ_file(path);
        Ok(())
    }

    /// Put a stop into (or take it out of) a file-defined swell box.
    pub fn assign_enclosure(
        &mut self,
        enclosure: &str,
        stop: StopId,
        inside: bool,
    ) -> Result<(), String> {
        let Control::Organ(console) = &self.control else {
            return Err("no organ is loaded".into());
        };
        let Some(name) = console
            .stop_states()
            .iter()
            .find(|(id, ..)| *id == stop)
            .map(|(_, name, ..)| name.to_string())
        else {
            return Err("no such stop".into());
        };
        let path = self.organ_file()?;
        if !config::assign_composite_enclosure_stop(&path, enclosure, &name, inside)? {
            return Err(format!(
                "{enclosure:?} is the sample set's own box — only boxes this \
                 file defines hold stops by name"
            ));
        }
        self.reload_organ_file(path);
        Ok(())
    }

    /// Move (and optionally size) a console panel on the canvas: all
    /// four are normalized fractions, clamped and rounded to four
    /// decimals before they're written. Size given as `None` keeps
    /// whatever size the panel already has on record — a plain move
    /// never un-sizes a resized jamb. Cosmetic geometry only — unlike
    /// every structural edit above, this never queues a rebuild; the
    /// in-memory layout is updated directly instead.
    pub fn place_panel(
        &mut self,
        panel: &str,
        x: f32,
        y: f32,
        size: Option<(f32, f32)>,
    ) -> Result<(), String> {
        if !matches!(self.control, Control::Organ(_)) {
            return Err("no organ is loaded".into());
        }
        let manual_names = self.manual_names();
        let valid = matches!(panel, "couplers" | "shoes")
            || ["keyboard:", "jamb:"].iter().any(|prefix| {
                panel
                    .strip_prefix(prefix)
                    .is_some_and(|name| manual_names.iter().any(|existing| existing == name))
            });
        if !valid {
            return Err(format!("{panel:?} is not a panel of this organ"));
        }
        let path = self.organ_file()?;
        let round4 = |v: f32| (v.clamp(0.0, 1.0) * 10_000.0).round() / 10_000.0;
        let kept = self.layout.get(panel);
        let (w, h) = match size {
            Some((w, h)) => (
                Some(round4(w.max(0.02))),
                Some(round4(h.max(0.02))),
            ),
            None => (kept.and_then(|pos| pos.w), kept.and_then(|pos| pos.h)),
        };
        let pos = instrument::PanelPos {
            x: round4(x),
            y: round4(y),
            w,
            h,
        };
        config::write_composite_panel(&path, panel, pos)?;
        self.layout.insert(panel.to_string(), pos);
        Ok(())
    }

    /// A division's drawknob order — display only: the file keeps the
    /// console names top-first (couplers seated in the jamb as
    /// `coupler:<name>` entries), the snapshot deals the rank out in
    /// that order, and nothing structural moves (ids, voicing,
    /// combinations all stay put). Live, like panel placement. A
    /// coupler has one seat, so listing it here unseats it from every
    /// other division's rank.
    pub fn set_rank_order(&mut self, manual: usize, items: &[RankItem]) -> Result<(), String> {
        let manual_names = self.manual_names();
        let Some(manual_name) = manual_names.get(manual).cloned() else {
            return Err("no such manual".into());
        };
        let (names, seated) = {
            let Control::Organ(console) = &self.control else {
                return Err("no organ is loaded".into());
            };
            let states = console.stop_states();
            let couplers = console.coupler_states();
            let mut names = Vec::with_capacity(items.len());
            let mut seated: Vec<String> = Vec::new();
            for item in items {
                match item {
                    RankItem::Stop(id) => {
                        let Some((_, name, ..)) = states
                            .iter()
                            .find(|(existing, _, _, midx, _)| existing == id && *midx == manual)
                        else {
                            return Err(format!(
                                "the order names a stop that isn't on {manual_name:?} — \
                                 reordering raced an edit; try again"
                            ));
                        };
                        names.push(name.to_string());
                    }
                    RankItem::Coupler(index) => {
                        let Some((_, name, _, _)) = couplers.get(*index) else {
                            return Err(
                                "the order names a coupler that no longer exists — \
                                 reordering raced an edit; try again"
                                    .into(),
                            );
                        };
                        seated.push(format!("coupler:{name}"));
                        names.push(format!("coupler:{name}"));
                    }
                }
            }
            (names, seated)
        };
        let path = self.organ_file()?;
        config::write_composite_stop_order(&path, &manual_name, &names)?;
        if names.is_empty() {
            self.stop_order.remove(&manual_name);
        } else {
            self.stop_order.insert(manual_name.clone(), names);
        }
        // Unseat the couplers just listed from every other division —
        // in memory and in the file, one rewrite per list that changes.
        for other in manual_names {
            if other == manual_name {
                continue;
            }
            let Some(list) = self.stop_order.get(&other) else { continue };
            let kept: Vec<String> = list
                .iter()
                .filter(|name| !seated.iter().any(|token| token.eq_ignore_ascii_case(name)))
                .cloned()
                .collect();
            if kept.len() == list.len() {
                continue;
            }
            config::write_composite_stop_order(&path, &other, &kept)?;
            if kept.is_empty() {
                self.stop_order.remove(&other);
            } else {
                self.stop_order.insert(other, kept);
            }
        }
        Ok(())
    }
}
