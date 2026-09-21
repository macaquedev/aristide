# Workspace redesign — 2026-09-21

The user requested a complete redesign around `docs/design/design-rules.md`.
Those rules are now required by `CLAUDE.md` and referenced from `AGENTS.md` and
`DESIGN.md`; they supersede the older menu placement and rocker styling.
The user further specified that partially loaded organs must never be playable,
and that recording is a future workspace, **when implemented**.

## Surfaces and actions

| Workspace | Physical situation | Three primary actions |
|---|---|---|
| Play | Console, touchscreen at arm's length | Draw stops; recall registrations; advance the sequencer |
| Library | Desk | Load an organ; browse sample sets; create an organ |
| Build | Desk | Edit divisions and stops; assign MIDI controls; arrange the console |
| Voice | Desk | Tune the instrument; voice stops and pipes; adjust acoustics |
| Setup | Desk | Adjust screen zoom; choose sample memory behavior; reload after memory changes |

One Edit/Play lock replaces the old app/organ/View menus. Play has no settings,
context-menu entry points, native tooltips or edit-through-Ctrl gestures. Desk
workspaces occupy the page rather than overlaying Play. Existing forms are moved
into their workspace, never cloned, retaining their live save paths and scope
contracts. The command palette (Ctrl+K) reaches workspaces, musical editors,
individual stops and keyboards, file actions, fullscreen and the keyboard map.
Ctrl+1–4 switches desk workspaces; Ctrl+E operates the lock.

The entire stylesheet is replaced with the specified dark palette, flat controls,
Inter (bundled locally with its license), consistent fields, spacing and type.
Engaged stops use the lit fill. Crescendo engagement also has a dashed outline.
Active pistons reflect server-side registration matches, including changes made
over MIDI. Play piston taps explicitly recall; they cannot consume an armed
setter and write a registration. The existing setter remains in Build's console
layout editor. The sequencer shows its current and next frame.

Play uses one division group per column, with divisionals beneath its stops.
Density reduces automatically before paging, preserving 96×64 stop targets and
56×56 other playing targets. Generals, divisionals and couplers have independent
paging when needed. Couplers and expression share a lower row on smaller desktop
windows. Stop pages do not scroll. Saved panel geometry is preserved for Build's
layout editor; responsive Play never writes geometry into the organ file.

Both the Tauri shell and the server's root browser URL now use the same frontend.
The server embeds only public runtime assets at build time, including the font;
no separate web server or network font service is required. The old embedded
server-only console is removed. Loading stays in Library until the complete
instrument is ready. No progressive playback is introduced.

## Validation

- Frontend unit tests: 40 passed, covering async editors, pointer ownership,
  requests, MIDI scan completion, tuning inheritance and page coverage.
- Server regression suite: 198 passed, 12 optional diagnostics ignored.
- Targeted tests cover active registration matches, recall-only Play with an
  armed setter, embedded asset MIME types and exclusion of tests/path traversal.
- `cargo clippy -p aristide-server --all-targets` passes with pre-existing warnings
  in audio/console code and large engine/control enum variants. Strict
  `-D warnings` is blocked by those existing warnings; no audio representation
  changes are made for a UI task.
- Browser audit: **61 checks passed**. `tools/e2e/workspaces-audit.js` exercises the real server, live stop toggles,
  MIDI editor access, tuning persistence, command navigation, desk keyboard
  audition, the shared server URL and the complete-load gate. Its explicitly
  synthetic layout fixtures use French, German and English stop names, with a
  third drawn, at 5 stops / 1 division and 150 stops / 5 divisions. Sizes include
  1500×950, 1080×1920 portrait, 900×600, 768×1024, 390×844 and 320×740.
- Audible output remains a desktop check; this machine is headless.

## Design previews

Saved from the browser audit: [small organ](../design/workspaces/play-small.png),
[large organ](../design/workspaces/play-large.png),
[portrait Play](../design/workspaces/play-portrait.png),
[Library](../design/workspaces/library.png), [Build](../design/workspaces/build.png),
[Voice](../design/workspaces/voice.png) and [Setup](../design/workspaces/setup.png).
The small/large organ images use the synthetic stress fixtures described above.

## Self-review and remaining rules

The brief includes capabilities beyond the current engine/editor contracts.
This redesign does not represent those capabilities as implemented:

- **Rule 3.7 / 7:** edits keep the existing autosave paths, but there is no
  application-wide undo history. Existing structural removal and protected
  sample-set copy confirmations remain. Implementing safe file-level undo is
  needed before removing those protections. Structural changes still reload;
  tuning and voicing remain live.
- **Rule 5:** the supplied border colour does not meet the brief's 3:1 control
  boundary contrast. Controls use the existing `text-muted` token for their
  outlines. The explicit 17px stop-name and 14px pitch requirements take
  precedence over the conflicting general type-scale/minimum statements.
  These are proposed clarifications, not silent additions to the palette.
- **Rule 6:** per-window paging is implemented; assigning divisions to multiple
  connected displays is not. Per-stop loading/playback is deliberately absent
  under the user's full-load-only requirement. The existing server reports
  load warnings globally, not separate per-stop loading/error states.
- **Rule 7:** the command palette covers navigation and musical editor entry
  points. Individual field changes use normal keyboard form navigation rather
  than receiving a separate named shortcut. MIDI learn retains the existing
  range-measurement and conflict-resolution steps.
- **Rule 8:** Play cleans numeric prefixes and underscores without changing
  stored identities. Existing user-authored labels, technical editor names and
  coupler abbreviations are retained; a comprehensive imported-name editor and
  abbreviation dictionary are not introduced.
- **Workspace scope:** Setup exposes the existing screen and memory controls.
  A full sound-output/routing editor, multi-window management and recording
  remain future capabilities. Recording is labelled “when implemented.”

The weakest part is the desk editor's inherited action-list navigation: it is
consistent and preserves complex editing behavior, but still takes several
steps to reach a particular pipe or MIDI binding. A future inspector and safe
undo history would improve that workflow without crowding the playing surface.
