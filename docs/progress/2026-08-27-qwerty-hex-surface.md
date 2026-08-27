# 2026-08-27 — the computer keyboard plays hex manuals

The computer keyboard used to speak only piano: two letter rows,
naturals and sharps — meaningless vocabulary on a microtonal manual.
Now the mapping follows the manual it addresses. On a hand keyboard,
nothing changes. On a microtonal manual, all four QWERTY rows become
a window onto the manual's own hex layout: Z row is board row 0, A
row 1, Q row 2, digits row 3, and each cap sounds
`HexLayout::key_at(col, row)` plus the input's shift — the exact
axial math the on-screen board uses, so isomorphic shapes (and their
duplicate notes) carry from screen to keyboard cap for hex. Under
12-EDO Bosanquet, Q sounds the same key as Z, the way board row 2
duplicates row 0.

The physical stagger of a keyboard is close enough to a hex grid's
for the shapes to feel right; the fourth (digit) row physically leans
the wrong way, but it's bonus range and bindings still win there
(`=` bound to octave-up never plays a note).

Mechanics: `control::KEYBOARD_GRID` is the (code, col, row) table —
the piano `KEYBOARD_ROWS` untouched beside it; `State::key` resolves
per assignment, so one keyboard confirmed onto a hand manual and a
hex manual plays each in its own vocabulary. The legend rebuilds as
the staggered four-row grid, each cap carrying the raw key number it
plays and the key's Lumatone map colour where a bound `.ltn`
provides one — the same tint as the on-screen hexes, so the legend
and the board read as one instrument.

Verified live on the rig (held keys land on the lattice, board hexes
light from QWERTY presses, duplicates included) and in the unit suite
(`the_computer_keyboard_plays_a_hex_manual_isomorphically`); the
`tools/e2e/hex-audit.js` net grew a QWERTY step.

Known rough edge, deliberate: `transpose` (and the octave-up binding,
+12) is in raw key steps — on a 31-EDO manual an octave is 31 steps,
so the octave buttons move the window by less than an octave there.
Transpose semantics under non-12 EDOs are a future conversation.
