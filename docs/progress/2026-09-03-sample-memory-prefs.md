# 2026-09-03 — sample memory is the player's: `[samples]` moves to Preferences

Disk streaming shipped yesterday with its policy in the organ file's
`[samples]` section, next to `bits` and `cache`. That was the wrong
scope, and the 2026-08-28 split already said why: an organ file
travels, and whether Solignac fits in RAM is not a fact about Solignac.
It is a fact about the box loading it. A 64 GB desktop and a 16 GB
laptop should read the identical organ file and reach different
residency decisions. A setting belongs where its truth lives.

## What moved

The whole section — `streaming`, `ram_budget_mb`, `bits`, `cache` — is
now the user config's `[samples]` (`server/config.rs::SamplePrefs`,
next to the library and the wiring, which are the same kind of fact).
The loader takes it as a parameter (`load::prepare_with`); the organ
file no longer has a say.

- **Organ files that still carry `[samples]`** load: the section is
  kept as an optional, all-`Option` struct so `deny_unknown_fields`
  does not refuse the file, and the load emits a warning naming the
  keys it ignored, in the same channel the console already shows for
  healed references. Wrapping a set no longer copies the section out
  of its sidecar.
- **`streaming` is an enum** in the config (`auto | on | off`) with
  `#[serde(other)]` on `auto`: a typo in a hand-edited file reads as
  auto instead of costing the player their wiring. `bits` outside
  16/32 falls back to 16 the same way.

## The pane

Preferences gained a second pane, *Sample memory*, under the skin:

- **Release tails** — Auto / Stream / In RAM, with a note deriving what
  each means from the invariant (attacks and loops never stream).
- **RAM budget** — a MiB field, empty for half of physical RAM (the
  placeholder shows the number). Only Auto reads it.
- **Resolution** 16/32-bit and **Load cache** on/off.
- A status line: what the loaded organ actually does (`446 MiB
  resident, 839 MiB streaming (1575 of 1596 samples), 16-bit`).
- Because the engine's bank is fixed at construction, an edit cannot
  live-apply. The server keeps the preferences each bank was built
  under (`State::memory`); the snapshot carries both, and when they
  differ the pane shows *Changes apply when an organ loads* with a
  Reload button. Flipping streaming never re-decodes: one load cache
  serves both residencies (2026-09-02).

Edits go to `POST /api/prefs/samples` — the one server call
Preferences makes, and it touches the user config only. The
prefs-split audit now proves that: memory edits reach `/api/prefs`
and nothing else, the user config takes `[samples]`, the organ's file
does not.

## Deferred

- A per-organ override (this set streams, that one does not) belongs
  in the user config's per-organ table, not the organ file. Not built:
  nothing asks for it yet.
