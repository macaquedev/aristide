# How work lands in Aristide

This project is built mostly by AI coding sessions, often several at once, over a
long time. That shapes the rules below more than any house style does. Read this
once at the start of a session; the short version lives in `CLAUDE.md`.

## The one invariant

**`main` always builds, always passes tests, always runs.**

Everything else here is downstream of that, for two reasons:

1. `main` is the test rig. The maintainer's organ console is on a different
   machine; verifying anything audible means pulling `main` and running
   `aristide-server` there. A broken `main` stalls the only loop that can hear.
2. Audio regressions are found by ear, days later. "When did the crackle come
   back?" is answered by `git bisect`, and bisect only works if every commit on
   `main` compiles and passes. That is why history is linear and every merge is
   one squashed commit.

GitHub enforces this: `main` rejects direct pushes and requires a green `ci`
check. If you find yourself wanting to bypass it, you want a revert instead.

## The loop

```
claim an issue  →  branch  →  commit  →  push  →  PR  →  CI green  →  squash-merge
```

### 1. Claim

Work lives in GitHub issues, labelled by milestone (`M4`, `M5`, …).

```sh
gh issue list --label M4 --state open
gh issue view 42
```

Before starting, look at what other sessions are already doing — every agent
here pushes as the same GitHub account, so an assignee tells you nothing. The
branch list is the real claim register:

```sh
git fetch --prune && git branch -r
gh pr list
```

Claim by pushing your branch early — the first commit can be a stub. A remote
branch named `feat/sinc-tails` is the signal that someone is on it. Add a
comment on the issue naming your branch if the work will run long.

### 2. Branch

Name it `<type>/<slug>`, with the type drawn from the same set as commit
messages: `feat`, `fix`, `perf`, `refactor`, `docs`, `style`, `test`, `chore`.

```sh
git switch -c feat/sinc-tails
```

If another session might be active, work in a **worktree** instead of the shared
checkout — two agents editing `/home/macaque/aristide` at once corrupt each
other's edits and fight over Cargo's target-directory lock:

```sh
scripts/new-worktree.sh feat/sinc-tails
```

That creates `~/aristide-wt/feat-sinc-tails` with its own branch and links the
gitignored fixtures into it. Each worktree carries its own `target/` (~6 GB) and
the box has 4 cores, so **three concurrent worktrees is the ceiling** — more and
builds thrash. Remove yours when the PR merges:

```sh
git worktree remove ~/aristide-wt/feat-sinc-tails
```

### 3. Commit

Conventional commits, atomic, imperative subject ≤50 chars — unchanged from
before. Keep a PR to one logical change; if it grows past roughly 400 lines,
split it. Small PRs merge faster, conflict less, and revert cleanly.

### 4. Open the PR

```sh
git push -u origin feat/sinc-tails
gh pr create --title "feat(engine): …" --body-file /tmp/pr-body.md
```

Fill in the sections from `.github/pull_request_template.md` — `--fill` skips the
template, so write the body yourself. The section that matters is **what this
changes about the sound**, and how to hear it: CI compiles and asserts, it has
no ears.

### 5. Merge

When `ci` is green, rebase and squash-merge it yourself:

```sh
git fetch origin && git rebase origin/main && git push --force-with-lease
gh pr merge --squash --delete-branch
```

**Except** — stop and ask the maintainer instead of self-merging when the change:

- alters a locked decision in `DESIGN.md` (the core-decisions table), the legal
  boundary on encrypted sample sets, or a milestone's definition;
- weakens the RT invariants — anything that could allocate, lock, or do I/O on
  the audio thread;
- changes how the organ sounds by default (voicing, tuning, wind, release or
  resampling maths, new default parameter values);
- breaks the sidecar TOML schema or the HTTP/IPC surface;
- adds a dependency, or removes a feature.

For audible changes there is usually a better move than waiting: put the new
behaviour behind a sidecar or config flag that **defaults to the current
behaviour**, merge that, and ask for a listen afterwards. `main` keeps moving,
and the maintainer can A/B by flipping one value. When that is genuinely
impossible, label the PR `needs-ears` and leave it open.

### 6. Close the session

Add a progress note — a **new file**, never an edit to someone else's:

```
docs/progress/YYYY-MM-DD-short-slug.md
```

and one line linking it at the top of the list in `docs/PROGRESS.md`. Then close
the issue and remove your worktree.

## When `main` breaks

Whoever notices owns it, ahead of whatever they were doing. Prefer reverting the
offending squash commit over a forward fix — it is one command, it is always
correct, and the author can re-land at leisure:

```sh
git revert <sha>
```

## Staying out of each other's way

- **Pick tasks in different crates.** `aristide-engine`, `aristide-formats`,
  `aristide-server` and `aristide-console` are the natural lanes; two concurrent
  PRs in `engine/src/lib.rs` will conflict.
- **Hot files:** `DESIGN.md`, `CLAUDE.md`, `Cargo.toml` workspace deps, and the
  `docs/PROGRESS.md` index. Touch them in a small, separate commit and merge it
  promptly rather than letting it sit on a long branch.
- **Research and progress notes are one file per topic/session** — under
  `docs/research/` and `docs/progress/` — precisely so they never conflict.
- **`Cargo.lock` conflicts:** never hand-merge. Take either side and regenerate:
  `git checkout --theirs Cargo.lock && cargo check`.

## What CI covers, and what it can't

Covered: `cargo fmt --check`, `cargo clippy -D warnings`, and
`cargo test`, on Linux, for every crate except `aristide-console` (a Tauri shell
with no tests that would drag webkit2gtk into every run). The GrandOrgue demo
set is fetched best-effort so the loader tests actually execute.

Not covered, and never will be by a shared runner:

- **How it sounds.** Only the maintainer's rig can judge that.
- **Timing.** Xruns, callback budget, and the render benchmark are meaningless on
  a noisy cloud runner — run those locally (`cargo bench`-style targets in
  `aristide-server`) and quote numbers in the PR body.
- **Real MIDI hardware** and real (large) sample sets.

So a green check means "it compiles, it is clean, the assertions hold". It does
not mean the organ sounds right. Say which of the two you have verified.

## Starting a session from cold

```sh
git fetch --prune
```

1. `DESIGN.md` — architecture, locked decisions, milestone plan.
2. `docs/PROGRESS.md` — the last few entries say where things actually stand.
3. `gh issue list` — what is queued; `git branch -r` and `gh pr list` — what is
   already in flight.
4. Claim something, branch, and follow the loop above.
