import { commands, localFetch } from "./api.js";
import { setText } from "./dom.js";

const CATEGORIES = [
  ["general", "Organ & sound", "Name, tuning and the room"],
  ["keyboards", "Keyboards & inputs", "Connect and configure your keyboards"],
  ["stops", "Stops & pipes", "Voicing, pitch and sample sources"],
  ["couplers", "Couplers", "Connections between keyboards"],
  ["sources", "Sample sets", "The sounds in this instrument"],
  ["bindings", "Buttons & shortcuts", "MIDI controls and computer keys"],
];
const PANEL_CATEGORY = {
  "editor-stop": "stops", "editor-key-voicing": "stops",
  "editor-midi": "keyboards", "editor-compass": "keyboards",
  "editor-hex": "keyboards", "editor-keyboard-menu": "keyboards",
  "editor-division-menu": "keyboards", "editor-coupler": "couplers",
  "editor-couplers-menu": "couplers", "editor-bindings": "bindings",
  "editor-piston": "bindings", "editor-room": "general", "editor-trem": "general",
};
const node = (tag, text = "", cls = "") => Object.assign(document.createElement(tag), {
  textContent: text, className: cls,
});

// One workspace owns the live forms. Console shortcuts enter here as well;
// appearance and memory always stay in App preferences. Forms are moved, never
// cloned, so a setting has one field, listener and persistence path.
export class OrganPreferences {
  constructor(root, editor) {
    this.root = root;
    this.editor = editor;
    editor.settings = this;
    this.category = "general";
    this.modal = root.getElementById("organ-prefs");
    this.index = root.getElementById("organ-prefs-index");
    this.host = root.getElementById("organ-prefs-editors");
    this.back = root.getElementById("organ-prefs-back");
    this.content = this.modal.querySelector(".organ-prefs-content");
    this.section = root.getElementById("organ-prefs-section");
    this.panels = [...root.querySelectorAll(".editor-add"), editor.el.removeConfirm, editor.el.linkConfirm];
    this.moved = [];
    for (const [id, label] of CATEGORIES) {
      const button = this.button(label, () => this.select(id));
      button.dataset.category = id;
      root.getElementById("organ-prefs-nav").append(button);
      const option = node("option", label);
      option.value = id;
      this.section.append(option);
    }
    this.section.addEventListener("change", () => this.select(this.section.value));
    for (const button of this.modal.querySelectorAll("[data-organ-prefs-close]")) {
      button.addEventListener("click", () => this.close());
    }
    this.back.addEventListener("click", () => this.goBack());
    this.observer = new MutationObserver(() => this.syncPanels());
    for (const panel of this.panels) {
      this.observer.observe(panel, { attributes: true, attributeFilter: ["class"] });
    }
    window.addEventListener("keydown", event => {
      if (event.key !== "Escape" || !this.isOpen || this.modal.inert) return;
      event.preventDefault();
      event.stopImmediatePropagation();
      if (this.active || this.keyboard != null) this.goBack();
      else this.close();
    }, true);
  }

  get isOpen() { return !this.modal.classList.contains("hidden"); }

  move(element, parent) {
    const marker = document.createComment("instrument editor home");
    element.before(marker);
    parent.append(element);
    element.inert = false;
    this.moved.push([element, marker]);
  }

  open(preserveEditors = false) {
    if (this.isOpen || !this.editor.lastSnapshot?.organ) return;
    this.editor.setInspect(false);
    if (!preserveEditors) this.editor.closeAllPopovers();
    this.editor.closeDrawer();
    for (const panel of this.panels) this.move(panel, this.host);
    this.move(this.editor.el.error, this.root.getElementById("organ-prefs-errors"));
    this.modal.classList.remove("hidden");
    this.render();
    this.syncNavigation();
    this.syncPanels();
  }

  close() {
    if (!this.isOpen) return;
    if (!this.finishEditing()) return;
    this.clearConfirmation();
    this.modal.classList.add("hidden");
    for (const [element, marker] of this.moved) {
      element.classList.remove("settings-suspended");
      marker.replaceWith(element);
    }
    this.moved = [];
    this.active = null;
  }

  finishEditing() {
    const previous = this.editor.pendingLink;
    this.editor.closeAllPopovers();
    // Finishing a coupler can ask whether identical routes should be linked.
    // Leave that existing choice visible before completing navigation.
    if (this.editor.pendingLink && this.editor.pendingLink !== previous) {
      this.syncPanels();
      return false;
    }
    return true;
  }

  clearConfirmation() {
    this.editor.hideRemoveConfirm();
    this.editor.pendingLink = null;
    this.editor.el.linkConfirm.classList.add("hidden");
  }

  // Called before a live form initializes its subject. Only the stop's tuning
  // and pipe subviews retain their parent; unrelated editors make way.
  prepare(kind) {
    if (!this.isOpen) this.open();
    else if (!(this.editor.stopOpen != null && ["keyVoicing", "tuning"].includes(kind))) {
      this.editor.closeAllPopovers();
    }
  }

  // positionPopover calls this after initialization. Quick menus use it too,
  // which makes touch Add, right-click shortcuts and settings lists converge.
  present(panel) {
    if (!this.panels.includes(panel)) return;
    if (!this.isOpen) this.open(true);
    if (!this.isOpen) return;
    const changed = this.active !== panel;
    this.active = panel;
    let category = PANEL_CATEGORY[panel.id];
    if (panel === this.editor.el.tuning) {
      const kind = this.editor.tuningScope?.kind;
      category = kind === "division" ? "keyboards" : kind === "source" ? "sources"
        : ["stop", "rank"].includes(kind) ? "stops" : "general";
    }
    if (category) this.category = category;
    this.syncNavigation();
    this.syncPanels();
    if (changed) {
      this.content.scrollTop = 0;
      // A heading gives context without selecting or changing a live field.
      const heading = panel.querySelector(".editor-tuning-head, .menu-heading, h2") ?? panel;
      heading.tabIndex = -1;
      requestAnimationFrame(() => {
        if (this.active === panel && !this.modal.inert) heading.focus({ preventScroll: true });
      });
    }
  }

  goBack() {
    if (this.active && this.editor.stopOpen != null &&
        [this.editor.el.tuning, this.editor.el.keyVoicing].includes(this.active)) {
      this.editor.closeTuningForm();
      this.editor.closeKeyVoicing();
      this.present(this.editor.el.stop);
      return;
    }
    if (this.active) {
      if (!this.finishEditing()) return;
      this.clearConfirmation();
      this.active = null;
    } else this.keyboard = null;
    this.render();
    this.syncPanels();
    this.content.scrollTop = 0;
    this.index.querySelector("h2")?.focus({ preventScroll: true });
  }

  button(label, run, note) {
    const button = node("button", "", "organ-pref-action");
    button.type = "button";
    const copy = node("span", "", "organ-pref-action-copy");
    copy.append(node("span", label));
    if (note) copy.append(node("small", note));
    const arrow = node("span", "›", "organ-pref-arrow");
    arrow.setAttribute("aria-hidden", "true");
    button.append(copy, arrow);
    button.addEventListener("click", run);
    return button;
  }

  action(label, run, note, target = this.index) {
    const button = this.button(label, () => { run(); this.syncPanels(); }, note);
    target.append(button);
    return button;
  }

  edit(run) { this.editor.closeAllPopovers(); run(); this.syncPanels(); }

  add(kind) {
    this.edit(() => {
      this.editor.pendingPlace = null;
      this.editor.openAddMenu(20, 80);
      if (kind) this.root.getElementById(`editor-add-${kind}`).click();
    });
  }

  select(category) {
    if (!this.finishEditing()) return;
    this.clearConfirmation();
    this.active = null;
    this.keyboard = null;
    this.category = category;
    this.syncNavigation();
    this.render();
    this.syncPanels();
    this.content.scrollTop = 0;
  }

  syncNavigation() {
    this.section.value = this.category;
    for (const button of this.modal.querySelectorAll("[data-category]")) {
      button.setAttribute("aria-current", String(button.dataset.category === this.category));
    }
    setText(this.root.getElementById("organ-prefs-context"),
      `${this.editor.lastSnapshot?.organ ?? ""} · Edits save automatically`);
  }

  syncPanels() {
    if (!this.isOpen) return;
    const visible = this.panels.filter(panel => !panel.classList.contains("hidden"));
    if (!visible.includes(this.active)) this.active = visible.at(-1) ?? null;
    // A duplicate-route or removal confirmation is above its editor.
    const confirmation = visible.find(panel => [this.editor.el.removeConfirm, this.editor.el.linkConfirm].includes(panel));
    const current = confirmation ?? this.active;
    for (const panel of this.panels) {
      const suspend = visible.includes(panel) && panel !== current;
      if (panel.classList.contains("settings-suspended") !== suspend) {
        panel.classList.toggle("settings-suspended", suspend);
      }
    }
    const returning = this.index.classList.contains("hidden") && !current;
    this.index.classList.toggle("hidden", !!current);
    this.back.classList.toggle("hidden", !current && this.keyboard == null);
    setText(this.back, current && this.editor.stopOpen != null &&
      [this.editor.el.tuning, this.editor.el.keyVoicing].includes(current)
      ? "← Back to stop" : `← ${CATEGORIES.find(([id]) => id === this.category)?.[1] ?? "Settings"}`);
    if (returning) this.render();
  }

  update(snapshot) {
    this.root.getElementById("instrument-settings").disabled = !snapshot.organ;
    if (!this.isOpen) return;
    if (!snapshot.organ) { this.close(); return; }
    this.syncNavigation();
    const signature = JSON.stringify([
      snapshot.setup?.file, snapshot.organ,
      snapshot.manuals?.map(m => [m.idx, m.name, m.kind]),
      snapshot.stops?.map(s => [s.id, s.name, s.midx]),
      snapshot.couplers?.map(c => [c.idx, c.name, c.hidden]), snapshot.setup?.sources,
    ]);
    if (signature !== this.signature) {
      // Preserve a typed name or search; defer the refresh until focus leaves.
      if (this.active) this.signature = signature;
      else if (!this.index.contains(this.root.activeElement)) {
        this.signature = signature;
        this.render();
      }
    }
    this.syncPanels();
  }

  heading(text, target = this.index) {
    const heading = node("h2", text);
    heading.tabIndex = -1;
    target.append(heading);
  }

  openKeyboard(idx) {
    this.open();
    this.select("keyboards");
    this.keyboard = idx;
    this.render();
    this.syncPanels();
  }

  renderKeyboard(s) {
    const m = s.manuals.find(manual => manual.idx === this.keyboard);
    if (!m) { this.keyboard = null; this.render(); return; }
    const e = this.editor;
    this.heading(m.name);
    this.index.append(node("p", "One keyboard, its inputs and the division it plays.", "pane-note section-description"));
    this.rename("Keyboard name", m.name, name => e.organCommand(commands.organManualRename(m.idx, name)));
    const type = node("label", "Keyboard type", "organ-pref-field");
    const select = node("select");
    select.setAttribute("aria-label", `${m.name} keyboard type`);
    for (const [value, label] of [["manual", "Hand keyboard"], ["pedal", "Pedalboard"], ["microtonal", "Microtonal keyboard"]]) {
      const option = node("option", label); option.value = value; select.append(option);
    }
    select.value = m.kind ?? (m.pedal ? "pedal" : "manual");
    select.addEventListener("change", () => e.organCommand(commands.organManualKind(m.idx, select.value)));
    type.append(select); this.index.append(type);
    this.action("Connect keyboard / MIDI input", () => e.openMidiForm(m.idx, 20, 80), "Devices, channels and input range");
    this.action("Key range", () => e.openCompassForm(m.idx, 20, 80), "The lowest and highest playable keys");
    this.action("Division tuning", () => e.openTuningForm({ kind: "division", idx: m.idx }, 20, 80), "Follow the instrument or choose a tuning");
    if (m.kind === "microtonal") this.action("Hex layout", () => e.openHexForm(m.idx, 20, 80));
    this.action("Add stops", () => {
      e.openDivisionMenu(m.idx, this.back);
      e.showDivisionStops(e.el.divisionMenu, m);
    }, "Choose sounds from your sample sets");
    this.action("Remove keyboard…", () => e.showRemoveConfirm("manual", {
      idx: m.idx, name: m.name, stopCount: s.stops.filter(stop => stop.midx === m.idx).length,
    }), "Also removes the stops assigned to this keyboard").classList.add("danger");
  }

  render() {
    const s = this.editor.lastSnapshot;
    if (!s) return;
    const e = this.editor;
    this.index.replaceChildren();
    if (this.category === "keyboards" && this.keyboard != null) { this.renderKeyboard(s); return; }
    const category = CATEGORIES.find(([id]) => id === this.category);
    this.heading(category?.[1] ?? "Instrument");
    this.index.append(node("p", category?.[2] ?? "", "pane-note section-description"));
    if (this.category === "general") {
      this.rename("Organ name", s.organ, name => e.send(commands.organRename(name)));
      this.action("Tuning & pitch", () => e.openTuningForm("organ", 20, 80), "Temperament, reference pitch and transposition");
      this.action("Room & noises", () => e.openRoomForm(20, 80), "Reverb and mechanical sounds");
      if (s.trems?.some(t => !t.wave)) this.action("Tremulant", () => e.openTremForm(20, 80), "Speed, depth and response");
      const boxes = node("details", "", "organ-pref-group");
      boxes.append(node("summary", "Swell boxes")); this.index.append(boxes);
      this.action("Add swell box", () => this.add("enc"), null, boxes);
      for (const box of s.enclosures ?? []) this.action(`Remove ${box.name}…`, () => e.showRemoveConfirm("enclosure", {
        name: box.name, stopCount: s.stops.filter(stop => (stop.enc ?? []).includes(box.idx)).length,
      }), "Stops remain in the organ", boxes);
      this.action(s.setup?.file ? "Save a copy…" : "Save organ…", () => s.setup?.file ? e.openSaveAsForm() : e.openSaveForm(), "Keep a named version of this instrument");
    } else if (this.category === "keyboards") {
      this.action("Add keyboard", () => this.add("manual"), "Hand keyboard, pedalboard or microtonal layout").classList.add("organ-pref-add");
      for (const m of s.manuals ?? []) this.action(m.name, () => this.openKeyboard(m.idx),
        `${m.kind === "pedal" ? "Pedalboard" : m.kind === "microtonal" ? "Microtonal keyboard" : "Hand keyboard"} · ${m.key_count} keys`);
    } else if (this.category === "stops") {
      this.renderStops(s);
    } else if (this.category === "couplers") {
      this.action("Add or restore coupler", () => this.add("coupler")).classList.add("organ-pref-add");
      for (const c of s.couplers ?? []) {
        if (!c.hidden) this.action(c.name, () => e.openCouplerForm(c.idx, 20, 80), "Routes, key ranges and behavior");
      }
      this.action("Coupler options", () => e.openCouplersMenu(20, 80), "Placement and out-of-range notes");
    } else if (this.category === "sources") {
      this.action("Add sample set", () => this.add("source")).classList.add("organ-pref-add");
      this.renderSources(s);
    } else if (this.category === "bindings") {
      this.action("Edit buttons & shortcuts", () => e.openBindingsForm(20, 80), "View all assignments, or listen for a MIDI control");
    }
  }

  renderStops(s) {
    const e = this.editor;
    if (!s.stops?.length) {
      this.index.append(node("p", "No stops yet. Add a sample set, then choose its stops from a keyboard’s settings.", "pane-note"));
      this.action("Open keyboards", () => this.select("keyboards"));
      return;
    }
    const search = node("input");
    search.type = "search"; search.placeholder = "Find a stop or division";
    search.setAttribute("aria-label", "Find a stop"); search.value = this.stopSearch ?? "";
    this.index.append(search);
    const list = node("div", "", "organ-pref-list"); this.index.append(list);
    const filter = () => {
      this.stopSearch = search.value;
      list.replaceChildren();
      const matches = s.stops.filter(stop => `${stop.name} ${stop.manual}`.toLocaleLowerCase().includes(search.value.toLocaleLowerCase()));
      for (const stop of matches) this.action(stop.name, () => e.openStopForm(stop.id, 20, 80), stop.manual, list);
      if (!matches.length) list.append(node("p", "No stops match your search.", "pane-note"));
    };
    search.addEventListener("input", filter); filter();
    const pipes = node("details", "", "organ-pref-group");
    pipes.append(node("summary", "Individual pipes")); this.index.append(pipes);
    const stopSelect = node("select"); stopSelect.setAttribute("aria-label", "Stop for pipe voicing");
    for (const stop of s.stops) { const option = node("option", `${stop.manual} · ${stop.name}`); option.value = stop.id; stopSelect.append(option); }
    pipes.append(stopSelect);
    const low = node("input"), high = node("input");
    for (const [label, input] of [["First key number", low], ["Last key number", high]]) {
      input.type = "number"; input.required = true; input.min = "0"; input.max = "65535";
      input.setAttribute("aria-label", label);
      const field = node("label", label); field.append(input); pipes.append(field);
    }
    const reset = () => {
      const stop = s.stops.find(v => v.id === Number(stopSelect.value));
      const manual = s.manuals.find(m => m.idx === stop?.midx);
      low.value = high.value = String(manual?.first_key ?? 0);
    };
    stopSelect.addEventListener("change", reset); reset();
    this.action("Edit selected pipes", () => {
      if (!low.reportValidity() || !high.reportValidity()) return;
      const id = Number(stopSelect.value);
      e.openStopForm(id, 20, 80);
      e.openKeyVoicing(id, [Math.min(+low.value, +high.value), Math.max(+low.value, +high.value)], 20, 80);
    }, null, pipes);
  }

  async renderSources(snapshot) {
    const list = node("div"); this.index.append(list);
    list.append(node("p", "Reading sample sets…", "pane-note"));
    // Own this request; the console drawer's background refresh may supersede
    // its own offerings request, but must not erase this page's result.
    const { ok, data, error } = await localFetch(this.editor.base, commands.organOfferings(), { json: true });
    if (!list.isConnected || snapshot.setup?.file !== this.editor.lastSnapshot?.setup?.file) return;
    list.replaceChildren();
    if (!ok) {
      list.append(node("p", error || "Could not read sample sets.", "pane-note"));
      this.action("Try again", () => this.render(), null, list);
      return;
    }
    for (const source of data.sources ?? []) {
      const group = node("section", "", "organ-pref-source"); list.append(group);
      this.heading(source.name ?? source.alias, group);
      group.append(node("p", source.path, "pane-note source-path"));
      this.action("Sample set tuning", () => this.editor.openTuningForm({ kind: "source", alias: source.alias }, 20, 80), "Follow the instrument or preserve this set’s tuning", group);
    }
    if (!data.sources?.length) list.append(node("p", "No sample sets added yet.", "pane-note"));
  }

  rename(label, value, save, target = this.index) {
    const form = node("form", "", "organ-pref-rename");
    const field = node("label", label);
    const input = node("input"); input.value = value; input.required = true;
    input.setAttribute("aria-label", label);
    field.append(input);
    const button = node("button", "Rename", "ghost"); button.type = "submit";
    form.append(field, button);
    form.addEventListener("submit", event => {
      event.preventDefault();
      const name = input.value.trim();
      if (name && name !== value) { save(name); input.blur(); }
    });
    target.append(form);
  }
}
