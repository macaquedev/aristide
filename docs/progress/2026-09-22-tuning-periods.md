# Tuning without octave assumptions

Alex broadly preferred Channel strip and clarified that tuning must support
both non-twelve step counts and systems without octave equivalence. The study
now separates step count, optional repeat interval, and the reference key/Hz.

Equal division supports 2:1, 3:1 and custom repeat sizes in cents. Pitch
collection supports individually editable, unequal intervals and defaults to no
repetition. Its seven initial intervals are illustrative, not a named tuning.
Negative intervals and values beyond 1200 cents remain intact; entries retain
their key order. The first step stays at zero, anchored to the reference Hz.
Add/remove-last adjusts the collection, with Undo. Repeat size is independent
of the final entry and is never inferred from it.

The graph uses the actual interval range and numbered steps outside the
conventional twelve-note view. A mapping readout calculates the selected key's
frequency and shows the finite range for non-repeating collections. Those
collections return no pitch outside their range; no modulo wrapping or invented
extension. Repeating scales use the declared period in both directions. Cents
are an interval unit here, not an assumption of octave equivalence.

The layout, inheritance and shared number controls remain the selected design.
The prototype is still silent/unsaved and makes no engine requests. Mapping is
consecutive key IDs only; arbitrary mappings, Scala import and real audio/MIDI
integration remain incomplete. The study's step count remains capped at 128.

Backend audit: `aristide-server/src/tuning.rs` distinguishes `edo` (1200/edo)
from the existing Scala scale with its own period. `steps_per_octave()` also
returns a loaded scale's degree count, so its name and consumers need an audit
before connection. An optional-period model and explicit finite-collection
mapping must be integrated without silently converting a missing period to
1200 or altering imported tunings. Existing files and audio code are untouched.

Validation covers 19 steps at 2:1, 13 steps at 3:1, negative period traversal,
finite boundary mapping, signed/large intervals, browser editing, custom
repetition, inheritance and narrow-screen overflow. Existing mouse/touch and
archived-study tests also exercise the shared graph and model. Screenshots of
the running channel strip check both non-octave repetition and no repetition
against the visual and control-flow rules.
