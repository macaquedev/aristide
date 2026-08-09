# Progress log

Newest first. One entry per work session; keep entries factual and short.
Milestones refer to DESIGN.md.

## 2026-08-09 — sidecar v0 + GrandOrgue critique

- First real sidecar: `<set>.aristide.toml` with `[registration] default`
  and `[midi] channels`. Pattern matching exact-first-then-shortest so
  "plein jeu" can't draw its drawstop noise. New generic channel default:
  keyboards first, pedal last (channel 0 = the Great). Demo sidecar sets
  a plein jeu (Bourdon 16', Montre 8', Prestant 4', Plein jeu III).
  53 tests green incl. a sidecar-driven end-to-end.
- `docs/go-critique.md`: cited critique of GO's renderer from a source
  read (`reference/grandorgue/`, gitignored). Key findings: condvar
  waits + mutexes inside the audio callback; 8-tap Lanczos resampler
  with rate frozen at voice start; 2-sample amplitude/slope release
  alignment; no wind model; 16-bit amplitude-only synth trem. Plus a
  what-they-get-right list (load cache, ODF leniency) to steal from.

## 2026-08-08 — M4 part 1: sinc resampling + phase-aligned releases

The two highest-impact quality items from DESIGN.md, both with
quantitative headless proof:

- **Windowed-sinc resampling** (`engine/resample.rs`): 16-tap Kaiser
  (β=9, cutoff 0.9·Nyquist) polyphase table, 512 phases + inter-row
  interpolation, per-row DC normalization. Fast contiguous dot-product
  path; slow path wraps kernel taps across the sustain-loop seam so loop
  passes never click. Measured: **90.6 dB SNR vs 17.1 dB for the old
  linear interp** at 40 % Nyquist, 44.1→48 kHz.
- **Phase-aligned release splicing** (`bank::ReleaseAlignment`): at bank
  build, one normalized cross-correlation search per sample locates the
  release-tail frame matching the loop start's phase; a 64-bucket table
  extrapolates the rest arithmetically. On note-off the RT side indexes
  the table (O(1)) to splice at matching phase. Measured at the
  adversarial anti-phase stop moment: **aligned splice holds 0.89 of the
  held level through the crossfade; the naive fixed splice dips to 0.17**
  (audible thump). Falls back to the fixed splice when analysis is
  impossible (no tail, tail shorter than a period, unpitched).
- Full demo set (853 files decoded + analyzed) loads in ~1 s in release.
  48 tests green.

Next in M4: wind supply model, synthesized tremulants, separate release
files + multi-attack selection, per-pipe voicing sidecars, disk streaming.

## 2026-08-08 — M3 code-complete: sampled voices end to end

First real organ sound (code-side; audible check pending on the user's
desktop — this box is headless).

- `aristide-engine` gains `bank`: immutable `SampleBank`/`Sample` (decoded
  interleaved f32, validated loop/release markers), shared with the RT
  thread via `Arc` at construction. RAM-resident for now; the API is shaped
  so a disk streamer later replaces the storage, not the interface.
- Engine voices are now `Tone` (M1 test tone, kept for no-set mode) or
  `Sampled`: attack → inclusive sustain loop → release splice (30 ms
  crossfade onto the embedded tail at the cue marker / post-loop position,
  GO's fallback order), emergency 15 ms kill fade, percussive (loop-less)
  samples play out and ignore stop. Block-based rendering, 2048-voice pool,
  voice stealing from dying voices. New commands: StartVoice / StopVoice /
  SetMasterGain — the engine still knows nothing about organs or keys.
- `aristide-server` gains `bank::build` (decode + dedup by path, per-pipe
  VoiceSpec with rate = file_rate/device_rate × cents, gain dB→linear;
  borrowed pipes resolve to their target's spec) and `console::Console`
  (drawn stops, MIDI channel → manual in model order, key → RankRange →
  pipe → StartVoice; retrigger accumulation; CC120–123 panic).
- CLI: `aristide-server set.organ [--stops name,name] [--list-stops]
  [--gain 0.35]`. Default registration: each manual's first stop.
- Tests 42 green, including a headless end-to-end: demo.organ → model →
  bank (1350/1350 pipes get specs, 0 skipped) → console note-on → engine
  render (nonzero energy) → note-off → silence after tails.

Deferred within M3 scope, tracked for M4: separate release-sample files
(demo set has none), multi-attack selection, ODF ReleaseEnd/crossfade
lengths, disk streaming for big sets, real channel routing.

## 2026-08-08 — borrowed pipes; demo set loads clean (M2 nearly done)

- Model: `Pipe` now carries an explicit `PipeSource` — `Sampled`,
  `Borrowed(PipeRef)`, or `Silent` — instead of bare attack/release vectors,
  so unit-organ borrowing is a first-class concept. `Organ::sounding_pipe`
  follows borrow chains (hop-capped against cycles).
- GO loader: `REF:<manual>:<stop>:<pipe>` resolved in a deferred pass after
  all stops load (forward references are legal); unresolvable/malformed refs
  and borrow cycles degrade to silent pipes with warnings (GO would abort;
  we stay lenient per the parser's charter).
- `inspect` example reports sampled/borrowed/dead-borrow/silent counts.
- Friesach demo set end-to-end: 3 manuals, 47 stops, 51 ranks, 5 couplers,
  853 sampled + 497 borrowed pipes (all chains terminate on samples),
  0 missing files, **0 warnings** (was 497). 33 tests green.
- Note: `releases: 0` is correct for this set — its release tails live
  after the loop inside each attack WAV, not in separate release files.

Remaining for M2: nothing blocking; wire loader → server at M3 start.

## 2026-08-08 — state audit after repo move (M1 done, M2 ~70%)

Repo moved from `~/github/aristide` to `/home/macaque/aristide`; full rebuild and
`cargo test --workspace` green here (30 passed, 0 failed).

What exists, by commit history and code review:

- **M0 complete** — workspace scaffold (5 crates), DESIGN.md, GPLv3, CLAUDE.md.
- **M1 complete (code-side)** — `aristide-engine`: fixed 256-voice pool, additive
  principal-chorus test tone, attack/sustain/release envelope, lock-free `rtrb`
  command queue, no alloc/lock/IO on the audio thread. `aristide-server`: cpal
  f32 output, connects every midir MIDI input, note-on/off + CC120–123.
  12-EDO→Hz lives control-side in one function; the engine only sees Hz.
  Audible verification happens on the user's desktop (this box is headless).
- **M2 in progress** — the loader stack is ahead of schedule:
  - `aristide-model`: format-neutral organ model — manuals, stops, ranks,
    pipes with multi-attack (loops, cents offset) and duration-selected
    releases, couplers as key deltas. No 12-EDO in the model.
  - `aristide-formats/wav`: hand-rolled RIFF reader (8/16/24/32-bit int +
    f32, extensible wrapper), `smpl`/`cue` loop metadata, header-only
    `read_info` for future disk streaming. 18 tests.
  - `aristide-formats/wavpack`: minimal libwavpack FFI (no bindgen);
    `wav::read` sniffs `wvpk` magic and delegates. 4 tests.
  - `aristide-formats/grandorgue`: lenient `.organ` ODF parser → model;
    warnings, not errors, for real-world oddities. 8 tests.
    `examples/inspect.rs` prints a set summary.
  - `docs/go-odf-notes.md` (633 lines) — GO format spec notes compiled from
    GrandOrgue's loader source; the authority for parser work.
- **Test fixture**: `testsets/grandorgue-demo/` (gitignored, 21 MB).

Remaining for M2: load the demo set end-to-end through `inspect`, validate
counts/pitches against GrandOrgue's own reading, wire loader warnings into
server startup. Then M3: attack-cache + streaming sampled voices.
