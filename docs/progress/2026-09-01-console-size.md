# 2026-09-01 — console size in Preferences

Preferences gains a **Size** row: 50–200 % in eleven steps,
with Ctrl + / Ctrl − / Ctrl 0 stepping the same choices. A user
preference like accent and density — localStorage, no organ command,
never in an organ file.

Why a webview zoom and not a CSS one: `style.css` is pixel-sized
throughout, and the panel canvas, every drag and every popover measure
in those CSS pixels (~55 `getBoundingClientRect`/`clientX` sites). CSS
`zoom` puts geometry APIs and `style.left` in different coordinate
spaces, engine-by-engine; a page zoom (`webview.set_zoom`, a new
`set_zoom` Tauri command) scales the device pixels under the page and
leaves every CSS pixel the one the code was written in — exactly what
Ctrl+plus does in a browser. Density still decides what fits; size
decides how big it all is. The two compose.

In a plain browser the row is shown but disabled with a note: the
browser's own zoom already does this and remembers it per site.
`tools/e2e/prefs-split-audit.js` checks the row, its default and its
silence.
