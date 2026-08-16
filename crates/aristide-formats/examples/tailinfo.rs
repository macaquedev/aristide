//! Inspect the release-tail structure of sample files: loop points,
//! cue markers, tail length, and the tail's early level profile.
//! Usage: cargo run -p aristide-formats --example tailinfo -- files...

fn main() -> anyhow::Result<()> {
    for path in std::env::args().skip(1) {
        let file = aristide_formats::wav::read(std::path::Path::new(&path))?;
        let info = &file.info;
        let frames = info.frames;
        let ch = info.channels as usize;
        let mean_abs = |start: u64, len: u64| -> f32 {
            let end = (start + len).min(frames);
            if start >= end {
                return 0.0;
            }
            let mut sum = 0.0f64;
            for f in start..end {
                sum += file.samples[f as usize * ch].abs() as f64;
            }
            (sum / (end - start) as f64) as f32
        };
        println!("{path}");
        println!(
            "  frames {frames} ({:.2} s)",
            frames as f64 / info.sample_rate as f64
        );
        for l in &info.loops {
            println!(
                "  loop {}..{} (ends {:.2} s before EOF)",
                l.start,
                l.end,
                (frames - l.end) as f64 / info.sample_rate as f64
            );
        }
        println!("  cues {:?}", info.cue_points);
        // The very end of the file: a nonzero level here means every
        // voice HARD-CUTS to silence at EOF — a click per note.
        let end_window = 512.min(frames as usize);
        let mut end_peak = 0.0f32;
        for f in (frames as usize - end_window)..frames as usize {
            end_peak = end_peak.max(file.samples[f * ch].abs());
        }
        println!("  EOF: last-512-frame peak {end_peak:.5}");
        if let Some(l) = info.loops.first() {
            let tail = l.end + 1;
            println!(
                "  level: loop {:.4}, tail@0-50ms {:.4}, +100ms {:.4}, +300ms {:.4}, +1s {:.4}",
                mean_abs(l.start, 4410),
                mean_abs(tail, 2205),
                mean_abs(tail + 4410, 2205),
                mean_abs(tail + 13230, 2205),
                mean_abs(tail + 44100, 2205),
            );
        }
    }
    Ok(())
}
