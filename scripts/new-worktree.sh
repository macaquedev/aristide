#!/usr/bin/env bash
# Give one task its own checkout, so concurrent sessions can't clobber each
# other's edits or fight over Cargo's target-directory lock.
#
#   scripts/new-worktree.sh feat/sinc-tails
#
# Creates ~/aristide-wt/feat-sinc-tails on a new branch off origin/main, and
# links in the gitignored fixtures (the demo sample set, reference/) that a
# fresh checkout would otherwise be missing.

set -euo pipefail

branch=${1:-}
if [[ -z $branch ]]; then
	echo "usage: ${0##*/} <type>/<slug>   e.g. feat/sinc-tails" >&2
	exit 1
fi

here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
# The main checkout, even when this is run from inside another worktree.
main=$(git -C "$here" worktree list --porcelain | sed -n '1s/^worktree //p')
dest=${ARISTIDE_WORKTREES:-$HOME/aristide-wt}/${branch//\//-}

# Each worktree carries its own ~6 GB target/, and the box has 4 cores.
existing=$(git -C "$main" worktree list | wc -l)
if ((existing >= 4)); then
	echo "warning: $((existing - 1)) worktrees already exist — builds will thrash past three." >&2
fi

git -C "$main" fetch --prune origin
git -C "$main" worktree add -b "$branch" "$dest" origin/main

# Fixtures are gitignored, so they don't come with the checkout. Link each
# untracked entry individually: the Aristide sidecar inside the demo set *is*
# versioned, and linking the whole directory would shadow it.
fixture=$main/testsets/grandorgue-demo
if [[ -d $fixture ]]; then
	mkdir -p "$dest/testsets/grandorgue-demo"
	for item in "$fixture"/*; do
		name=${item##*/}
		if [[ -n $(git -C "$main" ls-files -- "testsets/grandorgue-demo/$name") ]]; then
			continue
		fi
		ln -sfn "$item" "$dest/testsets/grandorgue-demo/$name"
	done
fi
[[ -d $main/reference ]] && ln -sfn "$main/reference" "$dest/reference"

cat <<EOF

worktree: $dest
branch:   $branch

  cd $dest
  # work, commit, then:
  git push -u origin $branch && gh pr create --fill

When the PR merges: git worktree remove $dest
EOF
