# Organ: blank stops, drag, rename and delete

Alex, 2026-09-27: in Organ, Add stop should offer a blank stop to shape
later; stops should drag around the list and between divisions; a selected
stop should delete with the Delete key, and right-click should offer Rename
and Delete with the appropriate dialogs.

## UI

- Right-click (long-press on touch) on a stop, division or coupler opens a
  menu: Rename (a dialog with the name selected) and Delete (a confirmation
  naming what goes). Delete or Backspace on the selected item asks the same;
  F2 renames. Editor buttons now read Delete stop / division / coupler.
- Stops drag by mouse from anywhere on the row, and by touch from the grip,
  so the list still scrolls. A line marks where the stop will land; Escape
  cancels. Within a division this sets the order; to another division it
  moves the stop (live, no rebuild), then places it. A division that already
  has a stop of that name refuses with a dialog.
- Add stop starts with Blank stop: a name (suggested "New stop", refused if
  the division has it) and Add blank stop. The new stop opens with its one
  event on its own, empty pipes ("0 voices per key"); choosing another stop
  as that event's source gives it sound.
- The stop editor's header follows live renames and moves.

## Server and file

- `/api/organ/stop/order?manual=&stops=id,…` lists every stop of a division,
  live and kept in `[console.order]` (the loader already read it). The
  snapshot deals stops out in that order, so Play follows it too. Renames
  and moves keep it in step.
- `/api/organ/stop/blank?on=&name=` writes `[[blank]] name/on` and rebuilds.
  `StopProvenance.blank` marks such stops so rename and delete edit that line.
- Rules follow their stops: stop rename, move and manual rename rewrite the
  `[[rule]]` rows and events naming them, and deleting a stop or division
  drops its rules and the events that sounded it. Before this, a custom stop
  lost its rule on the next rebuild after any of those edits.

Migration: files with `[[blank]]` do not load in older builds (the organ
schema denies unknown fields). Older files load unchanged.

## Validation

- `cargo test -p aristide-formats` and `-p aristide-server`: new tests cover
  blank assembly and moves, stop order over the API (live, saved, rename,
  move, stale lists refused), and blank lines and rules through rename,
  move, manual rename and removal. Three real-time timing tests in `bank.rs`
  fail only when the whole suite runs in parallel on this loaded machine
  and pass alone.
- `ARISTIDE_LIVE=1 bunx playwright test organ-structure-live organ-live`
  against a server with its own config: menu rename, Delete key with Cancel,
  drag within and across divisions, blank stop added, sourced, renamed,
  moved and kept through a rebuild, then deleted. Screenshots reviewed at
  1280 × 800.

## Gaps

- Touch drag was exercised only through the code path, not on a
  touchscreen.
- A blank stop's first event points at itself; the editor does not yet
  allow a rule with no events.
- Dragging does not auto-scroll the page, only the stop list.
