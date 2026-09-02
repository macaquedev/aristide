//! Disk streaming: a tail played off a store must be the same audio as
//! the same tail played out of RAM, and every way the stream can fail
//! must cost a fade rather than a click.

use std::fs::File;
use std::io::Write;
use std::path::PathBuf;

use super::*;
use crate::stream::{StreamCounters, TailSink};

/// Writes tails to a scratch file, handing back their offsets — the
/// test's stand-in for the loader's spool.
struct FileSink {
    file: File,
    offset: u64,
}

impl TailSink for FileSink {
    fn append(&mut self, bytes: &[u8]) -> std::io::Result<u64> {
        let at = self.offset;
        self.file.write_all(bytes)?;
        self.offset += bytes.len() as u64;
        Ok(at)
    }
}

fn scratch(name: &str) -> PathBuf {
    let mut path = std::env::temp_dir();
    path.push(format!(
        "aristide-stream-test-{}-{name}.bin",
        std::process::id()
    ));
    path
}

/// A pipe with a long, gently decaying tail: 4 s at 48 kHz, so its
/// stored region is many times a slot's ring and every wrap is
/// exercised. `quantize` picks the residency the sample is played from.
fn tailed_sample(period: usize, quantize: bool) -> Sample {
    let omega = std::f64::consts::TAU / period as f64;
    let loop_start = period * 8;
    let loop_end = period * 40;
    let frames = loop_end + 48_000 * 4;
    let data: Vec<f32> = (0..frames)
        .map(|n| {
            let envelope = if n >= loop_end {
                (-((n - loop_end) as f64) / 48_000.0 / 1.5).exp()
            } else {
                1.0
            };
            (0.5 * envelope * (omega * n as f64).sin()) as f32
        })
        .collect();
    let mut sample = Sample::new(
        data,
        1,
        48_000.0,
        Some((loop_start as u64, loop_end as u64)),
        loop_end as u64,
    )
    .expect("valid");
    sample.align_release(48_000.0 / period as f32);
    if quantize {
        sample.quantize_i16();
    }
    sample
}

/// The same sample twice: resident, and split so its tail lives in
/// `store`. Both banks are otherwise identical.
fn banks(name: &str, period: usize, quantize: bool) -> (Arc<SampleBank>, Arc<SampleBank>) {
    let resident = tailed_sample(period, quantize);
    let mut streamed = tailed_sample(period, quantize);
    let path = scratch(name);
    let mut sink = FileSink {
        file: File::create(&path).expect("scratch file"),
        offset: 0,
    };
    assert!(
        streamed.offload_tail(0, &mut sink).expect("offload"),
        "the sample has a tail worth streaming"
    );
    drop(sink);
    let mut stores = crate::stream::StreamStores::new();
    stores.push(File::open(&path).expect("reopen"));
    let _ = std::fs::remove_file(&path);

    let mut resident_bank = SampleBank::default();
    resident_bank.push(resident);
    let mut streamed_bank = SampleBank::default();
    streamed_bank.push(streamed);
    streamed_bank.set_stores(Arc::new(stores));
    (Arc::new(resident_bank), Arc::new(streamed_bank))
}

fn start(handle: &mut EngineHandle, voice: u64) {
    handle.send(Command::StartVoice {
        handle: voice,
        sample: 0,
        rate: 1.0,
        gain: 1.0,
        group: 0,
        wind_weight: 0.0,
        brightness: 0.0,
        enclosure: ENCLOSURE_NONE,
        bus: 0,
        delay_frames: 0,
        nominal_hz: 0.0,
    });
}

/// Render `blocks` blocks of `frames`, pumping the streamer between
/// them so the test is deterministic — no threads, no sleeping.
fn render_streaming(
    engine: &mut Engine,
    workers: &mut [crate::stream::StreamWorker],
    blocks: usize,
    frames: usize,
    release_at: Option<usize>,
    handle: &mut EngineHandle,
) -> Vec<f32> {
    let mut out = Vec::with_capacity(blocks * frames * 2);
    for block in 0..blocks {
        if Some(block) == release_at {
            handle.send(Command::StopVoice { handle: 1 });
        }
        // Ten passes is far more than one block's consumption; the
        // rings are always full when the audio thread reads them.
        for _ in 0..10 {
            for worker in workers.iter_mut() {
                worker.poll_once();
            }
        }
        out.extend_from_slice(&render(engine, frames));
    }
    out
}

type StreamedEngine = (Engine, EngineHandle, Vec<crate::stream::StreamWorker>);

/// An engine on a streamed bank, with the workers left in the test's
/// hands so it can pump them deterministically.
fn engine_on(bank: Arc<SampleBank>, slots: usize) -> StreamedEngine {
    let (mut engine, handle) = Engine::new(48_000.0, Arc::clone(&bank));
    engine.set_release_stagger(0.0);
    let mut workers = Vec::new();
    let counters = StreamCounters::default();
    if let Some((rt, built)) = crate::stream::attach(&bank, slots, 1, counters) {
        engine.set_streams(rt);
        workers = built;
    }
    (engine, handle, workers)
}

/// The decisive test: a note held, released, and rung out to silence
/// sounds *bit for bit* the same whether its tail came out of RAM or
/// off a disk. Same kernels, same window contents — the only thing
/// streaming changes is where the bytes were a moment earlier.
#[test]
fn streamed_tail_is_bit_identical_to_a_resident_one() {
    for (name, quantize) in [("f32", false), ("i16", true)] {
        let (resident, streamed) = banks(name, 109, quantize);
        let (mut a, mut handle_a) = Engine::new(48_000.0, resident);
        a.set_release_stagger(0.0);
        start(&mut handle_a, 1);
        let mut expected = Vec::new();
        for block in 0..80 {
            if block == 4 {
                handle_a.send(Command::StopVoice { handle: 1 });
            }
            expected.extend_from_slice(&render(&mut a, 1024));
        }

        let (mut b, mut handle_b, mut workers) = engine_on(streamed, 8);
        assert!(!workers.is_empty(), "the streamed bank got a pool");
        start(&mut handle_b, 1);
        let actual = render_streaming(&mut b, &mut workers, 80, 1024, Some(4), &mut handle_b);

        assert_eq!(expected.len(), actual.len());
        let worst = expected
            .iter()
            .zip(&actual)
            .map(|(a, b)| (a - b).abs())
            .fold(0.0f32, f32::max);
        assert_eq!(worst, 0.0, "{name}: streamed tail differs by {worst:e}");
        // The comparison is only worth anything if a tail was actually
        // played (and it must reach silence).
        let energy: f32 = expected[8 * 1024..].iter().map(|v| v.abs()).sum();
        assert!(energy > 1.0, "{name}: no tail was rendered at all");
    }
}

/// Staccato and mass releases: eight voices started and released in
/// quick succession, streamed against resident. Same audio, and the
/// slots all come back afterwards.
#[test]
fn mass_release_streams_identically_and_returns_its_slots() {
    let (resident, streamed) = banks("mass", 73, true);
    let mut expected = Vec::new();
    {
        let (mut engine, mut handle) = Engine::new(48_000.0, resident);
        engine.set_release_stagger(0.0);
        for voice in 1..=8u64 {
            start(&mut handle, voice);
        }
        for block in 0..60 {
            if block == 2 {
                for voice in 1..=8u64 {
                    handle.send(Command::StopVoice { handle: voice });
                }
            }
            expected.extend_from_slice(&render(&mut engine, 512));
        }
    }

    let (mut engine, mut handle, mut workers) = engine_on(streamed, 16);
    for voice in 1..=8u64 {
        start(&mut handle, voice);
    }
    let mut actual = Vec::new();
    for block in 0..60 {
        if block == 2 {
            for voice in 1..=8u64 {
                handle.send(Command::StopVoice { handle: voice });
            }
        }
        for _ in 0..10 {
            for worker in workers.iter_mut() {
                worker.poll_once();
            }
        }
        actual.extend_from_slice(&render(&mut engine, 512));
    }
    let worst = expected
        .iter()
        .zip(&actual)
        .map(|(a, b)| (a - b).abs())
        .fold(0.0f32, f32::max);
    assert_eq!(worst, 0.0, "mass release differs by {worst:e}");

    // Ring the four-second tails out; every slot must then be back in
    // the pool (the worker acks the stop, the engine reclaims it on
    // the next block).
    for _ in 0..420 {
        for worker in workers.iter_mut() {
            worker.poll_once();
        }
        let _ = render(&mut engine, 512);
    }
    assert_eq!(
        engine.stream_slots_active(),
        Some(0),
        "slots leaked after the voices ended"
    );
}

/// A streamer that never delivers: the voice must fade, not click. The
/// discontinuity scan is the same idiom the release tests use — a click
/// is a jump between neighbouring output samples far bigger than the
/// waveform's own slope.
#[test]
fn an_underrun_fades_instead_of_clicking() {
    let (_, streamed) = banks("dry", 109, true);
    let (mut engine, mut handle, mut workers) = engine_on(streamed, 8);
    start(&mut handle, 1);
    // Prime nothing: the rings stay empty for the whole render, so the
    // voice runs out the moment it leaves its resident head.
    let mut out = Vec::new();
    for block in 0..60 {
        if block == 4 {
            handle.send(Command::StopVoice { handle: 1 });
        }
        out.extend_from_slice(&render(&mut engine, 1024));
    }
    let _ = &mut workers;

    let mono: Vec<f32> = out.chunks(2).map(|f| f[0]).collect();
    // The loop's own slope at 440-ish Hz, generously bounded.
    let mut worst_jump = 0.0f32;
    for pair in mono.windows(2) {
        worst_jump = worst_jump.max((pair[1] - pair[0]).abs());
    }
    assert!(
        worst_jump < 0.05,
        "an underrun clicked: worst jump {worst_jump:.4}"
    );
    let tail: f32 = mono[mono.len() - 4096..].iter().map(|v| v.abs()).sum();
    assert!(tail < 1e-3, "the starved voice never went silent ({tail:e})");
}

/// One slot, four releases: three of them find the pool empty. They
/// play their resident head and the EOF guard fades them out — quieter
/// than they should be, but never a click, and never a stuck voice.
#[test]
fn slot_exhaustion_degrades_gracefully() {
    let (_, streamed) = banks("exhaust", 73, true);
    let (mut engine, mut handle, mut workers) = engine_on(streamed, 1);
    for voice in 1..=4u64 {
        start(&mut handle, voice);
    }
    let mut out = Vec::new();
    // Long enough that every tail — streamed or truncated — has run out.
    for block in 0..420 {
        if block == 2 {
            for voice in 1..=4u64 {
                handle.send(Command::StopVoice { handle: voice });
            }
        }
        for _ in 0..10 {
            for worker in workers.iter_mut() {
                worker.poll_once();
            }
        }
        out.extend_from_slice(&render(&mut engine, 512));
    }
    let mono: Vec<f32> = out.chunks(2).map(|f| f[0]).collect();
    let mut worst_jump = 0.0f32;
    for pair in mono.windows(2) {
        worst_jump = worst_jump.max((pair[1] - pair[0]).abs());
    }
    assert!(
        worst_jump < 0.2,
        "slot exhaustion clicked: worst jump {worst_jump:.4}"
    );
    let tail: f32 = mono[mono.len() - 4096..].iter().map(|v| v.abs()).sum();
    assert!(tail < 1e-3, "voices left sounding after exhaustion ({tail:e})");
    assert_eq!(engine.stream_slots_active(), Some(0), "the slot came back");
}

/// Manual bench: what a streamed tail costs the audio thread against a
/// resident one. The streamer's own work is deliberately outside the
/// timed region — on a real machine it happens on another core; what
/// the callback pays is the window bookkeeping and the copies out of
/// the ring.
///
/// `cargo test -p aristide-engine bench_streamed_voice -- --ignored --nocapture`
#[test]
#[ignore = "manual bench"]
fn bench_streamed_voice_cost() {
    const VOICES: u64 = 64;
    const BLOCKS: usize = 400;
    const FRAMES: usize = 512;
    let (resident, streamed) = banks("bench", 109, true);

    let mut resident_engine = {
        let (mut engine, mut handle) = Engine::new(48_000.0, resident);
        engine.set_release_stagger(0.0);
        for voice in 1..=VOICES {
            start(&mut handle, voice);
        }
        let _ = render(&mut engine, FRAMES);
        for voice in 1..=VOICES {
            handle.send(Command::StopVoice { handle: voice });
        }
        // Two blocks to get every voice into its tail.
        let _ = render(&mut engine, FRAMES);
        let _ = render(&mut engine, FRAMES);
        engine
    };
    let started = std::time::Instant::now();
    for _ in 0..BLOCKS {
        let _ = render(&mut resident_engine, FRAMES);
    }
    let resident_ns = started.elapsed().as_nanos() as f64;

    let (mut engine, mut handle, mut workers) = engine_on(streamed, 128);
    for voice in 1..=VOICES {
        start(&mut handle, voice);
    }
    let _ = render(&mut engine, FRAMES);
    for voice in 1..=VOICES {
        handle.send(Command::StopVoice { handle: voice });
    }
    for _ in 0..2 {
        for _ in 0..20 {
            for worker in workers.iter_mut() {
                worker.poll_once();
            }
        }
        let _ = render(&mut engine, FRAMES);
    }
    let mut streamed_ns = 0.0f64;
    for _ in 0..BLOCKS {
        for _ in 0..4 {
            for worker in workers.iter_mut() {
                worker.poll_once();
            }
        }
        let block = std::time::Instant::now();
        let _ = render(&mut engine, FRAMES);
        streamed_ns += block.elapsed().as_nanos() as f64;
    }

    let per = |total: f64| total / (BLOCKS * FRAMES * VOICES as usize) as f64;
    println!(
        "tail voice cost: resident {:.2} ns/frame/voice, streamed {:.2} ns/frame/voice \
         (+{:.0}%)",
        per(resident_ns),
        per(streamed_ns),
        100.0 * (streamed_ns / resident_ns - 1.0)
    );
}
