// The preferences dialog: everything about the instrument that isn't
// played — which keyboard drives which manual, temperament, room and
// mechanism noises, and the skin.
//
// Like the console proper it mirrors the server snapshot rather than
// holding its own copy: rows are rebuilt only when the shape changes
// (a device appears, a different organ loads), and values are written
// back on every poll except into a control the user is touching.

import { commands } from "./api.js";

const TABS = ["midi", "tuning", "sound", "appearance"];

export class Preferences {
  constructor(root, send) {
    this.root = root;
    this.send = send;
    this.dragging = new Set();
    this.tuning = null;
    this.midiSignature = null;
    this.manuals = [];
    this.tab = "midi";
    this.el = {
      modal: root.getElementById("prefs"),
      subject: root.getElementById("prefs-subject"),
      tabs: root.getElementById("prefs-tabs"),
      panes: [...root.querySelectorAll("#prefs .pane")],
      ports: root.getElementById("midi-ports"),
      channels: root.getElementById("midi-channels"),
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
    this.manuals = snapshot.manuals ?? [];
    this.refreshMidi(snapshot);
    this.refreshTuning(snapshot.tuning);
    this.refreshSound(snapshot);
  }

  // ---- MIDI --------------------------------------------------------------

  refreshMidi(snapshot) {
    const midi = snapshot.midi ?? { ports: [], channels: [] };
    const signature = JSON.stringify([
      midi.ports.map((p) => [p.id, p.name]),
      this.manuals.map((m) => [m.idx, m.name]),
      midi.channels.length,
    ]);
    if (signature !== this.midiSignature) {
      this.midiSignature = signature;
      this.buildPorts(midi.ports);
      this.buildChannels(midi.channels.length);
    }

    for (const port of midi.ports) {
      const row = this.el.ports.querySelector(`[data-port="${port.id}"]`);
      if (!row) continue;
      row.classList.toggle("muted", !port.enabled);
      row.querySelector(".port-enabled").checked = port.enabled;
      const route = row.querySelector(".port-route");
      if (this.root.activeElement !== route) route.value = String(port.route);
    }

    midi.channels.forEach((manual, channel) => {
      const select = this.el.channels.querySelector(`[data-channel="${channel}"]`);
      if (select && this.root.activeElement !== select) select.value = String(manual);
    });
  }

  buildPorts(ports) {
    this.el.ports.replaceChildren();
    if (!ports.length) {
      const empty = document.createElement("p");
      empty.className = "pane-empty";
      empty.textContent =
        "No MIDI inputs. Plug the console in — the list finds it by itself.";
      this.el.ports.append(empty);
      return;
    }
    for (const port of ports) {
      const row = document.createElement("div");
      row.className = "midi-port";
      row.dataset.port = port.id;

      const listen = document.createElement("input");
      listen.type = "checkbox";
      listen.className = "port-enabled";
      listen.title = "Listen to this device";
      listen.addEventListener("change", () =>
        this.send(commands.midiPort(port.id, { enabled: listen.checked ? 1 : 0 }))
      );

      const name = document.createElement("span");
      name.className = "port-name";
      name.textContent = port.name;
      name.title = port.name;

      const route = document.createElement("select");
      route.className = "port-route";
      const auto = document.createElement("option");
      auto.value = "-1";
      auto.textContent = "follow channel map";
      route.append(auto);
      for (const manual of this.manuals) {
        const option = document.createElement("option");
        option.value = String(manual.idx);
        option.textContent = manual.name;
        route.append(option);
      }
      route.addEventListener("change", () => {
        this.send(commands.midiPort(port.id, { route: route.value }));
        route.blur();
      });

      row.append(listen, name, route);
      this.el.ports.append(row);
    }
  }

  buildChannels(count) {
    this.el.channels.replaceChildren();
    for (let channel = 0; channel < count; channel++) {
      const cell = document.createElement("label");
      cell.className = "midi-channel";

      const label = document.createElement("span");
      label.className = "rail-label";
      label.textContent = `CH ${channel + 1}`;

      const select = document.createElement("select");
      select.dataset.channel = channel;
      for (const manual of this.manuals) {
        const option = document.createElement("option");
        option.value = String(manual.idx);
        option.textContent = manual.name;
        select.append(option);
      }
      select.addEventListener("change", () => {
        this.send(commands.midiChannel(channel, select.value));
        select.blur();
      });

      cell.append(label, select);
      this.el.channels.append(cell);
    }
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
