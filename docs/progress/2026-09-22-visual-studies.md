# Visual study navigation

Alex requested less explanatory text and more communication through the interface.
Applies the UI spec's Build, Tuning, Control flow and Visual rules.

- Eight miniature layout diagrams replace the text-only layout selector.
- One compact header identifies prototype status, no audio and no persistence.
- Removed task walkthroughs, repeated inheritance explanations, source-picker
  disclaimers and implementation commentary from the editor entry screen.
- Per-pipe editing shows All keys or the held note beside a short gesture hint.
  Tuning communicates inheritance through its existing Follow switch, disabled
  values and override marks.
- Assignment previews show parameter labels instead of internal addresses.
- Recorded this presentation rule in CLAUDE.md for future UI work.

Validation: production build; 21 Playwright checks with installed Chromium on
isolated port 1431, serial execution; screenshots of all eight layouts and a
390 px viewport. Checked mouse number gestures, visual layout switching, source
sheets, tuning inheritance and two-finger pipe editing. Navigation preserves
held-key on/off requests in the mocked engine checks. Audible verification is
unavailable on this machine. No audio/model code changed.

The studies remain disposable prototypes without audio, MIDI input or saving.
Neither editor direction has been selected. Their missing engine integration,
full tuning model and shared application shell remain as recorded in the
original study and foundation notes.
