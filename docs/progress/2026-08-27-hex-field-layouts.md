# 2026-08-27 — the hex field becomes a real isomorphic board

The microtonal keyboard used to render as a single chromatic ribbon —
two interlocked hex rows by index parity, one column per key. That is
not what a generalized keyboard is. It now renders as a proper 2-D hex
field with a configurable isomorphic mapping.

## The parameterization

Every generalized keyboard since Bosanquet (and every tool in the
field — the Terpstra web app, the Lumatone editor) is described by two
step-vectors over a hex grid: key numbers advance by `right` per hex
rightward and `upright` per hex up-rightward; the third axis is their
difference. `aristide_model::HexLayout` carries `{ rows, cols, right,
upright, anchor }`; a manual declares it in the organ file as
`[[manual]] hex = { ... }`. Absent fields settle against the compass
(columns fitted to reach its top); wild values clamp with a warning;
an absent table means the derived Bosanquet default — nothing bricks.

The named layouts are derived, not tabulated, from the tuning's best
fifth `f` among `N` steps per octave: Bosanquet = (2f−N, 7f−4N),
Wicki–Hayden = (2f−N, f), harmonic table = (4f−2N, f). In 12-EDO:
(2,1), (2,7), (4,7); in 31-EDO: (5,2), (5,18), (10,18) — matching the
Terpstra web app's published 31-ed2 Bosanquet mapping.

## The seams held

The layout is a console fact, like the kind. Key numbers stay the
manual's contiguous range; `right`/`upright` are *key-number* steps,
so pitch still comes only from the tuning layer, `.ltn` input maps
still route controllers independently, and duplicate hexes (the same
key reachable two ways — isomorphic boards' duplicate notes) share a
key number and light together because held state matches `data-midi`.
Hexes outside the compass draw dead: present, dimmed, unplayable.

## Surface

- Snapshot: microtonal manuals always carry their effective `hex`.
- `POST /api/organ/manual/hex` — explicit fields, `preset=` (server
  derives the vectors against the manual's effective steps-per-octave
  and refits the width), or `reset=1`. Same write-the-file-then-reload
  contract as `manual/kind`.
- Console: right-click a microtonal keyboard → "Hex layout…" popover —
  presets, both step-vectors, rows/columns, bottom-left key in pitch
  notation. Fields echo back what the server settled on after each
  edit.
- Harness hook: `?kbdHexForm=<manual>`.

Still waiting: Lumatone `.ltn` key colours on the hex field (parsed
since M6, not yet drawn).

## Follow-up, same day: edits went live, colours landed

Field testing caught the first cut's real flaw: layout edits reused
the kind-change contract — write the file, reload the organ. On a
sampled organ that meant seconds of rebuild (and silence) per
keystroke, and any edit made during the rebuild was refused with "an
organ is already loading" and silently snapped back — the board ended
up not matching what you typed. Measured on the e2e rig: of two edits
150 ms apart, the second always lost.

Layout edits are now live, on the tuning contract: the file line is
written and the layout applies to the running console in place —
`Console::set_manual_hex`, no reload, no sound interruption. A burst
of preset + rows + cols edits all land within one poll.

The M6 leftover landed too: a bound `.ltn` map's key colours now tint
the hex field. `MidiPort::map_colors` walks the same channel-rank
extended numbering as `note_lands`, so each colour sits exactly where
its key's notes land; the snapshot carries `colors` beside `hex` and
the console paints wired hexes with them (held keys light accent,
grown over the bezel gap, so they read on any colour). One rig
caveat: the virtual computer keyboard has no `MidiPort`, so colours
only flow for real device ports — which is what a Lumatone is.

The hexes also got bezels (a center scale opens an even gap on all
three axes — a border would be eaten by the clip-path), and the whole
surface got its regression net: `bun tools/e2e/hex-audit.js` runs the
real server + real console UI under headless-chromium CDP and asserts
the lattice pixel-for-pixel (position, key number, deadness per hex),
the live-edit contract, and the colour path.
