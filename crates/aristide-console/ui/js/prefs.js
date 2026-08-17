// The preferences dialog: everything about the instrument that isn't
// played — which keyboard drives which manual, temperament, room and
// mechanism noises, and the skin.
//
// Like the console proper it mirrors the server snapshot rather than
// holding its own copy: rows are rebuilt only when the shape changes
// (a device appears, a different organ loads), and values are written
// back on every poll except into a control the user is touching.

import { commands } from "./api.js";

const NOTE_NAMES = ["C", "C♯", "D", "E♭", "E", "F", "F♯", "G", "A♭", "A", "B♭", "B"];

/// MIDI note number in the naming organists read on a stoplist: middle
/// C (60) is C4, as every sample set's documentation writes it.
function keyName(key) {
  return `${NOTE_NAMES[key % 12]}${Math.floor(key / 12) - 1}`;
}

const TABS = ["midi", "tuning", "sound", "appearance"];

export class Preferences {
  constructor(root, send) {
    this.root = root;
    this.send = send;
    this.dragging = new Set();
    this.tuning = null;
    this.midiSignature = null;
    this.learning = null;
    this.tab = "midi";
    this.el = {
      modal: root.getElementById("prefs"),
      subject: root.getElementById("prefs-subject"),
      tabs: root.getElementById("prefs-tabs"),
      panes: [...root.querySelectorAll("#prefs .pane")],
      manuals: root.getElementById("midi-manuals"),
      ports: root.getElementById("midi-ports"),
      unassigned: root.getElementById("midi-unassigned"),
      rescan: root.getElementById("midi-rescan"),
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
    // Esc closes whichever dialog is up; the console keeps its keys
    // otherwise (keys.js stays quiet while `modal-open` is set).
    window.addEventListener("keydown", (event) => {
      if (event.key === "Escape" && this.root.body.classList.contains("modal-open")) {
        event.preventDefault();
        this.close();
      }
    });

    this.el.rescan.addEventListener("click", () => this.send(commands.midiRescan()));
    this.wireTuning();
    this.wireSound();
  }

  // ---- snapshot ----------------------------------------------------------

  update(snapshot) {
    this.el.subject.textContent = snapshot.organ ?? "";
    this.refreshMidi(snapshot);
    this.refreshTuning(snapshot.tuning);
    this.refreshSound(snapshot);
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
    channel.disabled = !input;
    channel.addEventListener("change", () =>
      this.send(commands.midiBind(manual, slot, input.device, channel.value))
    );

    const listen = document.createElement("button");
    listen.className = "ghost listen";
    listen.textContent = listening ? "Cancel" : "Listen";
    listen.title = "Assign by playing a key on the keyboard you mean";
    listen.addEventListener("click", () =>
      this.send(listening ? commands.midiLearn(null) : commands.midiLearn(manual, slot))
    );

    row.append(device, channel, listen);
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

  /// What this keyboard's compass costs the organ: the keys it reaches
  /// past the set's own compass are the repitched ones, and that is
  /// worth saying out loud rather than leaving to be discovered.
  compassNote(midi, manualEntry, input) {
    const note = document.createElement("span");
    note.className = "input-compass";
    const native = manualEntry.native;
    const learned = input.low != null && input.high != null;
    if (!learned) {
      note.textContent = native
        ? `${keyName(native[0])}–${keyName(native[1])} · the set's own compass`
        : "";
      note.classList.add("dim");
      return note;
    }
    const [low, high] = [Math.min(input.low, input.high), Math.max(input.low, input.high)];
    let text = `${keyName(low)}–${keyName(high)}`;
    if (native) {
      const filled = Math.max(0, native[0] - low) + Math.max(0, high - native[1]);
      text += filled
        ? ` · ${filled} key${filled === 1 ? "" : "s"} repitched past the set's ${keyName(
            native[0]
          )}–${keyName(native[1])}`
        : " · within the set's compass";
    }
    note.textContent = text;
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

  // ---- tuning ------------------------------------------------------------

  wireTuning() {
    this.el.temperament.addEventListener("change", () => {
      this.send(commands.tuning({ temperament: this.el.temperament.value }));
      this.el.temperament.blur(); // hand the field back to the snapshot
    });

    this.el.a4.addEventListener("change", () => {
      const a4 = Math.min(500, Math.max(300, Number(this.el.a4.value) || 440));
      this.el.a4.value = a4;
      this.send(commands.tuning({ a4 }));
      this.el.a4.blur();
    });

    for (const [button, step] of [
      [this.el.transposeDown, -1],
      [this.el.transposeUp, +1],
    ]) {
      button.addEventListener("click", () => {
        const at = this.tuning?.transpose ?? 0;
        const transpose = Math.min(12, Math.max(-12, at + step));
        if (transpose === at) return;
        // Optimistic, so rapid clicks step from the value just sent
        // rather than the last poll.
        if (this.tuning) this.tuning = { ...this.tuning, transpose };
        this.el.transposeValue.textContent =
          transpose > 0 ? `+${transpose}` : `${transpose}`;
        this.send(commands.tuning({ transpose }));
      });
    }
  }

  /// Mirrors the snapshot except into inputs the user is touching right
  /// now: a focused field or a mid-drag slider keeps its local value
  /// until the pointer lets go.
  refreshTuning(tuning) {
    this.tuning = tuning ?? null;
    for (const row of [this.el.temperamentRow, this.el.pitchRow, this.el.transposeRow]) {
      row.classList.toggle("hidden", !tuning);
    }
    if (!tuning) return;
    if (this.root.activeElement !== this.el.temperament) {
      this.el.temperament.value = tuning.temperament;
    }
    if (this.root.activeElement !== this.el.a4) this.el.a4.value = tuning.a4;
    this.el.transposeValue.textContent =
      tuning.transpose > 0 ? `+${tuning.transpose}` : `${tuning.transpose}`;
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
}
