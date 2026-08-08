# Aristide — project context for Claude sessions

Read `DESIGN.md` first: it holds the architecture, all locked decisions, and the
milestone plan (M0–M7). Do not re-litigate locked decisions without the user.

## Ground rules

- **RT invariants** (aristide-engine audio path): never allocate, lock, or do I/O on
  the audio thread. Control→RT via lock-free queues only. Uphold these in every PR.
- **No 12-EDO assumptions** in the model or engine: pitch travels as Hz/cents;
  MIDI-note→frequency conversion happens control-side in one replaceable place.
- **Legal boundary**: encrypted Hauptwerk sample sets are permanently out of scope.
  No decryption, ever. Only the open GrandOrgue format and unencrypted HW packages.
- Sample-set formats are read as-is; Aristide-specific data (voicing, tuning,
  routing, effects) goes in TOML sidecar files, never into the loaded set.
- `docs/go-odf-notes.md` is the authority for the GrandOrgue format — it was
  compiled from GO's loader source. Extend it (with citations) rather than guessing.

## Practical notes

- Test fixture: `testsets/grandorgue-demo/` (gitignored; 21 MB). If missing, unzip
  `packages/*.orgue` (a plain zip) from a shallow clone of GrandOrgue/grandorgue.
  Its samples are WavPack with `.wav` extensions — that's normal; `wav::read` sniffs.
- Requires system `libwavpack` (+ `libasound2-dev`/`alsa-lib` to build cpal).
- This machine may be a headless dev server: `cargo test`/`clippy` prove correctness
  here; anything audible is verified by the user pulling to their desktop and running
  `cargo run --release -p aristide-server`. Never assume an audio device exists.
- Commit style: conventional commits, atomic, imperative subject ≤50 chars.
  Push to `main` on green tests unless mid-refactor.
