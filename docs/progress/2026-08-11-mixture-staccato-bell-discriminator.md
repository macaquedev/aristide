# 2026-08-11 — mixture staccato + the bell discriminator (2d3125a..21f9c06)

User: mixture/octavin staccato releases "pingy, like bells". Diagnosis
from own renders: no level bump — the ping is a high pipe's recorded
resonator ring (near-pure decaying sine = a small bell's signature).
Fixes: (1) two-part staccato tail — full level through speech-off +
early reflections (~150 ms), then room-charge level AND (1−charge)·25
dB/s extra decay for the undeveloped diffuse field; (2) λ fits stop 45
dB below tail peak (noise floors flattened treble measurements); (3)
repitch comp clamp ±15→±25 (mixture −600-cent ranks need −16..−18);
(4) release pitch sag — bells don't bend, pipes do (Viscount patent,
Aeolus release detune): 4·sqrt(f0/100) cents clamped 1–12, τ ≈ 12
periods, strength A/B'd by the user on a rendered 30 s French plein
jeu piece (38 cents "far far too much", 12 "really nice" → locked).
Octavin staccato tail 3.7 s → 2.4 s. Render tests: render_mixture_
staccato, render_listening_demos, render_plein_jeu_music. Workflow
that worked: render takes, attach to Discord, iterate on his ear.
