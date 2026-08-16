# 2026-08-12 — enclosures / swell boxes (gap analysis §1)

Research mandate ("lots and lots of active research"): three parallel
sweeps — swell-box acoustics (Pykett 2023 position-resolved spectra,
Braasch JASA 2008 10–20 dB measured range, Palo's 3-organ difference
spectra, slit-transmission literature), Hauptwerk archaeology (CODM
guide 6-param per-pipe shelf spec, EnclosurePipe table, inertia, frozen
releases), GrandOrgue source (GetAttenuation linear-amplitude floor,
one-block fader ramp, gain baked into detached releases, open issue
#717 asking for exactly a shelf filter). Written up with citations in
docs/research/enclosure-modeling.md.

Model: per-voice gain + one-pole high-shelf (same form as the
brightness tilt, hinged at the box corner). Broadband floor from ODF
AmpMinimumLevel, shelf −10 dB default, corner morphing 8 kHz→1 kHz
geometrically (slit cutoff ~1/opening); dB-linear pedal taper (HW's
law, NOT GO's linear amplitude); critically damped 2nd-order shutter
inertia (0.5 s default) engine-side; per-voice ~5 ms one-pole gain
de-zipper (a linear block ramp failed on giant offline blocks —
measure before theorizing paid off); release tails freeze box state at
pallet close. Loader reads [Enclosure]/windchest membership; console
maps expression CC (default 11) per manual via stop→rank→chest→box;
web console gets per-box sliders; sidecar [enclosures] exposes
floor_db/shelf_db/corners/taper/full_sweep_s/cc. Deferred, documented:
closed-box pressure rise into the wind model, multi-box windchests.

Verification: engine tests measure −14 dB low / −24 dB high band
closed (matches params), bit-identical tails under post-release pedal
moves, click-free sweeps; demo-set test pins ODF→spec→CC wiring.
Rendered takes measured 300 Hz −13.6 / 6 kHz −22.0 dB closed-vs-open,
taper −7.9 dB at half. RTF 0.02 on the 4-stop Récit chord (enclosure
cost invisible). 105 green (was 92). Takes: swell_ab, swell_sweep,
swell_release_freeze.
