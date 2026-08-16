# 2026-08-11 — step back: the coupled-tutti spike + the octave-ghost bells

User: still crackles/pops on big coupled registrations, and releases
still bell-like; GO never crackles. Took the demanded step back, built
his EXACT scenario as a stress test (all Great+Swell stops, Swell→Great
at 8' and 16' = ~241 voices/chord) and measured: average 38–40 % of
realtime but **worst block 5.10 ms of the 5.33 ms budget** — the pops
are deadline breaches at chord transitions. Causes found and fixed:

- **O(N·M) command handling**: every StartVoice scanned 2048 slots for
  a free one; every StopVoice scanned all voices for its handle — a
  241-voice chord ran ~½ million scans in one block. Now: free-slot
  stack (O(1) allocation, invariant-maintained at every Idle
  transition) and StopVoice batching (sorted batch, ONE pool pass).
- **Crossfade storm**: a mass release doubles every voice's read cost
  for 30 ms. The outgoing (dying) leg now uses linear interpolation —
  its error fades to zero with it — and the pallet stagger widens
  adaptively with release-batch size (real tuttis spread too). Mass
  release worst block: 2.38 → **1.22 ms**.
- **AVX2**: mono kernel kept (2×8-wide FMA); stereo AVX2 measured ~10 %
  SLOWER than SSE2 on this host (shuffle overhead) and is parked
  behind a dead-code flag.
- **The bells, root-caused**: alignment used correlation *argmax*; on
  principal pipes whose 2nd harmonic rivals the fundamental, a
  half-period-off splice can win — fundamental cancels, octave
  reinforces: a missing-fundamental strike IS a bell. Replaced with
  **quadrature phase projection** at the measured fundamental
  (harmonic-immune, exact, cheaper), for embedded tails and separate
  releases both. New regression: strong-2nd-harmonic pipe stays
  fundamental-locked (the old argmax path had no such guarantee).
  A sign-convention bug was caught by the mistuned-pipe test — the
  phase-0-anchored tests couldn't see it.

87 tests green. If pops persist on the user's machine (check the RT
priority log line!), next tier: block-render/SoA refactor.
