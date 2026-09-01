//! Key release: splice selection, alignment, tails, level match.

use super::*;

#[test]
fn released_voice_splices_to_tail_and_ends() {
    let (mut engine, mut handle) = Engine::new(100.0, test_bank());
    engine.set_release_stagger(0.0);
    handle.send(Command::StartVoice {
        handle: 7,
        sample: 0,
        rate: 1.0,
        gain: 1.0,
        group: 0,
        wind_weight: 0.0,
        brightness: 0.0,
        enclosure: ENCLOSURE_NONE,
        bus: 0,
        delay_frames: 0,
        nominal_hz: 0.0,
    });
    render(&mut engine, 10);
    handle.send(Command::StopVoice { handle: 7 });
    // Tail is 40 frames (60..100) + crossfade 3 frames at sr=100.
    render(&mut engine, 30);
    let out = render(&mut engine, 100);
    let silent_after = &out[60 * 2..];
    assert!(
        silent_after.iter().all(|&v| v == 0.0),
        "voice should have ended after the release tail"
    );
}

#[test]
fn percussive_sample_ignores_stop_and_ends_itself() {
    let data: Vec<f32> = (0..50).map(|_| 0.5).collect();
    let sample = Sample::new(data, 1, 100.0, None, 0).expect("valid");
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
        enclosure: ENCLOSURE_NONE,
        bus: 0,
        delay_frames: 0,
        nominal_hz: 0.0,
    });
    handle.send(Command::StopVoice { handle: 1 });
    let out = render(&mut engine, 60);
    assert!(out[20] != 0.0, "percussive sample should keep playing");
    let out = render(&mut engine, 20);
    assert!(out.iter().all(|&v| v == 0.0), "and then end on its own");
}

#[test]
fn aligned_release_splice_never_cancels() {
    let period = 480; // 100 Hz at 48 kHz
    // Stop moments spread across the waveform cycle, including the
    // adversarial anti-phase one (2640 = 5.5 periods).
    for stop_after in [2400, 2520, 2580, 2640, 2700, 2763, 2885] {
        let aligned = release_dip_ratio(sine_pipe_bank(period, true), stop_after, period);
        assert!(
            aligned > 0.7,
            "stop at {stop_after}: aligned splice dipped to {aligned:.2} of held level"
        );
    }
    // And the naive splice really is the artifact we claim to fix:
    // anti-phase stop cancels hard.
    let naive = release_dip_ratio(sine_pipe_bank(period, false), 2640, period);
    let aligned = release_dip_ratio(sine_pipe_bank(period, true), 2640, period);
    println!("anti-phase stop: aligned holds {aligned:.2} of level, naive dips to {naive:.2}");
    assert!(
        naive < 0.45,
        "naive anti-phase splice should cancel (got {naive:.2}) — is this test still valid?"
    );
}

#[test]
fn alignment_targets_match_waveform_phase() {
    let period = 480usize;
    let bank = sine_pipe_bank(period, true);
    let sample = bank.get(0).expect("sample");
    let alignment = sample.release_alignment().expect("alignment");
    let (loop_start, _) = sample.sustain_loop().expect("loop");
    for probe in 0..32 {
        let position = loop_start as f64 + probe as f64 * 37.3;
        let target = alignment.target(position, loop_start);
        let source_phase = (position / period as f64).fract();
        let target_phase = (target as f64 / period as f64).fract();
        let mut delta = (source_phase - target_phase).abs();
        delta = delta.min(1.0 - delta);
        assert!(
            delta < 1.5 / bank::ALIGNMENT_BUCKETS as f64 + 0.01,
            "position {position}: source phase {source_phase:.3} vs target {target_phase:.3}"
        );
    }
}

#[test]
fn separate_releases_select_by_hold_time() {
    let period = 480usize;
    let omega = std::f64::consts::TAU / period as f64;
    let sine = |frames: usize, envelope: &dyn Fn(usize) -> f64| -> Vec<f32> {
        (0..frames)
            .map(|n| (envelope(n) * (omega * n as f64).sin()) as f32)
            .collect()
    };
    // Attack sample: loop, no embedded tail (tail beyond EOF).
    let attack_frames = period * 16;
    let attack = sine(attack_frames, &|_| 1.0);
    let mut source = Sample::new(
        attack,
        1,
        48000.0,
        Some(((period * 4) as u64, (period * 12) as u64)),
        attack_frames as u64,
    )
    .expect("valid");
    source.align_release(48000.0 / period as f32);
    // Short release: 0.15 s of decaying sine. Long: 1.5 s.
    let short = Sample::new(
        sine(7200, &|n| 1.0 - n as f64 / 7200.0),
        1,
        48000.0,
        None,
        7200,
    )
    .expect("valid");
    let long = Sample::new(
        sine(72000, &|n| 1.0 - n as f64 / 72000.0),
        1,
        48000.0,
        None,
        72000,
    )
    .expect("valid");

    let mut bank = SampleBank::default();
    // Push releases first so their indices exist for attach.
    let short_id = bank.push(short);
    let long_id = bank.push(long);
    source.attach_release(bank.get(short_id).expect("short"), short_id, Some(300), None, 0);
    source.attach_release(bank.get(long_id).expect("long"), long_id, None, None, 0);
    let source_id = bank.push(source);
    let bank = Arc::new(bank);

    let audible_seconds = |hold_frames: usize| -> f64 {
        let (mut engine, mut handle) = Engine::new(48000.0, Arc::clone(&bank));
        engine.set_release_stagger(0.0);
        handle.send(Command::StartVoice {
            handle: 1,
            sample: source_id,
            rate: 1.0,
            gain: 1.0,
            group: 0,
            wind_weight: 0.0,
            brightness: 0.0,
            enclosure: ENCLOSURE_NONE,
        bus: 0,
        delay_frames: 0,
            nominal_hz: 0.0,
        });
        let mut buffer = vec![0.0f32; hold_frames * 2];
        engine.process(&mut buffer, 2);
        handle.send(Command::StopVoice { handle: 1 });
        // Render 2 s and find the last audible frame.
        let mut buffer = vec![0.0f32; 96000 * 2];
        engine.process(&mut buffer, 2);
        let last = buffer
            .chunks(2)
            .rposition(|f| f[0].abs() > 0.001)
            .unwrap_or(0);
        last as f64 / 48000.0
    };

    let staccato = audible_seconds(4800); // held 100 ms → short release
    let tenuto = audible_seconds(24000); // held 500 ms → long release
    assert!(
        staccato < 0.35,
        "staccato should use the 0.15 s release, rang for {staccato:.2} s"
    );
    assert!(
        tenuto > 0.9,
        "tenuto should use the 1.5 s release, rang for {tenuto:.2} s"
    );
}

/// A wave tremulant switches which recorded release a note-off
/// splices to: pipes carry `IsTremulant`-marked variants, and the
/// voice matches the chest's wave-trem state at key-off.
#[test]
fn releases_select_by_wave_trem_state() {
    let period = 480usize;
    let omega = std::f64::consts::TAU / period as f64;
    let sine = |frames: usize, envelope: &dyn Fn(usize) -> f64| -> Vec<f32> {
        (0..frames)
            .map(|n| (envelope(n) * (omega * n as f64).sin()) as f32)
            .collect()
    };
    let attack_frames = period * 16;
    let mut source = Sample::new(
        sine(attack_frames, &|_| 1.0),
        1,
        48000.0,
        Some(((period * 4) as u64, (period * 12) as u64)),
        attack_frames as u64,
    )
    .expect("valid");
    source.align_release(48000.0 / period as f32);
    let plain = Sample::new(
        sine(7200, &|n| 1.0 - n as f64 / 7200.0),
        1,
        48000.0,
        None,
        7200,
    )
    .expect("valid");
    let tremmed = Sample::new(
        sine(72000, &|n| 1.0 - n as f64 / 72000.0),
        1,
        48000.0,
        None,
        72000,
    )
    .expect("valid");

    let mut bank = SampleBank::default();
    let plain_id = bank.push(plain);
    let tremmed_id = bank.push(tremmed);
    source.attach_release(bank.get(plain_id).expect("plain"), plain_id, None, Some(false), 0);
    source.attach_release(
        bank.get(tremmed_id).expect("tremmed"),
        tremmed_id,
        None,
        Some(true),
        0,
    );
    let source_id = bank.push(source);
    let bank = Arc::new(bank);

    let audible_seconds = |trem_on: bool| -> f64 {
        let (mut engine, mut handle) = Engine::new(48000.0, Arc::clone(&bank));
        engine.set_release_stagger(0.0);
        handle.send(Command::StartVoice {
            handle: 1,
            sample: source_id,
            rate: 1.0,
            gain: 1.0,
            group: 0,
            wind_weight: 0.0,
            brightness: 0.0,
            enclosure: ENCLOSURE_NONE,
            bus: 0,
            delay_frames: 0,
            nominal_hz: 0.0,
        });
        if trem_on {
            // Engaged mid-hold: the held voice must follow the
            // switch, exactly as an organist drawing the trem does.
            handle.send(Command::SetWaveTremulant {
                group: 0,
                engaged: true,
            });
        }
        let mut buffer = vec![0.0f32; 24000 * 2];
        engine.process(&mut buffer, 2);
        handle.send(Command::StopVoice { handle: 1 });
        let mut buffer = vec![0.0f32; 96000 * 2];
        engine.process(&mut buffer, 2);
        let last = buffer
            .chunks(2)
            .rposition(|f| f[0].abs() > 0.001)
            .unwrap_or(0);
        last as f64 / 48000.0
    };

    let dry = audible_seconds(false);
    let undulating = audible_seconds(true);
    assert!(
        dry < 0.35,
        "trem off must pick the plain 0.15 s release, rang {dry:.2} s"
    );
    assert!(
        undulating > 0.9,
        "trem on must pick the tremmed 1.5 s release, rang {undulating:.2} s"
    );
}

#[test]
fn alignment_ignores_strong_second_harmonics() {
    // A principal-like pipe: 2nd harmonic nearly as strong as the
    // fundamental. Correlation-argmax alignment could lock a half
    // period off here (fundamental cancels, octave reinforces — a
    // missing-fundamental strike, i.e. a bell). Quadrature must
    // track the FUNDAMENTAL phase.
    let period = 480usize;
    let omega = std::f64::consts::TAU / period as f64;
    let loop_start = 1913u64; // deliberately not phase-aligned
    let loop_end = loop_start + (period * 40) as u64;
    let frames = loop_end + (period * 20) as u64;
    let data: Vec<f32> = (0..frames)
        .map(|n| {
            let envelope = if n >= loop_end {
                1.0 - 0.5 * (n - loop_end) as f64 / (frames - loop_end) as f64
            } else {
                1.0
            };
            let t = n as f64;
            (envelope * ((omega * t).sin() + 0.9 * (2.0 * omega * t).sin() * 0.9)) as f32
        })
        .collect();
    let mut sample =
        Sample::new(data, 1, 48000.0, Some((loop_start, loop_end)), loop_end).expect("valid");
    sample.align_release(48000.0 / period as f32);
    let alignment = sample.release_alignment().expect("alignment built");
    for probe in 0..16 {
        let position = loop_start as f64 + probe as f64 * 1123.7;
        let target = alignment.target(position, loop_start);
        let source_phase = (position / period as f64).fract();
        let target_phase = (target as f64 / period as f64).fract();
        let mut delta = (source_phase - target_phase).abs();
        delta = delta.min(1.0 - delta);
        assert!(
            delta < 2.0 / bank::ALIGNMENT_BUCKETS as f64 + 0.01,
            "position {position}: fundamental phase {source_phase:.3} vs \
             {target_phase:.3} (delta {delta:.3}) — octave ghost splice"
        );
    }
}

#[test]
fn alignment_survives_mistuned_pipes_and_long_loops() {
    // Real pipes sit cents off their nominal pitch, and a voice can
    // be hundreds of periods away from the loop-start phase anchor:
    // without measuring the true period, the alignment table points
    // at effectively random phase. True period here: 479.3 frames
    // (non-integer); declared nominal: 483 (≈ 13 cents off).
    let true_period = 479.3f64;
    let omega = std::f64::consts::TAU / true_period;
    let loop_start = 1920u64;
    let loop_end = loop_start + 200 * 480; // ~200 periods of travel
    let frames = loop_end + 480 * 20;
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
    let mut sample =
        Sample::new(data, 1, 48000.0, Some((loop_start, loop_end)), loop_end).expect("valid");
    sample.align_release(48000.0 / 483.0);
    let alignment = sample.release_alignment().expect("alignment built");

    for probe in 0..24 {
        // Positions spread across the whole loop, far from anchor.
        let position = loop_start as f64 + probe as f64 * 3997.3;
        if position >= loop_end as f64 {
            break;
        }
        let target = alignment.target(position, loop_start);
        let source_phase = (position / true_period).fract();
        let target_phase = (target as f64 / true_period).fract();
        let mut delta = (source_phase - target_phase).abs();
        delta = delta.min(1.0 - delta);
        assert!(
            delta < 2.0 / bank::ALIGNMENT_BUCKETS as f64 + 0.01,
            "position {position}: phase {source_phase:.3} vs target {target_phase:.3} \
             (delta {delta:.3}) — period estimation failed"
        );
    }
}

#[test]
fn alignment_locks_phase_for_high_pipes_in_noise() {
    // A high pipe: ~30-frame period (≈1.5 kHz) with room noise on
    // top. One-period correlation windows can't lock phase against
    // noise; the widened window must.
    let period = 30usize;
    let omega = std::f64::consts::TAU / period as f64;
    let loop_start = 1200u64;
    let loop_end = loop_start + (period * 100) as u64;
    let frames = loop_end + (period * 40) as u64;
    let mut noise_state = 0x1234_5678u32;
    let mut noise = move || {
        noise_state ^= noise_state << 13;
        noise_state ^= noise_state >> 17;
        noise_state ^= noise_state << 5;
        ((noise_state >> 8) as f64 / (1u32 << 24) as f64 - 0.5) * 0.3
    };
    let data: Vec<f32> = (0..frames)
        .map(|n| {
            let envelope = if n >= loop_end {
                1.0 - 0.6 * (n - loop_end) as f64 / (frames - loop_end) as f64
            } else {
                1.0
            };
            (envelope * ((omega * n as f64).sin() + noise())) as f32
        })
        .collect();
    let mut sample =
        Sample::new(data, 1, 44100.0, Some((loop_start, loop_end)), loop_end).expect("valid");
    sample.align_release(44100.0 / period as f32);
    let alignment = sample.release_alignment().expect("alignment built");
    for probe in 0..16 {
        let position = loop_start as f64 + probe as f64 * 217.7;
        let target = alignment.target(position, loop_start);
        let source_phase = (position / period as f64).fract();
        let target_phase = (target as f64 / period as f64).fract();
        let mut delta = (source_phase - target_phase).abs();
        delta = delta.min(1.0 - delta);
        assert!(
            delta < 0.12,
            "position {position}: phase {source_phase:.3} vs {target_phase:.3} — \
             high-pipe phase lock failed (delta {delta:.3})"
        );
    }
}

#[test]
fn early_release_does_not_strike_like_a_bell() {
    // A pipe whose attack ramps up over 4 periods: releasing during
    // the ramp used to splice to the tail at FULL recorded level —
    // a bell strike. The level match must scale it down.
    let period = 480usize;
    let omega = std::f64::consts::TAU / period as f64;
    let loop_start = period * 8;
    let loop_end = period * 16;
    let frames = period * 28;
    let ramp_end = (period * 4) as f64;
    let data: Vec<f32> = (0..frames)
        .map(|n| {
            let envelope = if (n as f64) < ramp_end {
                n as f64 / ramp_end
            } else if n >= loop_end {
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
    sample.align_release(48000.0 / period as f32);
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
        enclosure: ENCLOSURE_NONE,
        bus: 0,
        delay_frames: 0,
        nominal_hz: 0.0,
    });
    // Release 1.5 periods in: the ramp is at ~37 % amplitude.
    let mut buffer = vec![0.0f32; (period * 3 / 2) * 2];
    engine.process(&mut buffer, 2);
    handle.send(Command::StopVoice { handle: 1 });
    let mut buffer = vec![0.0f32; 9600 * 2];
    engine.process(&mut buffer, 2);
    let peak = buffer.iter().fold(0.0f32, |m, &v| m.max(v.abs()));
    // Unmatched, the tail peaked at the full 1.0 × master. With the
    // match, the tail leg is scaled to ~0.37; the residual above
    // that is the main leg still swelling through the crossfade
    // (the pipe genuinely keeps speaking for those 30 ms).
    assert!(
        peak < 0.55 * DEFAULT_MASTER_GAIN,
        "release struck like a bell: peak {peak}"
    );
    assert!(peak > 0.02 * DEFAULT_MASTER_GAIN, "release went silent");
}
