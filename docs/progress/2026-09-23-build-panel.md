# Build panel connected

Spec sections: Core model (a stop is a programmable rule); Build, including
the selected piano-roll direction (Split desk) and Behaviour; Control flow
rules 2, 3, 4, 5, 6 and 7; Visual rules.

The Build tab now opens the Split desk on the live engine
(`desktop/src/build/BuildPanel.tsx`). In edit mode, long-press or right-click
a stop in Play to open it. Stops are listed down the left, grouped by
division. The editor has two piano rolls, Key down + ms and Key up + ms,
and the event inspector beside them. It reuses the study's roll canvas,
number control and timestamp behaviour. Every edit sends the whole rule
(`POST /api/organ/rule`), is heard from the next note and saves itself.
When requests arrive faster than they complete, only the latest is sent.
A sample set's own organ is saved as "<organ> (edited)" on the first edit,
as in Tuning and Route. The top bar's Undo steps back through Build's
edits while the panel is open. Reset returns the stop to its conventional
single event. The header shows the stop's division and its voices per key,
plus a Tuning button that opens that stop's scope in the Tuning panel.
Custom stops carry the ◇ mark in Play, in Build's stop list and in the header.

## Model and engine

- Engine: `StartVoice.end_frames` lets a voice release itself a set number
  of frames after it starts, onset included. `StopVoiceIn` arms the same
  timer on a held voice. Timers fire at the nearest chunk boundary, and the
  countdown pass is skipped while no voice has one. The golden render is
  unchanged.
- Rule (`crates/aristide-server/src/rule.rs`): shared timestamps, including
  the fixed `down` and `up` at 0 ms, and events. An event has a source (a
  stop of this organ, with all its ranks or one of them), a pitch in cents,
  a level in dB, a start timestamp, and an optional end timestamp. Invalid
  timings, duplicate timestamps and unknown sources are refused.
- Console: key-down events start with an onset delay and may end at a
  timestamp. An early key-up stops them and cancels starts still pending,
  because the engine drops a voice that has not yet spoken. A key-down event
  that holds past key-up keeps speaking to key-up + ms, but only if it had
  started before the key was released. Key-up events start on note-off and
  always end by themselves; they are separate from the sample's own release
  tail. Timed events use their own pipe-sharing lane (`PipeKey.event_lane`),
  so they never merge with the organ's held pipes. A conventional stop plays
  exactly as before. Held keys re-speak the stop when its rule changes.
- Layer: `[[rule]]` tables in the organ file, keyed by manual and stop name.
  Events name their source stop and rank the same way. A stop without a
  rule writes nothing.
- API: `GET /api/rule?stop=`; `POST /api/organ/rule?stop=&rule=<json>` or
  `&reset=1`. The state snapshot marks stops that have a rule (`custom`).

## Gaps

- Live references to another stop's *rule* are not built; a source is that
  stop's pipework. Nested references and cycle checks are still needed.
  Borrowing ranks from another loaded organ is not built either.
- Per-event Output, Key range and Tuning (follow the stop or keep the
  source's own) are not built. Every event follows the stop's routing and
  tuning.
- Per-key editing (hold a key and edit) and assigning a control to a number
  are not built. Assignment says so instead of doing nothing.
- The pitch guide chooses 12, 19 or 31 equal. It does not read the
  instrument's tuning.
- Endings are accurate to the audio chunk, not the sample.
- Renaming or moving a stop leaves its `[[rule]]` under the old name; the
  load warning names it. Older builds reject files that contain `[[rule]]`.
- Undo is panel-local, not the global persistent undo. There are no
  snapshots yet.
- Two keys that reach the same pipe at the same pitch through the same
  timed event share one voice (a pipe speaks once), so a finite ending cuts
  both.

## Validation

- `cargo test -p aristide-engine` (98), `-p aristide-formats` (89) and
  `-p aristide-server` (224) pass. New tests cover self-ending voices, timers
  that expire during the onset, both key edges of a rule, holds past key-up,
  live rule edits under held keys, refused sources, and a file round trip
  through the API.
- `desktop/tests/build-live.spec.ts` (`ARISTIDE_LIVE=1`) passed against a
  release server running the demo organ, with an isolated config and master
  gain at 0. Steps: open Build through the stop; add an event (the organ is
  saved as a copy automatically); set pitch through the stepper; give it a
  finite ending that creates a timestamp; Undo; go to Play and back; check
  the Play mark; Reset; relock. The held note stayed held throughout. The
  whole offline suite passes, with the play tests now opening the real
  editor.
- Screenshots at 1280 × 800 and 390 × 844 were reviewed against the control
  flow and visual rules. Checks: one accent for active items; the modified
  colour only on the ◇ mark; red only for Panic and Delete; stock Mantine
  controls; no horizontal page scroll on a phone. That review fixed the
  source button's label and the inspector's width on phones.
- Not verified by ear. Alex should try Titanique-like rules on a desktop run.
