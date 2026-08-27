# 2026-08-27 — divisions per octave, first-class

N-EDO used to require writing a Scala file. Now it's a tuning field:
`edo` (default 12) lives beside the temperament in `[tuning]` and per
manual on `[[manual]]`, travels the same `/api/tuning` contract
(`edo=N`, live, glide and all), and shows in both tuning UIs as a
"Divisions / octave" number.

The semantics keep the file honest and the vocabulary straight:

- **Temperaments are twelve-class vocabulary.** Away from 12 they mean
  nothing, so they go dormant exactly the way they do under a Scala
  scale — and the UIs (Preferences → Tuning, and the per-manual
  popover) show the temperament select only while the count is 12.
  The file writer enforces the same: `edo ≠ 12` writes an `edo` line
  and drops the temperament line; back at 12 the reverse. Never two
  claims at once.
- **Pitch**: away from 12, key `k` sounds `a′ · 2^((k−69)/edo)` — the
  same ladder a generated N-EDO scale with the linear mapping gives,
  without the ceremony of a file. One new branch in the one
  key→pitch place (`Tuning::deviation_cents`).
- **A scale still supersedes everything**; naming an `edo` (or a
  temperament) is the way back out of one.
- **`Tuning::steps_per_octave()`** is the new question to ask instead
  of assuming 12 — the hex-field presets ask it, so Bosanquet on a
  31-EDO manual comes out (5, 2) with no further ceremony.

Range: 1..=311. The menu-bar readout leads with whatever governs:
scale name, else "31-EDO", else the temperament.

Verified end-to-end on the rig: field edits in both UIs, temperament
reveal/hide, file round-trip (`edo = 31` on the manual, temperament
line gone, reload restores it), and the 31-EDO Bosanquet preset.
