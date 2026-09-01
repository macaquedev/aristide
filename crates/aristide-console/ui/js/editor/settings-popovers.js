// The MIDI-input, compass, Room & noises, Bindings, save, and save-as
// popovers — small, mostly-independent forms each posting straight to
// the server (never through send()/poll for their own errors), grouped
// here because closeSettingsPopovers (still on Editor) always closes
// the first five together, and the sixth (save-as) is the modal every
// refused edit on any of them can end up opening.

import { commands, localFetch } from "../api.js";
import { renderIfChanged, resetRender } from "../dom.js";
import { keyName, parseKeyName } from "../pitch.js";
import { buildManualInputs, buildControlsList, emptyNote, keyboardNote, pistonRow, PITCH_ACTIONS } from "../wiring.js";

// ---- the MIDI-input popover: what plays this manual -----------------------
//
// "What drives the Récit?" is asked at the Récit: the keyboard menu's
// "MIDI input…" (or the silent badge an unwired keyboard wears) opens
// this manual's own input rows — device, channel, shift, bend, Listen
// — plus quick piston rows for the pitch actions that shift it. The
// wiring is an organ fact and lands in the organ's file; the rows are
// the shared builders in wiring.js.

export function wireMidiForm(editor) {
  editor.el.midiClose.addEventListener("click", () => editor.closeMidiForm());
  editor.el.midiRescan.addEventListener("click", () => editor.send(commands.midiRescan()));
}

export function openMidiForm(editor, idx, x, y) {
  editor.openingPopover("midi");
  editor.midiManual = idx;
  resetRender(editor.el.midi);
  editor.syncMidiForm();
  editor.el.midi.classList.remove("hidden");
  editor.positionPopover(editor.el.midi, x, y);
}

export function closeMidiForm(editor) {
  if (editor.midiManual == null) return;
  // Leaving the popover ends any wait for a key: the next thing the
  // player touches should sound, not be swallowed as an assignment.
  if (editor.lastSnapshot?.midi?.learning) editor.send(commands.midiLearn(null));
  editor.midiManual = null;
  resetRender(editor.el.midi);
  editor.el.midi.classList.add("hidden");
}

/// Rebuilt only when something the rows depend on changes — the same
/// signature discipline the old dialog kept, so a poll never tears a
/// select out from under the pointer.
export function syncMidiForm(editor) {
  const idx = editor.midiManual;
  const midi = editor.lastSnapshot?.midi ?? { ports: [], manuals: [] };
  const entry = midi.manuals.find((m) => m.idx === idx);
  if (!entry) {
    editor.closeMidiForm();
    return;
  }
  const keyboardSpan = editor.lastSnapshot?.keyboard
    ? [editor.lastSnapshot.keyboard.low, editor.lastSnapshot.keyboard.high]
    : null;
  const pitchBindings = (editor.lastSnapshot?.controls ?? []).filter(
    (c) => PITCH_ACTIONS.includes(c.action) && c.manual === entry.name
  );
  const signature = JSON.stringify([
    midi.ports, entry, midi.learning ?? null, keyboardSpan, pitchBindings, editor.quickBind,
  ]);
  renderIfChanged(editor.el.midi, signature, () => {
    editor.el.midiTitle.textContent = `${entry.name} · MIDI input`;
    editor.el.midiInputs.replaceChildren();
    buildManualInputs(editor.el.midiInputs, {
      midi,
      manualEntry: entry,
      keyboardSpan,
      send: editor.send,
    });

    // The pitch actions that shift *this* keyboard, as quick piston
    // rows. Bindings that shift "the same keyboard" (no manual of
    // their own) live in the Bindings popover, where the whole list is.
    editor.el.midiPistons.replaceChildren();
    const heading = document.createElement("span");
    heading.className = "menu-heading";
    heading.textContent = "Pistons";
    editor.el.midiPistons.append(heading);
    for (const [action, label] of [
      ["octave-up", "Octave up"],
      ["octave-down", "Octave down"],
      ["transpose-up", "Transpose up"],
      ["transpose-down", "Transpose down"],
    ]) {
      const ctx = {
        snapshot: editor.lastSnapshot,
        send: editor.send,
        manual: entry.name,
        listening: editor.quickBind?.action === action && editor.quickBind?.manual === entry.name,
      };
      const row = document.createElement("div");
      row.className = "settings-row";
      const name = document.createElement("span");
      name.className = "rail-label";
      name.textContent = label;
      row.append(
        name,
        pistonRow(ctx, action, (act, cancelling) =>
          editor.quickBindListen(act, entry.name, cancelling)
        )
      );
      editor.el.midiPistons.append(row);
    }

    editor.el.midiPorts.replaceChildren();
    if (!midi.ports.length) {
      editor.el.midiPorts.append(
        emptyNote("No MIDI inputs. Plug the console in — the list finds it by itself.")
      );
    }
    for (const port of midi.ports) {
      const row = document.createElement("div");
      row.className = "midi-port";
      row.textContent = port.name;
      row.title = port.name;
      editor.el.midiPorts.append(row);
    }
  });
}

// ---- the compass popover: how far this manual reaches ----------------------

export function wireCompassForm(editor) {
  editor.el.compassClose.addEventListener("click", () => editor.closeCompassForm());
}

export function openCompassForm(editor, idx, x, y) {
  editor.openingPopover("compass");
  editor.compassManual = idx;
  resetRender(editor.el.compass);
  hideCompassError(editor);
  editor.syncCompassForm();
  editor.el.compass.classList.remove("hidden");
  editor.positionPopover(editor.el.compass, x, y);
}

export function closeCompassForm(editor) {
  editor.compassManual = null;
  resetRender(editor.el.compass);
  editor.el.compass.classList.add("hidden");
  hideCompassError(editor);
}

export function syncCompassForm(editor) {
  const idx = editor.compassManual;
  const manual = editor.lastSnapshot?.manuals.find((m) => m.idx === idx);
  const compass = (editor.lastSnapshot?.setup?.compass ?? []).find((c) => c.idx === idx);
  if (!manual || !compass) {
    editor.closeCompassForm();
    return;
  }
  const signature = JSON.stringify([manual.name, compass]);
  renderIfChanged(editor.el.compass, signature, () => {
    editor.el.compassTitle.textContent = `${manual.name} · compass`;
    editor.el.compassRow.replaceChildren(compassRow(editor, manual, compass));
  });
}

/// One manual's compass: two editable bounds and the two ways to
/// change them — type new values and press Set, or fall back to
/// whatever the sample set itself declares.
function compassRow(editor, manual, compass) {
  const row = document.createElement("div");
  row.className = "organ-compass-row";

  const low = compassField(compass.low ?? compass.native_low, compass.native_low);
  const high = compassField(compass.high ?? compass.native_high, compass.native_high);
  row.append(low.wrap, high.wrap);

  const set = document.createElement("button");
  set.className = "ghost";
  set.textContent = "Set";
  set.title = "Declare this manual's compass";
  set.addEventListener("click", () => {
    const lo = parseKeyName(low.input.value);
    const hi = parseKeyName(high.input.value);
    // A bound that doesn't name a note stays marked by its own field;
    // nothing is sent until both read as pitches.
    if (lo == null || hi == null) return;
    low.input.value = keyName(lo);
    high.input.value = keyName(hi);
    compassCommand(editor, commands.organCompass(manual.idx, lo, hi));
  });
  row.append(set);

  if (compass.declared) {
    const native = document.createElement("button");
    native.className = "ghost";
    native.textContent = "Native";
    native.title = "Go back to the sample set's own compass";
    native.addEventListener("click", () =>
      compassCommand(editor, commands.organCompass(manual.idx))
    );
    row.append(native);
  }

  return row;
}

/// A bound of the compass as a note name — "C2", "F♯4" — never as a
/// MIDI number. The echo confirms what a nonstandard spelling ("bb2")
/// reads as and flags text that names no note at all. Purely local
/// until Set is pressed: typing here never sends anything.
function compassField(value, native) {
  const wrap = document.createElement("span");
  wrap.className = "compass-field";

  const input = document.createElement("input");
  input.type = "text";
  input.autocomplete = "off";
  input.spellcheck = false;
  input.value = keyName(value);
  input.placeholder = keyName(native);
  input.title = `Sample set's own: ${keyName(native)} · C4 is middle C`;

  const note = document.createElement("i");
  input.addEventListener("input", () => {
    const parsed = parseKeyName(input.value);
    input.classList.toggle("invalid", parsed == null);
    const canonical = parsed == null ? null : keyName(parsed);
    note.textContent = parsed == null ? "?" : canonical === input.value.trim() ? "" : canonical;
  });

  wrap.append(input, note);
  return { wrap, input };
}

async function compassCommand(editor, query) {
  hideCompassError(editor);
  const { ok, error } = await editor.organCommandResult(query);
  if (error != null) showCompassError(editor, error);
  return ok;
}

function showCompassError(editor, text) {
  editor.el.compassError.textContent = text;
  editor.el.compassError.classList.remove("hidden");
}

function hideCompassError(editor) {
  editor.el.compassError.classList.add("hidden");
  editor.el.compassError.textContent = "";
}

// ---- the Room & noises popover: organ-wide sound character -----------------
//
// Reverb wet and the mechanism noises are the organ's, not the
// player's: both live in the organ's file and travel with it. The
// sliders report live while they move (~30 commands/s) and persist
// only on release, so a drag never writes the file per frame.

export function wireRoomForm(editor) {
  editor.el.roomClose.addEventListener("click", () => editor.closeRoomForm());

  throttledRoomSlider(editor, editor.el.roomReverb, "reverb", (persist) =>
    editor.send(commands.reverb(editor.el.roomReverb.value, persist))
  );
  const sendNoises = (persist) =>
    editor.send(
      commands.noises(editor.el.roomNoisesOn.checked, editor.el.roomNoisesVol.value, persist)
    );
  editor.el.roomNoisesOn.addEventListener("change", () => sendNoises(true));
  throttledRoomSlider(editor, editor.el.roomNoisesVol, "noises-vol", sendNoises);
}

/// A slider that reports while it moves: ~30 commands/s during the
/// drag, one final, persisted value on release.
function throttledRoomSlider(editor, slider, key, send) {
  let lastSent = 0;
  slider.addEventListener("pointerdown", () => editor.roomDragging.add(key));
  slider.addEventListener("input", () => {
    const now = performance.now();
    if (now - lastSent > 33) {
      lastSent = now;
      send(false);
    }
  });
  slider.addEventListener("change", () => {
    editor.roomDragging.delete(key);
    send(true);
  });
}

export function openRoomForm(editor, x, y) {
  editor.openingPopover("room");
  editor.roomOpen = true;
  editor.syncRoomForm();
  editor.el.room.classList.remove("hidden");
  editor.positionPopover(editor.el.room, x, y);
}

export function closeRoomForm(editor) {
  editor.roomOpen = false;
  editor.el.room.classList.add("hidden");
}

export function syncRoomForm(editor) {
  const snapshot = editor.lastSnapshot ?? {};
  editor.el.roomReverbRow.classList.toggle("hidden", snapshot.reverb == null);
  if (snapshot.reverb != null && !editor.roomDragging.has("reverb")) {
    editor.el.roomReverb.value = snapshot.reverb;
  }
  editor.el.roomNoisesRow.classList.toggle("hidden", !snapshot.noises);
  if (snapshot.noises) {
    editor.el.roomNoisesOn.checked = snapshot.noises.on;
    if (!editor.roomDragging.has("noises-vol")) {
      editor.el.roomNoisesVol.value = snapshot.noises.vol;
    }
  }
}

// ---- the Bindings popover: the whole flat list ------------------------------
//
// Every piston, pedal and key this organ answers to, in one place —
// the piston rows on stop and coupler editors are filtered views
// over this same list. Action-first, not manual-first: a binding
// doesn't belong to a manual, so a flat list is the honest shape.

export function wireBindingsForm(editor) {
  editor.el.bindingsClose.addEventListener("click", () => editor.closeBindingsForm());
  // A new slot doesn't exist on the server until either a bind or a
  // learned trigger names it; learning one past the end is enough —
  // learn_control defaults a slot with nothing saved to "octave-up".
  editor.el.bindingsAdd.addEventListener("click", () =>
    editor.send(commands.controlLearn((editor.lastSnapshot?.controls ?? []).length))
  );
}

export function openBindingsForm(editor, x, y) {
  editor.openingPopover("bindings");
  editor.bindingsOpen = true;
  resetRender(editor.el.bindings);
  editor.syncBindingsForm();
  editor.el.bindings.classList.remove("hidden");
  editor.positionPopover(editor.el.bindings, x, y);
}

export function closeBindingsForm(editor) {
  if (!editor.bindingsOpen) return;
  // Same contract as the MIDI popover: leaving ends any wait for a key.
  if (editor.lastSnapshot?.control_learning != null && !editor.quickBind) {
    editor.send(commands.controlLearn(null));
  }
  editor.bindingsOpen = false;
  resetRender(editor.el.bindings);
  editor.el.bindings.classList.add("hidden");
}

export function syncBindingsForm(editor) {
  const snapshot = editor.lastSnapshot;
  if (!snapshot) return;
  const learning = snapshot.control_learning ?? null;
  const signature = JSON.stringify([
    snapshot.controls ?? [],
    snapshot.actions ?? [],
    learning,
    (snapshot.stops ?? []).map((s) => s.name),
    (snapshot.couplers ?? []).map((c) => c.name),
    (snapshot.enclosures ?? []).map((e) => e.name),
    (snapshot.manuals ?? []).map((m) => m.name),
    snapshot.keyboard ?? null,
  ]);
  renderIfChanged(editor.el.bindings, signature, () => {
    editor.el.bindingsList.replaceChildren();
    buildControlsList(editor.el.bindingsList, { snapshot, learning, send: editor.send });
    editor.el.bindingsKeyboard.textContent = keyboardNote(snapshot);
  });
}

// ---- the save-as popover: an ad-hoc combination becomes a file -------------
//
// Opened from the organ-name menu, and once, automatically, for an
// organ combined on the command line. Saving bypasses send()/poll:
// a bad path has a specific, useful reason the server already wrote
// out, and it belongs in this popover.

export function wireSaveForm(editor) {
  editor.el.saveClose.addEventListener("click", () => editor.closeSaveForm());
  editor.el.saveBtn.addEventListener("click", () => saveOrgan(editor));
  editor.el.savePath.addEventListener("keydown", (event) => {
    if (event.key === "Enter") {
      event.preventDefault();
      saveOrgan(editor);
    }
  });
}

export function openSaveForm(editor, x, y) {
  const setup = editor.lastSnapshot?.setup;
  if (!setup || setup.file) return; // nothing unsaved to write
  editor.openingPopover("save");
  editor.saveOpen = true;
  editor.el.savePath.value = "";
  hideSaveError(editor);
  editor.el.save.classList.remove("hidden");
  editor.positionPopover(
    editor.el.save,
    x ?? window.innerWidth / 2 - 180,
    y ?? 96
  );
  requestAnimationFrame(() => editor.el.savePath.focus());
}

export function closeSaveForm(editor) {
  editor.saveOpen = false;
  editor.el.save.classList.add("hidden");
  hideSaveError(editor);
}

/// The popover only makes sense while the organ has no file — once
/// the save lands (this session's or another's), it closes itself.
export function syncSaveForm(editor) {
  const setup = editor.lastSnapshot?.setup;
  if (!setup || setup.file) editor.closeSaveForm();
}

async function saveOrgan(editor) {
  const path = editor.el.savePath.value.trim();
  if (!path) {
    showSaveError(editor, "Give it a path first.");
    return;
  }
  editor.el.saveBtn.disabled = true;
  const { ok, error } = await localFetch(editor.base, commands.organSave(path), { method: "POST" });
  if (!ok) {
    showSaveError(editor, error);
  } else {
    // The next poll picks up the now-saved organ; syncSaveForm sees
    // setup.file and closes the popover.
    hideSaveError(editor);
  }
  editor.el.saveBtn.disabled = false;
}

function showSaveError(editor, text) {
  editor.el.saveError.textContent = text;
  editor.el.saveError.classList.remove("hidden");
}

function hideSaveError(editor) {
  editor.el.saveError.classList.add("hidden");
  editor.el.saveError.textContent = "";
}

// ---- the save-as dialog: a set's own organ becomes the player's -----------
//
// A sample set's own organ (its file marked `adopted`) keeps the
// instrument the set defines: the player's wiring, room, pitch and
// layout land in its file, but the server answers any change to the
// instrument itself with 409, and main.js routes that here with the
// refused command in hand.
// Saving copies the file under the new name, the server switches to
// the copy, and the refused command is sent again — so the player's
// gesture lands after all, on an organ that is theirs. The same
// dialog is the organ-name menu's "Save as…" for any organ with a
// file, with nothing to replay.

export function wireSaveAsForm(editor) {
  for (const closer of editor.el.saveAs.querySelectorAll("[data-close]")) {
    closer.addEventListener("click", () => editor.closeSaveAsForm());
  }
  editor.el.saveAsCancel.addEventListener("click", () => editor.closeSaveAsForm());
  editor.el.saveAsBtn.addEventListener("click", () => saveOrganAs(editor));
  editor.el.saveAsName.addEventListener("keydown", (event) => {
    if (event.key === "Enter") {
      event.preventDefault();
      saveOrganAs(editor);
    }
  });
}

/// `pending` is the refused command, if a refusal opened the dialog.
export function openSaveAsForm(editor, pending = null) {
  const snapshot = editor.lastSnapshot;
  if (!snapshot?.setup?.file || !snapshot.organ) return;
  const organ = snapshot.organ;
  const adopted = Boolean(snapshot.setup.adopted);
  // A refused rename IS this dialog: the name goes on the copy, and
  // there is nothing left to send again.
  const rename = pending?.match(/^\/api\/organ\/rename\?name=([^&]*)/);
  editor.saveAsPending = rename ? null : pending;
  editor.saveAsFor = organ;
  editor.saveAsOpen = true;
  editor.closeSaveForm();
  const strong = (text) => Object.assign(document.createElement("strong"), { textContent: text });
  editor.el.saveAsNote.replaceChildren(
    ...(adopted
      ? [
          strong(organ),
          " is the sample set's own organ: your wiring, room and pitch are saved on it, " +
            "but the instrument itself stays as the set defines it. Save it under a " +
            "different name and the copy is yours to change" +
            (pending && !rename ? " — this change and every one after it." : ".") +
            " The set's own organ stays as it was.",
        ]
      : [
          "Save a copy of ",
          strong(organ),
          " under a new name and carry on playing the copy. ",
          strong(organ),
          " stays as it is.",
        ])
  );
  editor.el.saveAsName.value = rename
    ? decodeURIComponent(rename[1].replace(/\+/g, " "))
    : `My ${organ}`;
  hideSaveAsError(editor);
  editor.el.saveAs.classList.remove("hidden");
  editor.root.body.classList.add("modal-open");
  requestAnimationFrame(() => {
    editor.el.saveAsName.focus();
    editor.el.saveAsName.select();
  });
}

export function closeSaveAsForm(editor) {
  if (!editor.saveAsOpen) return;
  editor.saveAsOpen = false;
  editor.saveAsPending = null;
  editor.saveAsFor = null;
  editor.el.saveAs.classList.add("hidden");
  editor.root.body.classList.remove("modal-open");
  hideSaveAsError(editor);
}

/// The dialog is about one organ: if another loads under it, or the
/// one it is about has been saved elsewhere already, it no longer
/// applies.
export function syncSaveAsForm(editor) {
  const snapshot = editor.lastSnapshot;
  if (!snapshot?.setup?.file || snapshot.organ !== editor.saveAsFor) editor.closeSaveAsForm();
}

async function saveOrganAs(editor) {
  const name = editor.el.saveAsName.value.trim();
  if (!name) {
    showSaveAsError(editor, "Give it a name first.");
    return;
  }
  if (name === editor.saveAsFor) {
    showSaveAsError(editor, "Give the copy a name of its own.");
    return;
  }
  const pending = editor.saveAsPending;
  editor.el.saveAsBtn.disabled = true;
  const { ok, error } = await localFetch(editor.base, commands.organSaveAs(name), { method: "POST" });
  if (!ok) {
    showSaveAsError(editor, error);
  } else {
    editor.closeSaveAsForm();
    // The server has switched to the copy; the change it refused a
    // moment ago goes through now. The next poll shows the new name.
    if (pending) editor.send(pending);
  }
  editor.el.saveAsBtn.disabled = false;
}

function showSaveAsError(editor, text) {
  editor.el.saveAsError.textContent = text;
  editor.el.saveAsError.classList.remove("hidden");
}

function hideSaveAsError(editor) {
  editor.el.saveAsError.classList.add("hidden");
  editor.el.saveAsError.textContent = "";
}
