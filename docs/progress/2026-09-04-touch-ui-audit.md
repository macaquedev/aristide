# Touch interface and workflow audit — 2026-09-04

The console now has a responsive layout for phones and tablets, larger touch
controls, clearer editing entry points, and less prominent advanced settings.
The desktop canvas keeps its existing layout files and placement behavior.

## Using the interface

- Tap **Edit console**, then **+ Add** to add keyboards, couplers or sample sets.
- Choose **Control settings**, then tap a stop, keyboard, coupler or piston.
  Normal taps continue to play notes and switch stops when that tool is off.
- With a stop editor open, choose **Control settings** again and tap a key on
  that stop's keyboard to edit its individual pipe.
- Keyboard settings now include **Rename keyboard**. **Connect keyboard** opens
  input assignment directly, including the computer keyboard.
- **Silence** is the existing panic action. **Organ → Buttons & shortcuts** is
  the existing bindings editor. Musical behavior and saved file formats are unchanged.
- Preferences shows appearance first; **Memory & loading** expands the advanced
  options. Browser-only console zoom controls are replaced by a browser zoom hint.

## Fixed findings

| Finding | Correction |
| --- | --- |
| Releasing one finger released all notes pressed on screen. | Each pointer owns its note; releasing one preserves the rest of the chord. |
| Duplicate hex keys could release the same note while another finger held it. | Shared note ownership releases only after the last finger lifts. |
| Cancelled touches or a lost window could leave notes held. | Pointer cancellation, capture loss, blur, hidden documents and console rebuilds release local notes. |
| A second finger could move or finish another finger's fader or drag. | Gestures follow their initiating pointer. |
| Cancelled drags left floating controls or resize state behind. | Cancellation cleans up; cancelled panel moves and resizes restore the original appearance without saving. |
| Right-clicking a swell shoe also changed its value. | Only the primary button starts an expression gesture. |
| Clicking an unchanged volume slider could leave it excluded from server refreshes. | Release, cancellation and blur clear slider ownership even without a `change` event. |
| Touching a different open menu opened it on pointer-enter and immediately closed it on click. | Hover menu switching is limited to mouse pointers. |
| Dialog focus could escape into the instrument behind it. | Dialogs contain Tab navigation, restore focus and make background controls inactive; stacked dialogs retain their own order. |
| A slow folder request could overwrite a later folder selection. | The picker ignores responses from superseded browse requests. |
| Taller submenus/forms could extend below the viewport or under editing tools. | Popovers are bounded and reposition when their size or the viewport changes. |
| A crashed popover audit could report success. | Its exception path now records a failed check. |

Menus also support arrows, Home/End and Escape with accessible expanded/checked
states. Expression and crescendo controls support arrow keys and Home/End, and
report their values to assistive technology. Menu navigation no longer also
triggers a bound musical key. Reduced-motion preferences are respected.

## Validation

- Rust model, formats, engine and server suite: **403 passed**, 14 ignored
  benchmark/rendering tests; no failures.
- Native console: `cargo check -p aristide-console` passed.
- JavaScript suite: **30 passed**, including chord ownership, duplicate hex
  notes, cancellation, blur and gesture ownership.
- Ten browser audits against real servers with isolated organ files:
  touch UI, preferences, combinations, protected-organ copying, stop labels,
  polling stability, popover dismissal, click versus drag, loader reliability, and microtonal hex
  layout/keyboard mapping/colours.
- The touch audit uses Chromium touch events with multiple contacts, not just
  programmatic button clicks. It checks layouts at 320, 390, 768 and 1024 pixels,
  plus return to the desktop canvas at 1500 pixels.
- Desktop, phone, tablet and preferences screenshots were visually reviewed.
- The hex audit now uses an isolated configuration instead of the player's library.

The source review followed the design constraints, console modules and their
server command paths. Engine and format behavior was exercised by the existing
Rust suite; this is not an exhaustive proof of every DSP or importer branch.
Physical touchscreen behavior in the Tauri/WebKit host, real MIDI devices and
subjective audio quality still require a hardware session. Long keyboards scroll
horizontally on narrow displays so keys remain usable; desktop panel placement
and resizing remain available in the wide canvas layout.


## Follow-up: loading, editing and delayed requests

The loader now has searchable Recent entries, separate native Open and Remove
buttons, a clearer primary file-opening action, full-size touch targets, and
visible folder progress. Errors appear inside the loader and return keyboard
focus to the initiating control. Native file chooser failures are also visible.

| Finding | Correction |
| --- | --- |
| An old state poll could close the loader before its new load request was acknowledged. | Keep the loader open during acknowledgement and pause new polls while commands are pending. |
| Failed loads appeared in an error strip behind the loader. | Requests return their result to the initiating dialog, which displays the failure inline. |
| Enter on Recent's Remove button could also activate its containing load row. | Open and Remove are sibling native buttons. Removing an entry leaves the current organ and its file alone. |
| Repeated Enter could submit new-organ or save requests more than once. | Pending requests have explicit guards and disabled controls. |
| A save-copy poll could close its dialog before the refused edit was replayed. | Keep the save session active until acknowledgement; replay once on success. Closing the dialog invalidates its replay. |
| An idle poll could discard a quick MIDI assignment before learning started. | Wait for learn acknowledgement before interpreting the learned trigger or cancellation. |
| Slow source responses could overwrite a newer source list or a different division's menu. | Check request identity and the current organ before applying responses. |
| A failed Add stop left its button disabled. | Re-enable the action on refusal. |
| Adding a keyboard on a phone could save phone-derived desktop panel coordinates. | Use the same responsive-layout decision throughout the renderer and editor; skip placement persistence in flow layout. |
| Source browsing offered composite files that the Add sample set operation cannot accept. | Match the supported sample-set formats used by the native chooser. |
| A room slider touched without changing could stop reflecting remote updates. | Clear slider ownership on release, cancellation, capture loss and blur. |

Follow-up verification: **30 JavaScript tests passed**. Five browser audits were
run after the workflow changes: loader reliability (23 assertions), touch UI
(30), protected-organ copying (50), preferences/input assignment (41), and
polling stability (17). All passed. The new loader audit exercises delayed and
failed requests against an isolated real server, actual Enter activation,
duplicate submission, phone layout and instrument creation. Its phone screenshot
was visually reviewed. The unchanged Rust components retain the earlier suite
and native-check results above; no additional hardware testing was performed.
