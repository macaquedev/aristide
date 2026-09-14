# Approved rocker stops in the console

The playing surface now uses the user-selected rocker-tab design: ivory when
off, warm amber with a light rim when drawn, centred name and pitch, a subtle
pressed position, and no individual on/off indicator bars. Long labels keep
13px lettering and wrap instead of shrinking.

The normal preset matches the approved 62px head height. Compact uses a 54px
minimum and Spacious 76px. Narrower grid tracks fit more columns than the old
horizontal tiles, and empty tracks remain empty rather than stretching sparse
stop banks across the full panel. Touch targets remain larger than 44px.

Crescendo-only stops retain a raised ivory face with an amber rim so the player
can distinguish pedal-held sound from a hand-drawn stop. Existing tuning marks
still indicate tuning overrides; they are not on/off LEDs. Drag insertion seams
remain visible over the new bevels. Real stop buttons now expose aria-pressed
both on local clicks and when the server recalls or changes the registration.

## Validation

- 228 browser checks passed: 32 large-organ density, 56 homepage layout,
  42 real-touch, 46 Solignac saved-layout, 22 click/drag and 30 combination checks.
- The playable 200-stop instrument keeps all controls readable and reachable
  across density presets and desktop/tablet/phone sizes. At 2560×1440 and
  3838×1999, the complete console fits in normal and Compact. At 1919×1080,
  this more physical shape uses vertical scrolling. Spacious trades capacity
  for larger heads and can scroll at smaller desktop sizes.
- The density audit waits for scheduled browser layout instead of sampling
  during a pending ResizeObserver update. It also checks the actual amber and
  ivory fills, centred engraving, pressed states, aria-pressed and absent LEDs.
- Combination checks verify that server-recalled states retain the distinct
  crescendo-only appearance. Click/drag and touch tests exercise real input.
- 31 existing JavaScript tests passed. `cargo check -p aristide-console --offline`
  passed. Actual Solignac and 200-stop screenshots were visually inspected.

Audible output and native WebKit rendering were not exercised; the browser
checks used Chromium against the real isolated server and sample fixtures.
