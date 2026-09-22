# Build: four piano-roll variations

Alex chose the piano-roll direction and approved two relative timelines, shared
onset/ending timestamps, finite release events, and paired continuation arrows.
The design authority now records those decisions in its selected Build section.

Implemented four clickable variations: Split desk, Stacked, Focus and Source
lanes. All share one editable Titanique model and undo history. The default Build
comparison and the previous `layout=roll` link open the new studies; earlier
structural mocks remain explicitly accessible for historical comparison.

The fixture matches Alex's example: Théorbe at 0 cents / 0 ms until release,
Flute 4′ at +1,250 cents / 0–50 ms, Ophicleide 32′ at −2 cents / 50 ms until
release. “Try release example” reversibly demonstrates a held Théorbe continuing
to key-up +50 ms, plus a new Bourdon event from key-up +50 to +150 ms.

Starts and endings store marker IDs, never unanchored time coordinates. Marker
edits update all attached events. Invalid lifetimes and duplicate timestamps are
rejected. New key-down events sustain until release; new key-up events receive
a finite ending (creating a marker if no later one exists). Drag moves pitch and
onset; the right edge selects an ending. The inspector offers exact pitch and
marker selections. Source references store source IDs; the source-sheet pitch
fixture is shared by every reference, not copied into events.

Both axes pan and zoom independently, with wheel/modifier navigation, middle-drag,
a touch Pan tool, zoom buttons and scroll thumbs. Pitch starts at zero-centred.
Global tuning guide fixtures (12/19/31 equal) only constrain gestures when Snap
pitch is enabled. Optional 10 ms time lines are visual; marker binding is always
mandatory. Very close pitches retain exact dots with leader lines to separated
bars. Finite endpoints outside the viewport do not acquire false visible end caps.

## Validation and review

- Production frontend build (TypeScript and Vite): passed.
- Complete desktop Playwright suite: **34 passed** on the final run.
- Browser checks cover marker-linked edits, finite release rules, paired arrows,
  drawing, dragging, resizing, non-12 guide snapping, source references, undo,
  retained edits across four layouts, and mouse/touch navigation.
- Existing Play, control-number and Tuning tests remain part of validation.
- Screenshots reviewed for all four layouts, paired arrows and a 390 px screen:
  dark/flat surfaces, one active accent, custom mark, stock Mantine controls,
  no decorative keyboard, readable zero baseline and no page-width overflow.
  Corrected crowded inspector buttons, uneven pitch labels and sheet close-button
  accessibility during review. Timeline scrollbars use rectangular thumbs.
- Study requests do not call the sound-control API. Existing mocked Play tests
  verify held key release across panel navigation; native audible continuity
  cannot be established by these silent design studies.
- Tests use `ARISTIDE_TEST_PORT=1421` here because an unrelated checkout already
  serves port 1420. `ARISTIDE_TEST_BROWSER=/snap/bin/chromium` selects the installed
  browser. A single browser worker avoids this machine's intermittent concurrent
  Chromium GPU-process launch failures.

## Explicit limits and next increment

These are labelled “Prototype / No audio / Not saved”, not the connected Build
editor. They neither write imported organ data nor change engine/model/RT code.
The source-reference demo has a shared pitch field, not a full nested rule graph.
Playback, key-up scheduling, per-held-key event ownership, source-rule expansion,
cycle detection across nested stops, live tuning inheritance, layer persistence,
global application undo/snapshots, per-key overrides, output/range editing and
MIDI assignments need the connected implementation after Alex picks a layout.

The target scheduler must cancel pending key-down starts on early release, allow
only already-started held continuations across key-up, and give release-triggered
voices finite note-offs while preserving normal sample release tails. Referenced
stops need a defined event-owner context so cancellation/release cannot cut another
held key's voices. Store timestamp identity, trigger anchor and optional end in the
layer; existing imported stop/rank data must remain compatible. No storage
migration is performed in this prototype increment.

First-run role-play is not repeated here: this increment only compares Build
layouts and does not change onboarding. No layout is selected on Alex's behalf.
