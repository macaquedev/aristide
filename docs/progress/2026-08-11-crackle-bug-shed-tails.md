# 2026-08-11 — THE crackle bug: shed tails re-entering the sustain loop (da32937)

The user's `--record` capture ended the months-long saga. His wav showed
single corrupted frames at exact 512-frame block boundaries; the
`crackle_hunt` reproduction (44.1 kHz, coupled tutti, PRODUCTION stagger,
fast spam/chords/trills, d2 click scan) reproduced 142 of them, and a
temporary per-voice probe fingered every one: `phase=FadeOut`,
`fade=1.001`, cursor teleporting (e.g. pos 138434 → 69476 in one frame).

Root cause: loop-wrap and seam-tap logic gated on `phase != Tail`. Tail
shedding and KillVoice flip Tail→FadeOut, so a cursor deep in release
material became eligible for sustain-loop wrapping and jumped back into
full-level sustain at amplitude 1.0. Big jumps = "previous notes
reappear and bang"; small jumps = the persistent crackle; shedding runs
constantly under fast playing with full registrations, in --safe, at any
buffer, with or without RT priority — hence the symptom's immunity to
every prior fix. Fixed with an explicit `past_loop` flag (set at
crossfade handover) gating wrap, seam reads, and FadeOut EOF. Outgoing
crossfade leg restored to full sinc (linear shortcut kinked splice
starts; mass-release worst block 1.59 ms of 5.33 ms). Hunt now runs in
the default suite: 142 → 0 events above the 0.015 teleport floor.

Remaining quality item: ~-40 dB R-channel kinks at some release splices
(stereo phase alignment is L-biased). Also this session: RealtimeKit RT
promotion (MakeThreadRealtimeWithPID via dbus-send; plain
MakeThreadRealtime resolves tids in the CALLER), sample-rate landmine
fix (never with_max_sample_rate — pick f32 nearest 48 kHz), commit-
stamped `=== DIAGNOSTIC:` startup line (build.rs tracks .git/refs, not
just HEAD), Engine::limiter_gain_db diagnostic.

Lessons, earned expensively: force the recording experiment before any
blind fixing; cleanliness tests must run production config (release
stagger was zeroed in every test, hiding the staggered path); phase
enums that conflate lifecycle stage with material location breed
exactly this bug class.
