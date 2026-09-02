# GrandOrgue ODF (Organ Definition File) — Playback Reference

Notes compiled directly from the GrandOrgue source (`https://github.com/GrandOrgue/grandorgue`,
inspected at a shallow clone of `master` in August 2026) for the purpose of writing a
**playback-only** `.organ` parser in Rust. No GUI/console/panel keys are covered.
Where the loader code and any published help/wiki text disagree, the source wins — this
document was written by reading the loader (`GOConfigFileReader`, `GOConfigReader`,
`GOOrganModel`, `GOManual`, `GOStop`, `GORank`, `GOPipe`/`GOSoundingPipe`, `GOWindchest`,
`GOTremulant`, `GOCoupler`, `GOPipeConfig(Node)`, `GOSoundProviderWave`, `GOWave`), not from
the wiki. The repository's own `help/` directory (`help/grandorgue.xml`) is the *user manual*
for the application UI and does **not** contain an ODF key reference, so it is not cited below.

All facts are tagged with the source file(s) they came from. Anything not directly confirmed
in the code is marked **UNVERIFIED**.

---

## 1. File basics

**Syntax.** An ODF is a Windows-INI-style text file: `[SectionName]` headers, `Key=Value`
lines, and `;` starts a line comment (the parser truncates everything from the first `;`
onward on a line, trims, and skips the line if it's empty afterward). There is no `#`
comment syntax. A line that isn't a section header and appears before any `[Section]` is
an error. Values are **not** trimmed by the low-level line reader — trimming/whitespace
policy is applied later per-key by `GOConfigReader` (see below).
Source: `src/core/config/GOConfigFileReader.cpp` (`GOConfigFileReader::Read`, lines ~111-166).

**Encoding.** The whole file is read as raw bytes, then decoded as **ISO-8859-1** (Latin-1)
*unless* the file begins with a UTF-8 BOM (`EF BB BF`), in which case it is decoded as UTF-8
(BOM bytes stripped first). Note: this is ISO-8859-1, not exactly Windows-1252 (they differ
in the 0x80–0x9F range) — if your source claimed "windows-1252", that is an approximation;
the actual fallback codec in the loader is ISO-8859-1.
Source: `src/core/config/GOConfigFileReader.cpp` lines 94-104.

**Compression.** Before decoding, the raw byte buffer is checked with `isBufferCompressed()`
and transparently decompressed if so — GO ODFs can ship gzip/zlib-compressed. A Rust parser
consuming raw `.organ` files from disk will normally not need this (uncompressed is the
common case), but be aware some organ packages may ship compressed ODFs.
Source: `src/core/config/GOConfigFileReader.cpp` lines 87-92 (`isBufferCompressed`,
`uncompressBuffer` — implementation in `GOCompress.cpp`, not read in detail here).

**Section/key case sensitivity.** Section names and keys are matched **exactly first**
(`group + "/" + key`, exact case, in a hash map). If an exact match is not found *and* the
reader is not in strict/case-sensitive mode, it retries with the whole `group/key` string
lower-cased, logging `"Incorrect case for section '%s' entry '%s'"` on success. So: **write
keys with the documented exact case; a conformant parser may treat keys as
case-insensitive for compatibility**, matching GO's lenient fallback.
Source: `src/core/config/GOConfigReaderDB.cpp` lines 49-138 (`ReadData`, `GetString`).

**Duplicate sections/keys.** Both are tolerated with a logged warning (`Duplicate group`,
`Duplicate entry in section`); the last-parsed value for a duplicate key wins (map
overwrite).
Source: `src/core/config/GOConfigFileReader.cpp` lines 140-164.

**Boolean convention (`Y`/`N`).** Read via `ReadBooleanTriple`: value `"Y"`/`"y"` → true,
`"N"`/`"n"` → false. Anything else logs a warning and falls back to checking just the first
character upper-cased (`'Y'` → true, `'N'` → false); anything else throws a hard parse
error. There is also a "not specified" tri-state (`BOOL3_DEFAULT`, i.e. -1) used when the
key is absent and not required — this tri-state matters for `Percussive` and
`HasIndependentRelease` (see §5) because their *effective* default is inherited from the
parent object, not a fixed `Y`/`N`.
Source: `src/core/config/GOConfigReader.cpp` lines 193-234 (`ReadBooleanTriple`,
`ReadBoolean`).

**Numbers.** Integers are parsed with `std::stol` (locale-independent); floats are parsed
with a `C`-locale `istringstream`, and a literal comma is auto-replaced with `.` (with a
warning) to tolerate locale-mangled decimal separators. Integer/float reads that are ODF
settings (as opposed to `.cmb` settings) **throw a hard error** if out of the declared
`[min,max]` range, rather than clamping.
Source: `src/core/config/GOConfigReader.cpp` lines 355-502.

**Paths.** Sample file paths (`Pipe001=...`, `InfoFilename=...`, attack/release filenames)
use **backslash (`\`) as the path separator inside the ODF**, regardless of host OS; the
loader does `path.Replace("\\", <native separator>)` before resolving. Paths are relative
to the ODF's own directory (or to the containing package archive) unless absolute.
Source: `src/grandorgue/loader/GOLoaderFilename.cpp` lines 19-74
(`GOLoaderFilename::Assign`, `generateFullPath`); confirmed by hard-coded backslash sample
paths elsewhere in the codebase, e.g. `src/grandorgue/GOMetronome.cpp` line 92:
`wxT("sounds\\metronome\\beat.wv")`.

**`[Organ]` section — top-level keys relevant to playback:**

| Key | Type | Required | Range/Default | Notes |
|---|---|---|---|---|
| `ChurchName` | string (trimmed) | yes | — | organ display name |
| `HasPedals` | Y/N | yes | — | if `N`, there is no `Manual000`/pedal division |
| `NumberOfManuals` | int | yes | 1–16 | **excludes** the pedal; total manual slots loaded = this **+1** (slot 0 reserved for pedal, used only if `HasPedals=Y`) |
| `NumberOfWindchestGroups` | int | yes | 1–999 | |
| `NumberOfEnclosures` | int | yes | 0–999 | |
| `NumberOfSwitches` | int | no | 0–999, default 0 | must be loaded before manuals (manuals reference switches) |
| `NumberOfTremulants` | int | yes | 0–999 | |
| `NumberOfRanks` | int | no | 0–999, default 0 | count of standalone `[RankNNN]` sections (new-style organs); old-style organs (inline pipes under `[StopNNN]`) omit this or leave it 0 |
| `NumberOfReversiblePistons` | int | yes | 0–32 | not needed for pure playback |
| `NumberOfDivisionalCouplers` | int | yes | 0–8 | |
| `NumberOfGenerals` | int | yes | 0–99 | combination-system only, not needed for playback |
| `DivisionalsStoreIntermanualCouplers` / `...IntramanualCouplers` / `...Tremulants` / `GeneralsStoreDivisionalCouplers` | Y/N | yes | GO defaults all N | combination-system only — **Aristide reads the three `Divisionals*` flags** (see below) |
| `CombinationsStoreNonDisplayedDrawstops` | Y/N | no | default N | combination-system only |
| `HauptwerkOrganFileFormatVersion` | string | no | — | Hauptwerk-compat metadata, ignored by GO logic |
| `ChurchAddress`, `OrganBuilder`, `OrganBuildDate`, `OrganComments`, `RecordingDetails`, `InfoFilename` | string | no | — | pure metadata, not needed for playback |
| `NumberOfPanels` | int | no | 0–100, default 0 | GUI panels — skip for playback-only |

Also present at organ level (root of the pipe-config inheritance tree, §8):
`AmplitudeLevel`, `Gain`, `PitchTuning`, `PitchCorrection`, `TrackerDelay`, `Percussive` —
all optional, same semantics as at Windchest/Rank/Pipe level.

**How far a divisional reaches** (2026-09-02): the three `DivisionalsStore*` flags
are *not* "combination-system only" for a player who wants pistons that behave like
their own console's, so Aristide's loader reads them into
`aristide_model::CombinationScope` (`grandorgue.rs`, `[Organ]`). They mean:

| Flag | GO applies it in | Effect |
|---|---|---|
| `DivisionalsStoreIntermanualCouplers` | `GOCoupler.cpp:259-264` | divisionals also store the division's couplers **whose destination manual differs from their source** (`GOCoupler::IsIntermanual`, `GOCoupler.cpp:447`) |
| `DivisionalsStoreIntramanualCouplers` | same | …and those that stay on their own manual (octave couplers, unison off) |
| `DivisionalsStoreTremulants` | `GOTremulant.cpp:79-83` | divisionals also store the division's tremulants |

GO's own defaults for all five combination flags are `false`
(`GOOrganModel.cpp:44-47`), and a divisional's scope is that manual's own stops,
couplers, tremulants and switches (`GOCombinationDefinition::InitDivisional`) — a
coupler belongs to the manual it is *defined under*, i.e. the manual whose keys it
borrows. The GO demo set answers `Inter=Y, Intra=Y, Trem=N`.

The crescendo is not in the ODF at all: GO keeps four banks of `CRESCENDO_STEPS = 32`
stages in its settings (`GOSetter.cpp`), with a per-bank add/override mode; a
drawstop's engaged state is an OR over named internal states, the crescendo owning
one of them (`GODrawstop::SetInternalState` / `CalculateResultState`), which is what
makes its "add" mode additive over the hand registration. There is no conventional
MIDI CC for it — GO learns whatever the shoe sends.

Source: `src/grandorgue/model/GOOrganModel.cpp` lines 69-225 (`GOOrganModel::Load`),
`src/grandorgue/GOOrganController.cpp` lines 181-230 (`ReadOrganFile`, metadata-only keys).

---

## 2. Manuals

Manual sections are named `Manual%03d` (zero-padded to 3 digits), for indices
`0 .. NumberOfManuals` inclusive. **`Manual000` is the pedal division** and is only loaded
(and only need exist in the file) if `[Organ] HasPedals=Y`; if `HasPedals=N`, section
indices start at `Manual001` and `Manual000` is skipped entirely (internal manual index 0
is reserved as an empty slot).
Source: `src/grandorgue/model/GOOrganModel.cpp` lines 90-101 (`m_FirstManual`,
loop bounds), `src/grandorgue/model/GOManual.cpp` line 121+ (`GOManual::Load`).

| Key | Type | Required | Range/Default |
|---|---|---|---|
| `Name` | string | yes (non-empty) | — |
| `NumberOfLogicalKeys` | int | yes | 1–192 |
| `FirstAccessibleKeyLogicalKeyNumber` | int | yes | 1–`NumberOfLogicalKeys` |
| `FirstAccessibleKeyMIDINoteNumber` | int | yes | 0–127 |
| `NumberOfAccessibleKeys` | int | yes | 0–85 |
| `NumberOfStops` | int | yes | 0–999 |
| `NumberOfCouplers` | int | no | 0–999, default 0 |
| `NumberOfTremulants` | int | no | 0–`[Organ] NumberOfTremulants`, default 0 |
| `NumberOfSwitches` | int | no | 0–`[Organ] NumberOfSwitches`, default 0 |
| `Displayed` | Y/N | no | default N (GUI-only, safe to ignore) |
| `MIDIKey000`..`MIDIKey127` | int | no | 0–127, default = own index; a per-manual MIDI-note remap table, one optional key per note 0..127 |
| `StopNNN` (`NNN` = 001..`NumberOfStops`) | int | yes each | 1–999 | *indirection*: value is the numeric suffix of the `[StopMMM]` section this logical stop slot refers to (usually `NNN == MMM` but not required) |
| `CouplerNNN` | int | yes each (if any couplers) | 1–999 | same indirection scheme, resolves to `[CouplerMMM]` |
| `TremulantN` (`N` = 1..`NumberOfTremulants`) | int | yes each | 1–`[Organ] NumberOfTremulants` | index into the organ's global tremulant list |
| `SwitchN` (`N` = 1..`NumberOfSwitches`) | int | yes each | 1–`[Organ] NumberOfSwitches` | index into the organ's global switch list |

`FirstLogicalKeyMIDINoteNumber` (derived, not a key) =
`FirstAccessibleKeyMIDINoteNumber − FirstAccessibleKeyLogicalKeyNumber + 1`; this is the
MIDI note number that would correspond to logical key 1 of the manual, and it's what gets
passed down to stops/ranks as their pitch-numbering origin.

Source: `src/grandorgue/model/GOManual.cpp` lines 121-235 (`GOManual::Load`).

---

## 3. Stops and ranks

**Old-style stop (inline rank, no `[RankNNN]` sections; `NumberOfRanks` absent or 0).**
The `[StopNNN]` section itself doubles as the rank's config group. Required/optional keys
on the stop section:

| Key | Type | Required | Range/Default |
|---|---|---|---|
| `FirstAccessiblePipeLogicalKeyNumber` | int | yes | 1–128 |
| `NumberOfAccessiblePipes` | int | yes | 1–192 |
| `FirstAccessiblePipeLogicalPipeNumber` | int | yes (old-style only) | 1–192 |

The inline rank is constructed with `Name` (falls back to reading the *stop's* group), and
is given `NumberOfLogicalPipes = NumberOfAccessiblePipes`; its `FirstMidiNoteNumber`
defaults (if not explicitly set on the same section) to
`manualFirstMIDINote − FirstAccessiblePipeLogicalPipeNumber + FirstAccessiblePipeLogicalKeyNumber − 1`
— i.e. old-style stops don't need an explicit `FirstMidiNoteNumber`, it's back-derived.
Source: `src/grandorgue/model/GOStop.cpp` lines 83-101 (`GOStop::Load`, `else` branch).

**New-style stop (`NumberOfRanks` > 0, referencing standalone `[RankNNN]` sections).**

| Key | Type | Required | Range/Default |
|---|---|---|---|
| `NumberOfRanks` | int | no | 0–999, default 0 |
| `FirstAccessiblePipeLogicalKeyNumber` | int | yes | 1–128 |
| `NumberOfAccessiblePipes` | int | yes | 1–192 |
| `RankNNN` (`NNN`=001..`NumberOfRanks`) | int | yes each | 1–`[Organ] NumberOfRanks` — index of the `[RankMMM]` section |
| `RankNNNFirstPipeNumber` | int | no | 1–`rank.PipeCount`, default 1 |
| `RankNNNPipeCount` | int | no | 1–(`rank.PipeCount` − `FirstPipeNumber` + 1), default = remaining pipes in rank |
| `RankNNNFirstAccessibleKeyNumber` | int | no | 1–`NumberOfAccessiblePipes`, default 1 |

A stop can reference the same rank from multiple ranges, or reference several different
ranks (e.g. a mixture stop's separate breaks), each contributing a sub-range of accessible
keys. A stop with exactly one rank of exactly one pipe is a special "effects" stop (e.g. a
bell/chime toggle rather than a keyboard-played rank): turning the stop's drawstop on/off
directly sets that single pipe's velocity to full/off instead of tracking key state.
Source: `src/grandorgue/model/GOStop.cpp` lines 30-35 (`IsForEffects`), 37-106 (`Load`).

**`[RankNNN]` section:**

| Key | Type | Required | Range/Default |
|---|---|---|---|
| `Name` | string | yes | — |
| `FirstMidiNoteNumber` | int | required only for standalone `[RankNNN]` sections (old-style inline ranks derive it, see above) | 0–256 |
| `NumberOfLogicalPipes` | int | yes | 1–192 |
| `WindchestGroup` | int | yes | 1–`[Organ] NumberOfWindchestGroups` |
| `HarmonicNumber` | int | no | 1–1024, default 8 (i.e. unison/8′-equivalent pitch, GO's baseline) |
| `MinVelocityVolume` / `MaxVelocityVolume` | float | no | 0–1000, default 100 each — velocity-to-volume curve endpoints (see §8) |
| `AcceptsRetuning` | Y/N | no | default **Y** — whether this rank is retuned when a non-equal temperament is active |
| `PipeNNN` (`NNN`=001..`NumberOfLogicalPipes`) | string | yes each | see §4 |

Source: `src/grandorgue/model/GORank.cpp` lines 74-137 (`GORank::Load`).

`Percussive` and `WindchestGroup` interact with §5/§8: a rank has no `Percussive` key of
its own in the source read here — percussiveness is set per-pipe (or inherited down from
windchest/organ level, see §8's inheritance chain) via the shared `GOPipeConfig` mechanism,
not a `GORank`-specific field.

---

## 4. Pipes

Each `PipeNNN=` value in a `[RankNNN]` section is one of:

- a **sample path**, e.g. `PipeNNN=path\to\sample.wav` (backslash-separated, relative to
  the ODF unless absolute) — a normal sounding pipe (`GOSoundingPipe`).
- the literal string `DUMMY` — a silent placeholder pipe (`GODummyPipe`) that occupies a
  pipe slot but plays nothing.
- `REF:<manual>:<stop>:<pipe>` — a **borrowed/reference pipe** (`GOReferencePipe`) that
  echoes another pipe's velocity instead of having its own samples.
  `<manual>` is the **internal manual index** (0 = pedal if present, matching
  `Manual%03d` numbering — not necessarily the same as `NumberOfManuals`-relative
  numbering), `<stop>` and `<pipe>` are **1-based**: `<stop>` indexes that manual's
  `Stop%03d` slot, `<pipe>` indexes that stop's *first rank's* pipe list. Any malformed
  reference or out-of-range index throws `"Invalid reference"`.
  Source: `src/grandorgue/model/GOReferencePipe.cpp` lines 27-55.
- (trimmed value empty is not a valid pipe name; the loader will treat `DUMMY` as a special
  literal string comparison, so it must be exactly `DUMMY`, no path.)

Source of the dispatch: `src/grandorgue/model/GORank.cpp` lines 112-134.

**Per-pipe keys**, all read with prefix `PipeNNN` (i.e. `PipeNNNHarmonicNumber`,
`PipeNNNMIDIKeyNumber`, etc. — except the pipe's own filename, which is the bare
`PipeNNN=` value itself):

| Key (prefix `PipeNNN`) | Type | Required | Range/Default |
|---|---|---|---|
| *(bare value)* | filename/DUMMY/REF | yes | — |
| `AmplitudeLevel` | float | no | 0–1000, default 100 (%, see §8) |
| `Gain` | float | no | −120–40, default 0 (dB, see §8) |
| `PitchTuning` | float | no | −1800–1800, default 0 (cents, used *without* auto-tuning) |
| `PitchCorrection` | float | no | −1800–1800, default 0 (cents, used *with* auto-tuning/temperament) |
| `TrackerDelay` | int | no | 0–10000, default 0 (ms, additive up the inheritance chain) |
| `Percussive` | Y/N (tri-state) | no | unset → inherited (see §8) |
| `HasIndependentRelease` | Y/N (tri-state) | no, only read if effective `Percussive` is true | unset → inherited |
| `HarmonicNumber` | int | no | 1–1024, default = rank's `HarmonicNumber` |
| `WindchestGroup` | int | no | 1–`NumberOfWindchestGroups`, default = rank's `WindchestGroup` |
| `MIDIKeyNumber` | int | no | −1–127, default −1 (−1 means "use the value embedded in the WAV `smpl` chunk", see §9) |
| `MIDIPitchFraction` | float | no | 0–100, default −1 (−1 means "use the WAV `smpl` chunk value, unless `MIDIKeyNumber` was explicitly given, in which case fraction is assumed 0") |
| `AcceptsRetuning` | Y/N | no | default = rank's `AcceptsRetuning` |
| `AttackCount` | int | no | 0–100, default 0 |
| `ReleaseCount` | int | no | 0–100, default 0 |
| `MinVelocityVolume` / `MaxVelocityVolume` | float | no | 0–1000 | **note**: these are read with the *unprefixed* key name `MinVelocityVolume`/`MaxVelocityVolume` in the same `[RankNNN]` group as the rank-level keys of the same name — i.e. this is effectively a **per-rank**, not truly per-pipe, override despite living in `GOSoundingPipe::Load`; every pipe in the rank sees the same value. |

Source: `src/grandorgue/model/GOSoundingPipe.cpp` lines 216-283 (`GOSoundingPipe::Load`),
`src/grandorgue/model/pipe-config/GOPipeConfig.cpp` lines 143-165 (`GOPipeConfig::Load`).

**Default attack.** Every pipe, even with `AttackCount=0`, has an implicit "attack 0"
loaded from the bare `PipeNNN=` filename itself, with these fields (all optional, prefix =
`PipeNNN`, i.e. unsuffixed beyond the pipe number):

| Key | Type | Range/Default |
|---|---|---|
| `IsTremulant` | Y/N tri-state | unset (matches any tremulant state) |
| `LoadRelease` | Y/N | default = NOT effective-Percussive |
| `MaxKeyPressTime` | int | −1–100000, default −1 (see §5) |
| `CuePoint` | int | −1–`MAX_SAMPLE_LENGTH`, default −1 (−1 = use WAV `cue` chunk / release marker if present) |
| `AttackVelocity` | int | 0–127, default 0 (minimum MIDI velocity this attack sample applies to) |
| `MaxTimeSinceLastRelease` | int | −1–100000, default −1 (see §5) |
| `AttackStart` | int | 0–`MAX_SAMPLE_LENGTH`, default 0 (sample-frame offset where playback starts) |
| `ReleaseEnd` | int | −1–`MAX_SAMPLE_LENGTH`, default −1 (frame offset where the attack's *internal* release tail ends, if any; −1 = end of file) |
| `LoopCount` | int | 0–100, default 0 |
| `Loop%03dStart` / `Loop%03dEnd` | int | required if `LoopCount`>0; `End` must be > `Start`, both ≤ `MAX_SAMPLE_LENGTH` |
| `LoopCrossfadeLength` | int | 0–3000, default 0 — **milliseconds** (corrected 2026-08-26: `fade_len = len * sample_rate / 1000`, GOSoundAudioSection.cpp `Setup`); raised-cosine blend baked across each loop seam at load |
| `ReleaseCrossfadeLength` | int | 0–3000, default 0 — **milliseconds** (`m_ReleaseCrossfadeLength; // in ms`, GOSoundAudioSection.h) — only read if `LoadRelease` is true |

If `LoopCount=0` **and** the ODF doesn't declare loops for that attack, GO falls back to
reading loop points from the WAV file's own `smpl` chunk (see §9).
Source: `src/grandorgue/model/GOSoundingPipe.cpp` lines 96-178 (`LoadAttackFileInfo`);
loop fallback confirmed in `src/grandorgue/sound/providers/GOSoundProviderWave.cpp`
lines 56-62.

---

## 5. Multiple attacks and releases

**Additional attacks:** `PipeNNNAttack001` .. `PipeNNNAttack{AttackCount:03d}`, each with
the *same* field set as the default attack above but under prefix `PipeNNNAttackMMM`
instead of `PipeNNN` — including its own filename as the bare `PipeNNNAttackMMM=` value.

**Additional releases:** `PipeNNNRelease001` .. `PipeNNNRelease{ReleaseCount:03d}`. Fields
(prefix `PipeNNNReleaseMMM`):

| Key | Type | Range/Default |
|---|---|---|
| *(bare value)* | filename | required |
| `IsTremulant` | Y/N tri-state | unset |
| `MaxKeyPressTime` | int | −1–100000, default −1 |
| `CuePoint` | int | −1–`MAX_SAMPLE_LENGTH`, default −1 |
| `ReleaseEnd` | int | −1–`MAX_SAMPLE_LENGTH`, default −1 |
| `ReleaseCrossfadeLength` | int | 0–3000, default 0 |

Source: `src/grandorgue/model/GOSoundingPipe.cpp` lines 180-214 (`LoadReleaseFileInfo`).

If a pipe's effective `HasIndependentRelease` is true but `ReleaseCount=0`, GO logs a
warning ("independent release but ReleaseCount=0") but does not fail — it simply has no
dedicated release samples to pick from beyond whatever the attack-embedded release
provides. Source: same file, lines 262-270.

**Selection semantics at play time** (this governs which of several attack/release
samples is used for a given note-on/note-off — needed if your Rust engine reimplements GO's
sample-choice logic, not just parses the ODF):

- **`IsTremulant`** (tri-state, on attack or release): filters candidate samples by whether
  a wave-based tremulant (§6) is currently engaged (`BOOL3_TRUE` = only when tremulant on,
  `BOOL3_FALSE` = only when off, `BOOL3_DEFAULT`/unset = matches either state). Source:
  `src/grandorgue/sound/providers/GOSoundProvider.cpp` `IsWaveTremulantStateSuitable`
  usage in `GetAttack`/`GetRelease`.
- **Attack selection (`GetAttack(velocity, releasedDurationMs)`)**: among attacks whose
  tremulant-state matches and `AttackVelocity ≤ velocity` and
  `MaxTimeSinceLastRelease ≥ releasedDurationMs` (time since the pipe's previous release,
  in ms — `-1` stored as unsigned effectively means "always satisfied"), picks the
  candidate with the **highest `AttackVelocity`** that also has the **lowest
  `MaxTimeSinceLastRelease`** among ties (i.e., the most specific match); ties broken by a
  random rotation of the candidate list (`rand()`-based starting offset), not
  first-declared-wins.
- **Release selection (`GetRelease(tremulantState, playbackDurationMs)`)**: among releases
  whose tremulant-state matches exactly and whose `MaxKeyPressTime ≥ playbackDurationMs`
  (how long the note was held, in ms), picks the one with the **smallest**
  `MaxKeyPressTime` that still qualifies (closest match from above), again with random
  tie-breaking rotation.
- **`MaxKeyPressTime` on an *attack*** (not a release) is a *different* thing: it doesn't
  gate attack selection directly; instead it's used when consolidating attacks/releases
  during load (choosing which attack's embedded release is authoritative when the engine
  is configured not to load separate release files) — implementation detail, not required
  for a straightforward ODF-to-audio-graph parser, but the field must still be parsed.

Source: `src/grandorgue/sound/providers/GOSoundProvider.cpp` lines 203-254 (`GetAttack`,
`GetRelease`); `src/grandorgue/sound/providers/GOSoundProviderWave.cpp` lines 330-420
(load-time attack/release pruning, `LoadFromMultipleFiles`).

---

## 6. Tremulants and windchest groups

**`[TremulantNNN]`** (`NNN` = 1..`[Organ] NumberOfTremulants`):

| Key | Type | Required | Range/Default |
|---|---|---|---|
| `TremulantType` | enum `Synth`\|`Wave` | no | default `Synth` |
| `Period` | int | yes, only if `Synth` | 32–441000, **milliseconds** per full cycle |
| `StartRate` | int | yes, only if `Synth` | 1–100 |
| `StopRate` | int | yes, only if `Synth` | 1–100 |
| `AmpModDepth` | int | yes, only if `Synth` | 1–100 |

**Synth-tremulant semantics** (corrected 2026-08-26; the range check reads like a
sample count but the math says milliseconds): GO synthesizes the trem control
signal as a looping 16-bit sine "sample" at a fixed 44100 Hz with
`trem_freq = 1000.0 / period` Hz (so the demo set's `Period=196` ≈ 5.1 Hz),
amplitude `0x7FF0 · AmpModDepth / 100` — i.e. `AmpModDepth` is **percent of
full-scale amplitude modulation**. `StartRate`/`StopRate` are ramp speeds: the
synthesized attack section is `44100 / StartRate` frames and the release
`44100 / StopRate` frames — the engage ramp lasts **`1/StartRate` seconds**
(1–100 → 1 s down to 10 ms), disengage `1/StopRate` seconds. The signal
modulates windchest amplitude only (block-rate, no FM). Source:
`src/grandorgue/sound/providers/GOSoundProviderSynthedTrem.cpp` lines 29-80
(`Create`: `trem_freq`, `attack_samples`, `trem_amp`).

`Synth` tremulants are a GO-synthesized amplitude/pitch modulation applied to whatever is
currently sounding on windchests that reference this tremulant — no extra sample files
needed; safe to synthesize algorithmically. `Wave` tremulants have **no extra ODF fields of
their own** here — instead, individual pipe attack/release records opt into
tremulant-affected samples via their `IsTremulant` tri-state (§5), and the windchest
(below) simply toggles the tremulant on/off, causing `GOWindchest::UpdateTremulant` to
call `SetWaveTremulant` on every pipe on that windchest, which makes the sound provider
prefer `IsTremulant=Y` sample variants. **For a playback-only parser that doesn't need to
model tremulant audio precisely, both types can be "parsed and ignored" by simply not
switching sample variants** — you'd just always play the `IsTremulant`-unset/`N` variant.
Source: `src/grandorgue/model/GOTremulant.cpp` lines 55-77 (`Load`),
`src/grandorgue/model/GOWindchest.cpp` lines 116-129 (`UpdateTremulant`).

**`[WindchestGroupNNN]`** (`NNN` = 1..`[Organ] NumberOfWindchestGroups`):

| Key | Type | Required | Range/Default |
|---|---|---|---|
| `Name` | string | no | default `"Windchest N"` |
| `NumberOfEnclosures` | int | yes | 0–`[Organ] NumberOfEnclosures` |
| `NumberOfTremulants` | int | yes | 0–`[Organ] NumberOfTremulants` |
| `EnclosureNNN` | int | yes each | 1–`NumberOfEnclosures` (global index) |
| `TremulantNNN` | int | yes each | 1–`[Organ] NumberOfTremulants` (global index) |

Plus the shared pipe-config keys (`AmplitudeLevel`, `Gain`, `PitchTuning`,
`PitchCorrection`, `TrackerDelay`, `Percussive`) at this level — see §8.

For pure playback (ignoring enclosure swell-shutter attenuation), the windchest's role
reduces to: (a) grouping ranks/pipes for the amplitude/tuning inheritance chain, and
(b) which tremulant(s), if any, affect it. Enclosure attenuation
(`GOWindchest::GetVolume`/`UpdateVolume`, product of all attached `GOEnclosure`
attenuations) is a GUI/expression-pedal feature — safe to treat as a fixed multiplier of
1.0 if you don't model enclosures.
Source: `src/grandorgue/model/GOWindchest.cpp` lines 31-131.

---

## 7. Couplers

`[CouplerNNN]` sections, referenced from a manual's `CouplerNNN=` indirection (§2).

| Key | Type | Required | Range/Default |
|---|---|---|---|
| `UnisonOff` | Y/N | no | default N |
| `DestinationManual` | int | required unless `UnisonOff=Y` | `[Organ] first-manual-index`–`GetManualAndPedalCount()` |
| `DestinationKeyshift` | int | required unless `UnisonOff=Y` | −24–24 (semitones) |
| `CouplerType` | enum `Normal`\|`Bass`\|`Melody` | no (only read if not `UnisonOff`) | default `Normal` |
| `CoupleToSubsequentUnisonIntermanualCouplers` | Y/N | no (only for `Normal` type) | default N |
| `CoupleToSubsequentUpwardIntermanualCouplers` | Y/N | no | default N |
| `CoupleToSubsequentDownwardIntermanualCouplers` | Y/N | no | default N |
| `CoupleToSubsequentUpwardIntramanualCouplers` | Y/N | no | default N |
| `CoupleToSubsequentDownwardIntramanualCouplers` | Y/N | no | default N |
| `FirstMIDINoteNumber` | int | no (only if not `UnisonOff`) | 0–127, default 0 |
| `NumberOfKeys` | int | no (only if not `UnisonOff`) | 0–127, default 127 |

Playback semantics: a non-`UnisonOff` coupler routes key-down/up events from its source
manual to `DestinationManual`, shifted by `DestinationKeyshift` semitones, restricted to
notes in `[FirstMIDINoteNumber, FirstMIDINoteNumber+NumberOfKeys)`. `Bass`/`Melody` types
instead couple only the currently-lowest/highest held note on the source manual (a
"Melodie-/Bass-Koppel"), tracked incrementally as notes are pressed/released — see
`GOCoupler::ChangeKey`/`GetNextBasMelPressedKey` for the exact algorithm if you need to
replicate it precisely (it's stateful, not a pure function of current key state, because it
tracks "last coupled tone" across events).
`UnisonOff=Y` couplers instead silence the source manual's *own* sound (its
"unison"/principal rank output) while still allowing it to drive other coupled manuals —
implemented as a reference count on the manual (`GOManual::SetUnisonOff`), not a note
router.
Source: `src/grandorgue/model/GOCoupler.cpp` lines 18-257 (`Load`, `Init`), 328-378
(`ChangeKey`), 331-341 (`SetUnisonOff` call sites).

---

## 8. Amplitude / gain / tuning inheritance

GO builds a **tree** of `GOPipeConfigNode`s: `[Organ]` (root) → each `[WindchestGroupNNN]`
→ each `[RankNNN]` (parented to its windchest) → each pipe (parented to its rank). Each
node reads its *own* ODF value via `GOPipeConfig::Load` (all optional, defaulting as
below), and the *effective* value used at playback time is computed by walking up the
tree — **combination rule differs per field**:

| Field (ODF key) | Own default | Combination with parent |
|---|---|---|
| `AmplitudeLevel` (%) | 100 | **multiplicative**: `effective = own% × parent_effective / 100` (i.e. percentages compound) |
| `Gain` (dB) | 0 | **additive**: `effective = own + parent_effective` |
| `PitchTuning` (cents) | 0 | **additive** |
| `PitchCorrection` (cents) | 0 | **additive** |
| `TrackerDelay` → "Delay" (ms) | 0 | **additive** |
| `Percussive` (Y/N/unset) | unset | **inherited if unset**: unset pipe/rank/windchest falls through to parent's *effective* percussiveness; root default (organ-level unset) resolves to a global app setting, not a fixed constant — for a playback-only parser, treat an all-unset chain as **non-percussive** |
| `HasIndependentRelease` (Y/N/unset) | unset | same inheritance pattern as `Percussive`, but only meaningful (only read from ODF) when the *effective* `Percussive` for that node's parent context is true |

Final linear gain applied to a sample = `fixed_amplitude(0..1) × 10^(gain_dB / 20)`, where
`fixed_amplitude` folds in the `AmplitudeLevel` chain (as a 0..1 fraction, i.e. `%/100`
compounded up the tree) — see `GOSoundProviderWave::SetAmplitude`:
`m_Gain = fixed_amplitude * powf(10.0f, gain * 0.05f)` (note: `gain * 0.05` = `gain / 20`,
the standard dB-to-linear-amplitude conversion, confirming `Gain` is in dB and additive).

Pitch: the sample's actual playback pitch offset (cents) is
`PitchTuning + ManualTuning` (manual tuning is a `.cmb`/runtime-only concept, not ODF) when
temperament auto-tuning is off, or a harmonic/temperament-derived `AutoTuningPitchOffset`
(using `HarmonicNumber`, the sample's own detected MIDI key/pitch fraction vs. the pipe's
nominal MIDI key, `PitchCorrection`, and `AutoTuningCorrection`) when a non-original
temperament is active. For a first playback-only implementation, using
`PitchTuning` (cents, additive up the Organ→Windchest→Rank→Pipe chain) as the sole pitch
offset and ignoring the temperament/auto-tuning path (equal temperament, no retuning) is a
reasonable simplification — set `AcceptsRetuning=N`-equivalent behavior by default.

The two pitch paths, precisely (verified against source, 2026-08-26):

- GO's **default** temperament is "Original temperament" — an unset/unknown `.cmb`
  `Temperament` name falls back to `m_Temperaments[0]`, which is constructed
  original-based (`GOTemperamentList::GetTemperament` fallback comment "else return
  original temperament", `InitTemperaments` first entry; `GOTemperament` constructor
  defaults `isOriginalBased = true`). Under it, playback offset =
  `GetManualTuningPitchOffset()` = effective `PitchTuning` (+ runtime ManualTuning).
  Embedded sample pitch, `MIDIKeyNumber`, `HarmonicNumber` and `PitchCorrection` are all
  **ignored** — samples play as recorded, so a stock GO install renders each set's own
  recorded tuning.
- Every cent-table temperament, **including "Equal temperament"**, is constructed with
  `isOriginalBased = false` (`GOTemperamentCent` constructors pass `false`), which flips
  every pipe to `GetAutoTuningPitchOffset()`:
  `log2(HarmonicNumber/8)·1200 + (pipe_midi_key − sample_midi_key)·100 −
  sample_pitch_fraction_cents + PitchCorrection + AutoTuningCorrection`, i.e. the sample
  is retuned from its *declared recorded pitch* onto the pipe's nominal equal-tempered
  pitch. `PitchTuning` does **not** apply on this path — the two offsets are exclusive —
  which is why `PitchCorrection` exists at all ("keep me a semitone flat even when
  retuning", the baroque-pitch use case). The metadata term is skipped (offset
  contribution 0) when `GetEffectiveIgnorePitch()` is set (a `.cmb`-only key, not ODF) or
  the sample has no MIDI key (`m_SampleMidiKeyNumber == 0` — unity note 0 means "unset").
- `sample_midi_key`/`fraction` resolution (`GOSoundingPipe::Validate`): ODF
  `MIDIKeyNumber` if given, else the WAV `smpl` unity note; fraction = ODF
  `MIDIPitchFraction` if given, else **0 if ODF `MIDIKeyNumber` was given** (an explicit
  ODF key silences the file's own fraction), else the `smpl` fraction as cents.
- `AcceptsRetuning=N` (rank default **true**, `GORank.cpp` line 104) zeroes only the
  per-key `m_TemperamentOffset` — the metadata reconciliation above still applies under a
  non-original temperament.
- Validation gates (`GOSoundingPipe::Validate`): warns when a retunable pipe has no pitch
  information, warns when the original→auto difference exceeds ±600 cents, and logs an
  error past ±1800 cents (the offset is still applied; only the diagnostics stop).

Sources: `src/core/temperaments/GOTemperamentList.cpp` (`InitTemperaments`,
`GetTemperament` fallbacks), `GOTemperament.h` constructor defaults,
`GOTemperamentCent.cpp`; `src/grandorgue/model/GOSoundingPipe.cpp` lines 370-390
(`GetManualTuningPitchOffset`/`GetAutoTuningPitchOffset`), 415-470 (`Validate`), 526-560
(`UpdateTuning`, `SetTemperament`).

`MinVelocityVolume`/`MaxVelocityVolume` (§3/§4) define a **separate**, linear
velocity-to-volume-multiplier ramp (not part of the amplitude/gain tree): volume scales
linearly from `MinVelocityVolume`/100 at MIDI velocity 0 to `MaxVelocityVolume`/100 at
velocity 127.

Sources: `src/grandorgue/model/pipe-config/GOPipeConfigNode.h` lines 59-178 (all
`GetEffective*` accessors — multiplicative `GetEffectiveAmplitude`, additive
`GetEffectiveSum`-based accessors for gain/tuning/delay, `GetEffectiveBool` inheritance for
percussive/independent-release); `src/grandorgue/model/pipe-config/GOPipeConfig.cpp` lines
143-165 (`Load` defaults); `src/grandorgue/model/GOSoundingPipe.cpp` lines 370-390, 526-538
(`GetManualTuningPitchOffset`, `GetAutoTuningPitchOffset`, `UpdateAmplitude`,
`UpdateTuning`); `src/grandorgue/sound/providers/GOSoundProviderWave.cpp` lines 20-25
(`SetAmplitude`); velocity ramp in `src/grandorgue/sound/providers/GOSoundProvider.cpp`
(`SetVelocityParameter`/`GetVelocityVolume`, ~lines 190-201).

---

## 9. Supported sample audio

**Containers:** standard RIFF/WAVE (`.wav`) and WavPack (`.wv`, via `libwavpack`,
`src/core/GOWavPack.h`/`.cpp`). WavPack is auto-detected and unpacked into an in-memory PCM
buffer before the same WAV-chunk-parsing code processes it; a `.wv` file is functionally
just a compressed carrier for the same WAV structure GO expects (`GOWavPack::Unpack`
produces a `m_Wrapper` + `m_Samples` pair that feeds the same reader).
Source: `src/core/GOWavPack.h`/`.cpp`; used sample-file extension check
`"Audio sample files (*.wav;*.wv)"` in `src/grandorgue/gui/dialogs/settings/GOSettingsMetronome.cpp` line 28 (metronome dialog, but same set of supported formats applies to pipe samples).

**Bit depths / formats (from the WAV `fmt ` chunk):** `wFormatTag` must be `1` (integer PCM)
or `3` (IEEE float). For PCM (`1`): bits-per-sample must be a multiple of 8 and **≤ 24**
(i.e. 8/16/24-bit PCM supported; GO throws `"Unsupported PCM bit size"` above 24-bit). For
float (`3`): only exactly **32-bit** IEEE float is accepted (throws
`"Only 32bit IEEE float samples supported"` otherwise). Channel count is read directly from
the format chunk (mono/stereo typical; not further restricted in the code read here).
Source: `src/core/GOWave.cpp` lines 43-69 (`LoadFormatChunk`).

**Loop points / MIDI note metadata:** read from the WAV `smpl` chunk when present —
`dwMIDIUnityNote` (the WAV's own idea of its MIDI key number) and `dwMIDIPitchFraction`
(converted to cents: `fraction/UINT_MAX * 100.0`), plus zero or more `smpl`-chunk loop
records (`dwStart`/`dwEnd`, sample-frame positions). These are the fallback used when the
ODF doesn't declare `PipeNNNLoopCount`/`Loop%03dStart/End` for that attack (§4).
Source: `src/core/GOWave.cpp` lines 92-115 (`LoadSamplerChunk`).

**Release/cue marker:** read from the WAV `cue ` chunk — the maximum `dwSampleOffset`
across all cue points becomes `m_CuePoint` (`m_hasRelease = nbPoints > 0`). This is the
fallback release-start position used when neither `CuePoint` nor an explicit release file
is given for a percussive/single-file pipe (i.e. attack file contains an embedded release
after a cue point). Source: `src/core/GOWave.cpp` lines 71-90 (`LoadCueChunk`).

**Attack alignment.** GO does **not** do sample-rate-domain pitch-shifting/time-stretch
alignment; "alignment" here means: (a) trimming leading silence via `AttackStart` (§4) so
playback begins exactly at the declared frame offset, (b) picking, at note-on time, the
attack recording whose `MIDIKeyNumber`/embedded `smpl` MIDI-note metadata most closely
matches (used only for auto-tuning pitch correction, not for choosing *which* attack file
plays — that choice is purely the velocity/timing-based selection in §5), and (c) an
optional sample-domain crossfade (`LoopCrossfadeLength`, `ReleaseCrossfadeLength`, in
samples) applied at loop points and at attack→release splice points to avoid audible
clicks. There is no cross-pipe/cross-rank tempo or pitch alignment — each pipe's sample(s)
play back at their native sample rate, only re-pitched via the cents-based tuning offset
described in §8 (implemented as a resampling/pitch-shift at the mixer level, not covered by
files read for this document — **UNVERIFIED** exactly how the resampler works internally,
only that `SetTuning(cents)` is the API surface `GOSoundingPipe::UpdateTuning` calls).
Source: `src/grandorgue/model/GOSoundingPipe.cpp` lines 96-178 (`AttackStart`,
crossfade fields), lines 370-390 (pitch-offset computation feeding `SetTuning`).

---

## 10. Minimal valid organ — constructed example

**This file is not from GrandOrgue's sources; it is my own construction**, assembled purely
from the required-key tables above, to sanity-check a parser against the smallest ODF that
should satisfy GO's loader: one manual (no pedal), one stop with one new-style rank of 3
pipes, one windchest, zero enclosures/switches/tremulants/couplers/pistons.

```ini
[Organ]
ChurchName=Test Organ
HasPedals=N
NumberOfManuals=1
NumberOfWindchestGroups=1
NumberOfEnclosures=0
NumberOfTremulants=0
NumberOfRanks=1
NumberOfReversiblePistons=0
NumberOfDivisionalCouplers=0
NumberOfGenerals=0
DivisionalsStoreIntermanualCouplers=N
DivisionalsStoreIntramanualCouplers=N
DivisionalsStoreTremulants=N
GeneralsStoreDivisionalCouplers=N

[WindchestGroup001]
Name=Main Windchest
NumberOfEnclosures=0
NumberOfTremulants=0

[Rank001]
Name=Principal 8
FirstMidiNoteNumber=60
NumberOfLogicalPipes=3
WindchestGroup=1
HarmonicNumber=8
Pipe001=samples\principal8\c1.wav
Pipe002=samples\principal8\cs1.wav
Pipe003=samples\principal8\d1.wav

[Manual001]
Name=Manual I
NumberOfLogicalKeys=3
FirstAccessibleKeyLogicalKeyNumber=1
FirstAccessibleKeyMIDINoteNumber=60
NumberOfAccessibleKeys=3
NumberOfStops=1
Stop001=1

[Stop001]
Name=Principal 8
NumberOfRanks=1
FirstAccessiblePipeLogicalKeyNumber=1
NumberOfAccessiblePipes=3
Rank001=1
```

Notes on this example:
- `Manual000` is intentionally omitted because `HasPedals=N`.
- `[Stop001]`'s section number and `Manual001`'s `Stop001=1` indirection both happen to be
  `1` here, but per §2 they don't have to match — `Stop001=1` means "logical stop slot 1 on
  this manual is `[Stop001]`"; you could equally have written `Stop001=7` and named the
  section `[Stop007]`.
- Every optional key with a documented default (`AmplitudeLevel`, `Gain`, `PitchTuning`,
  `Percussive`, `AttackCount`, `ReleaseCount`, loop counts, etc.) is omitted here to exercise
  default-handling; a stricter/"clean" ODF would normally still declare `AcceptsRetuning`,
  `Displayed`, etc. explicitly.
- The three sample files (`c1.wav`, `cs1.wav`, `d1.wav`) are assumed to carry their own
  `smpl`-chunk MIDI note/loop metadata (§9), since no `PipeNNNMIDIKeyNumber` or
  `PipeNNNLoopCount` keys are given — a real organ package would ship such WAVs (or declare
  the metadata explicitly in the ODF).

---

## Sources consulted

All of the following were read directly from a shallow clone of
`https://github.com/GrandOrgue/grandorgue` (`master` branch, as of 2026-08-08). No wiki or
third-party ODF-reference documents were used — the repository's `help/grandorgue.xml`
(the in-app user manual) does not contain an ODF key reference, so it was not a source for
this document.

- `src/core/config/GOConfigFileReader.cpp` — INI-syntax tokenizer, encoding detection.
- `src/core/config/GOConfigReader.cpp` / `.h` — typed key readers, Y/N boolean parsing,
  range validation/throw behavior.
- `src/core/config/GOConfigReaderDB.cpp` — case-sensitivity fallback, duplicate handling.
- `src/grandorgue/model/GOOrganModel.cpp` — `[Organ]` section, top-level counts and load
  order.
- `src/grandorgue/GOOrganController.cpp` — organ metadata keys (`ChurchAddress`,
  `OrganBuilder`, etc.), `InfoFilename` resolution.
- `src/grandorgue/model/GOManual.cpp` / `.h` — `[ManualNNN]` keys, pedal-numbering.
- `src/grandorgue/model/GOStop.cpp` / `.h` — `[StopNNN]` old-/new-style rank references.
- `src/grandorgue/model/GORank.cpp` / `.h` — `[RankNNN]` keys, pipe dispatch (sample /
  `DUMMY` / `REF:`).
- `src/grandorgue/model/GOPipe.cpp` / `.h`, `GOSoundingPipe.cpp` / `.h`,
  `GOReferencePipe.cpp` / `.h`, `GODummyPipe.h` — pipe types, per-pipe keys, attack/release
  loading.
- `src/grandorgue/model/GOWindchest.cpp` / `.h` — `[WindchestGroupNNN]` keys, tremulant
  toggling, enclosure attenuation.
- `src/grandorgue/model/GOTremulant.cpp` / `.h` — `[TremulantNNN]` keys, Synth vs Wave.
- `src/grandorgue/model/GOCoupler.cpp` / `.h` — `[CouplerNNN]` keys, Bass/Melody logic.
- `src/grandorgue/model/pipe-config/GOPipeConfig.cpp` / `.h`,
  `GOPipeConfigNode.cpp` / `.h` — amplitude/gain/tuning/percussive inheritance tree.
- `src/grandorgue/sound/providers/GOSoundProviderWave.cpp` / `.h`,
  `GOSoundProvider.cpp` / `.h` — attack/release selection algorithm, loop handling, gain
  formula.
- `src/core/GOWave.cpp` / `.h`, `GOWaveTypes.h` — WAV chunk parsing (`fmt `, `cue `,
  `smpl`), bit-depth support.
- `src/core/GOWavPack.cpp` / `.h` — WavPack (`.wv`) support.
- `src/grandorgue/loader/GOLoaderFilename.cpp` / `.h` — path resolution, backslash
  separator handling.
