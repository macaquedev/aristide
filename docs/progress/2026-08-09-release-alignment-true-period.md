# 2026-08-09 — release alignment: measure the true period (bell fix, part 2)

User: still slightly bell-like, "doesn't sound sampled". Investigated
the actual demo files (`tailinfo` example): the recorded releases are
real and long (cue sits 1–4 s after the loop end at full sustain level;
the decay after the cue IS the sampled release — same data GO plays).
The defect was ours: the alignment table tracked phase as
(position − loop_start)/period with **period derived from nominal
12-EDO pitch**. Real pipes sit cents off nominal; across the hundreds
of periods between anchor and cursor that error wraps the phase
multiple times — the splice landed at effectively random phase, and a
random-phase splice through a crossfade is exactly the hollow "bell"
he heard. Synthetic tests had exact periods, so they passed.

Fix: `Sample::refine_period` — long-lag normalized autocorrelation over
the sustain loop (up to 24 nominal periods of lag, parabolic peak
refinement → relative error ≪ 1e-5), used by align_release; bails to
no-alignment (fixed splice) when the material doesn't self-correlate.
Regression test: 13-cents-mistuned non-integer period, 200-period
anchor distance — phase still lands within a bucket. Demo set load
stays ~1.2 s. 69 tests green.
