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
