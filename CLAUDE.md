# Aristide — project context for Claude sessions

Read `DESIGN.md` first: it holds the architecture, all locked decisions, and the
milestone plan (M0–M7). Do not re-litigate locked decisions without the user.

## Ground rules

- **RT invariants** (aristide-engine audio path): never allocate, lock, or do I/O on
  the audio thread. Control→RT via lock-free queues only. Uphold these in every PR.
- **No 12-EDO assumptions** in the model or engine: pitch travels as Hz/cents;
  MIDI-note→frequency conversion happens control-side in one replaceable place.
- **Legal boundary**: encrypted Hauptwerk sample sets are permanently out of scope.
  No decryption, ever. Only the open GrandOrgue format and unencrypted HW packages.
- Sample-set formats are read as-is; Aristide-specific data (voicing, tuning,
  routing, effects) goes in TOML sidecar files, never into the loaded set.
- `docs/go-odf-notes.md` is the authority for the GrandOrgue format — it was
  compiled from GO's loader source. Extend it (with citations) rather than guessing.

## Practical notes

- Test fixture: `testsets/grandorgue-demo/` (gitignored; 21 MB). If missing, unzip
  `packages/*.orgue` (a plain zip) from a shallow clone of GrandOrgue/grandorgue.
  Its samples are WavPack with `.wav` extensions — that's normal; `wav::read` sniffs.
- Requires system `libwavpack` (+ `libasound2-dev`/`alsa-lib` to build cpal).
- This machine may be a headless dev server: `cargo test`/`clippy` prove correctness
  here; anything audible is verified by the user pulling to their desktop and running
  `cargo run --release -p aristide-server`. Never assume an audio device exists.
- Commit style: conventional commits, atomic, imperative subject ≤50 chars.

## Git workflow — read before you write code

`main` must always build and pass tests: it is what the user's test rig pulls to
hear anything, and what `git bisect` walks when an audio regression turns up by
ear days later. GitHub enforces it — **direct pushes to `main` are rejected**.

Every task, no exceptions:

1. **Claim.** `gh issue list` for what's queued; `git fetch --prune && git branch -r`
   plus `gh pr list` for what other sessions already took. Every agent pushes as the
   same GitHub account, so the remote branch list — not the assignee — is the claim
   register. Push your branch early to stake it.
2. **Branch** `<type>/<slug>`, type from the commit-message set (`feat/sinc-tails`).
   If another session may be active, get your own checkout instead of sharing this
   one: `scripts/new-worktree.sh feat/sinc-tails` (three worktrees max — 4 cores).
3. **PR.** `git push -u origin <branch>`, then `gh pr create` with a body following
   `.github/pull_request_template.md` (not `--fill`, which skips it). Say what CI
   cannot: what this changes about the *sound*, and how to hear it.
4. **Merge it yourself** once `ci` is green: rebase on `origin/main`, then
   `gh pr merge --squash --delete-branch`.

**Stop and ask the user instead of self-merging** when the change touches a locked
`DESIGN.md` decision or the legal boundary, weakens the RT invariants, alters how the
organ sounds by default, breaks the sidecar/HTTP surface, adds a dependency, or
removes a feature. For audible changes, prefer merging behind a flag that defaults to
today's behaviour over blocking on a listen.

End a session with a **new** file `docs/progress/YYYY-MM-DD-slug.md` plus one link
line atop `docs/PROGRESS.md` — never by editing someone else's entry.

Full detail — worktrees, conflict lanes, what CI can't check: `docs/workflow.md`.
