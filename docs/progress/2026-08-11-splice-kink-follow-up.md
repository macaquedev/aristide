# 2026-08-11 — splice-kink follow-up: completion frame applied tail_gain twice (50f0fd9)

User confirmed the teleport fix by ear ("can't hear any crackles"); his
63 s recording measured 0 teleport-class events and exactly 1 faint
kink. Chasing that kink with the per-voice probe: XFADE-DONE storms
correlated, and component dumps showed the crossfade-completion frame
returning with the just-folded gain — tail_gain applied twice on that
single frame (blend already scales the tail leg), dipping it by up to
5x (staccato floor 0.2; trills expose it). Fixed with a frame_gain
snapshot before the phase arms mutate self.gain. crackle_hunt floor
tightened 0.015 → 0.008, zero events, permanent. Also: "*" stop
pattern + demo sidecar defaults to full organ (9a4c782).
