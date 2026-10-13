# Rule events limited by pitch (2026-10-13)

Spec: Organ → The stop editor, the voice field **Key range**. Alex widened
it: an event may answer only some keys, judged by the pressed key's pitch or
by the pitch the event itself sounds at — the way a mixture's ranks break.
Both are cents, never note names or MIDI keys, so they mean the same under
19-EDO, a Scala scale or a hex keyboard.

## Server and file

- `[[rule.event]]` takes optional `played` and `sounding` windows, each with
  an optional `low` and `high` (inclusive), in cents from the organ's pitch
  standard: 0 is the a′ the pitch reference implies, so moving the whole
  organ's pitch or fine offset moves no break.
- `Tuning::pitch_cents(key)` gives that measure, next to `deviation_cents`.
  As recorded, a key reads its place on the recorded ladder.
- The console judges both windows in the sounding stop's own tuning, at the
  key its own division sees, so a coupled press breaks where that keyboard's
  key would. `sounding` adds the event's cents and the voicer's pitch trim.
  A pitch trim on a stop with a sounding window re-prices held keys instead
  of gliding them, since it can carry an event across its edge.
- `/api/rule` reports `played`/`sounding` per event (null when unlimited);
  `/api/organ/rule` accepts them.

## Stop editor

- The event inspector's **Limits** group has a Key and a Sounding row, each
  with a from and a to edge. An edge reads Any until tapped, then is the
  shared number control (bounded by the other edge, so a window can't
  invert); × opens it again. They sit in the inspector, not on the roll:
  the roll's axis is cents from the pressed key, the windows are absolute.


## Validation

- `cargo test --workspace`: windows break a rank back an octave by key pitch
  and stop a twelfth by sounding pitch; `pitch_cents` holds under a pulled
  reference, fine offset and 19-EDO; windows are validated and survive the
  API, the file and a reload.
- `bunx playwright test play roll`: an edit in the stop editor posts the
  untouched event's windows back; edges open from Any and clear back to it.
  Screenshots reviewed at 1280 × 800, 900 px and 390 px wide.

## Gaps

- The roll does not show which events are limited; only the inspector does.
- Held keys are not re-judged when a tuning changes; the next press is.
- Beyond a borrowed rank's last pipe the console still stretches the nearest
  pipe rather than falling silent.
- The key-to-pitch measure still goes through the 12-step ladder internally;
  the planned tuning refactor removes that.
