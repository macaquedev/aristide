# 2026-08-09 — marathon 3/N: native GUI (aristide-gui v1)

`aristide-gui` is now a real eframe/egui 0.36 desktop app: stops as
toggle pills grouped by manual, couplers, tremulant, gain slider
(drag-safe against polling), full tuning panel (temperament dropdown,
a′ drag, transpose). All I/O on a dedicated network thread (ureq)
talking to the server's local HTTP API at 4 Hz with a command channel —
the UI thread never blocks; server death shows a banner and recovers.
Protocol layer (state JSON, command→query mapping) unit-tested; the
first *visual* run necessarily happens on the user's machine (this box
is headless) — v1 kept deliberately conservative for that reason.
Run: `cargo run --release -p aristide-gui` (optional arg: server URL).
