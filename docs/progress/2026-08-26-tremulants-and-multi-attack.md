# 2026-08-26 — tremulants come alive; attacks stop machine-gunning

Gap-analysis §2+§4, the "sets don't sound right" residue and the
re-verified top priority.

## ODF tremulants (§2)

- `[TremulantNNN]` sections parse into the model (`Tremulant` /
  `TremulantKind`), windchests carry their tremulant membership, and
  composites renumber both exactly like enclosures. Corrected
  `docs/go-odf-notes.md` from GO source: **`Period` is milliseconds
  per cycle** (`trem_freq = 1000/period`), `AmpModDepth` is percent
  amplitude, `StartRate`/`StopRate` are `1/rate`-second ramps
  (GOSoundProviderSynthedTrem.cpp).
- Synth tremulants drive the existing wind-model tremulant on exactly
  the chests the ODF names: rate = 1000/Period Hz, pressure depth
  inverted through the wind gain exponent so the author's amplitude
  depth comes out (FM + brightness then follow physically — better in
  kind than GO's AM-only synth trem), ramp = the two GO ramps averaged.
  The demo set's Tremblant (196 ms ≈ 5.1 Hz, Récit chest only) now
  works out of the box — previously the sidecar default swept every
  chest at 5 Hz.
- Wave tremulants switch recordings instead of modulating:
  `Command::SetWaveTremulant` flags the chest, note-ons prefer
  `IsTremulant=Y` attack variants, and held voices' note-offs select
  matching releases (state captured per voice, tails keep what they
  released under).
- Tremulants are per-control now: `State.trems`, `tremulant:<name>`
  binding vocabulary, `/api/trem?idx=N&on=1`, `"trems"` in the state
  JSON. Bare `tremulant` (and the console's single knob) toggles all.
- Precedence: a hand-written sidecar `[tremulant]` **replaces** the
  set's tremulants (it became `Option` to make presence detectable);
  no ODF trems and no sidecar keeps the old default-tremulant
  fallback. The demo sidecar's stale section (predating ODF support,
  wrong chest) became a commented example.

## Multi-attack selection (§4)

- Every attack variant decodes into the bank (was: `attacks.first()`,
  rest parsed-then-ignored). `LoadedBank.attack_options` carries GO's
  selectors per pipe; borrowed pipes inherit their target's table.
- Console-side `GetAttack` at every voice pricing site (note-on, stop
  drawn mid-hold, recouple): candidates filtered by `IsTremulant`
  tri-state vs the chest's wave-trem state, `AttackVelocity ≤ press`,
  and `MaxTimeSinceLastRelease` against a per-pipe last-release clock;
  most-specific wins (highest velocity bound, then tightest re-attack
  window), ties rotated by xorshift so repeated notes stop
  machine-gunning one transient recording.
- Separate releases now attach to *each* attack variant, with their
  own trem tri-state, selected engine-side by (trem state, hold time).

## Verification

Tests: tremulant/windchest/IsTremulant parsing; composite carry;
adoption equivalence extended to tremulants (demo: 1 trem, group 2,
5.1 Hz both ways); engine `releases_select_by_wave_trem_state`
(mid-hold engage flips the chosen release); console selection tests
(velocity bound beats re-attack window beats plain; tremmed variant
under the trem; random rotation among equals; rate factor follows the
variant). Workspace green.

## Deferred, named

- **Mid-hold wave-trem attack switch** (GO `SwitchToAnotherAttack`
  crossfade): engaging a wave trem doesn't make already-held notes
  undulate until re-pressed — synth trems (the demo, most free sets)
  are unaffected. Needs an engine crossfade-to-other-sample's-loop path.
- Multi-tremulant console UI (single knob still toggles all; named
  bindings/API reach one). Screenshot-harness work.
- GO's per-file tuning of additional attacks (variants assumed at the
  primary's recording pitch; rate follows file sample rate only).
- Release random tie-break among equal `MaxKeyPressTime` (we take the
  first; GO rotates).
