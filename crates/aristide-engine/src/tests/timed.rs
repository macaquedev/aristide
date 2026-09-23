//! Release timers: a stop rule's finite events end by themselves.

use super::*;

fn start(handle: &mut EngineHandle, id: u64, delay_frames: u32, end_frames: u32) {
    handle.send(Command::StartVoice {
        handle: id,
        sample: 0,
        rate: 1.0,
        gain: 1.0,
        group: 0,
        wind_weight: 0.0,
        brightness: 0.0,
        voicing_tilt: 1.0,
        enclosures: [ENCLOSURE_NONE; MAX_VOICE_ENCLOSURES],
        bus: 0,
        delay_frames,
        end_frames,
        nominal_hz: 0.0,
    });
}

fn silent(block: &[f32]) -> bool {
    block.iter().all(|&v| v == 0.0)
}

#[test]
fn a_finite_voice_releases_itself() {
    let (mut engine, mut handle) = Engine::new(100.0, test_bank());
    engine.set_release_stagger(0.0);
    start(&mut handle, 1, 0, 20);
    let mut heard = false;
    for _ in 0..10 {
        heard |= !silent(&render(&mut engine, 5));
    }
    assert!(heard, "speaks before its ending");
    // The tail is 40 frames; nothing is left well after it.
    render(&mut engine, 100);
    assert!(silent(&render(&mut engine, 50)), "ended without a StopVoice");
    engine.assert_slot_invariants();
}

#[test]
fn a_held_voice_keeps_speaking_until_its_timer() {
    let (mut engine, mut handle) = Engine::new(100.0, test_bank());
    engine.set_release_stagger(0.0);
    start(&mut handle, 1, 0, 0);
    render(&mut engine, 10);
    handle.send(Command::StopVoiceIn { handle: 1, frames: 200 });
    for _ in 0..30 {
        assert!(!silent(&render(&mut engine, 5)), "still held inside the timer");
    }
    render(&mut engine, 250);
    assert!(silent(&render(&mut engine, 50)), "released when the timer ran out");
    engine.assert_slot_invariants();
}

#[test]
fn a_timer_running_out_during_the_onset_never_sounds() {
    let (mut engine, mut handle) = Engine::new(100.0, test_bank());
    engine.set_release_stagger(0.0);
    start(&mut handle, 1, 100, 0);
    handle.send(Command::StopVoiceIn { handle: 1, frames: 20 });
    for _ in 0..40 {
        assert!(silent(&render(&mut engine, 10)), "the pallet never opened");
    }
    engine.assert_slot_invariants();
}
