//! Enclosures: shutter inertia, shelf, frozen tails.

use super::*;

/// Closing the box must attenuate broadband by ~floor_db and the
/// high band by ~floor_db + shelf_db: a muffle, not a volume knob.
#[test]
fn closed_enclosure_attenuates_highs_more_than_lows() {
    let (mut engine, mut handle) = enclosure_test_engine(0.0);
    let sr = 44_100usize;
    let out_open = render(&mut engine, sr);
    handle.send(Command::SetEnclosurePosition {
        enclosure: 0,
        position: 0.0,
    });
    let out_closed = render(&mut engine, sr);

    // 0.2 s windows late in each second (filters settled), whole
    // cycles of both tones.
    let (skip, window) = (sr / 2, sr / 5);
    let low_db =
        10.0 * (band_power(&out_closed, skip, window, 100.0, sr as f32)
            / band_power(&out_open, skip, window, 100.0, sr as f32))
        .log10();
    let high_db =
        10.0 * (band_power(&out_closed, skip, window, 6_000.0, sr as f32)
            / band_power(&out_open, skip, window, 6_000.0, sr as f32))
        .log10();
    let p = enclosure::EnclosureParams::default();
    assert!(
        (low_db - p.floor_db as f64).abs() < 1.5,
        "low band moved {low_db:.1} dB, expected ~{}",
        p.floor_db
    );
    assert!(
        (high_db - (p.floor_db + p.shelf_db) as f64).abs() < 2.0,
        "high band moved {high_db:.1} dB, expected ~{}",
        p.floor_db + p.shelf_db
    );
}

/// A released voice's tail is room decay that already left the box:
/// shutter moves after key-off must not touch it (bit-identical to
/// a run where the pedal never moves).
#[test]
fn release_tail_ignores_later_shutter_moves() {
    let run = |close_after_release: bool| -> Vec<f32> {
        let (mut engine, mut handle) = enclosure_test_engine(0.0);
        let sr = 44_100usize;
        render(&mut engine, sr / 2);
        handle.send(Command::StopVoice { handle: 1 });
        render(&mut engine, sr / 10);
        if close_after_release {
            handle.send(Command::SetEnclosurePosition {
                enclosure: 0,
                position: 0.0,
            });
        }
        render(&mut engine, sr / 2)
    };
    let open_tail = run(false);
    let closed_tail = run(true);
    assert!(
        open_tail
            .iter()
            .zip(&closed_tail)
            .all(|(a, b)| (a - b).abs() < 1e-7),
        "tail changed after key-off shutter move"
    );
    // And the tail is actually sounding (the assertion above must
    // not pass vacuously on silence).
    assert!(open_tail.iter().any(|&v| v.abs() > 1e-4), "tail silent");
}

/// A full pedal sweep through the inertia model must not click:
/// sample-to-sample steps stay comparable to the steady signal's.
#[test]
fn pedal_sweep_is_click_free() {
    let (mut engine, mut handle) = enclosure_test_engine(0.3);
    let sr = 44_100usize;
    let steady = render(&mut engine, sr);
    handle.send(Command::SetEnclosurePosition {
        enclosure: 0,
        position: 0.0,
    });
    let sweep = render(&mut engine, sr);
    let max_step = |out: &[f32]| -> f32 {
        out.chunks(2)
            .map(|f| f[0])
            .collect::<Vec<_>>()
            .windows(2)
            .map(|w| (w[1] - w[0]).abs())
            .fold(0.0f32, f32::max)
    };
    let steady_step = max_step(&steady[sr..]);
    let sweep_step = max_step(&sweep);
    assert!(
        sweep_step < 1.3 * steady_step,
        "sweep steps {sweep_step} vs steady {steady_step}"
    );
    // The sweep actually closed the box.
    assert!(engine.enclosure_position(0) < 0.05);
}

// ---- nested boxes -------------------------------------------------
//
// Real instruments stand a box inside a box (a Solo or Echo box within
// the Swell is standard English and American practice). A pipe in the
// inner box is heard through BOTH shutter fronts.

/// Two boxes, one voice, shutters instant so the pedals can be treated
/// as positions rather than sweeps.
fn nested_test_engine() -> (Engine, EngineHandle) {
    let (mut engine, mut handle) = Engine::new(44_100.0, two_tone_bank());
    engine.set_release_stagger(0.0);
    for enclosure in [0u8, 1] {
        handle.send(Command::SetEnclosure {
            enclosure,
            params: enclosure::EnclosureParams {
                full_sweep_s: 0.0,
                ..enclosure::EnclosureParams::default()
            },
        });
    }
    handle.send(Command::StartVoice {
        handle: 1,
        sample: 0,
        rate: 1.0,
        gain: 1.0,
        group: 0,
        wind_weight: 0.0,
        brightness: 0.0,
        enclosures: [0, 1],
        bus: 0,
        delay_frames: 0,
        nominal_hz: 0.0,
    });
    (engine, handle)
}

/// Closing either box attenuates; closing both attenuates by the sum
/// in dB — the shutter fronts are in series, so their gains multiply
/// (GO composes a chest's enclosures the same way).
#[test]
fn nested_boxes_attenuate_in_series() {
    let sr = 44_100usize;
    let level = |inner: f32, outer: f32| -> f64 {
        let (mut engine, mut handle) = nested_test_engine();
        handle.send(Command::SetEnclosurePosition {
            enclosure: 0,
            position: inner,
        });
        handle.send(Command::SetEnclosurePosition {
            enclosure: 1,
            position: outer,
        });
        let out = render(&mut engine, sr);
        // Low band: below both corners, so this is the broadband floor
        // alone, uncontaminated by the shelf legs.
        band_power(&out, sr / 2, sr / 5, 100.0, sr as f32)
    };
    let open = level(1.0, 1.0);
    let inner_shut = 10.0 * (level(0.0, 1.0) / open).log10();
    let outer_shut = 10.0 * (level(1.0, 0.0) / open).log10();
    let both_shut = 10.0 * (level(0.0, 0.0) / open).log10();
    let floor = enclosure::EnclosureParams::default().floor_db as f64;
    assert!(
        (inner_shut - floor).abs() < 1.0 && (outer_shut - floor).abs() < 1.0,
        "one box shut: inner {inner_shut:.1} dB, outer {outer_shut:.1} dB, \
         expected ~{floor}"
    );
    assert!(
        (both_shut - 2.0 * floor).abs() < 1.5,
        "both shut: {both_shut:.1} dB, expected ~{}",
        2.0 * floor
    );
}

/// A voice sits only in the boxes it was born in: moving a box it does
/// not belong to must not move a single bit of its output.
#[test]
fn an_unrelated_box_never_touches_the_voice() {
    let run = |move_other: bool| -> Vec<f32> {
        let (mut engine, mut handle) = enclosure_test_engine(0.0);
        if move_other {
            handle.send(Command::SetEnclosurePosition {
                enclosure: 1,
                position: 0.0,
            });
        }
        render(&mut engine, 44_100 / 2)
    };
    let still = run(false);
    let moved = run(true);
    assert_eq!(still, moved, "a box the voice is not in changed its sound");
    assert!(still.iter().any(|&v| v.abs() > 1e-4), "rendered silence");
}

/// Both boxes freeze at key-off: a tail is room decay that already left
/// both of them, so neither pedal may touch it afterwards.
#[test]
fn nested_release_tails_ignore_later_shutter_moves() {
    let run = |close_after_release: bool| -> Vec<f32> {
        let (mut engine, mut handle) = nested_test_engine();
        let sr = 44_100usize;
        render(&mut engine, sr / 2);
        handle.send(Command::StopVoice { handle: 1 });
        render(&mut engine, sr / 10);
        if close_after_release {
            for enclosure in [0u8, 1] {
                handle.send(Command::SetEnclosurePosition {
                    enclosure,
                    position: 0.0,
                });
            }
        }
        render(&mut engine, sr / 2)
    };
    let open_tail = run(false);
    let closed_tail = run(true);
    assert!(
        open_tail
            .iter()
            .zip(&closed_tail)
            .all(|(a, b)| (a - b).abs() < 1e-7),
        "nested tail changed after key-off shutter moves"
    );
    assert!(open_tail.iter().any(|&v| v.abs() > 1e-4), "tail silent");
}

/// Two pedals sweeping at once must not click either: each box keeps
/// its own ~5 ms gain de-zipper.
#[test]
fn nested_pedal_sweep_is_click_free() {
    let (mut engine, mut handle) = Engine::new(44_100.0, two_tone_bank());
    engine.set_release_stagger(0.0);
    for enclosure in [0u8, 1] {
        handle.send(Command::SetEnclosure {
            enclosure,
            params: enclosure::EnclosureParams {
                full_sweep_s: 0.3,
                ..enclosure::EnclosureParams::default()
            },
        });
    }
    handle.send(Command::StartVoice {
        handle: 1,
        sample: 0,
        rate: 1.0,
        gain: 1.0,
        group: 0,
        wind_weight: 0.0,
        brightness: 0.0,
        enclosures: [0, 1],
        bus: 0,
        delay_frames: 0,
        nominal_hz: 0.0,
    });
    let sr = 44_100usize;
    let steady = render(&mut engine, sr);
    for enclosure in [0u8, 1] {
        handle.send(Command::SetEnclosurePosition {
            enclosure,
            position: 0.0,
        });
    }
    let sweep = render(&mut engine, sr);
    let max_step = |out: &[f32]| -> f32 {
        out.chunks(2)
            .map(|f| f[0])
            .collect::<Vec<_>>()
            .windows(2)
            .map(|w| (w[1] - w[0]).abs())
            .fold(0.0f32, f32::max)
    };
    assert!(
        max_step(&sweep) < 1.3 * max_step(&steady[sr..]),
        "sweep steps {} vs steady {}",
        max_step(&sweep),
        max_step(&steady[sr..])
    );
    assert!(engine.enclosure_position(0) < 0.05 && engine.enclosure_position(1) < 0.05);
}

// ---- closed-box pressure rise -------------------------------------
//
// The box is the volume its pipes exhaust into: shut the shutters and
// their own outflow pressurizes it, which is a pressure LOSS for every
// pipe inside (a pipe speaks on chest minus mouth). See
// `enclosure::Enclosure::step` for the derivation.

/// A 400 Hz pipe whose pitch can be measured off zero crossings, in one
/// box, carrying the wind draw of a full chorus: one sounding voice
/// plus two silent load voices. The chest regulator is disabled so the
/// ONLY pressure the voice feels is the box's.
fn pressure_test_engine(
    rise_pct: f32,
    boxes: [u8; MAX_VOICE_ENCLOSURES],
    position: f32,
) -> (Engine, EngineHandle) {
    const PERIOD: usize = 120; // 400 Hz at 48 kHz
    let (mut engine, mut handle) = Engine::new(48_000.0, sine_pipe_bank(PERIOD, false));
    engine.set_release_stagger(0.0);
    handle.send(Command::SetWind {
        group: 0,
        params: wind::WindParams {
            sag_depth: 0.0,
            flow_noise: 0.0,
            ..wind::WindParams::default()
        },
    });
    for enclosure in [0u8, 1] {
        handle.send(Command::SetEnclosure {
            enclosure,
            params: enclosure::EnclosureParams {
                // Shutters already where the pedal says: this measures
                // the pressure leg, not the inertia model.
                full_sweep_s: 0.0,
                pressure_rise_pct: rise_pct,
                ..enclosure::EnclosureParams::default()
            },
        });
        handle.send(Command::SetEnclosurePosition {
            enclosure,
            position,
        });
    }
    // Reference demand is 30 per box; three pipes at 10 make a full
    // chorus, and only the first of them is heard.
    for handle_id in 1..=3u64 {
        handle.send(Command::StartVoice {
            handle: handle_id,
            sample: 0,
            rate: 1.0,
            gain: if handle_id == 1 { 1.0 } else { 0.0 },
            group: 0,
            wind_weight: 10.0,
            brightness: 0.0,
            enclosures: boxes,
            bus: 0,
            delay_frames: 0,
            nominal_hz: 400.0,
        });
    }
    (engine, handle)
}

/// Cents the measured pitch sits away from the sample's own 400 Hz,
/// over the second half of a one-second render (filters and lags
/// settled). Flat is negative.
fn measured_cents(engine: &mut Engine) -> f64 {
    let out = render(engine, 48_000);
    let period = measured_period(&out[48_000..]);
    -1200.0 * (period / 120.0).log2()
}

/// HW's "very slight, but just discernible detuning when the box is
/// fully closed": a closed box under a full chorus flattens its pipes
/// by about a cent at the default 2 % rise, and by nothing at all when
/// the knob is off.
#[test]
fn a_closed_box_flattens_the_pipes_inside_it() {
    let mut open = pressure_test_engine(2.0, [0, ENCLOSURE_NONE], 1.0).0;
    let mut closed = pressure_test_engine(2.0, [0, ENCLOSURE_NONE], 0.0).0;
    let mut disabled = pressure_test_engine(0.0, [0, ENCLOSURE_NONE], 0.0).0;
    let open_cents = measured_cents(&mut open);
    let closed_cents = measured_cents(&mut closed);
    let disabled_cents = measured_cents(&mut disabled);

    // 2 % of chest pressure through the pitch exponent (0.032):
    // 1200·log2(1 − 0.032·0.02) ≈ −1.11 cents.
    let expected = 1200.0 * (1.0 - 0.032 * 0.02f64).log2();
    let shift = closed_cents - open_cents;
    assert!(
        (shift - expected).abs() < 0.25,
        "closed box shifted {shift:.2} cents, expected {expected:.2}"
    );
    assert!(
        (disabled_cents - open_cents).abs() < 0.05,
        "the knob at 0 % still moved the pitch by {:.2} cents",
        disabled_cents - open_cents
    );
    // An open box barely pressurizes at all: the shutter front vents
    // it a hundred times faster than its own leakage.
    assert!(
        open.enclosure_pressure_rise(0) < 0.0005,
        "an open box held {} of pressure",
        open.enclosure_pressure_rise(0)
    );
}

/// The rise builds on the box's fill time constant — a first-order
/// lag, so ~63 % of the way there after one τ — and collapses the
/// moment the shutters crack.
#[test]
fn box_pressure_builds_on_its_time_constant() {
    let tau = enclosure::EnclosureParams::default().fill_seconds;
    let (mut engine, mut handle) = pressure_test_engine(2.0, [0, ENCLOSURE_NONE], 0.0);
    let final_rise = 0.02f32;
    render(&mut engine, (48_000.0 * tau) as usize);
    let at_tau = engine.enclosure_pressure_rise(0);
    assert!(
        (at_tau / final_rise - 0.632).abs() < 0.06,
        "after one τ the rise is {:.1} % of final, expected ~63 %",
        100.0 * at_tau / final_rise
    );
    render(&mut engine, 48_000);
    let settled = engine.enclosure_pressure_rise(0);
    assert!(
        (settled - final_rise).abs() < 0.0005,
        "settled at {settled}, expected {final_rise}"
    );
    // Crack the shutters: the box vents, fast.
    handle.send(Command::SetEnclosurePosition {
        enclosure: 0,
        position: 0.1,
    });
    render(&mut engine, 48_000 / 10);
    assert!(
        engine.enclosure_pressure_rise(0) < 0.15 * settled,
        "cracking the shutters left {} of {settled}",
        engine.enclosure_pressure_rise(0)
    );
}

/// Nested boxes stack: the inner box vents into the outer one, so its
/// own rise is measured relative to the outer box's and the pipe's
/// mouth sees the sum. Two closed boxes flatten twice as far as one.
#[test]
fn nested_box_pressure_stacks() {
    let mut one = pressure_test_engine(2.0, [0, ENCLOSURE_NONE], 0.0).0;
    let mut two = pressure_test_engine(2.0, [0, 1], 0.0).0;
    let mut open = pressure_test_engine(2.0, [0, 1], 1.0).0;
    let single = measured_cents(&mut one) - measured_cents(&mut open);
    let nested = measured_cents(&mut two) - measured_cents(&mut open);
    assert!(
        (nested - 2.0 * single).abs() < 0.25,
        "nested shift {nested:.2} cents, expected ~2× the single box's {single:.2}"
    );
}
