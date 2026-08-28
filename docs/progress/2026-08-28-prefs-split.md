# 2026-08-28 — user preferences vs organ settings, fully separated

The Preferences dialog had grown six tabs — organ, MIDI, controls,
tuning, sound, appearance — of which exactly one was actually a user
preference. The rest were organ facts wearing the wrong clothes, and
two of them (whole-instrument tuning, reverb/noises) weren't persisted
at all: set a′ 415 in the evening, get 440 back in the morning. This
change draws the line the design always implied and makes it
structural: **the Preferences dialog is the player's; every organ fact
is edited on the console and lands in the organ's file.**

## Server

- `config.rs` gains three comment-preserving writers:
  `write_composite_tuning` (top-level `[tuning]`, same absence rules as
  the per-manual writer), `write_composite_reverb_wet` (`[reverb] wet`,
  a no-op for an organ without an IR), `write_composite_noises`
  (`[noises] enabled/volume`, matching the sidecar's own names).
- `/api/tuning` with no `manual` now persists on every commit (the
  fields are discrete commits, not drags) and refuses mid-rebuild like
  every other file-writing edit. `/api/reverb` and `/api/noises` take
  `persist=1` — the client sends it on a slider's release only, so a
  drag stays ~30 live commands/s and one file write.

## Console

- **Preferences** (now under a new leftmost **Aristide** menu, with
  About; still Ctrl+,) holds only the appearance controls — a small
  card, no tabs, no organ name in its header. Nothing in it sends an
  API command, and the e2e audit asserts exactly that.
- **Keyboard right-click menu** gains *MIDI input…* (that manual's
  input rows — device/channel/shift/bend/Listen — plus quick piston
  rows for the pitch actions that shift it, plus the ports list and
  Rescan) and *Compass…*. The rows moved intact from the old dialog
  into `wiring.js`, shared builders both surfaces use.
- **Silent badge**: a keyboard whose manual has no input wears
  "silent — no input"; clicking it opens the MIDI popover. Silence is
  the honest default for an unwired organ, but it must not look like a
  fault — and now the fix is one click from the symptom.
- **Organ menu** carries the organ-wide, anchorless settings as
  popovers: *Tuning…* (the existing per-manual popover grown a
  whole-instrument mode — also what the bar's tuning readout opens
  now), *Room & noises…*, and *Bindings…* (the flat list, moved
  wholesale). The stop and coupler editors each gain a **Piston** row —
  a filtered view of the same bindings list: Listen learns a trigger
  and points it at `stop:<name>`/`coupler:<name>` (the quick-bind pump
  in editor.js finishes the two-step server flow).
- **Save-as** moved to the organ-name menu (and auto-offers once for
  an ad-hoc multi-set combination, replacing the old auto-open of the
  Organ tab). Hidden couplers restore from the add menu's coupler
  form, next to where couplers are created.
- Menu items now stop their click's propagation and pass their anchor
  to `run`, so a menu item can open a popover under itself without the
  same click closing it.

## Verified

`cargo test -p aristide-server` (143 pass; new round-trip,
comment-preservation and persist-endpoint tests) and clippy clean. New
end-to-end audit `tools/e2e/prefs-split-audit.js` (real server on a
throwaway `XDG_CONFIG_HOME`, real UI, headless chromium): 30 checks —
menu taxonomy; Preferences holds no organ controls and using it sends
zero API commands; a′ 415 lands live *and* as `[tuning] a4_hz` in the
file; noises edits write `[noises]`; the silent badge opens the MIDI
popover, binding the computer keyboard writes `[[midi.input]]` and
clears the badge; the bindings popover learns `key:KeyQ`; the stop
editor's piston row quick-binds `key:KeyW` to `stop:Contre- basse 16'`.
All green, screenshots under `target/prefs-split-audit/`.
