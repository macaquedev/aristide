# 2026-08-09 — release level matching (the "bell strike" fix)

User: releases sounded "like you hit a bell". Diagnosis: the splice was
phase-aligned but never **level**-matched — the tail always entered at
the recording's full loudness, so any voice quieter than that (early
releases during the attack; loop/tail level differences in the
recordings) got a step up followed by a decay: a bell strike. HW
explicitly matches release level at key-off (features datasheet);
now we do too:

- Each voice runs a ~10 ms envelope follower on its own pre-gain
  output; each sample stores the measured mean level of its tail's
  first stretch. At note-off the tail is scaled by their ratio
  (clamped ×0.05–1.3), folded into voice gain at fade completion.
- Crossfade curve linear → smoothstep (≈ raised cosine): linear fades
  dip on the uncorrelated noise floor (Appleton 2019).
- Regression test: releasing 1.5 periods into a ramping attack now
  peaks < 0.55× (was ~1.0×) with the tail leg itself at the voice's
  own ~0.37 level. 66 tests green.
