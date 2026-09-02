# Hauptwerk organ definitions (`.Organ_Hauptwerk_xml`) — Playback Reference

Notes for the `aristide-formats::hauptwerk` reader, compiled in September 2026 from
real definitions and from the two open-source projects that read the format:

- **OdfEdit** (GrandOrgue project, GPLv3, `github.com/GrandOrgue/OdfEdit`): its
  Hauptwerk→GrandOrgue converter (`src/OdfEdit.py`, functions prefixed `HW_ODF_` and
  `GO_ODF_build_`) and its column dictionary `src/resources/HwObjectsAttributesDict.txt`.
  Cited below as *OdfEdit* with the function name.
- **HWtoGO** (`github.com/ahall41/HWtoGO`, GPLv2): its `Test/ODF/` folder holds five
  real definitions, one of them uncompacted (Prospectum's *Skrzatusz*), which is what the
  column dictionary was checked against.
- The set this reader was written on: **AVO Solignac** (Augustine's Virtual Organs, free,
  Hauptwerk 4.2 format, CODM-generated, compacted), `testsets/avo-solignac/`.

Milan Digital Audio's own documentation (the CODM user guide, `downloadhauptwerk.com`
PDFs) was not reachable from the build machine, so nothing below is cited to it. Where
the meaning of a column is inferred from data rather than read from a converter that
handles it, it is marked **UNVERIFIED**. Hauptwerk itself is closed source; every
statement of "what Hauptwerk does" is really "what the converters do with it".

**Legal boundary.** Encrypted sets (Hauptwerk 5+-era commercial sets, and some older
ones) are refused: their definition is not XML and their samples are not WAV. The reader
sniffs both and stops (§1, §9). No decryption, ever.

---

## 1. File basics

**Container.** A package is a folder tree:

```
<root>/OrganDefinitions/<name>.Organ_Hauptwerk_xml       the definition
<root>/OrganInstallationPackages/<id, 6 digits>/...      samples, images, info
<root>/Temperaments/<name>.Temperament_Hauptwerk_xml     optional
```

Hauptwerk installs everything into one shared tree; a set downloaded as a
`.CompPkg.Hauptwerk.rar` (a plain RAR, v4 in the sets seen) unpacks to the same three
folders. The reader takes the definition's path and walks up at most four levels to the
first folder containing `OrganInstallationPackages` (`package_root`); that folder is the
organ's `base_path` and every sample path is relative to it.

**Syntax.** XML, declared UTF-8, one root `<Hauptwerk FileFormat="Organ"
FileFormatVersion="4.20014">`. Under it, one `<ObjectList ObjectType="T">` per table,
one child element per object (its tag is the table name again, or the object type's
name), one child per column. There are no attributes below `ObjectList`. Empty columns
appear as `<x></x>` or `<x/>`; absent columns mean "default".
Source: every definition inspected; the Solignac and Skrzatusz files show both spellings.

**Compaction.** `_General.Control_FileIsCompacted_AlwaysSetThisToNIfEditingManually=Y`
means every column of every table except `_General` is renamed to a letter: `a`…`z`
**skipping `o`** (25 names), then `a1`…`z1` *including* `o1` — that asymmetry is real
(`<o1>` occurs 7,000–66,000 times across the HWtoGO test files, `<o>` never), so the
reader carries an explicit per-table letter→name list (`hauptwerk_columns.rs`, generated
from OdfEdit's dictionary) instead of a formula. Columns at their default value are also
dropped, so a compacted row is sparse. Full names pass through untouched, so uncompacted
files need no special case. The dictionary was checked against the uncompacted Skrzatusz
file: every column it names exists there, with two caveats — the *file's* column order
disagrees with the letter order for `Keyboard` (the `Hint_*` columns are written after
`AccessibleForOutput` but lettered `x`,`y`,`z`,`a1`) and `SwitchLinkage`
(`ReevaluateIfCondSwitchChangesState` is written sixth but lettered `h`); the compacted
Solignac file confirms the *dictionary's* lettering in both cases (`Keyboard.x` holds
division IDs, `SwitchLinkage.h` holds `N`).
Source: OdfEdit `HwObjectsAttributesDict.txt`; HWtoGO test files; `hauptwerk.rs` tests.

**Numbers.** Written the way a C `printf("%g")` spells them: `-2e+1`, `1.1999e+3`,
`4.814035087719e-1`. Integers may appear this way too, so the reader parses everything
as a float and rounds where an integer is meant. Booleans are `Y`/`N`.

**IDs.** Every table has a numeric primary key (`KeyboardID`, `StopID`, `RankID`,
`PipeID`, `LayerID`, `SampleID`, `SwitchID`, …) and relations are by ID. IDs are
arbitrary, non-contiguous, and reused across tables (Solignac's stop 2111 is controlled
by switch 10068 and a `Switch` with ID 2111 also exists). The reader never assumes
contiguity or order.

**Encoding of compressed definitions.** Hauptwerk can also store a definition
compressed; such a file does not start with `<`. The reader refuses anything whose first
non-whitespace byte after an optional UTF-8 BOM is not `<` (`sniff_xml`) — this also
catches encrypted definitions, which are opaque blobs. **UNVERIFIED** whether the
compressed form is zlib; not needed, since none of the free sets seen use it.

---

## 2. `_General`

One row. Columns used:

| Column | Meaning | Reader |
|---|---|---|
| `Identification_Name` | organ name | `Organ::name` |
| `OrganInfo_InstallationPackageID` | the set's own package | informational |
| `AudioOut_AmplitudeLevelAdjustDecibels` | organ-wide output trim (Solignac −20 dB, Skrzatusz +9 dB) | folded into every pipe's `gain_db`, as OdfEdit folds it into GO `Organ.Gain` (`GO_ODF_build_Organ_object`) |
| `AudioEngine_BasePitchHz` | the pitch the organ plays at; `0` = 440 | `1200·log2(hz/440)` folded into every pipe's `pitch_tuning_cents` (OdfEdit → GO `Organ.PitchTuning`) |
| `AudioEngine_EnablePlayingAtOriginalOrganPitch` | whether the player may switch to the recorded pitch | ignored: Aristide measures the recorded pitch itself (`docs/progress/2026-08-29-pitch-reference.md`) |
| `Control_FileIsCompacted_…` | §1 | informational; letters are expanded regardless |

`_General` keeps its full column names even in compacted files.

---

## 3. Keyboards, divisions, manuals

Hauptwerk separates the *keyboard* (what the player touches) from the *division* (the
department the stops belong to). Stops reference a `DivisionID`; keyboards reach
divisions through `KeyAction` rows.

**`Keyboard`.** `DefaultInputOutputKeyboardAsgnCode` is the console position: `1` =
pedal, `2`…`5` = manuals I–IV, `6`/`7` = utility keyboards (noise triggers, MIDI
plumbing); absent/`0` = a keyboard that is not a console keyboard (CODM sets carry
`CustPg1_InputKbd_…`/`OutputKbd_…` pairs that only mirror the visible ones on screen).
Source: OdfEdit `GO_ODF_build_Manual_object` (the code list is a comment there).

Compass: `KeyGen_GenerateKeysAutomatically` (default `Y`) with `KeyGen_NumberOfKeys` and
`KeyGen_MIDINoteNumberOfFirstKey`; when `N`, the keyboard's `KeyboardKey` rows list each
key's `NormalMIDINoteNumber` and the compass is their span (OdfEdit reads
`KeyboardKey` first and falls back to `KeyGen_*`, function `GO_ODF_build_Manual_object`
around line 7017). Solignac: pedal 27 keys from 36 (C2), manual 54 from 36; Skrzatusz:
27/54/54 from 36.

**Which division a keyboard plays.** `Hint_PrimaryAssociatedDivisionID` when set; else
the keyboard's unconditional `KeyAction` (no `ConditionSwitchID`, zero increment) onto a
`DestDivisionID`. Both appear in every set inspected.

**Reader's rule** (`read_manuals`): keyboards with code 1–5 become manuals, ordered by
code; code 1 → `ManualId(0)`, kind Pedal; the rest take `ManualId(1…)` in code order,
kind Manual. A division that has pipe stops (§5) but no such keyboard becomes a
"floating" manual after them, named after the division, compass from
`Division.InpGen_NumberOfInputs`/`InpGen_MIDINoteNumberOfFirstInput` (defaults 61/36),
with a warning. Codes 6+ and code-0 keyboards are ignored. Manual names come from the
keyboard's `Name` (OdfEdit prefers the division's; the keyboard's is what the player
sees on Hauptwerk's console).

---

## 4. Samples and pitch

**`Sample`.** `InstallationPackageID` + `SampleFilename` (backslashes possible) →
`OrganInstallationPackages/<id:06>/<file>`. A row may name any file: pipe recordings,
release tails, noises, tremulant waveforms (§6), even images in some sets.

`LicenceSerialNumRequiredForSampleFile` non-zero marks an encrypted sample. The reader
refuses the whole set if any sample carries it (`refuse_encrypted`), and additionally
opens the first sampled pipe's attack and refuses if it does not begin `RIFF`
(`check_samples`) — the Groningen and Utrecht demo definitions in the HWtoGO test set
are plain XML with licensed samples, so the XML sniff alone is not enough.

**Recorded pitch, `Pitch_SpecificationMethodCode`** (OdfEdit comment in
`GO_ODF_build_Rank_object`, ~line 9755):

| Code | Meaning | Reader |
|---|---|---|
| absent/`0` | no pitch stated | `midi_key_number = None`: the file's `smpl` chunk speaks (§9) |
| `1` | from the sample file's metadata | same |
| `2`, `5` | tremulant waveform (§6) | n/a |
| `3` | `Pitch_NormalMIDINoteNumber` on the ladder `Pitch_RankBasePitch64ftHarmonicNum` | key = note + 12·log2(h/8), split into `midi_key_number` (floor) and `midi_pitch_fraction_cents` |
| `4` | `Pitch_ExactSamplePitch` in Hz | key = 69 + 12·log2(hz/440), split the same way |

Solignac states nothing for its pipe samples (their `smpl` chunks carry unity note +
fraction; the organ is at a′ = 419 Hz and the fractions say so) and code 3 for its key
noises; Skrzatusz states code 1 for pipes and 4 for noises. The engine's "home pitch is
measured at load" rule makes the metadata a hint either way.

**Harmonic ladder.** `Pitch_Tempered_RankBasePitch64ftHarmonicNum` on `Pipe_SoundEngine01`
is the same 64′ ladder GrandOrgue's `HarmonicNumber` uses: 8 = 8′ (unison), 16 = 4′,
32 = 2′, 4 = 16′, 24 = 2⅔′, 48 = 1⅓′, 64 = 1′. It is **per pipe**, and mixtures use that:
Solignac's Plein Jeu has pipes at 16, 32 and 64, its Sesquialtera at 24 and 48. The
reader folds it into `nominal_frequency_hz` exactly as the GO reader folds
`HarmonicNumber`. Absent/`0` = 8.
Source: OdfEdit writes it to GO `HarmonicNumber` per pipe when it differs from the rank's
first pipe (`GO_ODF_build_Rank_object` ~line 9840); the ladder convention is GO's.

**Pipe key.** `Pipe_SoundEngine01.NormalMIDINoteNumber`; absent means 60 (OdfEdit's
observation on Sonus Paradisi sets, same function ~line 9600).

---

## 5. Stops, ranks, pipes

The chain is `Stop → StopRank → Rank → Pipe_SoundEngine01 → Pipe_SoundEngine01_Layer →
{AttackSample, ReleaseSample} → Sample`.

**`Stop`.** `Name`, `DivisionID`, `ControllingSwitchID`, `Hint_PrimaryAssociatedRankID`.
Names are whatever the author typed (Solignac's CODM names carry an ID prefix,
`2111_Bourdon 8'`); the reader keeps them as written.

**`StopRank`** — one row per (stop, rank) link, with two action codes. Only
`ActionTypeCode = 1` **and** `ActionEffectCode = 1` (both default 1 when absent) is
"draw the stop, the rank sounds under the keys". Everything else is noise plumbing:

| type / effect | Meaning (OdfEdit `HW_ODF_get_switch_controlled_objects` comment) |
|---|---|
| 1 / 1 | pipes |
| 1 / 2, 1 / 3 | key-press / key-release noise, one row per key |
| 21 / 2, 21 / 3 | stop engage / disengage noise |
| 21 / 1 | sustaining noise while engaged (blower) |

Mapping columns: `MIDINoteNumOfFirstMappedDivisionInputNode` (absent/0 = the manual's
first key), `NumberOfMappedDivisionInputNodes` (absent/0 = the rank's whole pipe count),
`MIDINoteNumIncrementFromDivisionToRank` (division key + increment = rank note; a 4′
extension of an 8′ rank is +12). The reader clips the mapped range to both the manual's
compass and the rank's pipes and emits one `RankRange` per link
(`read_stops`). Source: OdfEdit `GO_ODF_build_Stop_pipes_attributes` ~line 9220.

`AlternateRankID` + `SwitchIDToSwitchToAlternateRank`: a second rank of re-recordings
selected while a switch (normally a tremulant's) is on — Sonus Paradisi's tremmed
samples. §6.

Stops with no `StopRank` rows at all fall back to `Hint_PrimaryAssociatedRankID`
(OdfEdit notes some Grabowski sets do this). Stops with rows but none of type 1/1 are
noise carriers — Solignac has a "stop" for the blower, one for the coupler's click and
one for the tremulant's — and are counted into one warning, not loaded.

**`Rank`.** `Name`, and `SoundEngine01_Layer1Desc`…`Layer8Desc` naming the layers its
pipes carry ("Main"; a second one in Grabowski sets holds tremmed samples, §6).

**`Pipe_SoundEngine01`.** One per pipe: `RankID`, `NormalMIDINoteNumber`, the harmonic
(§4), `WindSupply_SourceWindCompartmentID` (§7), and a lot of wind/randomisation physics
the reader ignores today (`WindSupply_MassFlowRateKilogramsPerSecAtReferencePressureDiff`,
`Pitch_Tempered_RandomTuningError_*`, `TremulantDepthRandomisation_*`,
`VirtualOutputPos_*Metres`). `ControllingPalletSwitchID` set = a pipe played directly by
a switch, not a key (blower). The reader builds a rank as a contiguous run from its lowest
to highest note, filling gaps with silent pipes so that pipe index = note − first, and
warns about the gaps.

**`Pipe_SoundEngine01_Layer`.** One or more per pipe, `PipeLayerNumber` 1… The reader
uses the lowest-numbered layer. Columns used:

| Column | Reader |
|---|---|
| `AmpLvl_LevelAdjustDecibels` | added to the pipe's `gain_db` (with the organ trim, §2) |
| `PitchLvl_DetuningPercentSemitones` | percent of a semitone = cents; added to `pitch_tuning_cents` |
| `Main_Sustaining` (`N` = one-shot) | informational; noise layers are never loaded |
| `AmpLvl_ScalingContinuousControlID` | an expression-style control on this layer; **not mapped** (OdfEdit turns it into a GO enclosure) |

`AmpLvl_PctOfReferenceAirFlowRateAtWhichPipe*`, `PitchLvl_PitchDecrementPctSemitonesAtThisFlowRate`,
`HarmonicShaping_*`, `VoicingEQ01_*`, `ReverbTailTruncation_*` are Hauptwerk's wind and
voicing model per pipe — the raw material for a future mapping onto Aristide's own wind
model (`docs/research/hauptwerk-wind-model.md`), not read yet.

**`Pipe_SoundEngine01_AttackSample`.** Per layer, in `UniqueID` order:

| Column | Reader |
|---|---|
| `SampleID` | the file |
| `AttackSelCriteria_HighestVelocity` (default 127) | *highest* velocity this attack answers to. The model wants a *lowest*, so the reader sorts the layer's distinct thresholds and gives each attack `min_velocity` = the next-lower threshold + 1 (`read_attacks`). Two attacks at 63/127 become bounds 0/64. (OdfEdit writes GO `AttackVelocity = 127 − highest`, which is wrong for more than two attacks.) |
| `AttackSelCriteria_MinTimeSincePrevPipeCloseMs` (default 0) | *least* time since the pipe closed. Mirror image of the above: `max_time_since_last_release_ms` = next-higher threshold − 1, `None` for the last. |
| `AttackSelCriteria_HighestCtsCtrlValue` (default 127) | < 127 = chosen by a continuous control; skipped and counted (OdfEdit does the same) |
| `LoopCrossfadeLengthInSrcSampleMs` | `loop_crossfade_ms` (0–3000) |
| `LoadSampleRange_StartPositionTypeCode`/`Value`, `…End…` | **UNVERIFIED** load-range codes (attacks: start 4, end 7 or 6; releases: start 1 value 1, end 6 value 0 in Skrzatusz). Ignored by both converters and by this reader. |

Sustain loops are not in the definition: they are the WAV's `smpl` chunk (§9).

**`Pipe_SoundEngine01_ReleaseSample`.** Per layer:

| Column | Reader |
|---|---|
| `SampleID` | the file |
| `ReleaseSelCriteria_LatestKeyReleaseTimeMs` | used when the key was held at most this long; `99999`, `-1` or absent = the unbounded one → `max_key_press_ms = None`. Sorted ascending, unbounded last. Solignac: 150 / 280 / unbounded (S, M, L folders). |
| `ReleaseCrossfadeLengthMs` | `release_crossfade_ms` (0–3000) |
| `ReleaseSelCriteria_HighestVelocity`, `…HighestCtsCtrlValue` | < 127 → skipped and counted |
| `ScaleAmplitudeAutomatically`, `PhaseAlignAutomatically` | Hauptwerk's release matching; Aristide does both by its own rules (`docs/research/release-modeling.md`) |
| `ReleaseSelCriteria_PreferThisRelForAttackID` | pairing with a specific attack; not mapped |

A layer with releases but no attacks is a key-off noise; a pipe whose layer has neither
is silent with a warning.

---

## 6. Tremulants

Hauptwerk tremulants are *modelled*: a `Tremulant` row names a switch and rates, and
`TremulantWaveform` rows point at recorded **modulation waveforms** — float WAVs under
`tremulant/` (Solignac: `036-C-TremulantPitchAndFundamentalAmplitudeWaveform.wav`,
stereo, 22 050 Hz, channel 0 = pitch deviation, channel 1 = fundamental-amplitude
deviation, values ≈ ±0.007 for a 16′-scale Bourdon and ±0.29 at the top of the compass,
with a `smpl` loop marking one cycle; plus a mono `…ThirdHarmonicAmplitudeWaveform.wav`).
`TremulantWaveformPipe` rows say which pipes each waveform drives.
**UNVERIFIED**: the units of the channels (they read as linear fractions; OdfEdit's
comments say the wav "stores AmpModDepth per pipe" and it settles for a constant 15).

Columns on `Tremulant`: `ControllingSwitchID`, `FrequencyWhenEngagedHz`,
`FrequencyWhenDisengagedHz`, `StartRatePercent`, `StopRatePercent`, and randomisation
figures. Solignac's compacted row carries only the *disengaged* frequency (6.5 Hz) and
Skrzatusz's both (5.19 / 5.3), so "engaged" is not always written. Reader's rate:
`FrequencyWhenEngagedHz` if > 0, else the first waveform's `smpl` loop length
(sample rate ÷ loop frames — the true cycle), else `FrequencyWhenDisengagedHz`, else
5 Hz. Depth: the mean over the tremulant's waveforms of channel 1's peak, ×100, clamped
1–100 %; 15 % when no waveform can be read. Start/stop rates pass through as GO-style
1–100 (OdfEdit passes them through likewise; **UNVERIFIED** that Hauptwerk's percent
means the same as GO's rate).

The result is a `TremulantKind::Synth`, which Aristide renders through its wind-driven
tremulant (`docs/progress/2026-08-27-tremulant-physics.md`), so the waveform files
themselves are not played. Membership: every pipe a waveform drives adds the tremulant
to its wind key (§7).

**Tremmed re-recordings.** Two conventions (OdfEdit ~line 7606):

- *alternate rank* (Sonus Paradisi): `StopRank.AlternateRankID` switched by
  `SwitchIDToSwitchToAlternateRank`. The reader folds the alternate rank's samples into
  the main rank's pipes note for note as `wave_tremulant = Some(true)` variants, marks the
  main ones `Some(false)` (or `None` for notes without a tremmed twin), finds the
  tremulant whose `ControllingSwitchID` is that switch (directly or one `SwitchLinkage`
  hop away), and makes it `TremulantKind::Wave` — the same model GO's `Wave` tremulants
  use (`merge_alternate_rank`, `wave_tremulant_for`).
- *second layer* (some Grabowski sets): `PipeLayerNumber = 2` with `Rank.SoundEngine01_Layer2Desc`
  set, scaled by a continuous control that a switch drives. **Not mapped yet**; the
  reader uses layer 1 only.

---

## 7. Wind compartments, enclosures, windchests

**`WindCompartment`** models the wind system physically: `InfiniteVolume`,
`StandardVolumeMetresCubed`, `DefaultAirPressureInches`, bellows geometry, springs, and
`WindCompartmentLinkage` rows for the trunks between them (`Blower → output chamber →
reservoir → chest`). Every pipe names its `WindSupply_SourceWindCompartmentID`. The
reader uses only the compartment identity and name; the physics is future input to
Aristide's wind model.

**`Enclosure`** (`Name`, `ShutterPositionContinuousControlID`) with `EnclosurePipe`
rows per member pipe carrying a six-parameter shelf spec for the closed and open
positions (`FiltParamWhenClsd_OverallAttnDb`, `…MaxFreqHz`, `…MinFreqHz`,
`…ExtraAttnAtMinDb`, `FiltParamWhenOpen_MaxFreqHz`, `…MinFreqHz`) — the same table
`docs/research/enclosure-modeling.md` describes. Reader: one `Enclosure` per row,
`amp_minimum_level` = 100·10^(median closed attenuation / 20) over its pipes (−20 dB →
10 %), 0 when no pipe rows exist; the shelf frequencies are not mapped (the engine
applies its own shelf).

**Windchests.** The model wants wind, enclosure and tremulant membership per *rank*;
Hauptwerk states all three per *pipe*. The reader keys each pipe by (compartment,
sorted enclosure indices, sorted tremulant indices), lets the rank follow the majority
key (warning if pipes disagree), and creates one `Windchest` per distinct key, numbered
in order of first use and named after the compartment plus "in <enclosure>"
(`windchest_number`). OdfEdit does the same in
`GO_ODF_build_WindchestGroup_object` (one GO windchest per compartment + control +
enclosure combination).

---

## 8. Couplers: `KeyAction`

A `KeyAction` sends a keyboard's keys somewhere: `SourceKeyboardID` → `DestDivisionID`
or `DestKeyboardID` (`DestIsKeyboardNotDivision`), shifted by `MIDINoteNumberIncrement`,
over `MIDINoteNumOfFirstSourceKey` + `NumberOfKeys` keys, *while* `ConditionSwitchID` is
on (`ConditionSwitchLinkIfEngaged=Y`) or off (`N`). `ActionTypeCode`/`ActionEffectCode`
other than 1/1 are pizzicato/reiteration effects (`PipeMIDINoteNum036_PizzOrReitPeriodMs`
etc.). Source: OdfEdit `GO_ODF_build_Coupler_attributes`.

Reader (`read_couplers`): the unconditional, unshifted action onto the keyboard's own
division is the keyboard playing itself (skipped). Every other action whose source is a
manual and whose destination resolves to one becomes a `CouplerRoute` (key shift, key
bounds only when narrower than the source compass); actions sharing a `ConditionSwitchID`
are one `Coupler` with several routes, named after the action (fallback: the switch).
Unconditional cross-division actions — permanently engaged in Hauptwerk — become
ordinary couplers with a warning, since the model has no "engaged by default".
Inverted conditions and pizzicato codes are loaded as plain couplers with a warning.

Solignac orig: pedal keyboard 1 → division 2 ("Hw. Ped.8", switch 10086) — the whole
organ is one manual, and the pedal only couples. Skrzatusz: `I/P`, `II/I`.

---

## 9. Sample audio

Plain WAV, PCM 24-bit (Solignac 44.1 kHz stereo; sets up to 96 kHz exist). Attack files
carry a `smpl` chunk with unity note + fraction and the sustain loops (Solignac: two
loops per pipe), and an empty `cue` chunk; release files (`S/`, `M/`, `L/` folders here)
carry a `smpl` chunk without loops. Noise files come with editor chunks (`JUNK`, `LGWV`,
`bext`). Everything the GO reader relies on in `crate::wav` applies unchanged: loops and
recorded pitch come from the file when the definition states none (§4).

Encrypted sets: the sample files are not RIFF (§4). WavPack does not occur in Hauptwerk
sets.

---

## 10. Temperament files

`Temperaments/<name>.Temperament_Hauptwerk_xml`: `<Hauptwerk FileFormat="Temperament"
FileFormatVersion="2.00">`, a `_General` row (`UniqueTemperamentID`, `Name`,
`SupplierID`) and a `note` table of `MIDINoteNumberOnEightFootStop` → `PitchHz` for all
128 keys. Solignac ships its meantone this way. Not read yet; it maps naturally onto the
Scala tuning seam (`docs/progress/2026-09-01-tuning-scopes.md`) as a 128-entry
keyboard mapping.

---

## 11. What the reader does not do (yet)

- Noise ranks: blower, key and stop action noises (every set has them). Counted and
  skipped — Aristide has no noise-rank concept in the model.
- Second-layer tremmed samples (§6), `AmpLvl_ScalingContinuousControlID` expression
  layers (§5), continuous-control-selected attacks/releases.
- Combinations (`Combination`/`CombinationElement`), display pages, images, switches as
  console controls — Aristide's console is its own.
- Wind physics (§7), per-pipe voicing/EQ (§5), virtual output positions.
- Temperament files (§10).

## Sources consulted

- `github.com/GrandOrgue/OdfEdit` — `src/OdfEdit.py` (Hauptwerk→GO conversion),
  `src/resources/HwObjectsAttributesDict.txt` (column dictionary), shallow clone
  2026-09-02.
- `github.com/ahall41/HWtoGO` — `Test/ODF/*.Organ_Hauptwerk_xml` (Skrzatusz uncompacted;
  Groningen, MMCOrgan3, Utrecht, Walcker Miskolc compacted), shallow clone 2026-09-02.
- `github.com/dividebysandwich/rusty-pipes` — `src/organ_hauptwerk.rs` (a minimal reader:
  stops, ranks, pipes, samples; no keyboards or couplers), for cross-checking table use.
- AVO Solignac package (`hauptwerk-augustine.info/Solgnac_sample.php`), both definitions
  and the `tremulant/` waveform files.
