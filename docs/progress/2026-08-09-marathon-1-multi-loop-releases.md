# 2026-08-09 — marathon 1/N: multi-loop + separate multi-releases

(User away ~10 h with a standing "build everything" mandate; one tested
feature per cycle.)

- **Multi-loop playback**: samples keep all their validated loops
  (primary = longest, alternates via `add_loop`); each pass the voice
  draws the next loop at random (GO's PickEndSegment idea) —
  decorrelates loop repetition and unison pipes. Demo set's multi-loop
  files (e.g. Bourdon 8') now rotate automatically.
- **Separate release samples**: `Sample::attach_release(target, id,
  max_hold_ms)` — one-shot bank entries with hold-time bounds (GO
  MaxKeyPressTime semantics, sorted bounded-asc/unbounded-last),
  cross-file phase maps (release-head correlation against the source
  loop template using the measured period), and head levels for the
  existing level matcher. Engine StopVoice now computes the hold time
  from voice age and selects; the crossfade reads its tail from the
  other sample and the voice migrates there at fade completion.
  Loader wires `ReleaseSample` paths (deduplicated) automatically.
- Tests: loop-rotation statistics; staccato (100 ms hold → 0.15 s
  release) vs tenuto (500 ms → 1.5 s release) selection end-to-end.
  71 tests green.
