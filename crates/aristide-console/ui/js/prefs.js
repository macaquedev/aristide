// The preferences dialog: everything about the instrument that isn't
// played — which keyboard drives which manual, temperament, room and
// mechanism noises, and the skin.
//
// Like the console proper it mirrors the server snapshot rather than
// holding its own copy: rows are rebuilt only when the shape changes
// (a device appears, a different organ loads), and values are written
// back on every poll except into a control the user is touching.

import { commands, COMPUTER_KEYBOARD } from "./api.js";
import { shiftWords } from "./pitch.js";

const NOTE_NAMES = ["C", "C♯", "D", "E♭", "E", "F", "F♯", "G", "A♭", "A", "B♭", "B"];

/// MIDI note number in the naming organists read on a stoplist: middle
/// C (60) is C4, as every sample set's documentation writes it.
function keyName(key) {
  return `${NOTE_NAMES[key % 12]}${Math.floor(key / 12) - 1}`;
}

const TABS = ["organ", "midi", "controls", "tuning", "sound", "appearance"];

/// Clamp to a MIDI note, the same tolerant parse the transpose and shift
/// fields use: whatever the box holds, not a value that can 400 the server.
function clampNote(value) {
  return Math.min(127, Math.max(0, Math.trunc(Number(value) || 0)));
}

// A page loads at most once; a combined-but-unsaved organ should ask
// "how should these go together?" exactly once, not every time the poll
// happens to notice it's still unsaved.
let implicitPromptShown = false;

// The pitch actions all take an optional target manual; everything else
// in the catalogue is either global (panic, cancel) or names its own
// target through a second word (stop:, coupler:, enclosure:).
const PITCH_ACTIONS = [
  "octave-up",
  "octave-down",
  "transpose-up",
  "transpose-down",
  "transpose-reset",
];
const NAMED_ACTIONS = ["stop:", "coupler:", "enclosure:"];

const ACTION_LABELS = {
  "octave-up": "Octave up",
  "octave-down": "Octave down",
  "transpose-up": "Transpose up",
  "transpose-down": "Transpose down",
  "transpose-reset": "Transpose reset",
  tremulant: "Tremulant",
  cancel: "General cancel",
  panic: "Panic",
  "stop:": "Stop…",
  "coupler:": "Coupler…",
  "enclosure:": "Enclosure…",
};
function actionLabel(action) {
  return ACTION_LABELS[action] ?? action;
}

/// Split "stop:Montre 8'" into its verb ("stop:") and argument, the way
/// the server itself reads an action string.
function actionVerb(action) {
  const at = action.indexOf(":");
  return at === -1 ? action : action.slice(0, at + 1);
}
function actionArg(action) {
  const at = action.indexOf(":");
  return at === -1 ? "" : action.slice(at + 1);
}

function namesFor(verb, snapshot) {
  if (verb === "stop:") return (snapshot.stops ?? []).map((s) => s.name);
  if (verb === "coupler:") return (snapshot.couplers ?? []).map((c) => c.name);
  if (verb === "enclosure:") return (snapshot.enclosures ?? []).map((e) => e.name);
  return [];
}

// A handful of physical keys read better as the character they print
// ("=") than as their event.code name ("Equal"); the rest are left as
// the code itself, which is still legible enough for a letter or digit.
const KEY_GLYPHS = {
  Equal: "=", Minus: "-", Comma: ",", Period: ".", Slash: "/",
  Semicolon: ";", Quote: "'", BracketLeft: "[", BracketRight: "]",
  Backquote: "`", Backslash: "\\", Space: "Space",
};
function keyGlyph(code) {
  if (code in KEY_GLYPHS) return KEY_GLYPHS[code];
  if (code.startsWith("Key")) return code.slice(3);
  if (code.startsWith("Digit")) return code.slice(5);
  return code;
}

/// The trigger cell reads as prose, not as the wire format: a MIDI
/// message names its device and channel; a computer key just prints the
/// character it is, since the device is always the same one.
function triggerText(control) {
  if (!control || !control.trigger) return "— press Listen —";
  if (control.trigger.startsWith("key:")) {
    return `key:${keyGlyph(control.trigger.slice(4))}`;
  }
  const channel = control.channel ? ` ch${control.channel}` : "";
  return `${control.trigger} · ${control.device}${channel}`;
}

export class Preferences {
  constructor(root, base, send) {
    this.root = root;
    this.base = base;
    this.send = send;
    this.dragging = new Set();
    this.tuning = null; // the shared, instrument-wide tuning
    this.manualTuning = new Map(); // idx -> its own tuning, for divisions tuned apart
    this.tuningTarget = null; // null = whole instrument, else a manual idx
    this.tuningTargetSignature = null;
    this.displayed = null; // whichever tuning the fields currently show
    this.midiSignature = null;
    this.learning = null;
    this.controlsSignature = null;
    this.controlLearning = null;
    this.controlsCount = 0;
    this.organSignature = null;
    this.tab = "midi";
    this.el = {
      modal: root.getElementById("prefs"),
      subject: root.getElementById("prefs-subject"),
      tabs: root.getElementById("prefs-tabs"),
      panes: [...root.querySelectorAll("#prefs .pane")],
      organImplicitNote: root.getElementById("organ-implicit-note"),
      organSummary: root.getElementById("organ-summary"),
      organSave: root.getElementById("organ-save"),
      organSavePath: root.getElementById("organ-save-path"),
      organSaveBtn: root.getElementById("organ-save-btn"),
      organSaveError: root.getElementById("organ-save-error"),
      organCompass: root.getElementById("organ-compass"),
      organStops: root.getElementById("organ-stops"),
      organCouplers: root.getElementById("organ-couplers"),
      manuals: root.getElementById("midi-manuals"),
      ports: root.getElementById("midi-ports"),
      unassigned: root.getElementById("midi-unassigned"),
      rescan: root.getElementById("midi-rescan"),
      controlsList: root.getElementById("controls-list"),
      controlsAdd: root.getElementById("controls-add"),
      controlsKeyboard: root.getElementById("controls-keyboard"),
      tuningTarget: root.getElementById("tuning-target"),
      tuningTargetRow: root.getElementById("tuning-target-row"),
      tuningReset: root.getElementById("tuning-reset"),
      temperament: root.getElementById("set-temperament"),
      a4: root.getElementById("set-a4"),
      temperamentRow: root.getElementById("temperament-row"),
      pitchRow: root.getElementById("pitch-row"),
      transposeRow: root.getElementById("transpose-row"),
      transposeDown: root.getElementById("transpose-down"),
      transposeUp: root.getElementById("transpose-up"),
      transposeValue: root.getElementById("transpose-value"),
      reverbRow: root.getElementById("reverb-row"),
      reverb: root.getElementById("reverb"),
      noisesRow: root.getElementById("noises-row"),
      noisesOn: root.getElementById("noises-on"),
      noisesVol: root.getElementById("noises-vol"),
      about: root.getElementById("about"),
    };
    this.wire();
    this.select(this.tab);
  }

  // ---- the dialog itself ------------------------------------------------

  open(tab) {
    if (tab) this.select(tab);
    this.el.modal.classList.remove("hidden");
    this.root.body.classList.add("modal-open");
  }

  close() {
    // Leaving the dialog ends any wait for a key: the next thing the
    // player touches should sound, not be swallowed as an assignment.
    if (this.learning) this.send(commands.midiLearn(null));
    if (this.controlLearning != null) this.send(commands.controlLearn(null));
    this.el.modal.classList.add("hidden");
    this.el.about.classList.add("hidden");
    this.root.body.classList.remove("modal-open");
  }

  get isOpen() {
    return !this.el.modal.classList.contains("hidden");
  }

  openAbout() {
    this.el.about.classList.remove("hidden");
    this.root.body.classList.add("modal-open");
  }

  select(tab) {
    this.tab = TABS.includes(tab) ? tab : "midi";
    for (const button of this.el.tabs.children) {
      button.classList.toggle("on", button.dataset.tab === this.tab);
    }
    for (const pane of this.el.panes) {
      pane.classList.toggle("hidden", pane.dataset.pane !== this.tab);
    }
  }

  wire() {
    for (const button of this.el.tabs.children) {
      button.addEventListener("click", () => this.select(button.dataset.tab));
    }
    for (const closer of this.root.querySelectorAll("#prefs [data-close], #about [data-close]")) {
      closer.addEventListener("click", () => this.close());
    }
    // Esc closes preferences or about when either is the one up — not
    // just whenever *some* modal (the organ picker, say) has set
    // `modal-open` on the body. The console keeps its keys otherwise
    // (keys.js stays quiet while `modal-open` is set).
    window.addEventListener("keydown", (event) => {
      const open = this.isOpen || !this.el.about.classList.contains("hidden");
      if (event.key === "Escape" && open) {
        event.preventDefault();
        this.close();
      }
    });

    this.el.rescan.addEventListener("click", () => this.send(commands.midiRescan()));
    // A new slot doesn't exist on the server until either a bind or a
    // learned trigger names it; learning one past the end is enough —
    // learn_control defaults a slot with nothing saved to "octave-up".
    this.el.controlsAdd.addEventListener("click", () =>
      this.send(commands.controlLearn(this.controlsCount))
    );
    this.wireTuning();
    this.wireSound();
    this.wireOrgan();
  }

  // ---- snapshot ----------------------------------------------------------

  update(snapshot) {
    this.el.subject.textContent = snapshot.organ ?? "";
    this.refreshOrgan(snapshot);
    this.refreshMidi(snapshot);
    this.refreshControls(snapshot);
    this.refreshTuning(snapshot);
    this.refreshSound(snapshot);

    // An organ combined ad hoc on the command line has nobody to ask "how
    // should these go together?" but the player — open straight to the
    // tab that answers it, once, and never fight them for it again.
    if (snapshot.setup?.implicit && !implicitPromptShown) {
      implicitPromptShown = true;
      this.open("organ");
    }
  }

  // ---- MIDI --------------------------------------------------------------
  //
  // Read manual-first, the way an organist asks the question: *what
  // plays the Récit?* A manual holds a list of inputs, so two keyboards
  // can share a division and one console can feed several manuals by
  // splitting itself across channels.
  //
  // Nothing here is edited in place: the rows are rebuilt whenever the
  // assignments change, which is only ever just after the user acted on
  // one (and so blurred it). Between those the pane is inert.

  refreshMidi(snapshot) {
    const midi = snapshot.midi ?? { ports: [], manuals: [] };
    this.learning = midi.learning ?? null;
    // The computer keyboard's own span (the two letter rows), from the
    // server — present exactly while an input row names it.
    this.keyboardSpan = snapshot.keyboard
      ? [snapshot.keyboard.low, snapshot.keyboard.high]
      : null;
    const signature = JSON.stringify([midi.ports, midi.manuals, midi.learning ?? null]);
    if (signature !== this.midiSignature) {
      this.midiSignature = signature;
      this.buildManuals(midi);
      this.buildPorts(midi.ports);
    }

    // Silence is the honest default for an organ nobody has set up, but
    // it looks like a fault unless the dialog says so.
    const nothing =
      midi.manuals.length > 0 && midi.manuals.every((m) => !m.inputs.length);
    this.el.unassigned.classList.toggle("hidden", !nothing);
  }

  buildManuals(midi) {
    this.el.manuals.replaceChildren();
    if (!midi.manuals.length) {
      this.el.manuals.append(this.emptyNote("No organ loaded — nothing to assign."));
      return;
    }
    for (const manual of midi.manuals) {
      const row = document.createElement("div");
      row.className = "midi-manual";

      const name = document.createElement("span");
      name.className = "manual-name";
      name.textContent = manual.name;
      name.title = manual.name;

      const inputs = document.createElement("div");
      inputs.className = "manual-inputs";
      for (const input of manual.inputs) {
        inputs.append(this.inputRow(midi, manual.idx, input.slot, input));
        inputs.append(this.compassNote(midi, manual, input));
      }
      // A manual with nothing on it still shows one row: the empty state
      // has to be assignable, not just described.
      const learning = midi.learning;
      const pending =
        learning && learning.manual === manual.idx && learning.slot >= manual.inputs.length;
      if (!manual.inputs.length || pending) {
        inputs.append(this.inputRow(midi, manual.idx, manual.inputs.length, null));
      }
      if (manual.inputs.length && !pending) {
        const add = document.createElement("button");
        add.className = "ghost add-input";
        add.textContent = "+ add input";
        add.title = "A second keyboard playing this same manual";
        add.addEventListener("click", () =>
          this.send(commands.midiLearn(manual.idx, manual.inputs.length))
        );
        inputs.append(add);
      }

      row.append(name, inputs);
      this.el.manuals.append(row);
    }
  }

  /// One assignment: which device, on which channel, plus the two ways
  /// to set it — play a key, or say so.
  inputRow(midi, manual, slot, input) {
    const listening =
      midi.learning && midi.learning.manual === manual && midi.learning.slot === slot;
    const row = document.createElement("div");
    row.className = "manual-input";
    row.classList.toggle("listening", !!listening);
    row.classList.toggle("missing", !!input && !input.connected);

    const device = document.createElement("select");
    device.className = "input-device";
    if (!input) {
      device.append(this.option("", "— no input —"));
    }
    for (const port of midi.ports) {
      device.append(this.option(port.name, port.name));
    }
    // A binding survives its keyboard being unplugged; the row says so
    // rather than quietly dropping the assignment.
    if (input && !midi.ports.some((port) => port.name === input.device)) {
      device.append(this.option(input.device, `${input.device} (not connected)`));
    }
    device.value = input ? input.device : "";
    device.addEventListener("change", () => {
      if (!device.value) return;
      // An existing row keeps the channel it had, "any" included; a new
      // one sends none, which lets the server apply what the set
      // suggests for this manual.
      const channel = input ? (input.channel ?? "any") : null;
      this.send(commands.midiBind(manual, slot, device.value, channel));
    });

    const channel = document.createElement("select");
    channel.className = "input-channel";
    channel.append(this.option("any", "any channel"));
    for (let ch = 1; ch <= 16; ch++) channel.append(this.option(String(ch), `channel ${ch}`));
    channel.value = input ? (input.channel == null ? "any" : String(input.channel)) : "any";
    // The computer keyboard has no channels to tell apart.
    channel.disabled = !input || input.device === COMPUTER_KEYBOARD;
    channel.addEventListener("change", () =>
      this.send(commands.midiBind(manual, slot, input.device, channel.value))
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
      this.send(
        commands.midiBind(manual, slot, input.device, input.channel ?? "any", null, null, semitones)
      );
      shift.blur(); // hand the field back to the snapshot
    });

    const listen = document.createElement("button");
    listen.className = "ghost listen";
    listen.textContent = listening ? "Cancel" : "Listen";
    listen.title = "Assign by playing a key on the keyboard you mean";
    listen.addEventListener("click", () =>
      this.send(listening ? commands.midiLearn(null) : commands.midiLearn(manual, slot))
    );

    row.append(device, channel, shift, listen);
    if (input) {
      const remove = document.createElement("button");
      remove.className = "ghost remove-input";
      remove.textContent = "×";
      remove.setAttribute("aria-label", `Remove ${input.device}`);
      remove.addEventListener("click", () => this.send(commands.midiUnbind(manual, slot)));
      row.append(remove);
    }
    if (listening) {
      const hint = document.createElement("span");
      hint.className = "listen-hint";
      hint.textContent =
        midi.learning.step === "high" ? "now the highest key…" : "play the lowest key…";
      row.append(hint);
    }
    return row;
  }

  /// What this input's keys will actually sound, shift included. The
  /// line leads with the resulting pitches and speaks the shift in
  /// words ("an octave lower"): the reader is an organist, not a MIDI
  /// programmer. Keys reaching past the set's own compass are worth
  /// saying out loud rather than leaving to be discovered — repitched
  /// pipes for a real keyboard, silence for the computer keyboard.
  compassNote(midi, manualEntry, input) {
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
        ? this.keyboardSpan
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

  buildPorts(ports) {
    this.el.ports.replaceChildren();
    if (!ports.length) {
      this.el.ports.append(
        this.emptyNote("No MIDI inputs. Plug the console in — the list finds it by itself.")
      );
      return;
    }
    for (const port of ports) {
      const row = document.createElement("div");
      row.className = "midi-port";
      row.textContent = port.name;
      row.title = port.name;
      this.el.ports.append(row);
    }
  }

  option(value, label) {
    const option = document.createElement("option");
    option.value = value;
    option.textContent = label;
    return option;
  }

  emptyNote(text) {
    const empty = document.createElement("p");
    empty.className = "pane-empty";
    empty.textContent = text;
    return empty;
  }

  // ---- Controls ------------------------------------------------------------
  //
  // Unlike MIDI's pane this one is action-first, not manual-first: a
  // binding doesn't belong to a manual, so a flat list is the honest
  // shape. Same discipline otherwise — rebuilt only when something the
  // rows depend on changes, driven entirely by the snapshot.

  refreshControls(snapshot) {
    const controls = snapshot.controls ?? [];
    const actions = snapshot.actions ?? [];
    this.controlLearning = snapshot.control_learning ?? null;
    this.controlsCount = controls.length;
    const signature = JSON.stringify([
      controls,
      actions,
      this.controlLearning,
      (snapshot.stops ?? []).map((s) => s.name),
      (snapshot.couplers ?? []).map((c) => c.name),
      (snapshot.enclosures ?? []).map((e) => e.name),
      (snapshot.manuals ?? []).map((m) => m.name),
    ]);
    if (signature !== this.controlsSignature) {
      this.controlsSignature = signature;
      this.buildControls(snapshot);
    }
    this.refreshKeyboardNote(snapshot);
  }

  buildControls(snapshot) {
    const controls = snapshot.controls ?? [];
    // enclosure: is real and useful but isn't in the server's catalogue
    // (it predates this pane); offer it here regardless.
    const catalogue = [...(snapshot.actions ?? []), "enclosure:"];
    this.el.controlsList.replaceChildren();
    // A binding just started from "+ add binding" has no row of its own
    // yet — the same pending-row trick the MIDI pane uses while it
    // waits for a first key.
    const pending = this.controlLearning === controls.length ? controls.length : null;
    if (!controls.length && pending == null) {
      this.el.controlsList.append(this.emptyNote("No bindings yet — add one below."));
      return;
    }
    for (const control of controls) {
      this.el.controlsList.append(this.controlRow(control, control.slot, catalogue, snapshot));
    }
    if (pending != null) {
      this.el.controlsList.append(this.controlRow(null, pending, catalogue, snapshot));
    }
  }

  /// One binding: what arrived, what it does, and the two ways to set
  /// each — Listen for the trigger, a pair of selects for the action.
  controlRow(control, slot, catalogue, snapshot) {
    const listening = this.controlLearning === slot;
    const action = control?.action ?? "octave-up";
    const verb = actionVerb(action);

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
    for (const entry of catalogue) actionSelect.append(this.option(entry, actionLabel(entry)));
    actionSelect.value = catalogue.includes(verb) ? verb : catalogue[0];
    actionSelect.addEventListener("change", () => {
      const named = NAMED_ACTIONS.includes(actionSelect.value);
      // A named action means nothing without a target; default to the
      // first one on the list rather than sending a bare "stop:".
      const names = named ? namesFor(actionSelect.value, snapshot) : [];
      const next = named ? `${actionSelect.value}${names[0] ?? ""}` : actionSelect.value;
      this.send(commands.controlBind(slot, next));
    });
    row.append(actionSelect);

    if (NAMED_ACTIONS.includes(actionSelect.value)) {
      const names = namesFor(actionSelect.value, snapshot);
      const arg = actionArg(action);
      const target = document.createElement("select");
      target.className = "control-target";
      for (const name of names) target.append(this.option(name, name));
      target.value = names.includes(arg) ? arg : names[0] ?? "";
      target.addEventListener("change", () =>
        this.send(commands.controlBind(slot, `${actionSelect.value}${target.value}`))
      );
      row.append(target);
    } else if (PITCH_ACTIONS.includes(verb)) {
      const manuals = snapshot.manuals ?? [];
      const target = document.createElement("select");
      target.className = "control-target";
      // "Same keyboard" is the default and by far the common case —
      // the transposer on a console shifts the console it is part of.
      target.append(this.option("any", "same keyboard"));
      for (const manual of manuals) target.append(this.option(manual.name, manual.name));
      target.value = manuals.some((m) => m.name === control?.manual) ? control.manual : "any";
      target.addEventListener("change", () =>
        this.send(commands.controlBind(slot, action, { manual: target.value }))
      );
      row.append(target);
    }

    const listen = document.createElement("button");
    listen.className = "ghost listen";
    listen.textContent = listening ? "Cancel" : "Listen";
    listen.title = "Assign by pressing the piston, pedal or key you mean";
    listen.addEventListener("click", () =>
      this.send(commands.controlLearn(listening ? null : slot))
    );
    row.append(listen);

    const remove = document.createElement("button");
    remove.className = "ghost remove-input";
    remove.textContent = "×";
    remove.setAttribute("aria-label", "Remove this binding");
    remove.addEventListener("click", () => this.send(commands.controlUnbind(slot)));
    row.append(remove);

    if (listening) {
      const hint = document.createElement("span");
      hint.className = "listen-hint";
      hint.textContent = "press the piston, pedal or key…";
      row.append(hint);
    }
    return row;
  }

  /// A read-only line, not a control: the computer keyboard is assigned
  /// in the MIDI tab like any other device, this just says where it
  /// currently lands.
  refreshKeyboardNote(snapshot) {
    const keyboard = snapshot.keyboard;
    if (!keyboard) {
      this.el.controlsKeyboard.textContent =
        "Computer keyboard: unassigned — give it a manual in the MIDI tab, like any other device.";
      return;
    }
    const manual = (snapshot.manuals ?? []).find((m) => m.idx === keyboard.manual);
    const where = manual ? manual.name : `manual ${keyboard.manual}`;
    const shift = keyboard.transpose;
    this.el.controlsKeyboard.textContent = shift
      ? `Computer keyboard plays ${where}, ${shiftWords(shift)}.`
      : `Computer keyboard plays ${where}, at pitch.`;
  }

  // ---- tuning ------------------------------------------------------------
  //
  // Most organs have one tuning for the whole instrument; a division can
  // be pulled apart from it and tuned on its own. The DIVISION picker
  // just decides which tuning the fields below are looking at right
  // now — it is never itself sent anywhere. Every field send carries
  // `manual` when a division is picked, so it lands on that division's
  // own tuning rather than the instrument's.

  wireTuning() {
    this.el.tuningTarget.addEventListener("change", () => {
      this.tuningTarget =
        this.el.tuningTarget.value === "" ? null : Number(this.el.tuningTarget.value);
      this.syncTuningDisplay();
    });

    this.el.tuningReset.addEventListener("click", () => {
      if (this.tuningTarget == null) return;
      this.send(commands.tuning({ manual: this.tuningTarget, reset: 1 }));
    });

    this.el.temperament.addEventListener("change", () => {
      const fields = { temperament: this.el.temperament.value };
      if (this.tuningTarget != null) fields.manual = this.tuningTarget;
      this.send(commands.tuning(fields));
      this.el.temperament.blur(); // hand the field back to the snapshot
    });

    this.el.a4.addEventListener("change", () => {
      const a4 = Math.min(500, Math.max(300, Number(this.el.a4.value) || 440));
      this.el.a4.value = a4;
      const fields = { a4 };
      if (this.tuningTarget != null) fields.manual = this.tuningTarget;
      this.send(commands.tuning(fields));
      this.el.a4.blur();
    });

    for (const [button, step] of [
      [this.el.transposeDown, -1],
      [this.el.transposeUp, +1],
    ]) {
      button.addEventListener("click", () => {
        const at = this.displayed?.transpose ?? 0;
        const transpose = Math.min(12, Math.max(-12, at + step));
        if (transpose === at) return;
        // Optimistic, so rapid clicks step from the value just sent
        // rather than the last poll.
        this.displayed = { ...this.displayed, transpose };
        this.el.transposeValue.textContent =
          transpose > 0 ? `+${transpose}` : `${transpose}`;
        const fields = { transpose };
        if (this.tuningTarget != null) fields.manual = this.tuningTarget;
        this.send(commands.tuning(fields));
      });
    }
  }

  /// A division with no tuning of its own simply plays the instrument's —
  /// that's what "effective" means here, and it's what the fields show
  /// until the player gives the division one.
  effectiveTuning(target) {
    if (target == null) return this.tuning;
    return this.manualTuning.get(target) ?? this.tuning;
  }

  /// Mirrors whichever tuning the DIVISION picker currently points at,
  /// except into inputs the user is touching right now: a focused field
  /// keeps its local value until it's blurred.
  syncTuningDisplay() {
    this.displayed = this.effectiveTuning(this.tuningTarget);
    this.el.tuningReset.classList.toggle(
      "hidden",
      !(this.tuningTarget != null && this.manualTuning.has(this.tuningTarget))
    );
    if (!this.displayed) return;
    if (this.root.activeElement !== this.el.temperament) {
      this.el.temperament.value = this.displayed.temperament;
    }
    if (this.root.activeElement !== this.el.a4) this.el.a4.value = this.displayed.a4;
    this.el.transposeValue.textContent =
      this.displayed.transpose > 0 ? `+${this.displayed.transpose}` : `${this.displayed.transpose}`;
  }

  /// The picker's option list: "Whole instrument" plus one entry per
  /// manual, marked when that manual already has tuning of its own.
  /// Rebuilt only when the roster or those marks change.
  buildTuningTargets(manuals) {
    const kept = this.tuningTarget;
    this.el.tuningTarget.replaceChildren();
    this.el.tuningTarget.append(this.option("", "Whole instrument"));
    for (const manual of manuals) {
      const mark = this.manualTuning.has(manual.idx) ? " •" : "";
      this.el.tuningTarget.append(this.option(String(manual.idx), `${manual.name}${mark}`));
    }
    this.el.tuningTarget.value = kept == null ? "" : String(kept);
  }

  refreshTuning(snapshot) {
    const tuning = snapshot.tuning ?? null;
    this.tuning = tuning;
    this.manualTuning = new Map((snapshot.manual_tuning ?? []).map((t) => [t.idx, t]));

    for (const row of [
      this.el.tuningTargetRow, this.el.temperamentRow, this.el.pitchRow, this.el.transposeRow,
    ]) {
      row.classList.toggle("hidden", !tuning);
    }
    if (!tuning) return;

    const manuals = snapshot.manuals ?? [];
    const signature = JSON.stringify([
      manuals.map((m) => [m.idx, m.name]),
      [...this.manualTuning.keys()].sort((a, b) => a - b),
    ]);
    if (signature !== this.tuningTargetSignature) {
      this.tuningTargetSignature = signature;
      this.buildTuningTargets(manuals);
    }

    // The division being looked at can vanish out from under the player
    // — a different organ loads, or that manual is gone. Whole
    // instrument is always a safe fallback.
    if (this.tuningTarget != null && !manuals.some((m) => m.idx === this.tuningTarget)) {
      this.tuningTarget = null;
      if (this.root.activeElement !== this.el.tuningTarget) this.el.tuningTarget.value = "";
    }

    this.syncTuningDisplay();
  }

  // ---- sound --------------------------------------------------------------

  wireSound() {
    this.throttled(this.el.reverb, "reverb", () =>
      this.send(commands.reverb(this.el.reverb.value))
    );
    const sendNoises = () =>
      this.send(commands.noises(this.el.noisesOn.checked, this.el.noisesVol.value));
    this.el.noisesOn.addEventListener("change", sendNoises);
    this.throttled(this.el.noisesVol, "noises-vol", sendNoises);
  }

  /// A slider that reports while it moves: ~30 commands/s during the
  /// drag, one final value on release.
  throttled(slider, key, send) {
    let lastSent = 0;
    slider.addEventListener("pointerdown", () => this.dragging.add(key));
    slider.addEventListener("input", () => {
      const now = performance.now();
      if (now - lastSent > 33) {
        lastSent = now;
        send();
      }
    });
    slider.addEventListener("change", () => {
      this.dragging.delete(key);
      send();
    });
  }

  refreshSound(snapshot) {
    this.el.reverbRow.classList.toggle("hidden", snapshot.reverb == null);
    if (snapshot.reverb != null && !this.dragging.has("reverb")) {
      this.el.reverb.value = snapshot.reverb;
    }
    this.el.noisesRow.classList.toggle("hidden", !snapshot.noises);
    if (snapshot.noises) {
      this.el.noisesOn.checked = snapshot.noises.on;
      if (!this.dragging.has("noises-vol")) this.el.noisesVol.value = snapshot.noises.vol;
    }
  }

  // ---- organ (composite setup) -------------------------------------------
  //
  // Where this instrument came from — one sample set or several combined
  // — and, when it was assembled ad hoc rather than loaded from a
  // composite file, the one place to widen a manual's compass and to
  // save the combination. Nothing here commits as the user types: a
  // compass edit waits for "Set", a save path waits for "Save", so a
  // rebuild from the next poll never tears out a field mid-edit — it
  // only ever needs to run when the setup itself has actually changed.

  wireOrgan() {
    this.el.organSaveBtn.addEventListener("click", () => this.saveOrgan());
    this.el.organSavePath.addEventListener("keydown", (event) => {
      if (event.key === "Enter") {
        event.preventDefault();
        this.saveOrgan();
      }
    });
  }

  refreshOrgan(snapshot) {
    const setup = snapshot.setup ?? null;
    const manuals = snapshot.manuals ?? [];
    const signature = JSON.stringify([
      snapshot.organ,
      setup?.implicit ?? false,
      setup?.file ?? null,
      setup?.sources ?? [],
      (setup?.compass ?? []).map((c) => [
        c.idx, c.low, c.high, c.native_low, c.native_high, c.declared,
      ]),
      manuals.map((m) => [m.idx, m.name]),
      (snapshot.stops ?? []).map((s) => [s.id, s.name, s.midx]),
      (snapshot.couplers ?? []).map((c) => [c.idx, c.name, !!c.hidden]),
    ]);
    if (signature === this.organSignature) return;
    this.organSignature = signature;

    this.el.organImplicitNote.classList.toggle("hidden", !setup?.implicit);
    this.buildOrganSummary(snapshot, setup);
    this.buildOrganSave(setup);
    this.buildOrganCompass(setup, manuals);
    this.buildOrganStops(snapshot, manuals);
    this.buildOrganCouplers(snapshot);
  }

  buildOrganSummary(snapshot, setup) {
    this.el.organSummary.replaceChildren();

    const title = document.createElement("div");
    title.className = "organ-name-line";
    title.textContent = snapshot.organ ?? "Untitled organ";
    this.el.organSummary.append(title);

    const sources = setup?.sources ?? [];
    if (sources.length) {
      const list = document.createElement("div");
      list.className = "organ-sources";
      for (const source of sources) {
        const row = document.createElement("div");
        row.className = "organ-source";
        const name = document.createElement("span");
        name.className = "organ-source-name";
        name.textContent = source.name;
        const path = document.createElement("span");
        path.className = "organ-source-path";
        path.textContent = source.path;
        path.title = source.path;
        row.append(name, path);
        list.append(row);
      }
      this.el.organSummary.append(list);
    }

    if (setup?.file) {
      const file = document.createElement("div");
      file.className = "organ-file-line";
      file.textContent = `Lives in ${setup.file}`;
      file.title = setup.file;
      this.el.organSummary.append(file);
    }
  }

  /// The save row only makes sense for an organ with a setup that isn't
  /// already backed by a file — an ordinary single-set load has nothing
  /// to save.
  buildOrganSave(setup) {
    const needsSave = !!setup && !setup.file;
    this.el.organSave.classList.toggle("hidden", !needsSave);
    if (!needsSave) return;
    this.el.organSavePath.value = "";
    this.hideSaveError();
  }

  buildOrganCompass(setup, manuals) {
    this.el.organCompass.replaceChildren();
    const compassByIdx = new Map((setup?.compass ?? []).map((c) => [c.idx, c]));
    if (!compassByIdx.size) {
      this.el.organCompass.append(this.emptyNote("No compass information for this organ."));
      return;
    }
    for (const manual of manuals) {
      const compass = compassByIdx.get(manual.idx);
      if (compass) this.el.organCompass.append(this.organCompassRow(manual, compass));
    }
  }

  /// One manual's compass: two editable bounds and the two ways to change
  /// them — type new values and press Set, or fall back to whatever the
  /// sample set itself declares.
  organCompassRow(manual, compass) {
    const row = document.createElement("div");
    row.className = "organ-compass-row";

    const name = document.createElement("span");
    name.className = "manual-name";
    name.textContent = manual.name;
    name.title = manual.name;
    row.append(name);

    const low = this.compassField(compass.low ?? compass.native_low, compass.native_low);
    const high = this.compassField(compass.high ?? compass.native_high, compass.native_high);
    row.append(low.wrap, high.wrap);

    const set = document.createElement("button");
    set.className = "ghost";
    set.textContent = "Set";
    set.title = "Declare this manual's compass";
    set.addEventListener("click", () => {
      const lo = clampNote(low.input.value);
      const hi = clampNote(high.input.value);
      low.input.value = lo;
      high.input.value = hi;
      this.send(commands.organCompass(manual.idx, lo, hi));
    });
    row.append(set);

    if (compass.declared) {
      const native = document.createElement("button");
      native.className = "ghost";
      native.textContent = "Native";
      native.title = "Go back to the sample set's own compass";
      native.addEventListener("click", () => this.send(commands.organCompass(manual.idx)));
      row.append(native);
    }

    return row;
  }

  /// A number field paired with the note name it currently reads as —
  /// the same C4-is-60 naming as everywhere else in the dialog. Purely
  /// local until Set is pressed: typing here never sends anything.
  compassField(value, native) {
    const wrap = document.createElement("span");
    wrap.className = "compass-field";

    const input = document.createElement("input");
    input.type = "number";
    input.min = 0;
    input.max = 127;
    input.step = 1;
    input.value = value;
    input.placeholder = native;
    input.title = `Sample set's own: ${keyName(native)}`;

    const note = document.createElement("i");
    note.textContent = keyName(clampNote(value));
    input.addEventListener("input", () => {
      note.textContent = keyName(clampNote(input.value));
    });

    wrap.append(input, note);
    return { wrap, input };
  }

  // ---- organ stops --------------------------------------------------------
  //
  // Grouped by the manual whose division actually plays them (`midx`),
  // not by whichever jamb they were drawn on when their set loaded —
  // that's the whole point of being able to move one. A stop reporting
  // an out-of-range `midx` (a set whose manual didn't survive the
  // combination) is treated as unassigned rather than guessed at.

  buildOrganStops(snapshot, manuals) {
    this.el.organStops.replaceChildren();
    const stops = snapshot.stops ?? [];
    if (!stops.length) {
      this.el.organStops.append(this.emptyNote("No stops on this organ."));
      return;
    }
    const manualByIdx = new Map(manuals.map((m) => [m.idx, m]));
    const groups = new Map(); // manual idx (or null, unassigned) -> stops
    for (const stop of stops) {
      const manual = manualByIdx.get(stop.midx);
      const key = manual ? manual.idx : null;
      if (!groups.has(key)) groups.set(key, []);
      groups.get(key).push(stop);
    }
    for (const manual of manuals) {
      const group = groups.get(manual.idx);
      if (group) {
        this.el.organStops.append(this.organStopGroup(manual.name, group, manual.idx, manuals));
      }
    }
    const unassigned = groups.get(null);
    if (unassigned) {
      this.el.organStops.append(this.organStopGroup("Unassigned", unassigned, null, manuals));
    }
  }

  organStopGroup(title, stops, currentIdx, manuals) {
    const group = document.createElement("div");
    group.className = "organ-stop-group";
    const heading = document.createElement("h3");
    heading.className = "organ-stop-group-title";
    heading.textContent = title;
    group.append(heading);
    for (const stop of stops) group.append(this.organStopRow(stop, currentIdx, manuals));
    return group;
  }

  /// A stop's name plus a select of the *other* manuals — choosing one
  /// moves it there. Nothing to type, nothing to blur: picking an
  /// option closes the dropdown itself, so committing on `change` alone
  /// is enough to never fight a mid-poll rebuild.
  organStopRow(stop, currentIdx, manuals) {
    const row = document.createElement("div");
    row.className = "organ-stop-row";

    const name = document.createElement("span");
    name.className = "organ-stop-name";
    name.textContent = stop.name;
    name.title = stop.name;
    row.append(name);

    const move = document.createElement("select");
    move.className = "organ-stop-move";
    move.append(this.option("", "Move to…"));
    for (const manual of manuals) {
      if (manual.idx === currentIdx) continue;
      move.append(this.option(String(manual.idx), manual.name));
    }
    move.value = "";
    move.addEventListener("change", () => {
      if (move.value === "") return;
      this.send(commands.organMove(stop.id, Number(move.value)));
    });
    row.append(move);

    return row;
  }

  // ---- organ couplers -------------------------------------------------------
  //
  // The rail only ever shows the couplers currently on the console; this
  // is the one place every coupler the organ has, hidden ones included,
  // so one can be brought back after being taken off.

  buildOrganCouplers(snapshot) {
    this.el.organCouplers.replaceChildren();
    const couplers = snapshot.couplers ?? [];
    if (!couplers.length) {
      this.el.organCouplers.append(this.emptyNote("No couplers on this organ."));
      return;
    }
    for (const coupler of couplers) this.el.organCouplers.append(this.organCouplerRow(coupler));
  }

  organCouplerRow(coupler) {
    const row = document.createElement("label");
    row.className = "organ-coupler-row";

    const checkbox = document.createElement("input");
    checkbox.type = "checkbox";
    checkbox.checked = !coupler.hidden;
    checkbox.setAttribute("aria-label", `${coupler.name} on the console`);
    checkbox.addEventListener("change", () =>
      this.send(commands.organCoupler(coupler.idx, checkbox.checked))
    );
    row.append(checkbox);

    const name = document.createElement("span");
    name.className = "organ-coupler-name";
    name.textContent = coupler.name;
    row.append(name);

    return row;
  }

  /// Saving bypasses the usual send()/poll flow: every other command's
  /// error just needs to say "the organ is unreachable", but a bad path
  /// here has a specific, useful reason the server already wrote out,
  /// and this is the one place worth fetching directly to show it.
  async saveOrgan() {
    const path = this.el.organSavePath.value.trim();
    if (!path) {
      this.showSaveError("Give it a path first.");
      return;
    }
    this.el.organSaveBtn.disabled = true;
    try {
      const response = await fetch(this.base + commands.organSave(path), { method: "POST" });
      if (!response.ok) {
        this.showSaveError((await response.text()) || `${response.status} ${response.statusText}`);
        return;
      }
      // The next poll (at most POLL_INTERVAL_MS away) picks up the
      // now-saved organ and rebuilds this pane without the save row.
      this.hideSaveError();
    } catch (err) {
      this.showSaveError(String(err));
    } finally {
      this.el.organSaveBtn.disabled = false;
    }
  }

  showSaveError(text) {
    this.el.organSaveError.textContent = text;
    this.el.organSaveError.classList.remove("hidden");
  }

  hideSaveError() {
    this.el.organSaveError.classList.add("hidden");
    this.el.organSaveError.textContent = "";
  }
}
