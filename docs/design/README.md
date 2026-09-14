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

## Rocker refinement

The user selected C. Rockers are ivory when off and saturated warm amber when
on, with a light amber outline. A 3px pressed position
reinforces engagement; retired tabs sit raised. The individual indicator bars
have been removed, leaving a clean face with centred lettering. C opens by
default. The other proposals remain available for comparison.

The initial monochrome treatment was too subtle. The on-state colour now changes
the whole tab face, so active registrations can be scanned across a large organ.
Text size and stop dimensions are preserved.

Validation: 24 browser checks passed across 320–2560px, both densities and
24/200-stop instruments; no JavaScript exceptions. Text contrast measures
7.25:1 on amber and 11.92:1 on ivory. The standalone HTML also opens and toggles
correctly without a server.

To bypass an embedded-browser failure, open this standalone HTML file directly
in an ordinary browser. All code is included; no server or build is required.
