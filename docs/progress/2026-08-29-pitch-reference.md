# 2026-08-29 — the pitch anchor is a key/Hz pair, not "a′"

"A = 440" only names a pitch in a tuning that has an A. Under 15-EDO,
Bohlen–Pierce, or any Scala scale whose degrees don't pass through the
twelve letter names, the statement that *stays* meaningful is the one
a `.kbm` file has always made: **this physical key sounds this many
Hz**. The tuning layer collapsed that pair into `a4_hz` — key 69
hard-wired as the anchor — so a 15-EDO piece with its tonic on C had
to back-solve "C4 = 261.6" into a fictitious a′ of 396.6 Hz. Now the
pair is the model everywhere.

## Model

- `tuning::PitchReference { key, hz }` replaces `Tuning::a4_hz`;
  default `A440`. `deviation_cents` puts the anchor first and derives
  everything from it: N-EDO steps out from the reference key; a
  temperament's table is read *relative to the reference key's own
  offset*, so "C4 = 256 Hz" under meantone puts C4 at 256 exactly and
  keeps the pure third above it. The linear Scala mapping is built
  from the pair (`KeyboardMapping::linear(key, hz)`); an explicit
  `.kbm` still owns its own reference.
- The old a′ 300–500 Hz sanity window survives as
  `PitchReference::clamped()`, applied through the *implied* a′ — so
  "C4 = 100 Hz" is refused the same way "A4 = 168 Hz" was.

## File and API

- `[tuning]` and `[[manual]]`: `reference_key = "C4"` (scientific
  pitch notation or a MIDI number) + `reference_hz = 256.0`. `a4_hz`
  is a serde alias for `reference_hz` (an A4 anchor), so every
  existing organ file loads unchanged; the writer emits the pair and
  removes the old key so a file never says it twice.
- `/api/tuning?reference_key=C4&reference_hz=256` — either field alone
  keeps the other; `a4=` remains as the legacy single-field form. A
  spelling that names no key is a 400, not a silent fallback.
- Snapshots carry `tuning.reference = {key, hz}` (also per entry of
  `manual_tuning[]`) in place of `tuning.a4`.

## Console

- The tuning popover's "Pitch a′" row is now "Reference [A4] = [440]
  Hz": the key field autocompletes from all 128 note names and
  re-spells what you type canonically; a non-key shows the popover's
  own error and reverts. The bar readout reads `equal · A4 = 440`,
  `19-EDO · C4 = 261.6`.
- `tools/e2e/prefs-split-audit.js` step 3 checks the pair live and in
  the file; the fallback `console.html` grew the same two fields.
