//! Engine tests, grouped by what they exercise. Fixtures shared by
//! more than one group live here; each submodule pulls them in with
//! `use super::*`.

use super::*;
use crate::bank::Sample;

mod allocation;
mod tone_voice;
mod playback;
mod release;
mod windchest;
mod swell;
mod buses;
mod golden;

/// 100-frame mono ramp 0..1, loop frames 20..=59, release tail at 60.
fn test_bank() -> Arc<SampleBank> {
    let data: Vec<f32> = (0..100).map(|i| i as f32 / 100.0).collect();
    let sample = Sample::new(data, 1, 100.0, Some((20, 60)), 60).expect("valid");
    let mut bank = SampleBank::default();
    bank.push(sample);
    Arc::new(bank)
}

fn render(engine: &mut Engine, frames: usize) -> Vec<f32> {
    let mut buffer = vec![0.0f32; frames * 2];
    engine.process(&mut buffer, 2);
    buffer
}

/// A long loop-less linear ramp: output value ≈ cursor position, so
/// a block's per-frame output delta measures the playback rate.
fn ramp_bank(frames: usize) -> Arc<SampleBank> {
    let data: Vec<f32> = (0..frames).map(|i| i as f32 / frames as f32).collect();
    let sample = Sample::new(data, 1, 100.0, None, 0).expect("valid");
    let mut bank = SampleBank::default();
    bank.push(sample);
    Arc::new(bank)
}

/// Mean per-frame slope of the left channel over a block's middle
/// (edges avoided: sinc taps clamp near the sample ends).
fn slope(block: &[f32]) -> f32 {
    let frames = block.len() / 2;
    let first = block[2 * 2];
    let last = block[(frames - 2) * 2];
    (last - first) / (frames - 4) as f32
}

/// A synthetic pipe: continuous sine through attack, loop, and a
/// gently decaying release tail, so waveform phase is knowable
/// everywhere. `aligned` controls whether the alignment table is
/// built (false = naive fixed splice, the M3 behaviour).
fn sine_pipe_bank(period: usize, aligned: bool) -> Arc<SampleBank> {
    let omega = std::f64::consts::TAU / period as f64;
    let loop_start = period * 4;
    let loop_end = period * 12;
    let frames = period * 24;
    let data: Vec<f32> = (0..frames)
        .map(|n| {
            let envelope = if n >= loop_end {
                1.0 - 0.5 * (n - loop_end) as f64 / (frames - loop_end) as f64
            } else {
                1.0
            };
            (envelope * (omega * n as f64).sin()) as f32
        })
        .collect();
    let mut sample = Sample::new(
        data,
        1,
        48000.0,
        Some((loop_start as u64, loop_end as u64)),
        loop_end as u64,
    )
    .expect("valid");
    if aligned {
        sample.align_release(48000.0 / period as f32);
        assert!(sample.release_alignment().is_some(), "alignment built");
    }
    let mut bank = SampleBank::default();
    bank.push(sample);
    Arc::new(bank)
}

/// A misaligned splice doesn't click through a 30 ms crossfade — it
/// *cancels*: output amplitude dips toward zero mid-fade. Measure
/// the worst period-length RMS during the crossfade relative to the
/// held level.
fn release_dip_ratio(bank: Arc<SampleBank>, stop_after: usize, period: usize) -> f32 {
    let (mut engine, mut handle) = Engine::new(48000.0, bank);
    engine.set_release_stagger(0.0);
    handle.send(Command::StartVoice {
        handle: 1,
        sample: 0,
        rate: 1.0,
        gain: 1.0,
        group: 0,
        wind_weight: 0.0,
        brightness: 0.0,
        enclosures: [ENCLOSURE_NONE; MAX_VOICE_ENCLOSURES],
        bus: 0,
        delay_frames: 0,
        nominal_hz: 0.0,
    });
    let mut buffer = vec![0.0f32; stop_after * 2];
    engine.process(&mut buffer, 2);
    let held: Vec<f32> = buffer.chunks(2).map(|f| f[0]).collect();
    let steady = rms(&held[held.len() - period..]);

    handle.send(Command::StopVoice { handle: 1 });
    // 30 ms crossfade = 1440 frames at 48 kHz.
    let mut buffer = vec![0.0f32; 1500 * 2];
    engine.process(&mut buffer, 2);
    let fade: Vec<f32> = buffer.chunks(2).map(|f| f[0]).collect();
    let mut worst = f32::MAX;
    let mut start = 0;
    while start + period <= fade.len() {
        worst = worst.min(rms(&fade[start..start + period]));
        start += period / 8;
    }
    worst / steady
}

fn rms(window: &[f32]) -> f32 {
    (window.iter().map(|v| v * v).sum::<f32>() / window.len() as f32).sqrt()
}

/// Mean rising-zero-crossing period of channel 0, sub-sample refined.
fn measured_period(buffer: &[f32]) -> f64 {
    let mono: Vec<f32> = buffer.chunks(2).map(|f| f[0]).collect();
    let mut crossings = Vec::new();
    for i in 1..mono.len() {
        if mono[i - 1] < 0.0 && mono[i] >= 0.0 {
            let t = (i - 1) as f64 + (-mono[i - 1] as f64) / ((mono[i] - mono[i - 1]) as f64);
            crossings.push(t);
        }
    }
    assert!(crossings.len() > 3, "not enough periods to measure");
    (crossings.last().unwrap() - crossings[0]) / (crossings.len() - 1) as f64
}

/// Two-tone fixture for enclosure tests: 100 Hz + 6 kHz at 44.1 kHz,
/// seamless loop (both periods divide the loop length... 100 Hz does
/// exactly; 6 kHz nearly — the sinc seam wrap absorbs the rest), and
/// a gently decaying tail past the loop for release tests.
fn two_tone_bank() -> Arc<SampleBank> {
    let sr = 44_100.0f64;
    let frames = 4 * 44_100usize;
    let loop_start = 441 * 10;
    let loop_end = 441 * 250;
    let data: Vec<f32> = (0..frames)
        .map(|n| {
            let t = n as f64 / sr;
            let envelope = if n >= loop_end {
                (-((n - loop_end) as f64) / sr / 0.8).exp()
            } else {
                1.0
            };
            (0.3 * (core::f64::consts::TAU * 100.0 * t).sin()
                + 0.3 * (core::f64::consts::TAU * 6_000.0 * t).sin())
                as f32
                * envelope as f32
        })
        .collect();
    let sample = Sample::new(
        data,
        1,
        sr as f32,
        Some((loop_start as u64, loop_end as u64)),
        loop_end as u64,
    )
    .expect("valid");
    let mut bank = SampleBank::default();
    bank.push(sample);
    Arc::new(bank)
}

/// Signal power at `freq` over `window` frames of channel 0
/// (quadrature correlation — windows hold whole cycles).
fn band_power(output: &[f32], skip: usize, window: usize, freq: f32, sr: f32) -> f64 {
    let mut sin_acc = 0.0f64;
    let mut cos_acc = 0.0f64;
    for i in 0..window {
        let phase = core::f64::consts::TAU * freq as f64 * (skip + i) as f64 / sr as f64;
        let v = output[(skip + i) * 2] as f64;
        sin_acc += v * phase.sin();
        cos_acc += v * phase.cos();
    }
    let norm = 2.0 / window as f64;
    (sin_acc * norm).powi(2) + (cos_acc * norm).powi(2)
}

fn enclosure_test_engine(full_sweep_s: f32) -> (Engine, EngineHandle) {
    let (mut engine, mut handle) = Engine::new(44_100.0, two_tone_bank());
    engine.set_release_stagger(0.0);
    handle.send(Command::SetEnclosure {
        enclosure: 0,
        params: enclosure::EnclosureParams {
            full_sweep_s,
            ..enclosure::EnclosureParams::default()
        },
    });
    handle.send(Command::StartVoice {
        handle: 1,
        sample: 0,
        rate: 1.0,
        gain: 1.0,
        group: 0,
        wind_weight: 0.0,
        brightness: 0.0,
        enclosures: [0, ENCLOSURE_NONE],
        bus: 0,
        delay_frames: 0,
        nominal_hz: 0.0,
    });
    (engine, handle)
}
