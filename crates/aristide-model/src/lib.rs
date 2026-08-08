//! The organ model: a format-neutral description of an instrument.
//!
//! Loaders in `aristide-formats` populate this; the engine renders it.
//! No 12-EDO assumptions: pitch identity lives in explicit key→pipe
//! mappings and per-pipe pitch metadata, never in note-number math.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ManualId(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct StopId(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RankId(pub u32);

/// A keyboard (or pedalboard). "Manual" is used inclusively, as GO does.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manual {
    pub id: ManualId,
    pub name: String,
    /// MIDI note number of the manual's lowest key (conventional default
    /// mapping; the input-mapping layer may override arbitrarily).
    pub first_midi_note: u8,
    pub key_count: u16,
}

/// A sustain loop within a sample, in frames.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SampleLoop {
    pub start: u64,
    pub end: u64,
}

/// One recorded attack/sustain sample of a pipe.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttackSample {
    pub path: PathBuf,
    pub loops: Vec<SampleLoop>,
    /// Recorded pitch in cents relative to the pipe's nominal pitch
    /// (0 = in tune as recorded).
    pub pitch_offset_cents: f64,
}

/// One recorded release tail, selected by how long the note was held.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReleaseSample {
    pub path: PathBuf,
    /// Only used when the note was held at most this long (ms);
    /// `None` = the default/longest release.
    pub max_key_press_ms: Option<u32>,
}

/// Address of one pipe within an [`Organ`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PipeRef {
    pub rank: RankId,
    /// Index into the rank's `pipes`.
    pub pipe: u16,
}

/// Where a pipe's sound comes from.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PipeSource {
    /// The pipe owns recorded samples.
    Sampled {
        attacks: Vec<AttackSample>,
        releases: Vec<ReleaseSample>,
    },
    /// Unit-organ borrowing: sounding this pipe sounds another pipe.
    /// The target may itself be borrowed; consumers follow the chain
    /// (loaders guarantee it is acyclic).
    Borrowed(PipeRef),
    /// A placeholder occupying a key slot but never sounding.
    Silent,
}

/// A single pipe: the atomic sounding unit. Everything in Aristide is
/// ultimately addressed per-pipe (tuning, voicing, effects, routing).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Pipe {
    /// Nominal pitch this pipe sounds at concert tuning, before
    /// temperament/retuning layers.
    pub nominal_frequency_hz: f64,
    /// Set-author tuning correction in cents (already combined across
    /// the organ→windchest→rank→pipe inheritance chain by the loader).
    pub pitch_tuning_cents: f64,
    /// Total gain in dB (amplitude chain folded in by the loader).
    pub gain_db: f64,
    /// Explicit MIDI key of the recording, overriding the sample file's
    /// own `smpl`-chunk unity note when present.
    pub midi_key_number: Option<u8>,
    pub source: PipeSource,
}

impl Pipe {
    /// The pipe's own samples, if it has any (borrowed/silent pipes don't).
    pub fn samples(&self) -> Option<(&[AttackSample], &[ReleaseSample])> {
        match &self.source {
            PipeSource::Sampled { attacks, releases } => Some((attacks, releases)),
            _ => None,
        }
    }
}

/// A rank: one row of pipes of common construction (e.g. "Principal 8'").
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Rank {
    pub id: RankId,
    pub name: String,
    pub pipes: Vec<Pipe>,
}

/// Maps a contiguous run of a manual's keys onto a rank's pipes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RankRange {
    pub rank: RankId,
    /// Index of the first manual key this range covers.
    pub first_key: u16,
    pub key_count: u16,
    /// Index into the rank's pipes for `first_key`.
    pub first_pipe: u16,
}

/// A stop: a drawable voice on a manual, sounding one or more ranks.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Stop {
    pub id: StopId,
    pub name: String,
    pub manual: ManualId,
    pub ranks: Vec<RankRange>,
}

/// A coupler: plays another manual's engaged stops from this manual,
/// possibly shifted (sub/super octave = ±1200 cents at 12-EDO, but
/// stored as a key delta since shift semantics are keyboard-relative).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Coupler {
    pub name: String,
    pub from_manual: ManualId,
    pub to_manual: ManualId,
    pub key_shift: i16,
}

/// A complete instrument, format-neutral.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Organ {
    pub name: String,
    /// Directory the sample paths are relative to.
    pub base_path: PathBuf,
    pub manuals: Vec<Manual>,
    pub stops: Vec<Stop>,
    pub ranks: Vec<Rank>,
    pub couplers: Vec<Coupler>,
}

impl Organ {
    pub fn rank(&self, id: RankId) -> Option<&Rank> {
        self.ranks.iter().find(|r| r.id == id)
    }

    pub fn pipe(&self, at: PipeRef) -> Option<&Pipe> {
        self.rank(at.rank)?.pipes.get(at.pipe as usize)
    }

    /// Follow a borrow chain to the pipe that actually sounds.
    /// Returns `None` for dangling references (loaders prevent cycles).
    pub fn sounding_pipe(&self, at: PipeRef) -> Option<&Pipe> {
        let mut hops = self.ranks.iter().map(|r| r.pipes.len()).sum::<usize>() + 1;
        let mut pipe = self.pipe(at)?;
        while let PipeSource::Borrowed(target) = &pipe.source {
            pipe = self.pipe(*target)?;
            hops = hops.checked_sub(1)?;
        }
        Some(pipe)
    }
}
