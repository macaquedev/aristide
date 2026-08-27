# 2026-08-27 — the tremulant stops being a Hammond

Field verdict on the synth tremulant: "genuinely hideous — sounds
like a Hammond organ." Diagnosis, confirmed by recording the demo set
through `--record` and FFT-ing the amplitude envelope: the modulation
was a **pure sine** (one clean 5 Hz spectral line, 2nd harmonic
−20 dB, 3rd −43 dB) applied **uniformly and instantly to every pipe**
(single-LFO lock-step), and the combined gain + P³-brightness swing
at ±22 % pressure reached **±5 dB of amplitude — a 55 % depth chop**
on a single 8' pipe. Every one of those is the definition of an
electronic vibrato.

What changed, all derived from what a tremulant physically is — a
valve venting one wind system:

- **The valve wave** (`wind::valve_wave`): a relaxation cycle, not a
  sine — phase-skewed (fast fall, slower recovery), value-bent (the
  vent digs deeper than it crests), plus `VENT_BIAS`: a running vent
  bleeds the reservoir, so engaging the stop settles the chest
  slightly flat and soft. Smooth everywhere; nothing to alias.
- **Per-pipe speech dynamics** (engine, per voice): the chest says
  where pressure is; the *pipe* decides how it answers.
  `Command::StartVoice` now carries `nominal_hz`, and each voice gets
  one-pole lags scaled to its period — pitch follows pressure within
  ~4 periods, amplitude and timbre only over ~25 (the speech time).
  A 16' bass barely flutters at 5 Hz; a 2' pipe follows the valve;
  every pipe sits at its own phase. A fixed ±25 % per-voice
  sensitivity spreads depth across the chorus. `nominal_hz = 0`
  (noises) takes the chest unlagged, exactly as before.
- **One beater, one wind system**: chests share the tremulant's
  random rate/depth wander (identical sequences) and differ only by
  a few milliseconds of fixed propagation lag. The first cut gave
  each chest independent wander — the chests beat against each other
  at ~1 Hz, a seasick pump no organ makes. Recorded, measured,
  reverted.
- **Brightness saturation**: `P^3` is calibrated on few-percent
  regulator sags; at tremulant swings it said ±6 dB of treble. A
  pipe's spectrum saturates long before that — the tilt swing is now
  capped at ≈ ±2.5 dB.

Measured on one Gamba 8' pipe (identical recording script, before →
after): AM depth 55 % → 22 % (~2 dB, the literature's "a few dB"),
envelope 2nd harmonic −20 → −12 dB, 3rd −43 → −21 dB, fall/rise
steepness 1.17 → 1.22. FM (±10–13 cents, trough-heavy) is preserved
and stays coherent across the organ — the pitch wobble is the true
voice of a flue tremulant. In the ensemble recording the 5 Hz line no
longer dominates the envelope at all: pipes undulate diversely
instead of pumping as one fader.

A false lead worth remembering: the first ensemble analysis showed a
dominant ~1 Hz envelope line that survived every fix — it turned out
to be the *test chord* (an F pedal under C–E–G, near-coincident
partials beating). Single-pipe probes for tremulant character; the
chord measures the temperament.
