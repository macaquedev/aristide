# 2026-08-09 — wind model v2 after user feedback ("super slow and horrible")

v1's first-order reservoir glided pitch over ~120 ms — chorus-wide
portamento, not wind. Rebuilt as a **damped second-order regulator**
(ω = 2π·3.5 Hz, ζ = 0.5, semi-implicit Euler substepped for stability at
any block size): fast dip reached in ~70 ms, one springy bounce, settled
in ~250 ms, slight overshoot on release. Per-voice **pallet-opening
boost** (+0.8× weight decaying over 70 ms) makes single notes dip too.
Depth cut to a quarter: defaults now ≈ −3 cents steady full-chorus
(sidecar `[wind] sag_cents`, plus `bounce_hz` / `damping`). Tests assert
the dynamics: dip ≤ 120 ms, ~16 % undershoot (matches ζ=0.5 theory),
release overshoot, stability at 93 ms blocks. 60 tests green.
