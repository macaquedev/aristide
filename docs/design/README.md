# Console design studies

Open `stop-concepts.html` directly in a browser, or serve this directory with a
static HTTP server. It is a self-contained HTML/CSS/JavaScript prototype with no
network dependencies and no connection to the audio engine.

The September 14 study compares three possible default stop designs requested
by the user: round drawknobs, oval stop heads and rocker tabs. Each comparison
uses the same names, pitches and registration; clicking one specimen updates
all three. The console below switches between shapes, compact/comfortable
spacing and 24/200 illustrative stops. Stops and sample general registrations
are interactive; they do not play audio or modify instrument files.

This is a proposal for selection and refinement. It does not change the shipped
console or the locked appearance decisions in `DESIGN.md`. The style choice,
size and registration are transient and reset on reload.

## Validation

Inspected the actual rendered comparison and phone layouts. A browser review
covered all three shapes with 24 and 200 stops at widths 320, 390, 768, 1280,
1919 and 2560, plus comfortable sizing, shared specimen states, recalling and
retiring registrations, keeping registration when switching shapes, and Space
activation. All 45 checks passed; no uncaught JavaScript exceptions occurred
while loading and exercising the prototype. No Rust or production UI code is
changed by this study.
