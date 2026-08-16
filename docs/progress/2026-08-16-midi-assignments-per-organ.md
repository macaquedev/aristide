# 2026-08-16 — MIDI assignments persist, per organ

Follow-up to the menu bar / preferences work: routing was live-only, so
every restart put each device back to guessing from the MIDI channel.
Two questions had to be answered before writing any of it down.

**Where.** What is being stored joins two facts that live in different
places. A port name (`"Midiplus AKM320 MIDI 1"`) is a fact about *this
machine* — different on the user's desktop than on the dev box, and it
changes when a cable moves. A manual name (`"Récit"`) is a fact about
*the organ* — it doesn't exist on a set that calls its keyboards First
and Second. Sidecars are meant to travel with a set, so hardware names
must not go in them. Assignments therefore live in
`$XDG_CONFIG_HOME/aristide/midi.toml` (`~/.config/…`), the machine's
half, keyed **per organ** so one rig drives many instruments
differently. Manuals are stored by name and resolved through the same
fuzzy matcher the sidecar uses, so a renamed manual leaves the device
unassigned rather than playing the wrong division.

**What an unconfigured organ does.** Maintainer's call: an input the
player has not placed on *this* organ is silent. Nothing carries over
from another instrument, and a strange keyboard can never blast a
random division the first time it is plugged in. Devices therefore have
three states, and one control expresses all of them — a separate mute
switch would only have been a second way to say "none":

- `Unassigned` (default) — silent
- `ChannelMap` — obey the organ's channel map (a console whose manuals
  speak on separate channels)
- `Manual(i)` — pinned (a keyboard that only ever sends one channel)

Because silence-by-default reads as a fault, the MIDI tab says so
outright when nothing is assigned, and each unconfigured port says the
same in the startup log.

The 16-channel map is saved per organ too. `route` on the wire is now a
string (`"none"` / `"channels"` / a manual index) rather than the
sentinel `-1`, so the three states are legible in the API as well.

## Verified

`cargo test -p aristide-server`: config round-trips through TOML per
organ, saved names resolve against the loaded organ (and leave the
device unassigned when they don't match), an unassigned device sounds
nothing on any manual, and a pinned one still ignores the channel map.
Tests never touch the user's real config — `State.config_path` is
`None` unless the server set it. The dialog was driven in headless
Chromium, including the nothing-assigned state.
