//! Builds the engine's [`SampleBank`] from a loaded organ model.
//!
//! Control-side only: decoding, validation, and per-pipe playback math
//! all happen here so the RT engine receives nothing but indices, rates,
//! and gains. Files are deduplicated by path (borrowed pipes and shared
//! samples decode once).

use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::Result;
use aristide_engine::bank::{Sample, SampleBank};
use aristide_formats::wav;
use aristide_model::{Organ, Pipe, PipeRef, PipeSource, RankId};

/// Playback parameters for one sounding pipe, precomputed against the
/// device sample rate.
#[derive(Debug, Clone, Copy)]
pub struct VoiceSpec {
    pub sample: u32,
    /// Source frames per output frame.
    pub rate: f32,
    /// Linear gain.
    pub gain: f32,
    /// Loop-less percussive samples get no StopVoice on key release.
    pub percussive: bool,
    /// Wind group (0-based engine index, from the ODF windchest).
    pub group: u8,
    /// Wind draw while sounding; 0 for noises and percussives.
    pub wind_weight: f32,
    /// Tilt-filter coefficient for pressure→brightness coupling
    /// (0 = no filter, e.g. noises).
    pub brightness: f32,
}

pub struct LoadedBank {
    pub bank: SampleBank,
    /// (rank, pipe index) → playback spec. Borrowed pipes carry their
    /// target's spec; silent and failed pipes are absent.
    pub specs: HashMap<(RankId, u16), VoiceSpec>,
    /// Human-readable notes about anything that didn't load.
    pub skipped: Vec<String>,
}

pub fn build(organ: &Organ, device_rate: f32) -> Result<LoadedBank> {
    let mut bank = SampleBank::default();
    let mut specs: HashMap<(RankId, u16), VoiceSpec> = HashMap::new();
    let mut skipped = Vec::new();
    // path → Ok(bank index + source metadata) or failure already noted.
    let mut decoded: HashMap<PathBuf, Option<DecodedInfo>> = HashMap::new();

    // Separate release files, deduplicated independently of attacks.
    let mut release_cache: HashMap<PathBuf, Option<u32>> = HashMap::new();

    for rank in &organ.ranks {
        for (pipe_index, pipe) in rank.pipes.iter().enumerate() {
            let PipeSource::Sampled { attacks, releases } = &pipe.source else {
                continue;
            };
            let Some(attack) = attacks.first() else {
                skipped.push(format!("{} pipe {pipe_index}: no attacks", rank.name));
                continue;
            };
            let absolute = organ.base_path.join(&attack.path);
            let entry = decoded.entry(attack.path.clone()).or_insert_with(|| {
                match decode(&absolute, &attack.loops) {
                    Ok((mut sample, info)) => {
                        // Phase-align the release splice to this pipe's
                        // fundamental (shared files share the pitch).
                        sample.align_release(pipe.nominal_frequency_hz as f32);
                        // Separate recorded releases become their own
                        // one-shot bank entries, attached with hold-time
                        // bounds and cross-file phase maps.
                        for release in releases {
                            let release_index = *release_cache
                                .entry(release.path.clone())
                                .or_insert_with(|| {
                                    let path = organ.base_path.join(&release.path);
                                    match decode_release(&path) {
                                        Ok(release_sample) => Some(bank.push(release_sample)),
                                        Err(reason) => {
                                            skipped.push(format!(
                                                "{}: {reason}",
                                                release.path.display()
                                            ));
                                            None
                                        }
                                    }
                                });
                            if let Some(index) = release_index {
                                if let Some(target) = bank.get(index) {
                                    sample.attach_release(
                                        target,
                                        index,
                                        release.max_key_press_ms,
                                    );
                                }
                            }
                        }
                        let index = bank.push(sample);
                        Some(DecodedInfo { index, ..info })
                    }
                    Err(reason) => {
                        skipped.push(format!("{}: {reason}", attack.path.display()));
                        None
                    }
                }
            });
            let Some(info) = entry else { continue };

            let cents = pipe.pitch_tuning_cents + attack.pitch_offset_cents;
            specs.insert(
                (rank.id, pipe_index as u16),
                VoiceSpec {
                    sample: info.index,
                    rate: (info.sample_rate / device_rate as f64
                        * (cents / 1200.0).exp2()) as f32,
                    gain: db_to_linear(pipe.gain_db),
                    percussive: info.percussive,
                    group: (rank.windchest.saturating_sub(1))
                        .min(aristide_engine::wind::MAX_WIND_GROUPS as u32 - 1)
                        as u8,
                    wind_weight: wind_weight(pipe.nominal_frequency_hz, info.percussive),
                    brightness: brightness_coefficient(
                        pipe.nominal_frequency_hz,
                        device_rate,
                        info.percussive,
                    ),
                },
            );
        }
    }

    // Borrowed pipes sound their target pipe verbatim.
    for rank in &organ.ranks {
        for (pipe_index, pipe) in rank.pipes.iter().enumerate() {
            if !matches!(pipe.source, PipeSource::Borrowed(_)) {
                continue;
            }
            let target = resolve_borrow(organ, pipe);
            match target.and_then(|t| specs.get(&(t.rank, t.pipe)).copied()) {
                Some(spec) => {
                    specs.insert((rank.id, pipe_index as u16), spec);
                }
                None => skipped.push(format!(
                    "{} pipe {pipe_index}: borrow target has no sample",
                    rank.name
                )),
            }
        }
    }

    Ok(LoadedBank {
        bank,
        specs,
        skipped,
    })
}

struct DecodedInfo {
    index: u32,
    sample_rate: f64,
    percussive: bool,
}

/// Decode one attack file into an engine [`Sample`].
///
/// Loop points come from the ODF when declared, else from the file's
/// `smpl` chunk (both use inclusive end frames). The release tail starts
/// at the file's last cue marker past the loop, else right after it —
/// GO's own fallback order.
fn decode(path: &std::path::Path, odf_loops: &[aristide_model::SampleLoop]) -> Result<(Sample, DecodedInfo), String> {
    let file = wav::read(path).map_err(|e| e.to_string())?;
    let frames = file.info.frames;

    let mut loops: Vec<(u64, u64)> = if odf_loops.is_empty() {
        file.info.loops.iter().map(|l| (l.start, l.end + 1)).collect()
    } else {
        odf_loops.iter().map(|l| (l.start, l.end + 1)).collect()
    };
    loops.retain(|&(start, end)| start < end && end <= frames);
    // Longest loop wins until multi-loop selection lands (M4).
    let sustain_loop = loops.iter().copied().max_by_key(|&(start, end)| end - start);

    let release_start = match sustain_loop {
        Some((_, loop_end)) => file
            .info
            .cue_points
            .iter()
            .copied()
            .filter(|&cue| cue >= loop_end && cue < frames)
            .max()
            .unwrap_or(loop_end),
        None => frames,
    };

    let mut sample = Sample::new(
        file.samples,
        file.info.channels,
        file.info.sample_rate as f32,
        sustain_loop,
        release_start,
    )?;
    // Alternate loops beyond the primary: voices rotate through them.
    for &(start, end) in &loops {
        if Some((start, end)) != sustain_loop {
            let _ = sample.add_loop(start, end);
        }
    }
    Ok((
        sample,
        DecodedInfo {
            index: 0, // filled by the caller after push
            sample_rate: file.info.sample_rate as f64,
            percussive: sustain_loop.is_none(),
        },
    ))
}

/// Decode a separate release file: a one-shot entry (no loops — it's a
/// decay), played from its start on key-off.
fn decode_release(path: &std::path::Path) -> Result<Sample, String> {
    let file = wav::read(path).map_err(|e| e.to_string())?;
    let frames = file.info.frames;
    Sample::new(
        file.samples,
        file.info.channels,
        file.info.sample_rate as f32,
        None,
        frames,
    )
}

/// Walk a borrow chain to the sampled pipe's address (hop-capped; the
/// loader guarantees chains terminate).
fn resolve_borrow(organ: &Organ, pipe: &Pipe) -> Option<PipeRef> {
    let mut current = match pipe.source {
        PipeSource::Borrowed(target) => target,
        _ => return None,
    };
    for _ in 0..64 {
        match &organ.pipe(current)?.source {
            PipeSource::Borrowed(next) => current = *next,
            PipeSource::Sampled { .. } => return Some(current),
            PipeSource::Silent => return None,
        }
    }
    None
}

fn db_to_linear(db: f64) -> f32 {
    10f64.powf(db / 20.0) as f32
}

/// One-pole coefficient for the voice's brightness tilt, hinged around
/// the pipe's 2nd harmonic so "upper partials" breathe with pressure
/// while the fundamental stays put. Deep bass keeps a floor on the
/// hinge (HW had to disable bass brightness modulation for distortion;
/// a 150 Hz floor sidesteps that). Percussive noises skip the filter.
fn brightness_coefficient(frequency_hz: f64, device_rate: f32, percussive: bool) -> f32 {
    if percussive || !(frequency_hz > 0.0) {
        return 0.0;
    }
    let hinge_hz = (2.0 * frequency_hz).clamp(150.0, 8000.0);
    1.0 - (-core::f64::consts::TAU * hinge_hz / device_rate as f64).exp() as f32
}

/// How hard a pipe draws on its windchest. Wind consumption roughly
/// halves per octave of speaking pitch (Walker US5508472 scales
/// 8'/4'/2' as 1.0/0.5/0.25), i.e. weight ∝ 1/f, normalized to 1.0 at
/// ~150 Hz. Percussive one-shots (action noises) draw nothing.
fn wind_weight(frequency_hz: f64, percussive: bool) -> f32 {
    if percussive || !(frequency_hz > 0.0) {
        return 0.0;
    }
    ((150.0 / frequency_hz) as f32).clamp(0.1, 4.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    /// The gitignored demo set; tests skip gracefully without it.
    fn demo_organ() -> Option<PathBuf> {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../testsets/grandorgue-demo/demo.organ");
        path.is_file().then_some(path)
    }

    /// Reproduce "fast spam distorts": hammer the plein jeu with rapid
    /// on/off pairs and measure what actually comes out — NaNs, clicks,
    /// peaks past the limiter ceiling, and the real-time cost.
    #[test]
    fn spam_stress_output_is_clean_and_realtime() {
        let Some(path) = demo_organ() else {
            eprintln!("skipping: demo set not present");
            return;
        };
        let organ = aristide_formats::grandorgue::load(&path)
            .expect("demo set loads")
            .organ;
        let loaded = build(&organ, 48000.0).expect("bank builds");
        // Full plein jeu on the Great.
        let manual_id = organ.manuals[1].id;
        let drawn: Vec<_> = organ
            .stops
            .iter()
            .filter(|s| {
                s.manual == manual_id
                    && ["Bourdon 16'", "Montre 8'", "Prestant 4'", "Plein jeu III"]
                        .contains(&s.name.as_str())
            })
            .map(|s| s.id)
            .collect();
        assert_eq!(drawn.len(), 4);
        let mut console =
            crate::console::Console::new(organ, loaded.specs, drawn, Vec::new());
        let (mut engine, mut handle) =
            aristide_engine::Engine::new(48000.0, std::sync::Arc::new(loaded.bank));

        // 8 s of spam: every 40 ms, note-off then note-on across a
        // 10-key cluster (≈ organist mashing), 256-frame blocks.
        let block = 256usize;
        let blocks = 8 * 48000 / block;
        let mut buffer = vec![0.0f32; block * 2];
        let mut worst_delta = 0.0f32;
        let mut peak = 0.0f32;
        let mut previous = 0.0f32;
        let mut nan = false;
        let keys = [55u8, 57, 59, 60, 62, 64, 65, 67, 69, 71];
        let started = std::time::Instant::now();
        for b in 0..blocks {
            if b % 8 == 0 {
                // toggle a rotating pair of keys
                let key = keys[(b / 8) % keys.len()];
                for handle_id in console.note_off(0, key) {
                    handle.send(aristide_engine::Command::StopVoice { handle: handle_id });
                }
                let (starts, retriggered) = console.note_on(0, key);
                for handle_id in retriggered {
                    handle.send(aristide_engine::Command::StopVoice { handle: handle_id });
                }
                for start in starts {
                    handle.send(aristide_engine::Command::StartVoice {
                        handle: start.handle,
                        sample: start.spec.sample,
                        rate: start.spec.rate,
                        gain: start.spec.gain,
                        group: start.spec.group,
                        wind_weight: start.spec.wind_weight,
                        brightness: start.spec.brightness,
                    });
                }
            }
            engine.process(&mut buffer, 2);
            for frame in buffer.chunks(2) {
                let v = frame[0];
                if !v.is_finite() {
                    nan = true;
                }
                peak = peak.max(v.abs());
                worst_delta = worst_delta.max((v - previous).abs());
                previous = v;
            }
        }
        let elapsed = started.elapsed().as_secs_f64();
        let realtime_factor = elapsed / 8.0;
        eprintln!(
            "spam stress: peak {peak:.3}, worst frame delta {worst_delta:.3}, \
             {:.1}% of realtime",
            realtime_factor * 100.0
        );
        assert!(!nan, "NaN in output");
        assert!(peak <= 0.98, "limiter ceiling breached: {peak}");
        // A frame-to-frame jump beyond ~0.5 at these levels is a click.
        assert!(
            worst_delta < 0.5,
            "click in spam output: delta {worst_delta}"
        );
        // Performance is only meaningful with optimizations; debug
        // builds run this same test for correctness only.
        if !cfg!(debug_assertions) {
            assert!(
                realtime_factor < 0.5,
                "engine too slow: {:.0}% of realtime in release",
                realtime_factor * 100.0
            );
        }
    }

    /// The user's exact worst case: ALL Great + Swell stops, Swell
    /// coupled to Great at 8' and 16'. ~25 pipes per key. This is the
    /// registration the engine must survive.
    #[test]
    fn full_organ_coupled_tutti_is_realtime() {
        let Some(path) = demo_organ() else {
            eprintln!("skipping: demo set not present");
            return;
        };
        let organ = aristide_formats::grandorgue::load(&path)
            .expect("demo set loads")
            .organ;
        let loaded = build(&organ, 48000.0).expect("bank builds");
        let great = organ.manuals[1].id;
        let swell = organ.manuals[2].id;
        let drawn: Vec<_> = organ
            .stops
            .iter()
            .filter(|s| {
                (s.manual == great || s.manual == swell) && !s.name.contains("noise")
            })
            .map(|s| s.id)
            .collect();
        // Swell→Great at unison and 16'.
        let couplers: Vec<usize> = organ
            .couplers
            .iter()
            .enumerate()
            .filter(|(_, c)| c.from_manual == great && c.to_manual == swell)
            .map(|(i, _)| i)
            .collect();
        assert!(couplers.len() >= 2, "need II/I and 16' II/I couplers");
        let mut console =
            crate::console::Console::new(organ, loaded.specs, drawn, Vec::new());
        for &c in &couplers {
            console.set_coupler(c, true);
        }
        // Production pre-faults at startup; match it or the first
        // strike measures page faults instead of the engine.
        let _ = loaded.bank.pre_fault();
        let (mut engine, mut handle) =
            aristide_engine::Engine::new(48000.0, std::sync::Arc::new(loaded.bank));

        let keys = [48u8, 50, 52, 53, 55, 57, 59, 60, 62, 64];
        let block = 256usize;
        let mut buffer = vec![0.0f32; block * 2];
        let mut voices_started = 0usize;
        let mut send_chord = |console: &mut crate::console::Console,
                              handle: &mut aristide_engine::EngineHandle,
                              on: bool| {
            for &key in &keys {
                if on {
                    let (starts, _) = console.note_on(0, key);
                    voices_started += starts.len();
                    for start in starts {
                        handle.send(aristide_engine::Command::StartVoice {
                            handle: start.handle,
                            sample: start.spec.sample,
                            rate: start.spec.rate,
                            gain: start.spec.gain,
                            group: start.spec.group,
                            wind_weight: start.spec.wind_weight,
                            brightness: start.spec.brightness,
                        });
                    }
                } else {
                    for h in console.note_off(0, key) {
                        handle.send(aristide_engine::Command::StopVoice { handle: h });
                    }
                }
            }
        };

        // 6 s: hold 1 s, release, re-strike every second (tails stack).
        let started = std::time::Instant::now();
        let mut worst_block = 0.0f64;
        let blocks = 6 * 48000 / block;
        for b in 0..blocks {
            let second = b * block / 48000;
            let phase_in_second = (b * block) % 48000;
            if phase_in_second < block {
                send_chord(&mut console, &mut handle, second % 2 == 0);
            }
            let t0 = std::time::Instant::now();
            engine.process(&mut buffer, 2);
            worst_block = worst_block.max(t0.elapsed().as_secs_f64());
        }
        let factor = started.elapsed().as_secs_f64() / 6.0;
        eprintln!(
            "coupled tutti: ~{} voices/chord, {:.1}% of realtime, worst block \
             {:.2} ms of {:.2} ms",
            voices_started / 3,
            factor * 100.0,
            worst_block * 1000.0,
            block as f64 / 48.0
        );
        if !cfg!(debug_assertions) {
            assert!(
                factor < 0.7,
                "coupled tutti not realtime-safe: {:.0}%",
                factor * 100.0
            );
        }
    }

    /// "Previous notes reappear and bang": hunt voice-resurrection.
    /// Play/release cycles under the coupled registration, then full
    /// silence — no audio may ever come back, and the engine's slot
    /// invariants must hold throughout.
    #[test]
    fn released_notes_never_resurrect() {
        let Some(path) = demo_organ() else {
            eprintln!("skipping: demo set not present");
            return;
        };
        let organ = aristide_formats::grandorgue::load(&path)
            .expect("demo set loads")
            .organ;
        let loaded = build(&organ, 48000.0).expect("bank builds");
        let great = organ.manuals[1].id;
        let swell = organ.manuals[2].id;
        let drawn: Vec<_> = organ
            .stops
            .iter()
            .filter(|s| {
                (s.manual == great || s.manual == swell) && !s.name.contains("noise")
            })
            .map(|s| s.id)
            .collect();
        let couplers: Vec<usize> = organ
            .couplers
            .iter()
            .enumerate()
            .filter(|(_, c)| c.from_manual == great && c.to_manual == swell)
            .map(|(i, _)| i)
            .collect();
        let mut console =
            crate::console::Console::new(organ, loaded.specs, drawn, Vec::new());
        for &c in &couplers {
            console.set_coupler(c, true);
        }
        let (mut engine, mut handle) =
            aristide_engine::Engine::new(48000.0, std::sync::Arc::new(loaded.bank));

        // F-major with octave doublings (shared pipes via 16' coupler),
        // struck and released 6 times with overlapping (legato) edges.
        let chord = [41u8, 45, 48, 53, 57, 60];
        let block = 256usize;
        let mut buffer = vec![0.0f32; block * 2];
        for cycle in 0..6 {
            for &key in &chord {
                let (starts, retriggered) = console.note_on(0, key);
                for h in retriggered {
                    handle.send(aristide_engine::Command::StopVoice { handle: h });
                }
                for start in starts {
                    handle.send(aristide_engine::Command::StartVoice {
                        handle: start.handle,
                        sample: start.spec.sample,
                        rate: start.spec.rate,
                        gain: start.spec.gain,
                        group: start.spec.group,
                        wind_weight: start.spec.wind_weight,
                        brightness: start.spec.brightness,
                    });
                }
                // Stagger key events across blocks like real playing.
                engine.process(&mut buffer, 2);
            }
            for _ in 0..40 {
                engine.process(&mut buffer, 2);
            }
            // Release in a different order than pressed (legato-ish).
            for &key in chord.iter().rev() {
                for h in console.note_off(0, key) {
                    handle.send(aristide_engine::Command::StopVoice { handle: h });
                }
                engine.process(&mut buffer, 2);
            }
            for _ in 0..(cycle % 3) * 10 {
                engine.process(&mut buffer, 2);
            }
            engine.assert_slot_invariants();
        }

        // All keys are up. Render 8 s: energy must decay to silence and
        // NEVER come back.
        let mut last_seconds_energy = Vec::new();
        for _ in 0..(8 * 48000 / block) {
            engine.process(&mut buffer, 2);
            last_seconds_energy.push(buffer.iter().map(|v| (*v as f64) * (*v as f64)).sum::<f64>());
        }
        engine.assert_slot_invariants();
        let blocks_per_second = 48000 / block;
        let second_energy: Vec<f64> = last_seconds_energy
            .chunks(blocks_per_second)
            .map(|c| c.iter().sum())
            .collect();
        // Tails run ~4 s; seconds 6-8 must be silent.
        assert!(
            second_energy[6] < 1e-9 && second_energy[7] < 1e-9,
            "audio persists/returns after full release: {second_energy:?}"
        );
        // And strictly no resurgence: every second quieter than the one
        // two seconds before it.
        for i in 2..second_energy.len() {
            assert!(
                second_energy[i] <= second_energy[i - 2] + 1e-9,
                "energy resurged at second {i}: {second_energy:?}"
            );
        }
    }

    /// The reported "awful pop on mass release": releasing a big chord
    /// doubles those voices' cost at once (crossfade = two sinc reads).
    /// The pallet stagger + SIMD must keep every block under budget.
    #[test]
    fn mass_release_stays_under_block_budget() {
        let Some(path) = demo_organ() else {
            eprintln!("skipping: demo set not present");
            return;
        };
        let organ = aristide_formats::grandorgue::load(&path)
            .expect("demo set loads")
            .organ;
        let loaded = build(&organ, 48000.0).expect("bank builds");
        let manual_id = organ.manuals[1].id;
        let drawn: Vec<_> = organ
            .stops
            .iter()
            .filter(|s| s.manual == manual_id && !s.name.contains("noise"))
            .map(|s| s.id)
            .collect();
        let mut console =
            crate::console::Console::new(organ, loaded.specs, drawn, Vec::new());
        let (mut engine, mut handle) =
            aristide_engine::Engine::new(48000.0, std::sync::Arc::new(loaded.bank));

        // Hold a 10-key chord over EVERY Great stop, settle, then
        // release everything in one burst.
        let keys = [48u8, 50, 52, 53, 55, 57, 59, 60, 62, 64];
        for &key in &keys {
            let (starts, _) = console.note_on(0, key);
            for start in starts {
                handle.send(aristide_engine::Command::StartVoice {
                    handle: start.handle,
                    sample: start.spec.sample,
                    rate: start.spec.rate,
                    gain: start.spec.gain,
                    group: start.spec.group,
                    wind_weight: start.spec.wind_weight,
                    brightness: start.spec.brightness,
                });
            }
        }
        let block = 256usize;
        let mut buffer = vec![0.0f32; block * 2];
        for _ in 0..64 {
            engine.process(&mut buffer, 2);
        }
        for &key in &keys {
            for handle_id in console.note_off(0, key) {
                handle.send(aristide_engine::Command::StopVoice { handle: handle_id });
            }
        }
        // Watch half a second of blocks through the release storm.
        let budget = block as f64 / 48000.0;
        let mut worst = 0.0f64;
        for _ in 0..(24000 / block) {
            let started = std::time::Instant::now();
            engine.process(&mut buffer, 2);
            worst = worst.max(started.elapsed().as_secs_f64());
        }
        eprintln!(
            "mass release: worst block {:.2} ms of {:.2} ms budget",
            worst * 1000.0,
            budget * 1000.0
        );
        if !cfg!(debug_assertions) {
            assert!(
                worst < budget * 0.8,
                "release storm blows the block budget: {:.2} ms",
                worst * 1000.0
            );
        }
    }

    /// The whole M3 pipeline, headless: ODF → model → bank → console →
    /// RT engine → nonzero audio frames.
    #[test]
    fn demo_set_plays_end_to_end() {
        let Some(path) = demo_organ() else {
            eprintln!("skipping: demo set not present");
            return;
        };
        let organ = aristide_formats::grandorgue::load(&path)
            .expect("demo set loads")
            .organ;
        let loaded = build(&organ, 48000.0).expect("bank builds");
        assert!(loaded.skipped.is_empty(), "skipped: {:?}", loaded.skipped);
        // Every sampled and borrowed pipe got a playback spec.
        assert_eq!(loaded.specs.len(), 853 + 497, "spec count");

        // Default channel map: channel 0 → the Great (manuals[1], since
        // the pedal is manuals[0]). Draw its first stop, press middle C.
        let manual_id = organ.manuals[1].id;
        let drawn = vec![
            organ
                .stops
                .iter()
                .find(|s| s.manual == manual_id)
                .expect("manual has stops")
                .id,
        ];
        let mut console = crate::console::Console::new(organ, loaded.specs, drawn, Vec::new());
        let (starts, _) = console.note_on(0, 60);
        assert!(!starts.is_empty(), "middle C should sound");

        let (mut engine, mut handle) =
            aristide_engine::Engine::new(48000.0, std::sync::Arc::new(loaded.bank));
        for start in &starts {
            assert!(handle.send(aristide_engine::Command::StartVoice {
                handle: start.handle,
                sample: start.spec.sample,
                rate: start.spec.rate,
                gain: start.spec.gain,
                group: start.spec.group,
                wind_weight: start.spec.wind_weight,
                brightness: start.spec.brightness,
            }));
        }
        let mut buffer = vec![0.0f32; 4800 * 2];
        engine.process(&mut buffer, 2);
        let energy: f32 = buffer.iter().map(|v| v * v).sum();
        assert!(energy > 0.0, "the organ should make sound");

        // Release: voices splice to their tails and eventually go quiet.
        for handle_id in console.note_off(0, 60) {
            handle.send(aristide_engine::Command::StopVoice { handle: handle_id });
        }
        // Long releases: give it a generous 30 s of rendering.
        for _ in 0..300 {
            engine.process(&mut buffer, 2);
        }
        let energy: f32 = buffer.iter().map(|v| v * v).sum();
        assert_eq!(energy, 0.0, "voices should have ended after release");
    }

    /// The user's machine, numerically: 44.1 kHz device rate, full
    /// Great+Swell coupled at 8'+16', PRODUCTION release stagger (every
    /// other cleanliness test zeroes it — the staggered
    /// pending_release→release() mid-block path has never been
    /// click-scanned), and a realistic playing schedule: fast spam,
    /// mass chord press/release, press-release-repress, trills. Scans
    /// the output for single-sample steps far above local signal level
    /// and writes /tmp/crackle_hunt.wav for listening/inspection.
    /// This is the test that caught the shed-tail loop-teleport bug
    /// (2026-08-11): FadeOut voices in release material were recaptured
    /// by the sustain-loop wrap and jumped the cursor back into
    /// full-level sustain — every user-facing crackle/pop/ghost-note
    /// symptom traced to it. ~5 s in release; skips without the demo set.
    #[test]
    fn crackle_hunt_under_realistic_fast_playing() {
        let Some(path) = demo_organ() else {
            eprintln!("skipping: demo set not present");
            return;
        };
        let organ = aristide_formats::grandorgue::load(&path)
            .expect("demo set loads")
            .organ;
        let device_rate = 44_100.0f32;
        let loaded = build(&organ, device_rate).expect("bank builds");
        let great = organ.manuals[1].id;
        let swell = organ.manuals[2].id;
        let drawn: Vec<_> = organ
            .stops
            .iter()
            .filter(|s| {
                (s.manual == great || s.manual == swell) && !s.name.contains("noise")
            })
            .map(|s| s.id)
            .collect();
        let couplers: Vec<usize> = organ
            .couplers
            .iter()
            .enumerate()
            .filter(|(_, c)| c.from_manual == great && c.to_manual == swell)
            .map(|(i, _)| i)
            .collect();
        let mut console =
            crate::console::Console::new(organ, loaded.specs, drawn, Vec::new());
        for &c in &couplers {
            console.set_coupler(c, true);
        }
        let _ = loaded.bank.pre_fault();
        let (mut engine, mut handle) =
            aristide_engine::Engine::new(device_rate, std::sync::Arc::new(loaded.bank));
        // NOTE: release stagger stays at the production default.

        // Deterministic schedule of (frame, key, on) events.
        let sr = device_rate as usize;
        let mut events: Vec<(usize, u8, bool)> = Vec::new();
        let mut rng = 0xA5F1_5EEDu32;
        let mut rand = move |n: usize| {
            rng ^= rng << 13;
            rng ^= rng >> 17;
            rng ^= rng << 5;
            (rng as usize) % n
        };
        let spam_keys = [48u8, 50, 52, 53, 55, 57, 59, 60, 62, 64, 65, 67, 69, 72];
        // Phase A, 0-3 s: fast spam — a new key every 30-90 ms, each held 60-200 ms.
        let mut t = sr / 10;
        while t < 3 * sr {
            let key = spam_keys[rand(spam_keys.len())];
            let hold = sr * (60 + rand(140)) / 1000;
            events.push((t, key, true));
            events.push((t + hold, key, false));
            t += sr * (30 + rand(60)) / 1000;
        }
        // Phase B, 3-6 s: mass F-major chord, hold, release all at once,
        // then 150 ms later press one coupler-sharing key (his exact
        // press-release-repress report), twice.
        let chord = [53u8, 57, 60, 65, 69, 72];
        for round in 0..2usize {
            let base = 3 * sr + round * (3 * sr / 2);
            for &k in &chord {
                events.push((base, k, true));
            }
            for &k in &chord {
                events.push((base + sr * 4 / 5, k, false));
            }
            events.push((base + sr * 4 / 5 + sr * 15 / 100, 67, true));
            events.push((base + sr * 4 / 5 + sr * 45 / 100, 67, false));
        }
        // Phase C, 6-8 s: trills — alternate two keys every 40 ms.
        let mut t = 6 * sr;
        let mut which = false;
        while t < 8 * sr {
            let key = if which { 60 } else { 62 };
            events.push((t, key, true));
            events.push((t + sr * 35 / 1000, key, false));
            which = !which;
            t += sr * 40 / 1000;
        }
        events.sort_by_key(|e| e.0);

        // Phase D, 8-9 s: one more mass chord released into a LONG quiet
        // decay — the user's recorded glitches cluster in the tail era.
        for &k in &chord {
            events.push((8 * sr + sr / 10, k, true));
        }
        for &k in &chord {
            events.push((9 * sr, k, false));
        }

        // Render 16 s in 512-frame blocks, events applied between blocks
        // (as a real MIDI thread would deliver them).
        let block = 512usize;
        let total_frames = sr * 16;
        let mut output = Vec::with_capacity(total_frames * 2);
        let mut buffer = vec![0.0f32; block * 2];
        let mut next_event = 0usize;
        let mut frame = 0usize;
        let mut limited_blocks = 0usize;
        let mut total_blocks = 0usize;
        let mut worst_reduction_db = 0.0f32;
        while frame < total_frames {
            while next_event < events.len() && events[next_event].0 < frame + block {
                let (_, key, on) = events[next_event];
                next_event += 1;
                if on {
                    let (starts, retriggered) = console.note_on(0, key);
                    for h in retriggered {
                        assert!(handle.send(aristide_engine::Command::StopVoice { handle: h }));
                    }
                    for start in starts {
                        assert!(handle.send(aristide_engine::Command::StartVoice {
                            handle: start.handle,
                            sample: start.spec.sample,
                            rate: start.spec.rate,
                            gain: start.spec.gain,
                            group: start.spec.group,
                            wind_weight: start.spec.wind_weight,
                            brightness: start.spec.brightness,
                        }));
                    }
                } else {
                    for h in console.note_off(0, key) {
                        assert!(handle.send(aristide_engine::Command::StopVoice { handle: h }));
                    }
                }
            }
            engine.process(&mut buffer, 2);
            output.extend_from_slice(&buffer);
            let reduction = engine.limiter_gain_db();
            if reduction < 0.0 {
                limited_blocks += 1;
                worst_reduction_db = worst_reduction_db.min(reduction);
            }
            total_blocks += 1;
            frame += block;
        }
        println!(
            "limiter: engaged in {limited_blocks}/{total_blocks} blocks, \
             worst reduction {worst_reduction_db:.1} dB"
        );

        // Write the take for by-ear/DAW inspection.
        write_wav_f32("/tmp/crackle_hunt.wav", &output, 2, sr as u32);

        // Click scan per channel: an outlier in the SECOND difference
        // against its own local statistics. This is what found the
        // one-frame impulses in the user's recorded take — a plain
        // step-vs-signal-RMS scan is deaf to a ±0.02 impulse inside
        // loud content, but d2 of band-limited audio is smooth and a
        // single wrong frame sticks out 12x+ in any context.
        let mut clicks: Vec<(f64, f32, f32)> = Vec::new(); // (sec, d2, local)
        for ch in 0..2usize {
            let mut d2_rms_sq = 1e-9f64;
            const ALPHA: f64 = 1.0 / 256.0;
            let mut x1 = 0.0f32;
            let mut x2 = 0.0f32;
            for (i, frame_index) in (ch..output.len()).step_by(2).enumerate() {
                let x = output[frame_index];
                let d2 = (x - 2.0 * x1 + x2).abs();
                let local = (d2_rms_sq.sqrt() as f32).max(1e-5);
                // Floor 0.008: teleport-class defects measured 0.02-0.09,
                // and the crossfade-completion double-gain dip measured
                // 0.008-0.014 — both must stay dead. Natural content under
                // this schedule stays well below the 12x-local gate.
                if i > 512 && d2 > (12.0 * local).max(0.008) {
                    clicks.push((i as f64 / sr as f64, d2, local));
                }
                d2_rms_sq += ALPHA * ((d2 as f64) * (d2 as f64) - d2_rms_sq);
                x2 = x1;
                x1 = x;
            }
        }
        clicks.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());
        clicks.dedup_by(|a, b| (a.0 - b.0).abs() < 0.005);
        println!("clicks found: {}", clicks.len());
        for (sec, delta, rms) in clicks.iter().take(25) {
            let near: Vec<String> = events
                .iter()
                .filter(|e| (e.0 as f64 / sr as f64 - sec).abs() < 0.03)
                .map(|e| format!("{}{}", if e.2 { "+" } else { "-" }, e.1))
                .collect();
            println!(
                "  t={sec:.3}s step={delta:.4} rms={rms:.4} events±30ms={}",
                near.join(",")
            );
        }
        assert!(
            clicks.is_empty(),
            "{} discontinuities in engine output (see /tmp/crackle_hunt.wav)",
            clicks.len()
        );
    }

    fn write_wav_f32(path: &str, samples: &[f32], channels: u16, rate: u32) {
        use std::io::Write as _;
        let mut f = std::fs::File::create(path).expect("wav create");
        let data_len = (samples.len() * 4) as u32;
        let byte_rate = rate * channels as u32 * 4;
        let mut header = Vec::with_capacity(44);
        header.extend_from_slice(b"RIFF");
        header.extend_from_slice(&(36 + data_len).to_le_bytes());
        header.extend_from_slice(b"WAVEfmt ");
        header.extend_from_slice(&16u32.to_le_bytes());
        header.extend_from_slice(&3u16.to_le_bytes()); // IEEE float
        header.extend_from_slice(&channels.to_le_bytes());
        header.extend_from_slice(&rate.to_le_bytes());
        header.extend_from_slice(&byte_rate.to_le_bytes());
        header.extend_from_slice(&(channels * 4).to_le_bytes());
        header.extend_from_slice(&32u16.to_le_bytes());
        header.extend_from_slice(b"data");
        header.extend_from_slice(&data_len.to_le_bytes());
        f.write_all(&header).expect("wav header");
        let mut bytes = Vec::with_capacity(samples.len() * 4);
        for s in samples {
            bytes.extend_from_slice(&s.to_le_bytes());
        }
        f.write_all(&bytes).expect("wav data");
    }
}
