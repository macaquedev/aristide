# Tuning panel connected

The Tuning tab now opens the Channel strip desk on the live engine
(`desktop/src/tuning/TuningPanel.tsx`) instead of a placeholder. It lists the
whole instrument, each division and, collapsed under it, each stop, from
`GET /api/tuning`, which resolves every scope and says which halves it owns.
Each edit is one `POST /api/tuning` for that scope, heard from the next note
(held notes glide) and saved by the engine; there is no save button. The
panel re-reads the scopes after every change and once a second.

Reference and Scale each have a Follow switch, so a division can keep the
instrument's pitch with its own temperament and still follow a later change
of pitch. Temperament offers As recorded, the engine's catalogue and Custom.
Import Scala opens a sheet that browses `.scl`/`.kbm` files
(`GET /api/browse?kind=scala`); without a `.kbm`, Step 1 key places the
scale. In perform mode every control is disabled; unlocking enables them.

A sample set's own organ stays as the set defines it: the engine refuses
edits below the instrument with 409. The panel's first such edit saves the
organ as the player's copy, "<organ> (edited)", and retries, which is the
spec's automatic layer without interrupting sound (`save_as` swaps the file,
it does not reload). The browser request helper no longer parses error
bodies as JSON, so 409 and 400 reach callers as statuses.

The top bar's Undo steps back through the Tuning panel's own edits while it
is open, by re-sending each scope's previous state. It is not yet the spec's
global, persistent undo. Long-pressing a tuning number explains that control
assignment is not available yet instead of doing nothing.

Gaps: sets and ranks are tunable in the engine but not listed; a stop that
resolves through its set is still linked to its division in the header;
dragging a number sends a request per step; the Play division tag shows the
engine's temperament id. Audible behaviour is covered by the engine's tests
and still needs Alex's ear on a desktop run.

Validation: `tests/tuning-live.spec.ts` (run with `ARISTIDE_LIVE=1` against a
server with an organ, `ARISTIDE_SCALES` naming a folder with
`bohlen-pierce.scl`) drives a real engine through a temperament and pitch
change, a division owning only its scale and following the pitch, a Scala
import, Undo and the perform-mode lock; it passed against the demo organ with
an isolated config directory. The study tests, still silent, cover the same
desk offline. Screenshots reviewed against the control-flow and visual rules.
