//! Command dispatch and voice allocation.

use super::*;

#[test]
fn set_voice_rate_snaps_when_glide_is_zero() {
    let (mut engine, mut handle) = Engine::new(100.0, ramp_bank(2000));
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
    render(&mut engine, 20);
    let before = slope(&render(&mut engine, 20));
    handle.send(Command::SetVoiceRate {
        handle: 1,
        rate: 2.0,
        glide_ms: 0.0,
    });
    let after = slope(&render(&mut engine, 20));
    let ratio = after / before;
    assert!(
        (ratio - 2.0).abs() < 0.1,
        "zero-glide rate change should double the cursor speed, got ×{ratio}"
    );
    // Nonsense targets are ignored, not applied.
    handle.send(Command::SetVoiceRate {
        handle: 1,
        rate: -1.0,
        glide_ms: 0.0,
    });
    handle.send(Command::SetVoiceRate {
        handle: 1,
        rate: f32::NAN,
        glide_ms: 0.0,
    });
    let still = slope(&render(&mut engine, 20));
    assert!(
        (still / before - 2.0).abs() < 0.1,
        "invalid rates must leave the voice untouched"
    );
}

#[test]
fn set_voice_rate_glides_geometrically() {
    let (mut engine, mut handle) = Engine::new(100.0, ramp_bank(2000));
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
    render(&mut engine, 20);
    let start = slope(&render(&mut engine, 10));
    // 400 ms at sr=100 → 40 frames of glide: four 10-frame blocks,
    // each a constant 2^(1/4) step (geometric = equal cents).
    handle.send(Command::SetVoiceRate {
        handle: 1,
        rate: 2.0,
        glide_ms: 400.0,
    });
    let mut slopes = Vec::new();
    for _ in 0..4 {
        slopes.push(slope(&render(&mut engine, 10)));
    }
    let settled = slope(&render(&mut engine, 10));
    assert!(
        (settled / start - 2.0).abs() < 0.1,
        "glide should settle on the target rate"
    );
    let quarter = 2.0f32.powf(0.25);
    for (i, pair) in slopes.windows(2).enumerate() {
        let step = pair[1] / pair[0];
        assert!(
            (step / quarter - 1.0).abs() < 0.08,
            "block {i}: expected a 2^(1/4) step, got ×{step}"
        );
    }
}

#[test]
fn a_release_freezes_a_glide_in_flight() {
    let (mut engine, mut handle) = Engine::new(100.0, test_bank());
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
    render(&mut engine, 10);
    handle.send(Command::SetVoiceRate {
        handle: 1,
        rate: 4.0,
        glide_ms: 10_000.0,
    });
    render(&mut engine, 10);
    handle.send(Command::StopVoice { handle: 1 });
    // Tail (40 frames) + crossfade at the barely-moved rate: the
    // voice must play out and end cleanly, glide abandoned.
    render(&mut engine, 60);
    let out = render(&mut engine, 100);
    assert!(
        out[100..].iter().all(|&v| v == 0.0),
        "released voice should end; the glide must not keep it alive"
    );
    engine.assert_slot_invariants();
}
