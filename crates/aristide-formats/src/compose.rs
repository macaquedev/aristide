//! Composing one playable instrument out of several loaded organs.
//!
//! Aristide's multi-organ story happens here, at the model layer: any
//! number of loaded sets are merged into a single [`Organ`], and
//! everything downstream — sample bank, engine, console, UI — keeps
//! playing exactly one instrument. That is the organ-native view of
//! it: a console with ranks from three builders is still one organ,
//! and a coupler between two sets' manuals is just a route once they
//! share an ID space.
//!
//! The merge renumbers every `ManualId`/`StopId`/`RankId` into one
//! namespace (loaders start their counters near zero, so two sets'
//! ids collide constantly), rewrites everything that references them
//! (stop→manual, stop→rank ranges, borrowed pipes, coupler routes),
//! offsets windchest numbers and enclosure indices onto the shared
//! rails, and folds each set's `base_path` into its sample paths so
//! the merged organ no longer needs a single root directory.
//!
//! Sources are read, never written: a composite is Aristide-side data,
//! like everything else the sidecar philosophy keeps out of the sets.

use std::collections::HashMap;
use std::path::PathBuf;

use aristide_model::{ManualId, Organ, PipeSource, RankId, StopId};

/// One source organ's ids as they came in → as they are in the merged
/// organ. Callers use this to carry pre-merge decisions across (a
/// sidecar's default registration is matched against its own set's
/// names, then remapped).
#[derive(Debug, Default)]
pub struct IdMap {
    pub manuals: HashMap<ManualId, ManualId>,
    pub stops: HashMap<StopId, StopId>,
    pub ranks: HashMap<RankId, RankId>,
}

#[derive(Debug)]
pub struct Merged {
    pub organ: Organ,
    /// Per source, in input order.
    pub maps: Vec<IdMap>,
    pub warnings: Vec<String>,
}

/// A reference the source organ left dangling stays dangling after the
/// merge instead of aliasing a renumbered id it never meant. Lookups
/// on it keep returning `None`, exactly as before.
const DANGLING: u32 = u32::MAX;

/// Merge loaded organs into one playable instrument. One source passes
/// through untouched — no renumbering, no renaming — so the single-set
/// path behaves byte-identically to loading the set directly.
pub fn merge(sources: Vec<Organ>) -> Merged {
    if sources.len() <= 1 {
        let organ = sources.into_iter().next().unwrap_or_default();
        let maps = vec![IdMap {
            manuals: organ.manuals.iter().map(|m| (m.id, m.id)).collect(),
            stops: organ.stops.iter().map(|s| (s.id, s.id)).collect(),
            ranks: organ.ranks.iter().map(|r| (r.id, r.id)).collect(),
        }];
        return Merged {
            organ,
            maps,
            warnings: Vec::new(),
        };
    }

    let labels = source_labels(&sources);
    let manual_collisions = colliding(sources.iter().flat_map(|o| &o.manuals).map(|m| &m.name));
    let coupler_collisions = colliding(sources.iter().flat_map(|o| &o.couplers).map(|c| &c.name));
    let enclosure_collisions =
        colliding(sources.iter().flat_map(|o| &o.enclosures).map(|e| &e.name));

    let mut merged = Organ {
        name: labels.join(" + "),
        // Sample paths are made absolute per source below; the merged
        // organ has no single root, so its base is the empty path
        // (joining onto it is the identity).
        base_path: PathBuf::new(),
        ..Default::default()
    };
    let mut maps = Vec::new();
    let warnings = Vec::new();
    let mut next_manual = 0u32;
    let mut next_stop = 0u32;
    let mut next_rank = 0u32;
    let mut windchest_offset = 0u32;

    for (source, label) in sources.into_iter().zip(&labels) {
        let mut map = IdMap::default();
        for manual in &source.manuals {
            map.manuals.insert(manual.id, ManualId(next_manual));
            next_manual += 1;
        }
        for stop in &source.stops {
            map.stops.insert(stop.id, StopId(next_stop));
            next_stop += 1;
        }
        for rank in &source.ranks {
            map.ranks.insert(rank.id, RankId(next_rank));
            next_rank += 1;
        }
        let manual_of = |id: ManualId| *map.manuals.get(&id).unwrap_or(&ManualId(DANGLING));
        let rank_of = |id: RankId| *map.ranks.get(&id).unwrap_or(&RankId(DANGLING));
        let enclosure_offset = merged.enclosures.len() as u32;

        for mut manual in source.manuals {
            manual.id = manual_of(manual.id);
            if manual_collisions.contains(&manual.name.to_lowercase()) {
                manual.name = format!("{} — {label}", manual.name);
            }
            merged.manuals.push(manual);
        }
        for mut stop in source.stops {
            stop.id = *map.stops.get(&stop.id).expect("own id just mapped");
            stop.manual = manual_of(stop.manual);
            for range in &mut stop.ranks {
                range.rank = rank_of(range.rank);
            }
            merged.stops.push(stop);
        }
        for mut rank in source.ranks {
            rank.id = rank_of(rank.id);
            // Windchest 0 means "unset" format-side; it must not climb
            // onto a later source's real chests.
            if rank.windchest > 0 {
                rank.windchest += windchest_offset;
            }
            for pipe in &mut rank.pipes {
                match &mut pipe.source {
                    PipeSource::Sampled { attacks, releases } => {
                        for attack in attacks {
                            attack.path = source.base_path.join(&attack.path);
                        }
                        for release in releases {
                            release.path = source.base_path.join(&release.path);
                        }
                    }
                    PipeSource::Borrowed(target) => target.rank = rank_of(target.rank),
                    PipeSource::Silent => {}
                }
            }
            merged.ranks.push(rank);
        }
        for mut coupler in source.couplers {
            if coupler_collisions.contains(&coupler.name.to_lowercase()) {
                coupler.name = format!("{} — {label}", coupler.name);
            }
            for route in &mut coupler.routes {
                route.from_manual = manual_of(route.from_manual);
                if let Some(target) = &mut route.target {
                    target.manual = manual_of(target.manual);
                }
            }
            merged.couplers.push(coupler);
        }
        for mut enclosure in source.enclosures {
            if enclosure_collisions.contains(&enclosure.name.to_lowercase()) {
                enclosure.name = format!("{} — {label}", enclosure.name);
            }
            merged.enclosures.push(enclosure);
        }
        let mut source_max_chest = 0;
        for mut windchest in source.windchests {
            source_max_chest = source_max_chest.max(windchest.number);
            windchest.number += windchest_offset;
            for enclosure in &mut windchest.enclosures {
                *enclosure += enclosure_offset;
            }
            merged.windchests.push(windchest);
        }
        windchest_offset += source_max_chest;
        maps.push(map);
    }

    Merged {
        organ: merged,
        maps,
        warnings,
    }
}

/// A label for each source, to suffix onto colliding console names:
/// its own name, or "name 2", "name 3"… when the same set is loaded
/// more than once.
fn source_labels(sources: &[Organ]) -> Vec<String> {
    let name_collides = colliding(sources.iter().map(|o| &o.name));
    let mut seen: HashMap<String, u32> = HashMap::new();
    sources
        .iter()
        .map(|source| {
            let key = source.name.to_lowercase();
            let nth = seen.entry(key.clone()).and_modify(|n| *n += 1).or_insert(1);
            if name_collides.contains(&key) && *nth > 1 {
                format!("{} {nth}", source.name)
            } else {
                source.name.clone()
            }
        })
        .collect()
}

/// The lowercased names that appear more than once — the ones that
/// need a source suffix to stay tellable apart on one console. Names
/// unique across all sources keep their identity untouched.
fn colliding<'a>(names: impl Iterator<Item = &'a String>) -> std::collections::HashSet<String> {
    let mut counts: HashMap<String, u32> = HashMap::new();
    for name in names {
        *counts.entry(name.to_lowercase()).or_insert(0) += 1;
    }
    counts
        .into_iter()
        .filter(|(_, count)| *count > 1)
        .map(|(name, _)| name)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use aristide_model::{
        AttackSample, Coupler, Manual, Pipe, PipeRef, Rank, RankRange, ReleaseSample, Stop,
        Windchest,
    };

    fn organ(name: &str, base: &str) -> Organ {
        let sampled = |path: &str| PipeSource::Sampled {
            attacks: vec![AttackSample {
                path: PathBuf::from(path),
                loops: Vec::new(),
                pitch_offset_cents: 0.0,
            }],
            releases: vec![ReleaseSample {
                path: PathBuf::from(path),
                max_key_press_ms: None,
            }],
        };
        let pipe = |source: PipeSource| Pipe {
            nominal_frequency_hz: 440.0,
            pitch_tuning_cents: 0.0,
            gain_db: 0.0,
            midi_key_number: None,
            source,
        };
        Organ {
            name: name.into(),
            base_path: PathBuf::from(base),
            manuals: vec![
                Manual {
                    id: ManualId(0),
                    name: "Great".into(),
                    first_midi_note: 36,
                    key_count: 56,
                },
                Manual {
                    id: ManualId(1),
                    name: "Swell".into(),
                    first_midi_note: 36,
                    key_count: 56,
                },
            ],
            stops: vec![Stop {
                id: StopId(1),
                name: "Principal 8".into(),
                manual: ManualId(1),
                ranks: vec![RankRange {
                    rank: RankId(1),
                    first_key: 0,
                    key_count: 2,
                    first_pipe: 0,
                }],
            }],
            ranks: vec![Rank {
                id: RankId(1),
                name: "Principal 8".into(),
                windchest: 1,
                pipes: vec![
                    pipe(sampled("064-C.wav")),
                    pipe(PipeSource::Borrowed(PipeRef {
                        rank: RankId(1),
                        pipe: 0,
                    })),
                ],
            }],
            couplers: vec![Coupler::simple("Swell to Great", ManualId(1), ManualId(0), 0)],
            enclosures: vec![aristide_model::Enclosure {
                name: "Swell box".into(),
                amp_minimum_level: 20.0,
                midi_input_number: None,
                displayed: true,
            }],
            windchests: vec![Windchest {
                number: 1,
                name: "Main".into(),
                enclosures: vec![0],
            }],
        }
    }

    #[test]
    fn single_source_passes_through_untouched() {
        let source = organ("St. Anne", "/sets/anne");
        let merged = merge(vec![source.clone()]);
        assert_eq!(merged.organ.name, source.name);
        assert_eq!(merged.organ.base_path, source.base_path);
        assert_eq!(merged.organ.manuals[0].name, "Great");
        assert_eq!(
            merged.organ.ranks[0].pipes[0].samples().unwrap().0[0].path,
            PathBuf::from("064-C.wav")
        );
        assert_eq!(merged.maps[0].stops[&StopId(1)], StopId(1));
    }

    #[test]
    fn ids_are_disjoint_and_references_follow() {
        let merged = merge(vec![organ("A", "/a"), organ("B", "/b")]);
        let organ = &merged.organ;
        // Every id is unique across the merged instrument.
        let mut stop_ids: Vec<u32> = organ.stops.iter().map(|s| s.id.0).collect();
        stop_ids.dedup();
        assert_eq!(stop_ids.len(), 2);
        let mut rank_ids: Vec<u32> = organ.ranks.iter().map(|r| r.id.0).collect();
        rank_ids.dedup();
        assert_eq!(rank_ids.len(), 2);
        assert_eq!(organ.manuals.len(), 4);
        // B's stop still sits on B's Swell and sounds B's rank.
        let b_stop = &organ.stops[1];
        assert_eq!(b_stop.manual, organ.manuals[3].id);
        assert_eq!(b_stop.ranks[0].rank, organ.ranks[1].id);
        // B's coupler routes between B's manuals, not A's.
        let b_coupler = &organ.couplers[1];
        assert!(b_coupler.couples(organ.manuals[3].id, organ.manuals[2].id));
        // The borrowed pipe follows its own rank's new id.
        match &organ.ranks[1].pipes[1].source {
            PipeSource::Borrowed(target) => assert_eq!(target.rank, organ.ranks[1].id),
            other => panic!("expected borrow, got {other:?}"),
        }
        assert!(organ.sounding_pipe(PipeRef { rank: organ.ranks[1].id, pipe: 1 }).is_some());
    }

    #[test]
    fn sample_paths_absorb_each_sources_base() {
        let merged = merge(vec![organ("A", "/a"), organ("B", "/b")]);
        assert_eq!(merged.organ.base_path, PathBuf::new());
        let path = |rank: usize| {
            merged.organ.ranks[rank].pipes[0].samples().unwrap().0[0]
                .path
                .clone()
        };
        assert_eq!(path(0), PathBuf::from("/a/064-C.wav"));
        assert_eq!(path(1), PathBuf::from("/b/064-C.wav"));
    }

    #[test]
    fn colliding_console_names_get_source_suffixes() {
        let merged = merge(vec![organ("A", "/a"), organ("B", "/b")]);
        let names: Vec<&str> = merged.organ.manuals.iter().map(|m| m.name.as_str()).collect();
        assert_eq!(names, ["Great — A", "Swell — A", "Great — B", "Swell — B"]);
        assert_eq!(merged.organ.couplers[0].name, "Swell to Great — A");
        assert_eq!(merged.organ.enclosures[1].name, "Swell box — B");
        assert_eq!(merged.organ.name, "A + B");
    }

    #[test]
    fn same_set_twice_stays_tellable_apart() {
        let merged = merge(vec![organ("St. Anne", "/a"), organ("St. Anne", "/a")]);
        assert_eq!(merged.organ.name, "St. Anne + St. Anne 2");
        assert_eq!(merged.organ.manuals[2].name, "Great — St. Anne 2");
    }

    #[test]
    fn windchests_and_enclosures_land_on_disjoint_rails() {
        let merged = merge(vec![organ("A", "/a"), organ("B", "/b")]);
        let organ = &merged.organ;
        assert_eq!(organ.windchests[0].number, 1);
        assert_eq!(organ.windchests[1].number, 2);
        assert_eq!(organ.ranks[0].windchest, 1);
        assert_eq!(organ.ranks[1].windchest, 2);
        // B's chest sits in B's enclosure (index 1), not A's.
        assert_eq!(organ.windchests[1].enclosures, vec![1]);
    }

    #[test]
    fn maps_carry_premerge_decisions_across() {
        let merged = merge(vec![organ("A", "/a"), organ("B", "/b")]);
        // A registration matched against B's own names pre-merge lands
        // on B's stop in the merged organ.
        let b_new = merged.maps[1].stops[&StopId(1)];
        assert_eq!(merged.organ.stops[1].id, b_new);
        assert_ne!(merged.maps[0].stops[&StopId(1)], b_new);
    }
}
