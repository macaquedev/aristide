//! The organ model: a format-neutral description of an instrument.
//!
//! Loaders in `aristide-formats` populate this; the engine renders it.
//! No 12-EDO assumptions: every pipe has an absolute frequency, every
//! key mapping is explicit.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PipeId(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct StopId(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct DivisionId(pub u32);

/// A single pipe: the atomic sounding unit. Everything in Aristide is
/// ultimately addressed per-pipe (tuning, voicing, effects, routing).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Pipe {
    pub id: PipeId,
    /// Target frequency in Hz after all tuning layers are applied.
    pub frequency_hz: f64,
    /// Cent offset applied on top of the base recording's pitch.
    pub tuning_offset_cents: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Stop {
    pub id: StopId,
    pub name: String,
    pub division: DivisionId,
    pub pipes: Vec<PipeId>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Division {
    pub id: DivisionId,
    pub name: String,
}

/// A complete instrument, format-neutral.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Organ {
    pub name: String,
    pub divisions: Vec<Division>,
    pub stops: Vec<Stop>,
    pub pipes: Vec<Pipe>,
}
