# Enclosures / swell boxes: acoustics, prior art, and Aristide's model

Research pass 2026-08-12 (three parallel sweeps: swell-box acoustics
literature, Hauptwerk documentation archaeology, GrandOrgue source
reading), feeding the M-series enclosure implementation. GO is the
floor; HW is the baseline to beat.

## What a swell box physically does

Measured sources: Pykett 2023 ("Swell boxes and swell pedals", parts
1–2, colinpykett.org.uk — the only published position-resolved
spectra); Braasch 2008 (JASA 123(3):1683, the one peer-reviewed
measurement set); Lars Palo's three closed-vs-open difference spectra
(familjenpalo.se/vpo/swellbox-modeling/); Tickel 2019 (J. American
Organbuilding 34(1), builder practice).

1. **The closed box is a low-pass filter, not a volume knob.** Pykett's
   filter fit: corner ≈ **160 Hz**, attenuation growing above it to
   **~35 dB at 8 kHz**; near-zero attenuation at 30 Hz (fundamentals of
   big pipes pass through the walls almost unhindered — mass-law
   transmission rises ~6 dB/octave). Braasch measured total swell-box
   dynamic range **10–20 dB** around 2 kHz. Palo's three organs:
   10–20 dB broadband, each box different, none monotonic (mid dips,
   re-rise above 10 kHz — coincidence + leakage break the pure mass
   law). Schroeder frequencies of two real swell boxes: ~250–265 Hz
   (Inspired Acoustics, PAB organ) — above that the box interior is a
   diffuse field, licensing a single filter per box.
2. **Attenuation vs. position is front-loaded near closed.** Pykett:
   "the sound level jumps abruptly as the shutters move from k=0 to
   k=0.1", most pronounced at high frequencies (slit acoustics: a
   nearly-closed slit passes low-mid freely — Gomperts & Kihlman 1967,
   Trompette 2009 — while the felted joint is "effectively soundproof
   towards the higher frequencies"). Builders *compensate* with staged
   or exponential pedal schedules so perceived loudness spreads evenly
   over pedal travel (Tickel; Peterson servo units).
3. **Shutters have inertia and often stages.** Electric actions move in
   discrete stages — **16 is the common number** (Pykett, Tickel;
   Peterson RC-150: 8 or 16 stages); mechanical linkages are continuous
   and remain the gold standard. No published full-sweep times; slew is
   limited by shutter mass (Pykett part 2). Order ~0.5 s full sweep is
   a defensible engineering guess, flagged as such.
4. **The recorded release tail is room decay.** Once the pallet closes,
   the sound the tail represents has already left the box; moving the
   shutters afterwards cannot un-radiate it. Tail state must freeze at
   key-off (both HW and GO agree — below).

## What GrandOrgue does (the floor)

Source read directly (github.com/GrandOrgue/grandorgue @ master
2026-08): `GOEnclosure::GetAttenuation()` =
`(MIDIValue·(100−AmpMinimumLevel) + 127·AmpMinimumLevel)/12700` —
**linear amplitude** between `AmpMinimumLevel`% and 100%. Multiple
enclosures on a windchest multiply (`GOWindchest::UpdateVolume`). The
product lands once per audio block in `GOSoundWindchestTask`, applied
with a per-sample linear gain ramp within the block
(`GOSoundFader::Process` — the `EXTERNAL_VOLUME_CHANGE_FRAMES = 1024`
slew is defeated by integer division; the target is reached in one
block). **No filtering of any kind**; no slew of the control value.
Releases: at key-off the sampler moves to a "detached windchest" with
the enclosure gain **baked into** `gain_target`
(`GOSoundSamplerPlayer.cpp:316-343`) — GO freezes tails too.
MIDI: CC/RPN/NRPN → linear rescale of a configured range to 0–127,
no ramping. ODF: `[Enclosure]` = `Name` + `AmpMinimumLevel` (0–100) +
`MIDIInputNumber`; windchests list member enclosures by global index.
GO's own issue **#717 (open since 2021)** asks for exactly what we're
building: maintainer consensus there is a shelf filter ("That's also
exactly what HW uses" — larspalo, Feb 2026), still unimplemented.

## What Hauptwerk does (the baseline)

Official docs (HW4/HW6 Features Data Sheets; CODM User Guide HW9 p.13;
User Guide 5.0.1 pp.213–214, 245–246) plus Martin Dyde forum posts
(forum.hauptwerk.com t=20783, t=15901):

- **Per-pipe gain + shelf filter** ("specially-designed high-speed
  filters ... each enclosed virtual pipe separately in real-time";
  "essentially shelf filters" — Dyde). Deliberately first-order-simple
  for CPU; each per-pipe filter costs HW ~25–30% polyphony.
- **Six producer-authored numbers per pipe** (`EnclosurePipe` table;
  CODM generates them from anchor notes 36/96): closed overall
  attenuation dB, closed peak/trough Hz, extra dB at trough; open
  peak/trough Hz. **Linear interpolation in shutter position**;
  attenuations reach zero at fully open; peak/trough may not cross.
  Suggested LP starting values: trough 8 kHz open → 1 kHz closed,
  ~10 dB extra at the trough; producers fit against real open/closed
  recordings (St Anne's methodology), then tune by ear.
- **Filter never fully bypassed when open** (avoids a discontinuity
  when a rank is enclosed vs not — Dyde, t=15901).
- **Shutter inertia** modeled on the pedal→shutter linkage
  (accelerating + damping coefficients in the full format ODF —
  a second-order smoother; constants not public; CODM: "configured
  automatically").
- **Releases unaffected by shutter movement after key-off** (Features
  Data Sheet wording; mechanism per-voice frozen state, inferred).
- **Closed-box pressure rise** via the wind model
  (`WindModel_BoxPressureRisePctAtMaxLoadWhenClosed`, 1–5% suggested:
  "a very slight, but just discernible detuning when the box is fully
  closed", wind-robbing at higher values).
- Per-pipe voicable swell **amplitude depth and harmonics depth**
  (± percent, polarity invertible, per output perspective).

## Aristide's model (decisions, each grounded)

Signal path: **per-voice**, like HW, not a per-enclosure bus — (a)
release tails must freeze their enclosure state at key-off, which
requires per-voice state; (b) voices already carry a per-voice tilt
filter (`brightness_a`), so the machinery and its cost model are
proven; (c) no bus infrastructure exists and building one for this
would couple enclosures to the future M6 routing work.

1. **Control law: dB-linear in shutter position** (HW's rule;
   perceptually even, which is what builders engineer toward), NOT
   GO's linear amplitude. The raw physics is front-loaded near closed,
   but every real installation inserts a compensating pedal schedule —
   modeling physics-then-compensation would be two curves canceling.
   A sidecar `taper` exponent lets a set lean either way.
2. **Two-part attenuation:** broadband `floor_db` (from the ODF's
   `AmpMinimumLevel`, e.g. 20 → −14 dB; measured range 10–20 dB
   supports trusting the set) plus a **high-shelf leg** `shelf_db`
   (default −10 dB closed, per HW's worked example and Pykett's
   high-frequency excess) whose corner morphs **8 kHz open → 1 kHz
   closed** (HW CODM starting values; Pykett's 160 Hz corner is the
   fully-closed asymptote of a much deeper model — 1 kHz at −10 dB
   approximates his family over the audible range without a second
   filter). Filter: one extra one-pole per channel per voice,
   `out = lp + hi_gain·(x − lp)` — same form as the brightness tilt,
   hinged at the box corner instead of the pipe's 2nd harmonic.
3. **Shutter inertia engine-side:** critically-damped second-order
   slew (the wind regulator's integrator form, reused) with
   `full_sweep_s` default 0.5; smooths MIDI CC jumps AND provides the
   anti-zipper ramp (factors recomputed per block, gain interpolated
   per frame like GO's fader). 16-stage quantization deliberately NOT
   modeled by default (mechanical action = continuous is the better
   sound; a sidecar knob can add it later if a set wants electric
   character).
4. **Release tails freeze** their enclosure gain and filter state when
   the release actually fires (post pallet-stagger) — physically right
   (the tail is room decay that already left the box) and what both HW
   and GO ship. Frozen ≠ unfiltered: a tail released with the box
   closed keeps the closed-box filter forever.
5. **Wind coupling (closed-box pressure rise):** deferred. HW's 1–5%
   "just discernible" detuning needs the enclosure to be a node in the
   wind topology; our wind groups are per-ODF-windchest and the
   audible payoff is marginal. Documented as future sidecar work
   alongside inter-chest coupling (see gap analysis §10).
6. **Multiple enclosures per windchest:** GO multiplies attenuations;
   real sets almost never use it (demo set: one per chest). A voice
   carries ONE enclosure index; extra memberships log a load warning.
   Revisit if a real set needs it.
7. **MIDI:** expression CC (default CC11, GO-convention `swell` also
   accepts CC7 per sidecar) on the channel of the enclosure's manuals;
   sidecar can pin `cc` and `channel` per enclosure. HTTP console gets
   a slider per displayed enclosure.

Tunable constants exposed in the sidecar (`[[enclosure]]`): `floor_db`
(overrides ODF AmpMinimumLevel), `shelf_db`, `corner_open_hz`,
`corner_closed_hz`, `taper`, `full_sweep_s`. Defaults above; the
user's ear is the spec for all of them.

## Citations

- Pykett, C.E. (2023) "Swell boxes and swell pedals" parts 1–2,
  colinpykett.org.uk/swell-boxes-and-swell-pedals-part1.htm, -part2.htm
- Braasch, J. (2008) "Acoustical measurements of expression devices in
  pipe organs", JASA 123(3):1683–1693, doi:10.1121/1.2828062
- Palo, L. "Swellbox modeling", familjenpalo.se/vpo/swellbox-modeling/
- Tickel, J. (2019) "Swell Box Design and Construction, Part One",
  J. American Organbuilding 34(1) (leekpipeorgans.com PDF)
- Gomperts & Kihlman (1967) Acustica 18(3):144; Trompette et al. (2009)
  JASA 125(1):31 — slit/aperture transmission loss
- Inspired Acoustics, "Acoustics of the PAB organ" (Schroeder
  frequencies of real swell boxes)
- Hauptwerk CODM User Guide (HW9) p.13 + v2.00 pp.63–74; HW User Guide
  5.0.1 pp.213–214, 245–246; HW4/HW6 Features Data Sheets ("Realistic
  swell boxes"); HW 9.0.1 Release Notice pp.28–29 (model quality tiers)
- forum.hauptwerk.com t=20783 (EnclosurePipe table, "essentially shelf
  filters"), t=15901 (never bypassed when open), t=15547, t=15674
- GrandOrgue source: GOEnclosure.cpp:86-90, GOWindchest.cpp:83-88,
  GOSoundWindchestTask.cpp:37-53, GOSoundFader.cpp:29-120,
  GOSoundSamplerPlayer.cpp:316-343; issues #717 (open: swell filtering),
  #1813/#1910, #2494/#2496
