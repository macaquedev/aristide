# Aristide — shared project instructions

These instructions apply to all agents working in this repository, including
Codex and Claude. Read this file in full at the start of every task.

Also read [`docs/design/ui-flow-spec.md`](docs/design/ui-flow-spec.md) **in full
at the start of every task/session**, including after a context reset. It is Alex's
authoritative UI, flow and target instrument-model specification, supplied on
2026-09-21. A summary, remembered conventions or previous screenshots are not a
substitute. Re-read both files if they change during a task.

Read `DESIGN.md` before implementation work for engine architecture, existing
contracts and implementation history. The new spec supersedes earlier product,
interaction, visual and target-model decisions in that document, the old design
rules and historical progress notes. Preserve audio safety and file compatibility
when adapting the backend to the new requirements; document needed migrations.

## New UI: start from the new spec

The old UI was deliberately removed on 2026-09-21. The user has now requested a
new UI, built slowly and incrementally from `docs/design/ui-flow-spec.md`.
**Do not restore, reskin or use the deleted UI as the design template.** Its
screens, navigation, styling and workflows are not a starting point. The old
`docs/design/design-rules.md` is only a redirect to the new specification.

The new Tauri shell and initial Play surface live in `desktop/`; the headless
server remains available. Reuse the audio engine, model,
sample loaders, MIDI/device I/O and JSON sound-control API where they support the
new requirements. Do not assume that the backend already implements every feature
in the spec, or change the spec to fit an old endpoint.
`crates/aristide-server/src/console.rs` is musical control logic (stops, couplers,
combinations and voices), not a graphical interface; it remains essential.
Existing organ-file metadata must stay compatible and edits must preserve unrelated
settings. Imported organs and samples remain untouched; customisation lives in layers.

## UI implementation workflow

- Use **Tauri** for the native desktop shell and **Bun** for frontend dependency
  installation, scripts and the lockfile (Alex, 2026-09-21). Do not use npm.
- Work in small, coherent, reviewable increments. State the scope and the relevant
  spec sections before implementing; avoid building every panel at once.
- The seven Control flow rules outrank individual screen designs. Play is home;
  the top-bar padlock separates perform and edit; Build opens through a stop in
  edit mode; sheets and edits must leave audio running. Follow the spec's autosave,
  global undo, snapshots and shared number behaviour.
- Use the new five-panel shell: Play, Build, Route, Tuning, Library, with Setup
  separate. Do not bring back the old Voice workspace or its settings hierarchy.
- Follow the new Visual rules without exceptions: flat and abstract, dark by
  default, three colour roles, one typeface/two weights/three sizes, about 60 px
  Play targets, meaningful marks only. Use a stock component library's defaults
  for everything outside the console. Do not inherit the old palette or tokens.
- Play, Route, Library and Setup each get one implementation. For Build and
  Tuning, first produce four structurally different clickable mocks as specified;
  let Alex pick by feel, then make four mutations of the chosen direction.
  Do not choose a winner on Alex's behalf or implement the old editor by default.
- Design for one medium touchscreen first, with touch and mouse working throughout;
  account for the spec's small-screen sheets and multiple-screen arrangements.
- Show through layout, controls and visible state; do not fill screens with
  explanatory prose. Use short labels, group related controls, and keep help
  contextual. Apply this to design studies as well as connected screens
  (Alex, 2026-09-22).
- Keep prototype behaviour distinct from connected functionality. Record backend
  gaps, incomplete requirements and validation in a progress note. Never present
  dummy controls, fake progress or an unimplemented feature as working.
- Screenshot the running UI and review it against **Control flow rules** and
  **Visual rules** before calling a UI increment complete. Exercise touch and
  mouse interactions and verify that navigation and editing preserve audio.
- For the first-run review, use a fresh session to role-play a first-time organist
  and record every hesitation as a bug, as the spec requests.

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
