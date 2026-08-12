# Release realism: acoustics, prior art, and Aristide's model

Research pass 2026-08-11 (three literature sweeps: organ decay acoustics,
Hauptwerk/GrandOrgue release engineering, general sampler release DSP),
plus numerical measurement of the engine against the GO demo set.

## What physically happens at key release

1. **Drive collapse (~10–50 ms):** pallet closes, foot pressure drains;
   flue pitch flattens slightly as blowing pressure falls (Viscount
   patent US7442869 models this explicitly).
2. **Passive ring-down:** each mode decays exponentially at its own rate
   (τ = 2Q/ω). Measured on a narrow labial pipe (Rucz 2015, PhD thesis
   BME, fig. 2.5b): ~10–15 period plateau, then straight-line dB decay;
   partial decay rates ≈ 0.5 / 0.9 / 1.2 / 1.7 / ~4 dB per period for
   H1…H5 — highs die first, fundamental last, total ring 20–150 periods
   (middle-C principal: top partials gone in ~100 ms, fundamental
   ~400–600 ms). Release is NOT a reversed attack (no H2 overshoot).
3. **Room takes over:** past the critical distance the listener mostly
   hears room decay — seconds of it (RT60 2–4 s church, 8–11 s
   cathedral). The room's decay RATE is fixed by the room; a short note
   leaves a *quieter* tail, never a faster-decaying one.

**Why bad releases read as "bells"** (Fletcher & Rossing ch. 21): a bell
is sparse partials in free exponential ring with the lowest partial
lasting tens of seconds. A synthetic release whose tail rings too long,
too slowly, or at a key-dependent wrong rate reproduces that signature.

## What HW/GO actually do

- **Multi-release capture** (short ~50–100 ms, medium ~200–400 ms, full)
  selected by held time — the only correct fix for "reverb not yet
  developed" (HW datasheets; GO `MaxKeyPressTime`, tightest-fit rule).
- **Phase-aligned splice:** GO's 2×32 (amplitude, derivative-sign) LUT
  over the release's first 50 ms — HW 1.x lineage. Aristide instead uses
  quadrature phase projection at the measured fundamental (finer than a
  64-cell LUT; immune to strong-2nd-harmonic octave errors).
- **Pitch-dependent crossfade:** GO defaults 184 ms (MIDI ≤42) → 6 ms
  (≥86); a fixed length either smears treble speech-off transients into
  an "artificial fade" (SourceForge FR #58: it ate reed "parp") or
  clicks in the bass. ≈ 9 fundamental periods.
- **Level match at the splice** (HW datasheet), GO mid-attack scale
  0.2 + 0.8(2a − a²).
- **GO's staccato model decays the tail RATE** (fade 200 ms–6 s for
  notes shorter than a 100–350 ms build-up threshold). This contradicts
  room physics (rate is invariant, level is what builds) and is audible
  as plucked/harp-like short notes — HW's own docs warn single-release
  playback sounds "harp-like" on short notes.

## Aristide's model (as of this session)

0. **Staccato tail (2-part):** full matched level through speech-off +
   early reflections (~150 ms deficit decay), settling to the room-
   charge level, plus (1−charge)·25 dB/s extra decay — an undeveloped
   diffuse field is quieter AND shorter. λ fits stop 45 dB below tail
   peak (noise floors flattened treble measurements).
1. **Splice:** quadrature phase alignment at the measured fundamental,
   raised-cosine (smoothstep) amplitude-preserving crossfade — correct
   for correlated material (Signalsmith crossfade analysis) — with
   **per-voice length ≈ 9 fundamental periods, clamped 6–184 ms, and
   additionally capped by note age** (a mid-attack release must not keep
   swelling; drive collapses at pallet close).
2. **Level:** envelope-follower match against the tail's reference
   level (floor 0.2, cap 1.1), unchanged.
3. **Staccato = level, not rate:** notes shorter than the 100–350 ms
   room build-up get tail_gain × (1 − e^(−age/(0.5·T_build))), floor
   0.1; the decay RATE stays the recording's. Replaces the GO port.
4. **Repitch decay compensation (new, not in GO or HW):** each sample's
   tail decay rate λ (dB/s) is measured at load (least-squares on 50 ms
   RMS windows, skipping the 150 ms drive-collapse plateau). A pipe
   played at repitch R hears its recorded room decay R× too fast/slow —
   on the GO demo set (2 recorded pipes per octave, repitches to ±600
   cents) this made every key ring at a different wrong speed:
   down-repitched keys rang up to 41 % too long (the literal bell
   signature), up-repitched keys were plucks. release() now applies a
   per-frame gain factor of λ(R−1) dB/s (clamped ±15 dB/s) so ring time
   is key-invariant. Native-pitch samples (real per-pipe sets) are
   untouched.
5. **EOF guard:** tails fade over their final ~46 ms instead of hard-
   cutting, so compensation (or hot-ended files) can never click at end
   of material.

Regression: `repitched_release_rings_at_native_decay_rate` (the +600-cent
demo pipe must decay within 10 dB/s of the recording's measured rate),
plus `crackle_hunt` unchanged at zero discontinuities.

## Not yet built (ranked from the research)

- Multi-release sets: selection exists (`MaxKeyPressTime`), but decay
  compensation only covers the embedded tail — extend λ measurement to
  separate release samples.
- ~~Release pitch sag~~ BUILT (a404213): pitch bends down at note-off,
  12·sqrt(f0/100) cents clamped 3–38, tau ≈ 12 periods (15–80 ms) —
  the bell/pipe discriminator (bells don't bend). Full per-partial
  detune toward inharmonic resonances (F&R §16.6, Aeolus) remains
  future work.
- Frequency-dependent tail shaping for dry conversion (HW truncation
  shapes decays per pipe frequency; RT60 falls with frequency).
- Per-partial release (additive) — highest ceiling, order-of-magnitude
  more work, still doesn't solve the reverberant tail.

## Sources

- Rucz, "Innovative methods for the sound design of organ pipes", PhD
  thesis, BME 2015 — §2.2.2, fig. 2.5 (the only quantitative release
  measurement found). http://www.hit.bme.hu/~rucz/pub/Rucz_-_PhD_Thesis.pdf
- Fletcher & Rossing, *The Physics of Musical Instruments*, ch. 16/17/21.
- Angster, Llorca-Bofí, Miklós, "Matching pipe organs to room
  acoustics", Physics Today quick study.
- Viscount patent US7442869B2 (note-off pitch flattening).
- Aeolus (Adriaensen): per-stop release time + release detune.
- Hauptwerk User Guide v8 / CODM Guide v8 / HW4–HW6 datasheets
  (multi-release, phase-aligned level-matched splice, shaped
  frequency-dependent truncation preserving release transients).
- GrandOrgue source: GOSoundReleaseAlignTable.cpp (2×32 LUT),
  GOSoundProviderWave `get_fader_length` (184→6 ms), CreateReleaseSampler
  (mid-attack scale, staccato tail fade); SourceForge FR #58.
- Signalsmith Audio, "A cheap energy-preserving-ish crossfade" (fade
  shape vs correlation).
- sfzformat `rt_decay`/`rt_decayN`; SA VPO sampling guidelines
  (3-release capture); Inspired Acoustics (250 ms truncation guideline).
