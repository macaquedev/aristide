# 2026-08-09 — brightness modulation + per-pipe flow noise

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
