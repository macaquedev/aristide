# 2026-08-11 — the F-major pop: pipes must speak once (refcounting)

User's precise repro (6-note F major, all Great+Swell, coupled 16'+8')
finally identified the real mechanism — and it was the pipe-doubling
bug spotted during the step-back and wrongly deferred as "bonus
correctness". With octave doublings + a 16' coupler, one pipe is
reached by TWO held keys (F4's 16'-coupled Swell pipe is F3's
8'-coupled pipe). Two voices on the same sample sum incoherently while
held (+3 dB), but at release the phase aligner sends BOTH to the same
tail at the same phase — coherent +6 dB — so the release is LOUDER
than the chord: a thump/pop scaling with how many pipes the chord
doubles. Console now refcounts speaking pipes: a pipe starts one voice
regardless of how many routes reach it, and stops only when the last
holder (key, coupler, stop) lets go. Retrigger, stop-retirement, and
all-off flow through the same refcount. Regression: octave-coupled
shared pipe starts once, survives the first key's release, stops with
the last. 88 tests green.
