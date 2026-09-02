# 2026-09-02 — Hauptwerk sets load (M7 opens)

The user asked to test a Hauptwerk organ. The picker had listed
`.Organ_Hauptwerk_xml` files since `beb4e98`, but nothing read them. Now
they load like any other set.

## Fixture

St. Anne's Moseley was the first idea and a dead end: it ships only inside
the 2.4 GB Hauptwerk installer. Grabowski's free sets go through a cart
and e-mail flow and start at 3.6 GB. The fixture is **AVO Solignac**
(Augustine's Virtual Organs, free, direct download): one manual and
pedal, eight stops, 407 pipes with three releases each, a waveform-driven
tremulant, noise ranks, and a second "extended" three-manual definition
over the same samples — plus a temperament file. Compacted (letter
columns) and CODM-generated, so it exercises the ugly path. Unpacked into
`testsets/avo-solignac/` (gitignored, 2 GB) with a throwaway `unrar`-crate
extractor, since the box has no RAR tool.

## Format notes first

`docs/hw-odf-notes.md`, written the way the GO notes were: from what
reads the format, not from memory. Sources: the GrandOrgue project's
OdfEdit converter and its column dictionary (verified against an
uncompacted definition from the HWtoGO test corpus and the compacted
Solignac file — including the finding that compaction skips the letter
`o` but uses `o1`), rusty-pipes' minimal reader, and the fixture itself.
Everything inferred from data rather than a converter is marked
UNVERIFIED there.

## Reader (`crates/aristide-formats/src/hauptwerk.rs`)

- Streams the XML with quick-xml into per-table rows, expanding compacted
  letters through an explicit per-table list generated from OdfEdit's
  dictionary (`hauptwerk_columns.rs`).
- Keyboards with console codes 1–5 become manuals (pedal = `ManualId(0)`);
  divisions with pipe stops but no keyboard get a floating manual.
- Stops sound ranks through `StopRank` rows of action type/effect 1/1;
  every other code is noise plumbing (key/stop/blower noises), counted
  into one warning. Division-to-rank offsets and ranges become
  `RankRange`s clipped to both compasses. Stops without rows follow their
  primary-rank hint.
- Pipes fold the per-pipe 64′ harmonic into `nominal_frequency_hz` (the
  mixtures break within a rank), the organ's dB trim and base pitch into
  gain/tuning, and declared sample pitches (key on a ladder, or exact Hz)
  into `midi_key_number` + fraction; undeclared ones defer to the `smpl`
  chunk, as GO sets do.
- Attack "highest velocity" / "least time since close" thresholds are
  turned into the model's lower bounds by ordering them; releases sort by
  hold bound with the unbounded (`99999`) one last.
- Windchests are minted per distinct (compartment, enclosures, tremulants)
  key of a rank's majority pipe; enclosure floors come from the median
  closed-shutter attenuation of the box's pipes.
- Tremulants: rate from the engaged frequency or the modulation waveform's
  loop length, depth measured from the waveform's amplitude channel
  (Solignac: 6.5 Hz, ~7 %). Alternate-rank tremmed re-recordings merge
  into the main rank as `wave_tremulant` variants and switch the tremulant
  to `Wave`.
- Couplers: conditional `KeyAction`s grouped by their switch.
- Refusals: non-XML definitions (compressed or encrypted), any sample
  needing a licence serial, a first sample that is not RIFF, missing
  installation packages (named).

`aristide_formats::load_set` dispatches on extension; the composite
loader, the server's load path, the browse endpoint and the inspect
example all go through it. `LoadResult` moved to the crate root (re-exported
from `grandorgue`). No console change was needed.

## Verified

- 11 unit tests on a synthetic compacted definition (manuals, shifted
  ranges, gaps, harmonics, thresholds, pitches, couplers, wind keys, noise
  counting, refusals) plus a fixture test on Solignac. Workspace green.
- Both Solignac definitions load headlessly; the three sample-less
  definitions from the HWtoGO corpus parse to the package check and fail
  with the right message; the two licensed demos are refused as encrypted.
- Audible verification is the user's, on the desktop.

## Deferred

Noise ranks (model has none), second-layer tremmed samples, expression
layers via `AmpLvl_ScalingContinuousControlID`, temperament files (a
natural fit for the Scala seam), Hauptwerk's wind-physics tables as input
to our wind model, `LoadSampleRange_*` codes (meaning unknown).
