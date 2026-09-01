// The console renderer: builds the DOM once per organ, then updates
// classes and values on every snapshot. Structure (which stops exist,
// keyboard geometry) changes only when a different organ loads, so the
// two paths are kept separate: `build` for structure, `refresh` for
// state. Interaction is optimistic — controls flip visually on click
// and the next snapshot reconciles.
//
// The console is a canvas of panels: one jamb panel per division, one
// keyboard panel per manual, the coupler rail, the swell-shoe rack.
// Each panel is absolutely positioned — from `snapshot.layout` (the
// organ file's own [console.layout], normalized 0..1 fractions of the
// canvas) when a panel has been placed, otherwise from `defaultLayout`,
// which reproduces the classic arrangement: jambs flanking, manuals
// stacked mid-case, pedalboard below, shoes beside it. Moving panels
// is the editor's job (editor.js); this file only draws and places.

import { keyboardScale, measureKeyboard } from "./kb-scale.js";
import { commands } from "./api.js";
import { renderIfChanged, setText } from "./dom.js";
import { formatFootage, keyName } from "./pitch.js";

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

/// A stop drawknob's two engraved lines, from data rather than parsed
/// out of the name. `stop.label` carries the contract: absent means
/// auto — engrave whatever footage the stop actually speaks at right
/// now (its own override if voiced, else its native pitch); `""` means
/// engrave nothing below the name, so the name stands alone on one
/// line; any other string is engraved verbatim. A mixture speaks no
/// single footage, and an old server sends no pitch at all — both fall
/// back to splitLabel's guess from the name text, same as before this
/// existed.
function stopFace(stop) {
  if (stop.label === "") return [stop.name, ""];
  if (stop.label != null) return [splitLabel(stop.name)[0], stop.label];
  const effective = stop.pitch ? (stop.pitch.footage ?? stop.pitch.native) : null;
  return effective != null
    ? [splitLabel(stop.name)[0], `${formatFootage(effective)}'`]
    : splitLabel(stop.name);
}

export class Console {
  /// `openTuning(x, y)` is how the bar's tuning readout opens the
  /// whole-instrument tuning popover right under itself — tuning is an
  /// organ fact and is edited on the console, never in Preferences.
  /// `enterEditMode(x, y)` is what an empty organ's card offers
  /// instead: editing unlocked and the add menu open at the click — an
  /// empty organ is already auto-unlocked, so unlocking alone would
  /// visibly do nothing.
  /// `decorate` is set by main.js after construction (see editor.js) and
  /// called at the end of every structural `build()` — Console builds
  /// the DOM, the editor only ever decorates what's already there.
  constructor(root, send, openTuning, enterEditMode) {
    this.root = root;
    this.send = send;
    this.openTuning = openTuning;
    this.enterEditMode = enterEditMode;
    this.decorate = null;
    this.snapshot = null;
    this.layoutSig = null; // JSON of the last snapshot.layout applied
    this.panels = new Map(); // panel id -> its element on the canvas
    this.dragging = new Set(); // control ids the pointer currently owns
    this.el = {
      offline: root.getElementById("offline"),
      organName: root.getElementById("organ-name"),
      gain: root.getElementById("gain"),
      tuning: root.getElementById("tuning"),
      panic: root.getElementById("panic"),
      canvas: root.getElementById("console-canvas"),
      emptyCard: root.getElementById("organ-empty-card"),
    };
    this.wireRail();
    window.addEventListener("resize", () => {
      if (!this.snapshot) return;
      this.layoutPanels(this.snapshot);
      // A zoom leaves every box the same size in CSS pixels but not
      // the type rendered into it, so the field observer below stays
      // quiet and the cheeks are refitted from here.
      this.fitCheeks();
      this.fitShoes();
    });
    // Each keyboard's key field is watched for size changes — density,
    // a resize drag, a stored width landing — and its cheek refitted
    // to the new room. The field rather than the cheek: a fit changes
    // the cheek's own width, and an observer there would chase itself.
    this.fields = new ResizeObserver((entries) => {
      for (const { target } of entries) {
        const cheek = target.parentElement?.querySelector(".cheek");
        if (cheek) this.fitCheek(cheek);
      }
    });
  }

  offline(message) {
    this.el.offline.textContent = `no connection to the organ — ${message}`;
    this.el.offline.classList.remove("hidden");
  }

  render(snapshot) {
    this.snapshot = snapshot;
    this.el.offline.classList.add("hidden");
    // With no organ there is no console to show — otherwise the lone
    // tremulant knob haunts the empty background behind the loader.
    this.root.body.classList.toggle("no-organ", !snapshot.organ);
    const signature = JSON.stringify([
      snapshot.organ,
      snapshot.stops.map((s) => [s.id, s.name, s.manual, ...stopFace(s)]),
      // A coupler's jamb seat (midx) is structure: seating one moves
      // its control from the rail into a division's knob rank.
      snapshot.couplers.map((c) => [c.name, !!c.hidden, c.midx ?? null]),
      snapshot.manuals.map((m) => [m.name, m.first_key, m.key_count, m.kind, m.hex, m.colors, m.rank]),
      // No organ (the picker's start-empty state) sends no enclosures
      // at all — same as an organ without a swell box.
      (snapshot.enclosures ?? []).filter((e) => e.displayed).map((e) => e.name),
      snapshot.reverb != null,
    ]);
    renderIfChanged(this.el.canvas, signature, () => this.build(snapshot));
    this.refresh(snapshot);
  }

  // ---- structure ----------------------------------------------------

  build(snapshot) {
    this.el.organName.textContent = snapshot.organ ?? "No organ";
    // A loaded organ with nothing built yet has no panels worth drawing —
    // a card points at the editor instead. The canvas stays live under
    // it: double-clicking the bare case is how the first manual arrives.
    const empty = !!snapshot.organ && !snapshot.stops.length && !snapshot.manuals.length;
    this.el.emptyCard.classList.toggle("hidden", !empty);
    this.layoutSig = null; // panels are new; place them on next refresh
    this.fields.disconnect(); // the old fields go with the old panels
    if (empty) {
      this.panels.clear();
      this.el.canvas.replaceChildren();
      this.buildEmptyCard(snapshot);
      this.decorate?.(snapshot);
      return;
    }
    this.buildPanels(snapshot);
    this.fitLabels();
    this.fitShoes();
    this.decorate?.(snapshot);
  }

  buildEmptyCard(snapshot) {
    this.el.emptyCard.replaceChildren();
    const title = document.createElement("h2");
    title.textContent = snapshot.organ ?? "Untitled organ";
    const note = document.createElement("p");
    note.textContent =
      "An empty organ, ready to edit — double-click anywhere to add " +
      "manuals and sample sets. Added sets offer their stops in the " +
      "Library drawer, ready to drag onto a manual.";
    const open = document.createElement("button");
    open.type = "button";
    open.textContent = "Start building";
    // stopPropagation: the click must not reach the window listener
    // that closes popovers, or it would shut the add menu it opens.
    open.addEventListener("click", (event) => {
      event.stopPropagation();
      this.enterEditMode(event.clientX, event.clientY);
    });
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

  /// A cheek's lettering never runs past its key field: the rendered
  /// run is measured against the room and the type shrunk until it
  /// fits. Rendered, not predicted — hinted glyphs at small device
  /// sizes come out longer than any em count says, so the fit is
  /// redone whenever the room changes (the field observer) or the
  /// rendering may have (a zoom; the resize listener). The box is the
  /// field's height regardless (style.css), so nothing stretches the
  /// panel or hangs off its ends even before a refit lands.
  fitCheek(cheek) {
    cheek.style.removeProperty("--cheek-fit");
    const room = cheek.clientHeight;
    if (!room) return;
    let fit = 1;
    // one step straight from the overrun, then nudges — shrinking is
    // not quite linear at the sizes where hinting bites
    for (let i = 0; i < 8 && cheek.scrollHeight > room; i++) {
      fit *= i === 0 ? room / cheek.scrollHeight : 0.97;
      cheek.style.setProperty("--cheek-fit", fit.toFixed(3));
    }
  }

  fitCheeks() {
    for (const cheek of this.root.querySelectorAll(".cheek")) this.fitCheek(cheek);
  }

  /// A shoe's scribble strip is a fixed width and two lines deep
  /// (style.css), so a long box name is shrunk to fit it — measured
  /// like the stop faces and the cheeks — until its longest word spans
  /// the strip and the whole name sits within the lines. The floor is
  /// the smallest legible size; past it the strip ellipsizes and the
  /// tooltip carries the whole name.
  fitShoe(label) {
    label.style.removeProperty("--shoe-fit");
    if (!label.clientWidth) return;
    const tooWide = () => label.scrollWidth > label.clientWidth;
    const tooTall = () => label.scrollHeight > label.clientHeight;
    let fit = 1;
    for (let i = 0; i < 8 && fit > 0.75 && (tooWide() || tooTall()); i++) {
      // a word's overrun says exactly how far to shrink; a spilt third
      // line only that it must shrink, so that one is nudged
      const step = i === 0 && tooWide() ? label.clientWidth / label.scrollWidth : 0.97;
      fit = Math.max(0.75, fit * step);
      label.style.setProperty("--shoe-fit", fit.toFixed(3));
    }
  }

  fitShoes() {
    for (const label of this.root.querySelectorAll(".shoe-label")) this.fitShoe(label);
  }

  // ---- panels -------------------------------------------------------

  /// One panel: DAW-style chrome (a slim title bar the editor drags,
  /// visible only while editing) above the content. The chrome is part
  /// of every panel from birth so locking and unlocking never rebuilds.
  panel(id, kind, title) {
    const el = document.createElement("section");
    el.className = `panel panel-${kind}`;
    el.dataset.panel = id;
    const chrome = document.createElement("div");
    chrome.className = "panel-chrome";
    const label = document.createElement("span");
    label.className = "panel-chrome-title";
    label.textContent = title;
    chrome.append(label);
    const body = document.createElement("div");
    body.className = "panel-body";
    el.append(chrome, body);
    this.panels.set(id, el);
    this.el.canvas.append(el);
    return body;
  }

  buildPanels(snapshot) {
    this.panels.clear();
    this.el.canvas.replaceChildren();

    // The model declares each manual's kind now; a manual predates the
    // field only on an old snapshot, where the pedal flag and then the
    // name sniff are the only signals there are.
    const kindOf = (manual) =>
      manual.kind ?? (manual.pedal || /p[ée]d/i.test(manual.name) ? "pedal" : "manual");
    const pedal = snapshot.manuals.find((m) => kindOf(m) === "pedal") ?? snapshot.manuals[0];
    this.pedalName = pedal?.name ?? null;

    // Stops grouped by the manual they sit on, in server stop order.
    const byManual = new Map(snapshot.manuals.map((m) => [m.idx, []]));
    for (const stop of snapshot.stops) {
      if (!byManual.has(stop.midx)) byManual.set(stop.midx, []);
      byManual.get(stop.midx).push(stop);
    }

    // One jamb panel per division, empty divisions included — an empty
    // jamb is where a new manual's first stop gets added. The rank is
    // the snapshot's token order: stops and seated couplers
    // interleaved, exactly as the organ file lists them.
    let lastColumn = null;
    for (const manual of snapshot.manuals) {
      const stops = byManual.get(manual.idx) ?? [];
      const stopById = new Map(stops.map((s) => [s.id, s]));
      const rank = manual.rank ?? stops.map((s) => `s${s.id}`);
      const body = this.panel(`jamb:${manual.name}`, "jamb", `${manual.name} · stops`);
      const jambPanel = body.parentElement;
      jambPanel.classList.toggle("empty", !rank.length);
      const column = document.createElement("div");
      column.className = "division";
      column.dataset.division = manual.idx;
      const head = document.createElement("div");
      head.className = "division-head";
      const title = document.createElement("h2");
      title.textContent = manual.name;
      head.append(title);
      column.append(head);
      // The knobs live in a wrap container of their own: one knob
      // wide it stacks the single column it always has, but a jamb
      // the player has resized (see layoutPanels) frees the width and
      // the rank wraps into columns.
      const knobs = document.createElement("div");
      knobs.className = "division-knobs";
      for (const token of rank) {
        if (token.startsWith("s")) {
          const stop = stopById.get(Number(token.slice(1)));
          if (!stop) continue;
          knobs.append(this.drawknob(stop.name, `stop-${stop.id}`, (on) =>
            this.send(commands.stop(stop.id, on)), stopFace(stop)
          ));
        } else if (token.startsWith("c")) {
          const coupler = snapshot.couplers.find((c) => c.idx === Number(token.slice(1)));
          if (coupler) knobs.append(this.couplerKnob(coupler));
        }
      }
      column.append(knobs);
      body.append(column);
      if (stops.length) lastColumn = knobs;
    }

    // Couplers rail (built before the tremulant so the trem knob has a
    // home even on an organ with manuals but no stops yet).
    const couplersBody = this.panel("couplers", "couplers", "Couplers");
    const rail = document.createElement("div");
    rail.className = "coupler-rail";
    // A coupler taken off the console (restorable from the add menu) is disengaged,
    // not deleted — it simply doesn't get a tablet on the rail. One
    // seated in a jamb (midx) wears a drawknob there instead.
    for (const coupler of snapshot.couplers.filter((c) => !c.hidden && c.midx == null)) {
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
      rail.append(rocker);
    }
    rail.append(this.cancelPiston());
    couplersBody.append(rail);

    // The tremulant behaves like a stop; it joins the bottom of the last
    // populated division so the jambs read as ranks of knobs.
    const trem = this.drawknob("Tremblant", "trem", (on) =>
      this.send(commands.tremulant(on))
    );
    if (lastColumn) lastColumn.append(trem);
    else rail.append(trem);

    // Keyboard panels: one per manual. Positioning does the stacking
    // (highest manual on top, pedal at the bottom), not the DOM order.
    for (const manual of snapshot.manuals) {
      const kind = kindOf(manual);
      const body = this.panel(`keyboard:${manual.name}`, `keyboard panel-${kind}`, `${manual.name} · keyboard`);
      body.append(this.keyboard(manual, kind));
    }

    // The swell shoes, a rack of their own beside the pedalboard.
    const shoes = this.shoes(snapshot);
    if (shoes.childElementCount) {
      this.panel("shoes", "shoes", "Swell shoes").append(shoes);
    }
  }

  /// `lines`, when given, is the `[nameLine, footageLine]` pair to
  /// engrave — see `stopFace` — so a stop's face comes from its data
  /// rather than being re-derived from its name here. Omitted (every
  /// non-stop knob, e.g. the Tremblant), it falls back to splitting the
  /// name the old way.
  drawknob(name, key, flip, lines) {
    const knob = document.createElement("button");
    knob.className = "knob";
    knob.dataset.key = key;
    const [line, foot] = lines ?? splitLabel(name);
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

  /// A coupler seated in a jamb wears a drawknob like the stops around
  /// it — same click to engage, same right-click to edit; only the
  /// engraving style says its job.
  couplerKnob(coupler) {
    const knob = this.drawknob(coupler.name, `coupler-${coupler.idx}`, (on) =>
      this.send(commands.coupler(coupler.idx, on))
    );
    knob.classList.add("coupler");
    return knob;
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

    if (kind === "microtonal") {
      this.hexKeys(board, keys, manual, last);
    } else {
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
    }
    board.append(keys);
    this.fields.observe(keys); // fires once the field is laid out: the first fit
    return board;
  }

  /// A Terpstra/Lumatone-style hex-grid key field: no naturals or
  /// sharps — that split is 12-EDO vocabulary, and this keyboard
  /// deliberately carries none. The snapshot's `hex` layout says what
  /// the grid means: `rows` × `cols` hexes, the key number advancing
  /// by `right` per hex rightward and `upright` per hex up-rightward
  /// from `anchor` at the bottom left — the two generators that make
  /// the board isomorphic. Odd rows sit half a hex right of even ones
  /// (each row re-centers, so the board reads as a staggered rectangle
  /// rather than a leaning parallelogram); rows count bottom-up, pitch
  /// rising toward the upper right. Distinct hexes can carry the same
  /// key number — isomorphic boards' duplicate notes — and since held
  /// state is matched by `data-midi`, they light together. Hexes whose
  /// key falls outside the compass render dead: present, dimmed,
  /// unplayable, so the board's shape never depends on the compass.
  hexKeys(board, keys, manual, last) {
    const hex = manual.hex ?? {
      rows: 1,
      cols: manual.key_count,
      right: 1,
      upright: 0,
      anchor: manual.first_key,
    };
    // On the board, not the field: the cheek beside the field needs
    // the row count too (style.css derives --kb-h from it).
    board.style.setProperty("--hex-rows", hex.rows);
    board.style.setProperty("--hex-cols", hex.cols);
    for (let row = 0; row < hex.rows; row++) {
      for (let col = 0; col < hex.cols; col++) {
        const key = document.createElement("div");
        key.className = "key hex";
        const axial = col - Math.floor(row / 2);
        const midi = hex.anchor + axial * hex.right + row * hex.upright;
        key.style.setProperty("--hx", col + (row % 2) * 0.5);
        key.style.setProperty("--hy", hex.rows - 1 - row);
        if (midi < manual.first_key || midi >= last) {
          key.classList.add("dead");
        } else {
          key.dataset.midi = midi;
          // A bound Lumatone map's key colours, keyed the same way
          // its notes land — the player's own navigation marks.
          const color = manual.colors?.[midi];
          if (color) key.style.setProperty("--hex-color", color);
          this.wireKey(key, manual.idx, midi);
        }
        keys.append(key);
      }
    }
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
      label.title = enclosure.name;

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

  // ---- placement ----------------------------------------------------

  /// Positions every panel: placed ones from snapshot.layout (fractions
  /// of the canvas), the rest from defaultLayout. A panel the editor is
  /// mid-drag on is left alone — its position is the pointer's.
  layoutPanels(snapshot) {
    const W = this.el.canvas.clientWidth;
    const H = this.el.canvas.clientHeight;
    if (!W || !H || !this.panels.size) return;
    const placed = snapshot.layout ?? {};
    // Sizes first, positions second: the default layout measures every
    // panel to seat the unplaced ones around the placed, so a jamb's
    // player-set width must be real before anything is measured — or
    // the auto-laid panels seat themselves over the columns it grew.
    for (const [id, el] of this.panels) {
      if (el.dataset.dragging) continue;
      const sized = placed[id]?.w != null;
      // A sized keyboard scales its keys to the recorded width (see
      // --kb-scale in style.css) rather than taking a width style —
      // the panel keeps hugging the (scaled) content, so keys are
      // never clipped or orphaned in space.
      if (id.startsWith("keyboard:")) {
        this.scaleKeyboard(el, sized ? placed[id].w * W : null);
        continue;
      }
      // A player-sized panel: the dragged width is what wraps a
      // jamb's knobs into columns; height always follows the content,
      // so nothing is ever clipped. (`h` still rides the layout for
      // symmetry; only the width is load-bearing today.)
      el.classList.toggle("sized", sized);
      el.style.width = sized ? `${Math.round(placed[id].w * W)}px` : "";
    }
    const defaults = this.defaultLayout(snapshot, W, H);
    for (const [id, el] of this.panels) {
      if (el.dataset.dragging) continue;
      const pos = placed[id]
        ? { x: placed[id].x * W, y: placed[id].y * H }
        : (defaults.get(id) ?? { x: 24, y: 24 });
      // Kept on the canvas: a placement recorded on a wider window (or
      // at a smaller zoom) would otherwise seat the panel past the
      // edge, where the canvas clips it — the same clamp the editor's
      // drag applies, so the panel sits where the drag would have
      // left it.
      const x = Math.max(0, Math.min(pos.x, W - el.offsetWidth));
      const y = Math.max(0, Math.min(pos.y, H - el.offsetHeight));
      el.style.left = `${Math.round(x)}px`;
      el.style.top = `${Math.round(y)}px`;
    }
  }

  /// Scales a keyboard panel so it comes out `targetPx` wide (null =
  /// natural size); the math is kb-scale.js's, shared with the editor's
  /// grip drag.
  scaleKeyboard(el, targetPx) {
    if (targetPx == null) {
      el.style.removeProperty("--kb-scale");
      return;
    }
    const measured = measureKeyboard(el);
    if (measured) el.style.setProperty("--kb-scale", keyboardScale(measured, targetPx));
  }

  /// The classic console, derived rather than hard-coded: coupler rail
  /// on top, manuals stacked beneath it highest-first, pedalboard at
  /// the bottom with the shoes at its right, jambs flanking — first
  /// half of the divisions on the left, the rest on the right.
  defaultLayout(snapshot, W, H) {
    const pos = new Map();
    const size = (id) => {
      const el = this.panels.get(id);
      return el ? { w: el.offsetWidth, h: el.offsetHeight } : null;
    };
    const GAP = 26; // room for the edit-mode title bar above each panel
    const PAD = 24;

    const pedal = snapshot.manuals.find((m) => m.name === this.pedalName) ?? null;
    const manuals = snapshot.manuals.filter((m) => m !== pedal);

    // The flanking jambs first — the keyboard stack centers in the
    // space they leave, never under them.
    const jambs = snapshot.manuals
      .map((m) => `jamb:${m.name}`)
      .filter((id) => this.panels.has(id));
    const split = Math.ceil(jambs.length / 2);
    const leftJambs = jambs.slice(0, split);
    const rightJambs = jambs.slice(split);
    const groupWidth = (ids) =>
      ids.reduce((sum, id) => sum + (size(id)?.w ?? 0), 0) + 14 * Math.max(0, ids.length - 1);
    let x = PAD;
    for (const id of leftJambs) {
      const s = size(id);
      pos.set(id, { x, y: Math.max(PAD, (H - s.h) / 2) });
      x += s.w + 14;
    }
    x = W - PAD;
    for (const id of rightJambs.slice().reverse()) {
      const s = size(id);
      x -= s.w;
      pos.set(id, { x, y: Math.max(PAD, (H - s.h) / 2) });
      x -= 14;
    }

    const stack = [];
    if (this.panels.has("couplers")) stack.push("couplers");
    for (const manual of [...manuals].reverse()) stack.push(`keyboard:${manual.name}`);
    if (pedal) stack.push(`keyboard:${pedal.name}`);

    const innerLeft = PAD + groupWidth(leftJambs) + (leftJambs.length ? GAP : 0);
    const innerRight = W - PAD - groupWidth(rightJambs) - (rightJambs.length ? GAP : 0);
    const stackW = Math.max(0, ...stack.map((id) => size(id)?.w ?? 0));
    const cx = innerLeft + Math.max(0, (innerRight - innerLeft - stackW) / 2);
    const totalH = stack.reduce((sum, id) => sum + (size(id)?.h ?? 0), 0)
      + GAP * Math.max(0, stack.length - 1);
    let y = Math.max(PAD, (H - totalH) / 2);
    for (const id of stack) {
      const s = size(id);
      if (!s) continue;
      pos.set(id, { x: cx + Math.max(0, (stackW - s.w) / 2), y });
      y += s.h + GAP;
    }

    // Shoes go beside the pedalboard, tops level with it: the manuals
    // above and the jambs beside stay clear, and the rack hangs down
    // into the apron, which is bare anyway.
    if (this.panels.has("shoes")) {
      const s = size("shoes");
      const anchor = pedal && pos.get(`keyboard:${pedal.name}`);
      const anchorSize = pedal && size(`keyboard:${pedal.name}`);
      pos.set("shoes", anchor
        ? {
            x: anchor.x + anchorSize.w + GAP,
            y: Math.min(anchor.y, H - s.h - PAD),
          }
        : { x: W - s.w - PAD, y: H - s.h - PAD });
    }
    return pos;
  }

  // ---- state --------------------------------------------------------

  refresh(snapshot) {
    const layoutSig = JSON.stringify(snapshot.layout ?? {});
    if (layoutSig !== this.layoutSig) {
      this.layoutSig = layoutSig;
      this.layoutPanels(snapshot);
    }

    for (const stop of snapshot.stops) this.setToggle(`stop-${stop.id}`, stop.on);
    for (const coupler of snapshot.couplers) {
      this.setToggle(`coupler-${coupler.idx}`, coupler.on);
    }
    this.setToggle("trem", snapshot.tremulant);

    for (const manual of snapshot.manuals) {
      const board = this.root.querySelector(`.keyboard[data-manual="${manual.idx}"]`);
      if (!board) continue;
      const held = new Set(manual.held);
      // Keys an engaged coupler is pulling down — the mechanical-
      // action view, drawn dipped but quieter than a played key.
      const coupled = new Set(manual.coupled ?? []);
      for (const key of board.querySelectorAll(".key")) {
        // Keys the local pointer is holding stay lit regardless.
        const midi = Number(key.dataset.midi);
        const down = held.has(midi) || key.classList.contains("pressed");
        key.classList.toggle("held", down);
        key.classList.toggle("coupled", !down && coupled.has(midi));
      }
    }

    for (const enclosure of snapshot.enclosures ?? []) {
      if (this.dragging.has(`enclosure-${enclosure.idx}`)) continue;
      const shoe = this.root.querySelector(`.shoe[data-enclosure="${enclosure.idx}"]`);
      if (shoe) this.setShoe(shoe, enclosure.value);
    }

    if (!this.dragging.has("gain")) this.el.gain.value = snapshot.gain;

    const tuning = snapshot.tuning;
    // What the readout leads with is whatever actually governs pitch:
    // an active scale, else a division count away from 12, else the
    // temperament (absent edo on an old snapshot means 12) — "original"
    // reads as "as recorded" here, the LCD's own lowercase idiom for
    // the tuning popover's "As recorded".
    const governs = tuning
      ? tuning.scale?.name ??
        ((tuning.edo ?? 12) !== 12
          ? `${tuning.edo}-EDO`
          : tuning.temperament === "original" ? "as recorded" : tuning.temperament)
      : "";
    // "C4 = 261.6" rather than "a′ NNN Hz": the anchor is a key/Hz pair
    // now, not a 12-EDO-only concept — up to one decimal, no trailing ".0".
    const hz = tuning ? tuning.reference.hz.toFixed(1).replace(/\.0$/, "") : "";
    // Through setText: this button is repainted every poll, and a text
    // node swapped under a press costs the click on WebKit (dom.js).
    setText(
      this.el.tuning,
      tuning
        ? `${governs} · ${keyName(tuning.reference.key)} = ${hz}` +
          (tuning.transpose ? ` · ${tuning.transpose > 0 ? "+" : ""}${tuning.transpose}` : "")
        : ""
    );
    this.el.tuning.classList.toggle("hidden", !tuning);
  }

  setToggle(key, on) {
    // All matches, not the first: a coupler seated in a jamb and its
    // rail tablet never coexist today, but nothing should break if a
    // control ever wears two faces.
    for (const control of this.root.querySelectorAll(`[data-key="${key}"]`)) {
      control.classList.toggle("on", on);
    }
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
    this.el.tuning.addEventListener("click", (event) => {
      // The same click must not travel on to the window's close-all
      // listener and shut the popover it just opened.
      event.stopPropagation();
      const rect = this.el.tuning.getBoundingClientRect();
      this.openTuning(rect.left, rect.bottom + 6);
    });
  }
}
