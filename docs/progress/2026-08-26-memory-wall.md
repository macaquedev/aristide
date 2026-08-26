# 2026-08-26 — the memory wall comes down (gap §3)

Samples were f32-resident, decoded single-threaded, re-analyzed every
launch: an 8 GB HW-style set cost 30+ GB here. Three landings, user
decision: 16-bit default with f32 as the A/B opt-out.

## 16-bit residency (default)

- `SampleData::{F32(Vec<f32>), I16(Vec<i16>)}` behind the same
  `Sample` API; sidecar `[samples] bits = 16|32`. Every analysis pass
  (period refinement, quadrature phase maps, tail measurement) runs at
  full decode precision *before* quantization; playback dequantizes.
- The sinc fast path gets dedicated i16 kernels (SSE2 mono/stereo,
  AVX2 mono): sign-extend in-register (self-unpack + arithmetic shift
  / `vpmovsxwd`), dequant scale folded into the final horizontal sum —
  no scalar conversion pre-pass. Measured ≈ +8% read cost (81 vs 75 ms
  per 3M mono reads) for −50% RAM and halved memory traffic. The −96 dB
  quantization floor sits below organ recordings' own room noise —
  effectively what GO and HW play from by default. An equivalence test
  pins i16 reads within quantization noise of f32 across fast path,
  loop-seam slow path and edges.

## Parallel decode

Every unique file (attack and release alike) decodes and analyzes on
an `available_parallelism` worker pool over a shared job queue;
assembly (release attaching, spec building, the rank-wide pitch
decisions) stays sequential and cheap. WavPack decode + long-lag
autocorrelation was the single-threaded load-time hog.

## Load cache (GO's `GOCache` trick)

- `server/cache.rs`: decoded samples + all analysis persist under
  `~/.config/aristide/cache/<hash>.samples`, one file per (source
  paths, residency). `[samples] cache = false` opts out.
- Validity is **per entry**: source mtime+size plus a hash of the
  exact decode inputs (the serialized ODF attack/release record, the
  aligning pipe's pitch, residency). Editing an ODF invalidates only
  the entries it touched; the rest stay hot. Version-tagged, atomic
  temp+rename writes, any structural surprise = miss.
- Samples are never cloned to be cached: the writer borrows them where
  they live, so peak RAM stays one bank.
- Demo set, release build: 440 ms cold → ~30–60 ms warm. Big sets gain
  far more (WavPack + analysis dominate their loads).
- Engine `Sample::write_cache`/`read_cache` own the field layout
  (little-endian, POD byte views); cached entries are pre-attach, so
  release options (bank indices — an assembly fact) rebuild each load.

Tests: cache hit proven by corrupting the source file under a
preserved stamp (only the cache can serve), invalidation by bumping
the stamp; 16 vs 32 residency exactly halves `resident_bytes` with
identical specs. Manual benches: `bench_read_f32_vs_i16` (engine),
`bench_demo_cache` (server), both `--ignored`.

Remaining §3 residue, named: **streaming** (sets beyond RAM even at
16-bit — the M4-era design sketch stands), per-rank load options
(mono downmix, first-loop/first-release), and GO-style lossless delta
compression only if a target set still doesn't fit.
