# 2026-08-11 — release realism: the room was transposing (ce593d8)

Research mandate ("read a ton of papers"): three parallel literature
sweeps (Rucz 2015 thesis — the only quantitative organ release
measurement found; Fletcher & Rossing; HW/GO docs, source, and forum
archaeology; sampler DSP incl. Signalsmith crossfade analysis and SFZ
rt_decay), written up with citations in docs/research/release-modeling.md.
Then numerical measurement of the engine against the demo set found the
"artificial/bell" release had three causes:

1. Repitch time-scales the recorded room decay: the demo set records 2
   pipes/octave and repitches ±600 cents, so down-repitched keys rang
   41% too long (bell), up-repitched were plucks — key-dependent wrong
   ring time. Fixed by measuring each sample's tail decay rate λ at
   load and compensating λ(R−1) dB/s (clamp ±15) in release(). Ring
   time is now key-invariant; native-pitch samples untouched. Novel —
   neither GO nor HW does this.
2. The GO staccato port decayed tail RATE; physics says rate is
   invariant, level builds. Now: room-charge level scaling.
3. Fixed 30ms crossfade; now ~9 fundamental periods (184→6ms), capped
   by note age (mid-attack releases must collapse, not swell).

Plus EOF guard fade (46ms). Diagnostic tests: release_envelope
(#[ignore], /tmp wavs + level-match inputs), regression
repitched_release_rings_at_native_decay_rate. 92 green. Debug journey
recorded: envelope-vs-reference red herring (master-gain accounting),
√2 spec.rate discovery via RELDBG, per-pipe ODF archaeology (PIPEDBG).
