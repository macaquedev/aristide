# 2026-09-02 — release tails play off the disk (M4, gap §3e)

Everything was RAM-resident. 16-bit residency and the load cache
(2026-08-26) bought a factor of two and a fast reload, but a set that
does not fit still does not fit. This is the streamer that closes §3.

## What may stream, and what may never

Derived, not chosen. A held note must never depend on a disk: if the
attack or the sustain loop could stall, every note in the instrument
is a gamble. So everything from the first frame through the end of the
**last** sustain loop stays resident — bit-for-bit what it was.

What is left is the release, and that is where a set's bytes actually
are: tails run 5–15 s against 1–3 s of attack-plus-loop, and each is
read exactly once, forward, by one voice. Textbook streaming material.

The head of every tail stays resident too. The splice at note-off is
the most delicate moment in the engine — phase-aligned target, level
match, a crossfade of up to 184 ms — and it must start the instant the
key comes up. `HEAD_SECONDS = 0.35` comes from the worst case a
streamer can be late by: a worker mid-sweep over its half of the pool
(80 slots × ~0.2 ms per 64 KiB read on a SATA SSD ≈ 16 ms), plus its
1 ms poll and the read itself — ~20 ms. 350 ms of source is >10× that
at unity rate and still ~4× for a pipe repitched two octaves up (which
eats source frames four times as fast). It also exceeds the longest
crossfade, so the splice itself is always RAM.

Loop-less samples (separate releases, percussives, action noises) keep
the same head from frame 0 and stream the rest — but only if what is
left is worth a slot: below `MIN_TAIL_FRAMES` (4096, ~85 ms) the sample
stays whole, which is every short action noise. A sample with a loop
but nothing recorded past it never splits: there is nothing behind the
loop to stream.

## The store is the load cache

Streaming must not decode WavPack on a streamer thread, and it must
not hold the set in RAM even for the moment between decoding it and
building the bank — that spike is the wall itself. So:

- **Fresh decodes** hand their tail to a *spool* the instant analysis
  is done (`server/spool.rs`): one file, appended concurrently by the
  decode workers with positional writes, unlinked as soon as it is
  opened so the kernel reclaims it even after a crash.
- **The load cache is the streaming store.** Cache entries are now
  always *split*: the head lives in `<hash>.samples`, the tail in a
  companion `<hash>.tails`. A streaming load reads only the heads and
  points each sample at its tail's offset — a warm streaming load
  copies no audio at all. A fully-resident load reads the tails back
  and makes the samples whole again, so **one cache serves both
  residencies** and toggling `streaming` never forces a re-decode. The
  two files carry a shared generation stamp, so a crash between their
  renames reads as a miss rather than as garbage at stale offsets.

Every load-time analysis pass — period refinement, quadrature phase
maps, tail decay, EOF level, tail reference level — still runs on the
whole recording, before a byte leaves RAM. Nothing audible changes.

## The RT path

A fixed pool, allocated at engine construction and only when the bank
actually streams: 160 slots (the engine already caps concurrent tails
at `TAIL_VOICE_BUDGET` = 128; the rest is headroom for shedding lag
and long one-shots) × a 128 KiB SPSC byte ring + an 8 KiB linear
window ≈ 21 MiB. That is the whole fixed cost, and it is small against
the gigabytes it exists to not hold.

- Streamer threads (2) poll their rings' free space — an atomic load —
  and fill them with positional reads. The audio thread never signals
  them: no futex wake, no syscall, in the callback.
- The sinc reader needs `taps` *contiguous* frames, so each slot keeps
  a linear window it slides forward: a memmove of what is still needed
  plus one or two memcpys out of the ring. No allocation, no lock, no
  I/O.
- The stored region begins `OVERLAP_FRAMES` (64) *before* the split, so
  the window can serve a kernel that straddles the crossing. The
  crossover sits at `resident_end − 33`: the widest kernel reads 31
  frames back and 33 forward, so that is the one point where both
  sides can serve a whole kernel. Below it every tap is in RAM, at or
  above it every tap is in the window — a kernel never straddles the
  two.
- Resident reads are untouched: `SincTables::read` is the same
  function, reached through one `position < limit` compare against
  `f64::INFINITY` for anything fully resident.

Because the window holds the sample's own resident format (i16 or f32)
and goes through the same kernels — the i16 SIMD dispatch was factored
out and is now shared — a streamed tail is **bit-identical** to the
resident one, not merely close.

## Failure is a fade, never a click

- **Underrun** (the ring is dry with region left): the read clamps on
  the last frame it has, so the waveform freezes rather than jumping,
  and the voice immediately takes the 15 ms kill ramp. A counter goes
  up; the server logs it in the same watchdog that reports late audio
  callbacks.
- **Pool exhaustion**: a release that finds no free slot is *denied
  once* (never retried — a slot acquired mid-tail would start
  delivering seconds behind the cursor). The voice's readable end
  becomes its resident head, which means the existing EOF guard fades
  its last 46 ms. A shortened tail, not a click, and counted.
- **A vanished or truncated store**: the worker stops feeding that
  slot, which is the underrun path.
- **Organ swap**: the stream is dropped first (taking the engine and
  its slots), then the worker threads are stopped and joined —
  `AudioOutput::start` does both, in that order.

## Policy

Sidecar `[samples] streaming = "auto" | "on" | "off"` (default auto)
and `[samples] ram_budget_mb`. `auto` must decide *before* the first
file is decoded — by then the RAM is spent — so the only measure
available is what the source files weigh on disk: estimate = 1.5×
(16-bit residency) against half of physical RAM. The factor is a
deliberate compromise: WavPack holds organ samples at ~55 % of 16-bit
PCM (≈1.8× on decode), 24-bit PCM shrinks to 0.67×. Being wrong upward
costs a stream pool; being wrong downward costs the process. Physical
RAM is read from `/proc/meminfo`; where that is absent `auto` declines
to guess and stays off. Loads log resident vs streamed MiB and the
pool size.

## Verification

- **Bit-identical**: the demo set (44.1 kHz, 16-bit residency) rendered
  through a held note, its release and the whole tail — streamed
  against resident — max sample difference exactly `0.0`. Same in the
  engine's synthetic tests for f32 and i16 residency, for a single
  note and for eight voices released together, over tails long enough
  that each slot's ring wraps several times.
- **Demo set RAM**: 85.8 MiB resident → 55.1 MiB resident + 30.7 MiB
  streamed (91 of 91 samples), i.e. −36 % with no audible change. (Its
  tails are short; real sets are far more lopsided — see below.)
- **AVO Solignac** (Hauptwerk, 2 GB of source wavs, 1596 samples,
  16-bit residency): **446 MiB resident + 839 MiB streamed** — the same
  set fully resident is 1285 MiB, so streaming holds **35 %** of it in
  RAM. 1575 of 1596 samples stream (the 21 that do not are short action
  noises). `resident + streamed` is what a resident load would have
  held, so one streaming load measures both without ever holding the
  set.
- **RT cost** (`bench_streamed_voice_cost`, release, 64 tail voices
  rendered for 400 blocks of 512 frames): resident 40.7 ns per frame
  per voice, streamed 47.6 ns — **+17 %** for the window bookkeeping
  and the copies out of the ring, on the tail phase only (attacks and
  loops are unchanged, and tails are shed above 128 anyway). The
  streamer's own reads are on another thread.
- **Underrun**: a streamer that never delivers — worst neighbouring-
  sample jump in the whole render stays under the waveform's own
  slope, and the voice reaches silence (the discontinuity-scan idiom
  from the release tests).
- **Exhaustion**: one slot, four simultaneous releases — no
  discontinuity, every voice ends, the slot comes back.
- **Slot lifecycle**: after a mass release rings out, `active_slots()`
  is 0 — the worker acked every stop and the engine reclaimed it.
- **Cache**: a cold streaming load writes both files; a warm streaming
  load streams the same samples with no decode; the same cache read by
  a fully-resident load reproduces the whole recording byte for byte
  (`pre_fault()` checksums match a fresh decode).

## Deferred, named

- A cold streaming load writes its tails twice (spool, then the
  cache's tail file). A warm load copies nothing. Repointing the
  samples at the freshly written cache and dropping the spool would
  remove the duplicate disk use; it is offsets-into-a-file work with a
  garbage-audio failure mode, so it waits for a reason to exist.
- The head is one constant for every rank. A per-rank head (short for
  treble ranks, long for 32′ bass) would shave more RAM.
- `auto`'s estimate is a factor on source bytes, not a measurement.
  Reading each file's header at plan time would make it exact.
- Streaming attacks (Hauptwerk does not either) and GO's lossless
  delta compression remain out of scope.
