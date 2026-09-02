//! Scratch store for the release tails decoded during this load.
//!
//! A streaming load must never hold a whole set in RAM, not even for
//! the moment between decoding it and building the bank — that is the
//! wall streaming exists to get past. So each decode worker writes its
//! sample's tail here the instant its analysis is done and drops it
//! from memory; the sample keeps a byte offset into this file, and the
//! streamer threads read it back.
//!
//! The file is unlinked as soon as it is opened (on unix): it has no
//! name for anything else to find, and the kernel reclaims its blocks
//! when the last handle closes — including after a crash. Tails that
//! came from a warm cache never pass through here at all; the cache's
//! own tail file is already a seekable store.

use std::fs::File;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

use aristide_engine::stream::TailSink;

pub struct Spool {
    file: File,
    /// Next free byte. Workers append concurrently, so the cursor is an
    /// atomic and every write is positional — no shared seek.
    next: AtomicU64,
}

impl Spool {
    /// Create the spool next to the sample cache when there is one (the
    /// same filesystem the set's decoded bytes already fit on), else in
    /// the system temp directory.
    pub fn create(dir: Option<&Path>) -> std::io::Result<Spool> {
        let dir = dir.map(Path::to_path_buf).unwrap_or_else(std::env::temp_dir);
        std::fs::create_dir_all(&dir)?;
        let path = dir.join(format!("aristide-spool-{}.tmp", std::process::id()));
        let file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .read(true)
            .write(true)
            .open(&path)?;
        #[cfg(unix)]
        std::fs::remove_file(&path)?;
        Ok(Spool {
            file,
            next: AtomicU64::new(0),
        })
    }

    /// A second handle on the same file, for the stream stores.
    pub fn reader(&self) -> std::io::Result<File> {
        self.file.try_clone()
    }

    pub fn bytes(&self) -> u64 {
        self.next.load(Ordering::Relaxed)
    }
}

impl TailSink for &Spool {
    fn append(&mut self, bytes: &[u8]) -> std::io::Result<u64> {
        let at = self.next.fetch_add(bytes.len() as u64, Ordering::Relaxed);
        write_all_at(&self.file, at, bytes)?;
        Ok(at)
    }
}

#[cfg(unix)]
fn write_all_at(file: &File, offset: u64, bytes: &[u8]) -> std::io::Result<()> {
    std::os::unix::fs::FileExt::write_all_at(file, bytes, offset)
}

#[cfg(windows)]
fn write_all_at(file: &File, offset: u64, bytes: &[u8]) -> std::io::Result<()> {
    let mut done = 0usize;
    while done < bytes.len() {
        let wrote = std::os::windows::fs::FileExt::seek_write(
            file,
            &bytes[done..],
            offset + done as u64,
        )?;
        if wrote == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::WriteZero,
                "spool write stalled",
            ));
        }
        done += wrote;
    }
    Ok(())
}
