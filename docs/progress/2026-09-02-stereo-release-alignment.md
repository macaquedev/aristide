# 2026-09-02 — the release splice serves both channels

Phase-aligned release splicing has been the engine's biggest realism win
since 2026-08-08, but it was built on one channel. The phase map was
measured from channel 0 and the tail frame it picked continued the left
waveform; whatever the right channel did at that frame, it did. The
2026-08-11 note recorded the symptom — "~-40 dB R-channel kinks at some
release splices (stereo phase alignment is L-biased)". This closes it.

## What the two channels actually do

A stereo pipe recording carries the same fundamental in both channels,
separated by whatever inter-channel phase the mic geometry imposes. If
that separation were the same in the sustain loop and in the tail, then
aligning the left would align the right for free and there would be
nothing to fix. Write the required tail offset for channel `c`, in turns
of the fundamental, as

    o_c = (theta_tail,c - theta_loop,c) / 2*pi

and the disagreement between the channels is

    m = o_R - o_L = ((theta_tail,R - theta_tail,L) - (theta_loop,R - theta_loop,L)) / 2*pi

— exactly the *change* in inter-channel phase between loop and tail. One
tail frame satisfies both channels only when `m = 0`.

`crates/aristide-server/examples/stereophase.rs` measures `m` on real
sets: quadrature projection of each channel at the loop anchor and at the
tail cue, at the period `Sample::measure_period` measures. To separate a
genuine shift from a weak channel's fundamental drowning in room noise it
re-measures from anchors a whole number of periods apart and over 4/8/16
cycle windows and reports the peak-to-peak spread; a real shift is
invariant, noise scatters.

GrandOrgue demo set, 84 stereo files with spliceable tails:

| mismatch magnitude (turns) | value |
| --- | --- |
| median | 0.0128 |
| p90 | 0.1095 |
| worst | 0.4705 (Trompette 8′ F#3 — 169°, nearly anti-phase) |

So the common case is a continuous take and the channels agree to about
5°, which is why this never dominated the sound. But the tail of the
distribution is real and reproducible: FlHarm 8′ F#7 sits at 0.182 turns
with a 0.008 spread, Octavin 2′ F#7 at 0.298 with 0.003, Octavin 2′ G7 at
−0.203 with 0.002. The large mismatches concentrate on the *high* pipes,
and that is the physics: their periods here are 7–30 frames, so a
wavelength is a few centimetres, and above the room's correlation
frequency the inter-channel phase of the diffuse field is essentially a
different draw at every position in the recording. Loop and tail are
different positions.

The Hauptwerk fixture (AVO Solignac, a drier hall, different placement,
106 stereo files) shows the same shape at a smaller scale: median 0.0059,
p90 0.0268, worst 0.1676 turns.

## The fix: a joint target, not a left-channel target

Crossfading a leg of amplitude `a` into one of amplitude `b` at a
fundamental phase error `e` costs the sum `2ab(1 - cos e) ≈ ab·e²` of
power. So the total discontinuity energy over the channels is minimized
by the *amplitude-weighted circular mean* of the per-channel
requirements, weighting channel `c` by the product of its loop and tail
fundamental amplitudes. That is `Sample::alignment_turns` in
`crates/aristide-engine/src/bank.rs`, and both alignment paths —
`align_release` (embedded tail) and `attach_release` (separate release
file, cross-file, and the case most likely to disagree since it is its
own take) — now build their bucket tables from it.

Properties that made this the choice:

- It is never worse than aligning on one channel. With equal weights the
  cost ratio is `1 / (2 cos²(pi·m/2))`, below 1 for every `m < 0.5` and
  exactly 1 at half a period (where no splice can succeed). No bail-out
  branch is needed.
- Mono reduces to the old left-channel answer *bit for bit*: the
  correction term is a literal `+ 0.0`. All the existing mono alignment
  regressions pass unchanged.
- It is pure control-side analysis. `ReleaseAlignment::target` is
  untouched — still two fmods and a table index, still allocation-free,
  still O(1). The table layout (`period: f64`, `offsets: Vec<u32>`) is
  unchanged; only the numbers in it differ.

Because the persisted numbers change meaning without changing layout, the
load cache's magic went `ARISBK02` → `ARISBK03` (`aristide-server/src/cache.rs`)
so stale entries read as misses instead of restoring left-biased tables.

## Verification

**Residual phase error and cancelled crossfade power**, over the demo
set's 84 stereo pipes (`stereophase`, which computes both strategies from
the same measurements):

| | L-only | joint |
| --- | --- | --- |
| \|err L\| median / p90 / worst | 0 / 0 / 0 | 0.0041 / 0.0384 / 0.2843 |
| \|err R\| median / p90 / worst | 0.0128 / 0.1095 / 0.4705 | 0.0051 / 0.0504 / 0.4081 |
| cancelled power median / p90 / worst | 0.0027 / 0.0931 / 2.4751 | 0.0013 / 0.0569 / 0.6193 |

Median cancelled power halves, p90 falls 39 %, worst-case falls 4×.
On the Hauptwerk set the same three numbers go 0.0006 / 0.0137 / 0.2453 →
0.0002 / 0.0045 / 0.0673.

**Rendered through the engine**, per channel:
`crates/aristide-server/examples/splicekink.rs` plays every stereo pipe
of a set at its own rate, stops it at eight instants spread across the
fundamental cycle (the adversarial anti-phase one included), and measures
each channel separately — `dip`, the worst period-length RMS through the
crossfade over the level the note held (a phase-wrong splice does not
click, it *cancels*), and `kink`, crackle_hunt's second-difference
outlier ratio narrowed to the splice region. 84 pipes × 8 stop phases =
672 splices:

| | before | after |
| --- | --- | --- |
| dip L median / p10 / worst | 0.918 / 0.439 / 0.137 | 0.914 / 0.452 / 0.137 |
| dip R median / p10 / worst | 0.879 / 0.452 / 0.112 | 0.905 / 0.466 / 0.111 |

The L/R gap at the median closes from 0.039 to 0.009. 172 of the 672
splices move at all (the rest have no mismatch to fix); on those the
paired change is **dip R +0.045 mean, best +0.438**, dip L +0.009 mean.
The one pipe that trades the other way is Octavin 2′ F#7 (mismatch 0.298
turns, right channel 3.4× the left): dip R 0.55 → 0.97, dip L 0.63 →
0.45. That is the energy weighting doing its job — the loud channel is
the one worth continuing, and the absolute cancelled amplitude there
drops about 3×.

`kink` found nothing either before or after: every splice in the set
scores 2.4–7.9 against crackle_hunt's 12× gate, on both channels. The
residual stereo defect was cancellation, not a click.

**Regression** (`crates/aristide-engine/src/tests/release.rs`,
`stereo_release_splice_serves_both_channels`): a synthetic stereo pipe
whose tail carries a quarter-period inter-channel shift, released at
seven instants across the cycle. Left-only alignment leaves the right
channel at `cos(45°) = 0.71` of its held level at the fade midpoint;
splitting the error puts both at `cos(22.5°) = 0.92`. The test asserts
both channels above 0.85 and within 0.06 of each other, and separately
that a zero-mismatch stereo pipe stays above 0.95 on both. Verified to
fail on the previous code (L 0.992, R 0.699) and to pass now (L 0.898–
0.929, R 0.902–0.941).

## What else was checked, and left alone

- **Every other per-frame release gain is already symmetric.** The
  crossfade-completion frame (`frame_gain` snapshot, `self.gain *=
  tail_gain`), `apply_tail_charge`, `apply_eof_guard`, `decay_tail` and
  the release bend all apply one scalar to both channels, and the bend
  moves both legs' cursors together so it cannot open a relative phase.
  No per-channel bias found.
- **The level matcher stays mono-summed, deliberately.** A stereo tail
  really does sit at a different L/R balance from its sustain — measured
  on the demo set at median 0.78 dB, p90 4.38 dB, worst 11.42 dB — but
  that is the room, not an artifact: the direct sound that favours the
  near mic stops with the pipe and only the more symmetric diffuse field
  is left. Matching per channel would overwrite the recorded release's
  stereo image with the sustain's. Phase is the opposite case: a phase
  mismatch buys nothing and only cancels. The reasoning is recorded on
  `match_tail_level`.

## Deferred

- **Sub-frame splice targets.** `offsets` stores integer frames, so on a
  7-frame-period pipe (Octavin 2′ top octave, ~6 kHz) the target is
  quantized to 1/7 of a cycle — up to 0.07 turns of error, common to both
  channels and therefore invisible to every measurement above.
  `release_position` is already an `f64` read through the sinc kernel, so
  storing fractional offsets would work; it is a table-layout change
  (another cache bump) and belongs with its own measurement.
- **Cross-file stereo mismatch is unmeasured on real data.** Neither
  fixture has separate release files with stereo attacks, so
  `attach_release`'s joint path — where the mismatch should be *largest*,
  since the release is its own take at its own placement — is covered
  only by the synthetic regression.
