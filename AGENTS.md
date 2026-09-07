# Aristide agent instructions

Read `CLAUDE.md` for project context and ground rules, and follow its instruction
to read `DESIGN.md` before implementation work.

## Commit and push workflow

The user has authorized this workflow for all future tasks in this repository:

- Complete work as focused, atomic commits: each commit contains one coherent
  change and leaves the repository in a working state.
- Run the checks appropriate to each change before committing. Do not commit
  unfinished refactors or changes with failing required checks.
- Use conventional commit messages with an imperative subject of at most 50
  characters.
- Stage only changes belonging to the task; preserve unrelated work in progress.
- Commit directly on `main` and immediately push each completed commit to
  `origin/main`. Do not create a feature branch or pull request unless the user
  requests one. No additional confirmation is needed to commit or push.
- If working in a separate worktree or branch, integrate the completed change
  onto `main` safely and push `main`; do not overwrite unrelated work.
- Use normal fast-forward pushes, never force-push. If the remote has advanced,
  reconcile safely and rerun affected checks before retrying. If a conflict,
  branch protection, or authentication issue prevents pushing, report the
  blocker and do not claim that the push succeeded.

A Git commit is atomic locally; committing and pushing are separate operations.
If a push fails, retain the local commit and report that it remains unpushed.
