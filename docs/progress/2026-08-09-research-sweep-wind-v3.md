# 2026-08-09 — research sweep + wind v3 recalibration

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
