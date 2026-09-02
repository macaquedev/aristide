//! Stereo release-splice analysis: how far apart the two channels'
//! phase requirements are at every spliceable release in a sample set.
//!
//! For each file it measures the fundamental period from the sustain
//! loop, then the fundamental phase and amplitude of every channel at
//! the loop anchor and at the tail. `mismatch` is the disagreement
//! between the channels' required tail offsets, in turns of the
//! fundamental: 0 = one tail frame continues both channels, 0.5 = the
//! right channel splices exactly anti-phase when the left is perfect.
//!
//! Usage: cargo run -p aristide-server --example stereophase -- DIR|FILE...

use aristide_engine::bank::Sample;
use aristide_formats::wav;

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

fn main() -> anyhow::Result<()> {
    let mut paths = Vec::new();
    for arg in std::env::args().skip(1) {
        wavs(std::path::Path::new(&arg), &mut paths);
    }
    let mut mismatches: Vec<f64> = Vec::new();
    // Residual per-channel phase error, in turns, under each strategy,
    // plus the crossfade power each leaves cancelled: summed over
    // channels as weight * 2(1 - cos e), normalized by the total
    // weight so pipes are comparable.
    let mut residual_left_only: Vec<(f64, f64, f64)> = Vec::new();
    let mut residual_joint: Vec<(f64, f64, f64)> = Vec::new();
    let mut balance: Vec<f64> = Vec::new();
    println!(
        "{:<40} {:>3} {:>9} {:>8} {:>8} {:>9} {:>7} {:>7}",
        "file", "ch", "period", "turnsL", "turnsR", "mismatch", "ampR/L", "spread"
    );
    for path in &paths {
        let file = match wav::read(path) {
            Ok(file) => file,
            Err(error) => {
                println!("{}: {error}", path.display());
                continue;
            }
        };
        let info = file.info.clone();
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
        let Ok(sample) = Sample::new(
            file.samples,
            info.channels,
            info.sample_rate as f32,
            Some(sustain),
            tail,
        ) else {
            continue;
        };
        let unity = info.midi_unity_note.filter(|&note| note != 0).unwrap_or(60);
        let hz = 440.0 * 2.0f64.powf((unity as f64 - 69.0) / 12.0);
        let Some(period) = sample.measure_period(info.sample_rate as f64 / hz) else {
            continue;
        };
        let period_frames = period.round() as u64;
        let window = (period_frames * 8).clamp(512, 2400);
        if tail + period_frames + window >= sample.frames() {
            continue;
        }
        let turns_of = |channel: u16| {
            let (theta_loop, amplitude) = sample.quadrature(channel, sustain.0, window, period);
            let (theta_tail, _) = sample.quadrature(channel, tail, window, period);
            ((theta_tail - theta_loop) / std::f64::consts::TAU, amplitude)
        };
        let amplitudes_of = |channel: u16| {
            let (_, amplitude_loop) = sample.quadrature(channel, sustain.0, window, period);
            let (_, amplitude_tail) = sample.quadrature(channel, tail, window, period);
            (amplitude_loop, amplitude_tail)
        };
        let (turns_l, amplitude_l) = turns_of(0);
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
        if info.channels < 2 {
            println!(
                "{name:<40} {:>3} {period:>9.3} {:>8.4} {:>8} {:>9} {:>7} {:>7}",
                info.channels,
                turns_l.rem_euclid(1.0),
                "-",
                "-",
                "-",
                "-"
            );
            continue;
        }
        let (turns_r, amplitude_r) = turns_of(1);
        let raw = turns_r - turns_l;
        let mismatch = raw - (raw + 0.5).floor();
        mismatches.push(mismatch.abs());
        // Is a mismatch a real inter-channel phase shift or just a weak
        // channel's fundamental drowning in room noise? Re-measure it
        // from anchors a whole number of periods apart and over
        // different window lengths: a real shift is invariant, noise
        // scatters. `spread` is the peak-to-peak of those estimates.
        let mut probes = Vec::new();
        for cycles in [4u64, 8, 16] {
            for step in 0..3u64 {
                let probe_window = (period_frames * cycles).max(256);
                let anchor = tail + (period * step as f64).round() as u64;
                if anchor + probe_window >= sample.frames() {
                    continue;
                }
                let phase = |channel: u16| {
                    let (theta_loop, _) =
                        sample.quadrature(channel, sustain.0, probe_window, period);
                    let (theta_tail, _) = sample.quadrature(channel, anchor, probe_window, period);
                    (theta_tail - theta_loop) / std::f64::consts::TAU
                };
                let raw = phase(1) - phase(0);
                probes.push(raw - (raw + 0.5).floor());
            }
        }
        let spread = match (
            probes.iter().cloned().fold(f64::MIN, f64::max),
            probes.iter().cloned().fold(f64::MAX, f64::min),
        ) {
            (high, low) if !probes.is_empty() => high - low,
            _ => f64::NAN,
        };
        // What each strategy leaves on the table. The weight is the
        // product of the two legs' fundamental amplitudes: crossfading
        // amplitude a into b at phase error e loses 2ab(1 - cos e).
        let (amp_loop_l, amp_tail_l) = amplitudes_of(0);
        let (amp_loop_r, amp_tail_r) = amplitudes_of(1);
        let weight_l = amp_loop_l * amp_tail_l;
        let weight_r = amp_loop_r * amp_tail_r;
        let joint = turns_l + weight_r * mismatch / (weight_l + weight_r).max(1e-30);
        let cost = |chosen: f64| {
            let error = |want: f64, weight: f64| {
                let raw = chosen - want;
                let wrapped = raw - (raw + 0.5).floor();
                (wrapped, weight * 2.0 * (1.0 - (std::f64::consts::TAU * wrapped).cos()))
            };
            let (error_l, loss_l) = error(turns_l, weight_l);
            let (error_r, loss_r) = error(turns_r, weight_r);
            (
                error_l.abs(),
                error_r.abs(),
                (loss_l + loss_r) / (weight_l + weight_r).max(1e-30),
            )
        };
        residual_left_only.push(cost(turns_l));
        residual_joint.push(cost(joint));
        balance.push(
            20.0 * ((amp_tail_r / amp_tail_l.max(1e-12))
                / (amp_loop_r / amp_loop_l.max(1e-12)))
                .max(1e-9)
                .log10(),
        );
        println!(
            "{name:<40} {:>3} {period:>9.3} {:>8.4} {:>8.4} {:>+9.4} {:>7.2} {spread:>7.4}",
            info.channels,
            turns_l.rem_euclid(1.0),
            turns_r.rem_euclid(1.0),
            mismatch,
            amplitude_r / amplitude_l.max(1e-12)
        );
    }
    let report = |label: &str, rows: &[(f64, f64, f64)]| {
        let column = |pick: fn(&(f64, f64, f64)) -> f64| {
            let mut values: Vec<f64> = rows.iter().map(pick).collect();
            values.sort_by(|a, b| a.partial_cmp(b).unwrap());
            let at = |quantile: f64| values[((values.len() - 1) as f64 * quantile) as usize];
            format!("median {:.4}  p90 {:.4}  worst {:.4}", at(0.5), at(0.9), at(1.0))
        };
        println!("  {label:<7} |errL| {}", column(|row| row.0));
        println!("  {:<7} |errR| {}", "", column(|row| row.1));
        println!("  {:<7} loss  {}", "", column(|row| row.2));
    };
    if !residual_joint.is_empty() {
        println!("\nresidual splice phase error (turns) and cancelled crossfade power:");
        report("L-only", &residual_left_only);
        report("joint", &residual_joint);
        balance.sort_by(|a, b| a.abs().partial_cmp(&b.abs()).unwrap());
        println!(
            "\ntail-vs-loop L/R balance shift (dB): median {:.2}, p90 {:.2}, worst {:.2}",
            balance[balance.len() / 2].abs(),
            balance[(balance.len() - 1) * 9 / 10].abs(),
            balance[balance.len() - 1].abs()
        );
    }
    if !mismatches.is_empty() {
        mismatches.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let at = |quantile: f64| mismatches[((mismatches.len() - 1) as f64 * quantile) as usize];
        println!(
            "\n{} stereo files: |mismatch| median {:.4}, p90 {:.4}, max {:.4} turns (mean {:.4})",
            mismatches.len(),
            at(0.5),
            at(0.9),
            at(1.0),
            mismatches.iter().sum::<f64>() / mismatches.len() as f64
        );
    }
    Ok(())
}
