# 2026-09-02 — the combination action, whole (gap §7 residue)

Generals and the setter landed on 2026-08-26 with everything else in
the combination action named as deferred: divisionals, the
stepper/sequencer, the crescendo, GO's `DivisionalsStore*` semantics,
and a console piston rail. All of it is here, plus the bug that made
the generals themselves half-real.

## Where a combination lives, and the bug that found it

Generals were stored in the **per-organ user config**. Every organ is a
composite with an organ file (locked 2026-08-19), and `State::install`
replaces that organ's whole `OrganConfig` with what the file's `[midi]`
section says — wholesale, because the file is the organ's authority for
wiring. The file carried no generals, so **every reload silently wiped
every stored general**. Nobody had noticed because the combination
action had no UI yet.

The rule (DESIGN.md, 2026-08-30/09-01) settles the fix rather than
inventing one: a combination is a *player-usage* fact about this
instrument, exactly like the MIDI wiring, so it belongs in the organ's
own file and is accepted even on an adopted set's own organ. New
section, written comment-preservingly beside `[midi]`:

```toml
[combinations]
divisional_tremulants = true    # optional; overrides what the set says

[[combinations.general]]        n = 1, stops/couplers/tremulants
[[combinations.divisional]]     manual = "Récit", n = 1, …
[[combinations.frame]]          n = 1, …           # the stepper
[[combinations.crescendo]]      stage = 1, stops   # the pedal
```

Everything is stored by **name** — the bindings' text vocabulary — and a
name this organ hasn't got is reported and skipped at recall, never
dropped from the file: the stop it names may well come back.

## The semantics, and where they come from

**Divisionals** (`divisional:<manual>:<n>`) recall a registration scoped
to one division. A divisional always reaches its own stops; whether it
also moves that division's couplers and tremulants is a *wiring* choice,
and consoles differ. GrandOrgue states the answer in the ODF's `[Organ]`
header, so we read it: `DivisionalsStoreIntermanualCouplers`,
`...IntramanualCouplers`, `...Tremulants` now parse into
`aristide_model::CombinationScope` and are honoured (they were
previously listed in docs/go-odf-notes.md as "combination-system only",
which is true for GO's *playback* and false for ours). A coupler belongs
to the manual its routes read *from* (GO files them under that manual);
it is intermanual when a route lands on a different manual
(`GOCoupler::IsIntermanual`). A tremulant is a division's when it blows
on wind that division's own pipes stand on. GO's defaults are all
`false`, and so are ours when nothing says otherwise; the organ file may
override each flag by hand and we never rewrite those lines. The GO demo
set says `Inter=Y, Intra=Y, Trem=N`, which the tests pin.

**The stepper** is a list of general-shaped frames with
`stepper:next|prev|goto:<n>|store|insert`. Two decisions worth stating:

- The ends are walls, not wraps. A sequencer that looped to frame 1
  mid-piece is a trap, and no console does it.
- `stepper:store` is its own action rather than "Set + the stepper's own
  piston". The stepper's advance piston is pressed constantly while
  playing; giving it a second, destructive meaning under an armed setter
  would overwrite a frame every time a player armed Set and then reached
  for the next registration. `stepper:insert` grows the sequence
  forwards — insert after the current frame, store into it, land there —
  so a piece's registrations are built in the order they are played.

**The crescendo** is 32 stages above the heel (GO's `CRESCENDO_STEPS`,
which is also the order of magnitude a real crescendo roller's contact
bank affords), and stage N sounds the union of stages ≤ N. Crucially it
is an **additive overlay**, not a registration: the hand keeps whatever
the drawknobs say, what sounds is hand ∪ crescendo, and rolling the
pedal back removes only what the pedal added — a stop the hand also drew
stays. One boolean per stop cannot express that, so `Console` now keeps
two layers (`hand`, `crescendo`) whose union is the `drawn` set every
sounding path already reads; the sounding path itself is untouched.
GrandOrgue reaches the same result from the other end, by making every
drawstop an OR over *named* internal states with the crescendo owning
one (`GODrawstop::SetInternalState`).

Consequences we chose deliberately:

- **Cancel clears the hand, not the pedal.** Cancel is a thumb on the
  jamb; it cannot move a foot. A crescendo past the heel keeps speaking
  what it holds, shown as the pedal's doing. Bring the pedal home and
  the organ is silent.
- **Storing a stage captures the drawknobs**, not what is sounding.
  Storing the sounding set while the pedal stands on that same stage
  would fold the overlay into itself and ratchet the stage upwards.
- **The gesture is Set + `crescendo:<stage>`** — the same arm-then-press
  idiom as a general, so there is one thing to learn — and deliberately
  *not* the pedal: a foot sweeping through the stages must never write.
- **No default CC.** GO has no convention for the crescendo shoe (unlike
  expression, where 11 is ours), so `crescendo` is bound by MIDI learn
  like any other continuous control. The full travel maps end to end:
  value 0 → heel, 127 → stage 32.

Generals and divisionals capture what is *sounding*, crescendo included
— GO's `FillWithCurrent` reads a drawstop's engaged state the same way,
and it is what the player means by pressing Set: this, please, again.

## The console

A `Combinations` panel on the panel canvas (layout in the organ file's
`[console.layout]` like every other panel): eight general pistons — the
common thumb bank, extended by any higher slot already stored into —
then Set (a latch, lit while armed), Cancel, the stepper's `‹ 3/8 ›`
with Store/+/−, and the crescendo pedal as a horizontal fader with its
stage readout. Divisionals sit **with their manual**, a row of six under
each keyboard panel, which is where a console puts them. The rail is
built like the coupler rail rather than offered in the double-click add
menu: every organ has a combination action, and a panel that can't be
absent doesn't belong in a menu of things to add.

Every piston, Set, Cancel, each stepper button and the crescendo pedal
carries `data-action`, and right-clicking one (edit mode, or ctrl
through the lock) opens a one-row popover holding the existing
quick-bind row — so the player's own MIDI piston ends up doing exactly
what the button on screen does, through the same machinery the stop and
coupler editors use.

A crescendo-held stop the hand hasn't drawn renders **lit but not
drawn**: the dark face of an undrawn knob, ringed and lettered in the
accent. The drawknob really is still in, and the pedal is about to take
the stop away again; claiming the player drew it would be a lie the
jamb tells at a glance. The snapshot carries `cres` and (only where the
two layers disagree) `hand`, so the UI never guesses.

## Verification

- Server round trips on the demo set (`bindings.rs`): divisionals stay
  inside their division; the demo's own `Inter=Y/Intra=Y/Trem=N` is
  honoured both ways (a stored intermanual coupler returns, the
  tremulant is left alone; with the flags flipped, the division's
  tremulant returns and its coupler is not stored); the stepper stores,
  inserts, walks and stops at both ends; the crescendo adds over the
  hand and takes back only its own; Cancel leaves the pedal alone;
  storing a stage captures the drawknobs; an unresolvable stored name is
  skipped and stays in the file; a CC sweeps 0 → 16 → 32.
- Persistence (`config.rs`): every general, divisional, frame and
  crescendo stage round-trips through the organ file, comments and a
  hand-written `divisional_tremulants = true` intact; frames read back
  in their numbered order, not their file order.
- Snapshot shape (`http/mod.rs`): the endpoints move the console and the
  JSON carries `generals`, `setter`, `combinations.{frame, frames,
  crescendo, crescendo_stages, divisionals}` and the per-stop
  `cres`/`hand`.
- End to end (`tools/e2e/combination-audit.js`, 29 checks, all green):
  a real server on a null ALSA device, the real console UI, real CDP
  pointer input — the rail exists with its controls, Set + a general
  stores, Cancel wipes, the general recalls, a divisional stays in its
  division, dragging the crescendo lights a stop as crescendo-held and
  rolling back takes only that away, the rail is mutation-free over a
  second of polling, and a 300 ms press on a piston's own numeral lands
  (the poll-DOM-churn invariant).

## Deferred, named

- GO's `GeneralsStoreDivisionalCouplers` and
  `CombinationsStoreNonDisplayedDrawstops`: we have neither divisional
  couplers nor non-displayed drawstops yet.
- Crescendo *banks* (GO has four, A–D) and a per-bank override mode
  (where a stage replaces the hand rather than adding to it). One
  additive crescendo is what a real console has.
- A UI for the three `divisional_*` flags — they come from the set, and
  the organ file overrides them by hand.
- Reordering stepper frames from the console (the file's `n` does it).
- Divisional pistons for a *floating* division, which we don't have.
