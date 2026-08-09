# State of the art in sampler/VPO rendering

Research notes gathered 2026-08-09 (web survey; agent-assisted, conclusions
reviewed). Scope: techniques beyond GrandOrgue's baseline. Vendor marketing
flagged **[vendor claim]**. GO = GrandOrgue, HW = Hauptwerk.

Note on sourcing: hauptwerk.com and forum.hauptwerk.com blocked direct
fetching, so HW engine-internals claims rest on their published PDFs
(feature datasheets, Custom Organ Design Module guide), Wikipedia, and
forum excerpts surfaced via search — vendor-documented behavior, not
verified code.

---

## 1. Release handling

### 1.1 Multiple releases by key-press duration
- **HW (since 2006) selects among multiple release samples per pipe by hold
  duration** ("multi-release sampling") — staccato vs sustained notes excite
  a reverberant room differently. Producers ship 2–4 releases per pipe
  (staccato ~<250 ms, portato, sustained). Sources: Wikipedia: Hauptwerk;
  Inspired Acoustics Esztergom product page; forum.hauptwerk.com t=20768.
- **GO ODF**: `Pipe999ReleaseNNN` + `MaxKeyPressTime` (ms, −1 = ∞); engine
  picks the release bracketing the hold time. Attacks support
  `AttackVelocity` and wind-linked selection. Sources: magle.dk thread
  20991; GO discussion #781.
- **sfz**: `trigger=release` regions; duration handled *continuously*:
  `rt_decay` = dB attenuation per second held (0–200 dB/s), multi-segment
  via `rt_decayN`/`rt_decayN_time`. Worth stealing as continuous gain
  matching layered on discrete selection. Source: sfzformat.com opcodes.
- **Kontakt**: release-trigger groups, script-driven, gain-matched
  one-shots — no phase alignment. Phase-aligned releases are essentially
  organ-engine-specific. Sources: VI-Control 42811; piano.community.

### 1.2 Release truncation
- HW per-rank load option: truncate releases (e.g. "simulated dry, 250 ms"
  with shaped fade) — the standard prep for convolution reverb, and the
  single biggest polyphony lever (HW's own math: 400 sustained voices from
  release tails alone in a moderate texture). HW5+ can truncate tails in
  real time per output perspective. Sources: HW Technical Datasheet PDF
  (organovirtuale.com mirror); Inspired Acoustics HW-V reverb guide;
  forum.hauptwerk.com t=20300.
- HW's polyphony limiter **fades out the most inconspicuous release tails
  first** as the cap approaches — releases are the designated voice-steal
  victims. Source: HW Technical Datasheet.

### 1.3 Phase-aligned release crossfading
- HW **[vendor claim]**: releases auto phase-aligned + level-matched at
  key-off. No algorithm published. Sources: HW4 features datasheet
  (leyman.net mirror); hauptwerk.com/features.
- GO: `GOSoundReleaseAlignTable` — (amplitude, derivative) 2-sample lookup;
  see docs/go-critique.md §3 for why that's weak.
- **Best public treatment — Nick Appleton (2014), appletonaudio.com,
  "Release alignment in sampled pipe organs part 1"**: valid splice
  instants are t = t_r + T·n; cross-correlate a ~1024-sample release-head
  chunk against a long sustain window; positive maxima = phase-aligned
  entry points; combine with pitch estimate to map *any* sustain offset →
  release offset (precomputable). **Raised-cosine fades beat linear.**
  (Part 2 never published.) Aristide's fundamental-period correlation
  approach is this, independently arrived at; the offset-map formulation
  and raised-cosine fade are the upgrades to take.
- GO ODF: `ReleaseCrossfadeLength` per release; `LoopCrossfadeLength`
  0–120 ms. Sources: mps-orgelseite.de thread 1421; magle.dk 20991.

## 2. Loop techniques

### 2.1 Automatic loop finding — LoopAuditioneer (loopauditioneer.sourceforge.io)
- Sustain-section detection from both ends; candidate points gated by a
  **derivative threshold**; scored by **windowed cross-correlation** of
  loop start vs end (±2-sample windows, "quality factor" = allowed total
  difference); constraints: min loop length, **min distance between loop
  starts** (spreads loops), optional brute-force; **returns up to 8 loops**
  (format supports 16) ranked by quality. Release cue auto-placed at
  **lowest post-sustain RMS**. Crossfading offered only as remediation.
- Simpler framing: loop length ≈ integer fundamental periods, maximize
  boundary correlation (dsprelated thread 15880).

### 2.2 Butt loops vs crossfaded loops — Appleton 2019
- Crossfading sums the *power* (not amplitude) of the uncorrelated noise
  component → up to **3 dB noise dip mid-fade**, audible as periodic
  breathing. "If you can avoid cross-fading — avoid it; else milliseconds."
  Organ practice: correlation-found butt loops. GO bakes raised-cosine loop
  crossfades at load when asked (GOSoundAudioSection.cpp).

### 2.3 Multi-loop selection
- GO picks the **next loop end uniformly at random per pass**
  (`PickEndSegment`), decorrelating loop repetition between passes and
  between identical pipes. Constants: `SHORT_LOOP_LENGTH = 256`,
  `REMAINING_AFTER_CROSSFADE = 256`. HW ships "load only first loop" as a
  memory saver; multi-loop playback is a listed polyphony cost.

## 3. Tremulants on untremmed samples

- **HW's "modeled tremulants"** [vendor documentation]: recorded
  **tremulant-waveform samples** at intervals across each rank define the
  modulation of **amplitude, pitch, AND harmonic content** per pipe,
  derived from analysis of tremmed vs untremmed recordings; all pipes
  share one tremulant phase. Harmonic leg runs through HW's per-voice
  "harmonic shaping" filters (−30 % polyphony when enabled). Sources: HW
  Custom Organ Design Module guide PDF; HW6 Features Data Book PDF;
  HW Technical Datasheet.
- **Spectrogram finding (savirtualorgans Tremulant Discussion)**: in real
  tremmed pipes **each harmonic has its own AM depth and phase** — peaks
  don't coincide across harmonics, though all share the rate. Synchronized
  AM+FM (GO/SoundFont style) misses this.
- **Organteq** (modartt.com): tremulant = pressure modulation into the
  physical model; sample-engine tremulants dismissed as "a post effect
  filter". **Architecture consequence for Aristide: tremulant = periodic
  pressure disturbance into the existing wind model** (shared phase per
  chest), letting pressure→pitch/amplitude coupling do AM/FM, plus a
  pressure-tracked per-voice brightness tilt, plus de-synchronized
  per-band AM for the harmonic realism layer.
- Offline analysis route: DAFx AM/FM separation (Wells, DAFx-10) can fit
  per-harmonic modulation envelopes from a producer's tremmed samples —
  the open-source path to HW-style analyzed tremulant waveforms.
- GO fallback: synthesized sine "tremulant sample" at `1000/period` Hz
  (GOSoundProviderSynthedTrem.cpp) — the floor, not the target.

## 4. Interpolation in shipping samplers

- Kontakt: Standard = 4-point Lagrange; High/Perfect = windowed sinc
  (KVR t=558389; VI-Control 92843).
- KVR sweep-test comparison (t=601773): Kontakt Perfect / HALion Extreme
  clean; ReaSamplomatic 64-point sinc <1 ms latency; sfizz default
  polynomial aliases but 8× oversample + 72-tap sinc excellent.
  **~64-tap sinc = "transparent" tier; Aristide's 16-tap Kaiser polyphase
  already beats most shipping samplers; 4-point polynomial is the floor.**
- HW: never resamples engine-rate (refuses sets at unsupported device
  rates!); per-voice interpolation unspecified, can be disabled for 2–3×
  polyphony (HW Technical Datasheet).
- Design references: Niemitalo deip.pdf (optimal short interpolators on
  2× oversampled tables — argues cheap 4–6 point on pre-oversampled data
  can beat long sinc per voice); de Soras resampler-en.pdf (MIP-map +
  polynomial hybrid).

## 5. Convolution reverb & perspectives

- HW5+: IR per mixer **bus**; surround rigs = one IR per perspective bus;
  wet-mix % per IR + global scalar; IR truncation control. Standard
  workflow: wet set truncated to 250 ms + IR. Sources: Inspired Acoustics
  guide; vpoinstitute.org HW4/5 IR guide; hauptwerk.com/features.
- GO: zita-convolver (partitioned, non-uniform blocks, zero latency), one
  global reverb; **Dirac spike at t=0 added to the IR** so dry passes
  through the same convolver — elegant wet/dry trick (GO discussion #625).
- **Multi-perspective sets are the real SOTA**: Sonus Paradisi 8-channel /
  4-perspective (front-direct, front-diffuse, rear, …); users blend
  direct↔diffuse live; memory ×2 per perspective doubling; HW supports up
  to 4 perspectives per pipe with independent voicing/truncation.
  **Aristide architecture: perspective-multiplexed voices → per-perspective
  buses, each with gain + optional IR** — covers surround sets and dry+IR
  in one mechanism. Sources: sonusparadisi.cz blog posts (dry-wet,
  6-channel how-to, mixing perspectives); forum.hauptwerk.com t=20300.

## 6. Voice management at scale

- HW numbers: 500+ (dry) → 2000+ (wet large) → 4000+ (cathedral);
  ~(cores−1) × 1200–2500 voices (2011 CPUs); all-RAM, explicitly rejects
  disk streaming at this polyphony; NUMA-aware; graceful release-shedding
  near the cap. Feature costs: true stereo −20 %, swell filters −30 %,
  harmonic-shaping −30 %, interpolation off = 2–3×. Source: HW Technical
  Datasheet. **Memory bandwidth, not FLOPs, limits polyphony** (both
  engines).
- GO: scheduler threads ≈ cores, `MAX_FRAME_SIZE = 2048`, optional
  delta-compressed 8/16-bit cache (predictor `prev + (prev−last)/2`).
- SIMD consensus (KVR t=586455): for identical per-voice chains,
  **horizontal SoA (4/8 voices per SIMD lane) wins for filters/gain/mix;
  resampling stays per-voice vertical** (divergent positions), then
  horizontal for the rest; mask-handle loop wraps; thread at
  windchest/bus granularity.

## Highest-leverage items for Aristide

1. Precomputed sustain-offset→release-offset map per release + raised-
   cosine splice fade; multi-release selection by hold time with
   rt_decay-style continuous gain matching.
2. Built-in loop finder (derivative-gated candidates, windowed correlation
   scoring, keep several loops) + random loop-end selection at runtime;
   avoid baked crossfades (3 dB noise dip).
3. Tremulant as wind-pressure modulation (shared phase per chest) +
   pressure-tracked brightness + de-synchronized per-band AM.
4. Perspective buses with per-bus gain + partitioned convolution and the
   Dirac-spike dry path; 250 ms truncation mode for the dry/IR workflow.
5. Horizontal SoA SIMD for post-resampler voice DSP; release tails as
   first-class voice-steal victims.
