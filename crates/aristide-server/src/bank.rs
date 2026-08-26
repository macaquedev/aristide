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
    /// Source frames per output frame, playing the pipe at its own
    /// nominal pitch on this device.
    pub rate: f32,
    /// The pitch that rate sounds, in Hz. Repitching a pipe onto a key
    /// it was not recorded for is a ratio against this.
    pub nominal_hz: f32,
    /// Linear gain.
    pub gain: f32,
    /// The rank's velocity→volume ramp; the console multiplies its
    /// value for the press's velocity into `gain` at note-on.
    pub velocity: aristide_model::VelocityVolume,
    /// Loop-less percussive samples get no StopVoice on key release.
    pub percussive: bool,
    /// Wind group (0-based engine index, from the ODF windchest).
    pub group: u8,
    /// Wind draw while sounding; 0 for noises and percussives.
    pub wind_weight: f32,
    /// Tilt-filter coefficient for pressure→brightness coupling
    /// (0 = no filter, e.g. noises).
    pub brightness: f32,
    /// Swell box (0-based engine index from the ODF windchest
    /// membership; [`aristide_engine::enclosure::ENCLOSURE_NONE`] for
    /// unenclosed divisions).
    pub enclosure: u8,
    /// Output bus (0 = the main pair). Specs are built per pipe with
    /// the defaults; the console stamps these per stop from the
    /// sidecar's `[routing]`/`[voicing]` before a voice starts.
    pub bus: u8,
    /// Onset (speaking) delay in output frames.
    pub delay_frames: u32,
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

    // Windchest number → enclosure engine index. A voice carries ONE
    // enclosure; GO multiplies when a chest sits in several boxes, but
    // no real set seen does — warn and take the first.
    let mut chest_enclosures: HashMap<u32, u8> = HashMap::new();
    for chest in &organ.windchests {
        let Some(&first) = chest.enclosures.first() else {
            continue;
        };
        if chest.enclosures.len() > 1 {
            skipped.push(format!(
                "windchest {} ({}) sits in {} enclosures; using the first",
                chest.number,
                chest.name,
                chest.enclosures.len()
            ));
        }
        let index = (first as usize).min(aristide_engine::enclosure::MAX_ENCLOSURES - 1) as u8;
        chest_enclosures.insert(chest.number, index);
    }

    for rank in &organ.ranks {
        let enclosure = chest_enclosures
            .get(&rank.windchest)
            .copied()
            .unwrap_or(aristide_engine::enclosure::ENCLOSURE_NONE);
        // Pipes decode first, then pitch decisions settle rank-wide
        // (the junk-metadata guard below needs the whole rank in view)
        // before specs are built.
        let mut pending: Vec<PendingPipe> = Vec::new();
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
            let Some(info) = *entry else { continue };

            // Where the recording's pitch claim comes from: an explicit
            // ODF MIDIKeyNumber wins (and silences the file's own
            // fraction — GO's rule), else the file's smpl chunk.
            let (sample_key, fraction_cents, from_smpl) =
                match (pipe.midi_key_number, pipe.midi_pitch_fraction_cents) {
                    (Some(key), fraction) => (Some(key), fraction.unwrap_or(0.0), false),
                    (None, Some(fraction)) => (info.unity_note, fraction, true),
                    (None, None) => (info.unity_note, info.unity_fraction_cents, true),
                };
            let original_cents = pipe.pitch_tuning_cents + attack.pitch_offset_cents;
            let auto_cents = sample_key.map(|key| {
                let recorded_hz = ladder_hz(key as f64 + fraction_cents / 100.0);
                1200.0 * (pipe.nominal_frequency_hz / recorded_hz).log2()
                    + pipe.pitch_correction_cents
                    + attack.pitch_offset_cents
            });
            pending.push(PendingPipe {
                pipe_index: pipe_index as u16,
                info,
                path: attack.path.clone(),
                original_cents,
                auto_cents,
                from_smpl,
                unity: from_smpl.then_some(sample_key).flatten(),
            });
        }

        // Junk-metadata guard: several *distinct* files all claiming
        // the same smpl unity note across a rank whose slots span
        // different pitches is an editor's default (unity=60 stamped
        // everywhere), not a measurement — no honest rank records two
        // different keys at one pitch. Distrust the whole rank's smpl
        // pitch (explicit ODF MIDIKeyNumber declarations still count).
        let smpl_claims: HashMap<&PathBuf, u8> = pending
            .iter()
            .filter_map(|p| p.unity.map(|unity| (&p.path, unity)))
            .collect();
        let one_unity = smpl_claims.len() >= 3
            && smpl_claims.values().collect::<std::collections::HashSet<_>>().len() == 1;
        let distrust_smpl = one_unity && {
            let nominals: Vec<f64> = pending
                .iter()
                .filter(|p| p.unity.is_some())
                .map(|p| rank.pipes[p.pipe_index as usize].nominal_frequency_hz)
                .collect();
            nominals.iter().any(|&hz| (hz - nominals[0]).abs() > 1e-6)
        };
        if distrust_smpl {
            skipped.push(format!(
                "{}: ignoring embedded pitch metadata (distinct files share one \
                 unity note across differing keys — an editor default, not a \
                 measurement)",
                rank.name
            ));
        }

        let mut retuned = 0usize;
        let mut retuned_max_cents = 0f64;
        for p in pending {
            let pipe = &rank.pipes[p.pipe_index as usize];
            // The recording plays as the set voiced it (as recorded +
            // PitchTuning) unless its own declared pitch says that
            // lands somewhere else entirely — then the set relies on
            // retuning from metadata (unit/extended ranks, borrowed
            // top octaves, HW-style sets), which used to play wrongly
            // and silently here. Within the tolerance the declared
            // pitch and the voicing agree, and the recorded tuning —
            // the organ's actual temperament and drift — is kept
            // rather than flattened onto the equal ladder.
            let cents = match p.auto_cents {
                Some(auto) if (auto - p.original_cents).abs() > RETUNE_TOLERANCE_CENTS => {
                    if auto.abs() > 1800.0 {
                        // GO refuses retunes past 1800 cents; a claim
                        // that far out is junk metadata, not intent.
                        skipped.push(format!(
                            "{} pipe {}: embedded pitch asks for a {auto:.0}-cent \
                             retune; ignored",
                            rank.name, p.pipe_index
                        ));
                        p.original_cents
                    } else if p.from_smpl && distrust_smpl {
                        p.original_cents
                    } else {
                        retuned += 1;
                        retuned_max_cents = retuned_max_cents.max((auto - p.original_cents).abs());
                        auto
                    }
                }
                _ => p.original_cents,
            };
            specs.insert(
                (rank.id, p.pipe_index),
                VoiceSpec {
                    sample: p.info.index,
                    rate: (p.info.sample_rate / device_rate as f64
                        * (cents / 1200.0).exp2()) as f32,
                    nominal_hz: pipe.nominal_frequency_hz as f32,
                    gain: db_to_linear(pipe.gain_db),
                    velocity: rank.velocity_volume,
                    percussive: p.info.percussive,
                    group: (rank.windchest.saturating_sub(1))
                        .min(aristide_engine::wind::MAX_WIND_GROUPS as u32 - 1)
                        as u8,
                    wind_weight: wind_weight(pipe.nominal_frequency_hz, p.info.percussive),
                    brightness: brightness_coefficient(
                        pipe.nominal_frequency_hz,
                        device_rate,
                        p.info.percussive,
                    ),
                    enclosure,
                    bus: 0,
                    delay_frames: 0,
                },
            );
        }
        if retuned > 0 {
            skipped.push(format!(
                "{}: {retuned} pipe(s) retuned to their recorded-pitch metadata \
                 (largest shift {retuned_max_cents:.0} cents)",
                rank.name
            ));
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

/// How far a recording's declared pitch may sit from where the set's
/// voicing puts it before we believe the set *relies* on metadata
/// retuning. Under this, the difference is the organ's own recorded
/// tuning (temperament, drift — tens of cents) and is kept; over it,
/// the sample sits at another key entirely (unit/extended ranks reuse
/// on the semitone grid, ≥100 cents) and playing it as voiced would be
/// wrong by that much, silently.
const RETUNE_TOLERANCE_CENTS: f64 = 50.0;

/// One sampled pipe awaiting its rank-wide pitch decision.
struct PendingPipe {
    pipe_index: u16,
    info: DecodedInfo,
    path: PathBuf,
    /// Playback offset as the set voiced it: PitchTuning et al.
    original_cents: f64,
    /// Playback offset that lands the recording's *declared* pitch on
    /// the pipe's nominal (GO's auto-tuning formula, PitchCorrection
    /// folded in); `None` when nothing declares a pitch.
    auto_cents: Option<f64>,
    /// Whether the declaration came from the file's smpl chunk rather
    /// than the ODF — only smpl claims fall to the junk guard.
    from_smpl: bool,
    /// The smpl unity note backing `auto_cents`, for the junk guard.
    unity: Option<u8>,
}

/// The 12-EDO/A440 MIDI ladder that `smpl` unity notes and ODF
/// MIDIKeyNumber values are defined against. A format fact about the
/// metadata, not the tuning seam — live key→pitch policy stays in
/// `tuning.rs`.
fn ladder_hz(midi: f64) -> f64 {
    440.0 * ((midi - 69.0) / 12.0).exp2()
}

#[derive(Clone, Copy)]
struct DecodedInfo {
    index: u32,
    sample_rate: f64,
    percussive: bool,
    /// The file's own claim of what pitch it holds: `smpl`-chunk unity
    /// note (0 = "not set", as GO reads it) plus its fraction in cents
    /// above that note.
    unity_note: Option<u8>,
    unity_fraction_cents: f64,
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
            unity_note: file.info.midi_unity_note.filter(|&note| note != 0),
            // GO: dwMIDIPitchFraction / UINT_MAX × 100 cents.
            unity_fraction_cents: file
                .info
                .pitch_fraction
                .map(|fraction| fraction as f64 / u32::MAX as f64 * 100.0)
                .unwrap_or(0.0),
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
pub(crate) fn brightness_coefficient(frequency_hz: f64, device_rate: f32, percussive: bool) -> f32 {
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
pub(crate) fn wind_weight(frequency_hz: f64, percussive: bool) -> f32 {
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

    /// The manual MIDI channel `channel` used to reach under the old
    /// keyboards-first default map: a pedalboard, when present, is
    /// `manuals[0]` and sits at the end.
    fn default_manual(organ: &aristide_model::Organ, channel: u8) -> usize {
        let count = organ.manuals.len();
        if count == 0 {
            return 0;
        }
        let map: Vec<usize> = if count > 1 && organ.manuals[0].id == aristide_model::ManualId(0) {
            (1..count).chain(std::iter::once(0)).collect()
        } else {
            (0..count).collect()
        };
        map[channel as usize % map.len()]
    }

    /// A minimal mono 16-bit WAV with an `smpl` chunk claiming `unity`
    /// (0 = write no smpl chunk) and one sustain loop, written to
    /// `path`.
    fn write_test_wav(path: &Path, unity: u8) {
        let frames: u32 = 512;
        let mut bytes = Vec::new();
        let mut chunk = |id: &[u8; 4], payload: &[u8]| {
            bytes.extend_from_slice(id);
            bytes.extend_from_slice(&(payload.len() as u32).to_le_bytes());
            bytes.extend_from_slice(payload);
        };
        let mut fmt = Vec::new();
        fmt.extend_from_slice(&1u16.to_le_bytes()); // PCM
        fmt.extend_from_slice(&1u16.to_le_bytes()); // mono
        fmt.extend_from_slice(&44_100u32.to_le_bytes());
        fmt.extend_from_slice(&(44_100u32 * 2).to_le_bytes());
        fmt.extend_from_slice(&2u16.to_le_bytes());
        fmt.extend_from_slice(&16u16.to_le_bytes());
        chunk(b"fmt ", &fmt);
        if unity != 0 {
            let mut smpl = vec![0u8; 36];
            smpl[12..16].copy_from_slice(&(unity as u32).to_le_bytes());
            // fraction 0, one loop 64..=447
            smpl[28..32].copy_from_slice(&1u32.to_le_bytes());
            let mut record = [0u8; 24];
            record[8..12].copy_from_slice(&64u32.to_le_bytes());
            record[12..16].copy_from_slice(&447u32.to_le_bytes());
            smpl.extend_from_slice(&record);
            chunk(b"smpl", &smpl);
        }
        let mut pcm = Vec::new();
        for i in 0..frames {
            let value = (f64::sin(i as f64 * 0.1) * 8000.0) as i16;
            pcm.extend_from_slice(&value.to_le_bytes());
        }
        chunk(b"data", &pcm);
        let mut file = Vec::new();
        file.extend_from_slice(b"RIFF");
        file.extend_from_slice(&((bytes.len() + 4) as u32).to_le_bytes());
        file.extend_from_slice(b"WAVE");
        file.extend_from_slice(&bytes);
        std::fs::write(path, file).expect("write test wav");
    }

    /// A one-rank organ over `pipes` = (file name, unity note,
    /// nominal MIDI key, PitchTuning cents, ODF MIDIKeyNumber), with
    /// files created in a fresh temp dir.
    fn pitch_test_organ(
        tag: &str,
        pipes: &[(&str, u8, f64, f64, Option<u8>)],
    ) -> aristide_model::Organ {
        let dir = std::env::temp_dir().join(format!("aristide-pitch-test-{tag}"));
        std::fs::create_dir_all(&dir).expect("test dir");
        let mut rank_pipes = Vec::new();
        for &(name, unity, nominal_midi, tuning_cents, odf_key) in pipes {
            write_test_wav(&dir.join(name), unity);
            rank_pipes.push(aristide_model::Pipe {
                nominal_frequency_hz: 440.0 * ((nominal_midi - 69.0) / 12.0).exp2(),
                pitch_tuning_cents: tuning_cents,
                pitch_correction_cents: 0.0,
                gain_db: 0.0,
                midi_key_number: odf_key,
                midi_pitch_fraction_cents: None,
                source: aristide_model::PipeSource::Sampled {
                    attacks: vec![aristide_model::AttackSample {
                        path: PathBuf::from(name),
                        ..Default::default()
                    }],
                    releases: Vec::new(),
                },
            });
        }
        aristide_model::Organ {
            name: format!("pitch test {tag}"),
            base_path: dir,
            ranks: vec![aristide_model::Rank {
                id: aristide_model::RankId(1),
                name: "Test rank".into(),
                windchest: 1,
                velocity_volume: Default::default(),
                pipes: rank_pipes,
            }],
            ..Default::default()
        }
    }

    fn rate_cents(loaded: &LoadedBank, pipe: u16) -> f64 {
        let spec = loaded.specs.get(&(aristide_model::RankId(1), pipe)).expect("spec");
        // File and device rates match, so the rate is purely the fold.
        1200.0 * (spec.rate as f64).log2()
    }

    /// The §6 bug class: recordings whose declared pitch sits at
    /// another key entirely retune to their slot; declarations that
    /// agree with the voicing (within the organ's own tuning) keep the
    /// recorded character; absurd claims are refused.
    #[test]
    fn recorded_pitch_metadata_reconciles() {
        let organ = pitch_test_organ(
            "reconcile",
            &[
                // smpl claims 60, slot wants 57, nothing voiced: the
                // set relies on retuning — three semitones down.
                ("borrowed.wav", 60, 57.0, 0.0, None),
                // smpl agrees with the slot; +30 cents of voiced
                // PitchTuning is recorded character, kept verbatim.
                ("voiced.wav", 60, 60.0, 30.0, None),
                // ODF declares the recording an octave below the slot.
                ("odf-key.wav", 0, 60.0, 0.0, Some(48)),
                // smpl claims 8 octaves off: junk, refused.
                ("junk.wav", 127, 30.0, 0.0, None),
            ],
        );
        let loaded = build(&organ, 44_100.0).expect("bank builds");
        assert!((rate_cents(&loaded, 0) - -300.0).abs() < 1.0, "auto retune");
        assert!((rate_cents(&loaded, 1) - 30.0).abs() < 1.0, "voicing kept");
        assert!((rate_cents(&loaded, 2) - 1200.0).abs() < 1.0, "ODF key retune");
        assert!(rate_cents(&loaded, 3).abs() < 1.0, "junk refused");
        assert!(
            loaded.skipped.iter().any(|note| note.contains("retuned")),
            "retunes are reported: {:?}",
            loaded.skipped
        );
        assert!(
            loaded.skipped.iter().any(|note| note.contains("ignored")),
            "refusals are reported: {:?}",
            loaded.skipped
        );
    }

    /// Distinct files all claiming one unity note across a rank whose
    /// slots differ is an editor default, not a measurement — the rank
    /// keeps its voiced tuning and says why.
    #[test]
    fn junk_unity_notes_are_distrusted_rank_wide() {
        let organ = pitch_test_organ(
            "junk-unity",
            &[
                ("a.wav", 60, 55.0, 0.0, None),
                ("b.wav", 60, 60.0, 0.0, None),
                ("c.wav", 60, 65.0, 0.0, None),
            ],
        );
        let loaded = build(&organ, 44_100.0).expect("bank builds");
        for pipe in 0..3 {
            assert!(
                rate_cents(&loaded, pipe).abs() < 1.0,
                "pipe {pipe} must play as recorded"
            );
        }
        assert!(
            loaded
                .skipped
                .iter()
                .any(|note| note.contains("ignoring embedded pitch")),
            "guard is reported: {:?}",
            loaded.skipped
        );
    }

    /// The demo set's own metadata agrees with its voicing everywhere
    /// (its repitch grid is encoded as PitchTuning and its smpl chunks
    /// are honest), so reconciliation must not move a single pipe —
    /// the organ keeps its recorded tuning. And harmonics reach the
    /// nominal: the Plein jeu's first rank is pitched 2 octaves up
    /// (HarmonicNumber=32) from its C2 key.
    #[test]
    fn demo_set_keeps_its_recorded_tuning() {
        let Some(path) = demo_organ() else {
            eprintln!("skipping: demo set not present");
            return;
        };
        let organ = aristide_formats::grandorgue::load(&path).expect("loads").organ;
        let loaded = build(&organ, 44_100.0).expect("bank builds");
        assert!(
            !loaded
                .skipped
                .iter()
                .any(|note| note.contains("retuned") || note.contains("embedded pitch")),
            "demo pipes must all keep their voiced tuning: {:?}",
            loaded.skipped
        );
        let plein_jeu = organ
            .ranks
            .iter()
            .find(|rank| rank.name.contains("Plein jeu 1st"))
            .expect("plein jeu rank");
        let c4 = 440.0 * ((60.0 - 69.0) / 12.0f64).exp2();
        assert!(
            (plein_jeu.pipes[0].nominal_frequency_hz - c4).abs() < 1e-6,
            "C2 key at harmonic 32 sounds C4, got {}",
            plein_jeu.pipes[0].nominal_frequency_hz
        );
        // …and its −600-cent repitch grid is untouched.
        let spec = loaded.specs.get(&(plein_jeu.id, 0)).expect("spec");
        assert!(((spec.rate as f64).log2() * 1200.0 + 600.0).abs() < 1.0);
    }

    /// The demo set's two ODF enclosures must reach the voice specs:
    /// Récit chest (3) → enclosure 0, enclosed Great chest (2) →
    /// enclosure 1, unenclosed chest (1) → none. And an expression
    /// pedal on the Récit channel must drive the Récit box.
    #[test]
    fn demo_enclosures_reach_specs_and_expression() {
        let Some(path) = demo_organ() else {
            eprintln!("skipping: demo set not present");
            return;
        };
        let organ = aristide_formats::grandorgue::load(&path).expect("loads").organ;
        assert_eq!(organ.enclosures.len(), 2);
        assert_eq!(organ.enclosures[0].name, "Recit");
        assert_eq!(organ.enclosures[0].amp_minimum_level, 20.0);
        assert_eq!(organ.enclosures[1].amp_minimum_level, 30.0);
        let chest = |n: u32| organ.windchests.iter().find(|c| c.number == n).unwrap();
        assert_eq!(chest(1).enclosures, Vec::<u32>::new());
        assert_eq!(chest(2).enclosures, vec![1]);
        assert_eq!(chest(3).enclosures, vec![0]);

        let loaded = build(&organ, 44_100.0).expect("bank builds");
        let spec_for = |pattern: &str| {
            let stop = organ
                .stops
                .iter()
                .find(|s| s.name.contains(pattern))
                .unwrap_or_else(|| panic!("stop {pattern}"));
            let range = stop.ranks.first().expect("ranks");
            loaded
                .specs
                .get(&(range.rank, range.first_pipe))
                .copied()
                .unwrap_or_else(|| panic!("spec for {pattern}"))
        };
        assert_eq!(spec_for("Hautbois").enclosure, 0);
        assert_eq!(spec_for("Plein jeu III").enclosure, 1);
        assert_eq!(
            spec_for("Montre").enclosure,
            aristide_engine::enclosure::ENCLOSURE_NONE
        );

        // Channel 0 → Second Manual (Récit): the pedal reaches box 0.
        let mut console =
            crate::console::Console::new(organ.clone(), loaded.specs.clone(), Vec::new(), 44_100.0);
        let moves = console.expression_manual(2, 64);
        assert!(
            moves.iter().any(|&(e, p)| e == 0 && (p - 64.0 / 127.0).abs() < 1e-6),
            "Récit pedal did not move box 0: {moves:?}"
        );
    }

    /// The demo set loaded twice as an implicit composite must build
    /// one bank with disjoint voice specs for every copy — the
    /// collision this guards against is silent (two organs' `RankId`s
    /// aliasing in `specs`) — while identical recordings still decode
    /// once, and each copy's pipes sit in that copy's own enclosures.
    #[test]
    fn merged_demo_twice_builds_disjoint_specs_sharing_samples() {
        let Some(path) = demo_organ() else {
            eprintln!("skipping: demo set not present");
            return;
        };
        let load = || aristide_formats::grandorgue::load(&path).expect("loads").organ;
        let single = build(&load(), 44_100.0).expect("bank builds");
        let implicit = aristide_formats::instrument::Definition {
            name: "Twice".into(),
            ..Default::default()
        };
        let sources = vec![("A".to_string(), load()), ("B".to_string(), load())];
        let organ = aristide_formats::instrument::assemble(&implicit, &sources, Vec::new())
            .expect("assembles")
            .organ;
        assert_eq!(organ.stops.len(), load().stops.len() * 2);
        let loaded = build(&organ, 44_100.0).expect("merged bank builds");
        assert_eq!(loaded.specs.len(), single.specs.len() * 2);
        assert_eq!(loaded.bank.len(), single.bank.len());
        // Both copies carry a Hautbois; the first sits in its own
        // Récit box (enclosure 0), the second in ITS own (2), because
        // the merge offset the second copy's enclosure indices.
        let hautbois: Vec<u8> = organ
            .stops
            .iter()
            .filter(|s| s.name.contains("Hautbois") && !s.name.contains("noise"))
            .map(|stop| {
                let range = stop.ranks.first().expect("ranks");
                loaded.specs[&(range.rank, range.first_pipe)].enclosure
            })
            .collect();
        assert_eq!(hautbois, vec![0, 2]);
    }

    /// Render swell-box listening takes on the Récit reeds/strings
    /// (the registration a real swell box exists for): A/B states, a
    /// live pedal sweep through the inertia model, and a release with
    /// the box slammed shut (the tail must stay frozen).
    #[test]
    #[ignore = "renders /tmp swell wavs"]
    fn render_swell_demos() {
        let Some(path) = demo_organ() else { return };
        let organ = aristide_formats::grandorgue::load(&path).expect("loads").organ;
        let device_rate = 44_100.0f32;
        let loaded = build(&organ, device_rate).expect("bank builds");
        let sr = device_rate as usize;
        let recit = organ.manuals[2].id;
        let drawn: Vec<_> = organ
            .stops
            .iter()
            .filter(|s| {
                s.manual == recit
                    && !s.name.contains("noise")
                    && ["Bourdon 8", "Gamba 8", "Hautbois 8", "Trompette 8"]
                        .iter()
                        .any(|p| s.name.contains(p))
            })
            .map(|s| s.id)
            .collect();
        assert_eq!(drawn.len(), 4, "expected the four Récit 8' stops");

        enum Event {
            Note(u8, bool),
            Pedal(f32),
        }
        let render = |events: &[(usize, Event)], total: usize, out: &str| {
            let mut console = crate::console::Console::new(
                organ.clone(),
                loaded.specs.clone(),
                drawn.clone(),
                device_rate,
            );
            let (mut engine, mut handle) =
                aristide_engine::Engine::new(device_rate, std::sync::Arc::new(loaded.bank.clone()));
            handle.send(aristide_engine::Command::SetMasterGain { linear: 0.4 });
            // Sidecar-default box behaviour, floors from the ODF.
            for (index, enclosure) in organ.enclosures.iter().enumerate() {
                handle.send(aristide_engine::Command::SetEnclosure {
                    enclosure: index as u8,
                    params: aristide_engine::enclosure::EnclosureParams {
                        floor_db: 20.0
                            * (enclosure.amp_minimum_level as f32 / 100.0).max(0.01).log10(),
                        ..Default::default()
                    },
                });
            }
            let block = 512usize;
            let mut output = Vec::new();
            let mut buffer = vec![0.0f32; block * 2];
            let mut next = 0usize;
            let mut frame = 0usize;
            let started = std::time::Instant::now();
            while frame < total {
                while next < events.len() && events[next].0 < frame + block {
                    match events[next].1 {
                        Event::Note(key, true) => {
                            let (starts, retriggered) = console.note_on_manual(2, key.into(), 127);
                            for h in retriggered {
                                handle.send(aristide_engine::Command::StopVoice { handle: h });
                            }
                            for st in starts {
                                handle.send(aristide_engine::Command::StartVoice {
                                    handle: st.handle,
                                    sample: st.spec.sample,
                                    rate: st.spec.rate,
                                    gain: st.spec.gain,
                                    group: st.spec.group,
                                    wind_weight: st.spec.wind_weight,
                                    brightness: st.spec.brightness,
                                    enclosure: st.spec.enclosure,
                        bus: st.spec.bus,
                        delay_frames: st.spec.delay_frames,
                                });
                            }
                        }
                        Event::Note(key, false) => {
                            for h in console.note_off_manual(2, key.into()).0 {
                                handle.send(aristide_engine::Command::StopVoice { handle: h });
                            }
                        }
                        Event::Pedal(position) => {
                            for (enclosure, position) in
                                console.expression_manual(2, (position * 127.0) as u8)
                            {
                                handle.send(
                                    aristide_engine::Command::SetEnclosurePosition {
                                        enclosure,
                                        position,
                                    },
                                );
                            }
                        }
                    }
                    next += 1;
                }
                engine.process(&mut buffer, 2);
                output.extend_from_slice(&buffer);
                frame += block;
            }
            let rtf = started.elapsed().as_secs_f64() / (total as f64 / device_rate as f64);
            write_wav_f32(out, &output, 2, sr as u32);
            println!("wrote {out} (realtime factor {rtf:.3})");
        };

        let chord: [u8; 3] = [60, 64, 67];
        // Take 1: the same chord at open / half / closed.
        let mut events: Vec<(usize, Event)> = Vec::new();
        for (i, position) in [1.0f32, 0.5, 0.0].into_iter().enumerate() {
            let base = i * sr * 4 + sr / 4;
            events.push((base.saturating_sub(sr / 4), Event::Pedal(position)));
            for &k in &chord {
                events.push((base, Event::Note(k, true)));
                events.push((base + sr * 5 / 2, Event::Note(k, false)));
            }
        }
        render(&events, 3 * sr * 4 + sr, "/tmp/swell_ab.wav");

        // Take 2: held chord, pedal streaming closed→open→closed like a
        // real expression pedal (20 CC steps per move).
        let mut events: Vec<(usize, Event)> = vec![(0, Event::Pedal(1.0))];
        for &k in &chord {
            events.push((sr / 4, Event::Note(k, true)));
        }
        let stream = |events: &mut Vec<(usize, Event)>, at: usize, from: f32, to: f32| {
            for step in 0..=20 {
                let t = step as f32 / 20.0;
                events.push((
                    at + (t * 1.2 * sr as f32) as usize,
                    Event::Pedal(from + (to - from) * t),
                ));
            }
        };
        stream(&mut events, sr, 1.0, 0.0);
        stream(&mut events, 4 * sr, 0.0, 1.0);
        stream(&mut events, 7 * sr, 1.0, 0.0);
        for &k in &chord {
            events.push((10 * sr, Event::Note(k, false)));
        }
        events.sort_by_key(|e| e.0);
        render(&events, 12 * sr, "/tmp/swell_sweep.wav");

        // Take 3: release with the box just closed, then the pedal
        // reopens DURING the tail — the tail must not follow (frozen at
        // key-off; it is room decay that already left the box).
        let mut events: Vec<(usize, Event)> = vec![(0, Event::Pedal(1.0))];
        for &k in &chord {
            events.push((sr / 4, Event::Note(k, true)));
        }
        events.push((2 * sr, Event::Pedal(0.0)));
        for &k in &chord {
            events.push((3 * sr, Event::Note(k, false)));
        }
        events.push((3 * sr + sr / 3, Event::Pedal(1.0)));
        render(&events, 7 * sr, "/tmp/swell_release_freeze.wav");
    }

    /// Render a musical tour of the swell boxes: different music,
    /// registrations, boxes, and pedal behaviour on every take (the
    /// user A/Bs these by ear; no keyboard needed).
    #[test]
    #[ignore = "renders /tmp swell music wavs"]
    fn render_swell_music() {
        let Some(path) = demo_organ() else { return };
        let organ = aristide_formats::grandorgue::load(&path).expect("loads").organ;
        let device_rate = 44_100.0f32;
        let loaded = build(&organ, device_rate).expect("bank builds");
        let sr = device_rate as usize;
        let pick = |manual: usize, patterns: &[&str]| -> Vec<aristide_model::StopId> {
            let id = organ.manuals[manual].id;
            organ
                .stops
                .iter()
                .filter(|s| {
                    s.manual == id
                        && !s.name.contains("noise")
                        && patterns.iter().any(|p| s.name.contains(p))
                })
                .map(|s| s.id)
                .collect()
        };

        enum Event {
            /// (channel, key, on)
            Note(u8, u8, bool),
            /// (channel, position 0..1)
            Pedal(u8, f32),
        }
        // Channel 0 → Récit (manual 2), channel 1 → Great (manual 1).
        let render = |drawn: Vec<aristide_model::StopId>,
                      events: &mut Vec<(usize, Event)>,
                      total: usize,
                      master: f32,
                      out: &str| {
            events.sort_by_key(|e| e.0);
            let mut console = crate::console::Console::new(
                organ.clone(),
                loaded.specs.clone(),
                drawn,
                device_rate,
            );
            // The old channel map: channel 0 → manual 2, channel 1 → manual 1.
            let manual_of = |channel: u8| -> usize { [2usize, 1][channel as usize % 2] };
            let (mut engine, mut handle) =
                aristide_engine::Engine::new(device_rate, std::sync::Arc::new(loaded.bank.clone()));
            handle.send(aristide_engine::Command::SetMasterGain { linear: master });
            for (index, enclosure) in organ.enclosures.iter().enumerate() {
                handle.send(aristide_engine::Command::SetEnclosure {
                    enclosure: index as u8,
                    params: aristide_engine::enclosure::EnclosureParams {
                        floor_db: 20.0
                            * (enclosure.amp_minimum_level as f32 / 100.0).max(0.01).log10(),
                        ..Default::default()
                    },
                });
            }
            let block = 512usize;
            let mut output = Vec::new();
            let mut buffer = vec![0.0f32; block * 2];
            let (mut next, mut frame) = (0usize, 0usize);
            let started = std::time::Instant::now();
            while frame < total {
                while next < events.len() && events[next].0 < frame + block {
                    match events[next].1 {
                        Event::Note(channel, key, true) => {
                            let (starts, retriggered) =
                                console.note_on_manual(manual_of(channel), key.into(), 127);
                            for h in retriggered {
                                handle.send(aristide_engine::Command::StopVoice { handle: h });
                            }
                            for st in starts {
                                handle.send(aristide_engine::Command::StartVoice {
                                    handle: st.handle,
                                    sample: st.spec.sample,
                                    rate: st.spec.rate,
                                    gain: st.spec.gain,
                                    group: st.spec.group,
                                    wind_weight: st.spec.wind_weight,
                                    brightness: st.spec.brightness,
                                    enclosure: st.spec.enclosure,
                        bus: st.spec.bus,
                        delay_frames: st.spec.delay_frames,
                                });
                            }
                        }
                        Event::Note(channel, key, false) => {
                            for h in console.note_off_manual(manual_of(channel), key.into()).0 {
                                handle.send(aristide_engine::Command::StopVoice { handle: h });
                            }
                        }
                        Event::Pedal(channel, position) => {
                            for (enclosure, position) in console
                                .expression_manual(manual_of(channel), (position * 127.0) as u8)
                            {
                                handle.send(
                                    aristide_engine::Command::SetEnclosurePosition {
                                        enclosure,
                                        position,
                                    },
                                );
                            }
                        }
                    }
                    next += 1;
                }
                engine.process(&mut buffer, 2);
                output.extend_from_slice(&buffer);
                frame += block;
            }
            let rtf = started.elapsed().as_secs_f64() / (total as f64 / device_rate as f64);
            write_wav_f32(out, &output, 2, sr as u32);
            println!("wrote {out} (realtime factor {rtf:.3})");
        };
        // Helpers: notes at seconds, pedal streamed in 20 steps like a
        // real expression shoe.
        let s = |t: f64| (t * sr as f64) as usize;
        let note = |events: &mut Vec<(usize, Event)>, ch: u8, key: u8, at: f64, dur: f64| {
            events.push((s(at), Event::Note(ch, key, true)));
            events.push((s(at + dur), Event::Note(ch, key, false)));
        };
        let swell = |events: &mut Vec<(usize, Event)>, ch: u8, at: f64, dur: f64, from: f32, to: f32| {
            for step in 0..=20 {
                let t = step as f32 / 20.0;
                events.push((
                    s(at + dur * t as f64),
                    Event::Pedal(ch, from + (to - from) * t),
                ));
            }
        };

        // Take 1 — hymn phrase, full Récit 8' chorus, the classic
        // crescendo through the phrase and diminuendo to the cadence.
        let mut ev: Vec<(usize, Event)> = vec![(0, Event::Pedal(0, 0.0))];
        let chords: [(&[u8], f64, f64); 5] = [
            (&[48, 60, 64, 67], 0.3, 1.6), // C
            (&[45, 57, 60, 64], 1.9, 1.6), // Am
            (&[41, 53, 57, 65], 3.5, 1.6), // F
            (&[43, 55, 59, 62], 5.1, 1.6), // G
            (&[48, 60, 64, 72], 6.7, 2.6), // C
        ];
        for (keys, at, dur) in chords {
            for &k in keys {
                note(&mut ev, 0, k, at, dur);
            }
        }
        swell(&mut ev, 0, 0.3, 4.5, 0.0, 1.0);
        swell(&mut ev, 0, 5.1, 3.5, 1.0, 0.15);
        render(
            pick(2, &["Bourdon 8", "Gamba 8", "Hautbois 8", "Trompette 8"]),
            &mut ev,
            s(11.5),
            0.4,
            "/tmp/swell_hymn.wav",
        );

        // Take 2 — Hautbois solo line, pedal riding the phrase shape
        // (a reed exposes the muffle most).
        let mut ev: Vec<(usize, Event)> = vec![(0, Event::Pedal(0, 0.25))];
        let melody: [(u8, f64); 9] = [
            (64, 0.6),
            (67, 0.6),
            (69, 0.6),
            (72, 1.2),
            (69, 0.6),
            (67, 0.6),
            (64, 0.6),
            (62, 0.6),
            (60, 1.8),
        ];
        let mut at = 0.3;
        for (key, dur) in melody {
            note(&mut ev, 0, key, at, dur * 0.95);
            at += dur;
        }
        swell(&mut ev, 0, 0.3, 3.0, 0.25, 1.0);
        swell(&mut ev, 0, 3.9, 3.6, 1.0, 0.1);
        render(
            pick(2, &["Hautbois 8"]),
            &mut ev,
            s(9.5),
            0.9,
            "/tmp/swell_oboe.wav",
        );

        // Take 3 — echo: a trumpet motif open, echoed shut, then open.
        let mut ev: Vec<(usize, Event)> = Vec::new();
        for (repeat, position) in [(0u32, 1.0f32), (1, 0.05), (2, 1.0)] {
            let base = repeat as f64 * 2.8;
            ev.push((s(base), Event::Pedal(0, position)));
            for (i, key) in [55u8, 60, 64, 67].into_iter().enumerate() {
                note(&mut ev, 0, key, base + 0.4 + i as f64 * 0.18, 0.16);
            }
            for &k in &[60u8, 64, 67] {
                note(&mut ev, 0, k, base + 1.2, 1.2);
            }
        }
        render(
            pick(2, &["Trompette 8"]),
            &mut ev,
            s(10.0),
            0.5,
            "/tmp/swell_echo.wav",
        );

        // Take 4 — fast flute figuration with the pedal pumping: the
        // inertia keeps it musical, and there must be zero zipper.
        let mut ev: Vec<(usize, Event)> = vec![(0, Event::Pedal(0, 1.0))];
        let pattern = [60u8, 64, 67, 72, 76, 72, 67, 64];
        let step = 0.125;
        let mut at = 0.3;
        for cycle in 0..8 {
            for &key in &pattern {
                note(&mut ev, 0, key, at, step * 0.9);
                at += step;
            }
            if cycle % 2 == 0 {
                swell(&mut ev, 0, at - 1.0, 1.0, 1.0, 0.0);
            } else {
                swell(&mut ev, 0, at - 1.0, 1.0, 0.0, 1.0);
            }
        }
        render(
            pick(2, &["Bourdon 8", "Flute Oct"]),
            &mut ev,
            s(at + 3.0),
            0.9,
            "/tmp/swell_flutes.wav",
        );

        // Take 5 — the SECOND box (undisplayed "Grandorgue", chest 2,
        // floor −10.5 dB): Great plein jeu chords swelling open.
        let mut ev: Vec<(usize, Event)> = vec![(0, Event::Pedal(1, 0.0))];
        for (i, keys) in [[48u8, 55, 64], [50, 57, 65], [48, 55, 64]].iter().enumerate() {
            for &k in keys.iter() {
                note(&mut ev, 1, k, 0.3 + i as f64 * 2.2, 2.0);
            }
        }
        swell(&mut ev, 1, 0.5, 5.5, 0.0, 1.0);
        render(
            pick(1, &["Flute Harm", "Plein jeu III"]),
            &mut ev,
            s(9.5),
            0.3,
            "/tmp/swell_pleinjeu.wav",
        );

        // Take 6 — enclosed vs unenclosed at once: Great Montre drone
        // (no box) under a swelling Récit line — only the Récit moves.
        let mut ev: Vec<(usize, Event)> = vec![(0, Event::Pedal(0, 0.1))];
        for &k in &[48u8, 55, 60] {
            note(&mut ev, 1, k, 0.3, 10.5);
        }
        let line: [(u8, f64); 6] = [
            (67, 0.9),
            (72, 0.9),
            (76, 1.8),
            (74, 0.9),
            (71, 0.9),
            (67, 2.7),
        ];
        let mut at = 1.5;
        for (key, dur) in line {
            note(&mut ev, 0, key, at, dur * 0.95);
            at += dur;
        }
        swell(&mut ev, 0, 1.5, 3.5, 0.1, 1.0);
        swell(&mut ev, 0, 6.0, 3.5, 1.0, 0.1);
        render(
            [
                pick(1, &["Montre 8"]),
                pick(2, &["Gamba 8", "Hautbois 8"]),
            ]
            .concat(),
            &mut ev,
            s(13.0),
            0.4,
            "/tmp/swell_two_manuals.wav",
        );
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
        let manual_index = default_manual(&organ, 0);
        let mut console = crate::console::Console::new(organ, loaded.specs, drawn, 48000.0);
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
                for handle_id in console.note_off_manual(manual_index, key.into()).0 {
                    handle.send(aristide_engine::Command::StopVoice { handle: handle_id });
                }
                let (starts, retriggered) = console.note_on_manual(manual_index, key.into(), 127);
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
                        enclosure: start.spec.enclosure,
                        bus: start.spec.bus,
                        delay_frames: start.spec.delay_frames,
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
            .filter(|(_, c)| c.couples(great, swell))
            .map(|(i, _)| i)
            .collect();
        assert!(couplers.len() >= 2, "need II/I and 16' II/I couplers");
        let manual_index = default_manual(&organ, 0);
        let mut console = crate::console::Console::new(organ, loaded.specs, drawn, 48000.0);
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
                    let (starts, _) = console.note_on_manual(manual_index, key.into(), 127);
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
                        enclosure: start.spec.enclosure,
                        bus: start.spec.bus,
                        delay_frames: start.spec.delay_frames,
                        });
                    }
                } else {
                    for h in console.note_off_manual(manual_index, key.into()).0 {
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
            .filter(|(_, c)| c.couples(great, swell))
            .map(|(i, _)| i)
            .collect();
        let manual_index = default_manual(&organ, 0);
        let mut console = crate::console::Console::new(organ, loaded.specs, drawn, 48000.0);
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
                let (starts, retriggered) = console.note_on_manual(manual_index, key.into(), 127);
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
                        enclosure: start.spec.enclosure,
                        bus: start.spec.bus,
                        delay_frames: start.spec.delay_frames,
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
                for h in console.note_off_manual(manual_index, key.into()).0 {
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
        let manual_index = default_manual(&organ, 0);
        let mut console = crate::console::Console::new(organ, loaded.specs, drawn, 48000.0);
        let (mut engine, mut handle) =
            aristide_engine::Engine::new(48000.0, std::sync::Arc::new(loaded.bank));

        // Hold a 10-key chord over EVERY Great stop, settle, then
        // release everything in one burst.
        let keys = [48u8, 50, 52, 53, 55, 57, 59, 60, 62, 64];
        for &key in &keys {
            let (starts, _) = console.note_on_manual(manual_index, key.into(), 127);
            for start in starts {
                handle.send(aristide_engine::Command::StartVoice {
                    handle: start.handle,
                    sample: start.spec.sample,
                    rate: start.spec.rate,
                    gain: start.spec.gain,
                    group: start.spec.group,
                    wind_weight: start.spec.wind_weight,
                    brightness: start.spec.brightness,
                        enclosure: start.spec.enclosure,
                        bus: start.spec.bus,
                        delay_frames: start.spec.delay_frames,
                });
            }
        }
        let block = 256usize;
        let mut buffer = vec![0.0f32; block * 2];
        for _ in 0..64 {
            engine.process(&mut buffer, 2);
        }
        for &key in &keys {
            for handle_id in console.note_off_manual(manual_index, key.into()).0 {
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

        // The Great is manuals[1] (the pedal, manuals[0], is silent
        // here). Draw its first stop, press middle C.
        let manual_id = organ.manuals[1].id;
        let manual_index = default_manual(&organ, 0);
        let drawn = vec![
            organ
                .stops
                .iter()
                .find(|s| s.manual == manual_id)
                .expect("manual has stops")
                .id,
        ];
        let mut console = crate::console::Console::new(organ, loaded.specs, drawn, 48000.0);
        let (starts, _) = console.note_on_manual(manual_index, 60, 127);
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
                        enclosure: start.spec.enclosure,
                        bus: start.spec.bus,
                        delay_frames: start.spec.delay_frames,
            }));
        }
        let mut buffer = vec![0.0f32; 4800 * 2];
        engine.process(&mut buffer, 2);
        let energy: f32 = buffer.iter().map(|v| v * v).sum();
        assert!(energy > 0.0, "the organ should make sound");

        // Release: voices splice to their tails and eventually go quiet.
        for handle_id in console.note_off_manual(manual_index, 60).0 {
            handle.send(aristide_engine::Command::StopVoice { handle: handle_id });
        }
        // Long releases: give it a generous 30 s of rendering.
        for _ in 0..300 {
            engine.process(&mut buffer, 2);
        }
        let energy: f32 = buffer.iter().map(|v| v * v).sum();
        assert_eq!(energy, 0.0, "voices should have ended after release");
    }

    /// Parse the footage a stop's name advertises ("Montre 8'" → 8).
    /// Mixtures and noise effects carry no footage and return None.
    fn footage_from_name(name: &str) -> Option<f64> {
        name.split_whitespace().last()?.strip_suffix('\'')?.parse().ok()
    }

    /// Locate the fundamental of a rendered sustain near an expected
    /// pitch: harmonic-product scores over a ±17-semitone grid pick the
    /// octave (preferring the lowest candidate within a hair of the
    /// best — the standard guard against octave-up errors on
    /// harmonic-rich strings and reeds), then a fine scan settles cents.
    fn measured_f0(mono: &[f32], rate: f64, expected_hz: f64) -> f64 {
        let n = mono.len();
        let mag = |hz: f64| -> Option<f64> {
            if hz <= 10.0 || hz >= rate * 0.45 {
                return None;
            }
            let (mut re, mut im) = (0.0f64, 0.0f64);
            for (i, &s) in mono.iter().enumerate() {
                let w = 0.5 - 0.5 * (std::f64::consts::TAU * i as f64 / n as f64).cos();
                let phase = std::f64::consts::TAU * hz * i as f64 / rate;
                re += s as f64 * w * phase.cos();
                im += s as f64 * w * phase.sin();
            }
            Some((re * re + im * im).sqrt())
        };
        let score = |hz: f64| -> f64 {
            let mut sum = 0.0;
            let mut used = 0u32;
            for h in 1..=4u32 {
                if let Some(m) = mag(hz * h as f64) {
                    sum += (m + 1e-12).ln();
                    used += 1;
                }
            }
            if used == 0 { f64::MIN } else { sum / used as f64 }
        };
        let candidates: Vec<f64> = (-17..=17)
            .map(|s| expected_hz * (s as f64 / 12.0).exp2())
            .collect();
        let scores: Vec<f64> = candidates.iter().map(|&c| score(c)).collect();
        let funds: Vec<f64> = candidates
            .iter()
            .map(|&c| mag(c).unwrap_or(0.0))
            .collect();
        let best = scores.iter().copied().fold(f64::MIN, f64::max);
        let loudest = funds.iter().copied().fold(0.0, f64::max);
        // A low near-tie must have real energy at its own fundamental —
        // near-sinusoidal flute tones score their silent subharmonic
        // within the margin because half its probed harmonics coincide
        // with true partials.
        let coarse = candidates
            .iter()
            .zip(scores.iter().zip(&funds))
            .find(|&(_, (&s, &f))| s >= best - 1.0 && f >= loudest * 0.05)
            .map(|(&c, _)| c)
            .expect("at least one candidate scored");
        let mut fine = (coarse, f64::MIN);
        let mut cents = -60.0f64;
        while cents <= 60.0 {
            let hz = coarse * (cents / 1200.0).exp2();
            if let Some(m) = mag(hz) {
                if m > fine.1 {
                    fine = (hz, m);
                }
            }
            cents += 5.0;
        }
        fine.0
    }

    /// Every footage-labelled stop, drawn alone and played from its own
    /// manual, must sound at written pitch: 8' = unison at the key's
    /// MIDI note, 16' an octave below, 4' one above. Renders the lowest
    /// and a middle key of each stop through the real console→engine
    /// path and measures the fundamental. Catches key→pipe octave slips
    /// like the extended-compass stops (Montre 8', Bourdon 8',
    /// Trompette 8' run 85 pipes from logical key 1, twelve below the
    /// keyboard) sounding a rank-sharing octave off everywhere.
    #[test]
    fn every_stop_fundamental_matches_footage() {
        let Some(path) = demo_organ() else {
            eprintln!("skipping: demo set not present");
            return;
        };
        let organ = aristide_formats::grandorgue::load(&path).expect("loads").organ;
        let loaded = build(&organ, 48_000.0).expect("bank builds");
        let bank = std::sync::Arc::new(loaded.bank);
        let mut failures = Vec::new();
        let mut probed = 0;
        for stop in &organ.stops {
            let Some(footage) = footage_from_name(&stop.name) else {
                continue;
            };
            let manual_index = organ
                .manuals
                .iter()
                .position(|m| m.id == stop.manual)
                .expect("stop's manual exists");
            let manual = &organ.manuals[manual_index];
            let range = stop.ranks.first().expect("stop has a rank");
            let low = range.first_key;
            let high = (range.first_key + range.key_count).min(manual.key_count);
            assert!(low < high, "{}: no playable keys", stop.name);
            for key_index in [low, (low + high) / 2] {
                let midi = manual.first_midi_note as u16 + key_index;
                let expected = 440.0 * ((midi as f64 - 69.0) / 12.0).exp2() * 8.0 / footage;
                let mut console = crate::console::Console::new(
                    organ.clone(),
                    loaded.specs.clone(),
                    vec![stop.id],
                    48_000.0,
                );
                let (starts, _) = console.note_on_manual(manual_index, midi, 127);
                assert!(!starts.is_empty(), "{} key {key_index}: silent", stop.name);
                let (mut engine, mut handle) =
                    aristide_engine::Engine::new(48_000.0, bank.clone());
                for start in &starts {
                    assert!(handle.send(aristide_engine::Command::StartVoice {
                        handle: start.handle,
                        sample: start.spec.sample,
                        rate: start.spec.rate,
                        gain: start.spec.gain,
                        group: start.spec.group,
                        wind_weight: start.spec.wind_weight,
                        brightness: start.spec.brightness,
                        enclosure: start.spec.enclosure,
                        bus: start.spec.bus,
                        delay_frames: start.spec.delay_frames,
                    }));
                }
                let mut buffer = vec![0.0f32; 4800 * 2];
                let mut mono = Vec::with_capacity(4800 * 13);
                for _ in 0..13 {
                    engine.process(&mut buffer, 2);
                    mono.extend(buffer.chunks(2).map(|f| (f[0] + f[1]) * 0.5));
                }
                // Skip the attack transient, keep 1 s of sustain.
                let sustain = &mono[12_000..60_000];
                let f0 = measured_f0(sustain, 48_000.0, expected);
                let cents = 1_200.0 * (f0 / expected).log2();
                probed += 1;
                if cents.abs() > 100.0 {
                    failures.push(format!(
                        "{} ({footage}') key {key_index} (MIDI {midi}): expected {expected:.1} Hz, \
                         measured {f0:.1} Hz ({cents:+.0} cents)",
                        stop.name
                    ));
                }
            }
        }
        assert!(probed > 20, "probed only {probed} notes — demo set changed?");
        assert!(
            failures.is_empty(),
            "stops sounding off their written pitch:\n{}",
            failures.join("\n")
        );
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
            .filter(|(_, c)| c.couples(great, swell))
            .map(|(i, _)| i)
            .collect();
        let manual_index = default_manual(&organ, 0);
        let mut console = crate::console::Console::new(organ, loaded.specs, drawn, device_rate);
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
                    let (starts, retriggered) = console.note_on_manual(manual_index, key.into(), 127);
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
                        enclosure: start.spec.enclosure,
                        bus: start.spec.bus,
                        delay_frames: start.spec.delay_frames,
                        }));
                    }
                } else {
                    for h in console.note_off_manual(manual_index, key.into()).0 {
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

    /// Real-time budget check under the WORST CASE a player can reach:
    /// full organ ("*"), 256-frame blocks (the app default), the same
    /// fast-playing schedule as crackle_hunt. The user's live crackles
    /// with a clean in-app recording mean device underruns: the recorder
    /// taps rendered blocks before the device, so a callback that misses
    /// its ~5.8 ms deadline glitches the speakers but not the file. This
    /// measures per-block render cost so that regression is a number,
    /// not an ear. Run with: cargo test --release -- --ignored render_budget
    #[test]
    #[ignore]
    fn render_budget_under_full_organ() {
        render_budget(false);
    }

    /// Same bench in the engine's lite ("safe") mode: linear
    /// interpolation, no wind/tremulant/brightness/flow-noise. The gap
    /// between this and the full run is the price of the realism DSP —
    /// if the full run misses deadlines and this one doesn't, the engine
    /// is the bottleneck; if BOTH fit comfortably, the environment is.
    #[test]
    #[ignore]
    fn render_budget_lite_mode() {
        render_budget(true);
    }

    fn render_budget(lite: bool) {
        let Some(path) = demo_organ() else {
            eprintln!("skipping: demo set not present");
            return;
        };
        let organ = aristide_formats::grandorgue::load(&path)
            .expect("demo set loads")
            .organ;
        let device_rate = 44_100.0f32;
        let loaded = build(&organ, device_rate).expect("bank builds");
        // Full organ, as "*" draws it — every stop (Console
        // itself retires the noise stops from the drawn list).
        let drawn: Vec<_> = organ.stops.iter().map(|s| s.id).collect();
        let manual_index = default_manual(&organ, 0);
        let mut console = crate::console::Console::new(organ, loaded.specs, drawn, device_rate);
        let _ = loaded.bank.pre_fault();
        let (mut engine, mut handle) =
            aristide_engine::Engine::new(device_rate, std::sync::Arc::new(loaded.bank));
        engine.set_lite(lite);

        // Same deterministic schedule as crackle_hunt.
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
        let mut t = sr / 10;
        while t < 3 * sr {
            let key = spam_keys[rand(spam_keys.len())];
            let hold = sr * (60 + rand(140)) / 1000;
            events.push((t, key, true));
            events.push((t + hold, key, false));
            t += sr * (30 + rand(60)) / 1000;
        }
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
        let mut t = 6 * sr;
        let mut which = false;
        while t < 8 * sr {
            let key = if which { 60 } else { 62 };
            events.push((t, key, true));
            events.push((t + sr * 35 / 1000, key, false));
            which = !which;
            t += sr * 40 / 1000;
        }
        for &k in &chord {
            events.push((8 * sr + sr / 10, k, true));
        }
        for &k in &chord {
            events.push((9 * sr, k, false));
        }
        events.sort_by_key(|e| e.0);

        // Render 16 s in PRODUCTION 256-frame blocks, timing each one.
        let block = 256usize;
        let budget_us = block as f64 / device_rate as f64 * 1e6;
        let total_frames = sr * 16;
        let mut buffer = vec![0.0f32; block * 2];
        let mut next_event = 0usize;
        let mut frame = 0usize;
        let mut times_us: Vec<f64> = Vec::with_capacity(total_frames / block + 1);
        let mut voices_started = 0usize;
        while frame < total_frames {
            while next_event < events.len() && events[next_event].0 < frame + block {
                let (_, key, on) = events[next_event];
                next_event += 1;
                if on {
                    let (starts, retriggered) = console.note_on_manual(manual_index, key.into(), 127);
                    for h in retriggered {
                        assert!(handle.send(aristide_engine::Command::StopVoice { handle: h }));
                    }
                    for start in starts {
                        voices_started += 1;
                        assert!(handle.send(aristide_engine::Command::StartVoice {
                            handle: start.handle,
                            sample: start.spec.sample,
                            rate: start.spec.rate,
                            gain: start.spec.gain,
                            group: start.spec.group,
                            wind_weight: start.spec.wind_weight,
                            brightness: start.spec.brightness,
                            enclosure: start.spec.enclosure,
                        bus: start.spec.bus,
                        delay_frames: start.spec.delay_frames,
                        }));
                    }
                } else {
                    for h in console.note_off_manual(manual_index, key.into()).0 {
                        assert!(handle.send(aristide_engine::Command::StopVoice { handle: h }));
                    }
                }
            }
            let t0 = std::time::Instant::now();
            engine.process(&mut buffer, 2);
            times_us.push(t0.elapsed().as_secs_f64() * 1e6);
            frame += block;
        }

        let mut sorted = times_us.clone();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let pct = |p: f64| sorted[((sorted.len() - 1) as f64 * p) as usize];
        let over = times_us.iter().filter(|&&t| t > budget_us).count();
        let over_half = times_us.iter().filter(|&&t| t > budget_us * 0.5).count();
        println!(
            "mode={} blocks={} budget={budget_us:.0}us voices_started={voices_started}\n\
             p50={:.0}us p90={:.0}us p99={:.0}us max={:.0}us\n\
             over budget: {over} blocks, over 50% budget: {over_half} blocks",
            if lite { "lite" } else { "full" },
            times_us.len(),
            pct(0.50),
            pct(0.90),
            pct(0.99),
            pct(1.0),
        );
        // Report-only: no assert — the point is the printed numbers on
        // whatever machine this runs on. Underruns are a deployment
        // observation; the gate lives in the printed headroom.
    }

    /// A room's decay rate does not transpose. The demo set builds every
    /// pipe by repitching one of twelve F♯/G recordings, so one
    /// recording serves keys a tritone below it and a tritone above —
    /// rates 0.71 and 1.41 on the same tail. Played raw, that recorded
    /// room would ring twice as long on the low key as on the high one:
    /// the "artificial/bell" release, ring time following the key. The
    /// engine compensates the tail decay per voice, so the two must ring
    /// for comparable times.
    ///
    /// The reference key also pins the extended-compass mapping at the
    /// engine's end: middle C takes pipe 37 of the 85-pipe rank (a
    /// tritone below its recording, rate 0.71). While the loader clamped
    /// that rank to key 0 = pipe 1, middle C sounded pipe 25 — an octave
    /// low, at rate 1.41.
    #[test]
    fn repitched_release_rings_at_native_decay_rate() {
        let Some(path) = demo_organ() else {
            eprintln!("skipping: demo set not present");
            return;
        };
        let organ = aristide_formats::grandorgue::load(&path).expect("loads").organ;
        let device_rate = 44_100.0f32;
        let sr = device_rate as usize;
        let loaded = build(&organ, device_rate).expect("bank builds");
        let bank = std::sync::Arc::new(loaded.bank.clone());
        let great = organ.manuals[1].id;
        let montre = organ
            .stops
            .iter()
            .find(|s| s.manual == great && s.name.contains("Montre"))
            .expect("montre");

        let manual_index = default_manual(&organ, 0);
        // (sample, rate, seconds from key-off to 40 dB down).
        let ring = |key: u8| -> (u32, f64, f64) {
            let mut console = crate::console::Console::new(
                organ.clone(),
                loaded.specs.clone(),
                vec![montre.id],
                device_rate,
            );
            let (mut engine, mut handle) =
                aristide_engine::Engine::new(device_rate, bank.clone());
            engine.set_release_stagger(0.0);
            let (starts, _) = console.note_on_manual(manual_index, key.into(), 127);
            let voice = starts.first().expect("voice");
            let (sample, rate) = (voice.spec.sample, voice.spec.rate as f64);
            for st in starts {
                handle.send(aristide_engine::Command::StartVoice {
                    handle: st.handle,
                    sample: st.spec.sample,
                    rate: st.spec.rate,
                    gain: st.spec.gain,
                    group: st.spec.group,
                    wind_weight: st.spec.wind_weight,
                    brightness: st.spec.brightness,
                    enclosure: st.spec.enclosure,
                        bus: st.spec.bus,
                        delay_frames: st.spec.delay_frames,
                });
            }
            let block = 512usize;
            let mut buffer = vec![0.0f32; block * 2];
            let hold = 2 * sr;
            let mut output = Vec::new();
            let mut frame = 0usize;
            let mut released = false;
            while frame < hold + 5 * sr {
                if !released && frame >= hold {
                    released = true;
                    for h in console.note_off_manual(manual_index, key.into()).0 {
                        handle.send(aristide_engine::Command::StopVoice { handle: h });
                    }
                }
                engine.process(&mut buffer, 2);
                output.extend_from_slice(&buffer);
                frame += block;
            }
            let rms_db = |t: f64| -> f64 {
                let start = ((2.0 + t) * sr as f64) as usize;
                let window = sr / 20;
                let mut acc = 0.0f64;
                for i in 0..window {
                    let v = (output[(start + i) * 2] + output[(start + i) * 2 + 1]) as f64 * 0.5;
                    acc += v * v;
                }
                10.0 * (acc / window as f64).max(1e-14).log10()
            };
            // Ring time: measured from just after key-off, so the level
            // the tail starts at (which does vary by key) cancels out.
            let at_release = rms_db(0.02);
            let mut t = 0.02;
            while t < 4.0 && rms_db(t) > at_release - 40.0 {
                t += 0.01;
            }
            (sample, rate, t)
        };

        let (sample, rate, low_ring) = ring(60);
        assert!(
            (rate - std::f64::consts::FRAC_1_SQRT_2).abs() < 0.01,
            "middle C should take the tritone-down pipe, got rate {rate}"
        );
        let lambda = bank
            .get(sample)
            .expect("sample")
            .tail_decay_db_per_s() as f64;
        assert!(lambda > 10.0, "tail decay unmeasured: {lambda}");

        // An octave up: the same recording, now a tritone the other way.
        let (same, up_rate, high_ring) = ring(72);
        assert_eq!(same, sample, "keys 60 and 72 share one recording");
        assert!(
            (up_rate - std::f64::consts::SQRT_2).abs() < 0.01,
            "got rate {up_rate}"
        );

        assert!(
            (0.3..4.0).contains(&low_ring) && (0.3..4.0).contains(&high_ring),
            "implausible ring times: {low_ring:.2}s and {high_ring:.2}s"
        );
        // Uncompensated these differ by the full rate ratio (2.0x).
        let spread = low_ring.max(high_ring) / low_ring.min(high_ring);
        assert!(
            spread < 1.6,
            "ring time follows the key: {low_ring:.2}s at rate {rate:.3} vs \
             {high_ring:.2}s at rate {up_rate:.3} ({spread:.2}x apart)"
        );
    }

    /// Render listening demos: big loud chords + fast treble spam on the
    /// full coupled registration (the user's torture setup).
    #[test]
    #[ignore = "renders /tmp demo wavs"]
    fn render_listening_demos() {
        let Some(path) = demo_organ() else { return };
        let organ = aristide_formats::grandorgue::load(&path).expect("loads").organ;
        let device_rate = 44_100.0f32;
        let loaded = build(&organ, device_rate).expect("bank builds");
        let great = organ.manuals[1].id;
        let swell = organ.manuals[2].id;
        let drawn: Vec<_> = organ
            .stops
            .iter()
            .filter(|s| (s.manual == great || s.manual == swell) && !s.name.contains("noise"))
            .map(|s| s.id)
            .collect();
        let couplers: Vec<usize> = organ
            .couplers
            .iter()
            .enumerate()
            .filter(|(_, c)| c.couples(great, swell))
            .map(|(i, _)| i)
            .collect();
        let sr = device_rate as usize;
        let manual_index = default_manual(&organ, 0);

        let render = |events: &[(usize, u8, bool)], total: usize, out: &str| {
            let mut console = crate::console::Console::new(
                organ.clone(),
                loaded.specs.clone(),
                drawn.clone(),
                device_rate,
            );
            for &c in &couplers {
                console.set_coupler(c, true);
            }
            let (mut engine, mut handle) =
                aristide_engine::Engine::new(device_rate, std::sync::Arc::new(loaded.bank.clone()));
            // "very loud": +9 dB over the default -15 dB master.
            handle.send(aristide_engine::Command::SetMasterGain { linear: 0.5 });
            let block = 512usize;
            let mut output = Vec::new();
            let mut buffer = vec![0.0f32; block * 2];
            let mut next = 0usize;
            let mut frame = 0usize;
            while frame < total {
                while next < events.len() && events[next].0 < frame + block {
                    let (_, key, on) = events[next];
                    next += 1;
                    if on {
                        let (starts, retriggered) = console.note_on_manual(manual_index, key.into(), 127);
                        for h in retriggered {
                            handle.send(aristide_engine::Command::StopVoice { handle: h });
                        }
                        for st in starts {
                            handle.send(aristide_engine::Command::StartVoice {
                                handle: st.handle,
                                sample: st.spec.sample,
                                rate: st.spec.rate,
                                gain: st.spec.gain,
                                group: st.spec.group,
                                wind_weight: st.spec.wind_weight,
                                brightness: st.spec.brightness,
                        enclosure: st.spec.enclosure,
                        bus: st.spec.bus,
                        delay_frames: st.spec.delay_frames,
                            });
                        }
                    } else {
                        for h in console.note_off_manual(manual_index, key.into()).0 {
                            handle.send(aristide_engine::Command::StopVoice { handle: h });
                        }
                    }
                }
                engine.process(&mut buffer, 2);
                output.extend_from_slice(&buffer);
                frame += block;
            }
            write_wav_f32(out, &output, 2, sr as u32);
            println!("wrote {out}");
        };

        // Take 1: big chords, held ~1.6 s, clean gaps to expose releases.
        let chords: [&[u8]; 4] = [
            &[41, 53, 57, 60, 65, 69, 72],       // F major, wide
            &[36, 48, 55, 60, 64, 67, 72, 76],   // C major, huge
            &[43, 55, 62, 67, 71, 74, 79],       // G major, high
            &[41, 53, 57, 60, 65, 69, 72, 77, 81], // F again, higher crown
        ];
        let mut events: Vec<(usize, u8, bool)> = Vec::new();
        for (i, chord) in chords.iter().enumerate() {
            let base = i * sr * 5 / 2 + sr / 4;
            for &k in *chord {
                events.push((base, k, true));
                events.push((base + sr * 8 / 5, k, false));
            }
        }
        events.sort_by_key(|e| e.0);
        render(&events, chords.len() * sr * 5 / 2 + 2 * sr, "/tmp/demo_chords.wav");

        // Take 2: fast spam with a heavy treble bias (the old "super
        // high bells" register), 30-70 ms between onsets, 40-150 ms holds.
        let mut rng = 0xDEAD_BEEFu32;
        let mut rand = move |n: usize| {
            rng ^= rng << 13;
            rng ^= rng >> 17;
            rng ^= rng << 5;
            (rng as usize) % n
        };
        let keys = [60u8, 64, 67, 72, 74, 76, 79, 81, 84, 86, 88, 69, 71, 83];
        let mut events: Vec<(usize, u8, bool)> = Vec::new();
        let mut t = sr / 4;
        while t < 12 * sr {
            let key = keys[rand(keys.len())];
            let hold = sr * (40 + rand(110)) / 1000;
            events.push((t, key, true));
            events.push((t + hold, key, false));
            t += sr * (30 + rand(40)) / 1000;
        }
        events.sort_by_key(|e| e.0);
        render(&events, 15 * sr, "/tmp/demo_spam.wav");
    }

    /// Render the user's mixture-staccato scenario: highest mixture
    /// alone, short chords, releases exposed.
    #[test]
    #[ignore = "renders /tmp wavs"]
    fn render_mixture_staccato() {
        let Some(path) = demo_organ() else { return };
        let organ = aristide_formats::grandorgue::load(&path).expect("loads").organ;
        let device_rate = 44_100.0f32;
        let loaded = build(&organ, device_rate).expect("bank builds");
        let sr = device_rate as usize;
        for (pattern, out) in [("plein", "/tmp/mixture_staccato.wav"), ("octavin", "/tmp/octavin_staccato.wav")] {
            let stop = organ
                .stops
                .iter()
                .find(|s| s.name.to_lowercase().contains(pattern))
                .expect("stop");
            let mut console = crate::console::Console::new(
                organ.clone(),
                loaded.specs.clone(),
                vec![stop.id],
                device_rate,
            );
            let manual = organ
                .manuals
                .iter()
                .position(|m| m.id == stop.manual)
                .unwrap();
            let (mut engine, mut handle) =
                aristide_engine::Engine::new(device_rate, std::sync::Arc::new(loaded.bank.clone()));
            handle.send(aristide_engine::Command::SetMasterGain { linear: 0.4 });
            let chords: [&[u8]; 4] = [
                &[60, 64, 67],
                &[65, 69, 72],
                &[67, 71, 74],
                &[72, 76, 79],
            ];
            let mut events: Vec<(usize, u8, bool)> = Vec::new();
            let mut t = sr / 4;
            for _ in 0..2 {
                for chord in chords {
                    for &k in chord {
                        events.push((t, k, true));
                        events.push((t + sr / 8, k, false)); // 125 ms staccato
                    }
                    t += sr * 2 / 5; // 400 ms between chords
                }
            }
            // A final SOLO staccato note so pitch behavior is measurable
            // without chord partials interfering.
            events.push((t + sr / 2, 79, true));
            events.push((t + sr / 2 + sr / 8, 79, false));
            events.sort_by_key(|e| e.0);
            let total = t + 3 * sr;
            let block = 512usize;
            let mut output = Vec::new();
            let mut buffer = vec![0.0f32; block * 2];
            let mut next = 0usize;
            let mut frame = 0usize;
            while frame < total {
                while next < events.len() && events[next].0 < frame + block {
                    let (_, key, on) = events[next];
                    next += 1;
                    if on {
                        let (starts, retriggered) = console.note_on_manual(manual, key.into(), 127);
                        for h in retriggered {
                            handle.send(aristide_engine::Command::StopVoice { handle: h });
                        }
                        for st in starts {
                            handle.send(aristide_engine::Command::StartVoice {
                                handle: st.handle,
                                sample: st.spec.sample,
                                rate: st.spec.rate,
                                gain: st.spec.gain,
                                group: st.spec.group,
                                wind_weight: st.spec.wind_weight,
                                brightness: st.spec.brightness,
                        enclosure: st.spec.enclosure,
                        bus: st.spec.bus,
                        delay_frames: st.spec.delay_frames,
                            });
                        }
                    } else {
                        for h in console.note_off_manual(manual, key.into()).0 {
                            handle.send(aristide_engine::Command::StopVoice { handle: h });
                        }
                    }
                }
                engine.process(&mut buffer, 2);
                output.extend_from_slice(&buffer);
                frame += block;
            }
            write_wav_f32(out, &output, 2, sr as u32);
            println!("wrote {out} ({})", stop.name);
            let mut probe_console = crate::console::Console::new(
                organ.clone(),
                loaded.specs.clone(),
                vec![stop.id],
                48000.0,
            );
            for key in [67u8, 72, 76, 79] {
                let (starts, _) = probe_console.note_on_manual(manual, key.into(), 127);
                for st in &starts {
                    let smp = loaded.bank.get(st.spec.sample).unwrap();
                    println!(
                        "  key {key}: rate {:.3} lambda {:.1} dB/s tail {:.2}s (comp needed {:+.1}, clamp +-15)",
                        st.spec.rate,
                        smp.tail_decay_db_per_s(),
                        (smp.frames() - smp.release_start().unwrap_or(0)) as f32
                            / smp.sample_rate_hz(),
                        smp.tail_decay_db_per_s() * (st.spec.rate - 1.0)
                    );
                }
                for h in probe_console.note_off_manual(manual, key.into()).0 { let _ = h; }
            }
        }
    }

    /// Dump each probe stop's RAW embedded release material (from
    /// release_start to EOF, native rate, no engine processing) so the
    /// recorded tail can be compared against the engine's rendered one.
    #[test]
    #[ignore = "renders /tmp/rawtail_*.wav"]
    fn dump_raw_release_tails() {
        let Some(path) = demo_organ() else { return };
        let organ = aristide_formats::grandorgue::load(&path).expect("loads").organ;
        let device_rate = 44_100.0f32;
        let loaded = build(&organ, device_rate).expect("bank builds");
        for (pattern, tag) in [("flute harm", "flharm"), ("plein jeu iii", "plein")] {
            let stop = organ
                .stops
                .iter()
                .find(|s| s.name.to_lowercase().contains(pattern))
                .expect("stop");
            let manual = organ
                .manuals
                .iter()
                .position(|m| m.id == stop.manual)
                .unwrap();
            let mut console = crate::console::Console::new(
                organ.clone(),
                loaded.specs.clone(),
                vec![stop.id],
                device_rate,
            );
            let (starts, _) = console.note_on_manual(manual, 67, 127);
            for (i, st) in starts.iter().enumerate() {
                let smp = loaded.bank.get(st.spec.sample).unwrap();
                let frames = smp.frames();
                println!(
                    "{tag}#{i}: frames {frames} sr {} loop {:?} release_start {:?} \
                     ref_level {:.4} lambda {:.1} options {}",
                    smp.sample_rate_hz(),
                    smp.sustain_loop(),
                    smp.release_start(),
                    smp.tail_reference_level(),
                    smp.tail_decay_db_per_s(),
                    smp.release_options().len(),
                );
                let Some(tail) = smp.release_start() else { continue };
                let mut out = Vec::new();
                for pos in tail..frames {
                    let (l, r) = smp.read(pos as f64);
                    out.push(l);
                    out.push(r);
                }
                let path = format!("/tmp/rawtail_{tag}_{i}.wav");
                write_wav_f32(&path, &out, 2, smp.sample_rate_hz() as u32);
                println!("wrote {path}");
            }
            for h in console.note_off_manual(manual, 67).0 {
                let _ = h;
            }
        }
    }

    /// Solo-note release probes for per-partial decay measurement:
    /// native-pitch key vs worst-case repitched keys, long release
    /// window, one wav per (stop, key). Analyzed offline for band-wise
    /// tail decay rates (the "bell-like release" investigation).
    #[test]
    #[ignore = "renders /tmp/release_probe_*.wav"]
    fn render_release_probes() {
        let Some(path) = demo_organ() else { return };
        let organ = aristide_formats::grandorgue::load(&path).expect("loads").organ;
        let device_rate = 44_100.0f32;
        let loaded = build(&organ, device_rate).expect("bank builds");
        let sr = device_rate as usize;
        for (pattern, tag) in [("flute harm", "flharm"), ("plein jeu iii", "plein")] {
            let stop = organ
                .stops
                .iter()
                .find(|s| s.name.to_lowercase().contains(pattern))
                .expect("stop");
            let manual = organ
                .manuals
                .iter()
                .position(|m| m.id == stop.manual)
                .unwrap();
            for key in [67u8, 72, 73] {
                let mut console = crate::console::Console::new(
                    organ.clone(),
                    loaded.specs.clone(),
                    vec![stop.id],
                    device_rate,
                );
                let (mut engine, mut handle) = aristide_engine::Engine::new(
                    device_rate,
                    std::sync::Arc::new(loaded.bank.clone()),
                );
                handle.send(aristide_engine::Command::SetMasterGain { linear: 0.4 });
                let on_at = sr / 4;
                let off_at = on_at + 12 * sr / 10; // 1.2 s hold
                let total = off_at + 4 * sr; // 4 s release window
                let block = 512usize;
                let mut output = Vec::new();
                let mut buffer = vec![0.0f32; block * 2];
                let mut frame = 0usize;
                while frame < total {
                    if frame <= on_at && on_at < frame + block {
                        let (starts, _) = console.note_on_manual(manual, key.into(), 127);
                        for st in &starts {
                            let smp = loaded.bank.get(st.spec.sample).unwrap();
                            println!(
                                "{tag} key {key}: rate {:.3} lambda {:.1} dB/s f0 {:.1} Hz",
                                st.spec.rate,
                                smp.tail_decay_db_per_s(),
                                smp.measured_period()
                                    .map(|p| smp.sample_rate_hz() as f64 / p)
                                    .unwrap_or(0.0),
                            );
                            handle.send(aristide_engine::Command::StartVoice {
                                handle: st.handle,
                                sample: st.spec.sample,
                                rate: st.spec.rate,
                                gain: st.spec.gain,
                                group: st.spec.group,
                                wind_weight: st.spec.wind_weight,
                                brightness: st.spec.brightness,
                                enclosure: st.spec.enclosure,
                        bus: st.spec.bus,
                        delay_frames: st.spec.delay_frames,
                            });
                        }
                    }
                    if frame <= off_at && off_at < frame + block {
                        for h in console.note_off_manual(manual, key.into()).0 {
                            handle.send(aristide_engine::Command::StopVoice { handle: h });
                        }
                    }
                    engine.process(&mut buffer, 2);
                    output.extend_from_slice(&buffer);
                    frame += block;
                }
                let out = format!("/tmp/release_probe_{tag}_{key}.wav");
                write_wav_f32(&out, &output, 2, sr as u32);
                println!("wrote {out} ({})", stop.name);
            }
        }
    }

    /// Render ~30 s of music in the French classical style on the plein
    /// jeu registration (grand chords, suspension chain, cadential
    /// trill, Picardy final) — a listening demo, not a stress test.
    #[test]
    #[ignore = "renders /tmp/plein_jeu_music.wav"]
    fn render_plein_jeu_music() {
        let Some(path) = demo_organ() else { return };
        let organ = aristide_formats::grandorgue::load(&path).expect("loads").organ;
        let device_rate = 44_100.0f32;
        let loaded = build(&organ, device_rate).expect("bank builds");
        let names: Vec<&str> = organ.stops.iter().map(|s| s.name.as_str()).collect();
        let mut drawn: Vec<aristide_model::StopId> = Vec::new();
        for pattern in ["bourdon 16", "montre", "prestant", "plein jeu"] {
            for i in aristide_formats::sidecar::match_names(&names, pattern) {
                drawn.push(organ.stops[i].id);
            }
        }
        drawn.sort_by_key(|id| id.0);
        drawn.dedup();
        let manual_index = default_manual(&organ, 0);
        let mut console =
            crate::console::Console::new(organ.clone(), loaded.specs.clone(), drawn, device_rate);
        let (mut engine, mut handle) =
            aristide_engine::Engine::new(device_rate, std::sync::Arc::new(loaded.bank.clone()));
        handle.send(aristide_engine::Command::SetMasterGain { linear: 0.4 });
        let sr = device_rate as usize;
        let beat = 0.68f64; // ~88 bpm

        // (on_beat, off_beat, key)
        let mut notes: Vec<(f64, f64, u8)> = vec![
            // A: grand opening, D minor -> A (4-3 suspension) -> D minor
            (0.0, 4.0, 50), (0.0, 4.0, 62), (0.0, 5.0, 65), (0.0, 8.0, 69), (0.0, 6.0, 74),
            (4.0, 8.0, 57), (5.0, 8.0, 64), (6.0, 8.0, 73),
            (8.0, 12.0, 50), (8.0, 12.0, 57), (8.0, 12.0, 62), (8.0, 12.0, 65), (8.0, 12.0, 74),
            // B: descending chain Bb - Am - Gm - F - A
            (12.0, 14.0, 46), (12.0, 14.0, 58), (12.0, 14.0, 65), (12.0, 15.0, 70),
            (14.0, 16.0, 45), (14.0, 16.0, 57), (14.0, 16.0, 64), (15.0, 16.0, 69),
            (16.0, 18.0, 43), (16.0, 18.0, 58), (16.0, 18.0, 62), (16.0, 18.0, 67),
            (18.0, 20.0, 41), (18.0, 20.0, 57), (18.0, 20.0, 60), (18.0, 20.0, 65), (18.0, 20.0, 69),
            (20.0, 24.0, 45), (20.0, 24.0, 57), (20.0, 24.0, 61), (20.0, 24.0, 64), (20.0, 24.0, 69),
            // C: cadence
            (24.0, 26.0, 53), (24.0, 26.0, 62), (24.0, 26.0, 69), (24.0, 26.0, 74),
            (26.0, 28.0, 43), (26.0, 28.0, 58), (26.0, 28.0, 62), (26.0, 28.0, 67), (26.0, 28.0, 74),
            (28.0, 32.0, 45), (28.0, 32.0, 57), (28.0, 32.0, 64), (28.0, 32.0, 69),
            (28.0, 29.5, 74),
        ];
        // cadential trill 74/73, six alternations of ~0.18 beats
        let mut tb = 29.5;
        for i in 0..6 {
            let key = if i % 2 == 0 { 73 } else { 74 };
            notes.push((tb, tb + 0.18, key));
            tb += 0.18;
        }
        notes.push((tb, 32.0, 73));
        // final D major (Picardy), long hold into the room
        for &k in &[38u8, 50, 57, 62, 66, 69, 74] {
            notes.push((32.0, 39.0, k));
        }

        let mut events: Vec<(usize, u8, bool)> = Vec::new();
        for &(on, off, key) in &notes {
            events.push(((on * beat * sr as f64) as usize, key, true));
            events.push(((off * beat * sr as f64) as usize, key, false));
        }
        events.sort_by_key(|e| e.0);
        let total = (44.0 * beat * sr as f64) as usize;
        let block = 512usize;
        let mut output = Vec::new();
        let mut buffer = vec![0.0f32; block * 2];
        let mut next = 0usize;
        let mut frame = 0usize;
        while frame < total {
            while next < events.len() && events[next].0 < frame + block {
                let (_, key, on) = events[next];
                next += 1;
                if on {
                    let (starts, retriggered) = console.note_on_manual(manual_index, key.into(), 127);
                    for h in retriggered {
                        handle.send(aristide_engine::Command::StopVoice { handle: h });
                    }
                    for st in starts {
                        handle.send(aristide_engine::Command::StartVoice {
                            handle: st.handle,
                            sample: st.spec.sample,
                            rate: st.spec.rate,
                            gain: st.spec.gain,
                            group: st.spec.group,
                            wind_weight: st.spec.wind_weight,
                            brightness: st.spec.brightness,
                        enclosure: st.spec.enclosure,
                        bus: st.spec.bus,
                        delay_frames: st.spec.delay_frames,
                        });
                    }
                } else {
                    for h in console.note_off_manual(manual_index, key.into()).0 {
                        handle.send(aristide_engine::Command::StopVoice { handle: h });
                    }
                }
            }
            engine.process(&mut buffer, 2);
            output.extend_from_slice(&buffer);
            frame += block;
        }
        write_wav_f32("/tmp/plein_jeu_music.wav", &output, 2, sr as u32);
        println!("wrote /tmp/plein_jeu_music.wav");
    }

    /// Diagnostic: render one pipe at several hold lengths and dump the
    /// release for offline envelope comparison against the raw tail.
    /// cargo test -p aristide-server release_envelope -- --ignored --nocapture
    #[test]
    #[ignore = "diagnostic, writes /tmp wavs"]
    fn release_envelope_diagnostic() {
        let Some(path) = demo_organ() else { return };
        let organ = aristide_formats::grandorgue::load(&path).expect("loads").organ;
        let device_rate = 44_100.0f32;
        let loaded = build(&organ, device_rate).expect("bank builds");
        let great = organ.manuals[1].id;
        let montre = organ
            .stops
            .iter()
            .find(|s| s.manual == great && s.name.contains("Montre"))
            .expect("montre");
        let drawn = vec![montre.id];
        let sr = device_rate as usize;
        let manual_index = default_manual(&organ, 0);
        for hold_ms in [80usize, 200, 500, 2000] {
            let mut console =
                crate::console::Console::new(organ.clone(), loaded.specs.clone(), drawn.clone(), device_rate);
            let (mut engine, mut handle) =
                aristide_engine::Engine::new(device_rate, std::sync::Arc::new(loaded.bank.clone()));
            let block = 512usize;
            let hold_frames = sr * hold_ms / 1000;
            let total = hold_frames + sr * 5;
            let mut output = Vec::new();
            let mut buffer = vec![0.0f32; block * 2];
            let mut frame = 0usize;
            let mut on = false;
            let mut off = false;
            while frame < total {
                if !on {
                    on = true;
                    let (starts, _) = console.note_on_manual(manual_index, 60, 127);
                    for st in starts {
                        handle.send(aristide_engine::Command::StartVoice {
                            handle: st.handle,
                            sample: st.spec.sample,
                            rate: st.spec.rate,
                            gain: st.spec.gain,
                            group: st.spec.group,
                            wind_weight: st.spec.wind_weight,
                            brightness: st.spec.brightness,
                        enclosure: st.spec.enclosure,
                        bus: st.spec.bus,
                        delay_frames: st.spec.delay_frames,
                        });
                    }
                }
                if !off && frame >= hold_frames {
                    off = true;
                    for h in console.note_off_manual(manual_index, 60).0 {
                        handle.send(aristide_engine::Command::StopVoice { handle: h });
                    }
                }
                engine.process(&mut buffer, 2);
                output.extend_from_slice(&buffer);
                frame += block;
            }
            write_wav_f32(
                &format!("/tmp/release_{hold_ms}ms.wav"),
                &output,
                2,
                sr as u32,
            );
        }
        println!("wrote /tmp/release_{{80,200,500,2000}}ms.wav");

        // Ground truth from the same decoder the engine plays: the raw
        // tail envelope of the Montre pipe's own sample.
        let spec = loaded
            .specs
            .iter()
            .find(|((_, _), v)| {
                organ.stops.iter().any(|s| s.id == montre.id)
                    && v.wind_weight > 0.0
            })
            .map(|(_, v)| *v);
        // Find the montre middle-C spec through the console instead.
        let mut console =
            crate::console::Console::new(organ.clone(), loaded.specs.clone(), drawn.clone(), device_rate);
        let (starts, _) = console.note_on_manual(manual_index, 60, 127);
        let st = starts.first().expect("montre voice");
        let sample = loaded.bank.get(st.spec.sample).expect("sample");
        let tail = sample.release_start().unwrap_or(0);
        let sr_s = sample.sample_rate_hz();
        let win = (0.05 * sr_s) as u64;
        let mut env = Vec::new();
        let mut k = 0u64;
        while tail + (k + 1) * win < sample.frames() {
            let mut acc = 0.0f64;
            for i in 0..win {
                let (l, r) = sample.read((tail + k * win + i) as f64);
                let v = (l + r) * 0.5;
                acc += (v as f64) * (v as f64);
            }
            let rms = (acc / win as f64).sqrt();
            env.push(20.0 * (rms.max(1e-7)).log10());
            k += 1;
        }
        let pts = [0usize, 1, 2, 4, 8, 16, 24, 40, 60];
        let line: Vec<String> = pts
            .iter()
            .filter(|&&p| p < env.len())
            .map(|&p| format!("{}ms:{:.1}", 50 * p, env[p]))
            .collect();
        println!("RAW tail env dB: {}", line.join(" "));
        let _ = spec;

        // Level-match inputs: what the release() ratio actually sees.
        let (ls, le) = sample.sustain_loop().expect("loop");
        let mut acc = 0.0f64;
        let mut mean = 0.0f64;
        let count = (le - ls).min(8820);
        for i in 0..count {
            let (l, r) = sample.read((ls + i) as f64);
            let v = ((l + r) * 0.5) as f64;
            acc += v * v;
            mean += v.abs();
        }
        println!(
            "sustain loop: rms {:.4} mean-abs {:.4} | tail_reference_level {:.4} | ratio(loop-mean/ref) {:.3}",
            (acc / count as f64).sqrt(),
            mean / count as f64,
            sample.tail_reference_level(),
            (mean / count as f64) / sample.tail_reference_level() as f64
        );

        // Lite render (wind/tilt/wander off) of the 2 s hold isolates
        // whether the accelerating decay lives in the full-mode path.
        let mut console =
            crate::console::Console::new(organ.clone(), loaded.specs.clone(), drawn.clone(), device_rate);
        let (mut engine, mut handle) =
            aristide_engine::Engine::new(device_rate, std::sync::Arc::new(loaded.bank.clone()));
        engine.set_lite(true);
        let block = 512usize;
        let hold_frames = sr * 2;
        let total = hold_frames + sr * 5;
        let mut output = Vec::new();
        let mut buffer = vec![0.0f32; block * 2];
        let mut frame = 0usize;
        let mut sent_on = false;
        let mut sent_off = false;
        while frame < total {
            if !sent_on {
                sent_on = true;
                let (starts, _) = console.note_on_manual(manual_index, 60, 127);
                for st in starts {
                    handle.send(aristide_engine::Command::StartVoice {
                        handle: st.handle,
                        sample: st.spec.sample,
                        rate: st.spec.rate,
                        gain: st.spec.gain,
                        group: st.spec.group,
                        wind_weight: st.spec.wind_weight,
                        brightness: st.spec.brightness,
                        enclosure: st.spec.enclosure,
                        bus: st.spec.bus,
                        delay_frames: st.spec.delay_frames,
                    });
                }
            }
            if !sent_off && frame >= hold_frames {
                sent_off = true;
                for h in console.note_off_manual(manual_index, 60).0 {
                    handle.send(aristide_engine::Command::StopVoice { handle: h });
                }
            }
            engine.process(&mut buffer, 2);
            output.extend_from_slice(&buffer);
            frame += block;
        }
        write_wav_f32("/tmp/release_lite_2000ms.wav", &output, 2, sr as u32);
        println!("wrote /tmp/release_lite_2000ms.wav");

        // Zero-assumption check: the rendered tail vs the sample data it
        // should be replaying (RELDBG said relpos 164499 for this voice),
        // both through the same decoder. Master -15 dB default, voice
        // gain 1.2 * tail_gain 1.1.
        let rate = st.spec.rate as f64;
        let total_gain = 0.177828 * st.spec.gain * 1.1;
        println!(
            "spec.rate={} gain={} | sample_rate_hz={} frames={} tail_frames={} tail_seconds_at_rate={:.2}",
            st.spec.rate,
            st.spec.gain,
            sample.sample_rate_hz(),
            sample.frames(),
            sample.frames() - 164_450,
            (sample.frames() - 164_450) as f64 / (44100.0 * rate)
        );
        let relpos = 164_499.0f64;
        for t_ms in [200usize, 400, 800, 1200] {
            let w = (0.05 * sr as f64) as usize;
            let render_start = ((2.0 + 0.03 + t_ms as f64 / 1000.0) * sr as f64) as usize;
            let mut r_acc = 0.0f64;
            for i in 0..w {
                let v = (output[(render_start + i) * 2] + output[(render_start + i) * 2 + 1]) as f64 * 0.5;
                r_acc += v * v;
            }
            let mut e_acc = 0.0f64;
            for i in 0..w {
                let (l, r) = sample.read(relpos + (t_ms as f64 / 1000.0 * sr as f64 + i as f64) * rate);
                let v = ((l + r) * 0.5) as f64 * total_gain as f64;
                e_acc += v * v;
            }
            println!(
                "t+{t_ms}ms: render {:.1} dB, expected {:.1} dB, delta {:.1}",
                10.0 * (r_acc / w as f64).log10(),
                10.0 * (e_acc / w as f64).log10(),
                10.0 * (r_acc / e_acc).log10()
            );
        }
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
