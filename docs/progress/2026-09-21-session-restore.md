# Restore the last instrument — 21 September 2026

The Tauri app now queues the last successfully loaded instrument at startup and
remains in locked Play. There is no Library pick in the everyday launch path.
The user config stores the exact successful paths separately from the recent
library, so an unnamed multi-source session restores all its sources. A saved
copy becomes the remembered instrument immediately, without an audio reload.

Failed loads do not replace the remembered session. A missing last instrument
is reported through the existing load-error UI while the runtime stays available;
startup does not silently substitute a different organ. Existing configs without
the new field migrate from the most recent library entry. The standalone CLI
retains explicit loading, so command-line and diagnostic runs do not unexpectedly
load a large sample set.

Validation: startup selection/migration and config round-trip tests, existing
configuration tests, and the copy/adoption integration test. This does not yet
implement first-run discovery or the bundled organ.

Native release verification: started Tauri with an isolated test config whose
last instrument was the adopted GrandOrgue demo. With no interaction, the runtime
loaded that organ and WebKit displayed its 20 stops on Play in perform mode.
Inspected `/tmp/aristide-native-restored.png`; the native log reported
`organ ready: GrandOrgue demo V1`. The release build used
`CARGO_NET_OFFLINE=true bun run desktop:build`.
