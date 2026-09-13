# Compact, readable stop controls

The previous 58–60px stop tiles used too much space on large instruments,
while low-contrast pitch labels and shrinking text made names hard to read.

Stop controls now place the name and pitch side by side, with a 32px minimum
height by default, 28px in Compact and 40px in Spacious. Both labels retain
13px text with stronger weight; long names wrap instead of shrinking. Touch
controls retain a 44px minimum target. Panel padding and grid gaps are smaller,
and empty grid tracks no longer stretch a few stops across the whole panel.

Instruments with at least 80 stops use the available display width. Smaller
instruments retain the centered 1800px content limit. Saved custom panel
positions and existing overlap recovery retain their behavior.

## Validation

- 176 browser checks passed: 32 density, 56 homepage, 42 touch and 46 Solignac
  layout checks. The density audit loads a playable 200-stop composite using
  the GrandOrgue demo samples across five manuals.
- All 200 stop controls fit vertically at 1919×1080 in the default density.
  At 2560×1440 and 3838×1999 the complete console fits in default and Compact.
  Spacious deliberately allows more scrolling in exchange for roomier rows.
- Checked 320–1024px touch layouts, 44px targets, fractional pitches, long names,
  label bounds, panel overlap and activation of the intended stop only.
- 31 JavaScript tests passed; `cargo check -p aristide-console --offline` passed.
- Desktop and phone screenshots were inspected. Audible output and native
  WebKit rendering were not part of this browser-focused change.
