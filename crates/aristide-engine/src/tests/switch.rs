//! Wave-tremulant recording switches on notes that are already held:
//! the loop→loop crossfade, what it hands over, and what happens when
//! a second switch or a key-off lands inside it.

use super::*;

/// Frames per fundamental period of the synthetic pipe (100 Hz at
/// 48 kHz), and the phase the tremmed take was recorded at: a separate
/// take never starts where the other one did, which is the whole
/// reason the loop→loop phase map exists.
const PERIOD: usize = 480;
const TREM_PHASE: f64 = 0.48;
/// Inter-channel phase the (stereo) mic pair imposes — identical in
/// both takes, so one splice frame serves both channels exactly.
const CHANNEL_PHASE: f64 = 0.13;
/// Amplitude depth of the tremmed take and how many undulation cycles
/// fit in its loop. An integer count is what makes the tremmed loop
/// seamless in its envelope as well as its waveform.
const TREM_DEPTH: f64 = 0.15;
const TREM_CYCLES: f64 = 2.0;

const LOOP_START: usize = PERIOD * 4;
const LOOP_END: usize = PERIOD * 28;
const FRAMES: usize = PERIOD * 32;

/// One pipe recorded twice — plain, and under a wave tremulant — as a
/// bank. `wire` builds the loop→loop phase maps between them; without
/// it the pair is exactly a set that has no tremmed variants, which is
/// what the bit-identity test needs. `releases` gives each take its own
/// separate release, of clearly different length, so a key-off says
/// which recording the voice ended up on.
fn twin_bank(wire: bool, releases: bool) -> (Arc<SampleBank>, u32, u32) {
    let stereo = |amplitude: &dyn Fn(usize) -> f64, phase: f64| -> Vec<f32> {
        let mut data = Vec::with_capacity(FRAMES * 2);
        for n in 0..FRAMES {
            let turns = n as f64 / PERIOD as f64 + phase;
            let a = amplitude(n);
            data.push((a * (core::f64::consts::TAU * turns).sin()) as f32);
            data.push(
                (a * (core::f64::consts::TAU * (turns + CHANNEL_PHASE)).sin()) as f32,
            );
        }
        data
    };
    let undulation = |n: usize| {
        let turns = TREM_CYCLES * (n as f64 - LOOP_START as f64)
            / (LOOP_END - LOOP_START) as f64;
        0.65 * (1.0 + TREM_DEPTH * (core::f64::consts::TAU * turns).sin())
    };
    let sustain = Some((LOOP_START as u64, LOOP_END as u64));
    // Release start at EOF = no embedded tail; the takes splice out
    // through their separate releases or not at all.
    let mut plain =
        Sample::new(stereo(&|_| 0.5, 0.0), 2, 48000.0, sustain, FRAMES as u64).expect("valid");
    let mut tremmed = Sample::new(
        stereo(&undulation, TREM_PHASE),
        2,
        48000.0,
        sustain,
        FRAMES as u64,
    )
    .expect("valid");
    plain.align_release(48000.0 / PERIOD as f32);
    tremmed.align_release(48000.0 / PERIOD as f32);

    let mut bank = SampleBank::default();
    if releases {
        let decaying = |frames: usize| -> Sample {
            let mut data = Vec::with_capacity(frames * 2);
            for n in 0..frames {
                let turns = n as f64 / PERIOD as f64;
                let a = 0.5 * (1.0 - n as f64 / frames as f64);
                data.push((a * (core::f64::consts::TAU * turns).sin()) as f32);
                data.push(
                    (a * (core::f64::consts::TAU * (turns + CHANNEL_PHASE)).sin()) as f32,
                );
            }
            Sample::new(data, 2, 48000.0, None, frames as u64).expect("valid")
        };
        let short = bank.push(decaying(2400)); // 50 ms
        let long = bank.push(decaying(24000)); // 500 ms
        plain.attach_release(bank.get(short).expect("short"), short, None, None, 0);
        tremmed.attach_release(bank.get(long).expect("long"), long, None, None, 0);
    }
    let plain_id = bank.push(plain);
    let tremmed_id = bank.push(tremmed);
    if wire {
        bank.attach_switch(plain_id, tremmed_id);
        bank.attach_switch(tremmed_id, plain_id);
    }
    (Arc::new(bank), plain_id, tremmed_id)
}

fn start_on(bank: Arc<SampleBank>, sample: u32) -> (Engine, EngineHandle) {
    let (mut engine, mut handle) = Engine::new(48000.0, bank);
    engine.set_release_stagger(0.0);
    handle.send(Command::StartVoice {
        handle: 1,
        sample,
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
    (engine, handle)
}

fn switch(handle: &mut EngineHandle, sample: u32) {
    handle.send(Command::SwitchVoiceSample {
        handle: 1,
        sample,
        rate_factor: 1.0,
    });
}

/// Largest frame-to-frame jump in one channel of a span. A splice that
/// steps shows up here and nowhere else; a splice that is out of phase
/// shows up as a dip in [`level`] instead.
fn max_step(out: &[f32], channel: usize) -> f32 {
    out.chunks(2)
        .map(|frame| frame[channel])
        .collect::<Vec<f32>>()
        .windows(2)
        .map(|pair| (pair[1] - pair[0]).abs())
        .fold(0.0, f32::max)
}

/// RMS of one channel over `len` frames from `start`.
fn level(out: &[f32], channel: usize, start: usize, len: usize) -> f32 {
    let sum: f32 = (start..start + len)
        .map(|frame| out[frame * 2 + channel].powi(2))
        .sum();
    (sum / len as f32).sqrt()
}

/// The loudest and quietest half-trem-cycle of a span — how much the
/// output undulates, i.e. whether the tremmed recording is the one
/// actually sounding.
fn undulation(out: &[f32], channel: usize) -> f32 {
    let window = PERIOD * 4;
    let mut loudest = 0.0f32;
    let mut quietest = f32::MAX;
    let mut start = 0;
    while start + window <= out.len() / 2 {
        let rms = level(out, channel, start, window);
        loudest = loudest.max(rms);
        quietest = quietest.min(rms);
        start += window / 2;
    }
    loudest / quietest.max(1e-9)
}

/// Everything after the switch, as one continuous span: the tail of
/// the steady hold, the crossfade, and the settled result. Scanning
/// across the block seams is the point — a handover that lands on a
/// block boundary must be no different from one that doesn't.
fn hold_switch_settle(
    engine: &mut Engine,
    handle: &mut EngineHandle,
    target: u32,
) -> (Vec<f32>, usize) {
    let steady = render(engine, 48000);
    let keep = PERIOD * 8;
    let mut span = steady[(48000 - keep) * 2..].to_vec();
    switch(handle, target);
    // The fade is ~9 fundamental periods (90 ms here); render well
    // past it, in blocks, so the seams get scanned too.
    for _ in 0..6 {
        span.extend(render(engine, 6000));
    }
    (span, keep)
}

#[test]
fn switch_alignment_lands_on_the_matching_phase_of_the_other_take() {
    let (bank, plain, tremmed) = twin_bank(true, false);
    let sample = bank.get(plain).expect("plain");
    let option = sample.switch_option(tremmed).expect("switch wired");
    let alignment = option.alignment().expect("phase map built");
    let (loop_start, _) = sample.sustain_loop().expect("loop");
    for probe in 0..32 {
        let position = loop_start as f64 + probe as f64 * 37.3;
        let target = alignment.target(position, loop_start);
        let source_phase = (position / PERIOD as f64).fract();
        // The tremmed take's waveform phase at frame f is
        // f/period + TREM_PHASE — the offset the map has to undo.
        let target_phase = (target as f64 / PERIOD as f64 + TREM_PHASE).fract();
        let mut delta = (source_phase - target_phase).abs();
        delta = delta.min(1.0 - delta);
        assert!(
            delta < 1.5 / bank::ALIGNMENT_BUCKETS as f64 + 0.01,
            "position {position}: plain phase {source_phase:.3} vs tremmed {target_phase:.3}"
        );
    }
}

#[test]
fn engaging_mid_hold_crosses_into_the_tremmed_take_without_a_step() {
    let (bank, plain, tremmed) = twin_bank(true, false);
    let (mut engine, mut handle) = start_on(bank, plain);
    let (span, keep) = hold_switch_settle(&mut engine, &mut handle, tremmed);

    let settled = &span[(span.len() - 24000 * 2)..];
    for channel in 0..2 {
        // The two ends of the span are steady material; the crossfade
        // in between must not be less smooth than either of them.
        let before = max_step(&span[..keep * 2], channel);
        let after = max_step(settled, channel);
        let worst = max_step(&span, channel);
        assert!(
            worst <= 1.15 * before.max(after),
            "channel {channel}: crossfade steps {worst:.5}, steady is {before:.5}/{after:.5}"
        );
        // And it must not cancel: an unaligned loop→loop splice partly
        // subtracts the two legs instead of clicking. Scanned over the
        // crossfade itself (the ~4320-frame fade plus a margin) —
        // past it the tremmed take's own 0.75× trough is the answer,
        // not an artifact.
        let steady = level(&span, channel, 0, keep);
        let mut worst_dip = f32::MAX;
        let mut start = keep;
        while start + PERIOD <= keep + 5000 {
            worst_dip = worst_dip.min(level(&span, channel, start, PERIOD) / steady);
            start += PERIOD / 4;
        }
        assert!(
            worst_dip > 0.78,
            "channel {channel}: the crossfade dipped to {worst_dip:.2} of the held level"
        );
        // The undulation is audible where it wasn't before.
        let flat = undulation(&span[..keep * 2], channel);
        let waving = undulation(settled, channel);
        assert!(flat < 1.05, "channel {channel}: the plain take waves {flat:.2}:1");
        assert!(
            waving > 1.25,
            "channel {channel}: the tremmed take only waves {waving:.2}:1"
        );
    }
}

#[test]
fn releasing_the_tremulant_returns_the_held_note_to_the_plain_take() {
    let (bank, plain, tremmed) = twin_bank(true, false);
    let (mut engine, mut handle) = start_on(bank, plain);
    hold_switch_settle(&mut engine, &mut handle, tremmed);
    let (span, keep) = hold_switch_settle(&mut engine, &mut handle, plain);
    let settled = &span[(span.len() - 24000 * 2)..];
    for channel in 0..2 {
        let before = max_step(&span[..keep * 2], channel);
        let after = max_step(settled, channel);
        assert!(
            max_step(&span, channel) <= 1.15 * before.max(after),
            "channel {channel}: switching back steps"
        );
        assert!(
            undulation(settled, channel) < 1.05,
            "channel {channel}: back on the plain take, the undulation is gone"
        );
    }
}

#[test]
fn a_second_switch_mid_crossfade_reverses_exactly() {
    let (bank, plain, tremmed) = twin_bank(true, false);
    let (mut engine, mut handle) = start_on(bank, plain);
    let steady = render(&mut engine, 48000);
    let keep = PERIOD * 8;
    let mut span = steady[(48000 - keep) * 2..].to_vec();
    switch(&mut handle, tremmed);
    // A third of the way into the ~90 ms fade.
    span.extend(render(&mut engine, 1500));
    switch(&mut handle, plain);
    for _ in 0..5 {
        span.extend(render(&mut engine, 6000));
    }
    let settled = &span[(span.len() - 24000 * 2)..];
    for channel in 0..2 {
        let before = max_step(&span[..keep * 2], channel);
        let after = max_step(settled, channel);
        assert!(
            max_step(&span, channel) <= 1.15 * before.max(after),
            "channel {channel}: the reversal steps"
        );
        assert!(
            undulation(settled, channel) < 1.05,
            "channel {channel}: the voice ends on the plain take it started from"
        );
    }
}

#[test]
fn a_key_off_mid_crossfade_releases_cleanly_and_ends() {
    let (bank, plain, tremmed) = twin_bank(true, true);
    let (mut engine, mut handle) = start_on(bank, plain);
    let steady = render(&mut engine, 48000);
    let keep = PERIOD * 8;
    let mut span = steady[(48000 - keep) * 2..].to_vec();
    let before = [max_step(&span, 0), max_step(&span, 1)];
    switch(&mut handle, tremmed);
    span.extend(render(&mut engine, 1500));
    handle.send(Command::StopVoice { handle: 1 });
    for _ in 0..8 {
        span.extend(render(&mut engine, 6000));
    }
    for channel in 0..2 {
        assert!(
            max_step(&span, channel) <= 1.15 * before[channel],
            "channel {channel}: releasing mid-crossfade steps"
        );
    }
    let ending = render(&mut engine, 24000);
    assert!(
        ending.iter().all(|&v| v == 0.0),
        "the voice should have ended"
    );
}

#[test]
fn a_switched_voice_releases_out_of_the_recording_it_landed_on() {
    // The plain take's separate release is 50 ms, the tremmed take's
    // 500 ms — so how long the tail rings says which recording the
    // voice was holding when the key came up.
    let audible = |engage: bool| -> usize {
        let (bank, plain, tremmed) = twin_bank(true, true);
        let (mut engine, mut handle) = start_on(bank, plain);
        render(&mut engine, 48000);
        if engage {
            switch(&mut handle, tremmed);
            render(&mut engine, 24000);
        }
        handle.send(Command::StopVoice { handle: 1 });
        let out = render(&mut engine, 48000);
        out.chunks(2)
            .rposition(|frame| frame[0].abs() > 1e-4)
            .unwrap_or(0)
    };
    let plain_tail = audible(false);
    let tremmed_tail = audible(true);
    assert!(
        plain_tail < 4800,
        "the plain take's 50 ms release should be short (got {plain_tail} frames)"
    );
    assert!(
        tremmed_tail > 20000,
        "after the switch the 500 ms release should be chosen (got {tremmed_tail})"
    );
}

#[test]
fn a_set_without_tremmed_twins_renders_bit_identically() {
    // Same two recordings, no phase maps between them: exactly a set
    // whose pipes have no `IsTremulant` variants. The switch command
    // must then be a no-op, frame for frame.
    let reference = {
        let (bank, plain, _) = twin_bank(false, true);
        let (mut engine, _handle) = start_on(bank, plain);
        render(&mut engine, 24000)
    };
    let (bank, plain, tremmed) = twin_bank(false, true);
    let (mut engine, mut handle) = start_on(bank, plain);
    switch(&mut handle, tremmed);
    let switched = render(&mut engine, 24000);
    assert_eq!(reference, switched, "an unwired switch changed the render");
}
