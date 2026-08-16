# 2026-08-09 — couplers (console routing + web UI)

Couplers were parsed since M2 but never routed. Console now expands each
played key through engaged couplers (single-level: coupled notes don't
re-couple — the default organ behaviour; GO's opt-in propagation flags
can come later). Handles unison and octave shifts, self-couplers (16'),
out-of-compass drop, cycle pairs. Web console gets a Couplers section
(`/api/coupler?idx=N&on=0|1`). Sounding notes keep their coupling;
new presses use the new state. 68 tests green.
