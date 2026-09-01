//! The built-in additive test tone.

use super::*;

#[test]
fn tone_voice_still_works() {
    let (mut engine, mut handle) = Engine::new(48000.0, Arc::new(SampleBank::default()));
    engine.set_release_stagger(0.0);
    handle.send(Command::NoteOn {
        key: 69,
        freq_hz: 440.0,
    });
    let out = render(&mut engine, 512);
    assert!(out.iter().any(|&v| v != 0.0));
    handle.send(Command::NoteOff { key: 69 });
    render(&mut engine, 48000);
    let out = render(&mut engine, 512);
    assert!(out.iter().all(|&v| v == 0.0));
}
