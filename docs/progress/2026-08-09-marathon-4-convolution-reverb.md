# 2026-08-09 — marathon 4/N: convolution reverb (UPOLS)

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
