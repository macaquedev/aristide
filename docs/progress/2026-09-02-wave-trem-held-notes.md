# 2026-09-02 — a wave tremulant reaches the notes already held

Gap-analysis §2's last remaining item, and the M4 "mid-hold wave-trem
attack switch" deferral from 2026-08-26.

## What was wrong

A *wave* tremulant does not modulate anything: it chooses between
recordings. Since 2026-08-26 the chest carries the state
(`Command::SetWaveTremulant`), note-ons prefer the `IsTremulant=Y`
attack variant and note-offs pick a matching release — but a note
already sounding kept its plain recording until it was re-pressed.
Engaging the tremulant on a held chord did nothing audible, which is
the one thing a tremulant is for. GO solves this with
`SwitchToAnotherAttack`: a phase-aligned crossfade from the sustain
loop position into the other variant's loop. Synth-tremulant sets (the
GO demo, most free sets) were never affected; Hauptwerk sets with a
tremmed alternate rank — AVO Solignac, our fixture — are exactly the
case that was broken.

## The splice, derived

Two recordings of one pipe, tremmed and plain, are the same waveform
with a different envelope and a different starting phase. Crossing
between them is the release splice with the tail replaced by a loop,
so it reuses the same three pieces of machinery and adds one:

- **Phase.** `ReleaseAlignment` already maps "the voice's phase within
  its fundamental period" to "the frame of the other recording with
  the same phase". `Sample::attach_switch` builds the same 64-bucket
  table with the target's *loop start* as the anchor instead of the
  tail start, from the same quadrature projection (harmonic-immune,
  and jointly optimal across channels since 2026-09-02). Both phases
  are measured at the **source's** period: a tremmed take wobbles in
  pitch by construction, so its own measured period is only an
  average, and using it on the source side would smear every bucket.
  The residual that leaves is bounded by the trem's pitch depth over
  the distance travelled from the loop start — a fraction of a cycle,
  which is what the crossfade is long enough to hide. Measured on the
  synthetic twin (100 Hz, the tremmed take recorded 0.48 turns out of
  phase): the map lands within 1.5 buckets of the matching phase at
  every probe, and the crossfade never drops below 0.81 of the held
  level — which is the tremmed take's own trough — where the same
  splice with the phase map bypassed cancels to **0.12**.
- **Level.** The voice's envelope follower says how loud it is right
  now; the switch option stores the target's mean level over 512
  frames at its loop start — long enough to average the waveform, far
  shorter than a tremulant cycle, so the tremmed take's own undulation
  survives instead of being flattened. The voice's own gain divides
  out (the envelope is measured post-gain), and the ratio is clamped
  to a factor of three (≈ ±10 dB): two takes of one pipe further apart
  than that are a mislabeled pair.
- **Fade.** `pitch_scaled_fade_step` — ~9 fundamental periods, 6–184 ms,
  never longer than the note has lived. Same raised-cosine blend as the
  release splice.
- **New: the incoming leg loops.** A release leg runs off the end of
  its tail; this one wraps in the target's own sustain loop. It does
  not re-draw a random alternate loop mid-fade (the block context
  caches one range), and `past_loop` stays false throughout — the voice
  never leaves the loop, which is what keeps the 2026-08-11 crackle fix
  intact.

The phase stays `Held` for the whole crossfade, deliberately: wind
following, box following, the chest's wind draw, glide and release
selection all then behave exactly as they do for any held note. Only
the second leg is new, and it rides the block context's existing
"second sample" slot (a voice never has both a release leg and a
switch leg — see below), so the per-frame path is still two reads.

## Who chooses

The mapping sample → its tremmed twin is a *control-side* fact, so the
console sends `SwitchVoiceSample { handle, sample, rate_factor }` per
sounding voice rather than the bank carrying a `wave_twin` the RT path
would follow. The reason is that there is no unique twin: the console
already runs GO's `GetAttack` at every pricing site, and which
recording a pipe should be on depends on the velocity the key was
struck at, how long ago the pipe last closed, and a random tie-break
among equals. Re-deriving any of that in the audio thread would
duplicate the selection logic and get it wrong; the engine only needs
to be told the destination. `rate_factor` is the incoming file's
sample rate relative to the outgoing one — a property of the two
recordings alone, so it survives whatever tuning drift or glide the
voice's live rate is carrying.

The console re-runs **only the tremulant dimension** of the selection:
candidates are the variants that agree with the sounding one on
velocity bound and re-attack window, because neither of those facts
changed when the tremulant did. Among them, a recording made
explicitly for the new state beats one that serves both
(`IsTremulant` unset); if the voice is already on a valid one, nothing
is sent — a set whose recordings serve both states must not pop.

At load, `SampleBank::attach_switch` wires the phase map for every
ordered pair of a pipe's attack variants whose `IsTremulant` differs.
Pairs that agree cost nothing: a held note does not become
harder-struck.

## The two coincidences

Both are resolved without a discontinuity, which is why neither is a
"drop the other leg" case:

- **A second switch mid-crossfade** (trem off before the first fade
  landed) is a *reversal*: the two legs swap roles and the blend
  weight is mirrored. That is exact — `smoothstep(1 − f) = 1 −
  smoothstep(f)` — so the output does not move by one bit, and the
  gain and rate factors simply trade places.
- **A key-off mid-crossfade** waits. Dropping either leg is a step
  (they are level- and phase-matched, but a tremmed twin's own
  undulation is not), and splicing the release out of a *composite* of
  two recordings would need a third cursor. So the pallet waits for
  the crossfade to land — at most one fade length, ≤ 184 ms, and only
  for a key released inside that window of a tremulant toggle — and
  then splices cleanly out of the recording the voice ended up on.
  `fire_pending_release` and `begin_release` both refuse to fire while
  a switch is in flight; `switch_voice_sample` refuses a voice with a
  release already scheduled. Kill ramps and panic need no special case
  at all: `FadeOut` scales whatever the two legs sum to.

The one remaining step is a *third* recording arriving mid-crossfade
(a set with three or more variants disagreeing on `IsTremulant`, plus
two toggles inside one fade): the minority leg is dropped.

## Verification

`crates/aristide-engine/src/tests/switch.rs`, offline renders through
`Engine::process` on a synthetic stereo pipe recorded twice — plain,
and with a 15 % amplitude undulation at 0.48 turns of waveform phase
offset — scanning per channel across block seams:

- engaging mid-hold: the worst frame-to-frame step over the whole
  span is 0.00128, exactly the steady material's own (threshold
  1.15×); the worst level dip is 0.81, the tremmed take's own trough;
  and the settled output undulates 1.27:1 where the plain take was
  flat to 1.00:1. Both scans are load-bearing: bypassing the phase map
  dips the crossfade to 0.12, and forcing the handover at half blend
  raises the step to 1.38× steady;
- releasing the tremulant returns the note to the plain take,
  undulation back under 1.05:1, no step;
- a second switch a third of the way into the fade reverses with no
  step and ends on the recording it started from;
- a key-off mid-crossfade neither steps nor strands the voice (it ends
  in silence);
- after a switch, key-off takes the **new** recording's release (500 ms
  vs the plain take's 50 ms);
- a pair with no phase maps wired — exactly a set without tremmed
  variants — renders bit-identically with and without the command;
- the loop→loop phase map lands within 1.5 buckets at 32 probes.

Console side: a wave tremulant engaging under a held key emits one
switch for the sounding voice and switching it off emits the reverse;
a re-assertion of the same state emits nothing; a hard-struck voice
crosses to the tremmed twin of its own velocity layer, not to the
gentle recording; another chest's tremulant reaches nothing.

Full workspace tests and clippy green.

## Deferred, named

- **The AVO Solignac fixture was not rendered here.** The set is 2 GB
  and this box has 7 GB with several agents on it; the load path has no
  "just these ranks" mode, so a real-pipe A/B is the user's on the
  desktop rig. The synthetic twin carries the numbers.
- Hauptwerk's *second-layer* tremmed samples (the loader's own
  deferral) are still unread; only alternate-rank tremmed
  re-recordings become `wave_tremulant` variants, which is what
  Solignac ships.
- The fade length is ours (~9 periods), not GO's fixed 184 ms
  key-scaled figure. If an A/B says GO's is better on a real set, it is
  one constant.
- A third recording arriving mid-crossfade still drops the minority
  leg (above).
