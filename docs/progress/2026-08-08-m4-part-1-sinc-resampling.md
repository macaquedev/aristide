# 2026-08-08 — M4 part 1: sinc resampling + phase-aligned releases

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
