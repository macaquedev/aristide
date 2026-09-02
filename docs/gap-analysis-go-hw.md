# Gap analysis: Aristide vs GrandOrgue and Hauptwerk

Written 2026-08-12 by the analysis agent (thread "engine gap analysis vs GO/HW").
**Re-verified and rewritten 2026-08-26** against the code at `616c03f` — much of the
original has since been implemented (enclosures, pitch reconciliation, routing/buses,
velocity, Bass/Melody couplers, tail freezing). Section numbers are kept from the
original for continuity; each now states its current status up front.
Scope: ways Aristide's audio engine is **worse** than GrandOrgue and/or Hauptwerk,
each with what GO/HW actually do, what Aristide does today (with code refs into
this repo), how audible/important the gap is, and hints for whoever fixes it.

Ground truth sources:
- GO: the actual source (github.com/GrandOrgue/grandorgue, master as of 2026-08);
  class/file names cited below are real. See also `docs/go-critique.md`.
- HW: official Milan Digital Audio PDFs — Hauptwerk V Features Data Sheet,
  User Guide v5.0.1, Hauptwerk 9.0.1 Release Notice (cumulative v5→v9),
  HW4 datasheet — plus Sonus Paradisi / Piotr Grabowski producer docs.
  Items that rest only on forum/search snippets are flagged.
- Aristide: full read of `crates/aristide-engine/*`, `crates/aristide-server/*`,
  `crates/aristide-formats/*`, `crates/aristide-model/*` at current main.

Related repo research (don't duplicate, extend): `docs/research/release-modeling.md`,
`docs/research/vpo-rendering-techniques.md`, `docs/research/hauptwerk-wind-model.md`,
`docs/go-odf-notes.md`.

Suggested overall priority (2026-08-26 revision, updated through the day):
§2+§4, §8+§9, §3's core (16-bit residency, load cache, parallel decode) and
§7's core (voicing trims, generals + setter) have all landed. **Next**: the
residues by musical value — divisionals/sequencer (§7), mid-hold wave-trem
switch (§2), streaming (§3) — plus §5/§12 residues and the HW-only fidelity
gaps (§10/§11).

---

## 1. Swell boxes / enclosures — ✅ LARGELY DONE (was: missing entirely)

**GO:** `[Enclosure]` ODF sections; each windchest holds enclosure pointers;
`GOEnclosure::GetAttenuation() = (MIDIValue*(100-AmpMinimumLevel) + 127*AmpMinimumLevel)/12700`
(linear amplitude in the MIDI value between `AmpMinimumLevel`% and 100%),
multiplied into the windchest volume once per buffer. No filtering — pure gain.

**HW:** frequency-dependent shutter *filters* per enclosed pipe, shutter inertia
modeled, releases unaffected by shutter movement after key-off, pressure rise
inside a closed box modeled, per-pipe voicable swell amplitude/harmonics mod
depths. (HW V Features Data Sheet p12; UG5 p213.)

**Aristide now:** implemented end to end, and closer to HW than GO in kind:
- Loader reads `NumberOfEnclosures`/`[EnclosureNNN]` (`Name`, `AmpMinimumLevel`,
  `MIDIInputNumber`, `Displayed`) and windchest→enclosure membership
  (`grandorgue.rs::build`); model has `Enclosure`/`Windchest.enclosures`.
- Engine box model (`aristide-engine/src/enclosure.rs`): broadband gain floor
  (from `AmpMinimumLevel`) **plus a one-pole high-shelf** whose corner slides as
  the box closes, dB-linear taper law, critically-damped second-order shutter
  inertia (`full_sweep_s`). This beats GO's pure gain and approximates HW's
  shutter filtering.
- Release tails freeze their enclosure state at key-off (`lib.rs::process`
  refreshes box factors only while `Held`) — HW's correct rule.
- Control: sidecar `[enclosures] cc` (default CC11) → `Console::expression_manual`
  (moves every box a manual's stops sit in); generic `Action::Enclosure(name)`
  bindings for any MIDI trigger/computer key; `POST /api/enclosure`; console UI.
- User-defined boxes: `[[enclosure]]` sidecar tables let a user enclose arbitrary
  stops of a set that declares none (`config.rs`, `/api/organ/enclosure/*`).
- `--safe`/lite mode keeps the broadband gain and skips only the shelf filter.

**Remaining gaps (small):**
- The box character (shelf depth, corners, taper, inertia) is one sidecar-wide
  `EnclosuresConfig` — only `floor_db` is per-box (from the ODF). Per-box
  overrides are earmarked for the voicing layer (`sidecar.rs` comment).
- ODF `MIDIInputNumber` is parsed but never consumed — per-enclosure independent
  CC assignment from the ODF isn't wired (bindings cover it manually).
- A chest in multiple enclosures uses only the first, with a warning
  (`server/bank.rs::chest_enclosures`). GO composes all of them.
- No per-pipe filter/gain depths (HW voicing territory). `MAX_ENCLOSURES = 16`.

---

## 2. ODF tremulants unsupported; tremmed samples can't play — ✅ LARGELY DONE (2026-08-26)

**GO:** two tremulant types (`GOTremulantType`): synth and wave.
- Synth (`GOSoundProviderSynthedTrem`): 16-bit sine control signal, applied as a
  block-rate *amplitude-only* scalar on the windchest (no FM). Weak — ours is
  better in kind.
- Wave trems: attacks/releases marked `IsTremulant=1` in the ODF. Engaging the
  tremulant calls `GOWindchest::UpdateTremulant` → each sounding pipe
  crossfades into its tremmed attack (`SwitchToAnotherAttack`, default 184 ms
  key-scaled fade, phase-aligned via `InitAlignedStream`), and releases are
  selected with matching tremulant state (`ReleaseSelector.m_WaveTremulantStateFor`).
- Releases are moved to a "detached release" task so tremulants stop modulating
  them after key-off.

**HW:** modeled tremulant uses **per-pipe waveforms measured from real tremmed
recordings**, modulating pitch, amplitude AND harmonic content per pipe, all
phase-synchronized, with continuous subtle depth randomization; rate voicable
per tremulant. Sets may alternatively ship real tremmed ranks that play directly.
(DS5 p12/p15; UG5 p215.)

**Aristide now (commits `46d833f`/`b1bd337`):**
- `[TremulantNNN]` parses (Synth figures — Period is *ms* per cycle, see the
  corrected `go-odf-notes.md` §6 — and the Wave marker); windchests carry
  tremulant membership; composites renumber it like enclosures; adoption is
  test-proven equivalent.
- **Synth trems sound**, per chest, from the ODF: rate = 1000/Period Hz,
  pressure depth inverted through the wind gain exponent so the author's
  `AmpModDepth` amplitude comes out (FM + brightness then follow physically —
  still better in kind than GO's AM-only trem), engage/disengage ramp = GO's
  two `1/rate`-second ramps averaged. Demo's Tremblant: 5.1 Hz, Récit only.
- **Wave trems switch recordings**: `Command::SetWaveTremulant` flags the
  chest; note-ons prefer `IsTremulant=Y` attack variants (§4 machinery);
  note-offs select releases matching the per-voice state captured at key-off
  (held voices follow the switch; tails keep what they released under).
- Per-tremulant control: `State.trems`, `tremulant:<name>` bindings,
  `/api/trem?idx=`, `"trems"` in the state JSON. A hand-written sidecar
  `[tremulant]` (now `Option`) replaces the set's own; the default-tremulant
  fallback remains for trem-less sets.

**Remaining gaps:**
- **Mid-hold wave-trem attack switch** (GO `SwitchToAnotherAttack`, 184 ms
  key-scaled phase-aligned crossfade): engaging a wave trem doesn't make
  already-held notes undulate until re-pressed. Needs an engine
  crossfade-into-another-sample's-loop path (the release-splice machinery
  pointed at a loop instead of a tail). Synth-trem sets are unaffected.
- Console UI still renders one knob (toggles all); a per-tremulant panel is
  screenshot-harness work.
- HW's measured per-pipe trem waveforms remain §11.

---

## 3. Memory & load scalability (worse than GO and HW) — ✅ LARGELY DONE (2026-08-26)

**GO:** per-pipe/organ `BitsPerSample` 8/12/16/20/24; optional lossless delta
compression in RAM (`encode = val - (prev + (prev-last)/2)`, 8/16-bit packing,
skipped when it doesn't shrink); zlib disk cache of the fully decoded/analyzed
set keyed by load-parameter hash (instant reloads, `GOCache*`); `GOMemoryPool`
with mmap-backed cache and a page-touch task; multi-threaded loading
(`LoadConcurrency`); per-rank options: mono downmix, first-loop-only,
first-release-only, attack-load policy.

**HW:** all-RAM (explicitly rejects streaming), but 16/20/24-bit load resolutions
(20/24 stored 32-bit-aligned; 20-bit is the documented sweet spot), **lossless
compression on by default** (disabling costs 40–60% more RAM, "no effect on
audio"), per-rank disable of multiple attacks/loops/releases, release truncation
at load, rank enable/disable. (UG5 p76; DS5 p14.)

**Aristide now (2026-08-26):**
- **(a) 16-bit residency is the default** (`SampleData::{F32,I16}`;
  sidecar `[samples] bits = 16|32`, 32 = bit-exact f32 for A/B). Analysis
  (periods, phase maps, tail measurement) always runs at full decode
  precision, then quantizes. Dedicated i16 SIMD sinc kernels (SSE2 + AVX2,
  sign-extend in-register, dequant scale folded into the final sum) hold the
  read cost to ≈ +8% for −50% RAM and halved memory traffic; the equivalence
  test pins i16 reads within quantization noise of f32.
- **(b) GO-style load cache** (`server/cache.rs`; `[samples] cache = false`
  opts out): decoded samples + all analysis persist under
  `~/.config/aristide/cache/`, validity **per entry** (source mtime+size and a
  hash of the exact decode inputs — the ODF attack/release record, aligning
  pitch, residency), so an ODF edit invalidates only what it touched. Atomic
  temp+rename writes; any structural surprise reads as a miss. Demo set:
  440 ms cold → ~30 ms warm.
- **(c) parallel decode**: every unique file decodes and analyzes on a worker
  pool (`available_parallelism`), assembly stays sequential.
- **(e) disk streaming of release tails** (2026-09-02): everything through the
  last sustain loop stays resident — a held note never waits for a disk — plus
  0.35 s of each tail so the splice at note-off starts instantly; the rest is
  read from a seekable store by streamer threads into per-voice SPSC rings and
  a linear window the sinc kernels dot against. Streamed tails are **bit-
  identical** to resident ones. The load cache became the store: entries are
  always split (`<hash>.samples` head + `<hash>.tails`), so one cache serves
  both residencies and a warm streaming load copies no audio at all. Failures
  are fades, never clicks — an underrun freezes on the last frame it has and
  takes the 15 ms kill ramp; a release that finds no free slot plays its
  resident head and the EOF guard fades it. Sidecar `[samples] streaming =
  auto|on|off` + `ram_budget_mb`. Demo set: 85.8 → 55.1 MiB resident.
  See docs/progress/2026-09-02-disk-streaming.md.

**Remaining (the true residue):**
- (d) per-rank load options (mono downmix, first-loop/first-release-only,
  rank disable).
- Lossless delta compression (GO's) would buy another ~1.5–2× at RT decode
  cost — worth revisiting only if 16-bit residency still doesn't fit a
  target set.

---

## 4. Multi-attack samples parsed, then ignored — ✅ LARGELY DONE (2026-08-26)

**GO:** `GetAttack(velocity, releasedDurationMs)` — attack selection by
`AttackVelocity` (min velocity) and `MaxTimeSinceLastRelease` (fast-repetition
re-attack: a pipe restruck shortly after speaking gets a different, shorter
attack). Random tie-breaking among equal candidates. Plus
`MinVelocityVolume`/`MaxVelocityVolume` gain ramp.

**HW:** multiple attack/sustain samples per pipe selected "to model tracker-action
response, randomly to reduce repetition, re-attack after recent stop"; layered
samples with per-layer selection (separately controllable chiff layer);
velocity→tracker-action model modifying attack harmonic content/pitch/amplitude.
(UG5 p76; DS5 p15.)

**Aristide now:**
- **Velocity gain is done** (`589691a`): the loader reads per-rank
  `MinVelocityVolume`/`MaxVelocityVolume` (`grandorgue.rs`, →
  `model::VelocityVolume::gain`), and `Console::note_on_manual` prices every
  voice through the ramp — including voices started late by a stop drawn
  mid-hold or a coupler recouple, which reuse the held press's velocity.
  Old §12c is closed.
- **Attack selection is real** (`b1bd337`): every variant decodes into the
  bank (`LoadedBank.attack_options`, borrowed pipes inherit their target's
  table); `console.rs::price` runs GO's `GetAttack` at all three voice-pricing
  sites (note-on, stop drawn mid-hold, recouple) — candidates gated by the
  `IsTremulant` tri-state vs the chest's wave-trem state, `AttackVelocity ≤`
  the press, `MaxTimeSinceLastRelease` against a per-pipe last-release clock;
  most-specific wins (highest velocity bound, then tightest window), ties
  rotated by xorshift. Separate releases attach to *each* variant and are
  selected engine-side by (trem state, hold time).

**Remaining gaps (small):**
- Additional attacks are assumed at the primary's recording pitch (rate
  follows file sample rate only); GO tunes each file through the full pitch
  pipeline.
- Release selection takes the first qualifying `MaxKeyPressTime` (sorted);
  GO adds a random rotation among exact ties.
- The last-release clock keys on (rank, sounded identity), not the physical
  pipe, so a *different* key borrowing the same pipe doesn't count as its
  re-attack.
- HW's layered chiff / velocity-morphing attacks remain out of scope.

---

## 5. Output routing — ✅ LARGELY DONE (was: stereo only); single-device + ODF AudioGroup remain

**GO:** engine mixes per audio *group* (stereo pairs), then a per-device-channel
dB matrix (−120…+40 dB) routes any group L/R to any channel of **multiple
simultaneous devices** (RtAudio/PortAudio/JACK). Pipes assigned to groups via
per-pipe/rank `AudioGroup`. Built-in multi-channel WAV recorder + downmix recorder.

**HW:** up to 1024 primary buses → 8 intermediate → 8 master buses per preset,
arbitrary inter-bus routing, any bus to any device channels; per-rank
bus-allocation algorithms to spread pipes over speakers; **4 output perspectives**
with per-pipe mix levels = per-pipe surround positioning; per-bus convolution
instances. (DS5 p4; RN9 pp56–58.)

**Aristide now (M6, 2026-08-24):** 8 stereo buses (`aristide-engine/src/routing.rs`,
bus 0 = main pair). Each bus: gain, output channel pair, and a delay insert
(`ms`/`feedback`/`mix`/`dry`) with a ~100 ms slewed read head (live time changes
bend tape-style). Assignment via sidecar `[[routing.bus]]` (stop/manual name
patterns → `output = [L, R]` 1-based); per-pipe onset delays via
`[[voicing.delay]]` (a delayed pipe is silent and windless until onset; released
early, it never speaks). The cpal stream widens to the channels routing asks for
when the device offers an f32 layout at the same rate; otherwise routed buses
fold to the main pair with a warning — wrong, never silent (mono devices fold
L+R). `POST /api/bus` is the live knob. Default path is bit-identical to the
pre-bus engine.

**Remaining gaps:**
- **One audio device** (`default_output_device()` only) — GO drives several at
  once. Multi-device is a named M6 deferral.
- **GO's ODF `AudioGroup` key is never read** — a GO set's authored speaker
  groups are silently ignored; only a hand-written sidecar recreates them.
  Cheap win: translate `AudioGroup` → bus assignments at load.
- No multi-perspective/surround handling (buses are fixed stereo pairs; no
  per-pipe perspective mixes). No bus→channel dB *matrix*, no per-bus IR.
- Bus inserts are delay-only; the full node graph is deferred to overlap M4.
- **NEW BUG (found 2026-08-26): `--record` writes a corrupt WAV on widened
  streams.** The engine tap is post-routing and channel-count-agnostic (good),
  but `spawn_recorder` (`main.rs`) hardcodes a stereo 16-bit header. Once
  `ensure_channels` widens the device beyond 2, the recorder keeps writing
  N-channel interleaved frames under a 2-channel header — a mislabeled file.
  Fix: thread the channel count into the recorder at spawn and on every
  stream rebuild (rewrite header or start a new segment).

---

## 6. Pitch metadata pipeline — ✅ MOSTLY DONE (was: silent wrong-pitch risk)

**GO:** expected pitch = key MIDI note adjusted by `HarmonicNumber` (default 8 =
unison 8′); sample pitch = embedded `smpl` chunk `MIDIUnityNote` +
`MIDIPitchFraction`, overridable by ODF `MIDIKeyNumber`/`MIDIPitchFraction`;
auto-tuning correction retunes the difference, gated by `IgnorePitch` and
switched per-temperament (`isOriginalBased`); plus `PitchTuning` and
`PitchCorrection` cents through the organ→windchest→rank→pipe inheritance chain.
~100+ built-in temperaments; "original temperament" mode bypasses auto-retune.
See `go-odf-notes.md` §"two pitch paths" (updated 2026-08-26).

**HW:** many historical temperaments switchable instantly; "original recorded
tuning" mode; global pitch adjustable. Per-pipe tuning voicable. (DS5 p12.)

**Aristide now (commits `9b353cf`/`bf99cb3`):** real pitch reconciliation in
`server/bank.rs`. The loader reads `HarmonicNumber` (rank + pipe),
`MIDIKeyNumber`, `MIDIPitchFraction`, and chained `PitchTuning` +
`PitchCorrection`; the wav/wavpack readers surface `smpl` unity note/fraction.
Resolution mirrors GO: explicit ODF key wins over the file's smpl data. Rather
than GO's per-temperament path switch, Aristide compares the recorded-path cents
(`PitchTuning`) with the declared-pitch path (nominal-vs-recorded +
`PitchCorrection`) and retunes when they disagree by more than 50 cents
(`RETUNE_TOLERANCE_CENTS`), discarding auto figures past ±1800 cents as junk —
plus a junk-metadata guard: a rank whose files all stamp the *same* smpl unity
note across different nominal pitches has its smpl data distrusted (editor
default, not a measurement). Repitched/borrowed/extended pipes ride the same
math. The "silent wrong-octave" bug class is closed.

**Aristide now (2026-08-31, home pitch measured):** metadata is no longer
the primary truth. The engine measures every looped pipe's fundamental at
load (staged autocorrelation, ±600 ¢ around the set's own voiced expectation,
refined over up to 24 cycles), and `server/bank.rs` fits the organ's home
tuning from it — per-rank anchor, a-referenced 12-class table, best-matching
named temperament, spread — exposed as `LoadedBank::home` and in the state
snapshot. Each `VoiceSpec` carries `home_cents` (measured offset from
nominal). `Temperament::Original` (the default) plays as recorded with the
reference showing the organ's own pitch; every other tuning is a target that
bends each pipe from its measured pitch — HW's "retune from measured pitch"
with GO's "original temperament" as the resting state. Key placement is
measured too: a pipe >50 ¢ from its rank anchor + class model
(`REANCHOR_TOLERANCE_CENTS`) is moved onto the model. The `smpl`/ODF
metadata path below survives only for pipes that cannot be measured.

**Remaining gaps:**
- ~~`AcceptsRetuning`~~ ✅ (2026-08-26): parsed per rank (default true) and per
  pipe (default = rank), folded into `Pipe.accepts_retuning`; `false` skips the
  auto-retune reconciliation entirely. `IgnorePitch` turns out to be a
  **CMBSetting** (GO's per-organ user settings file), not an ODF key
  (`GOPipeConfig.cpp` reads it with `CMBSetting`) — the equivalent user-side
  override belongs to our voicing sidecar, not the loader.
- ~~The tolerance heuristic~~ superseded 2026-08-31 by measurement (above); the
  50-cent metadata tolerance now only governs unmeasurable pipes.
- Home fit is per organ with per-rank anchors; a composite of two pitch
  standards reads as a large spread rather than two named homes. Per-pipe
  drift is kept under `original` and flattened under a target (no
  "keep the character" option yet).
- **Named temperaments: still 5** (`server/tuning.rs::Temperament::ALL`) vs GO's
  100+. In practice Scala support (M6) unlocks arbitrary tuning: `.scl`/`.kbm`
  parsing (`model/scala.rs`), applied per manual with nearest-pipe re-anchoring
  (whole semitones move which pipe sounds; only the residual bends —
  `console.rs`). A GO-parity named-temperament list is now just table work,
  lower priority since any Scala archive file loads.

---

## 7. Voicing tools & combination action — ✅ CORE LANDED (2026-08-26)

**GO:** per-pipe hierarchical config editable live in the UI and persisted per
organ (.cmb): Amplitude, Gain dB, ManualTuning, TrackerDelay, ReleaseTail ms,
ToneBalance tilt, AudioGroup, per-pipe load options. Full combination system
(generals, divisionals, sequencer, crescendo).

**HW:** per-pipe voicing screens: tuning, amplitude+stereo balance, brightness,
trem/wind/swell mod depths per target with polarity, per-perspective mix +
parametric EQ + release truncation. Combination system, crescendo, floating
divisions, MIDI learn on everything. (UG5 pp213–215.)

**Aristide now:**
- **Control/bindings are no longer a gap:** a generic `Trigger`↔`Action` system
  (`server/control.rs`) binds any MIDI message or computer key to transpose,
  stops, couplers, tremulant, cancel, enclosures — with genuine **MIDI learn**
  (`/api/midi/bind`, `/api/control/bind`, learn state machines in `main.rs`),
  persisted per organ. The Tauri console (panel canvas) edits all of it.
- **Voicing trims** (2026-08-26): `[[voicing.adjust]]` — `gain_db` and
  `cents` by stop pattern, stamped at voice pricing exactly like routing;
  cents ride the same pitch fold as tuning so wind draw and brightness follow
  the sounding pitch. The fix for one honking stop or an unbalanced division.
- **Generals** (2026-08-26): `general:<n>` recalls a stored registration —
  stops, couplers, tremulants diffed to the stored state, landing on held
  keys like an electric action; `set` arms the setter so the next general
  press *stores* (and disarms, as consoles do). Stored as names in the
  per-organ user config (bindings' text-vocabulary rule: a name the loaded
  organ hasn't got is reported and skipped, never dropped from the file).
  `POST /api/general?n=&store=`, `"generals"`/`"setter"` in the state JSON.

**Remaining:**
- Voicing at *pipe* scope (key ranges) and a brightness/EQ leg; a console
  voicing editor and live HTTP adjustment (sidecar is load-time).
- Divisionals, the stepper/sequencer, crescendo; GO's `DivisionalsStore*`
  semantics; a console piston rail (UI work, screenshot harness).
- HW-style release truncation as a voicing parameter (from §8).

---

## 8. Release handling: producer intent ignored — ✅ MOSTLY DONE (2026-08-26)

Aristide's release *model* (phase-aligned splice, level match, staccato charge,
repitch decay compensation, release bend — `lib.rs::release`,
`docs/research/release-modeling.md`) is ahead of GO. The gaps around it are
unchanged since 2026-08-12:

- ~~ODF `ReleaseCrossfadeLength` ignored.~~ ✅ (`2c5e559`): overrides the
  pitch-scaled fade on both the embedded tail and each separate release
  (`ReleaseOption.crossfade_ms`), still capped by note age so mid-attack
  releases collapse. It is **milliseconds** — notes corrected from GO source.
- ~~`AttackStart`/`CuePoint`/`ReleaseEnd` unparsed.~~ ✅ (`2c5e559`):
  `AttackStart` moves the voice's start cursor (clamped to reach the loop);
  an explicit `CuePoint` outranks the wav cue chunk (junk inside the loop
  falls back); `ReleaseEnd` trims the attack's embedded tail; separate
  releases are cut to their `CuePoint..ReleaseEnd` window at decode.
- **No release truncation** — ⚠ still open. HW: load-time truncation with
  frequency-shaped decays for the "wet set → short tails → convolution" dry
  workflow, plus real-time truncation; GO: per-pipe `ReleaseTail` ms voicing.
  This is voicing-sidecar territory (§7): a per-voice fade trigger over the
  existing `tail_decay` machinery, shaped by `tail_decay_db_per_s` + pipe f0.

---

## 9. `LoopCrossfadeLength` unsupported (worse than GO) — ✅ DONE (2026-08-26)

**GO:** bakes raised-cosine loop crossfades into the end-segment at load when the
ODF asks (0–3000 ms, `DoCrossfade`, GOSoundAudioSection.cpp; loops too short are
dropped with a warning). Sets with imperfect loop points depend on this.

**Aristide:** loops are butt-joined; the sinc reader wraps kernel taps across the
seam (`resample.rs::read` slow path) which fixes *interpolation* clicks only. A
bad loop point still thumps every pass. (Butt loops are the right default —
Appleton 2019, 3 dB noise-dip argument in `vpo-rendering-techniques.md` §2.2 —
but only when the producer's loops are good.)

**Done** (`c0c50fa`): honored at decode in `server/bank.rs::decode` — GO's
raised-cosine blend baked into each loop's final frames toward the material
preceding loop start (fade = ms × rate / 1000; the key is **milliseconds**,
notes corrected). Loops too short for the fade stay butt-spliced, like GO's
warning-and-skip. Butt loops remain the default when the ODF asks for nothing.

---

## 10. Wind model narrower than HW's (HW-only gap; we beat GO here) — ⚠ unchanged

**HW:** fluid-dynamics wind *system* model since v2: producer-defined
bellows/reservoirs/trunks per division, air pressure/flow computed per pipe,
**every pipe interacts with every other through the shared supply**, turbulence
randomization, regulator table oscillation; per-pipe voicable wind mod depths.
(DS5 p7/p15; UG5 p150; RN9 pp28–29; `docs/research/hauptwerk-wind-model.md`.)

**GO:** none at all (`GOWindchest` is a routing group with a static volume).

**Aristide:** per-chest independent 2nd-order regulator (`wind.rs`), demand =
heuristic 1/f `wind_weight` (`server/bank.rs::wind_weight`), pressure →
pitch/gain/brightness exponents, per-voice flow noise, pallet-gulp attack boost.
Since 2026-08-12: released voices freeze their wind factors AND stop drawing
demand (chest pressure recovers while tails ring) — physically right. Still:
**no inter-chest coupling** (no shared blower/trunk sag across divisions),
**no per-organ wind topology** (sidecar scalar defaults only — GO ODFs carry no
wind data to import, so this needs sidecar schema), demand is a guess.

**Impact:** moderate — the audible core (sag, wobble, trem) exists; what's
missing is inter-division interaction and per-organ fidelity. Fine for now;
document as future sidecar work.

---

## 11. Modulation-depth realism vs HW (HW-only gap) — ⚠ unchanged

HW: per-pipe measured tremulant waveforms, per-pipe depths with polarity,
harmonic-content leg through real per-voice filters, constant depth
randomization. Reality (spectrogram studies): each harmonic has its own AM
depth/phase. Aristide: one sine per chest, uniform depth, brightness = single
1-pole tilt hinged at the 2nd harmonic (`brightness_coefficient`,
`server/bank.rs`). Ours breathes convincingly but can't match analyzed per-pipe
modulation. Long-term path (already in `vpo-rendering-techniques.md` §3):
offline AM/FM separation (DAFx-10) of producers' tremmed samples → per-pipe
depth tables feeding the existing wind-trem machinery.

---

## 12. Smaller correctness/robustness items

a. ~~Trem/wind modulates release tails.~~ ✅ **Fixed** (`608e802`): wind factors
   (`wind_rate`/`wind_gain`/`wind_treble`) refresh only while `Held` and freeze
   at release, matching the swell-box rule; released voices also stop drawing
   wind demand. Regression test `released_pipes_stop_drawing_wind`.
b. **Pool exhaustion steals a sounding tail with no fade** — ⚠ still true.
   `allocate_slot` fallback overwrites a Tail/FadeOut voice instantly (click
   under extreme load). GO drops the *new* note instead; HW sheds "least
   conspicuous" tails early (we do too — `TAIL_VOICE_BUDGET` — but the
   last-resort steal is a hard cut). Ceiling `MAX_VOICES = 2048` vs HW's 32k —
   fine in practice, note only.
c. ~~Velocity ignored.~~ ✅ **Fixed for gain** (`589691a`): GO's
   `MinVelocityVolume`/`MaxVelocityVolume` ramp, applied per voice including
   late-started voices (stop drawn mid-hold, recouple). Velocity-based *attack
   selection* waits on §4.
d. ~~Bass/Melody couplers skipped.~~ ✅ **Fixed** (`616c03f`): `CouplerType`
   parses to `CouplerScope::{AllKeys, Bass, Melody}`; only the lowest/highest
   held key in range couples, re-judged on every note on/off (a note-off can
   start a voice on the next-extreme key). Rides the flexible-route coupler
   model (`7ef91be`): a coupler = named routes, each with source manual, key
   range, `unison_off`, scope, optional target manual + shift; user-defined via
   sidecar `[[couplers.define]]`.
e. **Single audio device via cpal default** — ⚠ still true (named M6 deferral);
   GO drives multiple devices via RtAudio/PortAudio/JACK simultaneously.
f. ~~GO-format sets only.~~ ✅ **Fixed** (2026-09-02): unencrypted Hauptwerk
   definitions load through `aristide-formats::hauptwerk` (`docs/hw-odf-notes.md`);
   encrypted sets are refused at the XML sniff and the first sample's header.
   Residue: noise ranks, second-layer tremmed samples and temperament files are
   not read yet (notes §11).
g. ~~GO synth trem ramps ignored.~~ ✅ **Fixed** (`b1bd337`): ODF
   `StartRate`/`StopRate` (each a `1/rate`-second ramp in GO) map onto
   `ramp_seconds` as their average; sidecar trems keep the 0.7 s default.
   Residue: one knob serves both directions where GO ramps asymmetrically.
h. ~~`--record` header lies on widened streams.~~ ✅ **Fixed** (2026-08-26):
   taps carry their channel count; the first tap writes the header, and a
   mid-run channel change closes the file and continues in a numbered
   segment (`spawn_recorder`).

---

## Where Aristide is already ahead (context, don't "fix")

Re-verified 2026-08-26 — all still accurate, list grown:

- Resampler: 16-tap Kaiser β=9, 512 interpolated phases, ~90 dB SNR, live
  per-voice rate — vs GO's 8-tap Lanczos, 8192 uninterpolated phases, rate
  frozen at note start. Now also *ramped* rate (M6 glides) — GO can't bend a
  sounding pipe at all.
- Release model: quadrature phase alignment + level match + staccato
  room-charge + repitch decay compensation + release pitch-bend — beyond GO's
  2×32 amplitude/slope LUT and plausibly beyond HW.
- Swell boxes: gain + sliding high-shelf + shutter inertia + frozen tails —
  beats GO's pure gain taper, approaches HW's shutter filters.
- Wind model exists at all (GO: none) and the trem does FM+AM+brightness
  (GO synth trem: block-rate AM only).
- Contemporary layer (M6): Scala per-manual tuning with nearest-pipe
  re-anchoring, live tuning drift on held voices, MPE per-note pitch, Lumatone
  `.ltn` maps, per-pipe onset delays, bus delays — none of which GO or HW have.
- RT-clean callback (GO waits on condvars/mutexes inside the audio callback).
- Partitioned convolution reverb with clean bit-exact-dry bypass.
- Multi-loop random selection ≈ GO parity (`PickEndSegment` equivalent).
- Master limiter (GO hard-clamps at ±1.0).

## Worker split suggestion (2026-08-26 revision)

| Work package | Sections | Independent? |
|---|---|---|
| ~~Tremulants + multi-attack~~ | §2 + §4 | ✅ landed 2026-08-26 (residue: mid-hold wave switch, trem UI) |
| ~~Memory (i16 + load cache + parallel decode)~~ | §3 | ✅ landed 2026-08-26 (residue: streaming, per-rank load options) |
| Release ODF keys + truncation | §8 | yes (small) |
| Loop crossfade baking | §9 | yes (small) |
| Voicing sidecar (gain/cents/brightness) + combination action | §7 | control-side only |
| ODF `AudioGroup` → buses; multi-device; record-header fix | §5 residue + §12h | yes |
| Pitch residue (`IgnorePitch`/`AcceptsRetuning`, temperament table) | §6 residue | yes (small) |
| Correctness nits | §12b | small, anytime |

Status ledger: §1 ✅, §2 ✅ (residue), §3 ✅ (residue: streaming, load
options), §4 ✅ (residue), §5 ✅ (residue), §6 ✅ (residue), §7 ✅ core
(residue: divisionals/sequencer/crescendo, pipe-scope voicing, UI),
§8 ✅ (residue: truncation), §9 ✅, §12a/c/d/g/h ✅;
§10, §11, §12b/e/f ⚠ open. What remains is residue and the HW-only
fidelity gaps — no whole package blocks real use any more.
