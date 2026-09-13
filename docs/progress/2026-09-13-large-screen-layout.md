# Large-screen console layout repair — 2026-09-13

The reported Solignac extend console had four keyboards, but automatic placement
always stopped at three columns. A saved GREAT panel at the top-left was then
replayed over the automatic PEDAL panel. Automatic panels reserved the old GREAT
slot anyway, leaving a large hole and pushing Echo and the registrations down.
The original UI audit used three-keyboard Friesach without overlapping saved
positions, so it did not cover this combination.

The grid now chooses columns from the actual keyboard count and available space,
balances additional rows, and limits content to 1800 CSS pixels on wider displays.
Four divisions and their keyboard cards fit into four aligned columns at the
reported desktop size. Stop names and pitches use separate lines. Canvas height
comes from the actual heading and measured content, avoiding an extra scrollbar.

Saved positions are checked against every visible panel after measurement.
Colliding or oversized arrangements display the automatic layout without changing
the organ file. Valid custom arrangements continue to replay their coordinates.
The Auto arrange button in the editing toolbar explicitly clears saved geometry
through the existing panel-placement API, preserving stop order, source data,
comments and musical settings. It is a live layout preference, including for
protected sample-set organs; it refuses changes during loading.

Validation:

- Reproduced the reported overlap against the real Solignac extend fixture with
  GREAT saved at x=0, y=0, w=0.305 before implementing the repair.
- New Solignac browser audit: 46 checks across 390–3838 CSS pixels, a 3838-pixel
  high-DPI capture, all spacing presets, saved overlaps, valid custom placement,
  expanded keyboards, Auto arrange, reopening and organ-file reload.
- Existing responsive layout and touch audits: 98 checks passed, including saved
  drag positions, touch chords, cancelled touches, dialogs and narrow toolbars.
- 31 JavaScript tests passed; native console compile check passed.
- Server panel-placement regression passed with new reset coverage: no reload,
  preserved non-layout TOML, protected-organ access, and loading refusal.
- Desktop and phone screenshots visually inspected. Browser automation uses
  Chromium; physical MIDI/audio and native WebKit rendering were not exercised.
