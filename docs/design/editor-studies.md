# Build and Tuning design studies

These are clickable prototypes for Alex to compare by feel, as required by the
[UI and flow spec](ui-flow-spec.md). **They do not edit the instrument, save data,
listen to MIDI or generate sound.** The connected Play surface is separate.

Run `bun run dev` from `desktop/`, then open the links below. The studies also
ship inside Tauri: unlock Edit, open Build, and choose **Compare layouts**. Switching variants retains the study data, so the same experiment
can be compared across layouts. Reloading resets it.

## Selected Build direction: four piano-roll variations

Alex selected the piano roll on 22 September. The default Build comparison now
opens four variations sharing one editable Titanique fixture:

| Variation | Try it |
|---|---|
| Split desk: both timelines side by side, event inspector alongside | [Open split desk](http://localhost:1420/?study=1&panel=build&layout=split) |
| Stacked: vertically aligned timelines, event inspector alongside | [Open stacked](http://localhost:1420/?study=1&panel=build&layout=stacked) |
| Focus: one large roll, timeline switch and event sheet | [Open focus](http://localhost:1420/?study=1&panel=build&layout=focus) |
| Source lanes: paired pitch rolls for each source, event sheet | [Open source lanes](http://localhost:1420/?study=1&panel=build&layout=lanes) |

Click a roll to create an event; its onset attaches to the nearest timestamp.
Choose Duration in the inspector. Key-down notes default to until-release arrows;
key-up notes always receive a finite ending, adding a timestamp if necessary.
**Try release example** demonstrates the paired arrows plus a new event after
key-up; Undo returns to the previous data. All edits survive layout switches.

Drag a note to change pitch and onset; drag its right edge to choose an ending.
Right-click a note to delete it. Delete removes the hovered note, or the selected
note when none is hovered. Deleting either continuation half removes the whole
event; Undo restores it. Delete keeps its usual text-editing behaviour in fields.
The event fields also support precise keyboard entry. Add timestamps with either
roll's **+ Timestamp** button; click a ruler marker to edit it. Attached onsets and
endings move together. Zoom Time and Pitch separately, scroll either axis, drag
with the middle mouse button, or choose Pan and drag with mouse/touch. Wheel pans,
Shift-wheel pans time, Ctrl/Command-wheel zooms time, and Alt-wheel zooms pitch.
**Centre 0 ¢** restores the overview. Pitch guides follow the study's global tuning
choice; they only constrain gestures when Snap pitch is on. Time grid is visual.
Closely spaced notes keep their exact pitch dots, with offset bars and leader lines
when necessary to keep them selectable.

The source sheet offers ranks and live stop-reference fixtures. Source pitch edits
update every reference's combined pitch readout, and Undo covers source edits too.
Full stop-rule execution, nested graph validation, real tuning inheritance,
autosave, MIDI and playback are **not connected** in these studies.

Navigation/zoom reference: [Image-Line piano-roll manual](https://www.image-line.com/fl-studio-learning/fl-studio-online-manual/html/pianoroll.htm).

## Selected Tuning direction: scope and card

Alex chose scope and card and requested DAW-like control arrangement. The default
Tuning comparison now offers four mutations with one shared editable fixture:

| Arrangement | Try it |
|---|---|
| Channel strip: reference, scale and fine offset above the graph | [Open channel strip](http://localhost:1420/?study=1&panel=tuning&layout=channel) |
| Device rack: adjacent modules, with selected-note controls beside scale | [Open device rack](http://localhost:1420/?study=1&panel=tuning&layout=rack) |
| Editor + inspector: graph and note dock beside a properties column | [Open inspector](http://localhost:1420/?study=1&panel=tuning&layout=inspector) |
| Tabbed device: Pitch, Scale and Note pages inside one card | [Open tabbed device](http://localhost:1420/?study=1&panel=tuning&layout=tabbed) |

The scope browser searches and expands down to the example pipe. Inherited
controls are disabled; turn Follow off to seed an override from the current
values, or use the link to edit the governing scope. Reference presets sit beside
the Hz control. Custom deviation bars drag by mouse or touch, with one Undo per
graph drag. Select a bar to edit its exact cents below; arrow keys navigate and
adjust it. Root changes the graph's starting pitch class. Equal divisions show
numbered intervals, with up to 128 steps. Switching arrangements keeps edits.

Alex broadly preferred Channel strip. Its Scale group now separates **Steps per
repeat** from **Repeat interval** (2:1, 3:1 or custom cents). **Pitch collection**
starts with seven illustrative unequal intervals and **None · no repetition**.
Add/remove steps and edit their intervals directly, including negative intervals
and pitches beyond 1200 cents. The first step is fixed at 0 cents and anchored
by Reference key/Hz. The mapping readout shows consecutive key IDs; a finite
collection leaves keys outside its range unmapped. Repeat size is optional, not
inferred from the last pitch. The graph adapts its range to the collection.

This is an arrangement study, not the complete tuning catalogue. As-recorded,
historical presets, just intonation, Scala import, arbitrary key mapping remain for the connected implementation. The key anchor
and consecutive mapping currently operate only in the silent study. No MIDI bar
lighting or audio audition is simulated.

## Earlier structural studies

Retained at their explicit links for comparison; they are not the default Build.

| Build direction | Try it |
|---|---|
| 1. Voice rows; compact density becomes a table with multi-select | [Open rows](http://localhost:1420/?study=1&panel=build&layout=rows) |
| 2. Step grid; each voice chooses a delay column, with fields below | [Open step grid](http://localhost:1420/?study=1&panel=build&layout=steps) |
| 3. Piano roll; pitch vertically, delay horizontally, click to move a voice | [Open piano roll](http://localhost:1420/?study=1&panel=build&layout=roll&legacy=1) |
| 4. Per-key view; hold a key, select a voice, edit its fields alongside | [Open per-key view](http://localhost:1420/?study=1&panel=build&layout=keys) |

Try adding a voice, moving its pitch/delay, choosing a source, assigning an output,
and holding a key while changing a number with a second finger. Release the key:
the whole-stop values return and the pipe gains an override mark. Undo reverses
study edits. Numbers drag vertically, tap for a stepper and typing, and long-press
or right-click for the assignment sheet. Assignment is explicitly a UI preview.

| Tuning direction | Try it |
|---|---|
| 1. Scope list and one tuning card | [Open scope/card](http://localhost:1420/?study=1&panel=tuning&layout=tree&legacy=1) |
| 2. Scope table with reference/fine offsets visible across the instrument | [Open table](http://localhost:1420/?study=1&panel=tuning&layout=table&legacy=1) |
| 3. Inheritance columns: instrument, divisions, stops, pipes | [Open columns](http://localhost:1420/?study=1&panel=tuning&layout=cascade&legacy=1) |
| 4. Keyboard and deviation bars first, scope controls below | [Open keyboard focus](http://localhost:1420/?study=1&panel=tuning&layout=keyboard&legacy=1) |

Try selecting Grand-orgue, turning off Follow, changing 440 to 415, and selecting
Bourdon to see its inherited pitch. Select Custom to drag deviation bars. More
tuning controls exposes the root, fine offset and arbitrary equal divisions
(up to 128 in this study). These are interaction examples, not a historical
temperament catalogue or a complete tuning implementation.

Alex selected Split desk for Build. Alex broadly preferred Channel strip for Tuning; the other
arrangements remain available for comparison.

Validation: frontend production build, mouse/touch browser tests, two-finger
per-pipe editing, inheritance and screenshot inspection. A scrolling/focus bug
in numeric popovers was fixed before review: focus is now handled by the stock
popover focus trap after placement, avoiding hidden fields when the page scrolls.

## Event limits and overlapping events (2026-10-13)

A silent study of how an event's key and sounding limits read, and of events that
share a time and pitch on the roll. Fixture: a Plein jeu with a two-rank
Fourniture breaking back an octave above B3 (−950 ¢), a Tierce that stops above
+3600 ¢, unison Bourdon and Flûte 8′s, and a pedal Bourdon that runs out at D4.
Pitches are cents from the reference key (A4 = 415 Hz); 19 equal re-derives
every key's pitch, so breaks and the pedal's last pipe move to the keys that now
carry those pitches, and keys get neutral numbers.

| Limits view | Link |
|---|---|
| Pitch chart: keys across, sounding pitch up, handles for both limits | [Open](http://localhost:1420/?study=1&panel=limits&view=chart&overlap=stack) |
| Break chart: keys across, offset from the key up; breaks are steps | [Open](http://localhost:1420/?study=1&panel=limits&view=breaks&overlap=stack) |
| Key strips: one strip per event, filled where it sounds, hatched where no pipe | [Open](http://localhost:1420/?study=1&panel=limits&view=strips&overlap=stack) |
| Sentence only: every event's limits as editable sentences | [Open](http://localhost:1420/?study=1&panel=limits&view=sentence&overlap=stack) |

Overlaps on the roll: Offset (today's leaders), Stack (thinner bars around the
shared pitch), Merge (one bar with a count; tapping cycles through it), Lanes
(one roll per source). Every view shares the inspector sentence ("Plays when the
key is between … and …, and only while it sounds between … and …. Pitches
relative to A4 = 415 Hz.") and a key readout saying what one key plays and why
each other event is silent there.

Validation: `bunx playwright test limits-study`: dragging a key limit, sentence
chips, 19 equal naming, stack and merge selection, no horizontal scroll at
1280 and 390 px; screenshots reviewed.

## Simple stop (2026-10-13)

[Open the Simple stop study](http://localhost:1420/?study=1&panel=simple): the
stop editor for an ordinary organist. A stop is a short list of sounds, each a
sentence of tappable words: what it plays ("Doublette 2′", "Bourdon 8′ from
Pédale", "Couple the Pédale"), its pitch in musical words with the footage it
sounds at ("an octave down · 4′"), and its keys ("all keys", "C4 – F6"), plus a
level slider. A strip under each sound shows where it plays across the keyboard,
hatched where it has no pipe. "Try a key" says what any key plays and why the
rest are silent.

Keys are chosen by dragging on a keyboard, never typed. A mixture break is one
action: tap a key and "Split here", which sends the keys above it an octave
lower. Timing and fine-tuning (cents, "never sounds higher than …") wait behind
⋯; the piano roll is one step further, for echoes and release sounds.

Validation: `bunx playwright test simple-stop`: sentences, Try a key, pitch
menu, split, add and undo, timing, no horizontal scroll at 1280 and 390 px;
screenshots reviewed.
