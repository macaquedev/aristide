# Organ settings apart from Aristide's

Alex, 23 Sept 2026: organ settings and Aristide settings are separate, there is
a real view for editing an organ, and routing moves out of its own jargon. The
spec (`docs/design/ui-flow-spec.md`, "Screens and shell", "Organ", "Sound",
"Tuning", "Settings and errors") now records this.

## Tabs

**Play · Organ · Sound · Tuning · Library**, with ⚙ **Settings**.

- **Organ** (formerly Build) lists the divisions, each with its stops, and
  then the couplers. Selecting one opens its editor beside the list. Organ
  is always reachable; long-pressing or right-clicking a stop in Play opens
  it there. There is no edit lock: everything is editable at all times.
- **Sound** is the former Route matrix. **Settings** is the former Setup:
  MIDI, speaker groups and appearance, which don't depend on the organ.
- **Tuning** lists only the whole instrument and the divisions. A stop's own
  tuning, and each rank's for a mixture, is edited in the stop editor's
  Tuning view. It links back to whatever the stop follows.

## What Organ edits

| Item | Live, no rebuild | Rebuilds the organ |
|---|---|---|
| Stop | rename; move to another division; its rule and tuning | add (from any source sample set, or again as a copy); remove |
| Division | — | add; rename; reorder; manual or pedal; remove |
| Coupler | rename; show or hide on the console | add; change play-from, also-sounds and pitch; remove |

Adding a sample set as a new source uses the folder browser in the Add stop
sheet. The sheet offers only playable stops: a set's control noises
(drawstop thumps, blower, tremulant) belong to their controls and are left
out, by the same name rule the console uses. Removals ask for confirmation.
Failures show one sentence in a modal.

## Backend fixes found on the way

- The organ file finds stops by division and name. If a division held two
  stops with the same name, removing the copy also removed the original.
  Pulling or moving a stop onto a division that already has one with that
  name is now refused, as renaming already was, and the UI doesn't offer it.
- A moved stop's pull line still names the division it was pulled onto.
  Removing, renaming or re-sourcing that stop now follows its `[[move]]`
  lines back to the pull line and removes only that stop's moves.
- Loading the automatic "(edited)" copy, or rebuilding after a structural
  edit, used to send the app back to Play and lock it. Now only an organ
  picked in the Library does that.

## Gaps

- **Structural edits rebuild the organ, and sound stops for the rebuild**
  (about 0.1 s on the demo; longer on large sets). That breaks Control flow
  rule 4 for these edits. Making add, remove and reorder live needs the
  console to change in place instead of reloading from the file.
- Stops cannot yet be duplicated in place or dragged to reorder within a
  division. Adding a stop again from its sample set is how to copy one for
  now, and the copy needs a different division or a rename first.
- A coupler with several routes can only be renamed, hidden or removed.
- Division compass and keyboard kinds other than manual/pedal (microtonal)
  have endpoints but no controls yet.
- No global undo for structural edits: the top-bar Undo still covers only
  the stop editor's and Tuning's own edits.

## Validation

- `cargo test -p aristide-server -p aristide-formats`: 226 + 89 pass. New
  tests cover refusing a second stop of one name and removing a moved copy
  while leaving its same-named original.
- The offline Playwright suite passes. Against a release server running the
  demo (isolated config, gain 0), the live tests pass:
  `organ-structure-live` (rename; add a division; add a stop into it; the
  division that already has that name is not offered as a move target;
  move; remove; add and remove a coupler; remove the division; the tab
  stays on Organ throughout; the stop set ends exactly as it began), plus
  `organ-live`, `tuning-live` (now using the repo's `scales/`) and
  `sound-live`.
- Screenshots at 1280 × 800 and 390 × 844 were reviewed against the Visual
  rules. The review split the stop header into identity and event actions,
  gave field labels one style, and made the list scroll to the selection on
  a phone.
