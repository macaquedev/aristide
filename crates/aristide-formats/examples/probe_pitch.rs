//! Print embedded pitch metadata and the measured fundamental of sample
//! files. Usage:
//! cargo run -p aristide-formats --example probe_pitch -- file.wav [...]

fn main() -> anyhow::Result<()> {
    for path in std::env::args().skip(1) {
        let file = aristide_formats::wav::read(std::path::Path::new(&path))?;
        let info = &file.info;
        let mono: Vec<f32> = file
            .samples
            .chunks(info.channels as usize)
            .map(|frame| frame.iter().sum::<f32>() / info.channels as f32)
            .collect();
        // Skip the attack transient, analyze up to 2 s of sustain.
        let start = (info.sample_rate as usize / 2).min(mono.len() / 2);
        let window = &mono[start..(start + info.sample_rate as usize * 2).min(mono.len())];
        let f0 = measure_f0(window, info.sample_rate as f64);
        let midi = 69.0 + 12.0 * (f0 / 440.0).log2();
        println!(
            "{path}: rate={} unity_note={:?} pitch_fraction={:?} measured_f0={f0:.2}Hz (midi {midi:.2})",
            info.sample_rate, info.midi_unity_note, info.pitch_fraction
        );
    }
    Ok(())
}

/// Harmonic-product-spectrum fundamental over a Hann-windowed FFT-free
/// DFT scan (coarse then fine), good to well under a cent for organ
/// sustains.
fn measure_f0(x: &[f32], rate: f64) -> f64 {
    let n = x.len();
    let spectrum_mag = |hz: f64| -> f64 {
        let mut re = 0.0f64;
        let mut im = 0.0f64;
        for (i, &s) in x.iter().enumerate() {
            let w = 0.5 - 0.5 * (core::f64::consts::TAU * i as f64 / n as f64).cos();
            let phase = core::f64::consts::TAU * hz * i as f64 / rate;
            re += s as f64 * w * phase.cos();
            im += s as f64 * w * phase.sin();
        }
        (re * re + im * im).sqrt()
    };
    // Coarse scan 20..1200 Hz on the harmonic product of 3 partials.
    let mut best = (0.0, f64::MIN);
    let mut hz = 20.0;
    while hz < 1200.0 {
        let score =
            spectrum_mag(hz).ln() + spectrum_mag(2.0 * hz).ln() + spectrum_mag(3.0 * hz).ln();
        if score > best.1 {
            best = (hz, score);
        }
        hz += 0.5;
    }
    // Fine scan ±0.5 Hz around the winner, fundamental only.
    let mut fine = (best.0, f64::MIN);
    let mut f = best.0 - 0.5;
    while f < best.0 + 0.5 {
        let score = spectrum_mag(f);
        if score > fine.1 {
            fine = (f, score);
        }
        f += 0.01;
    }
    fine.0
}
