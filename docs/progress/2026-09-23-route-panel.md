# Route panel connected

Spec sections: Route; Setup → Speakers; Control flow rules 2, 4, 6, 7;
Visual rules.

The Route tab opens a matrix on the live engine
(`desktop/src/route/RoutePanel.tsx`): divisions down the side, each
expanding to its stops, speaker groups across the top. Tapping an empty
cell connects the source at 0 dB; a connected cell follows the shared
number gesture — drag vertically (4 px per dB, heard while dragging, a
request at most every 100 ms), tap for a stepper with Disconnect, type
in the stepper, long-press or right-click for assign control (which says
it is not available yet). Levels run from −60 to +12 dB.

A stop follows its division until it has sends of its own; inherited
cells are outlined and greyed, owned sources carry the override mark
(◇) and a Follow/Reset button. A division whose stops are routed apart
opens by itself. Stops routed by an organ file's older `[[routing.bus]]`
tables are marked and labelled with the table's name. Perform mode
disables every cell. The top bar's Undo steps back through the panel's
own edits while it is open, restoring a source's whole send list
(`sends=`) or its follow state; it is not yet the global undo.

Setup → Speakers names groups as interface channel pairs; Main is
always 1/2. Groups live in the user config (`[[speakers]]`), because
they describe this machine's speakers. An organ that sends to a group
this machine lacks shows that column as "Plays through Main" and folds
it there; a group on channels the device lacks does the same and says
"not on device". Wrong, never silent.

## Model and engine

- Engine: a bus feeds up to eight output pairs, each at its own gain
  (`Command::SetBusSends`); gain changes ramp across a chunk, a send
  keeps the slot already feeding its pair, and an idle bus takes new
  sends at once. Buses rose from 8 to 16. The default path stays
  bit-identical (golden test unchanged).
- Server (`crates/aristide-server/src/routing.rs`): each distinct set of
  sends gets one bus; the default (Main at 0 dB) stays on bus 0. A
  changed set takes the bus it left, so held notes follow a level drag.
  Too many distinct routings (15 minus the file's bus tables) refuse the
  edit; at load the excess plays through Main with a warning.
- Layer: `[[routing.source]]` rows in the organ file (`manual`, optional
  `stop`, `sends = { Main = 0.0, Rear = -6.0 }`), written in place and
  read back by exact name. A sample set's own organ refuses with 409 and
  the panel saves "<organ> (edited)" first, as Tuning does.
- API: `GET/POST /api/routing`, `POST /api/speakers`.

## Gaps

- Rank- and pipe-level sources, and pipe presets (C/C♯ sides), are not
  built; the engine routes per stop.
- Build's Output field does not exist yet; it will read the same data.
- A new speaker group on channels past the running stream needs an
  organ reload before the stream widens; until then it folds to Main.
- Renaming a stop or division leaves its routing row under the old name
  (a load warning names it). Removing or renaming a speaker group does
  not rewrite organ files.
- Levels are not assignable to controls; undo is panel-local.
- A pipe speaks once: two differently routed stops sharing a pipe at the
  same pitch share its voice, which plays on the first stop's bus.
- Older builds reject files with `[[routing.source]]` (the sidecar
  schema denies unknown fields). Newer builds read old files unchanged.

## Validation

- `cargo test -p aristide-engine` (95), `-p aristide-formats` (89),
  `-p aristide-server` (216) pass; new tests cover multi-send buses,
  ramps, slot matching, bus allocation, file round-trip and the API.
- `desktop/tests/route.spec.ts` (fixture) and `route-live.spec.ts`
  (`ARISTIDE_LIVE=1`, run against a release server with the demo organ
  and isolated config) pass. The live run adds a speaker group, connects,
  drags, steps, overrides a stop, undoes, follows, relocks, and confirms
  a held note stayed held with no reload. The whole offline suite passes.
- Screenshots at 1280 × 800 and 390 × 844 reviewed against the
  control-flow and visual rules: one accent for connected, the modified
  colour only for override marks, red only for Panic and removal, no
  horizontal page scroll on a phone.
- Not verified: multichannel output on a real interface (this machine's
  device is stereo, so Rear folded to Main) and anything by ear.
