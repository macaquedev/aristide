# 2026-08-09 — tremulant + web console

- **Tremulant**, physically routed: a pressure LFO on the wind group
  (research-calibrated: 6 Hz, ±22 % pressure ≈ ±12 cents FM through the
  pitch path, ~1 dB AM through the gain path — one modulation source,
  consistent AM/FM like a real trem valve). Engage/disengage ramps over
  ~0.7 s; rate and depth wander ±8 % as slow damped random walks
  (xorshift, RT-safe), because a metronomic trem sounds fake. Works on
  sag-disabled chests. Sidecar `[tremulant] rate_hz / depth_cents /
  chests`. Engine: SetTremulantParams / SetTremulant commands. Tests pin
  depth (±0.64 % rate factor) and rate (12 cycles in 2 s).
- **Web console** (temporary until M5's IPC + native GUI):
  `http://127.0.0.1:9669/` (`--http-port`), served by the server on a
  thread via tiny_http. Draw/retire stops live (retiring stops its
  sounding voices via tracked (stop, handle) pairs), tremulant toggle,
  master gain slider. Single embedded HTML page, no build step, no
  external assets. Endpoint smoke tests included. 63 tests green.
