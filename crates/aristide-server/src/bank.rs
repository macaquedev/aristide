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
}
