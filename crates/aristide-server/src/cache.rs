//! Decoded-sample load cache (GO's `GOCache` trick, gap §3b).
//!
//! Decoding (WavPack!) and per-file analysis — period refinement,
//! phase maps, tail measurement — dominate load time and are pure
//! functions of (file bytes, ODF metadata, residency). So the decode
//! phase persists its per-file results next to the user config, and a
//! reload whose inputs are unchanged skips straight to assembly.
//!
//! The cache is always *split*: an entry holds the sample's resident
//! head (everything through the last sustain loop plus the head of its
//! tail) and its tail lives in a companion `.tails` file. That is what
//! lets one cache serve both residencies — a fully-resident load reads
//! the tail back into RAM, a streaming load leaves it on disk and reads
//! it through the streamer. The two files carry a shared generation
//! stamp so a crash between their renames reads as a miss rather than
//! as garbage audio at stale offsets.
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

use aristide_engine::bank::{Sample, StreamRegion};
use aristide_engine::stream::{StreamStores, TailSink};

/// Bump on ANY layout change here or in `Sample::write_cache` — and on
/// any change to what the persisted analysis *means*, since a stale
/// entry restores the old numbers verbatim (03: stereo-joint release
/// alignment; 04: entries split head/tail for streaming, both
/// 2026-09-02).
const MAGIC: &[u8; 8] = b"ARISBK04";
/// Companion file holding the samples' streamed tails.
const TAIL_MAGIC: &[u8; 8] = b"ARISTL01";
/// Header bytes before the first tail — every recorded offset is
/// absolute in the file, so it is also what a streamer seeks to.
const TAIL_HEADER: u64 = 16;

/// The tail file of one cache, open for the life of a load.
pub struct Tails {
    file: std::fs::File,
    generation: u64,
    /// Set when this load streams: the stream-store index the samples'
    /// regions point at. `None` = tails are read back into RAM.
    pub store: Option<u16>,
}

impl Tails {
    /// Open an existing tail file. A missing or foreign one is an error
    /// — the caller treats that as "the whole cache misses".
    pub fn open(path: &Path) -> std::io::Result<Tails> {
        let mut file = std::fs::File::open(path)?;
        let mut magic = [0u8; 8];
        file.read_exact(&mut magic)?;
        if &magic != TAIL_MAGIC {
            return Err(Error::new(ErrorKind::InvalidData, "tail file foreign"));
        }
        let generation = get_u64(&mut file)?;
        Ok(Tails {
            file,
            generation,
            store: None,
        })
    }

    /// Hand the open file to the stream stores (which then own it).
    pub fn take_file(&self) -> std::io::Result<std::fs::File> {
        self.file.try_clone()
    }

    fn read_bytes(&self, offset: u64, len: u64) -> std::io::Result<Vec<u8>> {
        let mut bytes = vec![0u8; len as usize];
        read_exact_at(&self.file, offset, &mut bytes)?;
        Ok(bytes)
    }
}

#[cfg(unix)]
fn read_exact_at(file: &std::fs::File, offset: u64, buf: &mut [u8]) -> std::io::Result<()> {
    std::os::unix::fs::FileExt::read_exact_at(file, buf, offset)
}

#[cfg(not(unix))]
fn read_exact_at(file: &std::fs::File, offset: u64, buf: &mut [u8]) -> std::io::Result<()> {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = file.try_clone()?;
    file.seek(SeekFrom::Start(offset))?;
    file.read_exact(buf)
}

/// Appends to the tail file being written, handing back the absolute
/// offset of every run — which is what the entries record.
struct TailWriter<W: Write> {
    out: W,
    offset: u64,
}

impl<W: Write> TailSink for TailWriter<W> {
    fn append(&mut self, bytes: &[u8]) -> std::io::Result<u64> {
        let at = self.offset;
        self.out.write_all(bytes)?;
        self.offset += bytes.len() as u64;
        Ok(at)
    }
}

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

pub fn read(path: &Path, tails: Option<&Tails>) -> std::io::Result<HashMap<PathBuf, Entry>> {
    let mut input = std::io::BufReader::new(std::fs::File::open(path)?);
    let mut magic = [0u8; 8];
    input.read_exact(&mut magic)?;
    if &magic != MAGIC {
        return Err(Error::new(ErrorKind::InvalidData, "cache version mismatch"));
    }
    let generation = get_u64(&mut input)?;
    // A half-written pair (a crash between the two renames) would put
    // this entry's tail at somebody else's offset: refuse it.
    if tails.is_some_and(|tails| tails.generation != generation) {
        return Err(Error::new(ErrorKind::InvalidData, "cache generation split"));
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
        let (mut sample, tail) = Sample::read_cache(&mut input)?;
        if let Some(tail) = tail {
            let Some(tails) = tails else {
                return Err(Error::new(ErrorKind::InvalidData, "cache tail file missing"));
            };
            match tails.store {
                // Streaming: the tail stays where it is and the sample
                // learns where to find it.
                Some(store) => sample.attach_stream(StreamRegion {
                    store,
                    offset: tail.offset,
                    first_frame: tail.first_frame,
                    frames: sample.frames().saturating_sub(tail.first_frame),
                }),
                // Fully resident: read it back and make the sample whole.
                None => {
                    let bytes = tails.read_bytes(tail.offset, tail.len)?;
                    sample
                        .absorb_tail(&bytes)
                        .map_err(|err| Error::new(ErrorKind::InvalidData, err))?;
                }
            }
        }
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
    tails_path: &Path,
    stores: Option<&StreamStores>,
    entries: impl Iterator<Item = (&'a PathBuf, EntryRef<'a>)>,
    count: usize,
) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let temp = path.with_extension("tmp");
    // Not `with_extension`: `<hash>.tails` and `<hash>.samples` would
    // both become `<hash>.tmp` and clobber each other.
    let tail_temp = tails_path.with_file_name(format!(
        "{}.tmp",
        tails_path.file_name().unwrap_or_default().to_string_lossy()
    ));
    // Ties the two files together: an entry read against a tail file of
    // another generation is refused, so a crash between the renames
    // costs a re-decode and never a wrong seek.
    let generation = {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::hash::DefaultHasher::new();
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
            .hash(&mut hasher);
        std::process::id().hash(&mut hasher);
        hasher.finish()
    };
    {
        let mut tail_out = TailWriter {
            out: std::io::BufWriter::new(std::fs::File::create(&tail_temp)?),
            offset: TAIL_HEADER,
        };
        tail_out.out.write_all(TAIL_MAGIC)?;
        tail_out.out.write_all(&generation.to_le_bytes())?;
        let mut out = std::io::BufWriter::new(std::fs::File::create(&temp)?);
        out.write_all(MAGIC)?;
        out.write_all(&generation.to_le_bytes())?;
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
            let tail = entry.sample.write_tail(&mut tail_out, stores)?;
            entry.sample.write_cache(&mut out, tail)?;
        }
        out.flush()?;
        tail_out.out.flush()?;
    }
    // Tails first: an entry file is only ever read against a tail file
    // of its own generation, so the visible pair is always consistent.
    std::fs::rename(&tail_temp, tails_path)?;
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
