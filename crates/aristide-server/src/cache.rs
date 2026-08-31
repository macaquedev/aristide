//! Decoded-sample load cache (GO's `GOCache` trick, gap §3b).
//!
//! Decoding (WavPack!) and per-file analysis — period refinement,
//! phase maps, tail measurement — dominate load time and are pure
//! functions of (file bytes, ODF metadata, residency). So the decode
//! phase persists its per-file results next to the user config, and a
//! reload whose inputs are unchanged skips straight to assembly.
//!
//! Validity is per entry, not per file-set: each entry carries the
//! source file's mtime+size and a hash of the exact decode inputs
//! (the ODF attack/release record, the aligning pipe's pitch, the
//! residency). A retuned ODF invalidates precisely the entries it
//! touches; the rest of the set stays hot. The whole file is guarded
//! by a magic+version tag — any structural surprise reads as a miss
//! and the loader simply decodes.

use std::collections::HashMap;
use std::io::{Error, ErrorKind, Read, Write};
use std::path::{Path, PathBuf};

use aristide_engine::bank::Sample;

/// Bump on ANY layout change here or in `Sample::write_cache`.
const MAGIC: &[u8; 8] = b"ARISBK02";

/// What one decoded file's cache entry restores. `info` is present for
/// attacks (the pitch metadata the spec pipeline needs) and absent for
/// releases.
pub struct Entry {
    /// Hash of the exact decode inputs (see module docs).
    pub meta_hash: u64,
    pub mtime_ns: u64,
    pub size: u64,
    pub sample: Sample,
    pub info: Option<crate::bank::DecodedInfo>,
}

/// The freshness stamp of a source file, or `None` when it can't be
/// statted (the decode will report the real error).
pub fn stamp(path: &Path) -> Option<(u64, u64)> {
    let meta = std::fs::metadata(path).ok()?;
    let mtime = meta
        .modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_nanos() as u64;
    Some((mtime, meta.len()))
}

/// Hash of the decode inputs beyond the file bytes themselves.
pub fn meta_hash(description: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::hash::DefaultHasher::new();
    description.hash(&mut hasher);
    hasher.finish()
}

pub fn read(path: &Path) -> std::io::Result<HashMap<PathBuf, Entry>> {
    let mut input = std::io::BufReader::new(std::fs::File::open(path)?);
    let mut magic = [0u8; 8];
    input.read_exact(&mut magic)?;
    if &magic != MAGIC {
        return Err(Error::new(ErrorKind::InvalidData, "cache version mismatch"));
    }
    let count = get_u64(&mut input)?;
    if count > 1 << 24 {
        return Err(Error::new(ErrorKind::InvalidData, "cache entry count absurd"));
    }
    let mut entries = HashMap::with_capacity(count as usize);
    for _ in 0..count {
        let path_len = get_u64(&mut input)?;
        if path_len > 1 << 16 {
            return Err(Error::new(ErrorKind::InvalidData, "cache path absurd"));
        }
        let mut path_bytes = vec![0u8; path_len as usize];
        input.read_exact(&mut path_bytes)?;
        let entry_path = PathBuf::from(
            String::from_utf8(path_bytes)
                .map_err(|_| Error::new(ErrorKind::InvalidData, "cache path not utf-8"))?,
        );
        let meta_hash = get_u64(&mut input)?;
        let mtime_ns = get_u64(&mut input)?;
        let size = get_u64(&mut input)?;
        let mut tag = [0u8; 1];
        input.read_exact(&mut tag)?;
        let info = match tag[0] {
            0 => None,
            _ => {
                let sample_rate = f64::from_le_bytes(get_bytes::<8>(&mut input)?);
                let mut flags = [0u8; 2];
                input.read_exact(&mut flags)?;
                let unity_note = if flags[1] != 0 {
                    let mut note = [0u8; 1];
                    input.read_exact(&mut note)?;
                    Some(note[0])
                } else {
                    None
                };
                let unity_fraction_cents = f64::from_le_bytes(get_bytes::<8>(&mut input)?);
                Some(crate::bank::DecodedInfo {
                    index: 0,
                    sample_rate,
                    percussive: flags[0] != 0,
                    unity_note,
                    unity_fraction_cents,
                })
            }
        };
        let sample = Sample::read_cache(&mut input)?;
        entries.insert(
            entry_path,
            Entry {
                meta_hash,
                mtime_ns,
                size,
                sample,
                info,
            },
        );
    }
    Ok(entries)
}

/// A borrowed entry for writing — samples are never cloned to be
/// cached; the writer borrows them wherever they already live.
pub struct EntryRef<'a> {
    pub meta_hash: u64,
    pub mtime_ns: u64,
    pub size: u64,
    pub sample: &'a Sample,
    pub info: Option<crate::bank::DecodedInfo>,
}

/// Write the cache atomically (temp file + rename): a crash mid-write
/// must never leave a half-cache that reads as valid.
pub fn write<'a>(
    path: &Path,
    entries: impl Iterator<Item = (&'a PathBuf, EntryRef<'a>)>,
    count: usize,
) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let temp = path.with_extension("tmp");
    {
        let mut out = std::io::BufWriter::new(std::fs::File::create(&temp)?);
        out.write_all(MAGIC)?;
        out.write_all(&(count as u64).to_le_bytes())?;
        for (entry_path, entry) in entries {
            let text = entry_path.to_string_lossy();
            out.write_all(&(text.len() as u64).to_le_bytes())?;
            out.write_all(text.as_bytes())?;
            out.write_all(&entry.meta_hash.to_le_bytes())?;
            out.write_all(&entry.mtime_ns.to_le_bytes())?;
            out.write_all(&entry.size.to_le_bytes())?;
            match &entry.info {
                None => out.write_all(&[0])?,
                Some(info) => {
                    out.write_all(&[1])?;
                    out.write_all(&info.sample_rate.to_le_bytes())?;
                    out.write_all(&[
                        u8::from(info.percussive),
                        u8::from(info.unity_note.is_some()),
                    ])?;
                    if let Some(note) = info.unity_note {
                        out.write_all(&[note])?;
                    }
                    out.write_all(&info.unity_fraction_cents.to_le_bytes())?;
                }
            }
            entry.sample.write_cache(&mut out)?;
        }
        out.flush()?;
    }
    std::fs::rename(&temp, path)
}

fn get_u64(input: &mut impl Read) -> std::io::Result<u64> {
    Ok(u64::from_le_bytes(get_bytes::<8>(input)?))
}

fn get_bytes<const N: usize>(input: &mut impl Read) -> std::io::Result<[u8; N]> {
    let mut bytes = [0u8; N];
    input.read_exact(&mut bytes)?;
    Ok(bytes)
}
