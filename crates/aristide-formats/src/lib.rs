//! Sample-set loaders and sidecar files.
//!
//! Reads the open GrandOrgue `.organ` format and unencrypted Hauptwerk
//! v1/v2-era packages. Encrypted Hauptwerk sets are out of scope,
//! permanently: no decryption, ever.
//!
//! Aristide-specific data (voicing, tuning, routing, effects) lives in
//! TOML sidecar files next to the loaded set; the set itself is never
//! modified.

pub mod grandorgue;
pub mod sidecar;
pub mod wav;
pub mod wavpack;
