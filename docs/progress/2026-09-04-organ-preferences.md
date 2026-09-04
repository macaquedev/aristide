# Compact console and unified preferences

This follows user testing of the initial touch UI. The previous layout left
keyboards at their full size, requiring horizontal scrolling, and the bottom
editing buttons were too cramped. Settings were scattered across menus and
context menus.

## Behavior

- **Organ → Preferences** is the central editing window. Categories cover
  organ sound, keyboards and inputs, stops and individual pipes, couplers,
  sample sets, and appearance/loading. The existing live editors are hosted
  inside the window, retaining their validation and persistence behavior.
- **Organ → Tuning** is removed. The tuning shortcut in the top bar remains.
- **Settings** in the editing toolbar opens the same preferences window.
  Toolbar buttons are at least 56px tall with more separation; Add menu
  choices have the same minimum height. Selecting a control on the console
  is an optional action inside Preferences rather than the main editing path.
- Keyboards initially show compact cards with their names, input settings,
  registration buttons and **Show keys**. Physical MIDI input is unaffected.
  Each keyboard can be expanded independently. Hiding it releases only its
  locally held screen notes, leaving other keyboards' notes alone.
- Expanded keys fit the compact layout's available width. Large keyboards
  are optional because fitting a complete manual onto a phone necessarily
  makes individual keys narrow. Individual pipe editing has an explicit
  selection form, so editing does not require touching those keys.
- Phone stops use the available width more efficiently. Compact keyboard cards
  share rows on tablets. The top bar wraps at tablet widths and fits at 320px.
  Unsaved desktop positions start nearer the top, and the automatic layout
  reserves the swell pedal's height before placing registration buttons.
  Saved panel coordinates retain their existing meaning.
- Changes in pointer type trigger relayout as well as window resize, including
  switching between touch and mouse layouts without a viewport resize.
- Appearance and sample memory are accessible here but retain their existing
  console/user storage scope. Organ settings still write to the organ's file;
  protected structural edits still use the Save a copy workflow.

## Verification

Final results: **31 JavaScript tests and 298 assertions across 11 browser audits passed**. `cargo check -p aristide-console --offline` passed.

The dedicated browser audit covers actual touch activation of Add and Settings,
320/390/768/1024/1500px layouts, compact keyboard cards, every expanded demo
keyboard fitting on a phone, settings editors, live tuning changes, instrument
creation, focus containment/restoration, and nested Save a copy dismissal.
Screenshots of phone, tablet, desktop, expanded phone keys and preferences were
visually reviewed during development. This caught clipped tablet controls,
unused keyboard padding and a desktop pedal/registration overlap.

JavaScript unit tests include a regression for hiding one keyboard while another
still holds notes. The existing touch, preferences/MIDI, protected-organ copy,
microtonal, polling, combination, dismissal, click/drag, labels and loader audits
exercise the preserved workflows. Geometry-dependent tests explicitly choose
Show keys before interacting with the optional key fields.

Native host compilation is checked separately. Browser touch emulation does not
replace physical touchscreen, Tauri/WebKit or MIDI hardware testing.
