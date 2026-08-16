# 2026-08-16 — menu bar + preferences, and per-device MIDI routing

The console UI grew by accretion: a single header rail carrying volume,
reverb, a KEYS button, a tuning pill and PANIC, with settings hidden in
a drawer that shared its strip of window with the keyboard legend. Every
new preference had to fight for space on that rail. This replaces the
rail with a proper menu bar and moves settings into a preferences
dialog, which gives per-instrument settings somewhere to live.

## UI

- **Menu bar** (`ui/js/menu.js`): Organ / Play / View / Help. Items are
  produced by a function called at open time, so checkmarks and the
  list of manuals the computer keyboard can play state what is true at
  that moment rather than needing a refresh path of their own.
  - Organ: cancel registration, silence everything, Preferences… (Ctrl+,)
  - Play: keyboard legend, octave shift, which manual the QWERTY keys play
  - View: full screen (a console PC wants it), Appearance…
  - Help: about
- **Preferences dialog** (`ui/js/prefs.js`), tabs MIDI / Tuning / Sound /
  Appearance. Tuning (temperament, a′, transpose), noises and the accent
  and density pickers moved here from the drawer, which is gone; reverb
  moved off the bar into Sound. The bar keeps what a player grabs
  mid-piece: volume, the tuning readout (which opens its own tab), PANIC.
- The computer keyboard stops sounding pipes while a dialog is open, so
  typing 415 into the pitch field can't play a chord.

## Per-device MIDI routing

The MIDI tab is the first preference that needed new server surface.
Until now every input was merged and only the channel decided which
manual sounded, so two single-channel keyboards both played the same
manual and there was no way to tell them apart.

- `State.midi_ports`: name, enabled, and `route` — `None` follows the
  channel map (unchanged behaviour), `Some(manual)` pins every note,
  note-off and expression pedal from that device to one manual.
- The 16-channel map is now editable per channel. Short maps wrapped
  (3 manuals: channel 4 played what channel 1 did), which is invisible
  until you want to change one channel, so the API expands the map to
  all sixteen before applying an edit.
- New endpoints: `POST /api/midi/port?id&enabled&route`,
  `POST /api/midi/channel?ch&manual`, `POST /api/midi/rescan`; the state
  snapshot gained a `midi` object.
- Inputs are now owned by a supervisor thread that re-reads the port
  list once a second and reconnects when it changes: a keyboard plugged
  in mid-session appears in Preferences without a restart. Per-port
  settings survive a rescan by name, so unplugging one keyboard never
  re-routes the others.

Nothing about the sound changed: no engine code was touched, and a
device with no route set behaves exactly as before.

## Verified

`cargo test -p aristide-server` (31 pass), including new tests that a
pinned device ignores the channel map, that a muted one is silent, and
that the routing endpoints clamp what they are given. The page itself
was driven in headless Chromium against a mock server — every tab
rendered, and each control sent the command it claims to.
