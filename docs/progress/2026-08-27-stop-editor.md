# 2026-08-27 — right-click a drawknob: the stop editor

Any stop on the console can now be edited in place: right-click its
drawknob while unlocked (or ctrl-right-click through the padlock) and
a popover offers its **name**, its **pitch** — footage the way it's
engraved ("16", "8", "5 1/3") plus fine cents — its **gain**, and its
**source** (which source stop feeds this drawknob). Everything but
the source lands live, no rebuild; re-sourcing rewrites the pull and
rebuilds, keeping the drawknob's label.

## How a footage change sounds (the part worth deriving)

A stop drawn at a different footage is **not** a tape-speed trick.
The per-stop cents fold into the same pricing math the tuning seam
uses: whole semitones **re-anchor each key to the neighbouring pipe
that really sounds there** — an 8' drawn at 4' plays the pipe an
octave up, at that pipe's own recorded rate — and only the
sub-semitone remainder is bent. Past the rank's ends the edge pipe
stands in, repitched by sinc resampling, exactly the borrowing a
widened compass already got. That is what a unit-organ extension
does, so derived stops (a 16' extension of an 8' rank, a 2⅔' quint
off a unit flute) come out the honest way. Native footage is read
off the recorded pitches (pipe under a key vs that key's 8' unison);
a mixture speaks several footages at once, so it has no single
number and is voiced in cents alone.

## Where it lives in the file

- Rename: the pull that brought the stop in carries the console name
  — `rename = "..."` on its own `[[stop]]` line, or an entry in its
  `[[division]]` line's new per-stop `rename` map. Exact references
  (`[[move]]`, enclosure member lists, voicing rules) follow the
  rename, and the console mutates live (names are in the structural
  signature, so knob faces repaint).
- Pitch/gain: the stop's own exact-name `[[voicing.adjust]]` rule,
  now with a `pitch` footage field (number or `"2 2/3"`-style
  string). All-neutral values remove the rule again. Pattern rules
  still apply to the sound; they just aren't editable per stop.
- Source: a `[[stop]]`-pulled stop's line is rewritten in place; a
  division-pulled stop is left out of its `[[division]]` via the new
  `except` list and pulled afresh by a line of its own, label kept.

Every per-stop edit addresses its file lines by **provenance**
(source alias, source manual, source stop, via-division) recorded at
assembly — never by guessing at console names, which renames would
poison. `remove_stop` (the bin) rides the same rails now, so
deleting a division-pulled stop finally works too.

## Server API

- `POST /api/organ/stop/rename?stop=&name=` — live.
- `POST /api/organ/stop/voice?stop=&footage=&cents=&gain=` (partial;
  `reset=1`) — live: held keys re-speak the stop at its new pitch.
- `POST /api/organ/stop/source?stop=&from=&manual=&source_stop=` —
  structural, rebuilds.
- Snapshot stops carry `src {from, manual, stop}` and
  `pitch {native, footage, cents, gain, own}`.

Verified end-to-end on the demo set (real server, headless
chromium): rename lands live and in the file, `pitch = 4` re-anchors,
the voicing rule survives a retarget by riding the console name, and
the retargeted pull keeps the label. New tests: assembly
except/rename/provenance, footage parse/format round-trips, config
writers, re-anchor pricing, native-footage derivation, and the three
endpoints.
