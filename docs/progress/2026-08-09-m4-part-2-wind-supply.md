# 2026-08-09 — M4 part 2: wind supply model

The organ breathes. Per-windchest reservoir model in the RT engine
(`engine/wind.rs`): `dP/dt = (1−P)/τ − s·D·P` where D sums the wind
weight of sounding voices on the chest (weight ≈ √(150 Hz/f), capped —
big pipes drink more; noises draw nothing). Pressure maps to per-voice
playback rate (P^0.4) and gain (P^0.8): chords sag flat and soften
slightly, then the reservoir recovers (default 2 % sag at a
full-chorus demand of 30, τ = 120 ms; sidecar `[wind] sag_percent /
recovery_ms`, 0 disables). Attack dips fall out of the dynamics.

This is architecturally impossible in GrandOrgue (rate frozen at note
start — critique §2); it's the first feature where Aristide does
something GO *can't*.

- Voices carry (wind group ← ODF windchest, wind weight); model `Rank`
  now records its windchest.
- Proof test: 30 silent heavy voices + 1 measured sine on one chest —
  measured zero-crossing period sags 480 → ~484 frames (≈ −14 cents)
  at the calibrated steady state, and pressure settles at 0.980 ± 0.004.
  Cost: one integration step + two powf per chest per block. 58 tests.
