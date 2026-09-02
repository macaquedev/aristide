//! Wind supply, tremulant, brightness and flow noise.

use super::*;

#[test]
fn wind_pressure_sags_pitch_under_load() {
    let period = 480usize;

    let run = |phantom_voices: usize| -> f64 {
        let (mut engine, mut handle) = Engine::new(48000.0, sine_pipe_bank(period, true));
        engine.set_release_stagger(0.0);
        // Phantoms draw wind but are silent: gain 0, weight 1.
        for i in 0..phantom_voices {
            handle.send(Command::StartVoice {
                handle: 100 + i as u64,
                sample: 0,
                rate: 1.0,
                gain: 0.0,
                group: 3,
                wind_weight: 1.0,
                brightness: 0.0,
                voicing_tilt: 1.0,
                enclosures: [ENCLOSURE_NONE; MAX_VOICE_ENCLOSURES],
        bus: 0,
        delay_frames: 0,
                nominal_hz: 0.0,
            });
        }
        handle.send(Command::StartVoice {
            handle: 1,
            sample: 0,
            rate: 1.0,
            gain: 1.0,
            group: 3,
            wind_weight: 0.0,
            brightness: 0.0,
            voicing_tilt: 1.0,
            enclosures: [ENCLOSURE_NONE; MAX_VOICE_ENCLOSURES],
        bus: 0,
        delay_frames: 0,
            nominal_hz: 0.0,
        });
        // Settle for ~1.5 s (12+ time constants), then measure.
        let mut buffer = vec![0.0f32; 1024 * 2];
        for _ in 0..70 {
            engine.process(&mut buffer, 2);
        }
        if phantom_voices > 0 {
            let pressure = engine.wind_pressure(3);
            assert!(
                (pressure - 0.94).abs() < 0.004,
                "steady pressure {pressure} should be ~0.94 at reference demand"
            );
        }
        let mut buffer = vec![0.0f32; 8192 * 2];
        engine.process(&mut buffer, 2);
        measured_period(&buffer)
    };

    let unloaded = run(0);
    // 30 phantoms × weight 1.0 = the default reference demand.
    let loaded = run(30);
    assert!(
        (unloaded - period as f64).abs() < 0.3,
        "unloaded period {unloaded} should be ~{period}"
    );
    // Expected: P=0.94 (realistic 6% chest drop), rate factor
    // 0.94^0.032 ≈ 0.99802 → ~481.0 (≈ −3.4 cents steady: the
    // physically calibrated sensitivity of ~0.55 cents per 1%).
    assert!(
        loaded > 480.5 && loaded < 481.5,
        "loaded period {loaded} should sag to ~481 frames"
    );
}

/// Releasing a chord closes its pallets: demand drops at key-off,
/// not when the tails finally die, so pressure starts recovering
/// while the tails are still sounding.
#[test]
fn released_pipes_stop_drawing_wind() {
    let period = 480usize;
    let (mut engine, mut handle) = Engine::new(48000.0, sine_pipe_bank(period, true));
    engine.set_release_stagger(0.0);
    for i in 0..30u64 {
        handle.send(Command::StartVoice {
            handle: 100 + i,
            sample: 0,
            rate: 1.0,
            gain: 1.0,
            group: 3,
            wind_weight: 1.0,
            brightness: 0.0,
            voicing_tilt: 1.0,
            enclosures: [ENCLOSURE_NONE; MAX_VOICE_ENCLOSURES],
            bus: 0,
            delay_frames: 0,
            nominal_hz: 0.0,
        });
    }
    let mut buffer = vec![0.0f32; 1024 * 2];
    for _ in 0..70 {
        engine.process(&mut buffer, 2);
    }
    let sagged = engine.wind_pressure(3);
    assert!(
        (sagged - 0.94).abs() < 0.004,
        "steady pressure {sagged} should be ~0.94 under the chord"
    );
    for i in 0..30u64 {
        handle.send(Command::StopVoice { handle: 100 + i });
    }
    // ~107 ms after key-off: the ~90 ms crossfades plus ~120 ms of
    // release material keep the tails sounding, but the pallets are
    // closed — pressure must already be well on its way back up.
    for _ in 0..5 {
        engine.process(&mut buffer, 2);
    }
    let recovering = engine.wind_pressure(3);
    assert!(
        recovering > 0.955,
        "pressure {recovering} should recover while tails still ring"
    );
    assert!(
        buffer.iter().any(|&v| v.abs() > 1e-4),
        "tails already silent — recovery assertion is vacuous"
    );
}

#[test]
fn brightness_tilt_attenuates_highs_under_load() {
    // A 1 kHz pipe with its tilt hinged at 200 Hz: nearly all of its
    // energy sits in the "upper partials" band, so the chest's
    // brightness factor acts on it almost directly.
    let period = 48; // 1 kHz at 48 kHz
    let tilt_a = 1.0 - (-std::f64::consts::TAU * 200.0 / 48000.0).exp() as f32;

    let run = |loaded: bool, brightness: f32| -> f32 {
        let (mut engine, mut handle) = Engine::new(48000.0, sine_pipe_bank(period, true));
        engine.set_release_stagger(0.0);
        // Kill per-voice noise so the comparison is deterministic.
        let mut params = wind::WindParams::default();
        params.flow_noise = 0.0;
        for group in 0..wind::MAX_WIND_GROUPS as u8 {
            handle.send(Command::SetWind { group, params });
        }
        if loaded {
            for i in 0..30 {
                handle.send(Command::StartVoice {
                    handle: 100 + i,
                    sample: 0,
                    rate: 1.0,
                    gain: 0.0,
                    group: 0,
                    wind_weight: 1.0,
                    brightness: 0.0,
                    voicing_tilt: 1.0,
                    enclosures: [ENCLOSURE_NONE; MAX_VOICE_ENCLOSURES],
        bus: 0,
        delay_frames: 0,
                    nominal_hz: 0.0,
                });
            }
        }
        handle.send(Command::StartVoice {
            handle: 1,
            sample: 0,
            rate: 1.0,
            gain: 1.0,
            group: 0,
            wind_weight: 0.0,
            brightness,
            voicing_tilt: 1.0,
            enclosures: [ENCLOSURE_NONE; MAX_VOICE_ENCLOSURES],
        bus: 0,
        delay_frames: 0,
            nominal_hz: 0.0,
        });
        let mut buffer = vec![0.0f32; 1024 * 2];
        for _ in 0..70 {
            engine.process(&mut buffer, 2);
        }
        let mut buffer = vec![0.0f32; 8192 * 2];
        engine.process(&mut buffer, 2);
        let mono: Vec<f32> = buffer.chunks(2).map(|f| f[0]).collect();
        rms(&mono)
    };

    let unloaded = run(false, tilt_a);
    let loaded_tilted = run(true, tilt_a);
    let loaded_flat = run(true, 0.0);
    // Gain factor alone: 0.94^0.75 ≈ 0.955. With the tilt, a further
    // ≈ 0.94^3 ≈ 0.83 on this (high) pipe.
    let plain = loaded_flat / unloaded;
    let tilted = loaded_tilted / unloaded;
    assert!(
        (plain - 0.955).abs() < 0.02,
        "plain gain ratio {plain} should be ~0.955"
    );
    assert!(
        tilted < plain * 0.88 && tilted > plain * 0.75,
        "tilted ratio {tilted} should add ~0.83x on top of {plain}"
    );
}

#[test]
fn flow_noise_wobbles_pitch_slightly_and_independently() {
    let period = 480;
    let measure_spread = |noise: f32| -> f64 {
        let (mut engine, mut handle) = Engine::new(48000.0, sine_pipe_bank(period, true));
        engine.set_release_stagger(0.0);
        let mut params = wind::WindParams::default();
        params.sag_depth = 0.0; // isolate the per-voice noise
        params.flow_noise = noise;
        handle.send(Command::SetWind { group: 0, params });
        handle.send(Command::StartVoice {
            handle: 1,
            sample: 0,
            rate: 1.0,
            gain: 1.0,
            group: 0,
            wind_weight: 1.0,
            brightness: 0.0,
            voicing_tilt: 1.0,
            enclosures: [ENCLOSURE_NONE; MAX_VOICE_ENCLOSURES],
        bus: 0,
        delay_frames: 0,
            nominal_hz: 0.0,
        });
        // 10 windows of 0.2 s: the wander drifts across them.
        let mut periods = Vec::new();
        let mut buffer = vec![0.0f32; 9600 * 2];
        for _ in 0..10 {
            engine.process(&mut buffer, 2);
            periods.push(measured_period(&buffer));
        }
        let min = periods.iter().cloned().fold(f64::MAX, f64::min);
        let max = periods.iter().cloned().fold(f64::MIN, f64::max);
        max - min
    };

    let quiet = measure_spread(0.0);
    let noisy = measure_spread(0.05);
    assert!(quiet < 0.05, "no noise → no drift, got {quiet}");
    assert!(
        noisy > 0.15,
        "5% flow noise should visibly wander pitch, got {noisy}"
    );
}

/// A tremulant is not one LFO on the master fader: each pipe
/// answers the chest through its own speech dynamics. A small pipe
/// follows the valve cycle for cycle; a bass, whose amplitude time
/// constant spans many tremulant periods, barely flutters.
#[test]
fn tremulant_moves_small_pipes_more_than_basses() {
    let depth_at = |nominal_hz: f32| -> f32 {
        let (mut engine, mut handle) = Engine::new(44_100.0, two_tone_bank());
        handle.send(Command::SetTremulant {
            group: 0,
            engaged: true,
        });
        handle.send(Command::StartVoice {
            handle: 7,
            sample: 0,
            rate: 1.0,
            gain: 1.0,
            group: 0,
            wind_weight: 1.0,
            brightness: 0.0,
            voicing_tilt: 1.0,
            enclosures: [ENCLOSURE_NONE; MAX_VOICE_ENCLOSURES],
            bus: 0,
            delay_frames: 0,
            nominal_hz,
        });
        let sr = 44_100usize;
        render(&mut engine, 2 * sr); // engage ramp + speech settle
        let out = render(&mut engine, 2 * sr);
        // Amplitude undulation depth from 50 ms RMS windows.
        let window = sr / 20 * 2;
        let levels: Vec<f32> = out
            .chunks(window)
            .map(|chunk| {
                (chunk.iter().map(|v| v * v).sum::<f32>() / chunk.len() as f32).sqrt()
            })
            .collect();
        let max = levels.iter().cloned().fold(f32::MIN, f32::max);
        let min = levels.iter().cloned().fold(f32::MAX, f32::min);
        (max - min) / (max + min)
    };
    let bass = depth_at(50.0);
    let small = depth_at(2000.0);
    assert!(
        small > 2.0 * bass,
        "speech dynamics missing: bass undulates {bass}, small pipe {small}"
    );
    assert!(
        small > 0.02,
        "the small pipe should audibly undulate: depth {small}"
    );
}

/// Same rule for the wind side: the pallet is closed at key-off,
/// so a tremulant engaged afterwards must not modulate the tail
/// (bit-identical to a run where the trem never engages).
#[test]
fn release_tail_ignores_later_tremulant() {
    let run = |trem_after_release: bool| -> Vec<f32> {
        let (mut engine, mut handle) = Engine::new(44_100.0, two_tone_bank());
        engine.set_release_stagger(0.0);
        handle.send(Command::StartVoice {
            handle: 1,
            sample: 0,
            rate: 1.0,
            gain: 1.0,
            group: 0,
            wind_weight: 0.0,
            brightness: 0.0,
            voicing_tilt: 1.0,
            enclosures: [ENCLOSURE_NONE; MAX_VOICE_ENCLOSURES],
            bus: 0,
            delay_frames: 0,
            nominal_hz: 0.0,
        });
        let sr = 44_100usize;
        render(&mut engine, sr / 2);
        handle.send(Command::StopVoice { handle: 1 });
        render(&mut engine, sr / 10);
        if trem_after_release {
            handle.send(Command::SetTremulant {
                group: 0,
                engaged: true,
            });
        }
        render(&mut engine, sr / 2)
    };
    let calm_tail = run(false);
    let trem_tail = run(true);
    assert!(
        calm_tail
            .iter()
            .zip(&trem_tail)
            .all(|(a, b)| (a - b).abs() < 1e-7),
        "tail changed after key-off tremulant engage"
    );
    assert!(calm_tail.iter().any(|&v| v.abs() > 1e-4), "tail silent");
}
