# Tuning: DAW control arrangements

Alex selected scope and card, then asked to focus on arranging Tuning controls
like a DAW. Four clickable mutations now share one fixture: channel strip,
device rack, editor with inspector, and tabbed device. Build remains Split desk.

The browser searches and expands instrument → division → stop → pipe. The card
groups reference pitch/presets, scale, fine offset and selected-note editing.
Inherited controls stay disabled, with a link to the governing scope. Turning
Follow off copies the resolved tuning. Custom graph gestures commit once for
Undo; precise cents use the shared number control. Graph selection supports
keyboard and touch. Non-twelve equal divisions display numbered intervals.

Control-flow review: Play remains one link away; assignment uses the existing
sheet; no save/apply control; prototype edits never call engine endpoints.
These studies are explicitly silent and unsaved. Real audio continuity cannot
be auditioned on this headless machine. Visual review uses screenshots of all
four arrangements at 1280×800, with narrower viewport overflow checks. Gray
panels, blue selection and violet override marks retain the specified roles;
stock Mantine controls surround the custom musical graph. The first review
prompted tighter spacing to keep the note editor near its graph.

Validation: production build and browser coverage for inheritance, seeded
overrides, Undo, state retention across layouts, graph dragging, keyboard
editing, assignment sheets, touch, scale steps, search and responsive widths.
The 47-test Play/Build/study run passed 46 checks; a development hot reload
interrupted the source-reference check. A clean targeted rerun passed that check
and all eleven Tuning checks (12/12).

Not connected: persistence/global undo, audio, MIDI, recorded/historical
temperaments, just intonation, Scala import, key mapping and reference-key
selection. Root currently reorders pitch-class display only. The fixture is not
the engine's full tuning model; existing tuning storage and imports are untouched.
No mock key lighting or audition is presented as live. Alex still chooses the
preferred control arrangement before connection work.
