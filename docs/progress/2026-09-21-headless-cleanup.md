# Remove the UI; retain the headless instrument

The user requested removal of all current UI while keeping the audio engine and
anything that produces sound. Aristide now ships four crates: model, formats,
engine and server. Run `aristide-server` directly; there is no desktop window or
browser page.

Removed the complete Tauri console, HTML/CSS/JavaScript, fonts, icons, generated
schemas, screenshot assets, visual prototypes, browser audit scripts and GUI
crash helper. The server no longer embeds or serves frontend assets. Its JSON API
remains available for scripting and musical control. UI-only routes for panel
placement, stop ordering/engraving and coupled-key display are gone, together
with their runtime state, snapshot fields and persistence writers. Lumatone
colour presentation and coupled-key animation helpers were also removed.

The audio engine, model and format loaders are unchanged. MIDI mappings, including
Lumatone note mapping and computer-key input through the API, remain operational.
Sample loading, organ composition, stops, couplers, combination action, voicing,
tuning, tremulants, wind, enclosures, reverb, routing, streaming, recording and
sound diagnostics remain. The server's `console.rs` is the musical action and
must remain: it decides which physical pipes speak.

Existing TOML files stay compatible. Legacy visual metadata can still be parsed,
and musical edits preserve and update existing references without exposing those
settings through the API. Tests use legacy metadata fixtures to cover this.
The localhost API resolves routes before applying adopted-organ edit protection,
so deleted routes return 404 even when a protected instrument is loaded.

Cargo.lock drops from 488 packages to 149, with no new package versions. Tauri,
GTK, WebKit, windowing and desktop-dialog dependencies are no longer required.
README and shared instructions now describe headless operation. Earlier UI
progress notes are historical, and the design rules remain a reference for a
future UI; no replacement screen or visual-rule exception is introduced.

## Validation

- `cargo test --workspace --offline`: 414 passed, 14 existing diagnostics/benchmarks
  ignored. GrandOrgue and Hauptwerk fixtures were present; sampled playback,
  recording pitch, streaming equivalence, coupling, voicing, MIDI and crackle/stress
  checks all passed. The 34 API tests include removed-route/asset checks and
  existing-file compatibility.
- `cargo clippy --workspace --all-targets --offline`: passed with existing warnings
  in engine/audio/control code. A strict `-D warnings` attempt reports those same
  pre-existing warnings; no unrelated DSP or control refactor was made.
- `cargo build --release --workspace --offline`: passed without GUI dependencies.
- Release CLI smoke test: `aristide-server testsets/grandorgue-demo/demo.organ
  --list-stops` loaded the fixture and listed its 47 stops without an audio device.
- `git diff --check`: passed. Lockfile audit found no added package versions and
  no remaining Tauri/GTK/WebKit/windowing dependencies.

Validation rendered audio in tests; live playback through an audio device was
not exercised. There is no replacement UI.
