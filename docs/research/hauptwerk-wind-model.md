# Hauptwerk's wind supply model & engine — documented behavior

Research notes gathered 2026-08-09 (public sources only; agent-assisted,
conclusions reviewed). **[V]** = vendor documentation, **[Dev]** = Martin
Dyde (HW's developer) on the official forum, **[C]** = community. Key
sources: HW5/HW9 User Guides + HW9 cumulative Release Notice + CODM guides
(downloadhauptwerk.com PDFs; HW4 CODM via web.archive.org), the OdfEdit and
HWtoGO reverse-engineered ODF dictionaries (GitHub), forum threads t=19989
(Wayback), t=21868.

## 1. Architecture (lumped-element network) [V]

- **WindCompartment** = any air volume (reservoir, windchest, swell box,
  atmosphere): volume m³, default pressure (inches WC), optional bellows =
  weighted/sprung moving board with gravity mass, inertia mass, damping
  coefficient, nonlinear opening/closing springs — a 2nd-order mechanical
  oscillator per regulator.
- **WindCompartmentLinkage** = trunk/valve between compartments: mass flow
  (kg/s) at a reference Δp (orifice law calibrated by one point), valve
  control (blower switches), flow-randomisation block.
- **Per-pipe** (`Pipe_SoundEngine01`): source AND output compartments
  (pipes exhaust into the swell box — that's how box pressurization
  works), own kg/s @ Δp draw, velocity-dependent flow onset ramp,
  per-pipe flow randomisation.
- **Response curves per pipe layer**: flow→amplitude (two-threshold ramp:
  % of reference flow where pipe begins to sound / reaches max — this
  implements blower-off die-away and starvation cutout); flow→pitch
  (calibration point: pitch drop in % of a semitone at a given % of
  reference flow, lockable, invertible); flow→brightness (3rd+ harmonic
  shelf, a calibration point in dB at a flow %). Each with per-pipe depth
  trims, user-voicable depth/polarity/stereo balance.
- Model runs on a fixed background tick decoupled from audio; step size is
  an ODF attribute (`AudioEngine_WindFineIterFreqNanoseconds`!) and a user
  quality setting (v7: lower/medium/higher model-quality modes).
- All stochastic elements (flow noise, trem rate/depth noise) are damped
  random processes (accelerating coeff + damping coeff + probability fn +
  max %), never white noise.

## 2. CODM (simplified format) numbers [V — HW4 CODM guide]

- Per division: one parallel-rise **weighted** reservoir + one windchest,
  regulated to 6" WC static; **70/30 volume split** reservoir/chest;
  "about 1 m³ for a medium division".
- Reservoir oscillation frequency **f ≈ 4.1333·√(table area m²) Hz**
  (∝ √area). Damping coefficient "5–10 is a good starting point".
- `WindchestPressureDropPctAtMaxLoad`: **1–10 % is the recommended
  range** — % of the 6" lost when ALL attached pipework sounds.
- **Pitch calibration: "a typical diapason falls ~3.3 % of a semitone
  when wind pressure falls ~6.3 %"** → ≈ 0.52 cents per 1 % pressure —
  agrees with Pykett's measured 0.65 cents/% (organ-wind-acoustics.md §3).
  Default rank response: 3.3 % of a semitone per 3.2 % flow deficit,
  linear, adjustable ±, invertible.
- Rank flow randomisation 1–5 % ("reeds fluctuate more than diapasons").
- Swell enclosure has its own wind model: box pressure rises 1–5 % of
  static when all enclosed pipes sound closed → "just discernible
  detuning"; shutter inertia modeled.
- Tremulant: `FrequencyHz`, waveform-sample-driven (see §4), rate and
  depth continuously randomized (`MaxFrequencyRandomisationPct`,
  `MaxDepthRandomisationPct`, e.g. 10 → 90–110 %).

## 3. What it audibly does — the designer's own words [Dev, t=19989]

- "Usually organs' wind supplies (especially 'modern' ones) mainly impart
  **'wobble' to pipes as pipes start or stop sounding**."
- Depth adjustments "primarily affect the magnitudes of any
  temporary/dynamic 'wind wobbles' … **the wind model mainly affects
  dynamic behaviour (as pipes start/stop sounding), not the normal static
  sound of sustained pipes**."
- Held-note interaction ("open a second key and the first pipe wobbles")
  is "the main effect of a real organ's wind supply, and of Hauptwerk's."
- Community heuristics [C]: big volume → slower/deeper character; big
  table area → faster oscillation; high damping → single overshoot; low
  damping → tremulant-like multi-bounce; high pressure-drop % → detune
  under high demand.

**This is the design spec in one line: the wind model is a
note-transition wobble generator with small sustained sag, not a
pitch-bend pedal.**

## 4. Tremulant model [V]

- Driven by **recorded "tremulant waveform" samples** (looped WAV LFOs,
  several per rank across the compass): one file for pitch+fundamental
  amplitude, one for **third-harmonic amplitude** (drives the same
  per-pipe harmonic-shaping filter as the wind model). LFO rate stored in
  the WAV smpl chunk ×128. All pipes on a tremulant stay phase-locked;
  trems start/stop smoothly while notes sound (spin-up/down rates).
- Rate + depth continuously randomized via damped random processes.
- Alternate path: switch to genuinely tremmed recorded ranks.

## 5. Engine facts worth knowing [V]

- Up to 3 real-time filters per pipe (harmonic-shaping / swell / EQ),
  each ≈ 25–30 % polyphony; brightness modulation of deep bass pipes was
  disabled in v6.0.2 (distortion) and re-enabled by v7's "higher" quality.
- Interpolation is mandatory since v6 (wind/trem pitch modulation needs
  it); v6 added "higher-definition pitch shifting" (+50 % cost) and a
  96 kHz engine mode; v7 added high-quality filter processing (audibly
  better trems/wind); v8 removed the middle mode, wind CPU "reduced
  marginally, functionally unchanged"; v9 re-defaults quality up.
- **Known bug [Dev-confirmed, t=21868]: v5.0–v9.0 invert the wind model's
  PITCH polarity** (chords make held notes rise instead of dip);
  amplitude/brightness legs correct; fix pending after 9.0.1.
  (Comforting: nobody's ear caught it for six years — the wobble shape
  matters more than its sign.)
- Wind demand counts ALL perspectives of a stop even when not loaded
  [Dev]; v7 added a user "pipe flow adjustment" to globally scale draws.
- CODM wind systems are per-division islands; full format allows free
  topology (St. Anne's feeds Great + Pedal from one regulator [Dev]).

## 6. Calibration corrections for Aristide (vs wind v2)

1. **Pitch sensitivity is ~0.5–0.65 cents per 1 % pressure** (HW + Pykett
   agree). v2 used ~6 cents/% (P^0.35) with unrealistically small
   pressure drops — right output, wrong internals. Fix: realistic
   pressure drops (1–10 % chest drop at full load, Fraunhofer measured
   up to 15–30 % on historic instruments) × physical sensitivity
   (exponent ≈ 0.03, i.e. cents ≈ 0.55 × pressure-%). This matters
   because the tremulant will ride the same pressure pipeline with
   ±20–40 % swings.
2. **Amplitude: Fletcher's 15 dB/decade → gain ∝ P^0.75** (v2's 0.8 was
   accidentally right).
3. **Emphasis: transient wobble at note on/off** (Dyde), not steady sag:
   deeper pallet-gulp demand transients, modest steady component.
4. Per-pipe flow randomisation (1–5 %, reeds more) as a damped random
   process — the "alive" layer GO's rand() detune fakes at note-on only.
5. Reservoir frequency from geometry: f ≈ 4.13·√area — sidecar could
   accept physical descriptions later.
