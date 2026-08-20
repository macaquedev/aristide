# 2026-08-20 — a second job is asked about, never assumed

One device driving two manuals, or one message doing two things, are
both legitimate wants — a keyboard sounding two divisions at once is a
coupler played by hand, and a piston that draws a stop *and* kicks the
tremulant is a tiny combination action. What they must never be is an
accident. So the rule is now uniform, for every bind path and every
trigger kind (a note, a control change, a program change, a computer
key): creating the second job **parks** the bind and asks —

    Computer keyboard already plays First Manual.
    Assign it to Second Manual as well, move it there,
    or leave things as they are?      [Keep both] [Replace] [Cancel]

## Where the question lives

Not in the UI. Learn completes on the server when the MIDI message
arrives — the console is only polling — so the server is the one that
knows a conflict just happened. `State::propose_input` and
`propose_control` are the bind paths every edit now takes (the HTTP
bind endpoints, the two learn flows, the computer-keyboard key path);
a collision stores a `Pending` instead of committing, the snapshot
reports it as `"conflict"`, and the console draws the dialog from
that. `POST /api/conflict?choice=keep|replace|cancel` answers it.
Nothing sounds, saves, or changes until the answer.

A row's *identity* is what can collide: device + channel for an input,
device + channel + trigger for a binding. Edits that keep the identity
— a shift, a compass, choosing a different action for a kept row —
never re-ask, so "keep both" is answered once, not on every later
touch. Channels overlap when either side is "any": a binding on
channel 3 and one on any channel can hear the same press, and that is
a collision even though the texts differ.

`Replace` means "this keyboard now plays here instead", so the facts
that belong to the hardware — a learned compass, a transpose — move
with it unless the new row states its own. That is the old
computer-keyboard steal, generalized and made consensual: the QWERTY
rows were the one device that silently moved before; now it is asked
about like everything else, and "keep both" genuinely leaves it on two
manuals (`State::keyboard` became a list, and a key press fans out).

A parked bind survives until answered, but any other edit — a new
learn, a removal, an organ load — clears it: acting on stale slot
indices would be worse than asking again.

## Notes actually land per route now

Dispatch already fanned out — every matching route and binding fires —
but a note's transpose was looked up once, from the first matching
route, and applied to all of them. With one keyboard legitimately on
two manuals at different shifts that would have been wrong notes, so
`MidiPort::note_lands` now yields `(manual, shifted key)` pairs and
each route applies its own shift. The compass rule is unchanged;
a shift that pushes a key off the MIDI range drops that landing alone.

## Verified

- `cargo test --workspace` green. New coverage: a device kept on two
  manuals plays both, each through its own shift; a message bound
  twice parks, keeps, or replaces (with the slot arithmetic when the
  removed rows sit under the target); cancel is a true no-op; editing
  a kept row's action never re-asks; the computer keyboard's replace
  carries the shift, its keep-both plays two manuals, and the whole
  flow works over the HTTP API.
- The dialog was rendered in headless Chromium against a stub server
  for both conflict kinds and the no-conflict state.
- Audible check is the user's, on their desktop rig.
