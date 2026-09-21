# Tauri and Bun foundation — 21 September 2026

First implementation increment under Alex's new UI and flow spec. **This is not
the complete specification.** No deleted UI code or design tokens were restored.
Alex confirmed Tauri for the desktop shell and Bun for frontend tooling.

## Implemented in this increment

- Tauri 2 desktop crate with React, TypeScript and Mantine stock components;
  Bun scripts, exact dependency versions and `bun.lock`.
- Shared Rust audio runtime as a library, hosted on its own thread. Native
  commands use the existing JSON control handlers without exposing a network
  port. The standalone CLI and localhost API still work.
- Play opens in perform mode. Flat outlined/filled stop rectangles, division
  columns, couplers, generals, Set/Cancel/Prev/Next and divisionals connect to
  the current backend. Stop and piston targets are at least 60 px in comfortable
  density. Keyboard note events live above panel navigation.
- Five-panel shell, padlock, memory readout, Panic and Setup. Build is disabled
  in perform mode; long-press and right-click only open it in edit mode. The
  Build/Route/Tuning panels explicitly state that their new editors are pending.
- Library loads existing entries and browses local organ files in a sheet.
- Setup exposes existing console assignment/learn operations and appearance.
  Light/dark and comfortable/compact persist locally. Changes require edit mode.
- Plain-language connection/load errors. CPU and undo are visibly unavailable,
  rather than simulated. No fake ranks, loading percentage or audio telemetry.

## Requirements still open

| Spec area | Remaining work |
|---|---|
| Core model | Programmable voice rows, stable addresses, live duplication without sample copies, borrowed-rank loading |
| Layers | Automatic layer creation on first edit, global deep undo, snapshots, migration of existing sidecars |
| Play | Stop drag/order, custom marks/variants, rename/hide/delete, division context menu, MIDI record/playback |
| Combinations | Stop on/off only; migrate legacy coupler/trem storage, separate named sets, Sequence sheet and labels, piston learn gestures |
| Build | [Four studies ready](../design/editor-studies.md); awaiting Alex's choice, then four mutations and the connected voice editor |
| Route | Named output groups, hierarchical level matrix, inheritance, pipe patterns and live shared routing data |
| Tuning | [Four studies ready](../design/editor-studies.md); awaiting Alex's choice, then four mutations and the connected tuning editor |
| Library | Discovery, rich metadata, snapshots, bundled organ; automatic last-instrument restore is implemented |
| Loading | Accurate memory preflight, lighter/rank-selection choices, rank progress, cancellation, remembered choices |
| Setup | First-run learn flow, device/buffer/rate controls, speaker groups and screen assignment |
| Shell | Real CPU telemetry, multiple screen placement/side-by-side panels, shared numeric gestures and assign-control sheet |
| Errors | Recoverable perform-mode log and live device failure/recovery reporting |
| Review | Audible performance test and full first-run review in a fresh session once implemented |

The backend still stores legacy combinations and exposes some edits that rebuild
an instrument. This increment does not expose those structural edits. They must
be adapted before the new editors can meet the sound-continuity contract.

## Validation

- Frontend production build, Tauri compile check and native release build pass
  (`CARGO_NET_OFFLINE=true bun run desktop:build`).
- Server regression suite: 197 passed, 12 intentionally ignored. Clippy passes
  for the server and desktop targets with existing engine/server warnings.
- Native bridge test proves it shares the real control handlers and rejects
  non-API requests. No audio device is needed for that test.
- Six Playwright browser tests: default locked Play, live stop requests, setter
  semantics in perform mode, protected stop editing, note-on/off order across
  navigation, touch long-press, narrow layout and screenshots.
- Inspected screenshots at 1280 × 800 and 390 × 844. The console uses only the
  active accent plus greys, with red reserved for Panic; outlined off stops,
  filled on stops and large piston targets remain legible.
- Browser tests use a deterministic API fixture. They prove UI/control traffic,
  **not** audible playback or native WebKit behaviour. Native and audible checks
  must be recorded separately.
- Native release smoke check: Tauri/WebKit renders Play, native commands receive
  the real state, and the default audio device starts at 44.1 kHz, stereo,
  512 frames. Inspected `/tmp/aristide-native-ready.png`. This verifies startup,
  not listening quality or a loaded organ performance. No real-time OS priority
  was available on this machine; no system settings were changed.
