# 2026-09-02 — the swell box pushes back, and boxes nest

The two pieces of enclosure work deferred when swell boxes shipped
(2026-08-12; `docs/research/enclosure-modeling.md` decisions 5 and 6):
a closed box now pressurizes under its own pipes' outflow, and a voice
can sit in more than one box.

## 1. Closed-box pressure rise

Hauptwerk exposes one number for this,
`WindModel_BoxPressureRisePctAtMaxLoadWhenClosed`, suggested 1–5 %:
"a very slight, but just discernible detuning when the box is fully
closed", with wind-robbing above that. It never says what the model
behind it is. Deriving it is short.

**The physics.** A swell box is a semi-sealed volume `V`, and the
pipes inside it exhaust into it — that is literally what Hauptwerk's
per-pipe *output compartment* is
(`docs/research/hauptwerk-wind-model.md` §1). Mass balance over the
box, linearized about the static pressure:

```text
(V/ρc²)·dδp/dt = Q_in − C(k)·δp
```

`Q_in` is the volume flow the sounding enclosed pipes push in — the
same wind draw the chest regulator already aggregates. `C(k)` is the
flow conductance *out*: the shutter gaps at opening `k`, plus the
box's own leakage (grille, joinery, the walls). That is a first-order
lag, and its steady state and its time constant are set by the **same**
conductance:

```text
δp∞ = Q_in / C(k)          τ = V / (ρc²·C(k))
```

Two consequences fall straight out and both are audibly right:

- The rise is a *closed-box* phenomenon. Modelling the conductance as
  `C(k) = C_closed·(1 + (R−1)·k)` — a swell front is square metres of
  opening against a few hundred square centimetres of residual gap, so
  `R = 100` is the conservative end — puts the steady rise at 50 % of
  its closed value with the shutters 1 % open and 9 % at 10 % open.
  Which is exactly why HW's parameter is named "…WhenClosed".
- Cracking the shutters both collapses the rise *and* dumps it fast,
  because `τ` scales the same way. The detune vanishes the instant the
  player opens the box.

**What the pipe feels.** A pipe speaks on the *difference* between its
chest and its mouth. Its mouth is inside the box. So a pressure rise
inside the box is a pressure **loss** for the pipe — the same physical
quantity the wind model already maps to pitch, gain and brightness
through `pitch_exponent` / `gain_exponent` / `brightness_exponent`. It
is therefore not a new modulation shape and does not get one: it enters
`WindState::follow_chest` at exactly the point per-pipe flow noise
does, as one signed pressure deviation feeding all three exponents.
Downstream, the per-voice speech lags from the tremulant work
(`docs/progress/2026-08-27-tremulant-physics.md`) do the rest for
free: pitch answers within a few speaking periods, amplitude and
timbre only over the pipe's speech time. Released voices freeze their
factors as before — `follow_chest` is a Held-only call — so a tail
never picks the detune up or puts it down. Lite mode steps neither the
chests nor the boxes, so it skips the leg entirely.

**Magnitudes, as a sanity check on HW's 1–5 %.** A 30 m³ box, a
division pushing ~0.1 m³/s, and 2 % of an 800 Pa chest (16 Pa) imply a
residual leak area of ~190 cm² — 16 Pa drives roughly a 5 m/s jet
through it, and 190 cm² is a plausible swell front's worth of gaps.
The same numbers give `τ = 2V·δp∞/(ρc²·Q_in) ≈ 0.07 s`; across
small-box/light-registration to big-chamber/full-organ the band is
roughly **0.03–0.5 s**. This is notably faster than the 0.5–2 s one
might guess: air fills a box quickly, because the pneumatic compliance
of even a large box is small next to the flow a division moves. The
default `fill_seconds` is **0.25 s** — mid-band and deliberately on
the slow side, so the detuning swells in rather than snapping on.

Sidecar: `[enclosures] pressure_rise_pct`, default **2** (HW's
midpoint), 0 disables, clamped 0–20 (past HW's suggested band is a
legitimate if extreme thing to ask for — that is where a box starts
robbing its own wind). It applies to every box the set defines, like
the rest of `[enclosures]`; there is no per-box override because the
section has never had per-box keys, and the console has no editor for
box *acoustics* at all (it edits box *membership*), so adding one knob
there would mean building that whole popover. Sidecar-only for now.

## 2. Multi-box windchests

A voice used to carry one enclosure index; a chest in two boxes logged
a warning and kept the first. But boxes genuinely nest — a Solo or
Echo box standing inside the Swell is ordinary English and American
practice — and both formats already say so:

- GrandOrgue's `[WindchestGroupNNN]` carries
  `NumberOfEnclosures`/`EnclosureNNN` and multiplies all of them
  (`GOWindchest::UpdateVolume`); our GO reader already parsed the full
  list.
- The Hauptwerk reader keys a windchest by its whole *sorted* enclosure
  set (`hauptwerk.rs`, `WindKey`), so a pipe with several
  `EnclosurePipe` rows already produced a multi-enclosure chest.

Both were being thrown away one stage later, in
`server/bank.rs::resolve_chest_enclosures`. Now a voice carries a fixed
`[u8; MAX_VOICE_ENCLOSURES]` (2 — two deep is the realistic maximum)
and every leg cascades: gains multiply, shelves filter in series, each
box keeps its own ~5 ms de-zipper, all slots seed from current box
state at note-on and all freeze together at release. Unused slots are
*skipped*, not filled with a transparent box — an open box's shelf is
`lp + 1.0·(x − lp)`, which is not bit-identical to `x` in floating
point, so filling it would have quietly changed every single-box
organ. Duplicate memberships are dropped so a box listed twice cannot
attenuate twice; memberships past the second are dropped with a load
warning.

The console's expression pedal follows: `map_enclosures` took only the
first box per chest and now takes them all, so a manual whose stops
stand in a box inside a box drives both from one pedal.

**Nesting and pressure stack additively.** The inner box vents into
the outer one, so the inner box's own rise is measured *relative* to
the outer box's; referenced to the room, a pipe in the inner box sees
the sum along the chain. Aggregation matches: a voice's draw counts
toward every box it sits in, because flow is conserved down the chain
— what the inner box vents is what the outer box receives. (The box
sum deliberately does **not** include the pallet-gulp attack boost the
chest sees: that transient fills the pipe's foot, it does not come out
of the mouth.)

Neither fixture nests. The GO demo's four chests carry 0, 1, 1 and 0
enclosures; AVO Solignac — a small unenclosed chamber organ — defines
**zero** enclosures at all (`Enclosure` and `EnclosurePipe` object
lists are both empty in both of its ODFs). So the loader side is
proved on a synthetic organ instead.

## Verification

All offline renders through `Engine::process`, measured (headless box,
no audio device).

Pressure rise — one 400 Hz pipe plus two silent load voices, chest
regulator disabled so the box is the only pressure in play, demand =
the box's full-load reference:

| measurement | result |
|---|---|
| closed box vs open, default 2 % | **−1.04 cents** (theory −1.11 from `1200·log2(1 − 0.032·0.02)`; the render window opens at 0.5 s, one τ short of settled) |
| knob at 0 % | 0.000 cents — inert |
| open box's held rise | 0.0002 of static, 1/100 of closed, exactly as `SHUTTER_VENT_RATIO` says |
| rise after one τ | 0.012642 of 0.02 = **63.212 %** — `1 − e⁻¹` to five digits |
| shutters cracked to 10 % open | under 15 % of the settled rise within 100 ms |
| nested (two closed boxes) | −2.085 cents vs −1.031 for one: **2.02×** |

Nesting, acoustic legs — one voice in two boxes, 100 Hz band:

| measurement | result |
|---|---|
| either box shut alone | −14 dB (= `floor_db`) |
| both shut | −28 dB — the sum in dB |
| tails under post-release pedal moves on both boxes | bit-identical |
| both pedals sweeping at once | max sample step under 1.3× the steady signal's — click-free |
| moving a box the voice is not in | bit-identical |

Loader: a synthetic organ with chests in 0, 1, 2, duplicate and 3
boxes resolves to the right slot arrays, warns exactly once, and drops
the duplicate.

Workspace: 359 tests pass (74 engine, 161 server, 81 formats, 43
model), clippy clean against the same baseline as before. The golden
bit-exactness hash was rerecorded: its script has two enclosed
wind-drawing voices and closes the box mid-render, so the new pressure
leg legitimately changes the output. The **lite** hash is unchanged, as
it must be — lite steps neither chests nor boxes.

## Deferred

- No per-box `[enclosures]` overrides, and no console editor for box
  acoustics (`floor_db`, `shelf_db`, corners, taper, sweep, rise).
  The console edits box *membership* only; a box-acoustics popover is
  its own piece of work.
- `fill_seconds` and the shutter vent ratio stay engine constants. The
  literature has no measurement for either; exposing knobs nobody can
  calibrate would be noise.
- Three or more nested boxes are dropped with a warning. No format
  forbids it; no instrument does it.
- Reeds should respond to a box's overpressure mostly in amplitude and
  timbre rather than pitch (`organ-wind-acoustics.md` §4), like they
  should for the chest. That split still waits on rank classification.
