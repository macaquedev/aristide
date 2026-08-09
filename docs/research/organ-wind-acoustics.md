# Pipe organ wind-system & pipe-speech dynamics: measured data

Research notes gathered 2026-08-09 (web survey; agent-assisted, conclusions
reviewed). Full citations at the end. This file is the empirical basis for
Aristide's wind model, tremulants, and attack behaviour.

## 1. Static pressures (context)

- Church fluework ~80 mm wg (~785 Pa); theatre organs ~250 mm wg (~2450 Pa)
  (Pykett 2009). Fraunhofer field measurements: working chest pressures
  ~600–900 Pa (Angster/Pitsch/Miklós ISMA 2007). Research values from
  160 Pa (Abel's miniature Schuke, ±6 Pa over 10 s) to 10 kPa sweeps
  (Fletcher 1976).

## 2. Wind system dynamics (Fraunhofer IBP, measured on real organs)

Source: Angster, Pitsch, Miklós, *Design of New Wind Systems for Pipe
Organs*, ISMA 2007; underlying: Pitsch PhD, Univ. Siegen 2005.

- **Bellows/reservoir oscillation: 3–10 Hz** (size/type dependent; wedge
  bellows most oscillation-prone). **Wind-trunk resonances: 10–20 Hz.**
  A faithful simulation carries BOTH oscillators plus the sag response.
- Measured chord transients: nominal ~600 Pa dips to **~400 Pa at chord
  onset** (−33 %); release overshoots to **~900 Pa** (+50 %) with decaying
  oscillation lasting ~1.5–2 s (~5–10 Hz visible). Another organ: ~900 Pa
  sags to ~650 Pa (−28 %) for the whole 4 s chord. Falkenhagen: ±100 Pa
  on/off oscillations settling in ~1 s (original) vs <0.3 s (with outlet
  valve). Plainveaux: 650→550 Pa (−15 %) sustained sag, eliminated by
  chest outlet valve.
- **Consequence: sag depth and wobble vary enormously between instruments
  (well-regulated: barely audible; historic/wedge-bellows: huge).** The
  sidecar knobs matter more than any single default.
- Walker Technical patent US5508472 (1996), digital-organ wind emulation:
  regulator as spring-mass, **natural frequency 2–5 Hz, damping 0.4–0.7**,
  wind draw per stop scaled **1.0 / 0.5 / 0.25 per octave (8'/4'/2')**,
  pitch response flutes 1.5× principals. (Aristide v2's 3.5 Hz / ζ 0.5
  regulator sits square in both the measured and patented ranges.)

## 3. Flue pipes vs pressure (measured)

- Pitch: Pykett measured f = 0.0829·p + 170.2 (Hz, mm wg) on a stopped
  flue — **≈0.65 cents per 1 % pressure change** in the working range.
  Flues tolerate ≈±1.4 % frequency (≈±24 cents) before over/underblowing.
  Kob 2003 saw 20-cent shifts from pressure/wall changes. Fletcher 1976:
  frequency rises monotonically with pressure within a regime; sensitivity
  grows near regime edges (no universal coefficient).
- Loudness: input power ∝ P^1.5 → up to ~15 dB per decade of pressure
  (Fletcher 1976). Efficiency ~1 % max.
- Attack: precursor/chiff strongest when foot-pressure rise time is
  **1–10 fundamental periods** (Nolle & Finch 1992). Steady speech reached
  at ~20–27 periods (~0.10–0.13 s on a 207 Hz principal; Otcenasek ISMA
  2019, jet 1→13.5–19.7 m/s over ~50–60 ms). Slow pressure buildup (slider
  chests) ↔ baroque chiff (Fletcher §XI).

## 4. Reed pipes (different!)

- Reed frequency rises ~linearly with pressure *for the reed itself*, but
  with resonator attached the slope is reed-controlled; resonator adds
  forbidden bands/jumps near its eigenfrequencies (Miklós et al. JASA
  2003, 2006). Trumpet = strong reed–resonator coupling; Vox humana =
  weak. Practically: **under pressure modulation reeds respond mostly in
  amplitude/timbre, flues mostly in pitch** — why they drift apart under
  tremulant. (Quantitative flue/reed split: unmeasured in literature —
  flagged gap.)

## 5. Tremulant (measured)

- Pressure swing: peak-to-peak up to **~75 % of static** (Pykett: ±30 mm
  on 80 mm church; ±100 mm on 250 mm theatre).
- FM: **~±1 % (±15–17 cents) typical; ±1.4 % (±24 cents) physical ceiling**
  before flues drop out. AM: 1 dB is the perception floor; deep trems
  nearly silence pipes at minima. Beat-to-beat irregularity is
  characteristic.
- Rate: **~6 Hz representative** (Pykett; Allen guidance 6–7 Hz).
- All partials share the rate; absolute Hz deviation scales with harmonic
  number (equal cents); **each harmonic's AM envelope has its own depth
  and phase** (savirtualorgans spectrograms).
- Organteq models tremulant as upstream pressure modulation: ~1 dB AM,
  ±15 cents FM — matching Pykett.
- Room reverb converts FM into perceived effect; dry rooms weaken it.
- → Aristide: tremulant = pressure LFO into the wind model at ~6 Hz with
  irregularity, moderate default ±10–15 cents on affected chest, plus
  pressure-tracked brightness and per-band AM phase spread (see
  vpo-rendering-techniques.md §3).

## 6. Pipe–pipe interaction (shared wind, proximity)

- Abel et al. 2006 (JASA/arXiv): two pipes on one chest lock within
  ~±20 cents detuning; locked at Δf=0 the fundamentals radiate in
  ANTIPHASE (−20 dB at the centerline!). Coupling dies by ~25–100 mm
  separation. Closing one pipe's valve raises the chest pressure and the
  other pipe's frequency — direct shared-wind coupling evidence.
- Unison ranks/celestes and en-chamade pairs are where this matters;
  a future "ensemble" model could lock near-unison voices and phase-spread
  them rather than summing identically.

## 7. Modeling directives distilled

1. Wind = **regulator (2nd order, 2–5 Hz, ζ 0.4–0.7)** ✓ implemented +
   optionally a **trunk resonance (10–20 Hz, high Q, small amplitude)**
   excited by demand steps — the "judder" layer.
2. Onset dips are 2–4× deeper than the sustained sag (600→400 dip vs
   −15/−28 % sustained): the pallet-gulp boost should be stronger and
   shorter than v2's (attack demand ×2 over ~40–60 ms).
3. Release overshoot is real and big (+50 % pressure was measured); ours
   should remain audible but default well below that.
4. Wind draw: scale per octave ≈ ×0.5 (Walker patent) → weight ∝ 1/f is
   better-supported than the current √(1/f). Flute-family pipes ~1.5×
   pitch response of principals; reeds ~0 pitch / mostly AM (needs rank
   classification — future voicing metadata).
5. Pressure→pitch: ≈0.65 cents/% for flues → a −15 % sag ≈ −10 cents;
   map exponents accordingly (P^kp with kp ≈ 0.35 gives −5.6 cents at
   −15 %: conservative by ~2×; defensible for a default, sidecar to taste).
6. Tremulant: 6 Hz pressure LFO, per-chest, FM target ±10–15 cents,
   with slight cycle-to-cycle irregularity.

## Sources

- Angster, Pitsch, Miklós — Design of New Wind Systems for Pipe Organs,
  ISMA 2007 (Fraunhofer Publica PDF).
- Angster, Rucz, Miklós — 25 Years Applied Pipe Organ Research at
  Fraunhofer IBP, ISMA 2019 (pub.dega-akustik.de/ISMA2019/000016).
- Otcenasek, Dlask, Otcenasek — ISMA 2019 (000031): jet/attack PIV.
- Fletcher — Sound production by organ flue pipes, JASA 60(4), 1976.
- Nolle & Finch — Starting transients of flue organ pipes…, JASA 91(4),
  1992. DOI 10.1121/1.403653.
- Miklós, Angster, Pitsch, Rossing — Reed vibration in lingual organ
  pipes…, JASA 113(2), 2003; Interaction of reed and resonator…, JASA
  119(5), 2006.
- Abel, Bergweiler, Gerhard-Multhaupt — Synchronization of organ pipes,
  JASA 119(4), 2006; arXiv:physics/0506094.
- Kob — Pitch and level changes in organ pipes due to wall resonances,
  J. Sound & Vibration, 2003.
- Pykett — Tremulant Simulation in Digital Organs (2009),
  colinpykett.org.uk/digitaltrems.htm; The Interaction of Tremulants with
  Room Acoustics, tremsandrooms.htm.
- Walker Technical — US Patent 5,508,472 (1996), wind supply emulation.
- Toevs — Organ Acoustics at High Altitudes, The Diapason.
- Modartt — Organteq physical modeling page.
