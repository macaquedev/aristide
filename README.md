# Aristide

An open-source virtual pipe organ, built to render sample sets better than anything
that exists — and to be the first VPO designed for contemporary music: microtonality,
per-pipe addressing, delays and live processing, arbitrary MIDI/audio routing.

Named for Aristide Cavaillé-Coll. GPLv3, free forever.

**Status: pre-alpha, under heavy construction.** See [DESIGN.md](DESIGN.md) for the
architecture and roadmap.

## Sample sets

Aristide loads GrandOrgue `.organ` sets and unencrypted Hauptwerk sets (the XML
`.Organ_Hauptwerk_xml` definitions free sets ship in) directly, with Aristide-specific
settings stored in sidecar files that never touch the original set. Encrypted Hauptwerk sample sets are not supported and never will
be — we do not and will not circumvent their protection.

## Organs are files

An Aristide organ is a small TOML file pointing at sample sets elsewhere. It can
wrap one set in three lines, or declare its own manuals and pull stops and whole
divisions from any number of sets — cross-set couplers, renames, compasses, its
own tuning and its own MIDI wiring included. Sources are never modified, and only
the ranks actually used are loaded:

```toml
name = "Frankenorgan"

[sources]
anne = "../sets/st-anne/demo.organ"

[[manual]]
name = "Great"

[[stop]]
from = "anne"
stop = "trompette"
on = "Great"
```

Launching with several sets (`aristide-server a.organ b.organ`) combines them
into one instrument, each keeping its own compass and couplers. The control API
can save the combination as an organ file.

## Building and running

Aristide is headless. The desktop shell and browser UI have been removed.
The workspace contains the audio engine, organ model, sample loaders, and the
server that owns audio/MIDI devices and exposes a localhost JSON control API.

Build dependencies: Rust, `libwavpack`, and ALSA development headers on Linux
(`libasound2-dev` on Debian/Ubuntu, `alsa-lib` on Arch).

```sh
cargo build --release
./target/release/aristide-server path/to/set.organ --list-stops
./target/release/aristide-server path/to/set.organ --stops "montre,prestant"
```

`--list-stops` prints the available stops without opening an audio device.
`--stops` draws stops matching the supplied case-insensitive name fragments;
use names from your set. Existing MIDI assignments and organ-file sound settings
still load. With no path, the server starts with its built-in test tone and
accepts organ loads through the API.

The server requires an audio output device for playback. Useful options:
`--buffer 256` requests a 256-frame buffer, `--gain 0.18` sets the master gain,
`--http-port 9669` chooses the API port, and `--record take.wav` records audio.
Stop with Ctrl+C so the recording's WAV header is finalized.

## Headless control

The JSON API binds to `127.0.0.1:9669` by default. `/` returns 404; no web
page or assets are served. Read `/api/state` for loading status, manual indices,
stop IDs, MIDI assignments, and sound settings. Wait for loading to finish
before playing.

```sh
curl http://127.0.0.1:9669/api/state
curl -X POST 'http://127.0.0.1:9669/api/note?manual=0&key=60&on=1'
curl -X POST 'http://127.0.0.1:9669/api/note?manual=0&key=60&on=0'
curl -X POST http://127.0.0.1:9669/api/panic
```

The note calls address a loaded organ's manual; draw a stop first using `--stops`
or `POST /api/stop?id=<id>&on=1`. Loading, instrument composition, MIDI bindings,
stops, couplers, pistons, tuning, voicing, swell, tremulants, reverb, routing, and
sample-memory settings remain available through the API and TOML files.
The audio comparison scripts in [tools/ab](tools/ab/README.md) also use this API.

Existing organ files remain compatible. Legacy visual metadata is preserved
when editing those files, but no longer drives runtime state or UI endpoints.

## Validation

```sh
cargo test --workspace
cargo clippy --workspace --all-targets
```

Tests render audio without needing an output device. Sample-set integration
coverage uses the local GrandOrgue and Hauptwerk fixtures described in
[CLAUDE.md](CLAUDE.md).
