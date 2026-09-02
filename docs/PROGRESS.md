# Progress log

Newest first. One entry per work session; keep entries factual and short.
Milestones refer to DESIGN.md.

- [2026-09-02 — the combination action, whole: divisionals, stepper, crescendo, piston rail](progress/2026-09-02-combination-action.md)
- [2026-09-02 — release tails play off the disk (gap §3e)](progress/2026-09-02-disk-streaming.md)
- [2026-09-02 — a recorded A/B against GrandOrgue](progress/2026-09-02-ab-grandorgue.md)
- [2026-09-02 — the release splice serves both channels](progress/2026-09-02-stereo-release-alignment.md)
- [2026-09-02 — the swell box pushes back, and boxes nest](progress/2026-09-02-box-pressure-and-nested-boxes.md)
- [2026-09-02 — Hauptwerk sets load (M7 opens)](progress/2026-09-02-hauptwerk-loader.md)
- [2026-09-01 — tuning scopes: sets, stops, ranks within stops](progress/2026-09-01-tuning-scopes.md)
- [2026-09-01 — console size in Preferences](progress/2026-09-01-console-size.md)
- [2026-08-29 — the pitch anchor is a key/Hz pair, not "a′"](progress/2026-08-29-pitch-reference.md)
- [2026-08-28 — user preferences vs organ settings, fully separated](progress/2026-08-28-prefs-split.md)
- [2026-08-27 — the tremulant stops being a Hammond](progress/2026-08-27-tremulant-physics.md)
- [2026-08-27 — right-click a drawknob: the stop editor](progress/2026-08-27-stop-editor.md)
- [2026-08-27 — the computer keyboard plays hex manuals](progress/2026-08-27-qwerty-hex-surface.md)
- [2026-08-27 — the hex field becomes a real isomorphic board](progress/2026-08-27-hex-field-layouts.md)
- [2026-08-27 — divisions per octave, first-class](progress/2026-08-27-divisions-per-octave.md)
- [2026-08-26 — voicing trims and the first combination action (gap §7)](progress/2026-08-26-voicing-and-generals.md)
- [2026-08-26 — the memory wall comes down: 16-bit residency, parallel decode, load cache (gap §3)](progress/2026-08-26-memory-wall.md)
- [2026-08-26 — tremulants come alive; attacks stop machine-gunning (gap §2+§4)](progress/2026-08-26-tremulants-and-multi-attack.md)
- [2026-08-24 — M6 contemporary layer: pitch moves, sound routes](progress/2026-08-24-m6-contemporary-layer.md)
- [2026-08-24 — keyboard kinds + Scala scales (M6 opens)](progress/2026-08-24-keyboard-kinds-and-scala.md)
- [2026-08-20 — a second job is asked about, never assumed](progress/2026-08-20-second-jobs-are-asked-about.md)
- [2026-08-18 — extension prefers a rank's real pipes over repitching](progress/2026-08-18-unit-rank-extension.md)
- [2026-08-18 — couplers become routes](progress/2026-08-18-flexible-couplers.md)
- [2026-08-18 — anything can be bound to anything](progress/2026-08-18-bindings.md)
- [2026-08-17 — repitching: playing keys the set has no pipes for](progress/2026-08-17-repitching.md)
- [2026-08-17 — MIDI assignment, read manual-first](progress/2026-08-17-manual-first-midi-assignment.md)
- [2026-08-16 — MIDI assignments persist, per organ](progress/2026-08-16-midi-assignments-per-organ.md)
- [2026-08-16 — menu bar + preferences, per-device MIDI routing](progress/2026-08-16-menu-bar-preferences.md)

## 2026-08-12 — enclosures / swell boxes (gap analysis §1)

Research mandate ("lots and lots of active research"): three parallel
sweeps — swell-box acoustics (Pykett 2023 position-resolved spectra,
Braasch JASA 2008 10–20 dB measured range, Palo's 3-organ difference
spectra, slit-transmission literature), Hauptwerk archaeology (CODM
guide 6-param per-pipe shelf spec, EnclosurePipe table, inertia, frozen
releases), GrandOrgue source (GetAttenuation linear-amplitude floor,
one-block fader ramp, gain baked into detached releases, open issue
#717 asking for exactly a shelf filter). Written up with citations in
docs/research/enclosure-modeling.md.

Model: per-voice gain + one-pole high-shelf (same form as the
brightness tilt, hinged at the box corner). Broadband floor from ODF
AmpMinimumLevel, shelf −10 dB default, corner morphing 8 kHz→1 kHz
geometrically (slit cutoff ~1/opening); dB-linear pedal taper (HW's
law, NOT GO's linear amplitude); critically damped 2nd-order shutter
inertia (0.5 s default) engine-side; per-voice ~5 ms one-pole gain
de-zipper (a linear block ramp failed on giant offline blocks —
measure before theorizing paid off); release tails freeze box state at
pallet close. Loader reads [Enclosure]/windchest membership; console
maps expression CC (default 11) per manual via stop→rank→chest→box;
web console gets per-box sliders; sidecar [enclosures] exposes
floor_db/shelf_db/corners/taper/full_sweep_s/cc. Deferred, documented:
closed-box pressure rise into the wind model, multi-box windchests.

Verification: engine tests measure −14 dB low / −24 dB high band
closed (matches params), bit-identical tails under post-release pedal
moves, click-free sweeps; demo-set test pins ODF→spec→CC wiring.
Rendered takes measured 300 Hz −13.6 / 6 kHz −22.0 dB closed-vs-open,
taper −7.9 dB at half. RTF 0.02 on the 4-stop Récit chord (enclosure
cost invisible). 105 green (was 92). Takes: swell_ab, swell_sweep,
swell_release_freeze.

## 2026-08-11 — mixture staccato + the bell discriminator (2d3125a..21f9c06)

User: mixture/octavin staccato releases "pingy, like bells". Diagnosis
from own renders: no level bump — the ping is a high pipe's recorded
resonator ring (near-pure decaying sine = a small bell's signature).
Fixes: (1) two-part staccato tail — full level through speech-off +
early reflections (~150 ms), then room-charge level AND (1−charge)·25
dB/s extra decay for the undeveloped diffuse field; (2) λ fits stop 45
dB below tail peak (noise floors flattened treble measurements); (3)
repitch comp clamp ±15→±25 (mixture −600-cent ranks need −16..−18);
(4) release pitch sag — bells don't bend, pipes do (Viscount patent,
Aeolus release detune): 4·sqrt(f0/100) cents clamped 1–12, τ ≈ 12
periods, strength A/B'd by the user on a rendered 30 s French plein
jeu piece (38 cents "far far too much", 12 "really nice" → locked).
Octavin staccato tail 3.7 s → 2.4 s. Render tests: render_mixture_
staccato, render_listening_demos, render_plein_jeu_music. Workflow
that worked: render takes, attach to Discord, iterate on his ear.

## 2026-08-11 — release realism: the room was transposing (ce593d8)

Research mandate ("read a ton of papers"): three parallel literature
sweeps (Rucz 2015 thesis — the only quantitative organ release
measurement found; Fletcher & Rossing; HW/GO docs, source, and forum
archaeology; sampler DSP incl. Signalsmith crossfade analysis and SFZ
rt_decay), written up with citations in docs/research/release-modeling.md.
Then numerical measurement of the engine against the demo set found the
"artificial/bell" release had three causes:

1. Repitch time-scales the recorded room decay: the demo set records 2
   pipes/octave and repitches ±600 cents, so down-repitched keys rang
   41% too long (bell), up-repitched were plucks — key-dependent wrong
   ring time. Fixed by measuring each sample's tail decay rate λ at
   load and compensating λ(R−1) dB/s (clamp ±15) in release(). Ring
   time is now key-invariant; native-pitch samples untouched. Novel —
   neither GO nor HW does this.
2. The GO staccato port decayed tail RATE; physics says rate is
   invariant, level builds. Now: room-charge level scaling.
3. Fixed 30ms crossfade; now ~9 fundamental periods (184→6ms), capped
   by note age (mid-attack releases must collapse, not swell).

Plus EOF guard fade (46ms). Diagnostic tests: release_envelope
(#[ignore], /tmp wavs + level-match inputs), regression
repitched_release_rings_at_native_decay_rate. 92 green. Debug journey
recorded: envelope-vs-reference red herring (master-gain accounting),
√2 spec.rate discovery via RELDBG, per-pipe ODF archaeology (PIPEDBG).

## 2026-08-11 — splice-kink follow-up: completion frame applied tail_gain twice (50f0fd9)

User confirmed the teleport fix by ear ("can't hear any crackles"); his
63 s recording measured 0 teleport-class events and exactly 1 faint
kink. Chasing that kink with the per-voice probe: XFADE-DONE storms
correlated, and component dumps showed the crossfade-completion frame
returning with the just-folded gain — tail_gain applied twice on that
single frame (blend already scales the tail leg), dipping it by up to
5x (staccato floor 0.2; trills expose it). Fixed with a frame_gain
snapshot before the phase arms mutate self.gain. crackle_hunt floor
tightened 0.015 → 0.008, zero events, permanent. Also: "*" stop
pattern + demo sidecar defaults to full organ (9a4c782).

## 2026-08-11 — THE crackle bug: shed tails re-entering the sustain loop (da32937)

The user's `--record` capture ended the months-long saga. His wav showed
single corrupted frames at exact 512-frame block boundaries; the
`crackle_hunt` reproduction (44.1 kHz, coupled tutti, PRODUCTION stagger,
fast spam/chords/trills, d2 click scan) reproduced 142 of them, and a
temporary per-voice probe fingered every one: `phase=FadeOut`,
`fade=1.001`, cursor teleporting (e.g. pos 138434 → 69476 in one frame).

Root cause: loop-wrap and seam-tap logic gated on `phase != Tail`. Tail
shedding and KillVoice flip Tail→FadeOut, so a cursor deep in release
material became eligible for sustain-loop wrapping and jumped back into
full-level sustain at amplitude 1.0. Big jumps = "previous notes
reappear and bang"; small jumps = the persistent crackle; shedding runs
constantly under fast playing with full registrations, in --safe, at any
buffer, with or without RT priority — hence the symptom's immunity to
every prior fix. Fixed with an explicit `past_loop` flag (set at
crossfade handover) gating wrap, seam reads, and FadeOut EOF. Outgoing
crossfade leg restored to full sinc (linear shortcut kinked splice
starts; mass-release worst block 1.59 ms of 5.33 ms). Hunt now runs in
the default suite: 142 → 0 events above the 0.015 teleport floor.

Remaining quality item: ~-40 dB R-channel kinks at some release splices
(stereo phase alignment is L-biased). Also this session: RealtimeKit RT
promotion (MakeThreadRealtimeWithPID via dbus-send; plain
MakeThreadRealtime resolves tids in the CALLER), sample-rate landmine
fix (never with_max_sample_rate — pick f32 nearest 48 kHz), commit-
stamped `=== DIAGNOSTIC:` startup line (build.rs tracks .git/refs, not
just HEAD), Engine::limiter_gain_db diagnostic.

Lessons, earned expensively: force the recording experiment before any
blind fixing; cleanliness tests must run production config (release
stagger was zeroed in every test, hiding the staggered path); phase
enums that conflate lifecycle stage with material location breed
exactly this bug class.

## 2026-08-11 — the F-major pop: pipes must speak once (refcounting)

User's precise repro (6-note F major, all Great+Swell, coupled 16'+8')
finally identified the real mechanism — and it was the pipe-doubling
bug spotted during the step-back and wrongly deferred as "bonus
correctness". With octave doublings + a 16' coupler, one pipe is
reached by TWO held keys (F4's 16'-coupled Swell pipe is F3's
8'-coupled pipe). Two voices on the same sample sum incoherently while
held (+3 dB), but at release the phase aligner sends BOTH to the same
tail at the same phase — coherent +6 dB — so the release is LOUDER
than the chord: a thump/pop scaling with how many pipes the chord
doubles. Console now refcounts speaking pipes: a pipe starts one voice
regardless of how many routes reach it, and stops only when the last
holder (key, coupler, stop) lets go. Retrigger, stop-retirement, and
all-off flow through the same refcount. Regression: octave-coupled
shared pipe starts once, survives the first key's release, stops with
the last. 88 tests green.

## 2026-08-11 — step back: the coupled-tutti spike + the octave-ghost bells

User: still crackles/pops on big coupled registrations, and releases
still bell-like; GO never crackles. Took the demanded step back, built
his EXACT scenario as a stress test (all Great+Swell stops, Swell→Great
at 8' and 16' = ~241 voices/chord) and measured: average 38–40 % of
realtime but **worst block 5.10 ms of the 5.33 ms budget** — the pops
are deadline breaches at chord transitions. Causes found and fixed:

- **O(N·M) command handling**: every StartVoice scanned 2048 slots for
  a free one; every StopVoice scanned all voices for its handle — a
  241-voice chord ran ~½ million scans in one block. Now: free-slot
  stack (O(1) allocation, invariant-maintained at every Idle
  transition) and StopVoice batching (sorted batch, ONE pool pass).
- **Crossfade storm**: a mass release doubles every voice's read cost
  for 30 ms. The outgoing (dying) leg now uses linear interpolation —
  its error fades to zero with it — and the pallet stagger widens
  adaptively with release-batch size (real tuttis spread too). Mass
  release worst block: 2.38 → **1.22 ms**.
- **AVX2**: mono kernel kept (2×8-wide FMA); stereo AVX2 measured ~10 %
  SLOWER than SSE2 on this host (shuffle overhead) and is parked
  behind a dead-code flag.
- **The bells, root-caused**: alignment used correlation *argmax*; on
  principal pipes whose 2nd harmonic rivals the fundamental, a
  half-period-off splice can win — fundamental cancels, octave
  reinforces: a missing-fundamental strike IS a bell. Replaced with
  **quadrature phase projection** at the measured fundamental
  (harmonic-immune, exact, cheaper), for embedded tails and separate
  releases both. New regression: strong-2nd-harmonic pipe stays
  fundamental-locked (the old argmax path had no such guarantee).
  A sign-convention bug was caught by the mistuned-pipe test — the
  phase-0-anchored tests couldn't see it.

87 tests green. If pops persist on the user's machine (check the RT
priority log line!), next tier: block-render/SoA refactor.

## 2026-08-09 — the spam-distortion saga: measured, found, fixed

"Fast playing distorts, slow holds don't" survived three level-side
fixes (−15 dB gain staging, master limiter, retrigger voice doubling —
all real bugs, all shipped). The breakthrough was reproducing it
headless: a spam stress test (10-key cluster hammered for 8 s over the
plein jeu) showed the output was *mathematically clean* but the engine
ran at **65 % of realtime in release** — his multi-finger spam
trivially crossed 100 % → audio callback overruns → crackling. It was
CPU all along.

Fixes: (1) per-frame invariants hoisted to per-block `VoiceBlockContext`
(`Sample::frames()` hid a u64 division — two per frame per voice);
(2) **release-tail shedding**: above a 128-tail budget the quietest
tails fast-fade, ≤8 per block (HW's documented polyphony strategy) —
spam can no longer pile up unbounded render cost. Result: **65 % →
37 %** of realtime at the stress load, with a hard ceiling now.
The stress test stays in the suite with a release-mode RT assertion
(< 50 %). Next perf tier when needed: horizontal-SoA SIMD (researched).

## 2026-08-09 — stop noises done right (user report)

Noise "stops" were showing as drawable stops. Investigating the actual
noise wavs revealed the GO-set convention: each is structured like a
pipe — pull-thump attack → **near-silent sustain loop** → push-in thump
as the release tail (Motor.wav likewise: blower start → running drone →
wind-down). So the right mechanism was already built: the engine's note
lifecycle. Drawing a stop note-ons its noise voice (thump, then holds
the silent loop); retiring note-offs it (splices to the push-in thump).
Same for coupler clacks and the tremulant. Level matcher gained a guard:
a near-silent loop means the tail is *meant* to be louder — play it as
recorded (the matcher would have crushed thumps to 5 %). Noise voices
draw no wind and skip the brightness filter. New `KillVoice` command
silences open noise voices when noises are disabled mid-flight.
Classification by name ("noise"), fuzzy-mapped to their control
(normalized-prefix/token scoring handles "Fl Harm 8' stop noise" →
"Flute Harm. 8'"). Hidden from all UIs; global enable + volume in
sidecar `[noises]`, `/api/noises`, web console, native GUI. E2E test on
the demo set covers the whole lifecycle. 83 tests green.

## 2026-08-09 — marathon 4/N: convolution reverb (UPOLS)

Uniformly partitioned overlap-save convolution (Wefers 2015, the
canonical real-time scheme; Gardner-style non-uniform zero-latency is
the noted v2 upgrade) in `engine/reverb.rs`: 256-frame internal blocks,
frequency-domain delay line, one FFT + P complex MACs + one IFFT per
block per channel; all storage preallocated control-side (RT invariants
hold), true-stereo IRs, energy-normalized, IR resampled via the sinc
reader only when rates differ (same-rate IRs pass through untouched —
the sinc kernel's 0.9-Nyquist cutoff would soften taps). Wet trails dry
by one block (~5 ms pre-delay). Sidecar `[reverb] ir/wet` — a wav next
to the set or `"synthetic"` (generated 1.4 s RT60 stereo hall) — plus
`/api/reverb`, web-console slider, native-GUI slider. Tests: impulse →
IR taps reproduced at exact positions/ratios; wet=0 bit-exact
passthrough; 8-partition tail rings after input stops. 82 tests green.

## 2026-08-09 — marathon 3/N: native GUI (aristide-gui v1)

`aristide-gui` is now a real eframe/egui 0.36 desktop app: stops as
toggle pills grouped by manual, couplers, tremulant, gain slider
(drag-safe against polling), full tuning panel (temperament dropdown,
a′ drag, transpose). All I/O on a dedicated network thread (ureq)
talking to the server's local HTTP API at 4 Hz with a command channel —
the UI thread never blocks; server death shows a banner and recovers.
Protocol layer (state JSON, command→query mapping) unit-tested; the
first *visual* run necessarily happens on the user's machine (this box
is headless) — v1 kept deliberately conservative for that reason.
Run: `cargo run --release -p aristide-gui` (optional arg: server URL).

## 2026-08-09 — release pop/ping fixes + web-UI tuning controls

Two user reports investigated with data:

- **Pop/crackle at key release**: introduced by marathon-1's multi-loop —
  the wrap jumped from loop A's end into loop B's start, an unvalidated
  splice (only end→own-start is author-guaranteed seamless). Now wraps
  always return to the current loop's own start; variety comes from
  choosing which loop's end to run toward next (all loops share one
  continuous recording, so that path is seamless — GO's scheme too).
  Regression: max frame-to-frame delta bounded in a two-loop fixture.
- **"Pinginess", worst on high pipes**: alignment correlation window was
  one period — ~30 frames on a 1.5 kHz pipe, far too little to lock
  phase against room noise, so high-pipe splices clicked and the click
  rang in the recorded reverb. Window now 2 periods clamped to ≥128
  frames (test: phase lock within 0.12 cycle on a noisy 30-frame-period
  pipe). Also: tail reference level window 2048→512 frames (fast
  high-frequency decay under-read the start level) and tail-gain boost
  cap 1.3→1.1 (never louder than the note being released).
- Tuning controls (temperament/a′/transpose) added to the web console —
  per the new standing rule: every feature ships in the testing UI.
  76 tests green (GUI crate builds separately).

## 2026-08-09 — marathon 2/N: temperaments, concert pitch, transposer

First slice of the M6 contemporary/tuning layer, entirely control-side
(per-note rate multiplier folded into StartVoice — the RT engine's
"pitch travels as Hz" design paying off). Temperaments: Equal,
Werckmeister III, Kirnberger III, ¼-comma meantone, Pythagorean —
a-referenced precise cent tables cross-checked against Carey Beebe's
reference (hpschd.nu/tech/tun/cents.html; every entry matches their
rounded values). Concert pitch a′ = 300–500 Hz; transposer shifts key
routing (selects different pipes, like the console gadget). Sidecar
`[tuning]` + `/api/tuning` endpoint. Tests: table-vs-CBH, a′ invariance
across temperaments, meantone retune factor, transpose routing/compass.
76 tests green.

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
