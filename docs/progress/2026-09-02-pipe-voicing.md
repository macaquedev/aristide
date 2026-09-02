# 2026-09-02 — voicing reaches the pipe (M4, gap §7 residue)

`[[voicing.adjust]]` could say things about a *stop*. A voicer says
things about a *pipe*: this one screams, that octave is heavy, the
tierce of the mixture is too present. Three pieces close that: rules
that narrow to pipes, a tone control worth having, and edits that land
while the pipe is sounding.

## Rules narrow to pipes

A rule may now carry `keys = "C2..B3"` (scientific pitch notation,
C4 = middle C — the same `parse_note_name` `[[couplers.define]]` uses),
`key = "F#3"` for one pipe, raw key numbers (`keys = "48..59"`) where
the keyboard has no note names, and `rank = "Tierce"` for one rank of a
mixture. The span is matched against the key that sounds the pipe on
the stop's OWN keyboard, so a coupled press is voiced where it lands —
it is that pipe that speaks.

**Rules do not stack, and that is a decision, not an omission.** A
voicer setting a pipe's level is stating what that pipe should do, not
adding an offset to what someone else said; two voicers' notes about
the same pipe must not multiply into a third thing nobody chose. So per
field:

1. the rule speaking about the fewest keys wins;
2. among equals, the one naming a rank;
3. among those, the later line in the file.

A field a rule leaves out says *nothing* — it does not say zero — and
the pipe keeps what the broader rule gave it. That is why every value
field became `Option<f64>`: absence is load-bearing. `pitch` is the one
exception: a footage is a fact about which rank of pipes each key draws
on, so it is read only on an unnarrowed rule and warns otherwise.

Resolution happens at the one pricing site every voice goes through
(`Console::voices_for_key`), so note-on, a stop drawn under a held key
and a recouple cannot disagree — the same seam the stop-scope trim
already used.

## The brightness leg: one tilt, and why no bass shelf

The engine already carries a one-pole tilt per voice —
`out = lp + treble·(x − lp)`, hinged near the pipe's 2nd harmonic —
because the wind model breathes timbre through it, and the swell
shutters use the same form hinged at the box corner. The voicer's
`brightness_db` is simply a second, *static* factor multiplied into
that same `treble`. One filter, no second one; no per-frame `powf`
(the dB→linear conversion is control-side, in `StartVoice`'s new
`voicing_tilt`); and at 1.0 the existing bypass gate keeps rendering
bit-identical — the golden-hash test did not move.

A **bass shelf was considered and rejected.** A flue pipe has
essentially no energy below its fundamental: the lowest partial IS the
fundamental. Any shelf that would do something must hinge *above* f0,
at which point it is the tilt again with the opposite sign plus a level
change — and `gain_db` already exists. What is left below the
fundamental is room, and room is the reverb layer's business, not the
pipe's. Hauptwerk's per-pipe voicing pair is exactly amplitude +
brightness for the same reason. Clamped to ±12 dB: voicing is trimming,
not filter design.

## Edits land on the pipe that is speaking

`POST /api/organ/voicing?stop=&key=|keys=&rank=&gain=&cents=&brightness=`
sets any field at any scope, writes one comment-preserving
`[[voicing.adjust]]` line per scope, and refuses with 409 on an adopted
organ (voicing is an instrument fact). An empty value unsays a field;
`clear=1` drops the rule.

Landing it live needed a new engine command. `SetVoiceTrim { handle,
gain, tilt }` moves a **Held** voice only — the rule rate glides and
shutter moves already follow, because a release tail is room decay that
already left the pipe. `gain` is a multiplier on the gain the voice
STARTED with, so a knob dragged five times cannot compound and the
press's velocity survives untouched; it slews with the box gain's
~5 ms one-pole, so a drag is a fade. Pitch rides the existing
`SetVoiceRate` glide (30 ms) — except when the trim crosses a
semitone, which re-anchors the key onto a *different* pipe (a footage
change is a unit-organ extension, not a tape-speed trick); nothing can
glide to another pipe, so the stop re-prices instead.

One correctness gain fell out: rules that came from a name *pattern*
now survive a live edit. Before, editing a stop's gain replaced the one
merged trim and silently discarded whatever `stops = ["Trompette*"]`
had contributed. `TrimRule::owned` marks the rules the console owns —
a `[[voicing.adjust]]` naming exactly one stop — and only those are
rewritten.

## The console

The stop editor gains **Brightness** (dB) and a **Voiced pipes** row:
one chip per key span the stop is voiced apart on, so a rule set on one
pipe stays findable without remembering which pipe it was.

The gesture is where a voicer would look for it: with a stop's editor
open, **right-click a key of that stop's own keyboard** (shift-click a
second key for a span) and a popover voices those pipes — gain, cents,
brightness, and on a mixture one rank of them. It is a subview of the
stop editor: it closes everything else, never the editor that says
which stop, and closing that closes it. Blank fields are drawn blank,
not zeroed, because that is the resolution rule made visible.

Key numbers go on the wire (unambiguous, nothing to escape, and the
only thing a microtonal board's keys have); the writer spells them back
as note names wherever the keyboard has any, and matches an existing
rule through the *parsed* span, so a hand-typed `keys = "48..59"` is
found and rewritten in place.

## Verification

- **Tilt, measured on a rendered pipe** (a 1 kHz tone whose hinge sits
  at 25 Hz, so the tone is on the shelf's flat top): asked −6/−3/+3/+6
  dB, measured −5.978/−2.992/+2.995/+5.991 — inside 0.1 dB, asserted.
- **Bit-identity at 0 dB**: a voice with a tilt filter configured and
  `voicing_tilt = 1.0` renders the same `f32` bits as one with no
  filter at all (the bypass gate still holds), and differs as soon as
  the tilt is real. The engine golden hash is unchanged.
- **A live trim** on a held voice settles at exactly ×0.5 (0.002
  tolerance) and its worst single-frame step stays under 4× the
  signal's own natural rise — a fade, not a cliff. A released voice's
  tail is bit-identical with and without a trim command.
- **Resolution precedence**: a table of four overlapping rules (stop,
  bass octave, one pipe, one rank) checked at four coordinates —
  including that the octave's −12 dB *replaces* the stop's −6 dB rather
  than summing to −18.
- **Pricing** stamps the narrowed gain and tilt on the key inside the
  span and leaves its neighbour at the stop's values.
- **Revoicing** returns trims (not a re-price) for level/tone, a glide
  for a few cents, and asks for a re-price for a whole octave; a second
  edit is still measured against the note-on gain.
- **The endpoint**, on the demo set with two keys held: a span rule and
  a single-key rule land with no rebuild queued, ride the snapshot with
  their spelled labels, and appear as three separate
  `[[voicing.adjust]]` tables (stop, span, key); clearing one leaves
  the others; a bad span and an unknown rank are 400s; an adopted organ
  refuses with 409.
- **Console**: `poll-churn-audit.js` extended — the key-voicing popover
  must make no DOM mutation over a second of idle polling and must not
  close its stop editor. Screenshotted against the stub harness.
- Full workspace tests and `clippy --all-targets` green.

## Deferred, named

- **Per-pipe modulation depths per target with polarity** (Hauptwerk's
  voicing screens): Aristide's wind and box responses are per-pipe, but
  derived from the pipe's pitch rather than separately dialable.
- **Stereo balance / per-perspective mix**, and parametric EQ beyond
  the one tilt.
- **Release truncation as a voicing parameter** (gap §8's residue).
- The per-key popover edits one scope at a time; there is no
  drag-across-the-compass "voicing curve" gesture yet.
