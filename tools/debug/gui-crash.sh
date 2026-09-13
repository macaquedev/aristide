#!/usr/bin/env bash
# Run the actual console binary (not `cargo run`) under GDB.
set -euo pipefail

if (( $# == 0 )); then
    echo "Usage: bash tools/debug/gui-crash.sh /path/to/aristide-console [console args…]" >&2
    exit 2
fi
if ! command -v gdb >/dev/null 2>&1; then
    echo "gdb is required; install it with your system package manager, then retry." >&2
    exit 1
fi

report=$(mktemp "${TMPDIR:-/tmp}/aristide-gui-crash.XXXXXX.log")
echo "Use the GUI normally, then close it. Diagnostic output: $report"
# Follow the GUI parent, leaving its audio server and WebKit subprocesses
# independent. On a fatal signal GDB stops before teardown loses the stack.
# Do not capture locals or a core: they can be huge with an organ loaded.
gdb --batch --quiet --return-child-result \
    -ex 'set pagination off' \
    -ex 'set confirm off' \
    -ex 'set print thread-events off' \
    -ex 'set follow-fork-mode parent' \
    -ex 'set detach-on-fork on' \
    -ex 'handle SIGPIPE nostop noprint pass' \
    -ex run \
    -ex 'thread apply all bt' \
    -ex 'info sharedlibrary' \
    --args "$@" 2>&1 | tee "$report"
