# Aristide

An open-source virtual pipe organ, built to render sample sets better than anything
that exists — and to be the first VPO designed for contemporary music: microtonality,
per-pipe addressing, delays and live processing, arbitrary MIDI/audio routing.

Named for Aristide Cavaillé-Coll. GPLv3, free forever.

**Status: pre-alpha, under heavy construction.** See [DESIGN.md](DESIGN.md) for the
architecture and roadmap.

## Sample sets

Aristide loads GrandOrgue `.organ` sets and unencrypted Hauptwerk (v1/v2-era) sets
directly, with Aristide-specific settings stored in sidecar files that never touch
the original set. Encrypted Hauptwerk sample sets are not supported and never will
be — we do not and will not circumvent their protection.

## Building

```sh
cargo build --release
```
