# Console and instrument settings redesign

## Audit and plan

The current console mixes several generations of visual styling and editing
navigation. App preferences are temporarily moved into Organ preferences;
keyboard properties have separate implementations in a context menu and the
settings index; settings requires an extra editing-mode step from the toolbar.
Tiny beveled controls and widely separated panels leave the playing surface
hard to scan. The baseline browser audit also found that sample-set tuning
sometimes fails to appear in the settings index.

The redesign gives instrument settings one home, with console shortcuts opening
the same workspace and existing live forms. App preferences remain the home of
appearance and machine memory. Layout unlocking remains a console operation.
Existing organ persistence, tuning inheritance, protected-source copying and
saved panel positions retain their meanings.

The visual system uses quiet warm charcoal surfaces, ivory text, and the user's
chosen accent for selected controls. Sentence case, consistent field and button
sizes, and section hierarchy replace beveled controls and repeated card borders.
Default panel placement groups divisions into useful columns. Phone and tablet
layouts flow vertically without changing saved desktop positions. Settings use
a sidebar on desktop and a compact section chooser on small screens.

## Implemented

- Instrument settings is available while the console is locked. The old Organ
  settings menu and the toolbar's competing settings mode are removed. App
  preferences keep appearance and memory, including Ctrl+, while console
  shortcuts enter the shared instrument workspace.
- Keyboard type and rename now have one implementation. Settings actions use
  a section heading, a short description and simple rows. MIDI fields retain
  visible labels; stop search survives polling. Sample-set tuning has an
  independent request, unaffected by the console drawer's background refresh.
- Back and Escape navigate within the workspace. Closing restores the playing
  surface. A parent stop editor remains alive behind its tuning/pipe subview,
  with only the current form visible. Finishing a duplicate coupler preserves
  the existing link choice before navigation continues.
- The console uses measured division columns and keyboard cards, larger stop
  labels, clear selected states and one Clear stops action. The locked arrange
  control sits beside the console heading rather than over playing controls.
  Panel size changes trigger relayout, and rendering uses the same canvas
  coordinate system as drag persistence.
- Header rows, flowing panels, touch targets, full-height mobile dialogs and
  short landscape views have deliberate layouts. Accent and spacing preferences
  still work. No images, external fonts, build tooling or runtime dependencies
  were added.

## Verification

31 JavaScript tests and 441 assertions across 13 real-browser audits passed.
`cargo check -p aristide-console --offline` passed. The browser harness uses
isolated configuration and the real audio server with the GrandOrgue demo.

Coverage includes 320–1920px with mouse/touch and all three spacing presets;
60 editor/viewport combinations including 844×390 landscape; saved panel
coordinates after reopening; real touch chords and cancelled touches; drag
versus click; typed-field commits on Back, Close, section changes and backdrop;
focus containment; stable polling DOM; tuning inheritance; MIDI learning;
protected-organ copying; stop labels; microtonal geometry; combinations and
loading. Desktop, phone, settings and MIDI screenshots were visually reviewed.

Chromium emulation and native compilation do not replace physical touchscreen,
MIDI hardware, WebKit or audible testing. The existing stop-compass/model/engine
work in the working tree was preserved separately from this UI change.
