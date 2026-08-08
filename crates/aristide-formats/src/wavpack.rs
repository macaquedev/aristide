//! Hand-rolled FFI to system libwavpack, for decoding WavPack-compressed
//! organ samples.
//!
//! GrandOrgue sample sets often ship WavPack-compressed audio under a
//! misleading `.wav` extension (magic bytes `wvpk`); GrandOrgue itself
//! sniffs this and decodes via libwavpack, so we do the same rather than
//! trusting the extension (see [`crate::wav::read`], which sniffs and
//! delegates here automatically).
//!
//! This binds only the handful of libwavpack entry points needed to
//! decode audio and recover the original RIFF "wrapper" bytes — no
//! bindgen, no bindings crate. See `/usr/include/wavpack/wavpack.h` (or
//! the WavPack5LibraryDoc) for the authoritative signatures this was
//! transcribed from.
//!
//! ## Recovering `smpl`/`cue` metadata
//!
//! Organ sustain loops live in the `smpl` chunk of the *original* WAV
//! file, which WavPack compression discards from the decodable audio
//! stream but keeps verbatim as a "wrapper" when the file was encoded
//! with wrapper support (the default for `wvpack.exe`/GrandOrgue-built
//! sets). Opening with `OPEN_WRAPPER` and calling
//! `WavpackGetWrapperData` after `WavpackSeekTrailingWrapper` returns
//! this wrapper without decoding any audio: the original RIFF header up
//! to (and including) the `data` chunk's 8-byte id+size — but *not* its
//! payload — followed by whatever chunks originally trailed the audio
//! (e.g. a trailing `smpl`), if any. That buffer isn't a playable WAVE
//! file (the `data` chunk's declared size has no bytes behind it in the
//! buffer), so [`crate::wav::parse_wrapper_metadata`] walks it with a
//! chunk iterator that treats `data` as zero-length instead of the wav
//! module's normal strict chunk walk.
//!
//! If a file has no wrapper at all (or it doesn't parse as RIFF/WAVE),
//! metadata recovery is best-effort: loops/cue points come back empty
//! and `midi_unity_note`/`pitch_fraction` come back `None`, rather than
//! erroring the whole decode.

use std::ffi::{CString, c_char, c_int};
use std::path::Path;

use crate::wav::{WavError, WavFile, WavInfo, parse_wrapper_metadata};

// Opaque handle; libwavpack never exposes its layout to callers.
#[repr(C)]
struct WavpackContext {
    _private: [u8; 0],
}

unsafe extern "C" {
    fn WavpackOpenFileInput(
        infilename: *const c_char,
        error: *mut c_char,
        flags: c_int,
        norm_offset: c_int,
    ) -> *mut WavpackContext;
    fn WavpackCloseFile(wpc: *mut WavpackContext) -> *mut WavpackContext;
    fn WavpackGetMode(wpc: *mut WavpackContext) -> c_int;
    fn WavpackGetNumSamples64(wpc: *mut WavpackContext) -> i64;
    fn WavpackGetNumChannels(wpc: *mut WavpackContext) -> c_int;
    fn WavpackGetSampleRate(wpc: *mut WavpackContext) -> u32;
    fn WavpackGetBitsPerSample(wpc: *mut WavpackContext) -> c_int;
    fn WavpackUnpackSamples(wpc: *mut WavpackContext, buffer: *mut i32, samples: u32) -> u32;
    fn WavpackGetWrapperBytes(wpc: *mut WavpackContext) -> u32;
    fn WavpackGetWrapperData(wpc: *mut WavpackContext) -> *mut u8;
    fn WavpackFreeWrapper(wpc: *mut WavpackContext);
    fn WavpackSeekTrailingWrapper(wpc: *mut WavpackContext);
}

// Flags for the `flags` argument of WavpackOpenFileInput.
const OPEN_WRAPPER: c_int = 0x4; // make audio wrapper (RIFF) available
const OPEN_NORMALIZE: c_int = 0x10; // normalize float data to +/- 1.0

// Bits returned by WavpackGetMode.
const MODE_FLOAT: c_int = 0x8;

// libwavpack asks for an error buffer of at least 80 bytes; round up.
const ERROR_BUF_LEN: usize = 128;

/// An open libwavpack decode context. Closes the underlying file on drop.
struct WavpackFile {
    wpc: *mut WavpackContext,
}

impl WavpackFile {
    fn open(path: &Path) -> Result<Self, WavError> {
        let c_path = path_to_cstring(path)?;
        let mut error_buf = [0 as c_char; ERROR_BUF_LEN];
        // SAFETY: `c_path` is a valid, NUL-terminated C string that
        // outlives the call; `error_buf` is a valid buffer of the
        // minimum size libwavpack documents for the `error` argument.
        let wpc = unsafe {
            WavpackOpenFileInput(
                c_path.as_ptr(),
                error_buf.as_mut_ptr(),
                OPEN_WRAPPER | OPEN_NORMALIZE,
                0,
            )
        };
        if wpc.is_null() {
            return Err(WavError::WavPack(c_buf_to_string(&error_buf)));
        }
        Ok(Self { wpc })
    }

    fn channels(&self) -> u16 {
        // SAFETY: `self.wpc` is a valid, open context for the lifetime
        // of `self`; all subsequent calls share this precondition.
        unsafe { WavpackGetNumChannels(self.wpc) as u16 }
    }

    fn sample_rate(&self) -> u32 {
        unsafe { WavpackGetSampleRate(self.wpc) }
    }

    fn bits_per_sample(&self) -> u16 {
        unsafe { WavpackGetBitsPerSample(self.wpc) as u16 }
    }

    fn num_samples(&self) -> u64 {
        // WavpackGetNumSamples64 returns -1 if the sample count is
        // unknown (e.g. streamed input); we always open seekable files
        // on disk, but guard against it regardless.
        unsafe { WavpackGetNumSamples64(self.wpc).max(0) as u64 }
    }

    fn is_float(&self) -> bool {
        unsafe { WavpackGetMode(self.wpc) & MODE_FLOAT != 0 }
    }

    /// The original RIFF header (and, if present, trailer) libwavpack
    /// stored verbatim when the file was encoded, per the module docs
    /// above. Forces a seek to the end of the (seekable) file first, to
    /// pick up any trailing wrapper without decoding audio.
    fn wrapper_bytes(&self) -> Vec<u8> {
        unsafe {
            WavpackSeekTrailingWrapper(self.wpc);
            let len = WavpackGetWrapperBytes(self.wpc) as usize;
            if len == 0 {
                return Vec::new();
            }
            let data = WavpackGetWrapperData(self.wpc);
            let owned = std::slice::from_raw_parts(data, len).to_vec();
            WavpackFreeWrapper(self.wpc);
            owned
        }
    }

    /// Decodes every remaining sample to interleaved `f32`, normalized
    /// to `[-1.0, 1.0]`.
    fn decode_all(&self) -> Vec<f32> {
        const FRAMES_PER_CALL: u32 = 4096;

        let channels = self.channels() as usize;
        let bits_per_sample = self.bits_per_sample();
        let is_float = self.is_float();
        if channels == 0 {
            return Vec::new();
        }

        let mut raw = vec![0i32; FRAMES_PER_CALL as usize * channels];
        let mut samples = Vec::new();
        loop {
            // SAFETY: `raw` holds room for `FRAMES_PER_CALL * channels`
            // interleaved 32-bit values, matching what WavpackUnpackSamples
            // requires for a `samples` request of `FRAMES_PER_CALL`.
            let got = unsafe { WavpackUnpackSamples(self.wpc, raw.as_mut_ptr(), FRAMES_PER_CALL) };
            if got == 0 {
                break;
            }
            let n = got as usize * channels;
            samples.extend(
                raw[..n]
                    .iter()
                    .map(|&v| decode_sample(v, bits_per_sample, is_float)),
            );
        }
        samples
    }
}

impl Drop for WavpackFile {
    fn drop(&mut self) {
        // SAFETY: `self.wpc` was returned by a successful
        // `WavpackOpenFileInput` and hasn't been closed yet.
        unsafe {
            WavpackCloseFile(self.wpc);
        }
    }
}

// libwavpack decode contexts aren't documented as thread-safe for
// concurrent use from multiple threads, but a single context accessed
// from one thread at a time (which is how `WavpackFile` is used) is
// fine to move between threads.
unsafe impl Send for WavpackFile {}

#[cfg(unix)]
fn path_to_cstring(path: &Path) -> Result<CString, WavError> {
    use std::os::unix::ffi::OsStrExt;
    CString::new(path.as_os_str().as_bytes())
        .map_err(|_| WavError::WavPack("path contains a NUL byte".into()))
}

#[cfg(not(unix))]
fn path_to_cstring(path: &Path) -> Result<CString, WavError> {
    CString::new(path.to_string_lossy().as_bytes())
        .map_err(|_| WavError::WavPack("path contains a NUL byte".into()))
}

fn c_buf_to_string(buf: &[c_char; ERROR_BUF_LEN]) -> String {
    let bytes: Vec<u8> = buf
        .iter()
        .take_while(|&&c| c != 0)
        .map(|&c| c as u8)
        .collect();
    String::from_utf8_lossy(&bytes).into_owned()
}

/// Converts one libwavpack-unpacked sample to `[-1.0, 1.0]` f32.
///
/// WavpackUnpackSamples returns audio "right-justified" in 32-bit words:
/// integer PCM at N bits occupies the low N bits of each i32 (so a
/// 16-bit sample reads back as an actual `i16`-range value, not shifted
/// up to fill 32 bits), and float-mode data is a raw `f32` bit pattern
/// reinterpreted as i32. `OPEN_NORMALIZE` (always passed when opening)
/// ensures float data is already normalized to +/-1.0.
fn decode_sample(raw: i32, bits_per_sample: u16, is_float: bool) -> f32 {
    if is_float {
        f32::from_bits(raw as u32)
    } else {
        let full_scale = (1i64 << bits_per_sample.saturating_sub(1).max(1)) as f32;
        raw as f32 / full_scale
    }
}

/// Reads only format/loop metadata for a WavPack-compressed file,
/// without decoding audio. Unlike [`crate::wav::read_info`], `smpl`/
/// `cue` recovery here can't avoid touching disk for the trailing
/// wrapper (libwavpack must seek to the end of file to check for one),
/// but no audio is unpacked either way.
///
/// `data_offset`/`data_len` are always `0`: WavPack's compressed stream
/// has no fixed byte range to point disk-streaming reads at, so the
/// notion of a "data chunk offset" doesn't apply. Use [`read`] to get
/// decoded samples instead.
pub fn read_info(path: &Path) -> Result<WavInfo, WavError> {
    let file = WavpackFile::open(path)?;
    let wrapper = file.wrapper_bytes();
    let meta = parse_wrapper_metadata(&wrapper);

    Ok(WavInfo {
        sample_rate: file.sample_rate(),
        channels: file.channels(),
        bits_per_sample: file.bits_per_sample(),
        frames: file.num_samples(),
        loops: meta.loops,
        midi_unity_note: meta.midi_unity_note,
        pitch_fraction: meta.pitch_fraction,
        cue_points: meta.cue_points,
        data_offset: 0,
        data_len: 0,
    })
}

/// Decodes a WavPack-compressed file to interleaved `f32` samples,
/// normalized to `[-1.0, 1.0]`, recovering `smpl`/`cue` loop metadata
/// from the file's wrapper when present.
pub fn read(path: &Path) -> Result<WavFile, WavError> {
    let file = WavpackFile::open(path)?;
    let wrapper = file.wrapper_bytes();
    let meta = parse_wrapper_metadata(&wrapper);

    let channels = file.channels();
    let samples = file.decode_all();
    let frames = if channels == 0 {
        0
    } else {
        samples.len() as u64 / channels as u64
    };

    let info = WavInfo {
        sample_rate: file.sample_rate(),
        channels,
        bits_per_sample: file.bits_per_sample(),
        frames,
        loops: meta.loops,
        midi_unity_note: meta.midi_unity_note,
        pitch_fraction: meta.pitch_fraction,
        cue_points: meta.cue_points,
        data_offset: 0,
        data_len: 0,
    };
    Ok(WavFile { info, samples })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wav;
    use std::path::PathBuf;

    /// The real WavPack fixtures used here live outside the repo (they're
    /// GrandOrgue demo-set samples, not something to commit); tests that
    /// need one skip gracefully when the directory isn't present so a
    /// checkout without the fixture still passes `cargo test`.
    fn fixture_dir() -> Option<PathBuf> {
        let dir = PathBuf::from(
            "/tmp/claude-1000/-home-macaque-github-aristide/be0874ed-9e11-4d49-afee-69c9c36d0e6e/scratchpad/go-demo",
        );
        dir.is_dir().then_some(dir)
    }

    fn some_fixture_files(dir: &Path) -> Vec<PathBuf> {
        let mut files = Vec::new();
        for entry in walk(dir) {
            if entry
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("wav"))
            {
                files.push(entry);
            }
        }
        files.sort();
        files
    }

    fn walk(dir: &Path) -> Vec<PathBuf> {
        let mut out = Vec::new();
        let Ok(read_dir) = std::fs::read_dir(dir) else {
            return out;
        };
        for entry in read_dir.flatten() {
            let path = entry.path();
            if path.is_dir() {
                out.extend(walk(&path));
            } else {
                out.push(path);
            }
        }
        out
    }

    #[test]
    fn decodes_real_wavpack_samples_when_fixture_present() {
        let Some(dir) = fixture_dir() else {
            eprintln!("skipping: go-demo fixture not present");
            return;
        };
        let files = some_fixture_files(&dir);
        assert!(!files.is_empty(), "fixture dir has no .wav files");

        let mut saw_loops = false;
        for path in files.iter().take(20) {
            let bytes = std::fs::read(path).unwrap();
            assert_eq!(
                &bytes[0..4],
                b"wvpk",
                "fixture {path:?} isn't WavPack-magic"
            );

            let wav = read(path).unwrap_or_else(|e| panic!("decode {path:?}: {e}"));
            assert!(wav.info.sample_rate > 0);
            assert!(wav.info.channels > 0);
            assert!(wav.info.bits_per_sample > 0);
            assert!(!wav.samples.is_empty());
            assert_eq!(
                wav.samples.len() as u64,
                wav.info.frames * wav.info.channels as u64
            );

            for &s in &wav.samples {
                assert!((-1.0..=1.0).contains(&s), "sample out of range: {s}");
            }
            assert!(
                wav.samples.iter().any(|&s| s.abs() > 1e-4),
                "{path:?} decoded as silence"
            );

            if !wav.info.loops.is_empty() {
                saw_loops = true;
            }
        }

        assert!(
            saw_loops,
            "expected at least one sustain loop recovered from the go-demo fixtures"
        );
    }

    #[test]
    fn wav_read_delegates_transparently_when_fixture_present() {
        let Some(dir) = fixture_dir() else {
            eprintln!("skipping: go-demo fixture not present");
            return;
        };
        let files = some_fixture_files(&dir);
        let Some(path) = files.first() else {
            return;
        };

        let via_wavpack = read(path).unwrap();
        let via_wav = wav::read(path).unwrap();
        assert_eq!(via_wavpack.info, via_wav.info);
        assert_eq!(via_wavpack.samples, via_wav.samples);
    }

    #[test]
    fn read_info_matches_read_when_fixture_present() {
        let Some(dir) = fixture_dir() else {
            eprintln!("skipping: go-demo fixture not present");
            return;
        };
        let files = some_fixture_files(&dir);
        let Some(path) = files.first() else {
            return;
        };

        let info = read_info(path).unwrap();
        let full = read(path).unwrap();
        assert_eq!(info, full.info);
    }

    #[test]
    fn non_wavpack_file_errors() {
        let dir = std::env::temp_dir();
        let path = dir.join(format!(
            "aristide-wavpack-test-not-wv-{}.bin",
            std::process::id()
        ));
        std::fs::write(&path, b"not a wavpack file at all").unwrap();

        let result = read_info(&path);
        std::fs::remove_file(&path).ok();

        assert!(matches!(result, Err(WavError::WavPack(_))));
    }
}
