# Library as a list, with rename and delete

Alex, 2026-09-27: the Library "looks hideous" and "functionally makes no
sense"; an organ that isn't loaded should be renameable and deletable, the
organs should appear in a proper list, and delete should ask "are you sure".

## Panel (`desktop/src/library/`)

- One row per organ, most recently played first: name, then format, sample
  folder and when it last played. The playing organ carries a Playing badge.
- Tapping a row loads it; tapping the playing organ returns to Play without
  reloading. Other rows are disabled while a load is running.
- A row's ⋮ menu offers Rename (inline field, Escape cancels) and Delete.
  Delete opens a modal, "Are you sure you want to delete …?", and says what
  will happen: Aristide's own organ file loses its settings and edits, and the
  sample files stay; anything else only leaves the Library. The playing organ
  cannot be deleted.
- A search field appears once there are more than six organs.
- Load from GrandOrgue/Hauptwerk opens the operating system's file picker
  (`tauri-plugin-dialog` 2.7, kept on Tauri 2.11). Organ's "Add a sample set"
  and Tuning's Import Scala use it too; the in-app folder browsers are gone.
  In a plain browser there is no system picker, so those buttons are disabled;
  tests inject `globalThis.aristidePickFile` in its place. `/api/browse` stays
  on the server for API clients but the UI no longer calls it.
- New organ asks for a name inline, creates a blank organ (`/api/organ/new`),
  loads it and opens Organ, where divisions and stops are added.
- Below 700 px the folder is hidden so the format and last play stay readable.

## Server

- `POST /api/library/rename?path=&name=` renames an organ whether or not it is
  loaded. It writes the organ file's `name`, or a sample set's sidecar, and
  moves wiring keyed by the old name. A name another Library organ already
  uses is refused, because two organs with one name would share wiring. It
  delegates to `rename_organ` for the loaded organ.
- `POST /api/library/delete?path=` refuses the loaded organ. It deletes a
  `.toml` in the config's `organs/` directory (`config::is_owned_organ`) and
  otherwise only forgets the entry. It never deletes a sample set or a file
  elsewhere.
- `GET /api/library` returns each present entry's format (GrandOrgue,
  Hauptwerk, Aristide) and sample folder, read from disk on request rather
  than on every state poll.
- Library entries in the state now carry `played`, `loaded` and `owned`.
  `[[library]]` in `midi.toml` gains an optional `played` (Unix seconds). Older
  files load unchanged, and entries gain it when they next load.

## Gaps

- The spec's size in memory and the list of snapshots per instrument are not
  shown: the server has neither figure for an organ that is not loaded.
- Loading still shows only the server's loading text; there is no progress
  bar, rank count or cancel.
- Delete has no undo. Deleted owned organ files are removed outright.
- Editing an adopted organ still saves a copy named "… (edited)". That copy is
  a separate row, which rename and delete can now tidy.
