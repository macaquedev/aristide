# 2026-08-09 — release pop/ping fixes + web-UI tuning controls

Two user reports investigated with data:

- **Pop/crackle at key release**: introduced by marathon-1's multi-loop —
  the wrap jumped from loop A's end into loop B's start, an unvalidated
  splice (only end→own-start is author-guaranteed seamless). Now wraps
  always return to the current loop's own start; variety comes from
  choosing which loop's end to run toward next (all loops share one
  continuous recording, so that path is seamless — GO's scheme too).
  Regression: max frame-to-frame delta bounded in a two-loop fixture.
- **"Pinginess", worst on high pipes**: alignment correlation window was
  one period — ~30 frames on a 1.5 kHz pipe, far too little to lock
  phase against room noise, so high-pipe splices clicked and the click
  rang in the recorded reverb. Window now 2 periods clamped to ≥128
  frames (test: phase lock within 0.12 cycle on a noisy 30-frame-period
  pipe). Also: tail reference level window 2048→512 frames (fast
  high-frequency decay under-read the start level) and tail-gain boost
  cap 1.3→1.1 (never louder than the note being released).
- Tuning controls (temperament/a′/transpose) added to the web console —
  per the new standing rule: every feature ships in the testing UI.
  76 tests green (GUI crate builds separately).
