//! Per-channel release-splice quality over a whole sample set.
//!
//! Renders every spliceable stereo pipe through the real engine, stops
//! it at a spread of instants across the fundamental cycle (including
//! the adversarial anti-phase one), and measures the splice on each
//! channel SEPARATELY:
//!
//! * `dip` — worst period-length RMS during the crossfade divided by
//!   the held level. A phase-wrong splice does not click, it *cancels*:
//!   the two legs partially subtract for the whole fade. 1.0 = perfect.
//! * `kink` — the loudest second-difference outlier in the splice
//!   region against the region's own d2 statistics (crackle_hunt's
//!   detector, narrowed to the splice). Band-limited audio has a smooth
//!   d2, so a harmonic that arrives out of phase sticks out; the ratio
//!   is scale- and pitch-invariant, unlike d2 against signal RMS.
//!
//! Run it before and after an alignment change; the interesting number
//! is the gap between the L and R columns. `--tsv PATH` dumps every
//! splice so two runs can be diffed pipe by pipe (the demo set's median
//! mismatch is small, so only a paired diff shows the change clearly).
//!
//! Usage: cargo run --release -p aristide-server --example splicekink -- DIR [--tsv PATH]

use aristide_engine::bank::{Sample, SampleBank};
use aristide_engine::enclosure::{ENCLOSURE_NONE, MAX_VOICE_ENCLOSURES};
use aristide_engine::{Command, Engine};
use aristide_formats::wav;
use std::sync::Arc;

fn wavs(root: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
    if root.is_file() {
        out.push(root.to_path_buf());
        return;
    }
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    let mut paths: Vec<_> = entries.flatten().map(|entry| entry.path()).collect();
    paths.sort();
    for path in paths {
        if path.is_dir() {
            wavs(&path, out);
        } else if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("wav")) {
            out.push(path);
        }
    }
}

fn rms(window: &[f32]) -> f32 {
    (window.iter().map(|v| v * v).sum::<f32>() / window.len().max(1) as f32).sqrt()
}

/// Worst period-length RMS in `signal`, relative to `held`.
fn dip(signal: &[f32], period: usize, held: f32) -> f32 {
    let mut worst = f32::MAX;
    let mut start = 0;
    while start + period <= signal.len() {
        worst = worst.min(rms(&signal[start..start + period]));
        start += (period / 8).max(1);
    }
    worst / held.max(1e-9)
}

/// Loudest second-difference outlier relative to the local d2 RMS —
/// crackle_hunt's gate, which fires above 12.0.
fn kink_ratio(signal: &[f32]) -> f32 {
    const ALPHA: f64 = 1.0 / 256.0;
    let mut d2_rms_sq = 1e-9f64;
    let mut worst = 0.0f32;
    for (index, window) in signal.windows(3).enumerate() {
        let d2 = (window[2] - 2.0 * window[1] + window[0]).abs();
        if index > 64 {
            worst = worst.max(d2 / (d2_rms_sq.sqrt() as f32).max(1e-6));
        }
        d2_rms_sq += ALPHA * ((d2 as f64) * (d2 as f64) - d2_rms_sq);
    }
    worst
}

struct Row {
    name: String,
    dip: [f32; 2],
    kink: [f32; 2],
}

fn main() -> anyhow::Result<()> {
    let mut paths = Vec::new();
    let mut tsv: Option<String> = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--tsv" {
            tsv = args.next();
        } else {
            wavs(std::path::Path::new(&arg), &mut paths);
        }
    }
    let mut rows: Vec<Row> = Vec::new();
    for path in &paths {
        let Ok(file) = wav::read(path) else { continue };
        let info = file.info.clone();
        if info.channels != 2 {
            continue;
        }
        let Some(sustain) = info.loops.first().map(|l| (l.start, l.end + 1)) else {
            continue;
        };
        let tail = info
            .cue_points
            .iter()
            .copied()
            .filter(|&cue| cue >= sustain.1 && cue < info.frames)
            .max()
            .unwrap_or(sustain.1);
        let rate = info.sample_rate as f32;
        let Ok(probe) = Sample::new(file.samples.clone(), 2, rate, Some(sustain), tail) else {
            continue;
        };
        let unity = info.midi_unity_note.filter(|&note| note != 0).unwrap_or(60);
        let hz = 440.0 * 2.0f64.powf((unity as f64 - 69.0) / 12.0);
        let Some(period) = probe.measure_period(info.sample_rate as f64 / hz) else {
            continue;
        };
        let period_frames = period.round().max(2.0) as usize;
        // Enough held frames for the envelope follower to settle and
        // for a period-RMS reference, but well inside the loop.
        let hold = (sustain.0 as usize + 4 * period_frames).max(4800);
        if hold + 2 >= sustain.1 as usize {
            continue;
        }
        let name = path
            .iter()
            .rev()
            .take(2)
            .collect::<Vec<_>>()
            .iter()
            .rev()
            .map(|s| s.to_string_lossy())
            .collect::<Vec<_>>()
            .join("/");

        // Stop instants spread over one cycle: every splice phase the
        // player can land on, the anti-phase one included.
        for step in 0..8usize {
            let stop_after = hold + (period * step as f64 / 8.0).round() as usize;
            let mut bank = SampleBank::default();
            let Ok(mut sample) = Sample::new(file.samples.clone(), 2, rate, Some(sustain), tail)
            else {
                continue;
            };
            sample.align_release((info.sample_rate as f64 / period) as f32);
            bank.push(sample);
            let (mut engine, mut handle) = Engine::new(rate, Arc::new(bank));
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
            let mut held = vec![0.0f32; stop_after * 2];
            engine.process(&mut held, 2);
            handle.send(Command::StopVoice { handle: 1 });
            // The engine's default crossfade (pitch_scaled_fade_step):
            // 9 fundamental periods, floored at 6 ms. Measure exactly
            // it — scanning past it would report the tail's own decay
            // as a dip.
            let fade_frames = (9 * period_frames).clamp((0.006 * rate) as usize, (0.184 * rate) as usize);
            let mut spliced = vec![0.0f32; fade_frames * 2];
            engine.process(&mut spliced, 2);

            let mut row = Row {
                name: format!("{name} +{step}/8"),
                dip: [0.0; 2],
                kink: [0.0; 2],
            };
            for channel in 0..2usize {
                let held_channel: Vec<f32> =
                    held.chunks(2).map(|frame| frame[channel]).collect();
                let reference = rms(&held_channel[held_channel.len() - period_frames..]);
                let leg: Vec<f32> = spliced.chunks(2).map(|frame| frame[channel]).collect();
                row.dip[channel] = dip(&leg, period_frames, reference);
                row.kink[channel] = kink_ratio(&leg);
            }
            rows.push(row);
        }
    }

    rows.sort_by(|a, b| {
        (b.kink[1] - b.kink[0])
            .partial_cmp(&(a.kink[1] - a.kink[0]))
            .unwrap()
    });
    if let Some(path) = tsv {
        let mut dump = String::new();
        for row in &rows {
            dump.push_str(&format!(
                "{}\t{:.6}\t{:.6}\t{:.6}\t{:.6}\n",
                row.name, row.dip[0], row.dip[1], row.kink[0], row.kink[1]
            ));
        }
        std::fs::write(&path, dump)?;
        println!("wrote {path}");
    }

    let mut by_dip: Vec<&Row> = rows.iter().collect();
    by_dip.sort_by(|a, b| a.dip[0].min(a.dip[1]).partial_cmp(&b.dip[0].min(b.dip[1])).unwrap());
    println!("worst splice dips:");
    println!("{:<34} {:>7} {:>7} {:>8} {:>8}", "pipe", "dipL", "dipR", "kinkL", "kinkR");
    for row in by_dip.iter().take(20) {
        println!(
            "{:<34} {:>7.3} {:>7.3} {:>8.2} {:>8.2}",
            row.name, row.dip[0], row.dip[1], row.kink[0], row.kink[1]
        );
    }
    println!("\nworst R-minus-L kink gaps:");
    println!("{:<34} {:>7} {:>7} {:>8} {:>8}", "pipe", "dipL", "dipR", "kinkL", "kinkR");
    for row in rows.iter().take(15) {
        println!(
            "{:<34} {:>7.3} {:>7.3} {:>8.2} {:>8.2}",
            row.name, row.dip[0], row.dip[1], row.kink[0], row.kink[1]
        );
    }

    let summary = |label: &str, values: &mut Vec<f32>, low_is_bad: bool| {
        values.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let at = |q: f64| values[((values.len() - 1) as f64 * q) as usize];
        let (tail_q, worst) = if low_is_bad { (0.1, at(0.0)) } else { (0.9, at(1.0)) };
        println!(
            "  {label:<10} median {:>8.3}  p{:.0} {:>8.3}  worst {:>8.3}",
            at(0.5),
            tail_q * 100.0,
            at(tail_q),
            worst
        );
    };
    println!("\n{} splices ({} pipes x 8 stop phases)", rows.len(), rows.len() / 8);
    for (channel, label) in [(0usize, "L"), (1, "R")] {
        println!(" channel {label}:");
        summary(
            "dip",
            &mut rows.iter().map(|r| r.dip[channel]).collect(),
            true,
        );
        summary(
            "kink x",
            &mut rows.iter().map(|r| r.kink[channel]).collect(),
            false,
        );
    }
    Ok(())
}
