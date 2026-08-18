# 2026-08-17 — repitching: playing keys the set has no pipes for

A sample set's compass is whatever the organ it was recorded from had.
A player's keyboard is whatever they bought. When the keyboard is the
wider of the two — a 61-note console over a 56-note set — the extra keys
were simply dead, and there was no way to say otherwise. The same hole
swallowed anything microtonal: pitch was carried as a *rate multiplier
keyed on MIDI note number*, which is 12-EDO reasoning wearing a ratio's
clothes.

## The rule

The keyboard decides. Press **Listen**, play the lowest key, play the
highest: that names the port and channel *and* measures the compass.
Inside it every key sounds, repitched from the nearest pipe however far
that is. Outside it nothing sounds, ever — the locked compass rule,
with the player's hardware supplying the number instead of the set.

No stretch limit, deliberately. A limit would be a second rule
competing with the first, and the first is the one the player can see
and set. An octave-stretched pipe sounds thin and hurried; that is
information about how far the keyboard has been widened, and it is the
player's to weigh.

The range lives on the **input**, not the manual: it is a fact about a
piece of hardware. Two keyboards on one division may be different
widths, and each brings its own. A manual's playable compass is the
union of its keyboards', which is what the on-screen keyboard draws.

## What fills, and what does not

Filling happens where a pipe is *missing*, never where a stop is
*short*:

- **Past the set's compass** — the widened region. A range that reaches
  the set's own edge carries on past it; a range that stopped earlier
  stays stopped. A half-compass stop (divided registers, treble-only
  mixtures) is a musical decision, and extending it would invent an
  instrument nobody built.
- **Holes inside a rank** — a sample that failed to load, a rank with a
  gap. That is a defect, not a decision, so the nearest neighbour
  stands in.

The search is by rank index, not by frequency: ranks are semitone
ladders, and index-nearest survives mixtures whose pitch series breaks
backwards mid-rank. The *pitch* is then a frequency ratio against the
pipe that was actually found.

## Couplers are not keyboards

Filling is for the **played** key. A coupled copy sounds only a pipe its
division actually has: a 16' coupler running off the bottom of a rank,
a coupler into a shorter compass, and a hole in the rank all stay silent
in the copy, while the key the player pressed is filled in as usual.
Repitching is a concession to the player's hardware; letting a coupler
use it would be inventing an organ rather than reaching one. Sets that
want otherwise set `[couplers] repitch = true`.

## Pitch as a ratio

`VoiceSpec` now carries `nominal_hz`, the pitch its `rate` sounds. One
place (`Console::voiced`) settles what a voice plays:

    rate = spec.rate × repitch_ratio × scale_deviation(key)

The ratio is exactly 1 whenever the key's own pipe exists, so nothing in
the ordinary compass moved. `scale_deviation` is the existing
temperament/concert-pitch multiplier — cents against 12-EDO, which is
already general enough to express any scale; a Scala table drops in
there without touching the pipe side.

Everything downstream of pitch moves with it: a pipe pressed into
service five semitones up draws wind and gets its brightness hinge as
the pipe it is imitating, not as the one that was recorded.

`Console::voices_for_key` is now the single resolver for key → pipes →
voice parameters. A key press and drawing a stop under a held key both
go through it, so they cannot disagree about what a key sounds — they
previously carried two copies of the same walk.

## Anti-aliasing

Reading a sample faster is decimation, and the resampler's kernel was
built for rates near 1: fixed 0.9-of-Nyquist cutoff, 16 taps. At an
octave up it folds everything above the output Nyquist back into the
band. Repitching without fixing that would have shipped the feature and
the artefact together, so the sinc table became a family of kernels
selected by rate, with cutoff and width scaled to the shift.

## Verified

- `cargo test --workspace` green.
- Console tests: keys past the set repitched at both ends, keys outside
  the compass silent, a hole filled by its neighbour, a short stop not
  extended.
- Audible check is the user's, on their desktop rig.
