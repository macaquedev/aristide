//! Manual, source, stop and rank tuning edits: the console's Tuning
//! popovers all funnel through here, each landing live on the console
//! immediately and, when the organ has a file, in its `[tuning]`
//! sections too — the tuning contract (see DESIGN.md).

use aristide_model::StopId;

use super::{Control, State};
use crate::{config, tuning};

impl State {
    /// Tune one division apart from the instrument, or with `None`
    /// return it to the shared tuning — live from the next note, and
    /// in the organ's file when it declares the manual.
    /// A live tuning as the organ file spells it.
    pub(super) fn tuning_fields_of(tuning: &tuning::Tuning) -> config::ManualTuningFields {
        config::ManualTuningFields {
            temperament: tuning.temperament.name().to_string(),
            edo: tuning.edo,
            reference: tuning.reference,
            transpose: tuning.transpose,
            scale: tuning.scale.as_ref().map(|scale| scale.scl.clone()),
            keymap: tuning.scale.as_ref().and_then(|scale| scale.kbm.clone()),
            pipes: tuning.pipes,
        }
    }

    /// Persist the instrument-wide tuning — a discrete field commit on
    /// the console, not a slider drag — into the organ's top-level
    /// `[tuning]` table, when it has a file. Unlike a manual's own
    /// tuning there is no "no such manual" to fail on: the table either
    /// exists already or gains one.
    pub fn persist_tuning(&mut self) {
        let Control::Organ(console) = &self.control else {
            return;
        };
        let fields = Self::tuning_fields_of(&console.tuning());
        if let Some(path) = self.composite_path.clone()
            && let Err(err) = config::write_composite_tuning(&path, &fields)
        {
            tracing::warn!("tuning not saved: {err}");
        }
    }

    pub fn tune_manual(&mut self, manual: usize, tuning: Option<tuning::Tuning>) -> bool {
        let names = self.manual_names();
        if manual >= names.len() {
            return false;
        }
        let Control::Organ(console) = &mut self.control else {
            return false;
        };
        let fields = tuning.as_ref().map(Self::tuning_fields_of);
        console.set_manual_tuning(manual, tuning);
        if let Some(path) = self.composite_path.clone() {
            match config::write_composite_manual_tuning(&path, &names[manual], fields) {
                Ok(true) => {}
                Ok(false) => tracing::warn!(
                    "manual tuning not saved: {} has no [[manual]] named {:?} — declare \
                     it to keep this tuning",
                    path.display(),
                    names[manual]
                ),
                Err(err) => tracing::warn!("manual tuning not saved: {err}"),
            }
        }
        true
    }

    /// Tune one sample set apart from the instrument (or with `None`
    /// return it), live and in the file. `alias` is a `[sources]`
    /// alias — the one the set's stops report as their source.
    pub fn tune_source(&mut self, alias: &str, tuning: Option<tuning::Tuning>) -> Result<(), String> {
        if !self.provenance.values().any(|prov| prov.source == alias) {
            return Err(format!("{alias:?} is not a source of this organ"));
        }
        let Control::Organ(console) = &mut self.control else {
            return Err("no organ is loaded".into());
        };
        let fields = tuning.as_ref().map(Self::tuning_fields_of);
        console.set_source_tuning(alias, tuning);
        if let Some(path) = self.composite_path.clone() {
            match config::write_composite_source_tuning(&path, alias, fields.as_ref()) {
                Ok(true) => {}
                Ok(false) => tracing::warn!(
                    "set tuning not saved: {} has no [sources] entry {alias:?}",
                    path.display()
                ),
                Err(err) => tracing::warn!("set tuning not saved: {err}"),
            }
        } else {
            tracing::warn!("tuning for set {alias:?} not saved: this organ has no file yet");
        }
        Ok(())
    }

    /// Pin what a stop follows, or give it a tuning of its own — live
    /// and in the file. `Follow::Auto` with no tuning is the default
    /// and removes the stop's row.
    pub fn tune_stop(
        &mut self,
        stop: StopId,
        change: Result<tuning::Follow, tuning::Tuning>,
    ) -> Result<(), String> {
        let (name, manual, _) = self.stop_coordinates(stop)?;
        let Control::Organ(console) = &mut self.control else {
            return Err("no organ is loaded".into());
        };
        let entry = match change {
            Ok(follow) => {
                console.set_stop_follow(stop, follow);
                (follow != tuning::Follow::Auto)
                    .then(|| config::StopTuningEntry::Follow(follow.name().to_string()))
            }
            Err(tuning) => {
                let fields = Self::tuning_fields_of(&tuning);
                console.set_stop_tuning(stop, Some(tuning));
                Some(config::StopTuningEntry::Own(fields))
            }
        };
        if let Some(path) = self.composite_path.clone() {
            config::write_composite_stop_tuning(&path, &name, &manual, None, entry)?;
        } else {
            tracing::warn!("tuning for {name:?} not saved: this organ has no file yet");
        }
        Ok(())
    }

    /// Tune one rank apart within a stop (or with `None` return it to
    /// the stop's tuning), live and in the file.
    pub fn tune_rank(
        &mut self,
        stop: StopId,
        rank: aristide_model::RankId,
        tuning: Option<tuning::Tuning>,
    ) -> Result<(), String> {
        let (name, manual, _) = self.stop_coordinates(stop)?;
        let Control::Organ(console) = &mut self.control else {
            return Err("no organ is loaded".into());
        };
        let Some(rank_name) = console
            .stop_ranks(stop)
            .into_iter()
            .find(|(id, _)| *id == rank)
            .map(|(_, name)| name.to_string())
        else {
            return Err(format!("{name:?} sounds no rank {}", rank.0));
        };
        let entry = tuning.as_ref().map(|tuning| config::StopTuningEntry::Own(Self::tuning_fields_of(tuning)));
        console.set_rank_tuning(stop, rank, tuning);
        if let Some(path) = self.composite_path.clone() {
            config::write_composite_stop_tuning(&path, &name, &manual, Some(&rank_name), entry)?;
        } else {
            tracing::warn!("tuning for {name:?} / {rank_name:?} not saved: this organ has no file yet");
        }
        Ok(())
    }
}
