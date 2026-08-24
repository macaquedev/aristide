# 2026-08-24 — keyboard kinds + Scala scales (M6 opens)

Two sessions' work, one arc: the console stops deducing what a keyboard
is, and a division can play a scale that isn't 12 notes long.

## Keyboard kinds (declared, never deduced)

`ManualKind` (`manual` | `pedal` | `microtonal`) replaces the model's
`pedal: bool`, threaded model → loaders → organ file (`[[manual]]
kind`, default expressed by absence) → snapshot JSON → console. GO's
`Manual000`→pedal stays a loader fact, not an inference; an unknown
kind in a file warns and loads as a hand keyboard. The add menu offers
all three; right-click a keyboard in edit mode → Change type
(structural: `/api/organ/manual/kind`, file line + reload) or Change
tuning (live popover over the existing `/api/tuning?manual=`).
`microtonal` renders as a Terpstra-style hex field — uniform hexes,
two interlocked rows, no natural/sharp vocabulary — same key wiring
as every keyboard. The kind is a console fact; pitch stays the tuning
layer's business.

## Scala tuning (M6 first slice; sinc resampler prerequisite already
landed 2026-08-08)

- `aristide-model::scala`: `.scl`/`.kbm` parser, pure std, never
  panics; `key_frequency` = the Hz a key sounds under scale+mapping.
- The pitch seam generalized: `Tuning::deviation_cents(key)` is now
  the one key→pitch conversion. The console splits the deviation into
  whole semitones — **re-anchoring the key to the nearest recorded
  pipe** — and a ≤50-cent residual bend folded into the voice rate.
  Voice identity moved to cent resolution (nominal×100+bend): keys
  bent apart on one pipe are distinct voices, keys a scale sends to
  one pitch share one. Temperaments deviate under a semitone, so
  their behaviour is bit-identical (tests unchanged).
- `[[manual]] scale/keymap` and sidecar-wide `[tuning] scale/keymap`
  name the files (resolved against the organ file's dir; load failure
  warns and keeps the temperament). `/api/tuning` takes
  `scale=`/`keymap=`/`scale=off`; snapshot carries the scale's name
  and note count. Without a `.kbm`: linear mapping, a′ anchored at
  `a4_hz`, re-anchored when a′ changes. Unmapped `.kbm` keys are
  silent by design.
- Proven by test on 19-EDO: mid-ladder keys sound the pipe 2 below
  bent +15.8¢, the octave lands on a real pipe exactly, the next
  manual is untouched.

Deferred within M6, in order: MPE/MIDI 2.0 per-note pitch + live
tuning drift (both need a ramped `SetVoiceRate` engine command —
today a voice's rate is fixed at StartVoice), Lumatone multi-channel
input maps, effects graph + multichannel routing (overlaps M4).

Example scales for testing live in `scales/` (generated, public
domain): 19-EDO, 24-EDO quarter-tones, Bohlen-Pierce equal-tempered.
