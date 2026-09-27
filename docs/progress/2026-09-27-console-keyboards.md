# Console settings: the organ's manuals, detected Hauptwerk-style

Alex, 2026-09-27: "Use computer keyboard" did nothing when another manual
already had the computer keyboard, and the console settings were hacky. Redo
them as Hauptwerk does, as friendly as possible, and split console settings
from the rest of Settings. Every organ is different, so show the organ's own
manuals and discover their keyboards; no invented "Manual 1" keyboards.

The cause of the bug: binding a device that already played another manual
parked a keep-both / replace / cancel question on the server
(`Pending::Input`), and the new UI never showed it.

An intermediate version added machine-wide named console keyboards mapped
onto every organ. Alex rejected it the same day and it was removed; any
`[[console]]` or `keyboards` entries it wrote are ignored on load.

## Model

Unchanged: assignments stay per organ, manual first, in `midi.toml` (or a
composite organ file's `[midi]`). A newly loaded organ is silent until its
manuals are detected.

- `/api/midi/learn?manual=&detect=1` waits for one key on any keyboard. That
  device and channel then alone play the manual. A letter key during
  detection picks the computer keyboard and plays nothing.
- `/api/midi/assign?manual=&device=&ch=` does the same by hand; an empty
  device clears the manual.
- Both move the keyboard: the same device on an overlapping channel is
  removed from every other manual, so nothing waits on a question.
- The snapshot's `midi.detected` counts keys detection has heard, so the UI
  can tell a key from a timeout; `midi.learning.detect` marks the one-press
  mode.

## UI

- Settings has tabs: Console, Speakers, Appearance.
- Console lists the loaded organ's manuals: MIDI device (None, the computer
  keyboard, every port, and a saved device that is not connected), channel,
  and Detect ("Press any key on the keyboard for Second Manual").
- Detect all is a sheet that walks the organ's manuals in order with Skip,
  ticking off the device each one got.

## Validation

`cargo test -p aristide-server` (detect by computer key, move, assign,
clear); `ARISTIDE_LIVE=1 bunx playwright test console-live` against a server
with its own `XDG_CONFIG_HOME` drives Detect, the move and Detect all while a
note is held.

## Gaps

- The first-run "Learn the console" step does not exist yet; it should reuse
  Detect all.
- A keyboard's own range (two-press learn) and pistons, swell pedals and toe
  studs (`/api/control/*`) have no new UI.
- Detection with real MIDI hardware is covered by the existing unit tests
  only.
