//! Builds the engine's [`SampleBank`] from a loaded organ model.
//!
//! Control-side only: decoding, validation, and per-pipe playback math
//! all happen here so the RT engine receives nothing but indices, rates,
//! and gains. Files are deduplicated by path (borrowed pipes and shared
//! samples decode once).

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Instant;

use anyhow::Result;
use aristide_engine::bank::{Sample, SampleBank};
use aristide_formats::wav;
use aristide_model::units::{cents_between, cents_to_ratio, db_to_linear, equal_ladder_hz};
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
    /// How far the pipe *really* sounds from `nominal_hz` at `rate`,
    /// cents, measured from the recording (or the organ's fitted home
    /// tuning when this pipe could not be measured; 0 when nothing
    /// could). A target tuning bends the pipe from here, not from the
    /// nominal — that is what makes "440 equal" exact on a set
    /// recorded at 415 in meantone.
    pub home_cents: f32,
    /// Where the organ's fitted tuning puts this pipe (rank anchor +
    /// class table), cents from `nominal_hz`: `home_cents` less the
    /// pipe's own drift. A target that keeps drift bends from here.
    pub model_cents: f32,
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
    /// The voicer's own treble tilt through that same filter, linear
    /// (`[[voicing.adjust]] brightness_db`, 1.0 = untouched). Specs
    /// are built per pipe at 1.0; the console stamps the resolved
    /// value per voice as it prices it, like the bus and the delay.
    pub voicing_tilt: f32,
    /// Swell boxes (0-based engine indices from the ODF windchest
    /// membership, innermost first;
    /// [`aristide_engine::enclosure::ENCLOSURE_NONE`] in unused slots,
    /// all of them for unenclosed divisions). Boxes nest, and both
    /// GO and Hauptwerk let a chest belong to several.
    pub enclosures: [u8; aristide_engine::enclosure::MAX_VOICE_ENCLOSURES],
    /// Output bus (0 = the main pair). Specs are built per pipe with
    /// the defaults; the console stamps these per stop from the
    /// sidecar's `[routing]`/`[voicing]` before a voice starts.
    pub bus: u8,
    /// Onset (speaking) delay in output frames.
    pub delay_frames: u32,
}

/// One selectable attack of a pipe (GO multi-attack): which bank sample
/// it is and the conditions under which GO's `GetAttack` would pick it.
#[derive(Debug, Clone, Copy)]
pub struct AttackOption {
    pub sample: u32,
    /// Multiplier on the pipe's primary [`VoiceSpec::rate`] when this
    /// attack replaces it (differing file sample rates; the recording
    /// pitch is assumed shared — variants are the same pipe re-miked).
    pub rate_factor: f32,
    /// GO `IsTremulant` tri-state against the chest's wave-trem state.
    pub wave_tremulant: Option<bool>,
    /// Lowest MIDI velocity this attack answers to.
    pub min_velocity: u8,
    /// Only when the pipe re-speaks within this many ms of its last
    /// release (fast-repetition re-attack); `None` = always.
    pub max_since_release_ms: Option<u32>,
}

pub struct LoadedBank {
    pub bank: SampleBank,
    /// (rank, pipe index) → playback spec. Borrowed pipes carry their
    /// target's spec; silent and failed pipes are absent.
    pub specs: HashMap<(RankId, u16), VoiceSpec>,
    /// Pipes with more than one decoded attack: the selection table the
    /// console consults at note-on. Absent for single-attack pipes.
    pub attack_options: HashMap<(RankId, u16), Vec<AttackOption>>,
    /// Human-readable notes about anything that didn't load.
    pub skipped: Vec<String>,
    /// The tuning the samples were recorded in, fitted from every
    /// pipe that measured; `None` when none did.
    pub home: Option<crate::tuning::HomeTuning>,
    /// Each rank's measured pitch anchor — the median of its pipes'
    /// deviation from the 440 ladder, cents — for ranks that measured.
    /// A rank comes from one set, so a set's own pitch is the median
    /// of these over its ranks.
    pub rank_anchors: HashMap<RankId, f64>,
}

/// `sample_bits`: resident audio resolution — 16 (default, half the
/// RAM) or anything else = f32. Quantization happens after each file's
/// analysis so periods, phase maps and tail measurements keep the full
/// decode precision.
/// `sample_bits`: resident audio resolution — 16 (default, half the
/// RAM) or anything else = f32. Quantization happens after each file's
/// analysis so periods, phase maps and tail measurements keep the full
/// decode precision.
/// A fully-resident build — what every test that has no opinion about
/// streaming wants. The server itself always goes through
/// [`build_with`] with the sidecar's policy.
#[cfg(test)]
pub fn build(
    organ: &Organ,
    device_rate: f32,
    sample_bits: u32,
    cache_path: Option<&std::path::Path>,
) -> Result<LoadedBank> {
    build_with(
        organ,
        device_rate,
        sample_bits,
        cache_path,
        StreamingPolicy::OFF,
    )
}

/// What a load may do with the release material that dominates a set's
/// bytes (see `aristide_engine::stream`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamingMode {
    /// Everything resident, as before streaming existed.
    Off,
    /// Stream every tail worth a slot, however small the set — the
    /// mode the tests and the demo set use.
    On,
    /// Stream only when the fully-resident bank would not fit the
    /// budget.
    Auto,
}

/// The user config's `[samples] streaming` / `ram_budget_mb`, resolved
/// against this machine.
#[derive(Debug, Clone, Copy)]
pub struct StreamingPolicy {
    pub mode: StreamingMode,
    pub ram_budget_mb: Option<u64>,
}

impl StreamingPolicy {
    /// The two ends of the policy, for tests: the server itself always
    /// builds one from the user config.
    #[cfg(test)]
    pub const OFF: StreamingPolicy = StreamingPolicy {
        mode: StreamingMode::Off,
        ram_budget_mb: None,
    };

    #[cfg(test)]
    pub const ON: StreamingPolicy = StreamingPolicy {
        mode: StreamingMode::On,
        ram_budget_mb: None,
    };

    /// Decide before a single file is decoded — by then it is too late,
    /// the RAM is already spent. The only measure available that early
    /// is what the source files weigh on disk, so the estimate is
    /// deliberately crude and deliberately pessimistic: WavPack holds
    /// organ samples at roughly 55 % of 16-bit PCM (so ~1.8× on decode
    /// to 16-bit residency), while 24-bit PCM shrinks to 0.67×. 1.5×
    /// sits between them, biased toward the compressed case — which is
    /// what the sets big enough to matter tend to be. Being wrong
    /// upward costs a stream pool and some disk reads; being wrong
    /// downward costs the process. `ram_budget_mb` overrides the guess
    /// for anyone near the line.
    fn resolve(&self, source_bytes: u64, quantize: bool) -> bool {
        match self.mode {
            StreamingMode::Off => false,
            StreamingMode::On => true,
            StreamingMode::Auto => {
                let estimate = source_bytes as f64 * if quantize { 1.5 } else { 3.0 };
                let budget = match self.ram_budget_mb {
                    Some(mb) => mb * 1024 * 1024,
                    None => match physical_ram_bytes() {
                        Some(total) => total / 2,
                        None => {
                            tracing::info!(
                                "samples: physical RAM unknown; streaming stays off \
                                 (set a RAM budget or streaming = on in Preferences)"
                            );
                            return false;
                        }
                    },
                };
                let stream = estimate > budget as f64;
                tracing::info!(
                    "samples: {:.0} MiB of source files ≈ {:.0} MiB resident against a \
                     {:.0} MiB budget — streaming {}",
                    source_bytes as f64 / (1024.0 * 1024.0),
                    estimate / (1024.0 * 1024.0),
                    budget as f64 / (1024.0 * 1024.0),
                    if stream { "on" } else { "off" }
                );
                stream
            }
        }
    }
}

/// This machine's physical memory. Linux only (`/proc/meminfo`);
/// elsewhere `auto` declines to guess and leaves streaming off. Read
/// once: the console polls it into every snapshot.
pub fn physical_ram_bytes() -> Option<u64> {
    static TOTAL: std::sync::OnceLock<Option<u64>> = std::sync::OnceLock::new();
    *TOTAL.get_or_init(|| {
        let text = std::fs::read_to_string("/proc/meminfo").ok()?;
        let line = text.lines().find(|line| line.starts_with("MemTotal:"))?;
        let kb: u64 = line.split_whitespace().nth(1)?.parse().ok()?;
        Some(kb * 1024)
    })
}

pub fn build_with(
    organ: &Organ,
    device_rate: f32,
    sample_bits: u32,
    cache_path: Option<&std::path::Path>,
    policy: StreamingPolicy,
) -> Result<LoadedBank> {
    let quantize = sample_bits == 16;
    let mut bank = SampleBank::default();
    let mut attack_options: HashMap<(RankId, u16), Vec<AttackOption>> = HashMap::new();
    let mut skipped = Vec::new();

    let chest_enclosures = resolve_chest_enclosures(organ, &mut skipped);

    let jobs = collect_decode_jobs(organ);
    let source_bytes: u64 = jobs
        .iter()
        .filter_map(|job| crate::cache::stamp(&organ.base_path.join(job.path())))
        .map(|(_, size)| size)
        .sum();
    let streaming = policy.resolve(source_bytes, quantize);

    // The stores the bank's streamed regions point into: the cache's
    // tail file for anything that came back hot, and this load's spool
    // for anything freshly decoded.
    let mut stores = aristide_engine::stream::StreamStores::new();
    let tails_path = cache_path.map(tails_path_for);
    let mut tails = tails_path
        .as_deref()
        .and_then(|path| crate::cache::Tails::open(path).ok());
    if streaming && let Some(open) = tails.as_mut() {
        match open.take_file() {
            Ok(file) => open.store = Some(stores.push(file)),
            Err(err) => tracing::warn!("sample cache tails unusable ({err}); re-decoding"),
        }
    }

    let plan = plan_cache(organ, cache_path, tails.as_ref(), quantize, jobs);
    // Fresh decodes need somewhere to put their tails as they finish —
    // holding them until the cache is written would be the very RAM
    // spike streaming exists to avoid.
    let spool = if streaming && plan.any_misses {
        match crate::spool::Spool::create(cache_path.and_then(|p| p.parent())) {
            Ok(spool) => Some(spool),
            Err(err) => {
                tracing::warn!("no stream spool ({err}); this load stays resident");
                None
            }
        }
    } else {
        None
    };
    let spool_store = match spool.as_ref() {
        Some(spool) => match spool.reader() {
            Ok(file) => Some(stores.push(file)),
            Err(err) => {
                tracing::warn!("stream spool unreadable ({err}); this load stays resident");
                None
            }
        },
        None => None,
    };
    let sink = spool.as_ref().zip(spool_store);

    let outcomes = decode_misses(organ, quantize, plan.misses, sink);
    let mut maps = finish_decode(
        cache_path,
        tails_path.as_deref(),
        (streaming || tails.is_some()).then_some(&stores),
        plan.hits,
        plan.miss_stamps,
        plan.total_jobs,
        plan.any_misses,
        outcomes,
    );

    // Every sampled pipe, decoded and measured, awaiting the pitch
    // decisions that need the whole instrument in view.
    let mut cache = DecodeCache {
        decoded: HashMap::new(),
        release_cache: HashMap::new(),
    };
    let mut staged: Vec<StagedPipe> = Vec::new();
    for (rank_index, rank) in organ.ranks.iter().enumerate() {
        let enclosures = chest_enclosures.get(&rank.windchest).copied().unwrap_or(
            [aristide_engine::enclosure::ENCLOSURE_NONE;
                aristide_engine::enclosure::MAX_VOICE_ENCLOSURES],
        );
        // Pipes decode first, then pitch decisions settle rank-wide
        // (the junk-metadata guard below needs the whole rank in view)
        // before specs are built.
        let pending = decode_rank_attacks(
            rank,
            &mut bank,
            &mut skipped,
            &mut attack_options,
            &mut cache,
            &mut maps,
        );
        staged.extend(stage_rank_pipes(
            rank,
            rank_index,
            enclosures,
            pending,
            &mut skipped,
            &bank,
        ));
    }

    let (home, rank_anchor) = fit_home_tuning(organ, &staged);
    let mut specs = assign_voice_specs(
        organ,
        device_rate,
        home.as_ref(),
        &rank_anchor,
        staged,
        &mut skipped,
    );
    assign_borrowed_pipe_specs(organ, &mut specs, &mut attack_options, &mut skipped);
    if bank.streamed_samples() > 0 {
        bank.set_stores(std::sync::Arc::new(stores));
    }

    Ok(LoadedBank {
        bank,
        specs,
        attack_options,
        skipped,
        home,
        rank_anchors: rank_anchor,
    })
}

/// The cache's companion tail file: `<hash>.samples` → `<hash>.tails`.
fn tails_path_for(cache_path: &std::path::Path) -> PathBuf {
    cache_path.with_extension("tails")
}

/// Windchest number → the enclosure engine indices its pipes sit in.
/// Boxes nest — an Echo or Solo box inside the Swell — so a chest can
/// legitimately belong to several: GO's `[WindchestGroupNNN]` lists
/// them by `NumberOfEnclosures`/`EnclosureNNN` and composes them all,
/// and the Hauptwerk reader keys a windchest by its whole sorted
/// enclosure set. The voice carries
/// [`MAX_VOICE_ENCLOSURES`](aristide_engine::enclosure::MAX_VOICE_ENCLOSURES)
/// of them; anything beyond that is dropped with a warning.
fn resolve_chest_enclosures(
    organ: &Organ,
    skipped: &mut Vec<String>,
) -> HashMap<u32, [u8; aristide_engine::enclosure::MAX_VOICE_ENCLOSURES]> {
    const SLOTS: usize = aristide_engine::enclosure::MAX_VOICE_ENCLOSURES;
    let mut chest_enclosures = HashMap::new();
    for chest in &organ.windchests {
        if chest.enclosures.is_empty() {
            continue;
        }
        let mut slots = [aristide_engine::enclosure::ENCLOSURE_NONE; SLOTS];
        let mut used = 0usize;
        for &member in &chest.enclosures {
            if (member as usize) >= aristide_engine::enclosure::MAX_ENCLOSURES {
                continue;
            }
            let index = member as u8;
            if slots[..used].contains(&index) {
                continue;
            }
            if used == SLOTS {
                skipped.push(format!(
                    "windchest {} ({}) sits in {} enclosures; the engine                      nests {SLOTS}, so enclosure {member} is ignored",
                    chest.number,
                    chest.name,
                    chest.enclosures.len()
                ));
                break;
            }
            slots[used] = index;
            used += 1;
        }
        if used > 0 {
            chest_enclosures.insert(chest.number, slots);
        }
    }
    chest_enclosures
}

/// One file waiting to decode — an attack (with the pitch its recording
/// should sit at) or a release — dispatched to the worker pool in
/// [`decode_misses`].
enum Job<'a> {
    Attack {
        path: &'a PathBuf,
        attack: &'a aristide_model::AttackSample,
        nominal_hz: f64,
    },
    Release {
        path: &'a PathBuf,
        release: &'a aristide_model::ReleaseSample,
    },
}

impl Job<'_> {
    fn path(&self) -> &PathBuf {
        match self {
            Job::Attack { path, .. } => path,
            Job::Release { path, .. } => path,
        }
    }
}

/// A decoded [`Job`], still keyed by its path for the caller to file
/// away.
enum Outcome {
    Attack(Result<(Sample, DecodedInfo), String>),
    Release(Result<Sample, String>),
}

/// Every unique file that must decode — and run its expensive analysis:
/// period refinement, phase maps, tail measurement — on a worker pool.
/// Decode is embarrassingly parallel per file and dominates load time
/// single-threaded; assembly stays sequential.
fn collect_decode_jobs(organ: &Organ) -> Vec<Job<'_>> {
    let mut jobs: Vec<Job> = Vec::new();
    let mut seen_attacks = std::collections::HashSet::new();
    let mut seen_releases = std::collections::HashSet::new();
    for rank in &organ.ranks {
        for pipe in &rank.pipes {
            let PipeSource::Sampled { attacks, releases } = &pipe.source else {
                continue;
            };
            for attack in attacks {
                if seen_attacks.insert(&attack.path) {
                    jobs.push(Job::Attack {
                        path: &attack.path,
                        attack,
                        // Where the recording should sit: the
                        // pipe's nominal, less what the set's own
                        // voicing shifts it by (a mixture rank
                        // repitched a tritone records a tritone
                        // away). Shared files share the pitch: the
                        // first referencing pipe measures, exactly
                        // as the sequential decode did.
                        nominal_hz: pipe.nominal_frequency_hz
                            * (-(pipe.pitch_tuning_cents + attack.pitch_offset_cents)
                                / 1200.0)
                                .exp2(),
                    });
                }
            }
            for release in releases {
                if seen_releases.insert(&release.path) {
                    jobs.push(Job::Release {
                        path: &release.path,
                        release,
                    });
                }
            }
        }
    }
    jobs
}

/// The load cache's verdict on one load's jobs (§3b): which are still
/// current on disk, which must decode, and what a fresh decode needs
/// to remember to be written back (see [`finish_decode`]).
struct CachePlan<'a> {
    total_jobs: usize,
    any_misses: bool,
    hits: Vec<(PathBuf, crate::cache::Entry)>,
    misses: Vec<Job<'a>>,
    /// path → (meta hash, mtime, size) for entries a fresh decode earns.
    miss_stamps: HashMap<PathBuf, (u64, u64, u64)>,
}

/// The load cache (§3b): entries whose file stamp and decode inputs
/// still match skip decode+analysis entirely; the rest decode below
/// and the cache is rewritten with the union.
fn plan_cache<'a>(
    organ: &Organ,
    cache_path: Option<&std::path::Path>,
    tails: Option<&crate::cache::Tails>,
    quantize: bool,
    jobs: Vec<Job<'a>>,
) -> CachePlan<'a> {
    let mut stored: HashMap<PathBuf, crate::cache::Entry> = match cache_path {
        Some(path) => match crate::cache::read(path, tails) {
            Ok(entries) => entries,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => HashMap::new(),
            Err(err) => {
                tracing::info!("sample cache unreadable ({err}); rebuilding it");
                HashMap::new()
            }
        },
        None => HashMap::new(),
    };
    let total_jobs = jobs.len();
    let mut hits: Vec<(PathBuf, crate::cache::Entry)> = Vec::new();
    let mut misses: Vec<Job> = Vec::new();
    // path → (meta hash, mtime, size) for entries a fresh decode earns.
    let mut miss_stamps: HashMap<PathBuf, (u64, u64, u64)> = HashMap::new();
    for job in jobs {
        let (path, description, is_attack) = match &job {
            Job::Attack {
                path,
                attack,
                nominal_hz,
            } => (
                (*path).clone(),
                format!(
                    "a|{}|{}|{quantize}",
                    serde_json::to_string(attack).unwrap_or_default(),
                    nominal_hz.to_bits()
                ),
                true,
            ),
            Job::Release { path, release } => (
                (*path).clone(),
                format!(
                    "r|{}|{quantize}",
                    serde_json::to_string(release).unwrap_or_default()
                ),
                false,
            ),
        };
        let meta = crate::cache::meta_hash(&description);
        let stamp = crate::cache::stamp(&organ.base_path.join(&path));
        match (stored.remove(&path), stamp) {
            (Some(entry), Some((mtime, size)))
                if entry.meta_hash == meta
                    && entry.mtime_ns == mtime
                    && entry.size == size
                    && entry.info.is_some() == is_attack =>
            {
                hits.push((path, entry));
            }
            (_, stamp) => {
                if let Some((mtime, size)) = stamp {
                    miss_stamps.insert(path, (meta, mtime, size));
                }
                misses.push(job);
            }
        }
    }
    let any_misses = !misses.is_empty();
    CachePlan {
        total_jobs,
        any_misses,
        hits,
        misses,
        miss_stamps,
    }
}

/// Decode every cache-miss job on a worker pool sized to the machine.
fn decode_misses(
    organ: &Organ,
    quantize: bool,
    misses: Vec<Job<'_>>,
    sink: Option<(&crate::spool::Spool, u16)>,
) -> Vec<(PathBuf, Outcome)> {
    let queue = std::sync::Mutex::new(misses);
    let results = std::sync::Mutex::new(Vec::new());
    let workers = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4);
    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| loop {
                let job = queue.lock().expect("job queue").pop();
                let Some(job) = job else { break };
                let outcome = match job {
                    Job::Attack {
                        path,
                        attack,
                        nominal_hz,
                    } => {
                        let absolute = organ.base_path.join(path);
                        let result = decode(&absolute, attack).map(|(mut sample, info)| {
                            // Phase-align the release splice to the
                            // pipe's fundamental.
                            sample.align_release(nominal_hz as f32);
                            if quantize {
                                sample.quantize_i16();
                            }
                            // Every analysis pass above ran on the whole
                            // recording; only now does the tail leave RAM.
                            offload(&mut sample, sink, &absolute);
                            (sample, info)
                        });
                        (path.clone(), Outcome::Attack(result))
                    }
                    Job::Release { path, release } => {
                        let absolute = organ.base_path.join(path);
                        let result = decode_release(&absolute, release).map(|mut sample| {
                            // Level/phase analysis at attach reads
                            // through the resident format; a −96 dB
                            // floor moves neither.
                            if quantize {
                                sample.quantize_i16();
                            }
                            offload(&mut sample, sink, &absolute);
                            sample
                        });
                        (path.clone(), Outcome::Release(result))
                    }
                };
                results.lock().expect("results").push(outcome);
            });
        }
    });
    results.into_inner().expect("results")
}

/// Move one freshly decoded sample's tail to the spool. A spool that
/// cannot be written (a full disk) is a warning, not a failure: the
/// sample simply stays whole, exactly as a non-streaming load leaves it.
fn offload(
    sample: &mut Sample,
    sink: Option<(&crate::spool::Spool, u16)>,
    path: &std::path::Path,
) {
    let Some((spool, store)) = sink else {
        return;
    };
    let mut sink = spool;
    if let Err(err) = sample.offload_tail(store, &mut sink) {
        tracing::warn!("{}: tail stays in RAM ({err})", path.display());
    }
}

/// Every unique file, decoded and keyed by path, awaiting assembly.
/// Fresh decodes and cache hits both end up here identically.
struct DecodedMaps {
    predecoded: HashMap<PathBuf, Result<(Sample, DecodedInfo), String>>,
    prereleased: HashMap<PathBuf, Result<Sample, String>>,
}

/// Turn fresh decode outcomes and surviving cache hits into one set of
/// decoded files, and rewrite the cache with the union — surviving
/// hits plus fresh successes, all borrowed in place: no sample is ever
/// cloned to be cached.
#[allow(clippy::too_many_arguments)]
fn finish_decode(
    cache_path: Option<&std::path::Path>,
    tails_path: Option<&std::path::Path>,
    stores: Option<&aristide_engine::stream::StreamStores>,
    hits: Vec<(PathBuf, crate::cache::Entry)>,
    miss_stamps: HashMap<PathBuf, (u64, u64, u64)>,
    total_jobs: usize,
    any_misses: bool,
    outcomes: Vec<(PathBuf, Outcome)>,
) -> DecodedMaps {
    if !hits.is_empty() {
        tracing::info!("sample cache: {} of {total_jobs} files hot", hits.len());
    }

    // Fresh results feed assembly; failures stay uncached and report
    // at assembly as always.
    let mut predecoded: HashMap<PathBuf, Result<(Sample, DecodedInfo), String>> = HashMap::new();
    let mut prereleased: HashMap<PathBuf, Result<Sample, String>> = HashMap::new();
    for (path, outcome) in outcomes {
        match outcome {
            Outcome::Attack(result) => {
                predecoded.insert(path, result);
            }
            Outcome::Release(result) => {
                prereleased.insert(path, result);
            }
        }
    }
    // Rewrite the cache — surviving hits plus fresh successes, all
    // borrowed in place: no sample is ever cloned to be cached.
    if let Some(path) = cache_path.filter(|_| any_misses) {
        let started = Instant::now();
        let mut refs: Vec<(&PathBuf, crate::cache::EntryRef)> = Vec::new();
        for (hit_path, entry) in &hits {
            refs.push((
                hit_path,
                crate::cache::EntryRef {
                    meta_hash: entry.meta_hash,
                    mtime_ns: entry.mtime_ns,
                    size: entry.size,
                    sample: &entry.sample,
                    info: entry.info,
                },
            ));
        }
        for (fresh_path, result) in &predecoded {
            if let (Ok((sample, info)), Some(&(meta_hash, mtime_ns, size))) =
                (result, miss_stamps.get(fresh_path))
            {
                refs.push((
                    fresh_path,
                    crate::cache::EntryRef {
                        meta_hash,
                        mtime_ns,
                        size,
                        sample,
                        info: Some(*info),
                    },
                ));
            }
        }
        for (fresh_path, result) in &prereleased {
            if let (Ok(sample), Some(&(meta_hash, mtime_ns, size))) =
                (result, miss_stamps.get(fresh_path))
            {
                refs.push((
                    fresh_path,
                    crate::cache::EntryRef {
                        meta_hash,
                        mtime_ns,
                        size,
                        sample,
                        info: None,
                    },
                ));
            }
        }
        let count = refs.len();
        let tails = tails_path
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| tails_path_for(path));
        match crate::cache::write(path, &tails, stores, refs.into_iter(), count) {
            Ok(()) => tracing::info!(
                "sample cache written: {count} entries in {:.1?}",
                started.elapsed()
            ),
            Err(err) => tracing::warn!("sample cache not written: {err}"),
        }
    }
    // Cache hits feed assembly like fresh decodes, moved — not cloned.
    for (path, entry) in hits {
        match entry.info {
            Some(info) => {
                predecoded.insert(path, Ok((entry.sample, info)));
            }
            None => {
                prereleased.insert(path, Ok(entry.sample));
            }
        }
    }
    DecodedMaps {
        predecoded,
        prereleased,
    }
}

/// Attack/release dedup state threaded across every rank: a file
/// shared by several pipes (borrowed pipes, shared samples) decodes
/// and enters the bank once.
struct DecodeCache {
    /// path → Ok(bank index + source metadata) or failure already noted.
    decoded: HashMap<PathBuf, Option<DecodedInfo>>,
    /// Separate release files, deduplicated independently of attacks.
    release_cache: HashMap<PathBuf, Option<u32>>,
}

/// Decode every sampled pipe in one rank into its bank entries: the
/// first attack variant that decodes becomes the pipe's primary (its
/// metadata drives the rank-wide pitch decision), the rest join the
/// selection table, and each attack's recorded releases attach as
/// splice targets. Attack and release files dedup by path via `cache`.
fn decode_rank_attacks(
    rank: &aristide_model::Rank,
    bank: &mut SampleBank,
    skipped: &mut Vec<String>,
    attack_options: &mut HashMap<(RankId, u16), Vec<AttackOption>>,
    cache: &mut DecodeCache,
    maps: &mut DecodedMaps,
) -> Vec<PendingPipe> {
    let mut pending: Vec<PendingPipe> = Vec::new();
    for (pipe_index, pipe) in rank.pipes.iter().enumerate() {
        let PipeSource::Sampled { attacks, releases } = &pipe.source else {
            continue;
        };
        if attacks.is_empty() {
            skipped.push(format!("{} pipe {pipe_index}: no attacks", rank.name));
            continue;
        }
        // Decode every attack variant; the first that decodes is
        // the pipe's primary (its metadata drives the rank-wide
        // pitch decision), the rest join the selection table.
        let mut variants: Vec<(usize, DecodedInfo)> = Vec::new();
        for (attack_index, attack) in attacks.iter().enumerate() {
            let entry = cache.decoded.entry(attack.path.clone()).or_insert_with(|| {
                match maps.predecoded.remove(&attack.path) {
                    Some(Ok((mut sample, info))) => {
                        // Separate recorded releases become their own
                        // one-shot bank entries, attached with hold-time
                        // bounds, trem state, and cross-file phase maps
                        // — to every attack variant, so a note started
                        // on any of them can splice out.
                        for release in releases {
                            let release_index = *cache
                                .release_cache
                                .entry(release.path.clone())
                                .or_insert_with(|| {
                                    match maps.prereleased.remove(&release.path) {
                                        Some(Ok(release_sample)) => {
                                            Some(bank.push(release_sample))
                                        }
                                        Some(Err(reason)) => {
                                            skipped.push(format!(
                                                "{}: {reason}",
                                                release.path.display()
                                            ));
                                            None
                                        }
                                        None => None,
                                    }
                                });
                            if let Some(index) = release_index
                                && let Some(target) = bank.get(index)
                            {
                                sample.attach_release(
                                    target,
                                    index,
                                    release.max_key_press_ms,
                                    release.wave_tremulant,
                                    release.release_crossfade_ms,
                                );
                            }
                        }
                        let index = bank.push(sample);
                        Some(DecodedInfo { index, ..info })
                    }
                    Some(Err(reason)) => {
                        skipped.push(format!("{}: {reason}", attack.path.display()));
                        None
                    }
                    None => None,
                }
            });
            if let Some(info) = *entry {
                variants.push((attack_index, info));
            }
        }
        let Some(&(primary_index, info)) = variants.first() else {
            continue;
        };
        let attack = &attacks[primary_index];
        if variants.len() > 1 {
            let options = variants
                .iter()
                .map(|&(index, variant)| AttackOption {
                    sample: variant.index,
                    rate_factor: (variant.sample_rate / info.sample_rate) as f32,
                    wave_tremulant: attacks[index].wave_tremulant,
                    min_velocity: attacks[index].min_velocity,
                    max_since_release_ms: attacks[index].max_time_since_last_release_ms,
                })
                .collect();
            attack_options.insert((rank.id, pipe_index as u16), options);
            // Wire the mid-hold recording switches. A wave tremulant
            // engaging or releasing crosses already-held voices from
            // the recording made under one state into the one made
            // under the other, so every ordered pair of variants whose
            // `IsTremulant` differs needs a loop→loop phase map.
            // Variants that agree on it never switch mid-hold — a note
            // does not change how hard it was struck, nor how long ago
            // the pipe last closed — so they cost nothing here.
            for &(a, from) in variants.iter() {
                for &(b, to) in variants.iter() {
                    if attacks[a].wave_tremulant != attacks[b].wave_tremulant {
                        bank.attach_switch(from.index, to.index);
                    }
                }
            }
        }

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
            let recorded_hz = equal_ladder_hz(key as f64 + fraction_cents / 100.0);
            cents_between(recorded_hz, pipe.nominal_frequency_hz)
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
    pending
}

/// Settle one rank's pitch decisions (the junk-metadata guard needs
/// the whole rank in view) and stage its pipes for the instrument-wide
/// tuning fit.
fn stage_rank_pipes(
    rank: &aristide_model::Rank,
    rank_index: usize,
    enclosures: [u8; aristide_engine::enclosure::MAX_VOICE_ENCLOSURES],
    pending: Vec<PendingPipe>,
    skipped: &mut Vec<String>,
    bank: &SampleBank,
) -> Vec<StagedPipe> {
    let mut staged = Vec::new();
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

    for p in pending {
        let pipe = &rank.pipes[p.pipe_index as usize];
        // What the metadata alone would decide — the fallback for
        // a pipe whose recording cannot be measured (no loop, or
        // material that doesn't repeat). The recording plays as
        // the set voiced it (as recorded + PitchTuning) unless
        // its own declared pitch says that lands somewhere else
        // entirely — then the set relies on retuning from
        // metadata (unit/extended ranks, borrowed top octaves,
        // HW-style sets). A pipe (or rank) declaring
        // AcceptsRetuning=N plays as voiced no matter what the
        // metadata claims.
        let metadata = match p.auto_cents.filter(|_| pipe.accepts_retuning) {
            Some(auto) if (auto - p.original_cents).abs() > RETUNE_TOLERANCE_CENTS => {
                if auto.abs() > 1800.0 {
                    // GO refuses retunes past 1800 cents; a claim
                    // that far out is junk metadata, not intent.
                    skipped.push(format!(
                        "{} pipe {}: embedded pitch asks for a {auto:.0}-cent \
                         retune; ignored",
                        rank.name, p.pipe_index
                    ));
                    None
                } else if p.from_smpl && distrust_smpl {
                    None
                } else {
                    Some(auto)
                }
            }
            _ => None,
        };
        // The recording's own fundamental, as the engine measured
        // it for release alignment: the truth every pitch decision
        // below works from.
        let measured_cents = bank
            .get(p.info.index)
            .and_then(|sample| sample.measured_period())
            .map(|period| {
                let recorded_hz = p.info.sample_rate / period;
                let voiced_hz = recorded_hz * cents_to_ratio(p.original_cents);
                cents_between(pipe.nominal_frequency_hz, voiced_hz)
            })
            .filter(|cents| cents.is_finite());
        staged.push(StagedPipe {
            rank: rank.id,
            rank_index,
            pipe_index: p.pipe_index,
            info: p.info,
            original_cents: p.original_cents,
            metadata_cents: metadata,
            measured_cents,
            enclosures,
        });
    }
    staged
}

/// A pipe's sounding pitch class against the 440 ladder (0 = A), plus
/// whether it sits exactly on a semitone.
fn sounding_class(hz: f64) -> (usize, bool) {
    let semitones = 12.0 * (hz / 440.0).log2();
    let nearest = semitones.round();
    (
        (nearest as i64 + 69).rem_euclid(12) as usize,
        (semitones - nearest).abs() < 0.005,
    )
}

/// Where the organ's own tuning puts a pipe: its rank's pitch standard
/// plus the instrument's tempering of its class.
fn model_of(
    organ: &Organ,
    home: &crate::tuning::HomeTuning,
    anchors: &HashMap<RankId, f64>,
    p: &StagedPipe,
) -> f64 {
    let (class, _) = sounding_class(nominal_of(organ, p));
    anchors
        .get(&p.rank)
        .copied()
        .unwrap_or_else(|| home.anchor_cents())
        + home.offsets_cents[class]
}

/// The organ's home tuning, from every pipe that measured: what the
/// samples were recorded in. Each rank anchors on its own median (a
/// composite may hold a 415 Positif beside a 440 Great, and a rank
/// comes from one set); the class table is instrument-wide.
fn fit_home_tuning(
    organ: &Organ,
    staged: &[StagedPipe],
) -> (Option<crate::tuning::HomeTuning>, HashMap<RankId, f64>) {
    let total = staged.len();
    let measured_total = staged.iter().filter(|p| p.measured_cents.is_some()).count();
    let fit = |keep: &dyn Fn(&StagedPipe) -> bool| {
        let home = crate::tuning::HomeTuning::fit(
            staged.iter().filter(|p| keep(p)).filter_map(|p| {
                let (class, on_ladder) = sounding_class(nominal_of(organ, p));
                p.measured_cents.map(|cents| (class, cents, on_ladder))
            }),
            total,
        );
        let mut per_rank: HashMap<RankId, Vec<f64>> = HashMap::new();
        for p in staged.iter().filter(|p| keep(p)) {
            if let Some(cents) = p.measured_cents {
                per_rank.entry(p.rank).or_default().push(cents);
            }
        }
        let anchors: HashMap<RankId, f64> = per_rank
            .into_iter()
            .filter_map(|(rank, mut values)| {
                crate::tuning::median(&mut values).map(|median| (rank, median))
            })
            .collect();
        (home, anchors)
    };
    // Two passes: the first fit finds the pipes sitting at another key
    // (see REANCHOR_TOLERANCE_CENTS), the second leaves them out so a
    // mis-keyed file cannot skew the class it lands in.
    let (mut home, rank_anchor) = match fit(&|_| true) {
        (Some(first), first_anchors) => fit(&|p| {
            p.measured_cents.is_none_or(|measured| {
                (measured - model_of(organ, &first, &first_anchors, p)).abs()
                    <= REANCHOR_TOLERANCE_CENTS
            })
        }),
        none => none,
    };
    if let Some(home) = home.as_mut() {
        home.measured = measured_total;
    }
    (home, rank_anchor)
}

/// Turn every staged pipe into its playback spec, moving pipes that
/// sat at another key onto the organ's tuning by measurement and
/// retuning unmeasured ones from their embedded metadata when it
/// applies (see `fit_home_tuning` and `RETUNE_TOLERANCE_CENTS`).
fn assign_voice_specs(
    organ: &Organ,
    device_rate: f32,
    home: Option<&crate::tuning::HomeTuning>,
    rank_anchor: &HashMap<RankId, f64>,
    staged: Vec<StagedPipe>,
    skipped: &mut Vec<String>,
) -> HashMap<(RankId, u16), VoiceSpec> {
    let mut specs = HashMap::new();
    let mut reanchored: HashMap<RankId, (usize, f64)> = HashMap::new();
    let mut retuned: HashMap<RankId, (usize, f64)> = HashMap::new();
    for p in staged {
        let rank = &organ.ranks[p.rank_index];
        let pipe = &rank.pipes[p.pipe_index as usize];
        let model = home.map(|home| model_of(organ, home, rank_anchor, &p));
        let (cents, home_cents) = match (p.measured_cents, model) {
            // Within the tolerance the pipe is where the organ's
            // tuning has it — temperament and drift, kept exactly.
            // Beyond it the sample sits at another key (a borrowed
            // neighbour, a mis-keyed file): playing it as voiced would
            // be a semitone wrong, so it is moved onto the model —
            // from its measured pitch, no metadata needed.
            (Some(measured), Some(model)) => {
                let residual = measured - model;
                if residual.abs() <= REANCHOR_TOLERANCE_CENTS {
                    (p.original_cents, measured)
                } else {
                    let entry = reanchored.entry(p.rank).or_insert((0, 0.0));
                    entry.0 += 1;
                    entry.1 = entry.1.max(residual.abs());
                    (p.original_cents - residual, model)
                }
            }
            (Some(measured), None) => (p.original_cents, measured),
            // Unmeasured: the metadata decides as it always did, and
            // the pipe is assumed to sit where its organ does — or
            // where the retune declared, when one applied.
            (None, model) => match p.metadata_cents {
                Some(auto) => {
                    let entry = retuned.entry(p.rank).or_insert((0, 0.0));
                    entry.0 += 1;
                    entry.1 = entry.1.max((auto - p.original_cents).abs());
                    (auto, pipe.pitch_correction_cents)
                }
                None => (p.original_cents, model.unwrap_or(0.0)),
            },
        };
        specs.insert(
            (rank.id, p.pipe_index),
            VoiceSpec {
                sample: p.info.index,
                rate: (p.info.sample_rate / device_rate as f64 * cents_to_ratio(cents)) as f32,
                nominal_hz: pipe.nominal_frequency_hz as f32,
                home_cents: home_cents as f32,
                model_cents: model.unwrap_or(home_cents) as f32,
                gain: db_to_linear(pipe.gain_db) as f32,
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
                voicing_tilt: 1.0,
                enclosures: p.enclosures,
                bus: 0,
                delay_frames: 0,
            },
        );
    }
    for rank in &organ.ranks {
        if let Some(&(count, largest)) = retuned.get(&rank.id) {
            skipped.push(format!(
                "{}: {count} pipe(s) retuned to their recorded-pitch metadata \
                 (largest shift {largest:.0} cents)",
                rank.name
            ));
        }
        if let Some(&(count, largest)) = reanchored.get(&rank.id) {
            skipped.push(format!(
                "{}: {count} pipe(s) sat at another key and were moved onto the \
                 organ's tuning by measurement (largest shift {largest:.0} cents)",
                rank.name
            ));
        }
    }
    specs
}

/// Borrowed pipes sound their target pipe verbatim.
fn assign_borrowed_pipe_specs(
    organ: &Organ,
    specs: &mut HashMap<(RankId, u16), VoiceSpec>,
    attack_options: &mut HashMap<(RankId, u16), Vec<AttackOption>>,
    skipped: &mut Vec<String>,
) {
    for rank in &organ.ranks {
        for (pipe_index, pipe) in rank.pipes.iter().enumerate() {
            if !matches!(pipe.source, PipeSource::Borrowed(_)) {
                continue;
            }
            let target = resolve_borrow(organ, pipe);
            match target.and_then(|t| specs.get(&(t.rank, t.pipe)).copied().map(|s| (t, s))) {
                Some((target, spec)) => {
                    specs.insert((rank.id, pipe_index as u16), spec);
                    if let Some(options) = attack_options.get(&(target.rank, target.pipe)) {
                        let options = options.clone();
                        attack_options.insert((rank.id, pipe_index as u16), options);
                    }
                }
                None => skipped.push(format!(
                    "{} pipe {pipe_index}: borrow target has no sample",
                    rank.name
                )),
            }
        }
    }
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

/// How far a measured pipe may sit from where the organ's own tuning
/// puts it before it is taken to be at another key altogether. Real
/// tuning — the widest temperament offsets, decades of drift — stays
/// within a quarter-tone of the model; a sample reused from the
/// neighbouring key sits a semitone off.
const REANCHOR_TOLERANCE_CENTS: f64 = 50.0;

/// One sampled pipe after decode, awaiting the instrument-wide pitch
/// decisions (see `REANCHOR_TOLERANCE_CENTS`).
struct StagedPipe {
    rank: RankId,
    rank_index: usize,
    pipe_index: u16,
    info: DecodedInfo,
    enclosures: [u8; aristide_engine::enclosure::MAX_VOICE_ENCLOSURES],
    /// Playback offset as the set voiced it: PitchTuning et al.
    original_cents: f64,
    /// The offset the metadata path would retune by, when it would.
    metadata_cents: Option<f64>,
    /// Where the recording really sounds, played as voiced, relative
    /// to the pipe's nominal — from the engine's period measurement.
    measured_cents: Option<f64>,
}

fn nominal_of(organ: &Organ, staged: &StagedPipe) -> f64 {
    organ.ranks[staged.rank_index].pipes[staged.pipe_index as usize].nominal_frequency_hz
}

#[derive(Clone, Copy)]
pub(crate) struct DecodedInfo {
    pub(crate) index: u32,
    pub(crate) sample_rate: f64,
    pub(crate) percussive: bool,
    /// The file's own claim of what pitch it holds: `smpl`-chunk unity
    /// note (0 = "not set", as GO reads it) plus its fraction in cents
    /// above that note.
    pub(crate) unity_note: Option<u8>,
    pub(crate) unity_fraction_cents: f64,
}

/// Decode one attack file into an engine [`Sample`].
///
/// Loop points come from the ODF when declared, else from the file's
/// `smpl` chunk (both use inclusive end frames). The release tail starts
/// at the file's last cue marker past the loop, else right after it —
/// GO's own fallback order.
fn decode(
    path: &std::path::Path,
    attack: &aristide_model::AttackSample,
) -> Result<(Sample, DecodedInfo), String> {
    let mut file = wav::read(path).map_err(|e| e.to_string())?;
    // ODF ReleaseEnd: the embedded tail ends here; material past it is
    // the producer saying "don't play this".
    if let Some(end) = attack.release_end_frame {
        let end = u64::from(end);
        if end > 0 && end < file.info.frames {
            file.samples.truncate(end as usize * file.info.channels as usize);
            file.info.frames = end;
        }
    }
    let frames = file.info.frames;
    let loop_crossfade_ms = attack.loop_crossfade_ms;

    let mut loops: Vec<(u64, u64)> = if attack.loops.is_empty() {
        file.info.loops.iter().map(|l| (l.start, l.end + 1)).collect()
    } else {
        attack.loops.iter().map(|l| (l.start, l.end + 1)).collect()
    };
    loops.retain(|&(start, end)| start < end && end <= frames);
    // Longest loop wins until multi-loop selection lands (M4).
    let sustain_loop = loops.iter().copied().max_by_key(|&(start, end)| end - start);

    // The producer asked for a loop crossfade: bake GO's raised-cosine
    // blend into the last `fade` frames of each loop, fading the
    // recorded material into the stretch that *precedes* loop start —
    // after which wrapping to the start continues the waveform. Butt
    // loops stay the default (Appleton 2019); this runs only when the
    // ODF says the loop points need the help.
    let fade = (u64::from(loop_crossfade_ms) * u64::from(file.info.sample_rate)) / 1000;
    if fade > 0 {
        let channels = file.info.channels as usize;
        for &(start, end) in &loops {
            if start < fade || end - start <= fade {
                continue; // GO drops these loops; we keep them butt-spliced
            }
            for pos in 0..fade {
                let keep = ((core::f64::consts::PI * (pos as f64 + 0.5) / fade as f64)
                    .cos()
                    + 1.0)
                    * 0.5;
                let into = end - fade + pos;
                let from = start - fade + pos;
                for channel in 0..channels {
                    let a = file.samples[into as usize * channels + channel];
                    let b = file.samples[from as usize * channels + channel];
                    file.samples[into as usize * channels + channel] =
                        (a as f64 * keep + b as f64 * (1.0 - keep)) as f32;
                }
            }
        }
    }

    let release_start = match sustain_loop {
        // An explicit ODF CuePoint outranks the file's own cue chunk;
        // one that lands inside the loop (or past the data) is junk
        // and falls back to the file's markers.
        Some((_, loop_end)) => attack
            .cue_point_frame
            .map(u64::from)
            .filter(|&cue| cue >= loop_end && cue < frames)
            .unwrap_or_else(|| {
                file.info
                    .cue_points
                    .iter()
                    .copied()
                    .filter(|&cue| cue >= loop_end && cue < frames)
                    .max()
                    .unwrap_or(loop_end)
            }),
        None => frames,
    };

    let mut sample = Sample::new(
        file.samples,
        file.info.channels,
        file.info.sample_rate as f32,
        sustain_loop,
        release_start,
    )?;
    sample.set_attack_start(u64::from(attack.attack_start_frame));
    sample.set_release_crossfade_ms(attack.release_crossfade_ms);
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
/// decay), played from its start on key-off. ODF `CuePoint` skips a
/// lead-in, `ReleaseEnd` trims the far end — GO builds the release
/// section between exactly those markers.
fn decode_release(
    path: &std::path::Path,
    release: &aristide_model::ReleaseSample,
) -> Result<Sample, String> {
    let file = wav::read(path).map_err(|e| e.to_string())?;
    let frames = file.info.frames;
    let channels = file.info.channels as usize;
    let start = release
        .cue_point_frame
        .map(u64::from)
        .filter(|&cue| cue < frames)
        .unwrap_or(0);
    let end = release
        .release_end_frame
        .map(u64::from)
        .filter(|&end| end > start && end <= frames)
        .unwrap_or(frames);
    let samples = if (start, end) == (0, frames) {
        file.samples
    } else {
        file.samples[start as usize * channels..end as usize * channels].to_vec()
    };
    Sample::new(
        samples,
        file.info.channels,
        file.info.sample_rate as f32,
        None,
        end - start,
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

/// One-pole coefficient for the voice's brightness tilt, hinged around
/// the pipe's 2nd harmonic so "upper partials" breathe with pressure
/// while the fundamental stays put. Deep bass keeps a floor on the
/// hinge (HW had to disable bass brightness modulation for distortion;
/// a 150 Hz floor sidesteps that). Percussive noises skip the filter.
pub(crate) fn brightness_coefficient(frequency_hz: f64, device_rate: f32, percussive: bool) -> f32 {
    if percussive || frequency_hz.is_nan() || frequency_hz <= 0.0 {
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
    if percussive || frequency_hz.is_nan() || frequency_hz <= 0.0 {
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
                accepts_retuning: true,
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

    /// A looped mono tone at `hz` (44.1 kHz, 8192 frames, loop over
    /// most of it, a few harmonics) with no smpl pitch claim — what
    /// a real recording is to the measurer.
    fn write_tone_wav(path: &Path, hz: f64) {
        let rate = 44_100u32;
        let frames = 8192u32;
        let (loop_start, loop_end) = (512u32, 7680u32);
        let mut bytes = Vec::new();
        let mut chunk = |id: &[u8; 4], payload: &[u8]| {
            bytes.extend_from_slice(id);
            bytes.extend_from_slice(&(payload.len() as u32).to_le_bytes());
            bytes.extend_from_slice(payload);
        };
        let mut fmt = Vec::new();
        fmt.extend_from_slice(&1u16.to_le_bytes());
        fmt.extend_from_slice(&1u16.to_le_bytes());
        fmt.extend_from_slice(&rate.to_le_bytes());
        fmt.extend_from_slice(&(rate * 2).to_le_bytes());
        fmt.extend_from_slice(&2u16.to_le_bytes());
        fmt.extend_from_slice(&16u16.to_le_bytes());
        chunk(b"fmt ", &fmt);
        let mut smpl = vec![0u8; 36];
        smpl[28..32].copy_from_slice(&1u32.to_le_bytes());
        let mut record = [0u8; 24];
        record[8..12].copy_from_slice(&loop_start.to_le_bytes());
        record[12..16].copy_from_slice(&(loop_end - 1).to_le_bytes());
        smpl.extend_from_slice(&record);
        chunk(b"smpl", &smpl);
        let mut pcm = Vec::new();
        for i in 0..frames {
            let phase = std::f64::consts::TAU * hz * i as f64 / rate as f64;
            let value = phase.sin() + 0.5 * (2.0 * phase).sin() + 0.3 * (3.0 * phase).sin();
            pcm.extend_from_slice(&((value * 12000.0) as i16).to_le_bytes());
        }
        chunk(b"data", &pcm);
        let mut riff = Vec::new();
        riff.extend_from_slice(b"RIFF");
        riff.extend_from_slice(&((bytes.len() + 4) as u32).to_le_bytes());
        riff.extend_from_slice(b"WAVE");
        riff.extend_from_slice(&bytes);
        std::fs::write(path, riff).expect("write tone");
    }

    /// A set recorded at a′ = 415 in ¼-comma meantone, with no pitch
    /// metadata at all, loads as exactly that: the home fit names the
    /// pitch standard and the temperament, every pipe plays as
    /// recorded (rate = the plain sample-rate ratio) and carries its
    /// measured offset for a target tuning to bend from. One pipe
    /// whose file is really the neighbouring key's is caught by
    /// measurement and moved onto the organ's own tuning.
    #[test]
    fn measured_home_tuning_of_a_baroque_set() {
        let dir = std::env::temp_dir().join("aristide-home-tuning-test");
        std::fs::create_dir_all(&dir).expect("test dir");
        let table = crate::tuning::Temperament::Meantone4.offsets_cents();
        let anchor = 1200.0 * (415.0f64 / 440.0).log2();
        let recorded = |midi: u8| {
            let class = (midi % 12) as usize;
            equal_ladder_hz(midi as f64) * ((anchor + table[class] as f64) / 1200.0).exp2()
        };
        let mut pipes = Vec::new();
        for midi in 36u8..=71 {
            let name = format!("{midi}.wav");
            // Key 65's file is the recording of key 64: a semitone flat
            // of where the organ's tuning would have it.
            let hz = if midi == 65 { recorded(64) } else { recorded(midi) };
            write_tone_wav(&dir.join(&name), hz);
            pipes.push(aristide_model::Pipe {
                nominal_frequency_hz: equal_ladder_hz(midi as f64),
                pitch_tuning_cents: 0.0,
                pitch_correction_cents: 0.0,
                gain_db: 0.0,
                midi_key_number: None,
                midi_pitch_fraction_cents: None,
                accepts_retuning: true,
                source: aristide_model::PipeSource::Sampled {
                    attacks: vec![aristide_model::AttackSample {
                        path: PathBuf::from(name),
                        ..Default::default()
                    }],
                    releases: Vec::new(),
                },
            });
        }
        let organ = aristide_model::Organ {
            name: "baroque".into(),
            base_path: dir,
            ranks: vec![aristide_model::Rank {
                id: aristide_model::RankId(1),
                name: "Principal 8".into(),
                windchest: 1,
                velocity_volume: Default::default(),
                pipes,
            }],
            ..Default::default()
        };
        let loaded = build(&organ, 48_000.0, 32, None).expect("builds");
        let home = loaded.home.expect("pipes measured");
        assert!((home.a4_hz - 415.0).abs() < 0.3, "a′ = {}", home.a4_hz);
        assert_eq!(
            home.temperament,
            Some(crate::tuning::Temperament::Meantone4),
            "{home:?}"
        );
        assert_eq!((home.measured, home.pipes), (36, 36));
        assert!(home.spread_cents < 1.0, "spread {}", home.spread_cents);

        let ratio = 44_100.0f32 / 48_000.0;
        for midi in 36u8..=71 {
            let spec = loaded.specs[&(aristide_model::RankId(1), (midi - 36) as u16)];
            let class = (midi % 12) as usize;
            let model = anchor + table[class] as f64;
            assert!(
                (spec.home_cents as f64 - model).abs() < 0.5,
                "key {midi}: home {} vs model {model}",
                spec.home_cents
            );
            assert!((spec.model_cents as f64 - model).abs() < 0.5, "key {midi}: model");
            if midi == 65 {
                // Moved up the semitone its file is short of — E's
                // recording to F's place, the tempering of each
                // included — so it sounds where the organ's tuning
                // puts F.
                let expected = ratio * ((100.0 + table[5] - table[4]) / 1200.0).exp2();
                assert!(
                    (spec.rate / expected - 1.0).abs() < 1e-3,
                    "mis-keyed pipe re-anchored: {} vs {expected}",
                    spec.rate
                );
            } else {
                assert!((spec.rate - ratio).abs() < 1e-6, "key {midi} plays as recorded");
            }
        }
        assert!(
            loaded.skipped.iter().any(|s| s.contains("1 pipe(s) sat at another key")),
            "{:?}",
            loaded.skipped
        );
    }

    /// Every attack variant decodes into the bank and the selection
    /// table carries GO's metadata; separate releases attach to each
    /// variant with their trem state.
    #[test]
    fn multi_attack_pipes_build_selection_tables() {
        let dir = std::env::temp_dir().join("aristide-multi-attack-test");
        std::fs::create_dir_all(&dir).expect("test dir");
        for name in ["plain.wav", "trem.wav", "rel.wav"] {
            write_test_wav(&dir.join(name), 60);
        }
        let organ = aristide_model::Organ {
            name: "multi attack".into(),
            base_path: dir,
            ranks: vec![aristide_model::Rank {
                id: aristide_model::RankId(1),
                name: "Test rank".into(),
                windchest: 1,
                velocity_volume: Default::default(),
                pipes: vec![aristide_model::Pipe {
                    nominal_frequency_hz: 440.0,
                    pitch_tuning_cents: 0.0,
                    pitch_correction_cents: 0.0,
                    gain_db: 0.0,
                    midi_key_number: None,
                    midi_pitch_fraction_cents: None,
                    accepts_retuning: true,
                    source: aristide_model::PipeSource::Sampled {
                        attacks: vec![
                            aristide_model::AttackSample {
                                path: PathBuf::from("plain.wav"),
                                wave_tremulant: Some(false),
                                ..Default::default()
                            },
                            aristide_model::AttackSample {
                                path: PathBuf::from("trem.wav"),
                                wave_tremulant: Some(true),
                                min_velocity: 64,
                                max_time_since_last_release_ms: Some(250),
                                ..Default::default()
                            },
                        ],
                        releases: vec![aristide_model::ReleaseSample {
                            path: PathBuf::from("rel.wav"),
                            wave_tremulant: Some(true),
                            ..Default::default()
                        }],
                    },
                }],
            }],
            ..Default::default()
        };
        let loaded = build(&organ, 44_100.0, 16, None).expect("builds");
        assert_eq!(loaded.bank.len(), 3, "two attacks + one release decoded");
        let key = (aristide_model::RankId(1), 0u16);
        let options = loaded.attack_options.get(&key).expect("selection table");
        assert_eq!(options.len(), 2);
        assert_eq!(options[0].sample, loaded.specs[&key].sample, "primary first");
        assert_eq!(options[0].wave_tremulant, Some(false));
        assert_eq!(options[1].wave_tremulant, Some(true));
        assert_eq!(options[1].min_velocity, 64);
        assert_eq!(options[1].max_since_release_ms, Some(250));
        assert!((options[1].rate_factor - 1.0).abs() < 1e-6, "same file rate");
        // Both variants can splice out to the separate release.
        for option in options {
            let sample = loaded.bank.get(option.sample).expect("sample");
            assert_eq!(sample.release_options().len(), 1);
            assert_eq!(sample.release_options()[0].wave_trem, Some(true));
        }
    }

    /// Not a test: `cargo test --release -p aristide-server bench_demo_cache -- --ignored --nocapture`
    #[test]
    #[ignore = "manual timing: demo set cold vs warm cache"]
    fn bench_demo_cache() {
        let Some(path) = demo_organ() else {
            eprintln!("skipping: demo set not present");
            return;
        };
        let organ = aristide_formats::grandorgue::load(&path).expect("loads").organ;
        let cache_dir = std::env::temp_dir().join("aristide-cache-bench");
        let _ = std::fs::remove_dir_all(&cache_dir);
        let cache = cache_dir.join("demo.samples");
        for label in ["cold", "warm", "warm2"] {
            let started = std::time::Instant::now();
            let loaded = build(&organ, 48_000.0, 16, Some(&cache)).expect("builds");
            println!(
                "{label}: {:?} for {} samples, {:.1} MiB resident",
                started.elapsed(),
                loaded.bank.len(),
                loaded.bank.resident_bytes() as f64 / (1024.0 * 1024.0)
            );
        }
        let _ = std::fs::remove_dir_all(&cache_dir);
    }

    /// The load cache: a warm load must come from the cache (proven by
    /// corrupting the source file while faking its stamp), and a
    /// changed stamp must invalidate the entry.
    #[test]
    fn sample_cache_hits_and_invalidates() {
        let organ = pitch_test_organ("cache", &[("a.wav", 60, 60.0, 0.0, None)]);
        let wav = organ.base_path.join("a.wav");
        let cache_dir = std::env::temp_dir().join("aristide-cache-test");
        let _ = std::fs::remove_dir_all(&cache_dir);
        let cache = cache_dir.join("test.samples");

        let cold = build(&organ, 44_100.0, 16, Some(&cache)).expect("cold build");
        assert!(cache.is_file(), "cache file written");
        assert!(cold.skipped.is_empty(), "{:?}", cold.skipped);

        // Corrupt the source but restore its stamp: only the cache can
        // now produce a playable sample.
        let stamp = std::fs::metadata(&wav).expect("stat").modified().expect("mtime");
        let size = std::fs::metadata(&wav).expect("stat").len();
        std::fs::write(&wav, vec![0u8; size as usize]).expect("corrupt");
        let file = std::fs::File::options()
            .write(true)
            .open(&wav)
            .expect("reopen");
        file.set_modified(stamp).expect("restore mtime");
        drop(file);

        let warm = build(&organ, 44_100.0, 16, Some(&cache)).expect("warm build");
        assert!(warm.skipped.is_empty(), "cache must serve: {:?}", warm.skipped);
        assert_eq!(cold.bank.resident_bytes(), warm.bank.resident_bytes());
        assert_eq!(cold.bank.pre_fault(), warm.bank.pre_fault(), "identical audio");

        // Move the stamp: the entry must invalidate, and the corrupted
        // file now fails to decode — honestly reported, not served
        // stale.
        let file = std::fs::File::options().write(true).open(&wav).expect("reopen");
        file.set_modified(std::time::SystemTime::now()).expect("bump mtime");
        drop(file);
        let stale = build(&organ, 44_100.0, 16, Some(&cache)).expect("stale build");
        assert!(
            !stale.skipped.is_empty(),
            "a changed file must be re-decoded, not served from cache"
        );
        let _ = std::fs::remove_dir_all(&cache_dir);
    }

    /// The default 16-bit residency halves sample RAM against f32 and
    /// changes nothing else the pitch pipeline computes.
    #[test]
    fn sixteen_bit_residency_halves_ram() {
        let organ = pitch_test_organ("bits", &[("a.wav", 60, 60.0, 0.0, None)]);
        let compact = build(&organ, 44_100.0, 16, None).expect("builds");
        let full = build(&organ, 44_100.0, 32, None).expect("builds");
        assert_eq!(
            compact.bank.resident_bytes() * 2,
            full.bank.resident_bytes(),
            "i16 must be exactly half of f32"
        );
        assert_eq!(
            format!("{:?}", compact.specs[&(aristide_model::RankId(1), 0u16)]),
            format!("{:?}", full.specs[&(aristide_model::RankId(1), 0u16)]),
            "residency must not touch the playback spec"
        );
    }

    /// ODF `AcceptsRetuning=N`: the pipe plays as voiced no matter how
    /// far its metadata says it sits from the slot.
    #[test]
    fn accepts_retuning_off_disables_the_auto_retune() {
        // smpl claims 60, slot wants 57: normally a 3-semitone retune.
        let mut organ =
            pitch_test_organ("no-retune", &[("borrowed.wav", 60, 57.0, 0.0, None)]);
        organ.ranks[0].pipes[0].accepts_retuning = false;
        let loaded = build(&organ, 44_100.0, 16, None).expect("builds");
        assert!(
            rate_cents(&loaded, 0).abs() < 1.0,
            "declared AcceptsRetuning=N must play as voiced: {} cents",
            rate_cents(&loaded, 0)
        );
    }

    /// ODF sample boundaries: `AttackStart` moves the playback origin,
    /// `ReleaseEnd` trims an attack's embedded tail, and a separate
    /// release is cut to its `CuePoint`..`ReleaseEnd` window; the
    /// producer's `ReleaseCrossfadeLength` rides the release option.
    #[test]
    fn odf_sample_boundaries_are_honoured() {
        let dir = std::env::temp_dir().join("aristide-boundaries-test");
        std::fs::create_dir_all(&dir).expect("test dir");
        write_test_wav(&dir.join("att.wav"), 60); // 512 frames, loop 64..447
        write_test_wav(&dir.join("rel.wav"), 60);
        let organ = aristide_model::Organ {
            name: "boundaries".into(),
            base_path: dir,
            ranks: vec![aristide_model::Rank {
                id: aristide_model::RankId(1),
                name: "Test rank".into(),
                windchest: 1,
                velocity_volume: Default::default(),
                pipes: vec![aristide_model::Pipe {
                    nominal_frequency_hz: 440.0,
                    pitch_tuning_cents: 0.0,
                    pitch_correction_cents: 0.0,
                    gain_db: 0.0,
                    midi_key_number: None,
                    midi_pitch_fraction_cents: None,
                    accepts_retuning: true,
                    source: aristide_model::PipeSource::Sampled {
                        attacks: vec![aristide_model::AttackSample {
                            path: PathBuf::from("att.wav"),
                            attack_start_frame: 32,
                            release_end_frame: Some(500),
                            release_crossfade_ms: 120,
                            ..Default::default()
                        }],
                        releases: vec![aristide_model::ReleaseSample {
                            path: PathBuf::from("rel.wav"),
                            cue_point_frame: Some(100),
                            release_end_frame: Some(400),
                            release_crossfade_ms: 80,
                            ..Default::default()
                        }],
                    },
                }],
            }],
            ..Default::default()
        };
        let loaded = build(&organ, 44_100.0, 16, None).expect("builds");
        let spec = loaded.specs[&(aristide_model::RankId(1), 0u16)];
        let sample = loaded.bank.get(spec.sample).expect("attack sample");
        assert_eq!(sample.attack_start(), 32);
        assert_eq!(sample.frames(), 500, "ReleaseEnd trims the attack file");
        assert_eq!(sample.release_crossfade_ms(), 120);
        let option = &sample.release_options()[0];
        assert_eq!(option.crossfade_ms, 80);
        let release = loaded.bank.get(option.sample).expect("release sample");
        assert_eq!(
            release.frames(),
            300,
            "the release is the CuePoint..ReleaseEnd window"
        );
    }

    /// ODF `LoopCrossfadeLength` (ms): the loop's last frames blend
    /// into the material preceding loop start, so the wrap continues
    /// the waveform instead of thumping (GO's raised-cosine bake).
    #[test]
    fn odf_loop_crossfade_is_baked_at_decode() {
        let dir = std::env::temp_dir().join("aristide-loop-crossfade-test");
        std::fs::create_dir_all(&dir).expect("test dir");
        write_test_wav(&dir.join("looped.wav"), 60);
        let organ_with = |crossfade_ms: u16| aristide_model::Organ {
            name: "crossfade".into(),
            base_path: dir.clone(),
            ranks: vec![aristide_model::Rank {
                id: aristide_model::RankId(1),
                name: "Test rank".into(),
                windchest: 1,
                velocity_volume: Default::default(),
                pipes: vec![aristide_model::Pipe {
                    nominal_frequency_hz: 440.0,
                    pitch_tuning_cents: 0.0,
                    pitch_correction_cents: 0.0,
                    gain_db: 0.0,
                    midi_key_number: None,
                    midi_pitch_fraction_cents: None,
                    accepts_retuning: true,
                    source: aristide_model::PipeSource::Sampled {
                        attacks: vec![aristide_model::AttackSample {
                            path: PathBuf::from("looped.wav"),
                            loop_crossfade_ms: crossfade_ms,
                            ..Default::default()
                        }],
                        releases: Vec::new(),
                    },
                }],
            }],
            ..Default::default()
        };
        let seam_step = |crossfade_ms: u16| -> f32 {
            let loaded = build(&organ_with(crossfade_ms), 44_100.0, 16, None).expect("builds");
            let spec = loaded.specs[&(aristide_model::RankId(1), 0u16)];
            let sample = loaded.bank.get(spec.sample).expect("sample");
            let (start, end) = sample.sustain_loop().expect("loop");
            // Wrap continuity: the loop's final frame should sit where
            // the frame before loop start sits, so end−1 → start reads
            // like start−1 → start.
            let (last, _) = sample.read((end - 1) as f64);
            let (before_start, _) = sample.read((start - 1) as f64);
            (last - before_start).abs()
        };
        // The fixture's smpl loop (64..447 of a slow sine) is a bad
        // butt splice on purpose.
        let butt = seam_step(0);
        let faded = seam_step(1); // 1 ms @ 44.1 kHz = 44 frames
        assert!(butt > 0.05, "fixture loop should butt-splice badly: {butt}");
        assert!(
            faded < 0.01,
            "crossfaded seam should continue the waveform: {faded} (butt {butt})"
        );
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
        let loaded = build(&organ, 44_100.0, 16, None).expect("bank builds");
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
        let loaded = build(&organ, 44_100.0, 16, None).expect("bank builds");
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
        let loaded = build(&organ, 44_100.0, 16, None).expect("bank builds");
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

    /// Boxes nest: GO's `[WindchestGroupNNN]` may list several
    /// enclosures (`NumberOfEnclosures`) and the Hauptwerk reader keys
    /// a windchest by its whole sorted enclosure set, so a chest inside
    /// a box inside a box must reach the voice as BOTH memberships —
    /// no set in `testsets/` nests, so this is checked on a synthetic
    /// organ. Beyond the engine's slots the surplus is dropped with a
    /// note, and duplicates never attenuate twice.
    #[test]
    fn a_chest_carries_every_box_it_sits_in() {
        let chest = |number: u32, enclosures: Vec<u32>| aristide_model::Windchest {
            number,
            name: format!("chest {number}"),
            enclosures,
            tremulants: Vec::new(),
        };
        let organ = Organ {
            windchests: vec![
                chest(1, vec![]),
                chest(2, vec![3]),
                chest(3, vec![3, 1]),
                chest(4, vec![2, 2]),
                chest(5, vec![0, 1, 2]),
            ],
            ..Organ::default()
        };
        let mut skipped = Vec::new();
        let resolved = resolve_chest_enclosures(&organ, &mut skipped);
        assert!(!resolved.contains_key(&1), "an unenclosed chest joins no box");
        assert_eq!(
            resolved[&2],
            [3, aristide_engine::enclosure::ENCLOSURE_NONE]
        );
        assert_eq!(resolved[&3], [3, 1], "inner box first, outer box second");
        assert_eq!(
            resolved[&4],
            [2, aristide_engine::enclosure::ENCLOSURE_NONE],
            "a box listed twice must not attenuate twice"
        );
        assert_eq!(resolved[&5], [0, 1], "the third box does not fit");
        assert_eq!(skipped.len(), 1, "exactly the dropped membership warns");
        assert!(skipped[0].contains("enclosure 2 is ignored"), "{}", skipped[0]);
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

        let loaded = build(&organ, 44_100.0, 16, None).expect("bank builds");
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
        assert_eq!(spec_for("Hautbois").enclosures[0], 0);
        assert_eq!(spec_for("Plein jeu III").enclosures[0], 1);
        assert_eq!(
            spec_for("Montre").enclosures[0],
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
        let single = build(&load(), 44_100.0, 16, None).expect("bank builds");
        let implicit = aristide_formats::instrument::Definition {
            name: "Twice".into(),
            ..Default::default()
        };
        let sources = vec![("A".to_string(), load()), ("B".to_string(), load())];
        let organ = aristide_formats::instrument::assemble(&implicit, &sources, Vec::new())
            .expect("assembles")
            .organ;
        assert_eq!(organ.stops.len(), load().stops.len() * 2);
        let loaded = build(&organ, 44_100.0, 16, None).expect("merged bank builds");
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
                loaded.specs[&(range.rank, range.first_pipe)].enclosures[0]
            })
            .collect();
        assert_eq!(hautbois, vec![0, 2]);
    }

    /// Streaming the demo set: the same audio out of the engine, with
    /// the tails on disk instead of in RAM. Renders a held note and its
    /// release through both banks, pumping the streamer by hand so the
    /// comparison is deterministic (no threads, no sleeping).
    #[test]
    fn streaming_the_demo_set_renders_identically() {
        let Some(path) = demo_organ() else { return };
        let organ = aristide_formats::grandorgue::load(&path).expect("loads").organ;
        let device_rate = 44_100.0f32;
        let resident = build(&organ, device_rate, 16, None).expect("resident build");
        let streamed =
            build_with(&organ, device_rate, 16, None, StreamingPolicy::ON).expect("streamed build");

        assert_eq!(resident.bank.len(), streamed.bank.len(), "same samples");
        assert!(
            streamed.bank.streamed_samples() > 0,
            "nothing streamed at all"
        );
        assert!(
            streamed.bank.resident_bytes() < resident.bank.resident_bytes(),
            "streaming freed no RAM: {} vs {}",
            streamed.bank.resident_bytes(),
            resident.bank.resident_bytes()
        );
        println!(
            "demo set: {:.1} MiB resident, streamed: {:.1} MiB resident + {:.1} MiB on disk \
             ({} of {} samples)",
            resident.bank.resident_bytes() as f64 / (1024.0 * 1024.0),
            streamed.bank.resident_bytes() as f64 / (1024.0 * 1024.0),
            streamed.bank.streamed_bytes() as f64 / (1024.0 * 1024.0),
            streamed.bank.streamed_samples(),
            streamed.bank.len()
        );

        // A pipe with a tail worth streaming, played the same way twice.
        let spec = *resident
            .specs
            .values()
            .find(|spec| {
                resident
                    .bank
                    .get(spec.sample)
                    .is_some_and(|sample| sample.release_start().is_some())
                    && streamed
                        .bank
                        .get(spec.sample)
                        .is_some_and(|sample| sample.stream().is_some())
            })
            .expect("a streamed pipe with a tail");
        let start = |handle: &mut aristide_engine::EngineHandle| {
            handle.send(aristide_engine::Command::StartVoice {
                handle: 1,
                sample: spec.sample,
                rate: spec.rate,
                gain: spec.gain,
                group: spec.group,
                wind_weight: 0.0,
                brightness: 0.0,
                voicing_tilt: 1.0,
                enclosures: [aristide_engine::enclosure::ENCLOSURE_NONE;
                    aristide_engine::enclosure::MAX_VOICE_ENCLOSURES],
                bus: 0,
                delay_frames: 0,
                nominal_hz: spec.nominal_hz,
            });
        };
        let blocks = 400usize;
        let block = 512usize;

        let mut expected = vec![0.0f32; blocks * block * 2];
        {
            let (mut engine, mut handle) = aristide_engine::Engine::new(
                device_rate,
                std::sync::Arc::new(resident.bank.clone()),
            );
            engine.set_release_stagger(0.0);
            start(&mut handle);
            for index in 0..blocks {
                if index == 40 {
                    handle.send(aristide_engine::Command::StopVoice { handle: 1 });
                }
                engine.process(
                    &mut expected[index * block * 2..(index + 1) * block * 2],
                    2,
                );
            }
        }

        let mut actual = vec![0.0f32; blocks * block * 2];
        {
            let bank = std::sync::Arc::new(streamed.bank);
            let (mut engine, mut handle) =
                aristide_engine::Engine::new(device_rate, std::sync::Arc::clone(&bank));
            engine.set_release_stagger(0.0);
            let (rt, mut workers) = aristide_engine::stream::attach(
                &bank,
                16,
                1,
                aristide_engine::stream::StreamCounters::default(),
            )
            .expect("the streamed bank gets a pool");
            engine.set_streams(rt);
            start(&mut handle);
            for index in 0..blocks {
                if index == 40 {
                    handle.send(aristide_engine::Command::StopVoice { handle: 1 });
                }
                for _ in 0..8 {
                    for worker in workers.iter_mut() {
                        worker.poll_once();
                    }
                }
                engine.process(&mut actual[index * block * 2..(index + 1) * block * 2], 2);
            }
        }

        let worst = expected
            .iter()
            .zip(&actual)
            .map(|(a, b)| (a - b).abs())
            .fold(0.0f32, f32::max);
        let energy: f32 = expected[60 * block * 2..].iter().map(|v| v.abs()).sum();
        assert!(energy > 1.0, "no release tail was rendered ({energy:e})");
        assert_eq!(worst, 0.0, "streamed demo render differs by {worst:e}");
    }

    /// The load cache is always split — head in `.samples`, tail in
    /// `.tails` — so one cache serves both residencies: written by a
    /// streaming load, read back by a fully-resident one and vice
    /// versa, with identical audio either way.
    #[test]
    fn the_split_cache_serves_both_residencies() {
        let Some(path) = demo_organ() else { return };
        let organ = aristide_formats::grandorgue::load(&path).expect("loads").organ;
        let dir = std::env::temp_dir().join("aristide-stream-cache-test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("cache dir");
        let cache = dir.join("demo.samples");

        // Cold, streaming: tails go to the spool, then into the cache's
        // tail file when it is written.
        let cold = build_with(&organ, 44_100.0, 16, Some(&cache), StreamingPolicy::ON)
            .expect("cold streaming build");
        assert!(cold.bank.streamed_samples() > 0);
        let listing: Vec<String> = std::fs::read_dir(&dir)
            .expect("dir")
            .filter_map(|e| e.ok().map(|e| e.file_name().to_string_lossy().into_owned()))
            .collect();
        assert!(
            cache.is_file() && dir.join("demo.tails").is_file(),
            "both files: {listing:?}"
        );

        // Warm, streaming: nothing decodes, and the tails are read
        // straight out of the cache's tail file.
        let warm = build_with(&organ, 44_100.0, 16, Some(&cache), StreamingPolicy::ON)
            .expect("warm streaming build");
        assert_eq!(
            warm.bank.streamed_samples(),
            cold.bank.streamed_samples(),
            "the warm load streams the same samples"
        );

        // Warm, fully resident: the same cache, with the tails read
        // back into RAM — byte for byte the audio a cold resident load
        // would have decoded.
        let fresh = build(&organ, 44_100.0, 16, None).expect("fresh resident build");
        let absorbed = build(&organ, 44_100.0, 16, Some(&cache)).expect("resident from cache");
        assert_eq!(absorbed.bank.streamed_samples(), 0, "tails came back to RAM");
        assert_eq!(
            absorbed.bank.resident_bytes(),
            fresh.bank.resident_bytes(),
            "the absorbed bank is the whole recording again"
        );
        assert_eq!(
            absorbed.bank.pre_fault(),
            fresh.bank.pre_fault(),
            "identical audio"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Manual measurement, not an assertion: what streaming buys on a
    /// real Hauptwerk set (AVO Solignac, gitignored — see CLAUDE.md).
    /// `resident + streamed` is what a fully-resident load would hold,
    /// so one streaming load reports both numbers without ever having
    /// the whole set in RAM.
    ///
    /// `cargo test -p aristide-server --bin aristide-server \
    ///   measure_solignac -- --ignored --nocapture`
    #[test]
    #[ignore = "loads the 2 GB Hauptwerk fixture"]
    fn measure_solignac_streaming() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../testsets/avo-solignac/OrganDefinitions/Solignac orig.Organ_Hauptwerk_xml");
        if !path.is_file() {
            eprintln!("skipping: Solignac fixture not present");
            return;
        }
        let organ = aristide_formats::hauptwerk::load(&path).expect("loads").organ;
        let started = Instant::now();
        let loaded =
            build_with(&organ, 48_000.0, 16, None, StreamingPolicy::ON).expect("builds");
        let mib = |bytes: usize| bytes as f64 / (1024.0 * 1024.0);
        println!(
            "Solignac: {} samples in {:.1?}; resident {:.0} MiB + streamed {:.0} MiB \
             = {:.0} MiB fully resident ({} of {} samples stream)",
            loaded.bank.len(),
            started.elapsed(),
            mib(loaded.bank.resident_bytes()),
            mib(loaded.bank.streamed_bytes()),
            mib(loaded.bank.resident_bytes() + loaded.bank.streamed_bytes()),
            loaded.bank.streamed_samples(),
            loaded.bank.len()
        );
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
        let loaded = build(&organ, device_rate, 16, None).expect("bank builds");
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
                                handle.send(st.command());
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
        let loaded = build(&organ, device_rate, 16, None).expect("bank builds");
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
                                handle.send(st.command());
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
        let loaded = build(&organ, 48000.0, 16, None).expect("bank builds");
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
                    handle.send(start.command());
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
        let loaded = build(&organ, 48000.0, 16, None).expect("bank builds");
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
                        handle.send(start.command());
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
                send_chord(&mut console, &mut handle, second.is_multiple_of(2));
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
        let loaded = build(&organ, 48000.0, 16, None).expect("bank builds");
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
                    handle.send(start.command());
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
        let loaded = build(&organ, 48000.0, 16, None).expect("bank builds");
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
                handle.send(start.command());
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
        let loaded = build(&organ, 48000.0, 16, None).expect("bank builds");
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
            assert!(handle.send(start.command()));
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
            if let Some(m) = mag(hz)
                && m > fine.1
            {
                fine = (hz, m);
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
        let loaded = build(&organ, 48_000.0, 16, None).expect("bank builds");
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
                    assert!(handle.send(start.command()));
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
        let loaded = build(&organ, device_rate, 16, None).expect("bank builds");
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
                        assert!(handle.send(start.command()));
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
        let loaded = build(&organ, device_rate, 16, None).expect("bank builds");
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
                        assert!(handle.send(start.command()));
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
        let loaded = build(&organ, device_rate, 16, None).expect("bank builds");
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
                handle.send(st.command());
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
        let loaded = build(&organ, device_rate, 16, None).expect("bank builds");
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
                            handle.send(st.command());
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
        let loaded = build(&organ, device_rate, 16, None).expect("bank builds");
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
                            handle.send(st.command());
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
        let loaded = build(&organ, device_rate, 16, None).expect("bank builds");
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
        let loaded = build(&organ, device_rate, 16, None).expect("bank builds");
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
                            handle.send(st.command());
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
        let loaded = build(&organ, device_rate, 16, None).expect("bank builds");
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
                        handle.send(st.command());
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
        let loaded = build(&organ, device_rate, 16, None).expect("bank builds");
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
                        handle.send(st.command());
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
                    handle.send(st.command());
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
