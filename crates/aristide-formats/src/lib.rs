//! Sample-set loaders and sidecar files.
//!
//! Reads the open GrandOrgue `.organ` format and unencrypted Hauptwerk
//! v1/v2-era packages. Encrypted Hauptwerk sets are out of scope,
//! permanently: no decryption, ever.
//!
//! Aristide-specific data (voicing, tuning, routing, effects) lives in
//! TOML sidecar files next to the loaded set; the set itself is never
//! modified.

use std::path::Path;

pub mod grandorgue;
pub mod hauptwerk;
pub mod instrument;
pub mod sidecar;
pub mod wav;
pub mod wavpack;

/// A parsed organ plus non-fatal deviations encountered on the way.
#[derive(Debug)]
pub struct LoadResult {
    pub organ: aristide_model::Organ,
    pub warnings: Vec<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum SetError {
    #[error(transparent)]
    GrandOrgue(#[from] grandorgue::OdfError),
    #[error(transparent)]
    Hauptwerk(#[from] hauptwerk::HwError),
}

/// Load a sample set in whichever format its extension names — the
/// one place that knows there is more than one. Everything downstream
/// sees the same [`aristide_model::Organ`].
pub fn load_set(path: &Path) -> Result<LoadResult, SetError> {
    if hauptwerk::is_definition(path) {
        Ok(hauptwerk::load(path)?)
    } else {
        Ok(grandorgue::load(path)?)
    }
}
