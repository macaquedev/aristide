// The wiring rows: MIDI inputs and control bindings, as reusable row
// builders. Both are organ facts — they live in the organ's file — so
// their UI lives on the console surface (the keyboard's MIDI popover,
// the Bindings popover, the piston rows on stop and coupler editors),
// never in the Preferences dialog, which is the player's.
//
// Every builder takes a plain context object rather than an instance:
// `send` posts a command, snapshot slices carry what is true now, and
// the caller owns when to rebuild (signature discipline stays theirs).

import { commands, COMPUTER_KEYBOARD } from "./api.js";
import { keyName, noteTriggerText, shiftWords } from "./pitch.js";

// The pitch actions all take an optional target manual; everything else
// in the catalogue is either global (panic, cancel) or names its own
// target through a second word (stop:, coupler:, enclosure:).
export const PITCH_ACTIONS = [
  "octave-up",
  "octave-down",
  "transpose-up",
  "transpose-down",
  "transpose-reset",
];
export const NAMED_ACTIONS = ["stop:", "coupler:", "enclosure:"];

/// The stems whose target is a *number* — a piston slot, a crescendo
/// stage, a stepper frame — with how far the offered range runs. The
/// server takes any number; these are just what a menu can sensibly
/// list, and the bank sizes match the console's own piston rail.
export const NUMBERED_ACTIONS = {
  "general:": 12,
  "crescendo:": 32,
  "stepper:goto:": 32,
};

/// The one stem whose target is a manual *and* a number: a divisional
/// piston belongs to a division, so it needs both.
export const DIVISIONAL_ACTION = "divisional:";
export const DIVISIONAL_PISTONS = 8;

const ACTION_LABELS = {
  "octave-up": "Octave up",
  "octave-down": "Octave down",
  "transpose-up": "Transpose up",
  "transpose-down": "Transpose down",
  "transpose-reset": "Transpose reset",
  tremulant: "Tremulant",
  cancel: "General cancel",
  panic: "Panic",
  set: "Set (arm the setter)",
  "stop:": "Stop…",
  "coupler:": "Coupler…",
  "enclosure:": "Enclosure…",
  "general:": "General piston…",
  "divisional:": "Divisional piston…",
  "stepper:next": "Stepper: next frame",
  "stepper:prev": "Stepper: previous frame",
  "stepper:goto:": "Stepper: go to frame…",
  "stepper:store": "Stepper: store this frame",
  "stepper:insert": "Stepper: insert a frame",
  crescendo: "Crescendo pedal (a shoe)",
  "crescendo:": "Crescendo stage…",
};
export function actionLabel(action) {
  return ACTION_LABELS[action] ?? action;
}

/// Which catalogue entry an action string belongs to. An action is
/// either an entry outright ("cancel", "stepper:next") or a *stem* plus
/// a target ("stop:Montre 8'", "crescendo:12", "divisional:Récit:3"), so
/// the longest stem it starts with wins — "stepper:goto:5" is a goto,
/// not a mangled "stepper:". Null when the server knows an action this
/// console's catalogue doesn't.
function actionEntry(action, catalogue) {
  if (catalogue.includes(action)) return action;
  let best = null;
  for (const entry of catalogue) {
    if (entry.endsWith(":") && action.startsWith(entry)) {
      if (!best || entry.length > best.length) best = entry;
    }
  }
  return best;
}

/// What follows the stem: the stop name, the piston number, the
/// "<manual>:<n>" pair.
function actionArg(action, entry) {
  return entry && entry.endsWith(":") && action.startsWith(entry)
    ? action.slice(entry.length)
    : "";
}

function namesFor(verb, snapshot) {
  if (verb === "stop:") return (snapshot.stops ?? []).map((s) => s.name);
  if (verb === "coupler:") return (snapshot.couplers ?? []).map((c) => c.name);
  if (verb === "enclosure:") return (snapshot.enclosures ?? []).map((e) => e.name);
  if (verb === DIVISIONAL_ACTION) return (snapshot.manuals ?? []).map((m) => m.name);
  return [];
}

/// A stem's default target, so choosing "General piston…" from the
/// menu sends something the server can parse rather than a bare stem.
function defaultTarget(entry, snapshot) {
  if (entry === DIVISIONAL_ACTION) {
    const manual = (snapshot.manuals ?? [])[0]?.name;
    return manual ? `${manual}:1` : null;
  }
  if (entry in NUMBERED_ACTIONS) return "1";
  if (NAMED_ACTIONS.includes(entry)) return namesFor(entry, snapshot)[0] ?? null;
  return "";
}

/// "Récit:3" → ["Récit", "3"]. The *last* colon separates, as the
/// server's own parser does, so a manual named with one survives.
function splitLast(text) {
  const at = text.lastIndexOf(":");
  return at === -1 ? [text, ""] : [text.slice(0, at), text.slice(at + 1)];
}

/// The numbers 1..n as `<option>`s — the target select for a piston
/// slot, a crescendo stage or a stepper frame.
function numberOptions(select, n, selected) {
  for (let i = 1; i <= n; i++) select.append(option(String(i), String(i)));
  select.value = selected && Number(selected) >= 1 && Number(selected) <= n
    ? String(Number(selected))
    : "1";
}

// A handful of physical keys read better as the character they print
// ("=") than as their event.code name ("Equal"); the rest are left as
// the code itself, which is still legible enough for a letter or digit.
const KEY_GLYPHS = {
  Equal: "=", Minus: "-", Comma: ",", Period: ".", Slash: "/",
  Semicolon: ";", Quote: "'", BracketLeft: "[", BracketRight: "]",
  Backquote: "`", Backslash: "\\", Space: "Space",
};
export function keyGlyph(code) {
  if (code in KEY_GLYPHS) return KEY_GLYPHS[code];
  if (code.startsWith("Key")) return code.slice(3);
  if (code.startsWith("Digit")) return code.slice(5);
  return code;
}

/// The trigger cell reads as prose, not as the wire format: a note
/// spells its pitch, a MIDI message names its device and channel; a
/// computer key just prints the character it is, since the device is
/// always the same one.
export function triggerText(control) {
  if (!control || !control.trigger) return "— press Listen —";
  if (control.trigger.startsWith("key:")) {
    return `key:${keyGlyph(control.trigger.slice(4))}`;
  }
  const channel = control.channel ? ` ch${control.channel}` : "";
  return `${noteTriggerText(control.trigger)} · ${control.device}${channel}`;
}

export function option(value, label) {
  const opt = document.createElement("option");
  opt.value = value;
  opt.textContent = label;
  return opt;
}

export function emptyNote(text) {
  const empty = document.createElement("p");
  empty.className = "pane-empty";
  empty.textContent = text;
  return empty;
}

// ---- MIDI input rows ------------------------------------------------------
//
// Read manual-first, the way an organist asks the question: *what
// plays the Récit?* A manual holds a list of inputs, so two keyboards
// can share a division and one console can feed several manuals by
// splitting itself across channels.
//
// Nothing here is edited in place: the caller rebuilds the rows
// whenever the assignments change, which is only ever just after the
// user acted on one (and so blurred it). Between those they are inert.

/// Everything one manual's rows need to know:
/// `send` posts a command; `midi` is the snapshot's midi object;
/// `manualEntry` its entry for this manual; `keyboardSpan` the computer
/// keyboard's own [low, high], when the snapshot carries one.
export function buildManualInputs(container, ctx) {
  const { midi, manualEntry } = ctx;
  for (const input of manualEntry.inputs) {
    container.append(inputRow(ctx, manualEntry.idx, input.slot, input));
    container.append(compassNote(ctx, manualEntry, input));
  }
  // A manual with nothing on it still shows one row: the empty state
  // has to be assignable, not just described.
  const learning = midi.learning;
  const pending =
    learning &&
    learning.manual === manualEntry.idx &&
    learning.slot >= manualEntry.inputs.length;
  if (!manualEntry.inputs.length || pending) {
    container.append(inputRow(ctx, manualEntry.idx, manualEntry.inputs.length, null));
  }
  if (manualEntry.inputs.length && !pending) {
    const add = document.createElement("button");
    add.className = "ghost add-input";
    add.textContent = "+ add input";
    add.title = "A second keyboard playing this same manual";
    add.addEventListener("click", () =>
      ctx.send(commands.midiLearn(manualEntry.idx, manualEntry.inputs.length))
    );
    container.append(add);
  }
}

/// One assignment: which device, on which channel, plus the two ways
/// to set it — play a key, or say so.
function inputRow(ctx, manual, slot, input) {
  const { midi, send } = ctx;
  const listening =
    midi.learning && midi.learning.manual === manual && midi.learning.slot === slot;
  const row = document.createElement("div");
  row.className = "manual-input";
  row.classList.toggle("listening", !!listening);
  row.classList.toggle("missing", !!input && !input.connected);

  const device = document.createElement("select");
  device.className = "input-device";
  if (!input) {
    device.append(option("", "— no input —"));
  }
  for (const port of midi.ports) {
    device.append(option(port.name, port.name));
  }
  // A binding survives its keyboard being unplugged; the row says so
  // rather than quietly dropping the assignment.
  if (input && !midi.ports.some((port) => port.name === input.device)) {
    device.append(option(input.device, `${input.device} (not connected)`));
  }
  device.value = input ? input.device : "";
  device.addEventListener("change", () => {
    if (!device.value) return;
    // An existing row keeps the channel it had, "any" included; a new
    // one sends none, which lets the server apply what the set
    // suggests for this manual.
    const channel = input ? (input.channel ?? "any") : null;
    send(commands.midiBind(manual, slot, device.value, channel));
  });

  const channel = document.createElement("select");
  channel.className = "input-channel";
  channel.append(option("any", "any channel"));
  for (let ch = 1; ch <= 16; ch++) channel.append(option(String(ch), `channel ${ch}`));
  channel.value = input ? (input.channel == null ? "any" : String(input.channel)) : "any";
  // The computer keyboard has no channels to tell apart.
  channel.disabled = !input || input.device === COMPUTER_KEYBOARD;
  channel.addEventListener("change", () =>
    send(commands.midiBind(manual, slot, input.device, channel.value))
  );

  // The keyboard's shift in semitones: a controller whose keys should
  // sound other pipes than the notes it sends — C2–C7 hardware playing
  // G1–G6 is a shift of −5. The same number the octave buttons move.
  const shift = document.createElement("input");
  shift.type = "number";
  shift.className = "input-transpose";
  shift.min = -36;
  shift.max = 36;
  shift.step = 1;
  shift.value = input ? (input.transpose ?? 0) : 0;
  shift.disabled = !input;
  shift.title = "Shift this keyboard, in semitones: −5 makes its C sound the G below";
  shift.setAttribute("aria-label", "Shift in semitones");
  shift.addEventListener("change", () => {
    const semitones = Math.min(36, Math.max(-36, Math.trunc(Number(shift.value) || 0)));
    shift.value = semitones;
    send(
      commands.midiBind(manual, slot, input.device, input.channel ?? "any", null, null, semitones)
    );
    shift.blur(); // hand the field back to the snapshot
  });

  const listen = document.createElement("button");
  listen.className = "ghost listen";
  listen.textContent = listening ? "Cancel" : "Listen";
  listen.title = "Assign by playing a key on the keyboard you mean";
  listen.addEventListener("click", () =>
    send(listening ? commands.midiLearn(null) : commands.midiLearn(manual, slot))
  );

  row.append(device, channel, shift, listen);
  if (input) {
    const remove = document.createElement("button");
    remove.className = "ghost remove-input";
    remove.textContent = "×";
    remove.setAttribute("aria-label", `Remove ${input.device}`);
    remove.addEventListener("click", () => send(commands.midiUnbind(manual, slot)));
    row.append(remove);
  }
  if (listening) {
    const hint = document.createElement("span");
    hint.className = "listen-hint";
    hint.textContent =
      midi.learning.step === "high" ? "now the highest key…" : "play the lowest key…";
    row.append(hint);
  }
  const fragment = document.createDocumentFragment();
  fragment.append(row, bendRow(ctx, manual, slot, input));
  return fragment;
}

/// The bend row: this manual/slot's MPE pitch-bend range, in semitones.
/// The input row itself is already edge-to-edge, so bend gets its own
/// compact line beneath it, styled like the compass note that already
/// lives there. Off (null) is the organ's default — bend messages are
/// ignored; 48 is the MPE convention; anything else types into the
/// field "Custom…" reveals.
function bendRow(ctx, manual, slot, input) {
  const { send } = ctx;
  const BEND_PRESETS = [2, 12, 48];
  const line = document.createElement("div");
  line.className = "input-bend-row";

  const label = document.createElement("span");
  label.className = "input-bend-label";
  label.textContent = "Bend";

  const bend = document.createElement("select");
  bend.className = "input-bend";
  bend.append(option("off", "off"));
  for (const semitones of BEND_PRESETS) {
    bend.append(option(String(semitones), semitones === 48 ? "48 (MPE)" : String(semitones)));
  }
  bend.append(option("custom", "Custom…"));
  bend.disabled = !input;
  bend.title = "Pitch-bend range this input sends, in semitones (MPE); off ignores bend messages";

  const custom = document.createElement("input");
  custom.type = "number";
  custom.className = "input-bend-custom";
  custom.min = 1;
  custom.max = 96;
  custom.step = 1;
  custom.title = "Custom pitch-bend range, in semitones";
  custom.setAttribute("aria-label", "Custom pitch-bend range in semitones");
  custom.disabled = !input;

  const unit = document.createElement("span");
  unit.className = "input-bend-label";
  unit.textContent = "semitones";

  const currentBend = input ? (input.bend ?? null) : null;
  if (currentBend == null) {
    bend.value = "off";
  } else if (BEND_PRESETS.includes(currentBend)) {
    bend.value = String(currentBend);
  } else {
    bend.value = "custom";
    custom.value = currentBend;
  }
  custom.classList.toggle("hidden", bend.value !== "custom");
  unit.classList.toggle("hidden", bend.value !== "custom");

  const sendBend = (value) =>
    send(
      commands.midiBind(
        manual, slot, input.device, input.channel ?? "any", null, null,
        input.transpose ?? 0, value
      )
    );
  bend.addEventListener("change", () => {
    const showCustom = bend.value === "custom";
    custom.classList.toggle("hidden", !showCustom);
    unit.classList.toggle("hidden", !showCustom);
    if (showCustom) {
      custom.value = BEND_PRESETS.includes(currentBend) || currentBend == null ? 24 : currentBend;
      custom.focus();
      // Wait for the custom field itself before sending anything.
    } else {
      sendBend(bend.value);
    }
  });
  custom.addEventListener("change", () => {
    const semitones = Math.min(96, Math.max(1, Math.trunc(Number(custom.value) || 0)));
    custom.value = semitones;
    sendBend(semitones);
    custom.blur(); // hand the field back to the snapshot
  });

  line.append(label, bend, custom, unit);
  return line;
}

/// What this input's keys will actually sound, shift included. The
/// line leads with the resulting pitches and speaks the shift in
/// words ("an octave lower"): the reader is an organist, not a MIDI
/// programmer. Keys reaching past the set's own compass are worth
/// saying out loud rather than leaving to be discovered — repitched
/// pipes for a real keyboard, silence for the computer keyboard.
function compassNote(ctx, manualEntry, input) {
  const note = document.createElement("span");
  note.className = "input-compass";
  const native = manualEntry.native;
  const shift = input.transpose ?? 0;
  const computer = input.device === COMPUTER_KEYBOARD;
  const learned = input.low != null && input.high != null;
  // The keys this input can send: measured by Listen, the two letter
  // rows for the computer keyboard, else assumed to be the set's own.
  const span = learned
    ? [Math.min(input.low, input.high), Math.max(input.low, input.high)]
    : computer
      ? ctx.keyboardSpan
      : native;
  if (!span) return note;
  const clamp = (key) => Math.min(127, Math.max(0, key + shift));
  const sounds = [clamp(span[0]), clamp(span[1])];
  const range = `${keyName(sounds[0])}–${keyName(sounds[1])}`;
  const parts = [
    learned
      ? `keys ${keyName(span[0])}–${keyName(span[1])} sound ${range}`
      : `sounds ${range}`,
  ];
  if (shift) parts.push(shiftWords(shift));
  if (!learned && !computer && !shift) parts.push("the set's own compass");
  if (native) {
    const outside =
      Math.max(0, native[0] - sounds[0]) + Math.max(0, sounds[1] - native[1]);
    if (outside) {
      const past = `past the set's ${keyName(native[0])}–${keyName(native[1])}`;
      parts.push(
        computer
          ? `${outside} key${outside === 1 ? "" : "s"} ${past} stay silent`
          : `${outside} key${outside === 1 ? "" : "s"} repitched ${past}`
      );
    } else if (learned) {
      parts.push("within the set's compass");
    }
  }
  note.classList.toggle("dim", !learned);
  note.textContent = parts.join(" · ");
  return note;
}

// ---- control binding rows -------------------------------------------------
//
// Unlike the MIDI rows these are action-first, not manual-first: a
// binding doesn't belong to a manual, so a flat list is the honest
// shape. The distributed piston rows (a stop's own trigger on its
// editor popover) are filtered views over the same list.

/// The full flat list. `ctx` = {snapshot, learning, send}; `learning`
/// is the snapshot's control_learning slot, or null.
export function buildControlsList(container, ctx) {
  const { snapshot, learning } = ctx;
  const controls = snapshot.controls ?? [];
  // enclosure: is real and useful but isn't in the server's catalogue
  // (it predates this pane); offer it here regardless.
  const catalogue = [...(snapshot.actions ?? []), "enclosure:"];
  // A binding just started from "+ add binding" has no row of its own
  // yet — the same pending-row trick the MIDI rows use while they
  // wait for a first key.
  const pending = learning === controls.length ? controls.length : null;
  if (!controls.length && pending == null) {
    container.append(emptyNote("No bindings yet — add one below."));
    return;
  }
  for (const control of controls) {
    container.append(controlRow(ctx, control, control.slot, catalogue));
  }
  if (pending != null) {
    container.append(controlRow(ctx, null, pending, catalogue));
  }
}

/// One binding: what arrived, what it does, and the two ways to set
/// each — Listen for the trigger, a pair of selects for the action.
function controlRow(ctx, control, slot, catalogue) {
  const { snapshot, learning, send } = ctx;
  const listening = learning === slot;
  const action = control?.action ?? "octave-up";
  const verb = actionEntry(action, catalogue);

  const row = document.createElement("div");
  row.className = "control-row";
  row.classList.toggle("listening", listening);

  const trigger = document.createElement("span");
  trigger.className = "control-trigger";
  trigger.classList.toggle("dim", !control?.trigger);
  trigger.textContent = triggerText(control);
  trigger.title = trigger.textContent;
  row.append(trigger);

  const actionSelect = document.createElement("select");
  actionSelect.className = "control-action";
  for (const entry of catalogue) actionSelect.append(option(entry, actionLabel(entry)));
  actionSelect.value = verb && catalogue.includes(verb) ? verb : catalogue[0];
  actionSelect.addEventListener("change", () => {
    // A stem means nothing without a target; default to the first one
    // rather than sending a bare "stop:" or "general:".
    const target = defaultTarget(actionSelect.value, snapshot);
    send(commands.controlBind(slot, `${actionSelect.value}${target ?? ""}`));
  });
  row.append(actionSelect);

  const stem = actionSelect.value;
  const arg = actionArg(action, stem);
  /// One target select on this row, sending "<stem><value>".
  const targetSelect = (fill, compose = (value) => value) => {
    const target = document.createElement("select");
    target.className = "control-target";
    fill(target);
    target.addEventListener("change", () =>
      send(commands.controlBind(slot, `${stem}${compose(target.value)}`))
    );
    row.append(target);
    return target;
  };

  if (NAMED_ACTIONS.includes(stem)) {
    const names = namesFor(stem, snapshot);
    targetSelect((target) => {
      for (const name of names) target.append(option(name, name));
      target.value = names.includes(arg) ? arg : names[0] ?? "";
    });
  } else if (stem in NUMBERED_ACTIONS) {
    targetSelect((target) => numberOptions(target, NUMBERED_ACTIONS[stem], arg));
  } else if (stem === DIVISIONAL_ACTION) {
    // A divisional is a manual *and* a slot, so it wears two selects
    // and either of them rewrites the whole action.
    const [argManual, argSlot] = splitLast(arg);
    const names = namesFor(stem, snapshot);
    let manual = names.includes(argManual) ? argManual : names[0] ?? "";
    let piston = argSlot || "1";
    targetSelect(
      (target) => {
        for (const name of names) target.append(option(name, name));
        target.value = manual;
        target.addEventListener("change", () => (manual = target.value));
      },
      (value) => `${value}:${piston}`
    );
    targetSelect(
      (target) => {
        numberOptions(target, DIVISIONAL_PISTONS, piston);
        target.addEventListener("change", () => (piston = target.value));
      },
      (value) => `${manual}:${value}`
    );
  } else if (PITCH_ACTIONS.includes(verb)) {
    const manuals = snapshot.manuals ?? [];
    const target = document.createElement("select");
    target.className = "control-target";
    // "Same keyboard" is the default and by far the common case —
    // the transposer on a console shifts the console it is part of.
    target.append(option("any", "same keyboard"));
    for (const manual of manuals) target.append(option(manual.name, manual.name));
    target.value = manuals.some((m) => m.name === control?.manual) ? control.manual : "any";
    target.addEventListener("change", () =>
      send(commands.controlBind(slot, action, { manual: target.value }))
    );
    row.append(target);
  }

  const listen = document.createElement("button");
  listen.className = "ghost listen";
  listen.textContent = listening ? "Cancel" : "Listen";
  listen.title = "Assign by pressing the piston, pedal or key you mean";
  listen.addEventListener("click", () =>
    send(commands.controlLearn(listening ? null : slot))
  );
  row.append(listen);

  const remove = document.createElement("button");
  remove.className = "ghost remove-input";
  remove.textContent = "×";
  remove.setAttribute("aria-label", "Remove this binding");
  remove.addEventListener("click", () => send(commands.controlUnbind(slot)));
  row.append(remove);

  if (listening) {
    const hint = document.createElement("span");
    hint.className = "listen-hint";
    hint.textContent = "press the piston, pedal or key…";
    row.append(hint);
  }
  return row;
}

/// A piston row for one action: the triggers already bound to it, each
/// removable, plus Listen — which learns a fresh trigger and then
/// points it at this action (the editor's quick-bind flow supplies
/// `onListen`). A filtered view over the same flat list the Bindings
/// popover shows in full. `ctx.manual`, when given, narrows a pitch
/// action to the bindings targeting that manual by name;
/// `ctx.listening` marks a quick-bind in flight for this very row.
export function pistonRow(ctx, action, onListen) {
  const { snapshot, send } = ctx;
  const row = document.createElement("div");
  row.className = "piston-row";

  const bound = (snapshot.controls ?? []).filter(
    (c) =>
      c.action === action &&
      c.trigger &&
      (ctx.manual === undefined || c.manual === ctx.manual)
  );
  for (const control of bound) {
    const chip = document.createElement("span");
    chip.className = "piston-chip";
    const text = document.createElement("span");
    text.textContent = triggerText(control);
    text.title = text.textContent;
    const remove = document.createElement("button");
    remove.className = "ghost remove-input";
    remove.textContent = "×";
    remove.setAttribute("aria-label", `Unbind ${triggerText(control)}`);
    remove.addEventListener("click", () => send(commands.controlUnbind(control.slot)));
    chip.append(text, remove);
    row.append(chip);
  }

  const listening = !!ctx.listening;
  if (!bound.length && !listening) {
    const none = document.createElement("span");
    none.className = "piston-none";
    none.textContent = "none";
    row.append(none);
  }
  if (listening) {
    const hint = document.createElement("span");
    hint.className = "listen-hint";
    hint.textContent = "press the piston, pedal or key…";
    row.append(hint);
  }

  const listen = document.createElement("button");
  listen.className = "ghost listen";
  listen.textContent = listening ? "Cancel" : "Listen";
  listen.title = "Bind a piston, pedal or key by pressing it";
  listen.addEventListener("click", () => onListen(action, listening));
  row.append(listen);

  return row;
}

/// A read-only line, not a control: the computer keyboard is assigned
/// in a keyboard's MIDI popover like any other device, this just says
/// where it currently lands.
export function keyboardNote(snapshot) {
  const keyboard = snapshot.keyboard;
  if (!keyboard) {
    return (
      "Computer keyboard: unassigned — give it a manual in a keyboard's " +
      "MIDI input popover, like any other device."
    );
  }
  const manual = (snapshot.manuals ?? []).find((m) => m.idx === keyboard.manual);
  const where = manual ? manual.name : `manual ${keyboard.manual}`;
  const shift = keyboard.transpose;
  return shift
    ? `Computer keyboard plays ${where}, ${shiftWords(shift)}.`
    : `Computer keyboard plays ${where}, at pitch.`;
}
