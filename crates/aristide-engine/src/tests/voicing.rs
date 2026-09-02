//! The voicer's own legs: a static treble tilt stamped at StartVoice,
//! and a live re-voicing of a sounding pipe.

use super::*;

/// The tilt is a one-pole high shelf: unity below the hinge, the asked
/// factor above it. Hinge a 1 kHz tone's filter far below the tone and
/// the whole tone sits on the shelf's flat top, so its rendered level
/// moves by exactly the dB asked for — measured, not derived.
fn tilted_level(brightness_db: f32) -> f32 {
    let period = 48; // 1 kHz at 48 kHz
    let hinge = 1.0 - (-std::f64::consts::TAU * 25.0 / 48000.0).exp() as f32;
    let (mut engine, mut handle) = Engine::new(48000.0, sine_pipe_bank(period, true));
    engine.set_release_stagger(0.0);
    // No wind modulation: the tilt under test must be the only thing
    // moving the treble.
    let mut params = wind::WindParams::default();
    params.sag_depth = 0.0;
    params.flow_noise = 0.0;
    for group in 0..wind::MAX_WIND_GROUPS as u8 {
        handle.send(Command::SetWind { group, params });
    }
    handle.send(Command::StartVoice {
        handle: 1,
        sample: 0,
        rate: 1.0,
        gain: 1.0,
        group: 0,
        wind_weight: 0.0,
        brightness: hinge,
        voicing_tilt: 10f32.powf(brightness_db / 20.0),
        enclosures: [ENCLOSURE_NONE; MAX_VOICE_ENCLOSURES],
        bus: 0,
        delay_frames: 0,
        nominal_hz: 1000.0,
    });
    let mut buffer = vec![0.0f32; 4096 * 2];
    for _ in 0..4 {
        engine.process(&mut buffer, 2);
    }
    let mono: Vec<f32> = buffer.chunks(2).map(|f| f[0]).collect();
    rms(&mono)
}

#[test]
fn the_voicing_tilt_measures_the_decibels_it_was_asked_for() {
    let flat = tilted_level(0.0);
    assert!(flat > 0.01, "the reference render is silent: {flat}");
    for asked in [-6.0f32, -3.0, 3.0, 6.0] {
        let measured = 20.0 * (tilted_level(asked) / flat).log10();
        assert!(
            (measured - asked).abs() < 0.5,
            "asked {asked} dB, measured {measured:.2} dB"
        );
    }
}

/// 0 dB must not merely be close — the filter must not run at all. A
/// voicing leg that costs a rounding error on every untouched pipe is
/// a leg that has to be argued for, so the bypass gate is the claim
/// under test: with a steady chest and a flat tilt, a voice with a
/// tilt filter configured renders the SAME BITS as one without.
#[test]
fn a_flat_tilt_bypasses_the_filter_bit_for_bit() {
    let render_one = |brightness: f32, tilt: f32| -> Vec<u32> {
        let (mut engine, mut handle) = Engine::new(48000.0, sine_pipe_bank(48, true));
        engine.set_release_stagger(0.0);
        let mut params = wind::WindParams::default();
        params.sag_depth = 0.0;
        params.flow_noise = 0.0;
        for group in 0..wind::MAX_WIND_GROUPS as u8 {
            handle.send(Command::SetWind { group, params });
        }
        handle.send(Command::StartVoice {
            handle: 1,
            sample: 0,
            rate: 1.0,
            gain: 0.7,
            group: 0,
            wind_weight: 1.0,
            brightness,
            voicing_tilt: tilt,
            enclosures: [ENCLOSURE_NONE; MAX_VOICE_ENCLOSURES],
            bus: 0,
            delay_frames: 0,
            nominal_hz: 1000.0,
        });
        let mut buffer = vec![0.0f32; 4096 * 2];
        engine.process(&mut buffer, 2);
        buffer.iter().map(|v| v.to_bits()).collect()
    };
    assert_eq!(
        render_one(0.2, 1.0),
        render_one(0.0, 1.0),
        "a flat tilt still ran the filter"
    );
    assert_ne!(
        render_one(0.2, 0.5),
        render_one(0.0, 1.0),
        "a real tilt did nothing"
    );
}

/// A live voicing edit lands on the pipe that is already speaking —
/// and lands as a fade, not a step: the per-frame jump the level
/// change makes must stay far below the change itself.
#[test]
fn a_live_trim_moves_a_held_voice_without_a_step() {
    // A long ramp sample: the output rises smoothly frame by frame, so
    // any discontinuity in the render is the trim's doing and nothing
    // else's.
    let run = |trim: Option<f32>| -> Vec<f32> {
        let (mut engine, mut handle) = Engine::new(48000.0, ramp_bank(48000));
        let mut params = wind::WindParams::default();
        params.sag_depth = 0.0;
        params.flow_noise = 0.0;
        for group in 0..wind::MAX_WIND_GROUPS as u8 {
            handle.send(Command::SetWind { group, params });
        }
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
            nominal_hz: 100.0,
        });
        render(&mut engine, 512);
        if let Some(gain) = trim {
            handle.send(Command::SetVoiceTrim {
                handle: 1,
                gain,
                tilt: 1.0,
            });
        }
        // 4608 frames ≈ 96 ms: many time constants of the ~5 ms ramp.
        render(&mut engine, 4608).chunks(2).map(|f| f[0]).collect()
    };
    let flat = run(None);
    let trimmed = run(Some(0.5));

    // It arrives, and it arrives at exactly −6 dB.
    let settled = trimmed[4500] / flat[4500];
    assert!(
        (settled - 0.5).abs() < 0.002,
        "settled at {settled}, expected 0.5"
    );

    // And it gets there as a fade: a hard switch would put a half-
    // amplitude cliff in one frame. The ramp's own worst step must
    // stay in the same order as the signal's natural rise.
    let step = |signal: &[f32]| {
        signal
            .windows(2)
            .map(|w| (w[1] - w[0]).abs())
            .fold(0.0f32, f32::max)
    };
    let natural = step(&flat);
    assert!(
        step(&trimmed) < 4.0 * natural,
        "the trim stepped by {} against a natural {natural}",
        step(&trimmed)
    );
}

/// A released voice is room decay that already left the pipe: voicing
/// it must do nothing, exactly as a shutter move or a rate glide does
/// nothing to a tail.
#[test]
fn a_released_voice_ignores_a_trim() {
    let render_tail = |trim: bool| -> Vec<u32> {
        let (mut engine, mut handle) = Engine::new(48000.0, sine_pipe_bank(48, true));
        engine.set_release_stagger(0.0);
        handle.send(Command::StartVoice {
            handle: 1,
            sample: 0,
            rate: 1.0,
            gain: 1.0,
            group: 0,
            wind_weight: 0.0,
            brightness: 0.2,
            voicing_tilt: 1.0,
            enclosures: [ENCLOSURE_NONE; MAX_VOICE_ENCLOSURES],
            bus: 0,
            delay_frames: 0,
            nominal_hz: 1000.0,
        });
        render(&mut engine, 256);
        handle.send(Command::StopVoice { handle: 1 });
        render(&mut engine, 256);
        if trim {
            handle.send(Command::SetVoiceTrim {
                handle: 1,
                gain: 0.25,
                tilt: 0.5,
            });
        }
        render(&mut engine, 2048)
            .iter()
            .map(|v| v.to_bits())
            .collect()
    };
    assert_eq!(render_tail(true), render_tail(false), "the tail moved");
}
