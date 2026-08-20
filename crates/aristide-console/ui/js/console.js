// The console renderer: builds the DOM once per organ, then updates
// classes and values on every snapshot. Structure (which stops exist,
// keyboard geometry) changes only when a different organ loads, so the
// two paths are kept separate: `build` for structure, `refresh` for
// state. Interaction is optimistic — controls flip visually on click
// and the next snapshot reconciles.

import { commands } from "./api.js";

const SHARPS = new Set([1, 3, 6, 8, 10]);
const isSharp = (midi) => SHARPS.has(midi % 12);

/// Naturals strictly before `midi`, counting from `first` — the x
/// position of a key is derived from this.
function naturalsBefore(first, midi) {
  let n = 0;
  for (let k = first; k < midi; k++) if (!isSharp(k)) n++;
  return n;
}

/// "Montre 8'" -> ["Montre", "8'"]; footage/rank tails go on their own
/// line so the knob face reads like an engraved label.
function splitLabel(name) {
  const at = name.lastIndexOf(" ");
  if (at > 0 && /^[0-9IVX]/.test(name.slice(at + 1))) {
    return [name.slice(0, at), name.slice(at + 1)];
  }
  return [name, ""];
}

export class Console {
  /// `openPreferences(tab)` is how the console reaches the settings that
  /// no longer live on the bar itself — the tuning readout is a button
  /// onto its own preferences tab.
  constructor(root, send, openPreferences) {
    this.root = root;
    this.send = send;
    this.openPreferences = openPreferences;
    this.signature = null;
    this.dragging = new Set(); // control ids the pointer currently owns
    this.el = {
      offline: root.getElementById("offline"),
      organName: root.getElementById("organ-name"),
      gain: root.getElementById("gain"),
      tuning: root.getElementById("tuning"),
      panic: root.getElementById("panic"),
      jambLeft: root.getElementById("jamb-left"),
      jambRight: root.getElementById("jamb-right"),
      center: root.getElementById("console-center"),
      couplers: root.getElementById("couplers"),
      manuals: root.getElementById("manuals"),
      pedals: root.getElementById("pedals"),
      emptyCard: root.getElementById("organ-empty-card"),
    };
    this.wireRail();
  }

  offline(message) {
    this.el.offline.textContent = `no connection to the organ — ${message}`;
    this.el.offline.classList.remove("hidden");
  }

  render(snapshot) {
    this.el.offline.classList.add("hidden");
    // With no organ there is no console to show — otherwise the lone
    // tremulant knob haunts the empty background behind the loader.
    this.root.body.classList.toggle("no-organ", !snapshot.organ);
    const signature = JSON.stringify([
      snapshot.organ,
      snapshot.stops.map((s) => [s.id, s.name, s.manual]),
      snapshot.couplers.map((c) => [c.name, !!c.hidden]),
      snapshot.manuals.map((m) => [m.name, m.first_key, m.key_count]),
      // No organ (the picker's start-empty state) sends no enclosures
      // at all — same as an organ without a swell box.
      (snapshot.enclosures ?? []).filter((e) => e.displayed).map((e) => e.name),
      snapshot.reverb != null,
    ]);
    if (signature !== this.signature) {
      this.signature = signature;
      this.build(snapshot);
    }
    this.refresh(snapshot);
  }

  // ---- structure ----------------------------------------------------

  build(snapshot) {
    this.el.organName.textContent = snapshot.organ ?? "Aristide";
    // A loaded organ with nothing built yet has no jambs and no
    // keyboards worth drawing — a lone Tremblant knob and a Cancel
    // rocker would just haunt an otherwise bare case. Point at the
    // editor instead.
    const empty = !!snapshot.organ && !snapshot.stops.length && !snapshot.manuals.length;
    this.el.jambLeft.classList.toggle("hidden", empty);
    this.el.jambRight.classList.toggle("hidden", empty);
    this.el.center.classList.toggle("hidden", empty);
    this.el.emptyCard.classList.toggle("hidden", !empty);
    if (empty) {
      this.buildEmptyCard(snapshot);
      return;
    }
    this.buildJambs(snapshot);
    this.buildCouplers(snapshot);
    this.buildKeyboards(snapshot);
    this.fitLabels();
  }

  buildEmptyCard(snapshot) {
    this.el.emptyCard.replaceChildren();
    const title = document.createElement("h2");
    title.textContent = snapshot.organ ?? "Untitled organ";
    const note = document.createElement("p");
    note.textContent = "An empty organ — build it in Preferences → Organ.";
    const open = document.createElement("button");
    open.type = "button";
    open.textContent = "Open Preferences";
    open.addEventListener("click", () => this.openPreferences("organ"));
    this.el.emptyCard.append(title, note, open);
  }

  /// Long stop names ("Trompette", "Tremblant") must never break
  /// mid-word; instead the engraving shrinks until the widest word fits
  /// on the knob face. Measured, not guessed, so it holds under any font.
  fitLabels() {
    for (const label of this.root.querySelectorAll(".stop-name")) {
      label.style.fontSize = "";
      for (let size = 10.5; label.scrollWidth > label.clientWidth && size > 7.5; ) {
        size -= 0.5;
        label.style.fontSize = `${size}px`;
      }
    }
  }

  buildJambs(snapshot) {
    // Group stops by division, preserving server order of divisions.
    const divisions = new Map();
    for (const stop of snapshot.stops) {
      if (!divisions.has(stop.manual)) divisions.set(stop.manual, []);
      divisions.get(stop.manual).push(stop);
    }
    const names = [...divisions.keys()];
    const split = Math.ceil(names.length / 2);

    for (const [jamb, its] of [
      [this.el.jambLeft, names.slice(0, split)],
      [this.el.jambRight, names.slice(split)],
    ]) {
      jamb.replaceChildren();
      for (const name of its) {
        const column = document.createElement("div");
        column.className = "division";
        const title = document.createElement("h2");
        title.textContent = name;
        column.append(title);
        for (const stop of divisions.get(name)) {
          column.append(this.drawknob(stop.name, `stop-${stop.id}`, (on) =>
            this.send(commands.stop(stop.id, on))
          ));
        }
        jamb.append(column);
      }
    }

    // The tremulant behaves like a stop; it joins the bottom of the last
    // division column so the jamb reads as one rank of knobs.
    const jamb = this.el.jambRight.childElementCount
      ? this.el.jambRight
      : this.el.jambLeft;
    let column = jamb.lastElementChild;
    if (!column) {
      column = document.createElement("div");
      column.className = "division";
      jamb.append(column);
    }
    column.append(this.drawknob("Tremblant", "trem", (on) =>
      this.send(commands.tremulant(on))
    ));
  }

  drawknob(name, key, flip) {
    const knob = document.createElement("button");
    knob.className = "knob";
    knob.dataset.key = key;
    const [line, foot] = splitLabel(name);
    const face = document.createElement("span");
    face.className = "face";
    const label = document.createElement("span");
    label.className = "stop-name";
    label.textContent = line;
    face.append(label);
    if (foot) {
      const pitch = document.createElement("span");
      pitch.className = "stop-pitch";
      pitch.textContent = foot;
      face.append(pitch);
    }
    knob.append(face);
    knob.addEventListener("click", () => {
      const on = !knob.classList.contains("on");
      knob.classList.toggle("on", on); // optimistic
      flip(on);
    });
    return knob;
  }

  buildCouplers(snapshot) {
    this.el.couplers.replaceChildren();
    // A coupler taken off the console (see the Organ tab) is disengaged,
    // not deleted — it simply doesn't get a tablet on the rail.
    for (const coupler of snapshot.couplers.filter((c) => !c.hidden)) {
      const rocker = document.createElement("button");
      rocker.className = "rocker";
      rocker.dataset.key = `coupler-${coupler.idx}`;
      // Inner face so the ivory tablet can tilt inside the button's slot.
      const face = document.createElement("span");
      face.className = "tab";
      face.textContent = coupler.name;
      rocker.append(face);
      rocker.addEventListener("click", () => {
        const on = !rocker.classList.contains("on");
        rocker.classList.toggle("on", on);
        this.send(commands.coupler(coupler.idx, on));
      });
      this.el.couplers.append(rocker);
    }
    this.el.couplers.append(this.cancelPiston());
  }

  /// General cancel: pushes in every stop and releases every coupler.
  /// Momentary — it never lights, so it carries no `on` state; the
  /// tremulant is a separate control and survives it.
  cancelPiston() {
    const piston = document.createElement("button");
    piston.className = "rocker cancel";
    piston.dataset.key = "cancel";
    const face = document.createElement("span");
    face.className = "tab";
    face.textContent = "Cancel";
    piston.append(face);
    piston.addEventListener("click", () => this.cancel());
    return piston;
  }

  cancel() {
    for (const control of this.root.querySelectorAll(
      '.knob.on:not([data-key="trem"]), .rocker.on'
    )) {
      control.classList.remove("on"); // optimistic
    }
    this.send(commands.cancel());
  }

  panic() {
    this.send(commands.panic());
  }

  buildKeyboards(snapshot) {
    // The pedalboard renders at the bottom; manuals stack above it in
    // reverse server order, so the highest manual sits on top, as on a
    // real console. The model says which manual is the pedal; the name
    // sniff only covers organs loaded before it did.
    const pedal = snapshot.manuals.find((m) => m.pedal)
      ?? snapshot.manuals.find((m) => /p[ée]d/i.test(m.name))
      ?? snapshot.manuals[0];
    const manuals = snapshot.manuals.filter((m) => m !== pedal);

    this.el.manuals.replaceChildren();
    for (const manual of [...manuals].reverse()) {
      this.el.manuals.append(this.keyboard(manual, "manual"));
    }

    this.el.pedals.replaceChildren();
    if (pedal) {
      this.el.pedals.append(this.keyboard(pedal, "pedal"));
      this.el.pedals.append(this.shoes(snapshot));
    }
  }

  keyboard(manual, kind) {
    const board = document.createElement("div");
    board.className = `keyboard ${kind}`;
    board.dataset.manual = manual.idx;

    const cheek = document.createElement("span");
    cheek.className = "cheek";
    cheek.textContent = manual.name;
    board.append(cheek);

    const keys = document.createElement("div");
    keys.className = "keys";
    const last = manual.first_key + manual.key_count;
    let naturals = 0;
    for (let midi = manual.first_key; midi < last; midi++) if (!isSharp(midi)) naturals++;
    keys.style.setProperty("--naturals", naturals);

    for (let midi = manual.first_key; midi < last; midi++) {
      const key = document.createElement("div");
      const sharp = isSharp(midi);
      key.className = sharp ? "key sharp" : "key natural";
      key.dataset.midi = midi;
      const n = naturalsBefore(manual.first_key, midi);
      key.style.setProperty("--n", sharp ? n - 1 : n);
      this.wireKey(key, manual.idx, midi);
      keys.append(key);
    }
    board.append(keys);
    return board;
  }

  wireKey(key, manual, midi) {
    const off = () => {
      if (!key.classList.contains("pressed")) return;
      key.classList.remove("pressed", "held");
      this.send(commands.note(manual, midi, false));
    };
    key.addEventListener("pointerdown", (event) => {
      event.preventDefault();
      key.classList.add("pressed", "held"); // optimistic
      this.send(commands.note(manual, midi, true));
      window.addEventListener("pointerup", off, { once: true });
    });
    key.addEventListener("pointerleave", off);
  }

  shoes(snapshot) {
    const rack = document.createElement("div");
    rack.className = "shoes";
    for (const enclosure of (snapshot.enclosures ?? []).filter((e) => e.displayed)) {
      const shoe = document.createElement("div");
      shoe.className = "shoe";
      shoe.dataset.enclosure = enclosure.idx;

      const track = document.createElement("div");
      track.className = "shoe-track";
      const fill = document.createElement("div");
      fill.className = "shoe-fill";
      const thumb = document.createElement("div");
      thumb.className = "shoe-thumb";
      track.append(fill, thumb);

      const label = document.createElement("span");
      label.className = "shoe-label";
      label.textContent = enclosure.name;

      shoe.append(track, label);
      this.wireShoe(track, enclosure.idx);
      rack.append(shoe);
    }
    return rack;
  }

  wireShoe(track, idx) {
    const key = `enclosure-${idx}`;
    let lastSent = 0;
    const set = (event) => {
      const rect = track.getBoundingClientRect();
      const value = Math.min(1, Math.max(0, (rect.bottom - event.clientY) / rect.height));
      this.setShoe(track.parentElement, value);
      // ~30 commands/s is plenty; the pointerup below sends the final value.
      const now = performance.now();
      if (now - lastSent > 33) {
        lastSent = now;
        this.send(commands.enclosure(idx, value.toFixed(3)));
      }
      return value;
    };
    track.addEventListener("pointerdown", (event) => {
      event.preventDefault();
      this.dragging.add(key);
      track.setPointerCapture(event.pointerId);
      set(event);
    });
    track.addEventListener("pointermove", (event) => {
      if (this.dragging.has(key)) set(event);
    });
    track.addEventListener("pointerup", (event) => {
      this.dragging.delete(key);
      this.send(commands.enclosure(idx, set(event).toFixed(3)));
    });
  }

  setShoe(shoe, value) {
    shoe.style.setProperty("--open", value);
  }

  // ---- state --------------------------------------------------------

  refresh(snapshot) {
    for (const stop of snapshot.stops) this.setToggle(`stop-${stop.id}`, stop.on);
    for (const coupler of snapshot.couplers) {
      this.setToggle(`coupler-${coupler.idx}`, coupler.on);
    }
    this.setToggle("trem", snapshot.tremulant);

    for (const manual of snapshot.manuals) {
      const board = this.root.querySelector(`.keyboard[data-manual="${manual.idx}"]`);
      if (!board) continue;
      const held = new Set(manual.held);
      for (const key of board.querySelectorAll(".key")) {
        // Keys the local pointer is holding stay lit regardless.
        const midi = Number(key.dataset.midi);
        key.classList.toggle("held", held.has(midi) || key.classList.contains("pressed"));
      }
    }

    for (const enclosure of snapshot.enclosures ?? []) {
      if (this.dragging.has(`enclosure-${enclosure.idx}`)) continue;
      const shoe = this.root.querySelector(`.shoe[data-enclosure="${enclosure.idx}"]`);
      if (shoe) this.setShoe(shoe, enclosure.value);
    }

    if (!this.dragging.has("gain")) this.el.gain.value = snapshot.gain;

    const tuning = snapshot.tuning;
    this.el.tuning.textContent = tuning
      ? `${tuning.temperament} · a′ ${tuning.a4.toFixed(0)} Hz` +
        (tuning.transpose ? ` · ${tuning.transpose > 0 ? "+" : ""}${tuning.transpose}` : "")
      : "";
    this.el.tuning.classList.toggle("hidden", !tuning);
  }

  setToggle(key, on) {
    const control = this.root.querySelector(`[data-key="${key}"]`);
    if (control) control.classList.toggle("on", on);
  }

  // ---- menu bar ------------------------------------------------------

  wireRail() {
    const slider = this.el.gain;
    let lastSent = 0;
    slider.addEventListener("pointerdown", () => this.dragging.add("gain"));
    slider.addEventListener("input", () => {
      const now = performance.now();
      if (now - lastSent > 33) {
        lastSent = now;
        this.send(commands.gain(slider.value));
      }
    });
    slider.addEventListener("change", () => {
      this.dragging.delete("gain");
      this.send(commands.gain(slider.value));
    });

    this.el.panic.addEventListener("click", () => this.panic());
    this.el.tuning.addEventListener("click", () => this.openPreferences("tuning"));
  }
}
