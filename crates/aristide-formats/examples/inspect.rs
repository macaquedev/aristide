//! Load a sample set (GrandOrgue or Hauptwerk) and print a summary. Usage:
//! cargo run -p aristide-formats --example inspect -- path/to/set.organ
//! (or path/to/OrganDefinitions/x.Organ_Hauptwerk_xml)

use std::path::Path;

fn main() -> anyhow::Result<()> {
    let path = std::env::args()
        .nth(1)
        .ok_or_else(|| anyhow::anyhow!("usage: inspect <file.organ>"))?;
    let result = aristide_formats::load_set(Path::new(&path))?;
    let organ = &result.organ;

    println!("organ: {}", organ.name);
    println!(
        "manuals: {}, stops: {}, ranks: {}, couplers: {}",
        organ.manuals.len(),
        organ.stops.len(),
        organ.ranks.len(),
        organ.couplers.len()
    );
    for manual in &organ.manuals {
        println!(
            "  manual {:?} {:20} keys {}..{}",
            manual.id.0,
            manual.name,
            manual.first_midi_note,
            manual.first_midi_note as u16 + manual.key_count - 1
        );
    }
    let mut missing = 0usize;
    let mut sampled_pipes = 0usize;
    let mut borrowed_pipes = 0usize;
    let mut dead_borrows = 0usize;
    let mut silent_pipes = 0usize;
    let mut total_attacks = 0usize;
    let mut total_releases = 0usize;
    for rank in &organ.ranks {
        let pipes = rank.pipes.len();
        for pipe in &rank.pipes {
            match &pipe.source {
                aristide_model::PipeSource::Sampled { attacks, releases } => {
                    sampled_pipes += 1;
                    total_attacks += attacks.len();
                    total_releases += releases.len();
                    for attack in attacks {
                        if !organ.base_path.join(&attack.path).is_file() {
                            missing += 1;
                        }
                    }
                }
                aristide_model::PipeSource::Borrowed(target) => {
                    borrowed_pipes += 1;
                    let sounds = organ
                        .sounding_pipe(*target)
                        .is_some_and(|p| p.samples().is_some());
                    if !sounds {
                        dead_borrows += 1;
                    }
                }
                aristide_model::PipeSource::Silent => silent_pipes += 1,
            }
        }
        println!("  rank {:24} {pipes} pipes", rank.name);
    }
    println!(
        "sampled pipes: {sampled_pipes}, borrowed: {borrowed_pipes} ({dead_borrows} dead), \
         silent: {silent_pipes}, attacks: {total_attacks}, releases: {total_releases}, \
         missing sample files: {missing}"
    );

    // Exercise the WAV reader on the first real sample.
    if let Some(attack) = organ
        .ranks
        .iter()
        .flat_map(|r| &r.pipes)
        .filter_map(|p| p.samples())
        .flat_map(|(attacks, _)| attacks)
        .next()
    {
        let info = aristide_formats::wav::read_info(&organ.base_path.join(&attack.path))?;
        println!(
            "first sample {:?}: {} Hz, {} ch, {} bit, {} frames, {} loop(s), unity note {:?}",
            attack.path,
            info.sample_rate,
            info.channels,
            info.bits_per_sample,
            info.frames,
            info.loops.len(),
            info.midi_unity_note
        );
    }

    for warning in &result.warnings {
        println!("warning: {warning}");
    }
    Ok(())
}
