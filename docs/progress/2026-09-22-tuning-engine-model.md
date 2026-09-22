# Tuning engine model: fifths-derived temperaments, periods and steps

Three commits: temperaments from their fifths, periods and inline steps, and
independent anchor/scale inheritance per scope (the last written separately,
after the first two).

## Commit 1 — temperaments from their fifths

`aristide-server/src/tuning.rs` no longer hand-transcribes deviation tables.
Each twelve-class temperament is a `FifthsDef` — 11 consecutive fifths (cents,
as a comma fraction narrowed from the pure 3/2 fifth) starting from a pitch
class — and `class_deviations` derives the per-class cents from that chain.
`Temperament::offsets_cents()` is unchanged in signature and in its existing
(A-referenced) output, so `tables_match_the_cbh_reference` still passes
unmodified. New temperaments: `meantone6`, `rameau1726`, `vallotti`, `young2`,
alongside the existing five, plus a `Custom([f32; 12])` variant.

`Tuning` gained `temperament_root: u8` (0 = C, the default) — the table
rotates via `rooted_offsets_cents()`, `deviation[pc] = table[(pc - root) mod
12]` — and `offset_cents: f64`, added to every key's deviation including
under `original`. Both are dormant under `Custom` (its table is already
absolute) and, `offset_cents` aside, away from 12-EDO.

**Sidecar fields** (`TuningConfig`, `TuningOverride`, `StopTuningDef`, and the
matching `[[manual]]` fields in `instrument.rs`), all optional and
`skip_serializing_if` their default:
- `temperament_root` — a pitch-class name ("D", "F#", "Bb"); omitted at C.
- `offsets` — `[f32; 12]`, only meaningful (and only written) for `custom`.
- `offset_cents` — cents; omitted at 0.

**API** (`POST /api/tuning`, any scope): `temperament=<id>` now 400s on an
unparseable name instead of silently keeping the old one; `root=<pitch class
name or 0-11>`; `offsets=<12 comma-separated cents>` (implies `custom`);
`offset_cents=<cents>`.

**Snapshot** (`TuningView`, every scope that carries one): `root` (0-11),
`offsets` (the resolved 12-class table in effect — rotated, custom, or the
measured home table under `original`; omitted away from 12-EDO or under a
scale/steps tuning), `offset_cents`.

**Generated catalogue**: `cargo test -p aristide-server
temperament_catalogue_matches_the_generated_json` writes
`desktop/src/tuning/temperaments.json` (`UPDATE_GOLDEN=1` to regenerate) as
`[{"id","name","offsets"}]` in menu order — the single source of truth the
desktop Tuning panel reads instead of a second hand-kept table.

**Migration**: none required. Every new field is optional and defaults to
today's behaviour; existing organ files round-trip unchanged (tested).

## Commit 2 — periods and inline steps

`Tuning::period: f64` (default 1200) generalizes equal division past the
octave: `edo` steps divide `period` cents, so 13 steps to a 3:1 tritave is
`edo = 13, period = 1901.955`. `Tuning::steps: Option<Arc<StepsTuning>>` adds
a fourth system — an inline pitch collection (`steps: Vec<f64>`, cents from
step 1; `period: Option<f64>`, `None` meaning no repetition at all, so a key
outside the collection sounds nothing, never wrapped or extrapolated;
`start_key: u8`, the key that plays step 1). The four systems — temperament,
equal division, steps, Scala file — stay mutually exclusive: naming one
clears the others, in both the API layer and the sidecar loader.

Priced the same way a Scala scale already was: an absolute target Hz, bent
from that key's *own* 12-EDO pitch (not the reference key's) — this is the
subtlety the first draft of `StepsTuning`'s pricing got wrong and the tests
caught: `deviation_cents(key)` is a distance from `equal_ladder_hz(key)`, so
two adjacent steps 150 cents apart differ in *deviation* by only 50 cents
(150 minus the semitone the ladder already accounts for).

**Sidecar fields**: `period` (shared between the two systems it can mean —
the equal division's period without `steps`, the steps collection's own
repeat interval with them), `steps` (array), `start_key` (a `KeySpec`, like
`reference_key`). Scala `.scl`/`.kbm` paths already accepted absolute paths
(no change needed there).

**API**: `edo=`, `period=<cents|none>` (a plain number for equal division;
`none`/`off` only valid — and only meaningful — with a steps tuning active),
`steps=<comma list>`, `start_key=<note name or MIDI number>`. A reference key
outside a non-repeating steps collection's range is rejected (400), both at
the API layer and (as a load-time warning, not a hard failure — a organ file
must still open) when loading a sidecar.

**Snapshot**: `system` (`"temperament" | "equal" | "steps" | "scale"`),
`period` (cents, or `null` under a non-repeating steps tuning), `steps`
(resolved intervals in cents from step 1 — the equal division's own ladder,
the steps collection verbatim, or for a scale `0` then its degrees before the
period), `start_key` (a steps tuning's own, or a scale's effective mapping
middle key). `ScaleView` also gained `middle_key` and `reference`, the
effective mapping's own (from the `.kbm` when one is loaded).

**Browse**: `GET /api/browse?kind=scala` now lists directories plus
`.scl`/`.kbm` files only. This is a **behaviour change**: the default (no
`kind`) used to list organs and Scala files mixed together, relying on the
client to filter; it now lists loadable organs only, as the endpoint's own
doc comment always claimed. Any caller that wants Scala files must pass
`kind=scala`.

**Audit**: `Tuning::steps_per_octave()` renamed to `steps_per_period()`
(same behaviour, extended to the steps system) — its one caller
(`aristide-server/src/http/organ.rs`, hex-layout presets) doesn't assume the
period is an octave, it just wants a step count.

**Migration**: none required; same as commit 1, every field is additive and
optional.

## Commit 3 — anchor and scale inherit separately

Every scope's tuning (instrument, set, division, stop, rank) is now two
halves: the **anchor** (reference key, reference Hz, `offset_cents`) and the
**scale** (temperament, root, offsets, edo, period, steps, start key,
scale/keymap, pipes). `Tuning::owns: Owns { anchor, scale }` records which
halves the holding scope owns; the instrument owns both. Storage stays one
`Option<Tuning>` per scope — the flags, not a second map, carry the split.

Resolution (`console.rs`, `resolve_owned`) walks the existing precedence
chain nearest first — rank, stop, then per the stop's pin/auto: division,
set; then the instrument — and takes the nearest scope owning the scale for
the intervals and the nearest owning the anchor for the pitch
(`Tuning::with_anchor_of`, which re-anchors a linear Scala mapping). One
owner of both is borrowed as before; a merge clones control-side only. The
reported scope is the nearest that owns either half. A division that owns
neither but transposes keeps its tuning for the transposer, as before.

**File.** A scope's fields decide what it owns at load: reference/offset
fields own the anchor, any scale field the scale. Writers write only the
owned half, so a Récit with its own meantone reads `temperament =
"meantone4"` and nothing about pitch, and keeps following the instrument's.
Existing files that name both kinds of field load as owning both: no
behaviour change. A scope that names only fields of one kind used to copy
the other half at load time; it now follows it live, which is the intended
semantics. Naming a temperament in a scope's table now also means 12 steps
to the octave, whatever division the scopes above play. `[[manual]]` now
carries root, offsets, offset, period, steps and start key through to the
loader (they were parsed but dropped).

**API.** `POST /api/tuning`: an anchor field makes the scope own its anchor,
a scale field its scale; `own=anchor|scale|both` adopts a half as it stands;
`follow=anchor|scale` returns one to the scopes above (owning neither
removes the scope's tuning). A bare scope request still takes the whole
tuning, and the existing `follow=` pins and `reset=1` are unchanged.
`start_key` now also places a Scala scale without a `.kbm` (its linear
mapping's middle key, kept when the reference changes). Choosing a
temperament resets the repeat to 1200 ¢, and `steps=` takes a `period=` in
the same request.

**Equal division.** `Tuning::equal_division()` is true for any count but 12
*or* for 12 steps to anything but 1200 ¢, so twelve steps to a 3:1 play as a
division instead of silently falling back to the temperament.

**`GET /api/tuning`** returns every scope the Tuning panel edits, resolved:
`{instrument: TuningView, manuals: [{idx, name, own, tuning}], stops: [{id,
name, midx, own, follow, scope, tuning, ranks: [{id, name, own, tuning}]}]}`
where `own` is `{anchor, scale}`.
`TuningView` itself gained `own`.

Validation: a console test drives a division owning only its scale through an
instrument pitch change, a division owning only its pitch, and a stop owning
only its scale at the division's pitch, comparing each against the same
tuning installed whole; an HTTP test on the demo organ covers ownership by
field, `own=`/`follow=`, what the file keeps, steps with no repetition,
twelve steps to a 3:1, and a stop owning only a fine offset; a unit test
prices 12 steps to a tritave.

Remaining gaps: sources have no UI; transposition is still one
field on a division's tuning rather than its own control; `.kbm` files carry
their own reference, which overrides the inherited anchor (as before).

## Summary of file fields (commits 1 and 2)

`[tuning]` (and `[[manual]]`, `[sources.<alias>.tuning]`, `[[tuning.stop]]`)
optional fields added: `temperament_root`, `offsets`, `offset_cents`,
`period`, `steps`, `start_key`. All omitted at their defaults; all existing
files parse and round-trip unchanged.

## API parameters added

`POST /api/tuning` (any scope): `root=`, `offsets=`, `offset_cents=`,
`period=`, `steps=`, `start_key=`. `GET /api/browse`: `kind=scala`.

## Snapshot contract additions (`TuningView`)

`root`, `offsets` (nullable/omitted), `offset_cents`, `system`, `period`
(nullable), `steps` (omitted for a plain temperament), `start_key`
(omitted outside steps/scale). `ScaleView` additionally carries `middle_key`
and `reference`.
