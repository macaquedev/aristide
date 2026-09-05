//! Decode every WAV in a downloaded set without modifying any files.
//! cargo run --release -p aristide-formats --example audit_samples -- <folder>
use std::path::{Path, PathBuf};

fn collect(dir: &Path, paths: &mut Vec<PathBuf>) -> std::io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        if kind.is_dir() {
            collect(&entry.path(), paths)?;
        } else if kind.is_file()
            && entry
                .path()
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("wav"))
        {
            paths.push(entry.path());
        }
    }
    Ok(())
}

fn main() -> anyhow::Result<()> {
    let folder = std::env::args()
        .nth(1)
        .ok_or_else(|| anyhow::anyhow!("usage: audit_samples <sample-set folder>"))?;
    let mut paths = Vec::new();
    collect(Path::new(&folder), &mut paths)?;
    paths.sort();
    anyhow::ensure!(!paths.is_empty(), "no WAV files found in {folder}");
    let mut failures = 0;
    let mut silent = 0;
    let mut frames = 0u64;
    for path in &paths {
        match aristide_formats::wav::read(path) {
            Ok(file) => {
                frames += file.info.frames;
                let finite = file.samples.iter().all(|v| v.is_finite());
                let loops_valid = file
                    .info
                    .loops
                    .iter()
                    .all(|l| l.start <= l.end && l.end < file.info.frames);
                if !finite || !loops_valid {
                    eprintln!(
                        "INVALID {}: finite audio={finite}, valid loops={loops_valid}",
                        path.display()
                    );
                    failures += 1;
                }
                if file.samples.iter().all(|v| *v == 0.0) {
                    eprintln!("SILENT {}", path.display());
                    silent += 1;
                }
            }
            Err(error) => {
                eprintln!("FAILED {}: {error}", path.display());
                failures += 1;
            }
        }
    }
    println!(
        "{} WAV files checked; {frames} frames; {failures} invalid; {silent} entirely silent.",
        paths.len()
    );
    anyhow::ensure!(failures == 0, "{failures} sample files failed validation");
    Ok(())
}
