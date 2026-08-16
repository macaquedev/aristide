## What and why

<!-- One paragraph. Link the issue: Closes #NN -->

## What this changes about the sound

<!-- CI has no ears. Say whether the render is affected at all, and if so what
     to listen for and on which stops — or "no audible change". Numbers from a
     local bench/xrun run go here too; the shared runner can't measure timing. -->

## Verified

- [ ] `cargo fmt --all -- --check`
- [ ] `cargo clippy --workspace --exclude aristide-console --all-targets -- -D warnings`
- [ ] `cargo test --workspace --exclude aristide-console`
- [ ] RT invariants intact — no allocation, locking, or I/O added on the audio thread
- [ ] Heard on the test rig, or gated behind a flag defaulting to today's behaviour
