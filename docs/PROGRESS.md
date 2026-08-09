# Progress log

Newest first. One entry per work session; keep entries factual and short.
Milestones refer to DESIGN.md.

## 2026-08-09 — research sweep + wind v3 recalibration

Three research reports gathered and distilled into `docs/research/`
(hauptwerk-wind-model.md, organ-wind-acoustics.md,
vpo-rendering-techniques.md): Hauptwerk's documented wind architecture
(lumped compartments/linkages/bellows; per-pipe flow→pitch/amp/brightness
curves; the designer's statement that the audible effect is **transient
wobble at note on/off, not static sag** — including a Dev-confirmed
pitch-polarity bug shipping since v5), Fraunhofer's measured wind
transients (3–10 Hz bellows modes, 10–20 Hz trunk modes, onset dips 2–4×
sustained sag), Pykett's pitch sensitivity (≈0.65 cents/% pressure,
matching HW's own 3.3-cents-at-6.3 % calibration), tremulant numbers
(~6 Hz, ±15 cents typical / ±24 ceiling, per-harmonic AM with independent
phases), Appleton's release-alignment and loop-crossfade analyses, and
HW's polyphony economics.

Wind v3 applied from the data: realistic pressure drops (6 % chest sag at
reference demand) × physical pitch sensitivity (P^0.032 ≈ 0.55 cents/%),
gain P^0.75 (Fletcher's 15 dB/decade), wind draw ∝ 1/f (Walker patent's
per-octave halving), pallet gulp 2× over 50 ms (onset dips 2–4× steady,
per ISMA 2007). Same regulator topology (validated by patent + field
data). Steady full-chorus ≈ −3.4 cents; transients dominate, as they
should. 60 tests green.

## 2026-08-09 — wind model v2 after user feedback ("super slow and horrible")

v1's first-order reservoir glided pitch over ~120 ms — chorus-wide
portamento, not wind. Rebuilt as a **damped second-order regulator**
(ω = 2π·3.5 Hz, ζ = 0.5, semi-implicit Euler substepped for stability at
any block size): fast dip reached in ~70 ms, one springy bounce, settled
in ~250 ms, slight overshoot on release. Per-voice **pallet-opening
boost** (+0.8× weight decaying over 70 ms) makes single notes dip too.
Depth cut to a quarter: defaults now ≈ −3 cents steady full-chorus
(sidecar `[wind] sag_cents`, plus `bounce_hz` / `damping`). Tests assert
the dynamics: dip ≤ 120 ms, ~16 % undershoot (matches ζ=0.5 theory),
release overshoot, stability at 93 ms blocks. 60 tests green.

## 2026-08-09 — M4 part 2: wind supply model

The organ breathes. Per-windchest reservoir model in the RT engine
(`engine/wind.rs`): `dP/dt = (1−P)/τ − s·D·P` where D sums the wind
weight of sounding voices on the chest (weight ≈ √(150 Hz/f), capped —
big pipes drink more; noises draw nothing). Pressure maps to per-voice
playback rate (P^0.4) and gain (P^0.8): chords sag flat and soften
slightly, then the reservoir recovers (default 2 % sag at a
full-chorus demand of 30, τ = 120 ms; sidecar `[wind] sag_percent /
recovery_ms`, 0 disables). Attack dips fall out of the dynamics.

This is architecturally impossible in GrandOrgue (rate frozen at note
start — critique §2); it's the first feature where Aristide does
something GO *can't*.

- Voices carry (wind group ← ODF windchest, wind weight); model `Rank`
  now records its windchest.
- Proof test: 30 silent heavy voices + 1 measured sine on one chest —
  measured zero-crossing period sags 480 → ~484 frames (≈ −14 cents)
  at the calibrated steady state, and pressure settles at 0.980 ± 0.004.
  Cost: one integration step + two powf per chest per block. 58 tests.

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
