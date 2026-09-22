# Build and Tuning: first structural studies

These are clickable prototypes for Alex to compare by feel, as required by the
[UI and flow spec](ui-flow-spec.md). **They do not edit the instrument, save data,
listen to MIDI or generate sound.** The connected Play surface is separate.

Run `bun run dev` from `desktop/`, then open the links below. The studies also
ship inside Tauri: unlock Edit, open Build, and choose **Compare layouts**. Switching variants retains the study data, so the same experiment
can be compared across layouts. Reloading resets it.

| Build direction | Try it |
|---|---|
| 1. Voice rows; compact density becomes a table with multi-select | [Open rows](http://localhost:1420/?study=1&panel=build&layout=rows) |
| 2. Step grid; each voice chooses a delay column, with fields below | [Open step grid](http://localhost:1420/?study=1&panel=build&layout=steps) |
| 3. Piano roll; pitch vertically, delay horizontally, click to move a voice | [Open piano roll](http://localhost:1420/?study=1&panel=build&layout=roll) |
| 4. Per-key view; hold a key, select a voice, edit its fields alongside | [Open per-key view](http://localhost:1420/?study=1&panel=build&layout=keys) |

Try adding a voice, moving its pitch/delay, choosing a source, assigning an output,
and holding a key while changing a number with a second finger. Release the key:
the whole-stop values return and the pipe gains an override mark. Undo reverses
study edits. Numbers drag vertically, tap for a stepper and typing, and long-press
or right-click for the assignment sheet. Assignment is explicitly a UI preview.

| Tuning direction | Try it |
|---|---|
| 1. Scope list and one tuning card | [Open scope/card](http://localhost:1420/?study=1&panel=tuning&layout=tree) |
| 2. Scope table with reference/fine offsets visible across the instrument | [Open table](http://localhost:1420/?study=1&panel=tuning&layout=table) |
| 3. Inheritance columns: instrument, divisions, stops, pipes | [Open columns](http://localhost:1420/?study=1&panel=tuning&layout=cascade) |
| 4. Keyboard and deviation bars first, scope controls below | [Open keyboard focus](http://localhost:1420/?study=1&panel=tuning&layout=keyboard) |

Try selecting Grand-orgue, turning off Follow, changing 440 to 415, and selecting
Bourdon to see its inherited pitch. Select Custom to drag deviation bars. More
tuning controls exposes the root, fine offset and arbitrary equal divisions
(up to 128 in this study). These are interaction examples, not a historical
temperament catalogue or a complete tuning implementation.

Alex chooses one direction for each editor; the next design step is four mutations
of each chosen direction. No winner has been selected by the implementation.

Validation: frontend production build, mouse/touch browser tests, two-finger
per-pipe editing, inheritance and screenshot inspection. A scrolling/focus bug
in numeric popovers was fixed before review: focus is now handled by the stock
popover focus trap after placement, avoiding hidden fields when the page scrolls.
