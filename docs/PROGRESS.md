# Progress log

Newest first. One entry per work session; keep entries factual and short.
Milestones refer to DESIGN.md.

## 2026-08-09 — marathon 1/N: multi-loop + separate multi-releases

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

## 2026-08-09 — release alignment: measure the true period (bell fix, part 2)

User: still slightly bell-like, "doesn't sound sampled". Investigated
the actual demo files (`tailinfo` example): the recorded releases are
real and long (cue sits 1–4 s after the loop end at full sustain level;
the decay after the cue IS the sampled release — same data GO plays).
The defect was ours: the alignment table tracked phase as
(position − loop_start)/period with **period derived from nominal
12-EDO pitch**. Real pipes sit cents off nominal; across the hundreds
of periods between anchor and cursor that error wraps the phase
multiple times — the splice landed at effectively random phase, and a
random-phase splice through a crossfade is exactly the hollow "bell"
he heard. Synthetic tests had exact periods, so they passed.

Fix: `Sample::refine_period` — long-lag normalized autocorrelation over
the sustain loop (up to 24 nominal periods of lag, parabolic peak
refinement → relative error ≪ 1e-5), used by align_release; bails to
no-alignment (fixed splice) when the material doesn't self-correlate.
Regression test: 13-cents-mistuned non-integer period, 200-period
anchor distance — phase still lands within a bucket. Demo set load
stays ~1.2 s. 69 tests green.

## 2026-08-09 — couplers (console routing + web UI)

Couplers were parsed since M2 but never routed. Console now expands each
played key through engaged couplers (single-level: coupled notes don't
re-couple — the default organ behaviour; GO's opt-in propagation flags
can come later). Handles unison and octave shifts, self-couplers (16'),
out-of-compass drop, cycle pairs. Web console gets a Couplers section
(`/api/coupler?idx=N&on=0|1`). Sounding notes keep their coupling;
new presses use the new state. 68 tests green.

## 2026-08-09 — release level matching (the "bell strike" fix)

User: releases sounded "like you hit a bell". Diagnosis: the splice was
phase-aligned but never **level**-matched — the tail always entered at
the recording's full loudness, so any voice quieter than that (early
releases during the attack; loop/tail level differences in the
recordings) got a step up followed by a decay: a bell strike. HW
explicitly matches release level at key-off (features datasheet);
now we do too:

- Each voice runs a ~10 ms envelope follower on its own pre-gain
  output; each sample stores the measured mean level of its tail's
  first stretch. At note-off the tail is scaled by their ratio
  (clamped ×0.05–1.3), folded into voice gain at fade completion.
- Crossfade curve linear → smoothstep (≈ raised cosine): linear fades
  dip on the uncorrelated noise floor (Appleton 2019).
- Regression test: releasing 1.5 periods into a ramping attack now
  peaks < 0.55× (was ~1.0×) with the tail leg itself at the voice's
  own ~0.37 level. 66 tests green.

## 2026-08-09 — brightness modulation + per-pipe flow noise

The third leg of the physical triple: **pressure now breathes timbre**,
not just pitch and volume. Each sampled voice carries a one-pole tilt
filter hinged at ~2× its fundamental (floor 150 Hz for deep bass — HW
had distortion trouble there); the chest's `P^3` brightness factor sets
the upper-band gain, so the tremulant's ±22 % pressure swings timbre
±5 dB and wind sag darkens the tutti slightly. Bypassed (bit-identical)
at neutral pressure; cost ≈ 4 ops/frame only while pressure is off
nominal. Plus **per-pipe flow noise**: every voice's wind draw wanders
independently (slow damped random walk, ±2 % default, sidecar
`flow_noise_percent`), replacing nothing — GO fakes this with a single
random detune at note-on, HW models it continuously. Factors are
linearized per voice around the chest state (no per-voice powf).
Quantitative tests for both (tilt ratio ≈ gain×P³; pitch drift appears
with noise and not without). 65 tests green.

## 2026-08-09 — tremulant + web console

- **Tremulant**, physically routed: a pressure LFO on the wind group
  (research-calibrated: 6 Hz, ±22 % pressure ≈ ±12 cents FM through the
  pitch path, ~1 dB AM through the gain path — one modulation source,
  consistent AM/FM like a real trem valve). Engage/disengage ramps over
  ~0.7 s; rate and depth wander ±8 % as slow damped random walks
  (xorshift, RT-safe), because a metronomic trem sounds fake. Works on
  sag-disabled chests. Sidecar `[tremulant] rate_hz / depth_cents /
  chests`. Engine: SetTremulantParams / SetTremulant commands. Tests pin
  depth (±0.64 % rate factor) and rate (12 cycles in 2 s).
- **Web console** (temporary until M5's IPC + native GUI):
  `http://127.0.0.1:9669/` (`--http-port`), served by the server on a
  thread via tiny_http. Draw/retire stops live (retiring stops its
  sounding voices via tracked (stop, handle) pairs), tremulant toggle,
  master gain slider. Single embedded HTML page, no build step, no
  external assets. Endpoint smoke tests included. 63 tests green.

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
