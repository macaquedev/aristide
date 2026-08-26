# 2026-08-26 — voicing trims and the first combination action (gap §7)

The last "blocks real use" package. Core landed; console UI panels
deferred to the screenshot-harness workflow.

## Voicing trims

`[[voicing.adjust]]` in the sidecar: `gain_db` and `cents` by stop
pattern (`stops = ["Trompette*"]`), resolved with the same
name-pattern rules as routing and stamped onto every voice the stop
prices. Cents ride the same pitch fold as tuning — wind draw and the
brightness hinge follow the sounding pitch, so a trimmed stop still
breathes correctly. Deferred: pipe-scope (key-range) voicing, a
brightness/EQ leg, live HTTP adjustment (the sidecar is load-time).

## Generals + setter

- `general:<n>` bindings (and `POST /api/general?n=`) recall a stored
  registration: stops, couplers and tremulants each *diff* to the
  stored state — landing on held keys immediately, as pistons on an
  electric action do (the existing set_drawn/set_coupler machinery).
- `set` arms the setter; the next general press stores the current
  console and disarms, exactly as a console's Set piston works.
- Stored as names (the bindings text-vocabulary rule) in the per-organ
  user config: a stored name the loaded organ hasn't got is reported
  and skipped, never dropped from the file. `"generals"`/`"setter"`
  ride the state JSON for the future piston rail.

Tests: store→wipe→recall round trip on the demo set (stops + coupler
+ tremulant all return; storing disarms the setter); voicing trim
lands on priced voices (gain and rate). Deferred, named: divisionals,
stepper/sequencer, crescendo, GO `DivisionalsStore*` semantics,
console piston rail and voicing editor.
