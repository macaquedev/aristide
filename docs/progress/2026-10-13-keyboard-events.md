# Rule events that press a keyboard (2026-10-13)

Agreed with Alex: a stop's rule line can play pipes, as before, or press a
keyboard "as if coupled". Couplers stay couplers on screen; underneath, the
two now share one path, so a stop that couples behaves like a coupler.

## Server and file

- `rule::Source` is `Pipes { stop, rank }` or `Keyboard(manual)`. In the file a
  keyboard line names its manual and no stop:
  `[[rule.event]] manual = "PEDAL"`, `cents`, `start`, …
- `Console::couple` adds a landing for each keyboard event of the drawn
  stops on the played keyboard: same key, the same compass and repitch policy
  as a coupler route, and it doesn't press further (couplers don't chain
  either). The landing carries the event's pitch, level, timing and limits,
  composed onto every part of whatever is drawn there (pitches and levels
  add, delays add, the earlier ending wins, timed events keep their lanes).
- Drawing, retiring or editing a stop that presses re-derives held keys as a
  coupler change does, so its sound comes and goes under held keys.
- Keyboard lines must start on key-down; a press after release is refused.
- `/api/rule` sends `{ "keyboard": index }` sources and lists `keyboards`;
  `/api/organ/rule` accepts them. Renaming or removing a manual follows or
  drops keyboard lines with the existing rule-following code.


## Stop editor

The Source drawer offers "<division>, as if coupled" above each division's
stops (disabled for events after key-up). The event reads "PEDAL · as
coupled" on the roll and in the inspector.

## Validation

- `cargo test --workspace`: a press sounds what is drawn on the other
  keyboard at its pitch, follows draws under a held key, stops at the pressed
  keyboard's compass (the pedal's D4 case), composes delay and level, lets go
  when retired or edited away; keyboard lines survive the API, file and reload.
- `bunx playwright test play`: choosing a keyboard posts `{ keyboard }`.
- Not yet heard on a real organ: the server exposes no sounding-voice count, so
  the audible check is Alex's.

## Gaps

- The "voices per key" figure counts a keyboard line by what is drawn there
  at the moment.
- A press does not trigger release events of its own after key-up.
