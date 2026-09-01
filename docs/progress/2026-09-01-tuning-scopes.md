# 2026-09-01 — tuning scopes: sets, stops, ranks within stops

Until now an organ had two tuning levels: the instrument (`[tuning]`)
and a division (`[[manual]]` fields). The user's brief: a global
preference per organ (default *as recorded*), everything new loading
at it, and the ability to retune a whole sample set, a division, a stop
— or one rank of a mixture — to anything: temperament, divisions per
octave, reference, Scala scale. And a UI that makes the resolution
legible instead of a puzzle.

## Model (server `console.rs`, `tuning.rs`)

- `Follow { Auto, Division, Source, Organ }` — what a stop plays when
  it has no tuning of its own. `TuningScope { Organ, Source, Division,
  Stop, Rank }` — where resolution landed, reported per stop.
- `Console::stop_tuning_resolved(stop)`: own → pin → auto (division's
  own, else set's own, else instrument). `voice_tuning(stop, rank)`
  puts a rank's own tuning first. Pricing (`voices_for_key`) and live
  drift (`retune_held`) both go through it; `Speaking` now remembers
  the stop and rank a voice sounded through instead of its manual.
- Transposition stays on `effective_tuning(division)` — set/stop/rank
  tunings have their transpose zeroed on install.
- Homes per scope: the bank exposes `rank_anchors`; `load.rs` builds a
  `HomeTuning::at_anchor(median of the set's ranks)` per source when
  the organ holds several, and the console derives a rank's home from
  its anchor. Set- and rank-scoped tunings are stamped with those, so
  *as recorded* at set scope reads the set's own a′.

## File

- `[sources.<alias>.tuning]` — `sidecar::TuningOverride` (temperament,
  edo, reference_key, reference_hz, scale, keymap, pipes; no transpose).
- `[[tuning.stop]]` rows — `stop`, `manual`, optional `rank`, and either
  `follow = "…"` or the override fields. `config::write_composite_
  source_tuning` / `write_composite_stop_tuning` upsert one row per
  (stop, manual, rank), leaving `[tuning]` implicit when it has no
  fields of its own. A bare-path source becomes a table with `path`.
- `Assembled` gains `source_tuning` and `rank_sources` (alias per rank).

## API

`POST /api/tuning` with `source=<alias>`, `stop=<id>`, `stop=<id>&
rank=<id>`, `follow=auto|division|source|organ|own` (`reset=1` /
`follow=organ` / `follow=stop` return a scope to its parent). Snapshot:
`source_tuning`, `stop_tuning`, `rank_tuning`, `source_home`, and per
stop `tuning: {scope, follow}` + `ranks: [{id, name, own}]`.

## Decisions (user, 2026-09-01)

1. Automatic precedence: division > sample set > instrument.
2. Following scopes show the resolved spec dimmed and disabled; an
   explicit switch to *Own tuning* (seeded, so audibly a no-op).
3. Ranks are tunable within their stop, never across stops.
4. The default is spelled "As recorded" in the UI (`original` in files).
