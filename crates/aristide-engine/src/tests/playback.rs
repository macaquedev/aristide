//! Sampled playback: cursors, loops, onset delay, channels.

use super::*;

#[test]
fn sampled_voice_plays_and_loops() {
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
        voicing_tilt: 1.0,
        enclosures: [ENCLOSURE_NONE; MAX_VOICE_ENCLOSURES],
        bus: 0,
        delay_frames: 0,
        nominal_hz: 0.0,
    });
    // 200 frames from a 100-frame sample: only survivable by looping.
    let out = render(&mut engine, 200);
    assert!(out[10] > 0.0, "audio should be flowing");
    let late = &out[300..400];
    assert!(
        late.iter().any(|&v| v != 0.0),
        "loop should keep the voice alive past the sample end"
    );
    // Looping stays within loop bounds: values in (0.2*g, 0.6*g).
    let master = DEFAULT_MASTER_GAIN;
    for (i, &v) in out.iter().enumerate().skip(140) {
        assert!(
            v >= 0.19 * master && v <= 0.61 * master,
            "frame {i}: {v} escaped the sustain loop"
        );
    }
}

#[test]
fn onset_delay_holds_the_pipe_silent_then_speaks() {
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
        voicing_tilt: 1.0,
        enclosures: [ENCLOSURE_NONE; MAX_VOICE_ENCLOSURES],
        bus: 0,
        delay_frames: 30,
        nominal_hz: 0.0,
    });
    let out = render(&mut engine, 50);
    assert!(
        out[..30 * 2].iter().all(|&v| v == 0.0),
        "silent through the onset delay"
    );
    assert!(
        out[32 * 2..].iter().any(|&v| v != 0.0),
        "speaks once the delay elapses"
    );

    // Released while still waiting: the pallet never opened, so
    // nothing may ever sound and the slot must come back.
    let (mut engine, mut handle) = Engine::new(100.0, test_bank());
    engine.set_release_stagger(0.0);
    handle.send(Command::StartVoice {
        handle: 2,
        sample: 0,
        rate: 1.0,
        gain: 1.0,
        group: 0,
        wind_weight: 0.0,
        brightness: 0.0,
        voicing_tilt: 1.0,
        enclosures: [ENCLOSURE_NONE; MAX_VOICE_ENCLOSURES],
        bus: 0,
        delay_frames: 200,
        nominal_hz: 0.0,
    });
    render(&mut engine, 50);
    handle.send(Command::StopVoice { handle: 2 });
    let out = render(&mut engine, 300);
    assert!(out.iter().all(|&v| v == 0.0), "never speaks");
    engine.assert_slot_invariants();
}

#[test]
fn multi_loop_voices_visit_all_loops() {
    // Two loops with distinct constant levels: 0.8 and 0.3. A voice
    // drawing loops at random must produce both levels over time.
    let mut data = vec![0.0f32; 600];
    for (index, value) in data.iter_mut().enumerate() {
        *value = match index {
            0..=99 => index as f32 / 100.0 * 0.8, // attack ramp
            100..=199 => 0.8,                     // loop A
            // smooth descent between the loops (the engine plays
            // through here when switching toward loop B)
            200..=299 => 0.8 - 0.5 * (index - 199) as f32 / 100.0,
            300..=399 => 0.3, // loop B
            _ => 0.3 - 0.3 * (index - 399) as f32 / 200.0, // tail out
        };
    }
    let mut sample = Sample::new(data, 1, 48000.0, Some((100, 200)), 400).expect("valid");
    sample.add_loop(300, 400).expect("alternate loop");
    let mut bank = SampleBank::default();
    bank.push(sample);
    let (mut engine, mut handle) = Engine::new(48000.0, Arc::new(bank));
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
    // ~100 loop passes.
    let mut buffer = vec![0.0f32; 10000 * 2];
    engine.process(&mut buffer, 2);
    let master = DEFAULT_MASTER_GAIN;
    let near = |target: f32| {
        buffer
            .chunks(2)
            .filter(|f| (f[0] - target * master).abs() < 0.05 * master)
            .count()
    };
    let high = near(0.8);
    let low = near(0.3);
    // These loops are disjoint and sequential (pathological — real
    // sets' loops overlap), so once the voice commits to the later
    // loop the earlier one is behind it; what matters is that both
    // get PLAYED and that every transition is seamless.
    assert!(
        high > 200 && low > 500,
        "both loops should be visited: high {high}, low {low}"
    );

    // Loop switching must never splice discontinuously: the data is
    // constants + a gentle ramp, so any frame-to-frame jump beyond
    // the ramp slope is a click (the old code jumped straight from
    // loop A's end into loop B's start: a 0.5-amplitude pop).
    let mono: Vec<f32> = buffer.chunks(2).map(|f| f[0]).collect();
    // Skip the attack ramp start-up.
    let max_delta = mono[200..]
        .windows(2)
        .map(|w| (w[1] - w[0]).abs())
        .fold(0.0f32, f32::max);
    assert!(
        max_delta < 0.02 * master,
        "loop transition clicked: max frame delta {max_delta}"
    );
}

/// ODF `AttackStart`: the voice's cursor begins at the marked
/// frame, skipping lead-in the producer excluded.
#[test]
fn attack_start_skips_lead_in() {
    let mut data = vec![0.0f32; 100];
    data.extend(std::iter::repeat(0.5f32).take(400));
    let mut sample = Sample::new(data, 1, 48000.0, None, 500).expect("valid");
    sample.set_attack_start(100);
    let mut bank = SampleBank::default();
    let id = bank.push(sample);
    let (mut engine, mut handle) = Engine::new(48000.0, Arc::new(bank));
    handle.send(Command::StartVoice {
        handle: 1,
        sample: id,
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
    let mut buffer = vec![0.0f32; 64 * 2];
    engine.process(&mut buffer, 2);
    assert!(
        buffer[0].abs() > 1e-4,
        "playback should begin in the marked material, not the lead-in: {}",
        buffer[0]
    );
}

#[test]
fn stereo_sample_reaches_both_channels() {
    // L ramps up, R constant — catches interleave mistakes.
    let mut data = Vec::new();
    for i in 0..100 {
        data.push(i as f32 / 100.0);
        data.push(0.25);
    }
    let sample = Sample::new(data, 2, 100.0, Some((10, 90)), 90).expect("valid");
    let mut bank = SampleBank::default();
    bank.push(sample);
    let (mut engine, mut handle) = Engine::new(100.0, Arc::new(bank));
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
    let out = render(&mut engine, 50);
    let master = DEFAULT_MASTER_GAIN;
    assert!((out[41] - 0.25 * master).abs() < 1e-6, "right channel");
    assert!((out[40] - out[41]).abs() > 1e-6, "channels differ");
}
