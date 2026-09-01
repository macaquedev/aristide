//! Bit-exactness guard over the whole audio path.

use super::*;

// ---- bit-exactness guard ------------------------------------------
//
// A refactor of the audio path must not move a single bit. This
// renders a fixed command script against a fixed synthetic bank and
// hashes every output sample. The constants are the engine's
// recorded output: they are NOT to be "updated" to make a change
// pass — a differing hash means the audio changed.

fn fnv1a(hash: &mut u64, value: f32) {
    for byte in value.to_bits().to_le_bytes() {
        *hash ^= u64::from(byte);
        *hash = hash.wrapping_mul(0x0000_0100_0000_01B3);
    }
}

/// Four pipes with different anatomy: a multi-loop mono pipe with an
/// embedded tail AND a separate short-hold release, that release,
/// a stereo looped pipe, and a loop-less percussive knock.
fn golden_bank() -> Arc<SampleBank> {
    const PERIOD: usize = 120; // 400 Hz at 48 kHz
    let sine = |n: usize| (core::f64::consts::TAU * n as f64 / PERIOD as f64).sin();
    let loop_start = PERIOD * 4;
    let loop_end = PERIOD * 20;

    let data: Vec<f32> = (0..PERIOD * 40)
        .map(|n| {
            let envelope = if n >= loop_end {
                (-((n - loop_end) as f64) / 8000.0).exp()
            } else {
                (n as f64 / loop_start as f64).min(1.0)
            };
            (envelope * sine(n)) as f32
        })
        .collect();
    let mut pipe = Sample::new(
        data,
        1,
        48_000.0,
        Some((loop_start as u64, loop_end as u64)),
        loop_end as u64,
    )
    .expect("valid pipe");
    pipe.add_loop(
        (loop_start + PERIOD) as u64,
        (loop_end - PERIOD) as u64,
    )
    .expect("valid second loop");
    pipe.align_release(48_000.0 / PERIOD as f32);

    let release_data: Vec<f32> = (0..PERIOD * 10)
        .map(|n| (0.4 * (-(n as f64) / 3000.0).exp() * sine(n)) as f32)
        .collect();
    let release = Sample::new(release_data, 1, 48_000.0, None, 0).expect("valid release");
    pipe.attach_release(&release, 1, Some(400), None, 25);

    let stereo_data: Vec<f32> = (0..PERIOD * 30)
        .flat_map(|n| [(0.5 * sine(n)) as f32, (0.5 * sine(n + 7)) as f32])
        .collect();
    let stereo = Sample::new(
        stereo_data,
        2,
        48_000.0,
        Some(((PERIOD * 6) as u64, (PERIOD * 18) as u64)),
        (PERIOD * 18) as u64,
    )
    .expect("valid stereo pipe");

    let knock: Vec<f32> = (0..900)
        .map(|n| (0.6 * (-(n as f64) / 200.0).exp() * sine(n * 3)) as f32)
        .collect();
    let knock = Sample::new(knock, 1, 48_000.0, None, 0).expect("valid knock");

    let mut bank = SampleBank::default();
    assert_eq!(bank.push(pipe), 0);
    assert_eq!(bank.push(release), 1);
    assert_eq!(bank.push(stereo), 2);
    assert_eq!(bank.push(knock), 3);
    Arc::new(bank)
}

/// Renders the fixed script and returns (hash, peak).
fn golden_render(lite: bool) -> (u64, f32) {
    const BLOCKS: usize = 200;
    const FRAMES: usize = 256;
    let (mut engine, mut handle) = Engine::new(48_000.0, golden_bank());
    engine.set_lite(lite);
    let start = |handle_id: u64, sample, rate, gain, group, wind_weight, brightness,
                 enclosure, bus, delay_frames, nominal_hz| Command::StartVoice {
        handle: handle_id,
        sample,
        rate,
        gain,
        group,
        wind_weight,
        brightness,
        enclosure,
        bus,
        delay_frames,
        nominal_hz,
    };
    let mut hash = 0xCBF2_9CE4_8422_2325u64;
    let mut peak = 0.0f32;
    let mut buffer = vec![0.0f32; FRAMES * 2];
    for block in 0..BLOCKS {
        match block {
            0 => {
                handle.send(Command::SetMasterGain { linear: 0.5 });
                handle.send(Command::SetWind {
                    group: 0,
                    params: wind::WindParams {
                        sag_depth: 0.08,
                        natural_hz: 4.0,
                        flow_noise: 0.03,
                        ..wind::WindParams::default()
                    },
                });
                handle.send(Command::SetTremulantParams {
                    group: 0,
                    params: wind::TremulantParams {
                        rate_hz: 5.5,
                        depth: 0.05,
                        ..wind::TremulantParams::default()
                    },
                });
                handle.send(Command::SetEnclosure {
                    enclosure: 0,
                    params: enclosure::EnclosureParams {
                        full_sweep_s: 0.4,
                        ..enclosure::EnclosureParams::default()
                    },
                });
                handle.send(Command::SetBusDelay {
                    bus: 1,
                    params: routing::DelayParams {
                        seconds: 0.012,
                        feedback: 0.2,
                        mix: 0.35,
                        dry: 1.0,
                    },
                });
                handle.send(Command::SetBusOutput {
                    bus: 1,
                    left: 0,
                    right: 1,
                    gain: 0.8,
                });
            }
            1 => {
                handle.send(start(1, 0, 1.0, 0.8, 0, 1.0, 0.2, 0, 0, 0, 400.0));
                handle.send(start(2, 0, 0.5, 0.6, 0, 1.0, 0.1, ENCLOSURE_NONE, 1, 64, 200.0));
                handle.send(start(3, 2, 1.3, 0.4, 1, 0.5, 0.0, 0, 0, 0, 800.0));
                handle.send(start(4, 3, 1.0, 0.9, 0, 0.0, 0.0, ENCLOSURE_NONE, 0, 0, 0.0));
            }
            5 => {
                handle.send(Command::SetTremulant {
                    group: 0,
                    engaged: true,
                });
            }
            20 => {
                handle.send(Command::SetEnclosurePosition {
                    enclosure: 0,
                    position: 0.2,
                });
            }
            40 => {
                handle.send(Command::SetVoiceRate {
                    handle: 1,
                    rate: 1.02,
                    glide_ms: 120.0,
                });
            }
            60 => {
                handle.send(Command::StopVoice { handle: 4 });
            }
            80 => {
                handle.send(Command::SetWaveTremulant {
                    group: 0,
                    engaged: true,
                });
            }
            90 => {
                handle.send(Command::StopVoice { handle: 2 });
            }
            120 => {
                handle.send(Command::StopVoice { handle: 1 });
            }
            140 => {
                handle.send(Command::SetEnclosurePosition {
                    enclosure: 0,
                    position: 1.0,
                });
            }
            160 => {
                handle.send(Command::SetVoiceRate {
                    handle: 3,
                    rate: 1.0,
                    glide_ms: 0.0,
                });
            }
            180 => {
                handle.send(Command::KillVoice { handle: 3 });
            }
            _ => {}
        }
        buffer.fill(0.0);
        engine.process(&mut buffer, 2);
        for &value in buffer.iter() {
            fnv1a(&mut hash, value);
            peak = peak.max(value.abs());
        }
    }
    engine.assert_slot_invariants();
    (hash, peak)
}

/// Refactors of the audio path must be bit-exact.
#[test]
fn engine_output_is_bit_exact() {
    let (hash, peak) = golden_render(false);
    assert!(peak > 0.05, "script rendered near-silence ({peak})");
    assert_eq!(hash, GOLDEN_HASH, "engine output changed");
    let (lite_hash, lite_peak) = golden_render(true);
    assert!(lite_peak > 0.05, "lite script rendered near-silence ({lite_peak})");
    assert_eq!(lite_hash, GOLDEN_HASH_LITE, "lite-mode output changed");
}

const GOLDEN_HASH: u64 = 0x6AB5_6B42_A985_D428;
const GOLDEN_HASH_LITE: u64 = 0x9333_3E2B_59C2_F760;
