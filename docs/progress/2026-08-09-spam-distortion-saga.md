# 2026-08-09 — the spam-distortion saga: measured, found, fixed

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
