# Aristide — shared project instructions

These instructions apply to all agents working in this repository, including
Codex and Claude. Read this file in full at the start of every task.

Read `DESIGN.md` first before implementation work: it holds the architecture, all locked decisions, and the
milestone plan (M0–M7). Do not re-litigate locked decisions without the user.

## Ground rules

- **RT invariants** (aristide-engine audio path): never allocate, lock, or do I/O on
  the audio thread. Control→RT via lock-free queues only. Uphold these in every change.
- **No 12-EDO assumptions** in the model or engine: pitch travels as Hz/cents;
  MIDI-note→frequency conversion happens control-side in one replaceable place.
- **Legal boundary**: encrypted Hauptwerk sample sets are permanently out of scope.
  No decryption, ever. Only the open GrandOrgue format and unencrypted HW packages.
- Sample-set formats are read as-is; Aristide-specific data (voicing, tuning,
  routing, effects) goes in TOML sidecar files, never into the loaded set.
- `docs/go-odf-notes.md` is the authority for the GrandOrgue format — it was
  compiled from GO's loader source — and `docs/hw-odf-notes.md` for the Hauptwerk
  format. Extend them (with citations) rather than guessing.

## Practical notes

- Test fixture: `testsets/grandorgue-demo/` (gitignored; 21 MB). If missing, unzip
  `packages/*.orgue` (a plain zip) from a shallow clone of GrandOrgue/grandorgue.
  Its samples are WavPack with `.wav` extensions — that's normal; `wav::read` sniffs.
- Hauptwerk fixture: `testsets/avo-solignac/` (gitignored; 2 GB). The free AVO
  Solignac package (hauptwerk-augustine.info, `AVO_Solignac.Organ.CompPkg.Hauptwerk.rar`,
  a plain RAR v4) unpacked so that `OrganDefinitions/` and `OrganInstallationPackages/`
  sit directly inside it. `docs/hw-odf-notes.md` is the authority for the Hauptwerk
  format, same rule as the GO notes.
- Requires system `libwavpack` (+ `libasound2-dev`/`alsa-lib` to build cpal).
- This machine may be a headless dev server: `cargo test` and `cargo clippy` provide automated validation
  here; anything audible is verified by the user pulling to their desktop and running
  `cargo run --release -p aristide-server`. Never assume an audio device exists.
- Commit style: conventional commits, atomic, imperative subject ≤50 chars.
  Follow `AGENTS.md` for the commit-and-push workflow: after appropriate checks
  pass, commit directly on `main` and immediately push each atomic commit to
  `origin/main`, without asking for additional confirmation.
