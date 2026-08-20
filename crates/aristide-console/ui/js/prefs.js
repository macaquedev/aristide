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
export function actionLabel(action) {
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
export function keyGlyph(code) {
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
    this.lastSnapshot = null;
    this.offerings = null; // last-fetched /api/organ/offerings, or null
    this.renamingManual = null; // manual idx whose header is a rename input
    this.drag = null; // the live ctrl-drag, if any — see startDrag()
    this.pendingRemove = null; // {kind: "manual"|"enclosure", ...} awaiting confirm
    this.fabPedal = false; // which form the FAB's manual form is for
    this.fabBrowseDir = null;
    this.fabBrowseParent = null;
    this.fabBrowseEntries = null;
    this.fabBrowseError = null;
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
      organEnclosures: root.getElementById("organ-enclosures"),
      organCouplers: root.getElementById("organ-couplers"),
      organFloats: root.getElementById("organ-floats"),
      organLoadingStatus: root.getElementById("organ-loading-status"),
      organLoadingText: root.getElementById("organ-loading-text"),
      organEditError: root.getElementById("organ-edit-error"),
      organOfferingsHeading: root.getElementById("organ-offerings-heading"),
      organOfferingsNote: root.getElementById("organ-offerings-note"),
      organOfferings: root.getElementById("organ-offerings"),
      organBin: root.getElementById("organ-bin"),
      organRemoveConfirm: root.getElementById("organ-remove-confirm"),
      organRemoveConfirmText: root.getElementById("organ-remove-confirm-text"),
      organRemoveConfirmYes: root.getElementById("organ-remove-confirm-yes"),
      organRemoveConfirmNo: root.getElementById("organ-remove-confirm-no"),
      organFabDock: root.getElementById("organ-fab-dock"),
      organFab: root.getElementById("organ-fab"),
      organFabMenu: root.getElementById("organ-fab-menu"),
      organFabAddManual: root.getElementById("organ-fab-add-manual"),
      organFabAddPedal: root.getElementById("organ-fab-add-pedal"),
      organFabAddEnc: root.getElementById("organ-fab-add-enc"),
      organFabAddSource: root.getElementById("organ-fab-add-source"),
      organFabManualForm: root.getElementById("organ-fab-manual-form"),
      organFabManualName: root.getElementById("organ-fab-manual-name"),
      organFabManualLow: root.getElementById("organ-fab-manual-low"),
      organFabManualHigh: root.getElementById("organ-fab-manual-high"),
      organFabManualCancel: root.getElementById("organ-fab-manual-cancel"),
      organFabEncForm: root.getElementById("organ-fab-enc-form"),
      organFabEncName: root.getElementById("organ-fab-enc-name"),
      organFabEncCancel: root.getElementById("organ-fab-enc-cancel"),
      organFabSourceForm: root.getElementById("organ-fab-source-form"),
      organFabSourcePath: root.getElementById("organ-fab-source-path"),
      organFabSourceAdd: root.getElementById("organ-fab-source-add"),
      organFabSourceCancel: root.getElementById("organ-fab-source-cancel"),
      organFabBrowseUp: root.getElementById("organ-fab-browse-up"),
      organFabBrowseDir: root.getElementById("organ-fab-browse-dir"),
      organFabBrowseError: root.getElementById("organ-fab-browse-error"),
      organFabBrowseList: root.getElementById("organ-fab-browse-list"),
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
    // The FAB/bin/confirm float over the Organ pane specifically, but
    // live outside it (see organ-floats' own comment in index.html) so
    // its scroll can never carry them out of place.
    this.el.organFloats.classList.toggle("hidden", this.tab !== "organ");
    if (this.tab === "organ" && this.lastSnapshot?.setup?.file) this.fetchOfferings();
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
    this.el.organRemoveConfirmYes.addEventListener("click", () => {
      const target = this.pendingRemove;
      this.hideRemoveConfirm();
      if (!target) return;
      if (target.kind === "enclosure") this.organCommand(commands.organEnclosureRemove(target.name));
      else this.organCommand(commands.organManualRemove(target.idx));
    });
    this.el.organRemoveConfirmNo.addEventListener("click", () => this.hideRemoveConfirm());
    this.wireOrganFab();
  }

  refreshOrgan(snapshot) {
    this.lastSnapshot = snapshot;
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
      manuals.map((m) => [m.idx, m.name, !!m.pedal]),
      (snapshot.stops ?? []).map((s) => [s.id, s.name, s.midx, s.enc ?? []]),
      (snapshot.couplers ?? []).map((c) => [c.idx, c.name, !!c.hidden]),
      (snapshot.enclosures ?? []).map((e) => [e.idx, e.name]),
    ]);
    const structuralChange = signature !== this.organSignature;
    if (structuralChange) {
      this.organSignature = signature;

      this.el.organImplicitNote.classList.toggle("hidden", !setup?.implicit);
      this.buildOrganSummary(snapshot, setup);
      this.buildOrganSave(setup);
      this.buildOrganCompass(setup, manuals);
      this.buildOrganStops(snapshot, manuals);
      this.buildOrganEnclosures(snapshot);
      this.buildOrganCouplers(snapshot);
    }
    // These run every poll rather than only on a structural change: the
    // rebuild status and which sources are on offer both need to track
    // `loading` (which flips independently of the shape it's rebuilding
    // towards) and the offerings fetch (which reads a different endpoint
    // the signature above says nothing about).
    this.refreshOrganStatus(snapshot);
    this.updateOfferingsSection(setup, structuralChange);
  }

  /// The rebuild strip: a structural edit answers immediately but the
  /// organ it names doesn't swap in until a later poll finds `loading`
  /// gone. Shown only once something is actually loaded — a first load
  /// has the picker's own progress for that.
  refreshOrganStatus(snapshot) {
    const show = !!snapshot.organ && !!snapshot.loading;
    this.el.organLoadingStatus.classList.toggle("hidden", !show);
    this.el.organLoadingText.textContent = snapshot.loading ?? "";
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

  // ---- organ stops: editable manual groups ---------------------------------
  //
  // Grouped by the manual whose division actually plays them (`midx`),
  // not by whichever jamb they were drawn on when their set loaded —
  // that's the whole point of being able to move one. A stop reporting
  // an out-of-range `midx` (a set whose manual didn't survive the
  // combination) is treated as unassigned rather than guessed at.
  //
  // A manual's own header doubles as its drag handle (reorder, or drop
  // it on the bin to remove it) and its rename control; a stop row is
  // both a ctrl-drag source (move it, or bin it) and a drop target isn't
  // — it's the *group* underneath it that receives drops. Everything
  // here rebuilds only on a structural change, same discipline as the
  // rest of the pane; a rename or drag in progress is tracked apart from
  // the snapshot and threaded back through on the next rebuild.

  buildOrganStops(snapshot, manuals) {
    this.el.organStops.replaceChildren();
    this.el.organFab.classList.toggle("pulse", !manuals.length);
    if (!manuals.length) {
      const hint = document.createElement("p");
      hint.className = "organ-hint";
      hint.textContent =
        "This organ is empty — add a manual, then pull stops onto it from a sample set.";
      this.el.organStops.append(hint);
      return;
    }
    const stops = snapshot.stops ?? [];
    const manualByIdx = new Map(manuals.map((m) => [m.idx, m]));
    const enclosuresByIdx = new Map((snapshot.enclosures ?? []).map((e) => [e.idx, e.name]));
    const groups = new Map(); // manual idx (or null, unassigned) -> stops
    for (const stop of stops) {
      const manual = manualByIdx.get(stop.midx);
      const key = manual ? manual.idx : null;
      if (!groups.has(key)) groups.set(key, []);
      groups.get(key).push(stop);
    }
    for (const manual of manuals) {
      this.el.organStops.append(
        this.organManualGroup(manual, groups.get(manual.idx) ?? [], manuals, enclosuresByIdx)
      );
    }
    const unassigned = groups.get(null);
    if (unassigned) {
      this.el.organStops.append(
        this.organManualGroup(
          { idx: null, name: "Unassigned", pedal: false }, unassigned, manuals, enclosuresByIdx
        )
      );
    }
  }

  /// Rebuilds the manual groups from whatever snapshot is on hand —
  /// entering or leaving a rename, or reflecting a drag's result, is a
  /// local state change the poll knows nothing about.
  rerenderOrganManuals() {
    if (this.lastSnapshot) this.buildOrganStops(this.lastSnapshot, this.lastSnapshot.manuals ?? []);
  }

  startManualRename(idx) {
    this.renamingManual = idx;
    this.rerenderOrganManuals();
  }

  organManualGroup(manual, stops, manuals, enclosuresByIdx) {
    const real = manual.idx != null; // false only for the synthetic "Unassigned" group
    const group = document.createElement("div");
    group.className = "organ-manual-group";
    if (real) {
      group.dataset.dropManual = manual.idx;
      group.dataset.manualName = manual.name;
    }

    const header = document.createElement("div");
    header.className = "organ-manual-header";
    if (real) {
      header.addEventListener("pointerdown", (event) => {
        if (!event.ctrlKey || this.renamingManual === manual.idx) return;
        this.startDrag(
          event, "manual",
          { idx: manual.idx, name: manual.name, stopCount: stops.length },
          manual.name
        );
      });
      header.addEventListener("dblclick", () => this.startManualRename(manual.idx));
    }

    if (real && this.renamingManual === manual.idx) {
      header.append(this.manualRenameField(manual));
    } else {
      const title = document.createElement("h3");
      title.className = "organ-stop-group-title";
      title.textContent = manual.name;
      header.append(title);
      if (manual.pedal) {
        const tag = document.createElement("span");
        tag.className = "organ-manual-pedal-tag";
        tag.textContent = "pedal";
        header.append(tag);
      }
      if (real) {
        const rename = document.createElement("button");
        rename.type = "button";
        rename.className = "organ-manual-rename-btn";
        rename.textContent = "✎";
        rename.title = "Rename this manual";
        rename.setAttribute("aria-label", `Rename ${manual.name}`);
        rename.addEventListener("click", (event) => {
          event.stopPropagation();
          this.startManualRename(manual.idx);
        });
        header.append(rename);
      }
    }
    group.append(header);

    for (const stop of stops) group.append(this.organStopRow(stop, manual.idx, manuals, enclosuresByIdx));
    if (!stops.length) {
      const empty = document.createElement("p");
      empty.className = "pane-empty";
      empty.textContent = real
        ? "Nothing pulled onto this manual yet."
        : "No stops.";
      group.append(empty);
    }
    return group;
  }

  manualRenameField(manual) {
    const input = document.createElement("input");
    input.className = "organ-manual-rename-input";
    input.value = manual.name;
    input.setAttribute("aria-label", `Rename ${manual.name}`);
    const commit = () => {
      if (this.renamingManual !== manual.idx) return;
      this.renamingManual = null;
      const name = input.value.trim();
      if (name && name !== manual.name) {
        this.organCommand(commands.organManualRename(manual.idx, name));
      }
      this.rerenderOrganManuals();
    };
    input.addEventListener("keydown", (event) => {
      event.stopPropagation();
      if (event.key === "Enter") {
        event.preventDefault();
        commit();
      } else if (event.key === "Escape") {
        event.preventDefault();
        this.renamingManual = null;
        this.rerenderOrganManuals();
      }
    });
    input.addEventListener("blur", commit);
    requestAnimationFrame(() => {
      input.focus();
      input.select();
    });
    return input;
  }

  /// A stop's name, a ctrl-drag handle (move it, or bin it), plus a
  /// select of the *other* manuals as a no-drag fallback. Picking an
  /// option closes the dropdown itself, so committing on `change` alone
  /// is enough to never fight a mid-poll rebuild.
  organStopRow(stop, currentIdx, manuals, enclosuresByIdx) {
    const row = document.createElement("div");
    row.className = "organ-stop-row";
    row.addEventListener("pointerdown", (event) => {
      if (!event.ctrlKey) return;
      this.startDrag(event, "stop", { id: stop.id, midx: currentIdx, name: stop.name }, stop.name);
    });

    const name = document.createElement("span");
    name.className = "organ-stop-name";
    name.textContent = stop.name;
    name.title = stop.name;
    row.append(name);

    // A stop a swell box already encloses carries that box's name —
    // the first one, if more than one box somehow shares it.
    const enc = stop.enc ?? [];
    if (enc.length && enclosuresByIdx) {
      const boxNames = enc.map((idx) => enclosuresByIdx.get(idx)).filter(Boolean);
      if (boxNames.length) {
        const tag = document.createElement("span");
        tag.className = "organ-stop-enc-tag";
        tag.textContent = boxNames[0];
        tag.title = boxNames.length > 1 ? `In swell boxes: ${boxNames.join(", ")}` : `In the ${boxNames[0]} box`;
        row.append(tag);
      }
    }

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

  // ---- swell boxes ----------------------------------------------------------
  //
  // A box is a cross-cutting view over the same stops the manual groups
  // above already show — enclosing one doesn't move it off its manual,
  // it just adds it to a box's membership. So each box lists its member
  // stops as a second reference to the same stop ids, with its own
  // ctrl-drag (out of the box) and a plain × button as the no-drag
  // fallback (the manual groups' equivalent is the "Move to…" select).

  buildOrganEnclosures(snapshot) {
    this.el.organEnclosures.replaceChildren();
    const enclosures = snapshot.enclosures ?? [];
    if (!enclosures.length) {
      this.el.organEnclosures.append(this.emptyNote("No swell boxes on this organ."));
      return;
    }
    const stops = snapshot.stops ?? [];
    for (const enclosure of enclosures) {
      const members = stops.filter((s) => (s.enc ?? []).includes(enclosure.idx));
      this.el.organEnclosures.append(this.organEnclosureGroup(enclosure, members));
    }
  }

  organEnclosureGroup(enclosure, members) {
    const group = document.createElement("div");
    group.className = "organ-manual-group organ-enclosure-group";
    group.dataset.dropEnclosure = enclosure.name;

    const header = document.createElement("div");
    header.className = "organ-manual-header";
    header.addEventListener("pointerdown", (event) => {
      if (!event.ctrlKey) return;
      this.startDrag(
        event, "enclosure",
        { name: enclosure.name, stopCount: members.length },
        enclosure.name
      );
    });

    const glyph = document.createElement("span");
    glyph.className = "organ-enclosure-glyph";
    glyph.textContent = "◐";
    glyph.setAttribute("aria-hidden", "true");
    header.append(glyph);

    const title = document.createElement("h3");
    title.className = "organ-stop-group-title";
    title.textContent = enclosure.name;
    header.append(title);

    const tag = document.createElement("span");
    tag.className = "organ-manual-pedal-tag";
    tag.textContent = "enc";
    header.append(tag);

    group.append(header);

    for (const stop of members) group.append(this.organEnclosureMemberRow(enclosure, stop));
    if (!members.length) {
      const empty = document.createElement("p");
      empty.className = "pane-empty";
      empty.textContent = "No stops in this box yet — ctrl-drag one in from a manual above.";
      group.append(empty);
    }
    return group;
  }

  organEnclosureMemberRow(enclosure, stop) {
    const row = document.createElement("div");
    row.className = "organ-stop-row";
    row.addEventListener("pointerdown", (event) => {
      if (!event.ctrlKey) return;
      this.startDrag(
        event, "boxed-stop",
        { id: stop.id, enclosure: enclosure.name, name: stop.name },
        stop.name
      );
    });

    const name = document.createElement("span");
    name.className = "organ-stop-name";
    name.textContent = stop.name;
    name.title = stop.name;
    row.append(name);

    const takeOut = document.createElement("button");
    takeOut.type = "button";
    takeOut.className = "ghost remove-input";
    takeOut.textContent = "×";
    takeOut.title = `Take out of ${enclosure.name}`;
    takeOut.setAttribute("aria-label", `Take ${stop.name} out of the ${enclosure.name} box`);
    takeOut.addEventListener("click", () =>
      this.organCommand(commands.organEnclosureAssign(enclosure.name, stop.id, false))
    );
    row.append(takeOut);

    return row;
  }

  // ---- ctrl-drag: stops, manuals, and offerings onto a manual group -------
  //
  // Plain pointer events, not HTML5 drag-and-drop — the ctrl key gates
  // it (a plain drag has to keep scrolling the pane), a floating label
  // follows the pointer, and the drop target is read straight off
  // `elementFromPoint` rather than fired dragenter/dragover events.

  binAllowed(kind) {
    return kind === "stop" || kind === "manual" || kind === "enclosure";
  }

  /// A stop already sitting in a box, an unpulled offering, and a box's
  /// own header are none of them meaningful drops onto a manual group.
  manualAllowed(kind) {
    return kind !== "enclosure";
  }

  /// Only a plain organ stop can be dropped into a swell box — a box
  /// doesn't take manuals, offerings, or another box's member.
  encAllowed(kind) {
    return kind === "stop";
  }

  startDrag(event, kind, payload, label) {
    event.preventDefault();
    event.stopPropagation();
    const ghost = document.createElement("div");
    ghost.className = "organ-drag-ghost";
    ghost.textContent = label;
    this.root.body.append(ghost);
    this.drag = { kind, payload, ghost, targetType: null, targetIdx: null, targetName: null };
    this.positionGhost(event.clientX, event.clientY);
    this.el.organFabDock.classList.add("dragging");
    if (this.binAllowed(kind)) this.el.organBin.classList.add("visible");
    this._dragMove = (e) => this.dragMove(e);
    window.addEventListener("pointermove", this._dragMove);
    window.addEventListener("pointerup", (e) => this.endDrag(e), { once: true });
  }

  positionGhost(x, y) {
    if (!this.drag) return;
    this.drag.ghost.style.left = `${x}px`;
    this.drag.ghost.style.top = `${y}px`;
  }

  dragMove(event) {
    if (!this.drag) return;
    this.positionGhost(event.clientX, event.clientY);
    this.applyDropHighlight(this.findDropTarget(event.clientX, event.clientY));
  }

  findDropTarget(x, y) {
    const el = this.root.elementFromPoint(x, y);
    if (!el || !this.drag) return null;
    const bin = el.closest("[data-drop-bin]");
    if (bin && this.binAllowed(this.drag.kind)) return { type: "bin" };
    const enclosure = el.closest("[data-drop-enclosure]");
    if (enclosure && this.encAllowed(this.drag.kind)) {
      return { type: "enclosure", name: enclosure.dataset.dropEnclosure };
    }
    const group = el.closest("[data-drop-manual]");
    if (group && this.manualAllowed(this.drag.kind)) {
      return { type: "manual", idx: Number(group.dataset.dropManual), name: group.dataset.manualName };
    }
    return null;
  }

  applyDropHighlight(hit) {
    for (const el of this.root.querySelectorAll(".organ-manual-group.drop-target")) {
      el.classList.remove("drop-target");
    }
    this.el.organBin.classList.remove("drop-target");
    this.drag.targetType = hit?.type ?? null;
    this.drag.targetIdx = hit?.idx ?? null;
    this.drag.targetName = hit?.name ?? null;
    if (!hit) return;
    if (hit.type === "bin") {
      this.el.organBin.classList.add("drop-target");
      return;
    }
    if (hit.type === "enclosure") {
      this.root
        .querySelector(`.organ-enclosure-group[data-drop-enclosure="${CSS.escape(hit.name)}"]`)
        ?.classList.add("drop-target");
      return;
    }
    // Dropping a stop back on its own manual, or a manual header on its
    // own group, isn't a move — no need to light it up as one.
    if (this.drag.kind === "stop" && hit.idx === this.drag.payload.midx) return;
    if (this.drag.kind === "manual" && hit.idx === this.drag.payload.idx) return;
    this.root
      .querySelector(`.organ-manual-group[data-drop-manual="${hit.idx}"]`)
      ?.classList.add("drop-target");
  }

  endDrag(event) {
    window.removeEventListener("pointermove", this._dragMove);
    const drag = this.drag;
    this.drag = null;
    if (!drag) return;
    this.positionGhost(event.clientX, event.clientY);
    drag.ghost.remove();
    this.el.organFabDock.classList.remove("dragging");
    this.el.organBin.classList.remove("visible", "drop-target");
    for (const el of this.root.querySelectorAll(".organ-manual-group.drop-target")) {
      el.classList.remove("drop-target");
    }

    const { targetType, targetIdx, targetName } = drag;
    if (!targetType) return;

    if (drag.kind === "stop") {
      if (targetType === "bin") {
        this.organCommand(commands.organUnpull(drag.payload.id));
      } else if (targetType === "enclosure") {
        this.organCommand(commands.organEnclosureAssign(targetName, drag.payload.id, true));
      } else if (targetType === "manual" && targetIdx !== drag.payload.midx) {
        // A live reassignment, not a rebuild — optimistic, the next
        // poll reconciles it like any other control.
        this.send(commands.organMove(drag.payload.id, targetIdx));
      }
    } else if (drag.kind === "manual") {
      if (targetType === "bin") {
        this.showRemoveConfirm("manual", drag.payload);
      } else if (targetType === "manual" && targetIdx !== drag.payload.idx) {
        this.organCommand(commands.organManualOrder(drag.payload.idx, targetIdx));
      }
    } else if (drag.kind === "boxed-stop" && targetType === "manual") {
      // Dropped back onto a manual — any of them — takes it out of the
      // box it came from; which manual plays it doesn't change.
      this.organCommand(commands.organEnclosureAssign(drag.payload.enclosure, drag.payload.id, false));
    } else if (drag.kind === "enclosure" && targetType === "bin") {
      this.showRemoveConfirm("enclosure", drag.payload);
    } else if (drag.kind === "offering-stop" && targetType === "manual") {
      this.organCommand(
        commands.organPull(drag.payload.alias, drag.payload.manualName, targetName, drag.payload.stopName)
      );
    } else if (drag.kind === "offering-division" && targetType === "manual") {
      this.organCommand(commands.organPull(drag.payload.alias, drag.payload.manualName, targetName));
    }
  }

  /// `kind` is "manual" (payload: {idx, name, stopCount}) or "enclosure"
  /// (payload: {name, stopCount}) — the two things this pane lets you
  /// remove outright, both confirmed the same way.
  showRemoveConfirm(kind, payload) {
    this.pendingRemove = { kind, ...payload };
    const n = payload.stopCount;
    this.el.organRemoveConfirmText.textContent =
      kind === "enclosure"
        ? `Remove the ${payload.name} box? Its stops stay, unenclosed.`
        : `Remove ${payload.name} and its ${n} stop${n === 1 ? "" : "s"}? ` +
          "Sources still offer everything.";
    this.el.organRemoveConfirm.classList.remove("hidden");
  }

  hideRemoveConfirm() {
    this.pendingRemove = null;
    this.el.organRemoveConfirm.classList.add("hidden");
  }

  // ---- organ edits: a fetch of their own, not send()/poll ------------------
  //
  // Every other command's failure just means "the organ is unreachable";
  // these can 400 with a specific, useful reason (a duplicate name, a
  // load already running) that's worth showing exactly, so — like
  // saveOrgan — they bypass the optimistic send() and read the response
  // themselves. A structural edit also doesn't land immediately: the
  // server answers with a snapshot mid-rebuild and the real result
  // arrives over the ordinary poll once `loading` clears.

  async organCommand(query) {
    this.hideOrganEditError();
    try {
      const response = await fetch(this.base + query, { method: "POST" });
      if (!response.ok) {
        this.showOrganEditError((await response.text()) || `${response.status} ${response.statusText}`);
        return false;
      }
      return true;
    } catch (err) {
      this.showOrganEditError(String(err));
      return false;
    }
  }

  showOrganEditError(text) {
    this.el.organEditError.textContent = text;
    this.el.organEditError.classList.remove("hidden");
  }

  hideOrganEditError() {
    this.el.organEditError.classList.add("hidden");
    this.el.organEditError.textContent = "";
  }

  // ---- sources: what each one offers, and what's already pulled -----------

  updateOfferingsSection(setup, structuralChange) {
    const hasFile = !!setup?.file;
    this.el.organOfferingsHeading.classList.toggle("hidden", !hasFile);
    this.el.organOfferingsNote.classList.toggle("hidden", !hasFile);
    this.el.organOfferings.classList.toggle("hidden", !hasFile);
    if (!hasFile) {
      this.offerings = null;
      return;
    }
    if (structuralChange || this.offerings === null) this.fetchOfferings();
  }

  async fetchOfferings() {
    try {
      const response = await fetch(this.base + commands.organOfferings());
      this.offerings = response.ok ? ((await response.json()).sources ?? []) : null;
    } catch {
      this.offerings = null;
    }
    this.buildOfferings(this.offerings);
  }

  buildOfferings(sources) {
    const container = this.el.organOfferings;
    container.replaceChildren();
    if (sources == null) {
      container.append(this.emptyNote("Couldn't read this organ's sources."));
      return;
    }
    if (!sources.length) {
      container.append(this.emptyNote("No sources yet — add one with the + button below."));
      return;
    }
    for (const source of sources) container.append(this.offeringSourceRow(source));
  }

  offeringSourceRow(source) {
    const details = document.createElement("details");
    details.className = "organ-offerings-source";

    const summary = document.createElement("summary");
    const alias = document.createElement("span");
    alias.className = "organ-offerings-alias";
    alias.textContent = source.alias;
    const name = document.createElement("span");
    name.className = "organ-offerings-name";
    name.textContent = source.name ?? "(unreadable)";
    const path = document.createElement("span");
    path.className = "organ-offerings-path";
    path.textContent = source.path;
    path.title = source.path;
    summary.append(alias, name, path);
    details.append(summary);

    if (source.error) {
      const error = document.createElement("p");
      error.className = "organ-offerings-error";
      error.textContent = source.error;
      details.append(error);
      return details;
    }

    const body = document.createElement("div");
    body.className = "organ-offerings-body";
    for (const manual of source.manuals ?? []) body.append(this.offeringDivision(source.alias, manual));
    details.append(body);
    return details;
  }

  offeringDivision(alias, manual) {
    const div = document.createElement("div");
    div.className = "organ-offerings-division";

    const head = document.createElement("div");
    head.className = "organ-offerings-division-head";
    if (!manual.pulled) {
      head.addEventListener("pointerdown", (event) => {
        if (!event.ctrlKey) return;
        this.startDrag(
          event, "offering-division",
          { alias, manualName: manual.name },
          `${manual.name} (whole division)`
        );
      });
    }
    const title = document.createElement("span");
    title.className = "organ-stop-group-title";
    title.textContent = manual.name;
    head.append(title);
    if (manual.pedal) {
      const tag = document.createElement("span");
      tag.className = "organ-manual-pedal-tag";
      tag.textContent = "pedal";
      head.append(tag);
    }
    if (manual.pulled) {
      const tag = document.createElement("span");
      tag.className = "organ-manual-pedal-tag";
      tag.textContent = "pulled";
      head.append(tag);
    }
    div.append(head);

    for (const stop of manual.stops ?? []) div.append(this.offeringStop(alias, manual.name, stop));
    return div;
  }

  offeringStop(alias, manualName, stop) {
    const row = document.createElement("div");
    row.className = "organ-offerings-stop";
    row.classList.toggle("pulled", !!stop.pulled);
    if (!stop.pulled) {
      row.addEventListener("pointerdown", (event) => {
        if (!event.ctrlKey) return;
        this.startDrag(event, "offering-stop", { alias, manualName, stopName: stop.name }, stop.name);
      });
    }
    const check = document.createElement("span");
    check.className = "organ-offerings-stop-check";
    check.textContent = stop.pulled ? "✓" : "";
    const name = document.createElement("span");
    name.textContent = stop.name;
    row.append(check, name);
    return row;
  }

  // ---- the "+" FAB: add a manual, a pedalboard, or a sample set -----------

  wireOrganFab() {
    this.el.organFab.addEventListener("click", (event) => {
      event.stopPropagation();
      const opening = this.el.organFabMenu.classList.contains("hidden") &&
        this.el.organFabManualForm.classList.contains("hidden") &&
        this.el.organFabEncForm.classList.contains("hidden") &&
        this.el.organFabSourceForm.classList.contains("hidden");
      this.closeFabPanels();
      if (opening) this.el.organFabMenu.classList.remove("hidden");
    });
    for (const el of [
      this.el.organFabMenu, this.el.organFabManualForm, this.el.organFabEncForm, this.el.organFabSourceForm,
    ]) {
      el.addEventListener("click", (event) => event.stopPropagation());
    }
    window.addEventListener("click", () => this.closeFabPanels());

    this.el.organFabAddManual.addEventListener("click", () => this.openManualForm(false));
    this.el.organFabAddPedal.addEventListener("click", () => this.openManualForm(true));
    this.el.organFabAddEnc.addEventListener("click", () => this.openEncForm());
    this.el.organFabAddSource.addEventListener("click", () => this.openSourceForm());
    this.el.organFabManualCancel.addEventListener("click", () => this.closeFabPanels());
    this.el.organFabEncCancel.addEventListener("click", () => this.closeFabPanels());
    this.el.organFabSourceCancel.addEventListener("click", () => this.closeFabPanels());

    this.el.organFabManualForm.addEventListener("submit", (event) => {
      event.preventDefault();
      const name = this.el.organFabManualName.value.trim();
      if (!name) return;
      const low = clampNote(this.el.organFabManualLow.value);
      const high = clampNote(this.el.organFabManualHigh.value);
      this.organCommand(commands.organManualAdd(name, low, high, this.fabPedal ? 1 : 0)).then(
        (ok) => ok && this.closeFabPanels()
      );
    });

    this.el.organFabEncForm.addEventListener("submit", (event) => {
      event.preventDefault();
      const name = this.el.organFabEncName.value.trim();
      if (!name) return;
      this.organCommand(commands.organEnclosureAdd(name)).then((ok) => ok && this.closeFabPanels());
    });

    this.el.organFabSourceAdd.addEventListener("click", () => {
      const path = this.el.organFabSourcePath.value.trim();
      if (!path) return;
      this.organCommand(commands.organSourceAdd(path)).then((ok) => {
        if (ok) this.el.organFabSourcePath.value = "";
      });
    });
    this.el.organFabBrowseUp.addEventListener("click", () => {
      if (this.fabBrowseParent) this.fabBrowse(this.fabBrowseParent);
    });
  }

  closeFabPanels() {
    this.el.organFabMenu.classList.add("hidden");
    this.el.organFabManualForm.classList.add("hidden");
    this.el.organFabEncForm.classList.add("hidden");
    this.el.organFabSourceForm.classList.add("hidden");
  }

  openManualForm(pedal) {
    this.fabPedal = pedal;
    this.closeFabPanels();
    this.el.organFabManualForm.classList.remove("hidden");
    this.el.organFabManualName.value = "";
    this.el.organFabManualLow.value = 36;
    this.el.organFabManualHigh.value = pedal ? 67 : 96;
    requestAnimationFrame(() => this.el.organFabManualName.focus());
  }

  openEncForm() {
    this.closeFabPanels();
    this.el.organFabEncForm.classList.remove("hidden");
    this.el.organFabEncName.value = "";
    requestAnimationFrame(() => this.el.organFabEncName.focus());
  }

  openSourceForm() {
    this.closeFabPanels();
    this.el.organFabSourceForm.classList.remove("hidden");
    this.el.organFabSourcePath.value = "";
    this.fabBrowseDir = null;
    this.fabBrowseParent = null;
    this.fabBrowse();
  }

  /// This organ's own directory listing, the same idiom as the picker's
  /// Browse pane but scoped to this form: fetched directly, not
  /// snapshot-driven, and picking a file adds it as a source outright
  /// rather than loading it.
  async fabBrowse(dir) {
    try {
      const query = dir ? `/api/browse?dir=${encodeURIComponent(dir)}` : "/api/browse";
      const response = await fetch(this.base + query);
      if (!response.ok) {
        this.fabBrowseError = (await response.text()) || `${response.status} ${response.statusText}`;
        this.renderFabBrowse();
        return;
      }
      const data = await response.json();
      this.fabBrowseDir = data.dir;
      this.fabBrowseParent = data.parent;
      this.fabBrowseEntries = data.entries;
      this.fabBrowseError = null;
      this.renderFabBrowse();
    } catch (err) {
      this.fabBrowseError = String(err);
      this.renderFabBrowse();
    }
  }

  renderFabBrowse() {
    this.el.organFabBrowseDir.textContent = this.fabBrowseDir ?? "";
    this.el.organFabBrowseDir.title = this.fabBrowseDir ?? "";
    this.el.organFabBrowseUp.disabled = !this.fabBrowseParent;
    this.el.organFabBrowseError.classList.toggle("hidden", !this.fabBrowseError);
    this.el.organFabBrowseError.textContent = this.fabBrowseError ?? "";
    this.el.organFabBrowseList.replaceChildren();
    if (this.fabBrowseError) return;
    const entries = this.fabBrowseEntries ?? [];
    if (!entries.length) {
      this.el.organFabBrowseList.append(this.emptyNote("Nothing here."));
      return;
    }
    for (const entry of entries) {
      const row = document.createElement("button");
      row.type = "button";
      row.className = entry.dir ? "picker-row picker-browse-dir" : "picker-row";
      row.title = entry.path;
      row.addEventListener("click", () => {
        if (entry.dir) {
          this.fabBrowse(entry.path);
        } else {
          this.el.organFabSourcePath.value = entry.path;
          this.organCommand(commands.organSourceAdd(entry.path));
        }
      });
      const name = document.createElement("span");
      name.className = "picker-row-name";
      name.textContent = entry.name;
      row.append(name);
      this.el.organFabBrowseList.append(row);
    }
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
