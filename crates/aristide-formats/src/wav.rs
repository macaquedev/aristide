//! Hand-rolled RIFF/WAVE reader.
//!
//! Organ samples carry sustain-loop points in the `smpl` chunk (and
//! sometimes `cue ` markers) that mainstream crates such as `hound`
//! don't expose, so this parses the container directly. PCM 8/16/24/32-bit
//! integer and IEEE float 32-bit are supported, plain or wrapped in
//! `WAVE_FORMAT_EXTENSIBLE`.
//!
//! [`read_info`] reads only chunk headers/metadata, leaving the `data`
//! chunk's bytes on disk unread — needed later for streaming large
//! release samples straight from disk instead of decoding into RAM.

use std::fs;
use std::path::Path;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum WavError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("not a RIFF/WAVE file")]
    NotRiff,
    #[error("truncated file: expected at least {expected} bytes, found {found}")]
    Truncated { expected: usize, found: usize },
    #[error("missing required '{0}' chunk")]
    MissingChunk(&'static str),
    #[error("unsupported format: {0}")]
    Unsupported(String),
    #[error("malformed '{chunk}' chunk: {reason}")]
    Malformed { chunk: &'static str, reason: String },
}

/// A sustain loop from the `smpl` chunk, in sample frames.
///
/// Per the RIFF spec, `end` is the last frame *included* in the loop
/// (not one-past-the-end): a loop with `start == end` is a single-frame
/// loop, and looping frame indices run `start..=end` inclusive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WavLoop {
    pub start: u64,
    pub end: u64,
}

/// Metadata describing a WAV file's format and layout, without decoded
/// sample data.
#[derive(Debug, Clone, PartialEq)]
pub struct WavInfo {
    pub sample_rate: u32,
    pub channels: u16,
    pub bits_per_sample: u16,
    /// Sample frame count (i.e. samples per channel), derived from the
    /// `data` chunk length.
    pub frames: u64,
    pub loops: Vec<WavLoop>,
    pub midi_unity_note: Option<u8>,
    /// Fractional pitch offset above `midi_unity_note`, as the `smpl`
    /// chunk's 32-bit fraction of a semitone (0 = exactly in tune).
    pub pitch_fraction: Option<u32>,
    pub cue_points: Vec<u64>,
    /// Byte offset of the `data` chunk's payload within the file.
    pub data_offset: u64,
    /// Byte length of the `data` chunk's payload.
    pub data_len: u64,
}

#[derive(Debug, Clone)]
pub struct WavFile {
    pub info: WavInfo,
    /// Interleaved samples, normalized to `[-1.0, 1.0]`.
    pub samples: Vec<f32>,
}

const RIFF_HEADER_LEN: usize = 12;
const CHUNK_HEADER_LEN: usize = 8;

#[derive(Clone, Copy)]
enum SampleFormat {
    PcmUnsigned8,
    PcmSigned16,
    PcmSigned24,
    PcmSigned32,
    Float32,
}

struct FmtChunk {
    format: SampleFormat,
    channels: u16,
    sample_rate: u32,
    bits_per_sample: u16,
}

/// One RIFF chunk's id, and the byte range of its payload in `bytes`.
struct ChunkRef {
    id: [u8; 4],
    start: usize,
    len: usize,
}

fn require_len(bytes: &[u8], needed: usize) -> Result<(), WavError> {
    if bytes.len() < needed {
        Err(WavError::Truncated {
            expected: needed,
            found: bytes.len(),
        })
    } else {
        Ok(())
    }
}

fn read_u16(bytes: &[u8], offset: usize) -> Result<u16, WavError> {
    require_len(bytes, offset + 2)?;
    Ok(u16::from_le_bytes([bytes[offset], bytes[offset + 1]]))
}

fn read_u32(bytes: &[u8], offset: usize) -> Result<u32, WavError> {
    require_len(bytes, offset + 4)?;
    Ok(u32::from_le_bytes([
        bytes[offset],
        bytes[offset + 1],
        bytes[offset + 2],
        bytes[offset + 3],
    ]))
}

/// Walks top-level chunks inside the WAVE container (i.e. after the
/// 12-byte RIFF/WAVE header), validating each chunk's declared size
/// against the buffer before yielding it.
fn iter_chunks(bytes: &[u8]) -> Result<Vec<ChunkRef>, WavError> {
    let mut chunks = Vec::new();
    let mut pos = RIFF_HEADER_LEN;
    while pos + CHUNK_HEADER_LEN <= bytes.len() {
        let id = [bytes[pos], bytes[pos + 1], bytes[pos + 2], bytes[pos + 3]];
        let len = read_u32(bytes, pos + 4)? as usize;
        let start = pos + CHUNK_HEADER_LEN;
        require_len(bytes, start + len)?;
        chunks.push(ChunkRef { id, start, len });
        // RIFF pads odd-sized chunks to an even boundary.
        let padded_len = len + (len & 1);
        pos = start + padded_len;
    }
    Ok(chunks)
}

fn find_chunk<'a>(chunks: &'a [ChunkRef], id: &[u8; 4]) -> Option<&'a ChunkRef> {
    chunks.iter().find(|c| &c.id == id)
}

fn parse_riff_header(bytes: &[u8]) -> Result<(), WavError> {
    require_len(bytes, RIFF_HEADER_LEN)?;
    if &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err(WavError::NotRiff);
    }
    Ok(())
}

// WAVE_FORMAT_* codes, as stored in the `fmt ` chunk's first field.
const WAVE_FORMAT_PCM: u16 = 0x0001;
const WAVE_FORMAT_IEEE_FLOAT: u16 = 0x0003;
const WAVE_FORMAT_EXTENSIBLE: u16 = 0xFFFE;

// GUID subformat tags used inside a WAVE_FORMAT_EXTENSIBLE fmt chunk;
// only the leading 16-bit field differs from the two codes above, the
// trailing 14 bytes are a fixed suffix shared by all standard subformats.
const SUBFORMAT_PCM_PREFIX: u16 = WAVE_FORMAT_PCM;
const SUBFORMAT_IEEE_FLOAT_PREFIX: u16 = WAVE_FORMAT_IEEE_FLOAT;

fn parse_fmt_chunk(bytes: &[u8], chunk: &ChunkRef) -> Result<FmtChunk, WavError> {
    let payload = &bytes[chunk.start..chunk.start + chunk.len];
    if payload.len() < 16 {
        return Err(WavError::Malformed {
            chunk: "fmt ",
            reason: format!("only {} bytes, need at least 16", payload.len()),
        });
    }

    let mut format_tag = read_u16(payload, 0)?;
    let channels = read_u16(payload, 2)?;
    let sample_rate = read_u32(payload, 4)?;
    let bits_per_sample = read_u16(payload, 14)?;

    if format_tag == WAVE_FORMAT_EXTENSIBLE {
        if payload.len() < 40 {
            return Err(WavError::Malformed {
                chunk: "fmt ",
                reason: "WAVE_FORMAT_EXTENSIBLE requires 40 bytes".into(),
            });
        }
        // Sub-format GUID starts at offset 24; its first 16 bits carry
        // the same codes as the plain format tag.
        format_tag = read_u16(payload, 24)?;
    }

    let format = match (format_tag, bits_per_sample) {
        (SUBFORMAT_PCM_PREFIX, 8) => SampleFormat::PcmUnsigned8,
        (SUBFORMAT_PCM_PREFIX, 16) => SampleFormat::PcmSigned16,
        (SUBFORMAT_PCM_PREFIX, 24) => SampleFormat::PcmSigned24,
        (SUBFORMAT_PCM_PREFIX, 32) => SampleFormat::PcmSigned32,
        (SUBFORMAT_IEEE_FLOAT_PREFIX, 32) => SampleFormat::Float32,
        (tag, bits) => {
            return Err(WavError::Unsupported(format!(
                "format tag 0x{tag:04X} at {bits} bits"
            )));
        }
    };

    Ok(FmtChunk {
        format,
        channels,
        sample_rate,
        bits_per_sample,
    })
}

/// The `smpl` chunk's fixed 36-byte header, before its loop table.
struct SmplHeader {
    midi_unity_note: u8,
    pitch_fraction: u32,
    loop_count: u32,
}

fn parse_smpl_header(payload: &[u8]) -> Result<SmplHeader, WavError> {
    if payload.len() < 36 {
        return Err(WavError::Malformed {
            chunk: "smpl",
            reason: format!("only {} bytes, need at least 36", payload.len()),
        });
    }
    let midi_unity_note = read_u32(payload, 12)? as u8;
    let pitch_fraction = read_u32(payload, 16)?;
    let loop_count = read_u32(payload, 28)?;
    Ok(SmplHeader {
        midi_unity_note,
        pitch_fraction,
        loop_count,
    })
}

fn parse_smpl_loops(payload: &[u8], loop_count: u32) -> Result<Vec<WavLoop>, WavError> {
    const HEADER_LEN: usize = 36;
    const LOOP_ENTRY_LEN: usize = 24;

    let mut loops = Vec::with_capacity(loop_count as usize);
    for i in 0..loop_count as usize {
        let entry_start = HEADER_LEN + i * LOOP_ENTRY_LEN;
        if payload.len() < entry_start + LOOP_ENTRY_LEN {
            return Err(WavError::Malformed {
                chunk: "smpl",
                reason: format!("loop table truncated at entry {i}"),
            });
        }
        let start = read_u32(payload, entry_start + 8)? as u64;
        let end = read_u32(payload, entry_start + 12)? as u64;
        loops.push(WavLoop { start, end });
    }
    Ok(loops)
}

fn parse_cue_points(payload: &[u8]) -> Result<Vec<u64>, WavError> {
    const CUE_ENTRY_LEN: usize = 24;
    if payload.len() < 4 {
        return Err(WavError::Malformed {
            chunk: "cue ",
            reason: format!("only {} bytes, need at least 4", payload.len()),
        });
    }
    let count = read_u32(payload, 0)? as usize;
    let mut points = Vec::with_capacity(count);
    for i in 0..count {
        let entry_start = 4 + i * CUE_ENTRY_LEN;
        if payload.len() < entry_start + CUE_ENTRY_LEN {
            return Err(WavError::Malformed {
                chunk: "cue ",
                reason: format!("cue table truncated at entry {i}"),
            });
        }
        // Offset 20..24 of each entry is the sample offset field, which
        // is what matters for playback-position markers; the other
        // fields (chunk id, position, block start, byte offset) address
        // compressed/multi-chunk layouts we don't support.
        let sample_offset = read_u32(payload, entry_start + 20)? as u64;
        points.push(sample_offset);
    }
    Ok(points)
}

fn build_info(bytes: &[u8], chunks: &[ChunkRef]) -> Result<(WavInfo, FmtChunk), WavError> {
    let fmt_chunk = find_chunk(chunks, b"fmt ").ok_or(WavError::MissingChunk("fmt "))?;
    let fmt = parse_fmt_chunk(bytes, fmt_chunk)?;

    let data_chunk = find_chunk(chunks, b"data").ok_or(WavError::MissingChunk("data"))?;
    let bytes_per_frame = fmt.channels as u64 * (fmt.bits_per_sample as u64 / 8);
    let frames = (data_chunk.len as u64)
        .checked_div(bytes_per_frame)
        .unwrap_or(0);

    let mut loops = Vec::new();
    let mut midi_unity_note = None;
    let mut pitch_fraction = None;
    if let Some(smpl_chunk) = find_chunk(chunks, b"smpl") {
        let payload = &bytes[smpl_chunk.start..smpl_chunk.start + smpl_chunk.len];
        let header = parse_smpl_header(payload)?;
        loops = parse_smpl_loops(payload, header.loop_count)?;
        midi_unity_note = Some(header.midi_unity_note);
        pitch_fraction = Some(header.pitch_fraction);
    }

    let mut cue_points = Vec::new();
    if let Some(cue_chunk) = find_chunk(chunks, b"cue ") {
        let payload = &bytes[cue_chunk.start..cue_chunk.start + cue_chunk.len];
        cue_points = parse_cue_points(payload)?;
    }

    let info = WavInfo {
        sample_rate: fmt.sample_rate,
        channels: fmt.channels,
        bits_per_sample: fmt.bits_per_sample,
        frames,
        loops,
        midi_unity_note,
        pitch_fraction,
        cue_points,
        data_offset: data_chunk.start as u64,
        data_len: data_chunk.len as u64,
    };
    Ok((info, fmt))
}

/// Reads only chunk headers and metadata, leaving `data` chunk bytes
/// unread. Use this to plan disk-streaming reads of large samples
/// without decoding them into memory.
pub fn read_info(path: &Path) -> Result<WavInfo, WavError> {
    let bytes = fs::read(path)?;
    parse_riff_header(&bytes)?;
    let chunks = iter_chunks(&bytes)?;
    let (info, _fmt) = build_info(&bytes, &chunks)?;
    Ok(info)
}

/// Decodes sample data to interleaved `f32`, normalized to `[-1.0, 1.0]`.
fn decode_samples(bytes: &[u8], data_chunk: &ChunkRef, fmt: &FmtChunk) -> Vec<f32> {
    let payload = &bytes[data_chunk.start..data_chunk.start + data_chunk.len];
    match fmt.format {
        SampleFormat::PcmUnsigned8 => payload
            .iter()
            .map(|&b| (b as f32 - 128.0) / 128.0)
            .collect(),
        SampleFormat::PcmSigned16 => payload
            .chunks_exact(2)
            .map(|c| i16::from_le_bytes([c[0], c[1]]) as f32 / 32768.0)
            .collect(),
        SampleFormat::PcmSigned24 => payload
            .chunks_exact(3)
            .map(|c| {
                // Sign-extend the 3-byte little-endian sample by placing
                // it in the top 3 bytes of an i32 and arithmetic-shifting
                // back down, which carries the sign bit through.
                let raw = (c[0] as i32) | (c[1] as i32) << 8 | (c[2] as i32) << 16;
                let signed = (raw << 8) >> 8;
                signed as f32 / 8_388_608.0
            })
            .collect(),
        SampleFormat::PcmSigned32 => payload
            .chunks_exact(4)
            .map(|c| i32::from_le_bytes([c[0], c[1], c[2], c[3]]) as f32 / 2_147_483_648.0)
            .collect(),
        SampleFormat::Float32 => payload
            .chunks_exact(4)
            .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
            .collect(),
    }
}

/// Parses a full WAV file from an in-memory buffer, decoding sample data.
pub fn parse(bytes: &[u8]) -> Result<WavFile, WavError> {
    parse_riff_header(bytes)?;
    let chunks = iter_chunks(bytes)?;
    let (info, fmt) = build_info(bytes, &chunks)?;
    let data_chunk = find_chunk(&chunks, b"data").ok_or(WavError::MissingChunk("data"))?;
    let samples = decode_samples(bytes, data_chunk, &fmt);
    Ok(WavFile { info, samples })
}

/// Reads and parses a WAV file from disk, decoding sample data.
pub fn read(path: &Path) -> Result<WavFile, WavError> {
    let bytes = fs::read(path)?;
    parse(&bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Assembles a RIFF chunk: 4-byte id, u32 length, payload, and (per
    /// spec) a zero pad byte if the payload length is odd.
    fn chunk(id: &[u8; 4], payload: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(id);
        out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        out.extend_from_slice(payload);
        if payload.len() % 2 == 1 {
            out.push(0);
        }
        out
    }

    /// Assembles a full RIFF/WAVE file from an already-built list of
    /// inner chunks (each produced by `chunk`).
    fn riff(inner_chunks: &[Vec<u8>]) -> Vec<u8> {
        let mut body = Vec::new();
        body.extend_from_slice(b"WAVE");
        for c in inner_chunks {
            body.extend_from_slice(c);
        }
        let mut out = Vec::new();
        out.extend_from_slice(b"RIFF");
        out.extend_from_slice(&(body.len() as u32).to_le_bytes());
        out.extend_from_slice(&body);
        out
    }

    fn fmt_chunk_pcm(channels: u16, sample_rate: u32, bits_per_sample: u16) -> Vec<u8> {
        let block_align = channels * (bits_per_sample / 8);
        let byte_rate = sample_rate * block_align as u32;
        let mut payload = Vec::new();
        payload.extend_from_slice(&WAVE_FORMAT_PCM.to_le_bytes());
        payload.extend_from_slice(&channels.to_le_bytes());
        payload.extend_from_slice(&sample_rate.to_le_bytes());
        payload.extend_from_slice(&byte_rate.to_le_bytes());
        payload.extend_from_slice(&block_align.to_le_bytes());
        payload.extend_from_slice(&bits_per_sample.to_le_bytes());
        chunk(b"fmt ", &payload)
    }

    fn fmt_chunk_float32(channels: u16, sample_rate: u32) -> Vec<u8> {
        let bits_per_sample: u16 = 32;
        let block_align = channels * (bits_per_sample / 8);
        let byte_rate = sample_rate * block_align as u32;
        let mut payload = Vec::new();
        payload.extend_from_slice(&WAVE_FORMAT_IEEE_FLOAT.to_le_bytes());
        payload.extend_from_slice(&channels.to_le_bytes());
        payload.extend_from_slice(&sample_rate.to_le_bytes());
        payload.extend_from_slice(&byte_rate.to_le_bytes());
        payload.extend_from_slice(&block_align.to_le_bytes());
        payload.extend_from_slice(&bits_per_sample.to_le_bytes());
        chunk(b"fmt ", &payload)
    }

    /// A WAVE_FORMAT_EXTENSIBLE `fmt ` chunk wrapping the given subformat
    /// tag (PCM or IEEE float).
    fn fmt_chunk_extensible(
        channels: u16,
        sample_rate: u32,
        bits_per_sample: u16,
        subformat_tag: u16,
    ) -> Vec<u8> {
        let block_align = channels * (bits_per_sample / 8);
        let byte_rate = sample_rate * block_align as u32;
        let mut payload = Vec::new();
        payload.extend_from_slice(&WAVE_FORMAT_EXTENSIBLE.to_le_bytes());
        payload.extend_from_slice(&channels.to_le_bytes());
        payload.extend_from_slice(&sample_rate.to_le_bytes());
        payload.extend_from_slice(&byte_rate.to_le_bytes());
        payload.extend_from_slice(&block_align.to_le_bytes());
        payload.extend_from_slice(&bits_per_sample.to_le_bytes());
        // cbSize
        payload.extend_from_slice(&22u16.to_le_bytes());
        // valid bits per sample
        payload.extend_from_slice(&bits_per_sample.to_le_bytes());
        // channel mask
        payload.extend_from_slice(&0u32.to_le_bytes());
        // subformat GUID: tag + fixed KSDATAFORMAT_SUBTYPE_PCM/IEEE_FLOAT suffix
        payload.extend_from_slice(&subformat_tag.to_le_bytes());
        payload.extend_from_slice(&[
            0x00, 0x00, 0x00, 0x00, 0x10, 0x00, 0x80, 0x00, 0x00, 0xAA, 0x00, 0x38, 0x9B, 0x71,
        ]);
        chunk(b"fmt ", &payload)
    }

    fn data_chunk(payload: &[u8]) -> Vec<u8> {
        chunk(b"data", payload)
    }

    #[test]
    fn roundtrip_16bit_stereo() {
        let samples: [i16; 4] = [0, i16::MAX, i16::MIN, -16384];
        let mut pcm = Vec::new();
        for s in samples {
            pcm.extend_from_slice(&s.to_le_bytes());
        }
        let bytes = riff(&[fmt_chunk_pcm(2, 44100, 16), data_chunk(&pcm)]);

        let wav = parse(&bytes).unwrap();
        assert_eq!(wav.info.sample_rate, 44100);
        assert_eq!(wav.info.channels, 2);
        assert_eq!(wav.info.bits_per_sample, 16);
        assert_eq!(wav.info.frames, 2);
        assert_eq!(wav.samples.len(), 4);
        assert!((wav.samples[0] - 0.0).abs() < 1e-6);
        assert!((wav.samples[1] - 1.0).abs() < 1e-4);
        assert!((wav.samples[2] - (-1.0)).abs() < 1e-6);
        assert!((wav.samples[3] - (-0.5)).abs() < 1e-4);
    }

    #[test]
    fn sign_extension_24bit() {
        // 0x800000 = most negative 24-bit value -> -1.0
        // 0x7FFFFF = most positive 24-bit value -> ~1.0
        // 0x000000 = zero
        // 0xFFFFFF = -1 (LSB) -> just below zero
        let mut pcm = Vec::new();
        pcm.extend_from_slice(&[0x00, 0x00, 0x80]); // 0x800000 LE
        pcm.extend_from_slice(&[0xFF, 0xFF, 0x7F]); // 0x7FFFFF LE
        pcm.extend_from_slice(&[0x00, 0x00, 0x00]);
        pcm.extend_from_slice(&[0xFF, 0xFF, 0xFF]);
        let bytes = riff(&[fmt_chunk_pcm(1, 48000, 24), data_chunk(&pcm)]);

        let wav = parse(&bytes).unwrap();
        assert_eq!(wav.samples.len(), 4);
        assert!((wav.samples[0] - (-1.0)).abs() < 1e-9);
        assert!((wav.samples[1] - 1.0).abs() < 1e-6);
        assert_eq!(wav.samples[2], 0.0);
        assert!((wav.samples[3] - (-1.0 / 8_388_608.0)).abs() < 1e-9);
    }

    #[test]
    fn float32_passthrough() {
        let values: [f32; 3] = [0.0, 0.5, -0.75];
        let mut pcm = Vec::new();
        for v in values {
            pcm.extend_from_slice(&v.to_le_bytes());
        }
        let bytes = riff(&[fmt_chunk_float32(1, 96000), data_chunk(&pcm)]);

        let wav = parse(&bytes).unwrap();
        assert_eq!(wav.samples, values);
    }

    #[test]
    fn u8_offset() {
        let pcm: [u8; 3] = [0, 128, 255];
        let bytes = riff(&[fmt_chunk_pcm(1, 8000, 8), data_chunk(&pcm)]);

        let wav = parse(&bytes).unwrap();
        assert!((wav.samples[0] - (-1.0)).abs() < 1e-6);
        assert!((wav.samples[1] - 0.0).abs() < 1e-6);
        assert!((wav.samples[2] - (127.0 / 128.0)).abs() < 1e-6);
    }

    #[test]
    fn smpl_loop_parsing() {
        let pcm = vec![0u8; 16]; // 8 frames of 16-bit mono
        let mut smpl = Vec::new();
        smpl.extend_from_slice(&0u32.to_le_bytes()); // manufacturer
        smpl.extend_from_slice(&0u32.to_le_bytes()); // product
        smpl.extend_from_slice(&0u32.to_le_bytes()); // sample period
        smpl.extend_from_slice(&60u32.to_le_bytes()); // midi unity note
        smpl.extend_from_slice(&(1u32 << 30).to_le_bytes()); // pitch fraction
        smpl.extend_from_slice(&0u32.to_le_bytes()); // smpte format
        smpl.extend_from_slice(&0u32.to_le_bytes()); // smpte offset
        smpl.extend_from_slice(&1u32.to_le_bytes()); // num sample loops
        smpl.extend_from_slice(&0u32.to_le_bytes()); // sampler data
        // one loop entry
        smpl.extend_from_slice(&0u32.to_le_bytes()); // cue point id
        smpl.extend_from_slice(&0u32.to_le_bytes()); // type (forward)
        smpl.extend_from_slice(&2u32.to_le_bytes()); // start frame
        smpl.extend_from_slice(&6u32.to_le_bytes()); // end frame (inclusive)
        smpl.extend_from_slice(&0u32.to_le_bytes()); // fraction
        smpl.extend_from_slice(&0u32.to_le_bytes()); // play count

        let bytes = riff(&[
            fmt_chunk_pcm(1, 44100, 16),
            chunk(b"smpl", &smpl),
            data_chunk(&pcm),
        ]);

        let wav = parse(&bytes).unwrap();
        assert_eq!(wav.info.midi_unity_note, Some(60));
        assert_eq!(wav.info.pitch_fraction, Some(1 << 30));
        assert_eq!(wav.info.loops, vec![WavLoop { start: 2, end: 6 }]);
    }

    #[test]
    fn cue_point_parsing() {
        let pcm = vec![0u8; 8];
        let mut cue = Vec::new();
        cue.extend_from_slice(&1u32.to_le_bytes()); // num cue points
        cue.extend_from_slice(&0u32.to_le_bytes()); // cue point id
        cue.extend_from_slice(&0u32.to_le_bytes()); // position
        cue.extend_from_slice(b"data"); // chunk id
        cue.extend_from_slice(&0u32.to_le_bytes()); // chunk start
        cue.extend_from_slice(&0u32.to_le_bytes()); // block start
        cue.extend_from_slice(&3u32.to_le_bytes()); // sample offset

        let bytes = riff(&[
            fmt_chunk_pcm(1, 44100, 16),
            chunk(b"cue ", &cue),
            data_chunk(&pcm),
        ]);

        let wav = parse(&bytes).unwrap();
        assert_eq!(wav.info.cue_points, vec![3]);
    }

    #[test]
    fn extensible_pcm16() {
        let samples: [i16; 2] = [1000, -1000];
        let mut pcm = Vec::new();
        for s in samples {
            pcm.extend_from_slice(&s.to_le_bytes());
        }
        let bytes = riff(&[
            fmt_chunk_extensible(1, 44100, 16, WAVE_FORMAT_PCM),
            data_chunk(&pcm),
        ]);

        let wav = parse(&bytes).unwrap();
        assert_eq!(wav.samples.len(), 2);
        assert!((wav.samples[0] - 1000.0 / 32768.0).abs() < 1e-6);
    }

    #[test]
    fn extensible_float32() {
        let values: [f32; 2] = [0.25, -0.25];
        let mut pcm = Vec::new();
        for v in values {
            pcm.extend_from_slice(&v.to_le_bytes());
        }
        let bytes = riff(&[
            fmt_chunk_extensible(1, 44100, 32, WAVE_FORMAT_IEEE_FLOAT),
            data_chunk(&pcm),
        ]);

        let wav = parse(&bytes).unwrap();
        assert_eq!(wav.samples, values);
    }

    #[test]
    fn truncated_file_errors_not_panics() {
        let full = riff(&[fmt_chunk_pcm(1, 44100, 16), data_chunk(&[0, 0, 0, 0])]);
        let truncated = &full[..full.len() - 6];

        let result = parse(truncated);
        assert!(result.is_err());
    }

    #[test]
    fn truncated_header_errors() {
        let result = parse(b"RIF");
        assert!(matches!(result, Err(WavError::Truncated { .. })));
    }

    #[test]
    fn not_riff_errors() {
        let result = parse(b"not a riff file at all, just junk bytes here");
        assert!(matches!(result, Err(WavError::NotRiff)));
    }

    #[test]
    fn unknown_chunk_skipped() {
        let pcm: [u8; 2] = [1, 2];
        let bytes = riff(&[
            fmt_chunk_pcm(1, 44100, 8),
            chunk(b"LIST", b"some metadata nobody parses"),
            chunk(b"JUNK", b"padding"),
            data_chunk(&pcm),
        ]);

        let wav = parse(&bytes).unwrap();
        assert_eq!(wav.samples.len(), 2);
    }

    #[test]
    fn odd_sized_chunk_padding() {
        let pcm: [u8; 3] = [10, 20, 30]; // odd length -> padded
        let bytes = riff(&[
            fmt_chunk_pcm(1, 44100, 8),
            data_chunk(&pcm),
            chunk(b"LIST", b"x"), // odd-length chunk after data, must still parse
        ]);

        let wav = parse(&bytes).unwrap();
        assert_eq!(wav.samples.len(), 3);

        let chunks = iter_chunks(&bytes).unwrap();
        assert!(find_chunk(&chunks, b"LIST").is_some());
    }

    #[test]
    fn read_info_does_not_decode_data() {
        let samples: [i16; 100] = [12345; 100];
        let mut pcm = Vec::new();
        for s in samples {
            pcm.extend_from_slice(&s.to_le_bytes());
        }
        let bytes = riff(&[fmt_chunk_pcm(1, 22050, 16), data_chunk(&pcm)]);

        let dir = std::env::temp_dir();
        let path = dir.join(format!("aristide-wav-test-{}.wav", std::process::id()));
        std::fs::write(&path, &bytes).unwrap();

        let info = read_info(&path).unwrap();
        assert_eq!(info.frames, 100);
        assert_eq!(info.data_len, pcm.len() as u64);
        assert_eq!(
            info.data_offset as usize + info.data_len as usize,
            bytes.len()
        );

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn read_from_path_roundtrip() {
        let pcm: [u8; 4] = [0, 0, 0, 0];
        let bytes = riff(&[fmt_chunk_pcm(1, 44100, 16), data_chunk(&pcm)]);

        let dir = std::env::temp_dir();
        let path = dir.join(format!("aristide-wav-test-read-{}.wav", std::process::id()));
        std::fs::write(&path, &bytes).unwrap();

        let wav = read(&path).unwrap();
        assert_eq!(wav.info.frames, 2);

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn unsupported_bit_depth_errors() {
        // 12-bit PCM: not one of our supported depths.
        let bytes = riff(&[fmt_chunk_pcm(1, 44100, 12), data_chunk(&[0, 0])]);
        let result = parse(&bytes);
        assert!(matches!(result, Err(WavError::Unsupported(_))));
    }

    #[test]
    fn missing_fmt_chunk_errors() {
        let bytes = riff(&[data_chunk(&[0, 0])]);
        let result = parse(&bytes);
        assert!(matches!(result, Err(WavError::MissingChunk("fmt "))));
    }

    #[test]
    fn missing_data_chunk_errors() {
        let bytes = riff(&[fmt_chunk_pcm(1, 44100, 16)]);
        let result = parse(&bytes);
        assert!(matches!(result, Err(WavError::MissingChunk("data"))));
    }
}
