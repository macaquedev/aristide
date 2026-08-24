// The organ-structure editor: a Max-MSP-style unlockable patch, not a
// dialog. Locked, the console behaves exactly as it always has, except
// that a ctrl-drag still edits — that's the "reach through the lock"
// gesture the rest of this module exists to serve. Unlocked, plain
// drags do the same thing, panels move by their title bars, and
// double-clicking empty canvas adds to the organ right there.
//
// This owns the editing chrome (padlock, drawer, bin, hint, the add
// popovers, the rebuild status strip) and decorates the DOM Console
// already built — it never builds jambs or keyboards itself.
// `decorateConsole(snapshot)` is called by Console right after every
// structural rebuild (see console.js's `decorate` hook); `update(snapshot)`
// is called on every poll, the same as Preferences and the other panels.

import { commands } from "./api.js";
import { parseKeyName } from "./pitch.js";

// What the native add-source dialog offers. Narrower than the picker's
// filter: a source must be a sample set — the server refuses another
// organ file — so Aristide's own .toml composites are left out.
const SET_FILTER = {
  name: "Sample sets (GrandOrgue, Hauptwerk)",
  extensions: ["organ", "Organ_Hauptwerk_xml"],
};

/// The keyboard context menu's "Change type" radio group, in the order
/// they're offered — the same vocabulary the add menu and the server's
/// `kind=` param share.
const KEYBOARD_KINDS = [
  ["manual", "Manual"],
  ["pedal", "Pedalboard"],
  ["microtonal", "Microtonal keyboard"],
];

/// Swallows the click a suppressed drag would otherwise leave behind —
/// a drag that crossed the threshold must not also toggle the drawknob
/// (or fire whatever else the element's own click listener does).
function suppressClick(event) {
  event.preventDefault();
  event.stopImmediatePropagation();
}

/// Anything with behavior of its own — a panel drag must never start on
/// these, or a drawknob could not be clicked and a key could not play.
const INTERACTIVE = ".knob, .key, .cheek, .rocker, .shoe, button, input, select, textarea";

export class Editor {
  constructor(root, base, send) {
    this.root = root;
    this.base = base;
    this.send = send;
    this.unlocked = false;
    this.drawerOpen = false;
    this.drag = null; // the live structural drag, if any — see startDrag()
    this.panelDrag = null; // the live panel move, if any
    this.lastSnapshot = null;
    this.autoUnlockedFor = null; // organ name already auto-unlocked once
    this.offerings = null;
    this.offeringsFile = null; // setup.file the cached offerings were fetched for
    this.renamingManual = null; // manual idx whose cheek is a rename input
    this.pendingRemove = null; // {kind: "manual"|"enclosure", ...} awaiting confirm
    this.pendingPlace = null; // {name, x, y}: place this manual's panels once it lands
    this.addAnchor = null; // where the add popover was opened, in px
    this.addKind = "manual"; // "manual" | "pedal" | "microtonal" — the add-manual form's target
    this.tuningManual = null; // manual idx the tuning popover is open for, or null
    this.tuningBrowseKind = null; // "scale" | "keymap" | null — the tuning form's own file browser
    this.tuningBrowseDir = null;
    this.tuningBrowseParent = null;
    this.tuningBrowseEntries = null;
    this.tuningBrowseError = null;
    this.addBrowseDir = null;
    this.addBrowseParent = null;
    this.addBrowseEntries = null;
    this.addBrowseError = null;
    this._lockNoteTimer = null;

    this.el = {
      lock: root.getElementById("editor-lock"),
      lockGlyph: root.getElementById("editor-lock-glyph"),
      lockNote: root.getElementById("editor-lock-note"),
      hint: root.getElementById("editor-hint"),
      status: root.getElementById("editor-status"),
      statusText: root.getElementById("editor-status-text"),
      error: root.getElementById("editor-error"),
      canvas: root.getElementById("console-canvas"),
      emptyCard: root.getElementById("organ-empty-card"),
      drawerTab: root.getElementById("editor-drawer-tab"),
      drawer: root.getElementById("editor-drawer"),
      drawerClose: root.getElementById("editor-drawer-close"),
      offerings: root.getElementById("editor-offerings"),
      bin: root.getElementById("editor-bin"),
      removeConfirm: root.getElementById("editor-remove-confirm"),
      removeConfirmText: root.getElementById("editor-remove-confirm-text"),
      removeConfirmYes: root.getElementById("editor-remove-confirm-yes"),
      removeConfirmNo: root.getElementById("editor-remove-confirm-no"),
      add: root.getElementById("editor-add"),
      addMenu: root.getElementById("editor-add-menu"),
      addManual: root.getElementById("editor-add-manual"),
      addPedal: root.getElementById("editor-add-pedal"),
      addMicrotonal: root.getElementById("editor-add-microtonal"),
      addEnc: root.getElementById("editor-add-enc"),
      addSource: root.getElementById("editor-add-source"),
      addManualForm: root.getElementById("editor-add-manual-form"),
      addManualName: root.getElementById("editor-add-manual-name"),
      addManualLow: root.getElementById("editor-add-manual-low"),
      addManualHigh: root.getElementById("editor-add-manual-high"),
      addManualCancel: root.getElementById("editor-add-manual-cancel"),
      addEncForm: root.getElementById("editor-add-enc-form"),
      addEncName: root.getElementById("editor-add-enc-name"),
      addEncCancel: root.getElementById("editor-add-enc-cancel"),
      addSourceForm: root.getElementById("editor-add-source-form"),
      addSourcePath: root.getElementById("editor-add-source-path"),
      addSourceAdd: root.getElementById("editor-add-source-add"),
      addSourceCancel: root.getElementById("editor-add-source-cancel"),
      addBrowseUp: root.getElementById("editor-add-browse-up"),
      addBrowseDir: root.getElementById("editor-add-browse-dir"),
      addBrowseError: root.getElementById("editor-add-browse-error"),
      addBrowseList: root.getElementById("editor-add-browse-list"),
      divisionMenu: root.getElementById("editor-division-menu"),
      keyboardMenu: root.getElementById("editor-keyboard-menu"),
      tuning: root.getElementById("editor-tuning"),
      tuningForm: root.getElementById("editor-tuning-form"),
      tuningTitle: root.getElementById("editor-tuning-title"),
      tuningReset: root.getElementById("editor-tuning-reset"),
      tuningScalePick: root.getElementById("editor-tuning-scale-pick"),
      tuningScaleActive: root.getElementById("editor-tuning-scale-active"),
      tuningScaleName: root.getElementById("editor-tuning-scale-name"),
      tuningScaleClear: root.getElementById("editor-tuning-scale-clear"),
      tuningKeymapRow: root.getElementById("editor-tuning-keymap-row"),
      tuningKeymapName: root.getElementById("editor-tuning-keymap-name"),
      tuningKeymapPick: root.getElementById("editor-tuning-keymap-pick"),
      tuningKeymapClear: root.getElementById("editor-tuning-keymap-clear"),
      tuningTemperamentRow: root.getElementById("editor-tuning-temperament-row"),
      tuningTemperament: root.getElementById("editor-tuning-temperament"),
      tuningA4: root.getElementById("editor-tuning-a4"),
      tuningTranspose: root.getElementById("editor-tuning-transpose"),
      tuningError: root.getElementById("editor-tuning-error"),
      tuningClose: root.getElementById("editor-tuning-close"),
      tuningBrowse: root.getElementById("editor-tuning-browse"),
      tuningBrowseTitle: root.getElementById("editor-tuning-browse-title"),
      tuningBrowseUp: root.getElementById("editor-tuning-browse-up"),
      tuningBrowseDir: root.getElementById("editor-tuning-browse-dir"),
      tuningBrowseError: root.getElementById("editor-tuning-browse-error"),
      tuningBrowseList: root.getElementById("editor-tuning-browse-list"),
      tuningBrowseCancel: root.getElementById("editor-tuning-browse-cancel"),
    };

    this.wireLock();
    this.wireDrawer();
    this.wireRemoveConfirm();
    this.wireAdd();
    this.wireTuningForm();
    this.wireCanvas();
  }

  // ---- the padlock ---------------------------------------------------------

  wireLock() {
    this.el.lock.addEventListener("click", () => this.togglePadlock());
    window.addEventListener("keydown", (event) => {
      if (event.key.toLowerCase() !== "e" || !(event.ctrlKey || event.metaKey)) return;
      if (event.target instanceof HTMLInputElement || event.target instanceof HTMLTextAreaElement) return;
      event.preventDefault();
      this.togglePadlock();
    });
  }

  togglePadlock() {
    if (this.unlocked) {
      this.lock();
      return;
    }
    // An ad-hoc combination has no file for a structural edit to write
    // into — the server would 400 on the first one. Say so instead of
    // unlocking into affordances that can't work yet.
    if (this.lastSnapshot?.setup?.implicit) {
      this.showLockNote(
        "This organ hasn't been saved as a file yet — save the combination " +
          "first (organ menu), then its structure can be edited here."
      );
      return;
    }
    this.unlock();
  }

  unlock() {
    this.unlocked = true;
    this.hideLockNote();
    this.root.body.classList.add("editing");
    this.el.lock.classList.add("on");
    this.el.lock.setAttribute("aria-pressed", "true");
    this.el.lock.setAttribute("aria-label", "Lock editing");
    this.el.lock.dataset.tip = "Lock editing (Ctrl+E)";
    this.el.lockGlyph.innerHTML = "&#128275;"; // open padlock
    this.el.hint.classList.remove("hidden");
    this.el.drawerTab.classList.remove("hidden");
  }

  lock() {
    this.unlocked = false;
    this.root.body.classList.remove("editing");
    this.el.lock.classList.remove("on");
    this.el.lock.setAttribute("aria-pressed", "false");
    this.el.lock.setAttribute("aria-label", "Unlock editing");
    this.el.lock.dataset.tip = "Unlock editing (Ctrl+E)";
    this.el.lockGlyph.innerHTML = "&#128274;"; // closed padlock
    this.el.hint.classList.add("hidden");
    this.el.drawerTab.classList.add("hidden");
    this.closeDrawer();
    this.closeAdd();
    this.closeDivisionMenu();
    this.closeKeyboardMenu();
    this.closeTuningForm();
  }

  // A double-click on a locked canvas is someone reaching for the add
  // gesture — silence would read as "there is no such gesture", so the
  // padlock answers instead.
  nudgeUnlock() {
    this.showLockNote(
      "The console is locked — click the padlock (Ctrl+E) to edit, " +
        "or hold Ctrl to reach through it."
    );
  }

  showLockNote(text) {
    this.el.lockNote.textContent = text;
    this.el.lockNote.classList.remove("hidden");
    clearTimeout(this._lockNoteTimer);
    this._lockNoteTimer = setTimeout(() => this.hideLockNote(), 6000);
  }

  hideLockNote() {
    this.el.lockNote.classList.add("hidden");
  }

  // ---- per-poll state --------------------------------------------------

  /// Called on every snapshot, structural rebuild or not — the rebuild
  /// status, the empty-organ auto-unlock, queued edits and offerings
  /// staleness all need to track fields (`loading`, `setup.file`) that
  /// don't necessarily change Console's own structural signature.
  update(snapshot) {
    this.lastSnapshot = snapshot;

    const empty = !!snapshot.organ && !snapshot.stops?.length && !snapshot.manuals?.length;
    if (empty && this.autoUnlockedFor !== snapshot.organ) {
      this.autoUnlockedFor = snapshot.organ;
      this.unlock();
    }

    const showStatus = !!snapshot.organ && !!snapshot.loading;
    this.el.status.classList.toggle("hidden", !showStatus);
    this.el.statusText.textContent = snapshot.loading ?? "";

    // A load that failed with the picker closed (an organ picked from
    // the menu's Recent list) would otherwise fail into silence: the
    // picker shows load_error only while it is up, so the error strip
    // carries it here. Warnings ride along — an organ whose file lines
    // were healed over loads emptier than the file intends, and that
    // must say so where the player is looking. Only transitions matter
    // — repainting on every poll would clobber the strip's own
    // transient command errors.
    const warnings = snapshot.load_warnings ?? [];
    const loadError =
      snapshot.load_error ??
      (warnings.length
        ? `the organ loaded, but ${warnings.length} line${
            warnings.length === 1 ? "" : "s"
          } of its file did not resolve — e.g. ${warnings[0]}`
        : null);
    if (loadError !== this.lastLoadError) {
      this.lastLoadError = loadError;
      loadError ? this.showError(loadError) : this.hideError();
    }

    const file = snapshot.setup?.file ?? null;
    if (file !== this.offeringsFile) {
      this.offeringsFile = file;
      this.offerings = null;
      if (this.drawerOpen) this.fetchOfferings();
    }

    if (this.tuningManual != null) this.syncTuningForm();
  }

  /// Called by Console right after every structural rebuild (see its
  /// `decorate` hook) — wires drag sources, drop targets and the
  /// editing chrome onto the DOM it just built. Nothing here duplicates
  /// Console's own rendering; it only adds listeners and small controls.
  decorateConsole(snapshot) {
    this.lastSnapshot = snapshot;
    const empty = !!snapshot.organ && !snapshot.stops.length && !snapshot.manuals.length;
    if (empty) return; // the empty card has its own single button, wired by Console
    this.tagManualTargets();
    this.wireStopDrags(snapshot);
    this.wireCheekDrags(snapshot);
    this.wireShoeDrags(snapshot);
    this.wireCheekRename();
    this.wireKeyboardContextMenu();
    this.wirePanelMoves(snapshot);
    this.addDivisionButtons(snapshot);
    this.placePending(snapshot);
  }

  /// Every keyboard and every jamb division carries its manual index in
  /// the DOM, so both are drop targets — including empty divisions,
  /// which is precisely where a new manual's first stop goes.
  tagManualTargets() {
    for (const board of this.root.querySelectorAll(".keyboard[data-manual]")) {
      board.dataset.dropManual = board.dataset.manual;
    }
    for (const division of this.root.querySelectorAll(".division[data-division]")) {
      division.dataset.dropManual = division.dataset.division;
    }
  }

  wireStopDrags() {
    for (const knob of this.root.querySelectorAll('.knob[data-key^="stop-"]')) {
      const id = Number(knob.dataset.key.slice("stop-".length));
      knob.title = "Ctrl-drag to move or enclose this stop — or unlock to drag it plain.";
      this.wireDragSource(knob, () => {
        const stop = this.lastSnapshot?.stops.find((s) => s.id === id);
        if (!stop) return null;
        return { kind: "stop", payload: { id: stop.id, midx: stop.midx, name: stop.name }, label: stop.name };
      });
    }
  }

  wireCheekDrags() {
    for (const board of this.root.querySelectorAll(".keyboard[data-manual]")) {
      const idx = Number(board.dataset.manual);
      const cheek = board.querySelector(".cheek");
      if (!cheek) continue;
      cheek.title =
        "Ctrl-drag to reorder or remove this manual — unlock to drag it plain. Double-click to rename.";
      this.wireDragSource(cheek, () => {
        const manual = this.lastSnapshot?.manuals.find((m) => m.idx === idx);
        if (!manual) return null;
        const stopCount = (this.lastSnapshot?.stops ?? []).filter((s) => s.midx === idx).length;
        return { kind: "manual", payload: { idx, name: manual.name, stopCount }, label: manual.name };
      });
    }
  }

  wireShoeDrags() {
    for (const shoe of this.root.querySelectorAll(".shoe[data-enclosure]")) {
      const idx = Number(shoe.dataset.enclosure);
      const label = shoe.querySelector(".shoe-label");
      if (!label) continue;
      label.title = "Ctrl-drag to the bin to remove this swell box — its stops stay, unenclosed.";
      this.wireDragSource(label, () => {
        const enclosure = this.lastSnapshot?.enclosures.find((e) => e.idx === idx);
        if (!enclosure) return null;
        const stopCount = (this.lastSnapshot?.stops ?? []).filter((s) => (s.enc ?? []).includes(idx)).length;
        return { kind: "enclosure", payload: { name: enclosure.name, stopCount }, label: enclosure.name };
      });
    }
  }

  wireCheekRename() {
    for (const board of this.root.querySelectorAll(".keyboard[data-manual]")) {
      const idx = Number(board.dataset.manual);
      const cheek = board.querySelector(".cheek");
      if (!cheek) continue;
      cheek.addEventListener("dblclick", (event) => {
        if (!this.unlocked && !event.ctrlKey) return;
        this.startManualRename(idx);
      });
    }
  }

  /// Right-click a keyboard panel — locked, it's the same nudge as any
  /// other reach at editing; unlocked, it opens the kind/tuning menu
  /// right there, the same popover idiom as the division "+".
  wireKeyboardContextMenu() {
    for (const board of this.root.querySelectorAll(".keyboard[data-manual]")) {
      const idx = Number(board.dataset.manual);
      board.addEventListener("contextmenu", (event) => {
        event.preventDefault();
        event.stopPropagation();
        if (!this.unlocked) {
          this.nudgeUnlock();
          return;
        }
        this.openKeyboardMenu(idx, event.clientX, event.clientY);
      });
    }
  }

  startManualRename(idx) {
    if (this.renamingManual === idx) return;
    const board = this.root.querySelector(`.keyboard[data-manual="${idx}"]`);
    const cheek = board?.querySelector(".cheek");
    const manual = this.lastSnapshot?.manuals.find((m) => m.idx === idx);
    if (!board || !cheek || !manual) return;
    this.renamingManual = idx;
    cheek.style.visibility = "hidden";

    const input = document.createElement("input");
    input.className = "editor-cheek-rename";
    input.value = manual.name;
    input.setAttribute("aria-label", `Rename ${manual.name}`);

    const commit = () => {
      if (this.renamingManual !== idx) return;
      this.renamingManual = null;
      input.remove();
      cheek.style.visibility = "";
      const name = input.value.trim();
      if (name && name !== manual.name) this.organCommand(commands.organManualRename(idx, name));
    };
    const abandon = () => {
      this.renamingManual = null;
      input.remove();
      cheek.style.visibility = "";
    };
    input.addEventListener("keydown", (event) => {
      event.stopPropagation(); // never falls through to a key binding
      if (event.key === "Enter") {
        event.preventDefault();
        commit();
      } else if (event.key === "Escape") {
        event.preventDefault();
        abandon();
      }
    });
    input.addEventListener("blur", commit);
    board.append(input);
    requestAnimationFrame(() => {
      input.focus();
      input.select();
    });
  }

  // ---- moving panels ------------------------------------------------------
  //
  // Every panel moves by its title bar when unlocked, and a ctrl-drag
  // anywhere on a panel that isn't a control moves it even locked —
  // "ctrl-drag anything" holds for panels too. The move is applied
  // live in pixels and persisted on release as fractions of the canvas
  // (POST /api/organ/panel/place), so it lands in the organ file.

  wirePanelMoves() {
    for (const panel of this.el.canvas.querySelectorAll(".panel")) {
      const chrome = panel.querySelector(".panel-chrome");
      chrome?.addEventListener("pointerdown", (event) => {
        if (event.button !== 0) return;
        this.startPanelDrag(panel, event);
      });
      panel.addEventListener("pointerdown", (event) => {
        if (event.button !== 0) return;
        if (!(event.ctrlKey || this.unlocked)) return;
        if (event.target.closest(INTERACTIVE)) return;
        if (event.target.closest(".panel-chrome")) return; // chrome handled above
        this.startPanelDrag(panel, event);
      });
    }
  }

  startPanelDrag(panel, event) {
    event.preventDefault();
    const rect = panel.getBoundingClientRect();
    const canvasRect = this.el.canvas.getBoundingClientRect();
    this.panelDrag = {
      panel,
      dx: event.clientX - rect.left,
      dy: event.clientY - rect.top,
      canvasRect,
      moved: false,
    };
    const move = (e) => this.panelDragMove(e);
    const up = (e) => {
      window.removeEventListener("pointermove", move);
      this.endPanelDrag(e);
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up, { once: true });
  }

  panelDragMove(event) {
    const drag = this.panelDrag;
    if (!drag) return;
    if (!drag.moved) {
      drag.moved = true;
      drag.panel.dataset.dragging = "1";
      drag.panel.classList.add("dragging");
    }
    const { canvasRect, panel } = drag;
    const w = panel.offsetWidth;
    const h = panel.offsetHeight;
    const x = Math.min(canvasRect.width - w, Math.max(0, event.clientX - canvasRect.left - drag.dx));
    const y = Math.min(canvasRect.height - h, Math.max(0, event.clientY - canvasRect.top - drag.dy));
    panel.style.left = `${Math.round(x)}px`;
    panel.style.top = `${Math.round(y)}px`;
  }

  endPanelDrag() {
    const drag = this.panelDrag;
    this.panelDrag = null;
    if (!drag || !drag.moved) return;
    const { panel, canvasRect } = drag;
    delete panel.dataset.dragging;
    panel.classList.remove("dragging");
    const x = parseFloat(panel.style.left) / canvasRect.width;
    const y = parseFloat(panel.style.top) / canvasRect.height;
    this.organCommand(commands.organPanelPlace(panel.dataset.panel, x, y));
  }

  // ---- the per-division "+" ------------------------------------------------

  /// Each jamb division gets a small + beside its name while editing:
  /// add a stop from what the sources offer, or throw the division's
  /// stops into a swell box of their own.
  addDivisionButtons(snapshot) {
    for (const head of this.el.canvas.querySelectorAll(".division-head")) {
      const idx = Number(head.parentElement.dataset.division);
      const button = document.createElement("button");
      button.type = "button";
      button.className = "division-add";
      button.textContent = "+";
      const manual = snapshot.manuals.find((m) => m.idx === idx);
      button.setAttribute("aria-label", `Add to ${manual?.name ?? "this division"}`);
      button.addEventListener("click", (event) => {
        event.stopPropagation();
        this.openDivisionMenu(idx, button);
      });
      head.append(button);
    }
  }

  openDivisionMenu(idx, anchor) {
    const menu = this.el.divisionMenu;
    menu.replaceChildren();
    this.buildDivisionMenuItems(menu, idx);
    menu.classList.remove("hidden");
    const rect = anchor.getBoundingClientRect();
    this.positionPopover(menu, rect.right + 6, rect.top);
  }

  buildDivisionMenuItems(menu, idx) {
    const snapshot = this.lastSnapshot;
    const manual = snapshot?.manuals.find((m) => m.idx === idx);
    if (!manual) return;

    const addStop = document.createElement("button");
    addStop.className = "menu-item";
    addStop.innerHTML = "<span>Add a stop&hellip;</span>";
    addStop.addEventListener("click", async (event) => {
      event.stopPropagation();
      await this.showDivisionStops(menu, manual);
    });
    menu.append(addStop);

    // "No enclosure already": none of this division's stops are boxed
    // and no box carries its name. The box takes the whole division.
    const stops = (snapshot.stops ?? []).filter((s) => s.midx === idx);
    const enclosed = stops.some((s) => (s.enc ?? []).length);
    const named = (snapshot.enclosures ?? []).some((e) => e.name === manual.name);
    if (stops.length && !enclosed && !named) {
      const addBox = document.createElement("button");
      addBox.className = "menu-item";
      addBox.innerHTML = "<span>Enclose in a swell box</span>";
      addBox.addEventListener("click", () => {
        this.closeDivisionMenu();
        // One rebuild each; runQueue waits each rebuild out.
        this.runQueue([
          commands.organEnclosureAdd(manual.name),
          ...stops.map((stop) => commands.organEnclosureAssign(manual.name, stop.id, true)),
        ]);
      });
      menu.append(addBox);
    }
  }

  /// Swap the division menu's items for a pick-list of every stop the
  /// sources still offer; clicking one pulls it onto this manual. The
  /// list stays open so a division can be registered in one visit.
  async showDivisionStops(menu, manual) {
    menu.replaceChildren(this.emptyNote("Reading the sources…"));
    if (!this.offerings) await this.fetchOfferings(false);
    menu.replaceChildren();
    const sources = this.offerings;
    if (sources == null) {
      menu.append(this.emptyNote("Couldn't read this organ's sources."));
      return;
    }
    let any = false;
    for (const source of sources) {
      for (const srcManual of source.manuals ?? []) {
        const remaining = (srcManual.stops ?? []).filter((s) => !s.pulled);
        if (!remaining.length) continue;
        any = true;
        const group = document.createElement("div");
        group.className = "division-add-group";
        const title = document.createElement("span");
        title.className = "organ-stop-group-title";
        title.textContent = `${source.alias} · ${srcManual.name}`;
        group.append(title);
        for (const stop of remaining) {
          const row = document.createElement("button");
          row.type = "button";
          row.className = "menu-item";
          row.innerHTML = `<span>${stop.name}</span>`;
          row.addEventListener("click", (event) => {
            event.stopPropagation();
            row.disabled = true; // optimistic: pulled now
            this.organCommand(
              commands.organPull(source.alias, srcManual.name, manual.name, stop.name)
            );
          });
          group.append(row);
        }
        menu.append(group);
      }
    }
    if (!any) {
      menu.append(
        this.emptyNote("The sources have nothing left to offer — add a sample set first.")
      );
    }
  }

  closeDivisionMenu() {
    this.el.divisionMenu.classList.add("hidden");
    this.el.divisionMenu.replaceChildren();
  }

  // ---- the keyboard context menu: change a manual's kind or tuning --------
  //
  // Right-click a keyboard panel while unlocked: a radio group of the
  // three kinds (picking a different one is a structural edit, same
  // contract as the add menu) and a way into the tuning popover below.

  openKeyboardMenu(idx, x, y) {
    this.closeAdd();
    this.closeDivisionMenu();
    this.closeTuningForm();
    const menu = this.el.keyboardMenu;
    menu.replaceChildren();
    this.buildKeyboardMenuItems(menu, idx);
    menu.classList.remove("hidden");
    this.positionPopover(menu, x, y);
  }

  buildKeyboardMenuItems(menu, idx) {
    const manual = this.lastSnapshot?.manuals.find((m) => m.idx === idx);
    if (!manual) return;
    const currentKind = manual.kind ?? (manual.pedal ? "pedal" : "manual");

    const heading = document.createElement("span");
    heading.className = "menu-heading";
    heading.textContent = "Change type";
    menu.append(heading);

    for (const [kind, label] of KEYBOARD_KINDS) {
      const item = document.createElement("button");
      item.className = "menu-item radio";
      item.classList.toggle("checked", kind === currentKind);
      item.innerHTML = `<span>${label}</span>`;
      item.addEventListener("click", (event) => {
        event.stopPropagation();
        this.closeKeyboardMenu();
        if (kind !== currentKind) this.organCommand(commands.organManualKind(idx, kind));
      });
      menu.append(item);
    }

    menu.append(document.createElement("hr"));

    const tuning = document.createElement("button");
    tuning.className = "menu-item";
    tuning.innerHTML = "<span>Change tuning&hellip;</span>";
    tuning.addEventListener("click", (event) => {
      event.stopPropagation();
      const rect = menu.getBoundingClientRect();
      this.closeKeyboardMenu();
      this.openTuningForm(idx, rect.left, rect.top);
    });
    menu.append(tuning);
  }

  closeKeyboardMenu() {
    this.el.keyboardMenu.classList.add("hidden");
    this.el.keyboardMenu.replaceChildren();
  }

  // ---- the tuning popover: this manual's own pitch, apart from the --------
  // instrument's, applied live field by field — never a rebuild. A
  // Scala scale (and its optional keymap) is just another field on the
  // same /api/tuning contract; picking one supersedes the temperament.
  //
  // Every field goes through `tuningCommand` rather than the plain
  // `send()` the rest of the console uses: a scale path can 400 (a bad
  // file, an unparseable one), and that reason needs to land in this
  // popover, not the app-wide status strip — see `showTuningError`.

  wireTuningForm() {
    this.el.tuningClose.addEventListener("click", () => this.closeTuningForm());

    this.el.tuningReset.addEventListener("click", () => {
      if (this.tuningManual == null) return;
      this.tuningCommand({ manual: this.tuningManual, reset: 1 });
    });

    this.el.tuningTemperament.addEventListener("change", () => {
      if (this.tuningManual == null) return;
      // Naming a temperament here is allowed even with a scale active —
      // the server reads it as leaving the scale (http.rs's /api/tuning
      // arm clears `tuning.scale` whenever `temperament` is given).
      this.tuningCommand({ manual: this.tuningManual, temperament: this.el.tuningTemperament.value });
      this.el.tuningTemperament.blur();
    });

    this.el.tuningA4.addEventListener("change", () => {
      if (this.tuningManual == null) return;
      const a4 = Math.min(500, Math.max(300, Number(this.el.tuningA4.value) || 440));
      this.el.tuningA4.value = a4;
      this.tuningCommand({ manual: this.tuningManual, a4 });
      this.el.tuningA4.blur();
    });

    this.el.tuningTranspose.addEventListener("change", () => {
      if (this.tuningManual == null) return;
      const transpose = Math.min(12, Math.max(-12, Math.round(Number(this.el.tuningTranspose.value) || 0)));
      this.el.tuningTranspose.value = transpose;
      this.tuningCommand({ manual: this.tuningManual, transpose });
      this.el.tuningTranspose.blur();
    });

    this.el.tuningScalePick.addEventListener("click", () => this.openTuningBrowse("scale"));
    this.el.tuningScaleClear.addEventListener("click", () => {
      if (this.tuningManual == null) return;
      this.tuningCommand({ manual: this.tuningManual, scale: "off" });
    });

    this.el.tuningKeymapPick.addEventListener("click", () => this.openTuningBrowse("keymap"));
    this.el.tuningKeymapClear.addEventListener("click", () => {
      if (this.tuningManual == null) return;
      const scl = this.currentScalePath();
      if (!scl) return;
      // An empty `keymap` param is indistinguishable, server-side, from
      // an omitted one (http.rs filters both to "no keymap") — sending
      // it explicitly just documents the intent here.
      this.tuningCommand({ manual: this.tuningManual, scale: scl, keymap: "" });
    });

    this.el.tuningBrowseUp.addEventListener("click", () => {
      if (this.tuningBrowseParent) this.tuningBrowse(this.tuningBrowseParent);
    });
    this.el.tuningBrowseCancel.addEventListener("click", () => this.closeTuningBrowse());
  }

  openTuningForm(idx, x, y) {
    this.closeAdd();
    this.closeDivisionMenu();
    this.closeKeyboardMenu();
    this.tuningManual = idx;
    this.hideTuningError();
    this.closeTuningBrowse();
    this.syncTuningForm();
    this.el.tuning.classList.remove("hidden");
    this.positionPopover(this.el.tuning, x, y);
  }

  closeTuningForm() {
    this.tuningManual = null;
    this.el.tuning.classList.add("hidden");
    this.hideTuningError();
    this.closeTuningBrowse();
  }

  /// This manual's effective tuning: its own override if the snapshot
  /// carries one, else whatever the instrument shares. Called on open
  /// and on every later poll, so a shared value another panel changes
  /// (or another session's edit) keeps the popover honest. Never touches
  /// the file-browser sub-view (see `openTuningBrowse`/`closeTuningBrowse`)
  /// — a poll landing mid-navigation must not yank it shut.
  syncTuningForm() {
    const idx = this.tuningManual;
    const manual = this.lastSnapshot?.manuals.find((m) => m.idx === idx);
    if (!manual) {
      this.closeTuningForm();
      return;
    }
    this.el.tuningTitle.textContent = manual.name;
    const own = (this.lastSnapshot?.manual_tuning ?? []).find((t) => t.idx === idx);
    this.el.tuningReset.classList.toggle("hidden", !own);
    const tuning = own ?? this.lastSnapshot?.tuning;
    if (!tuning) return;
    if (this.root.activeElement !== this.el.tuningTemperament) {
      this.el.tuningTemperament.value = tuning.temperament;
    }
    if (this.root.activeElement !== this.el.tuningA4) this.el.tuningA4.value = tuning.a4;
    if (this.root.activeElement !== this.el.tuningTranspose) this.el.tuningTranspose.value = tuning.transpose;

    const scale = tuning.scale ?? null;
    this.el.tuningScalePick.classList.toggle("hidden", !!scale);
    this.el.tuningScaleActive.classList.toggle("hidden", !scale);
    if (scale) {
      this.el.tuningScaleName.textContent = `${scale.name} · ${scale.notes} notes`;
      this.el.tuningScaleName.title = scale.scl;
    }

    this.el.tuningKeymapRow.classList.toggle("hidden", !scale);
    if (scale) {
      if (scale.kbm) {
        this.el.tuningKeymapName.textContent = scale.kbm.split("/").pop();
        this.el.tuningKeymapName.title = scale.kbm;
        this.el.tuningKeymapClear.classList.remove("hidden");
      } else {
        this.el.tuningKeymapName.textContent = "linear";
        this.el.tuningKeymapName.title = "";
        this.el.tuningKeymapClear.classList.add("hidden");
      }
    }

    // The scale IS the temperament while one is active — the select
    // stays live (choosing from it is a valid way back out) but reads
    // as superseded rather than in effect.
    this.el.tuningTemperamentRow.classList.toggle("tuning-dimmed", !!scale);
    this.el.tuningTemperament.title = scale
      ? "A scale is active — picking a temperament here leaves it"
      : "";
  }

  /// The manual's effective scale path right now, or null with none —
  /// what a keymap pick or clear re-sends alongside, since /api/tuning
  /// takes the scale and its keymap together (see http.rs).
  currentScalePath() {
    const idx = this.tuningManual;
    if (!this.lastSnapshot?.manuals.some((m) => m.idx === idx)) return null;
    const own = (this.lastSnapshot?.manual_tuning ?? []).find((t) => t.idx === idx);
    const tuning = own ?? this.lastSnapshot?.tuning;
    return tuning?.scale?.scl ?? null;
  }

  /// Sends a tuning field update directly (not through the app-wide
  /// `send()`), so a 400's reason can land in this popover instead of
  /// the global status strip — the same local-fetch idiom
  /// `organCommandResult` uses for structural edits.
  async tuningCommand(fields) {
    this.hideTuningError();
    try {
      const response = await fetch(this.base + commands.tuning(fields), { method: "POST" });
      if (!response.ok) {
        this.showTuningError((await response.text()) || `${response.status} ${response.statusText}`);
        return false;
      }
      return true;
    } catch (err) {
      this.showTuningError(String(err));
      return false;
    }
  }

  showTuningError(text) {
    this.el.tuningError.textContent = text;
    this.el.tuningError.classList.remove("hidden");
  }

  hideTuningError() {
    this.el.tuningError.classList.add("hidden");
    this.el.tuningError.textContent = "";
  }

  // ---- the tuning popover's own file browser: picks a .scl or .kbm --------
  // path, the same /api/browse idiom as the add-source browse, filtered
  // client-side to the relevant extension (directories stay navigable).

  openTuningBrowse(kind) {
    this.tuningBrowseKind = kind;
    this.tuningBrowseDir = null;
    this.tuningBrowseParent = null;
    this.tuningBrowseEntries = null;
    this.tuningBrowseError = null;
    this.el.tuningBrowseTitle.textContent = kind === "keymap" ? "Choose a keymap" : "Choose a scale";
    this.el.tuningForm.classList.add("hidden");
    this.el.tuningBrowse.classList.remove("hidden");
    this.tuningBrowse();
  }

  closeTuningBrowse() {
    this.tuningBrowseKind = null;
    this.el.tuningBrowse.classList.add("hidden");
    this.el.tuningForm.classList.remove("hidden");
  }

  async tuningBrowse(dir) {
    try {
      const query = dir ? `/api/browse?dir=${encodeURIComponent(dir)}` : "/api/browse";
      const response = await fetch(this.base + query);
      if (!response.ok) {
        this.tuningBrowseError = (await response.text()) || `${response.status} ${response.statusText}`;
        this.renderTuningBrowse();
        return;
      }
      const data = await response.json();
      this.tuningBrowseDir = data.dir;
      this.tuningBrowseParent = data.parent;
      this.tuningBrowseEntries = data.entries;
      this.tuningBrowseError = null;
      this.renderTuningBrowse();
    } catch (err) {
      this.tuningBrowseError = String(err);
      this.renderTuningBrowse();
    }
  }

  renderTuningBrowse() {
    this.el.tuningBrowseDir.textContent = this.tuningBrowseDir ?? "";
    this.el.tuningBrowseDir.title = this.tuningBrowseDir ?? "";
    this.el.tuningBrowseUp.disabled = !this.tuningBrowseParent;
    this.el.tuningBrowseError.classList.toggle("hidden", !this.tuningBrowseError);
    this.el.tuningBrowseError.textContent = this.tuningBrowseError ?? "";
    this.el.tuningBrowseList.replaceChildren();
    if (this.tuningBrowseError) return;
    const ext = this.tuningBrowseKind === "keymap" ? ".kbm" : ".scl";
    const entries = (this.tuningBrowseEntries ?? []).filter(
      (entry) => entry.dir || entry.name.toLowerCase().endsWith(ext)
    );
    if (!entries.length) {
      this.el.tuningBrowseList.append(this.emptyNote("Nothing here."));
      return;
    }
    for (const entry of entries) {
      const row = document.createElement("button");
      row.type = "button";
      row.className = entry.dir ? "picker-row picker-browse-dir" : "picker-row";
      row.title = entry.path;
      row.addEventListener("click", () => {
        if (entry.dir) this.tuningBrowse(entry.path);
        else this.pickTuningFile(entry.path);
      });
      const name = document.createElement("span");
      name.className = "picker-row-name";
      name.textContent = entry.name;
      row.append(name);
      this.el.tuningBrowseList.append(row);
    }
  }

  async pickTuningFile(path) {
    if (this.tuningManual == null) return;
    const fields =
      this.tuningBrowseKind === "keymap"
        ? { manual: this.tuningManual, scale: this.currentScalePath(), keymap: path }
        : { manual: this.tuningManual, scale: path };
    if (fields.scale == null) return;
    const ok = await this.tuningCommand(fields);
    if (ok) this.closeTuningBrowse();
  }

  // ---- drag controller: plain when unlocked, ctrl-drag always -------------
  //
  // Plain pointer events, not HTML5 drag-and-drop: a floating label
  // follows the pointer and the drop target is read straight off
  // `elementFromPoint`. Every drag source waits for ~4px of movement
  // before committing to a drag — below that it's a click (a drawknob
  // still toggles its stop; a cheek's dblclick still renames).

  binAllowed(kind) {
    return kind === "stop" || kind === "manual" || kind === "enclosure";
  }

  manualAllowed(kind) {
    return kind !== "enclosure";
  }

  encAllowed(kind) {
    return kind === "stop";
  }

  /// `getInfo()` returns `{kind, payload, label}` for the drag about to
  /// start, or null to refuse it. Called only once the pointer has
  /// actually moved past the threshold, so it can read live state.
  wireDragSource(el, getInfo) {
    el.addEventListener("pointerdown", (event) => {
      if (event.button !== 0) return;
      if (!(event.ctrlKey || this.unlocked)) return;
      event.stopPropagation(); // a control drag is never a panel move
      const startX = event.clientX;
      const startY = event.clientY;
      let moved = false;
      const onMove = (e) => {
        if (moved) return;
        if (Math.hypot(e.clientX - startX, e.clientY - startY) < 4) return;
        moved = true;
        window.removeEventListener("pointermove", onMove);
        const info = getInfo();
        if (!info) return;
        el.addEventListener("click", suppressClick, { capture: true, once: true });
        this.startDrag(e, info.kind, info.payload, info.label);
      };
      const onUp = () => window.removeEventListener("pointermove", onMove);
      window.addEventListener("pointermove", onMove);
      window.addEventListener("pointerup", onUp, { once: true });
    });
  }

  startDrag(event, kind, payload, label) {
    event.preventDefault();
    const ghost = document.createElement("div");
    ghost.className = "organ-drag-ghost";
    ghost.textContent = label;
    document.body.append(ghost);
    this.drag = { kind, payload, ghost, label, targetType: null, targetIdx: null };
    this.positionGhost(event.clientX, event.clientY);
    if (this.binAllowed(kind)) this.el.bin.classList.add("visible");
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
    const el = document.elementFromPoint(x, y);
    if (!el || !this.drag) return null;
    if (el.closest("[data-drop-bin]") && this.binAllowed(this.drag.kind)) return { type: "bin" };
    const shoe = el.closest(".shoe[data-enclosure]");
    if (shoe && this.encAllowed(this.drag.kind)) {
      return { type: "shoe", idx: Number(shoe.dataset.enclosure) };
    }
    const manual = el.closest("[data-drop-manual]");
    if (manual && this.manualAllowed(this.drag.kind)) {
      return { type: "manual", idx: Number(manual.dataset.dropManual) };
    }
    return null;
  }

  applyDropHighlight(hit) {
    for (const el of this.root.querySelectorAll(".drop-target")) el.classList.remove("drop-target");
    this.el.bin.classList.remove("drop-target");
    this.drag.targetType = hit?.type ?? null;
    this.drag.targetIdx = hit?.idx ?? null;
    this.drag.ghost.textContent = this.drag.label;
    if (!hit) return;

    if (hit.type === "bin") {
      this.el.bin.classList.add("drop-target");
      this.drag.ghost.textContent =
        this.drag.kind === "enclosure"
          ? `Remove the ${this.drag.label} box`
          : this.drag.kind === "manual"
            ? `Remove ${this.drag.label}`
            : `Drop to remove ${this.drag.label}`;
      return;
    }

    if (hit.type === "shoe") {
      const shoeEl = this.root.querySelector(`.shoe[data-enclosure="${hit.idx}"]`);
      shoeEl?.classList.add("drop-target");
      const enclosure = this.lastSnapshot?.enclosures.find((e) => e.idx === hit.idx);
      const stop = this.lastSnapshot?.stops.find((s) => s.id === this.drag.payload.id);
      if (enclosure) {
        const already = stop?.enc?.includes(hit.idx);
        this.drag.ghost.textContent = already
          ? `In ${enclosure.name} — drop to take out`
          : `Drop to add to ${enclosure.name}`;
      }
      return;
    }

    // Dropping a stop back on its own manual, or a manual's cheek on its
    // own board, isn't a move — no need to light it up as one.
    if (this.drag.kind === "stop" && hit.idx === this.drag.payload.midx) return;
    if (this.drag.kind === "manual" && hit.idx === this.drag.payload.idx) return;
    for (const el of this.root.querySelectorAll(`[data-drop-manual="${hit.idx}"]`)) {
      el.classList.add("drop-target");
    }
    const manual = this.lastSnapshot?.manuals.find((m) => m.idx === hit.idx);
    if (manual) this.drag.ghost.textContent = `${this.drag.label} → ${manual.name}`;
  }

  endDrag(event) {
    window.removeEventListener("pointermove", this._dragMove);
    const drag = this.drag;
    this.drag = null;
    if (!drag) return;
    drag.ghost.remove();
    this.el.bin.classList.remove("visible", "drop-target");
    for (const el of this.root.querySelectorAll(".drop-target")) el.classList.remove("drop-target");

    const { targetType, targetIdx } = drag;
    if (!targetType) return;

    if (drag.kind === "stop") {
      if (targetType === "bin") {
        this.organCommand(commands.organUnpull(drag.payload.id));
      } else if (targetType === "shoe") {
        const enclosure = this.lastSnapshot?.enclosures.find((e) => e.idx === targetIdx);
        const stop = this.lastSnapshot?.stops.find((s) => s.id === drag.payload.id);
        if (enclosure) {
          const already = stop?.enc?.includes(targetIdx);
          this.organCommand(commands.organEnclosureAssign(enclosure.name, drag.payload.id, !already));
        }
      } else if (targetType === "manual" && targetIdx !== drag.payload.midx) {
        // A live reassignment, not a rebuild — but the server refuses
        // it mid-rebuild (stale names would poison the file), so it
        // goes through the queue, which waits any rebuild out.
        this.runQueue([commands.organMove(drag.payload.id, targetIdx)]);
      }
    } else if (drag.kind === "manual") {
      if (targetType === "bin") {
        this.showRemoveConfirm("manual", drag.payload);
      } else if (targetType === "manual" && targetIdx !== drag.payload.idx) {
        this.organCommand(commands.organManualOrder(drag.payload.idx, targetIdx));
      }
    } else if (drag.kind === "enclosure" && targetType === "bin") {
      this.showRemoveConfirm("enclosure", drag.payload);
    } else if (drag.kind === "offering-stop" && targetType === "manual") {
      const manual = this.lastSnapshot?.manuals.find((m) => m.idx === targetIdx);
      if (manual) {
        this.organCommand(
          commands.organPull(drag.payload.alias, drag.payload.manualName, manual.name, drag.payload.stopName)
        );
      }
    } else if (drag.kind === "offering-division" && targetType === "manual") {
      const manual = this.lastSnapshot?.manuals.find((m) => m.idx === targetIdx);
      if (manual) this.organCommand(commands.organPull(drag.payload.alias, drag.payload.manualName, manual.name));
    }
  }

  // ---- removal: manuals and swell boxes, both confirmed the same way -----

  showRemoveConfirm(kind, payload) {
    this.pendingRemove = { kind, ...payload };
    const n = payload.stopCount;
    this.el.removeConfirmText.textContent =
      kind === "enclosure"
        ? `Remove the ${payload.name} box? Its stops stay, unenclosed.`
        : `Remove ${payload.name} and its ${n} stop${n === 1 ? "" : "s"}? ` +
          "Sources still offer everything.";
    this.el.removeConfirm.classList.remove("hidden");
  }

  hideRemoveConfirm() {
    this.pendingRemove = null;
    this.el.removeConfirm.classList.add("hidden");
  }

  wireRemoveConfirm() {
    this.el.removeConfirmYes.addEventListener("click", () => {
      const target = this.pendingRemove;
      this.hideRemoveConfirm();
      if (!target) return;
      if (target.kind === "enclosure") this.organCommand(commands.organEnclosureRemove(target.name));
      else this.organCommand(commands.organManualRemove(target.idx));
    });
    this.el.removeConfirmNo.addEventListener("click", () => this.hideRemoveConfirm());
  }

  // ---- organ edits: a fetch of their own, not send()/poll ------------------
  //
  // A structural edit can 400 with a specific, useful reason (a
  // duplicate name, a load already running) worth showing exactly, and
  // it doesn't land immediately — the server answers with a snapshot
  // mid-rebuild, and the real result arrives over the ordinary poll once
  // `loading` clears.

  async organCommand(query) {
    const { ok, error } = await this.organCommandResult(query);
    if (error != null) this.showError(error);
    return ok;
  }

  async organCommandResult(query) {
    this.hideError();
    try {
      const response = await fetch(this.base + query, { method: "POST" });
      if (!response.ok) {
        return { ok: false, error: (await response.text()) || `${response.status} ${response.statusText}` };
      }
      // Any successful edit can change what the sources offer (a new
      // source, a pull claiming a stop) — the cached offerings are
      // stale now whether or not the drawer is up to show it. The
      // division menu reads this cache, so a kept stale [] would keep
      // insisting there are no sources right after one was added.
      this.offerings = null;
      if (this.drawerOpen) this.fetchOfferings();
      return { ok: true, error: null };
    } catch (err) {
      return { ok: false, error: String(err) };
    }
  }

  /// Runs structural edits back to back. Each one rebuilds the organ,
  /// and the server refuses edits while a rebuild is in flight — so
  /// between commands this waits out `loading` (as the poll reports
  /// it), and a "still loading" refusal is retried rather than shown.
  async runQueue(queue) {
    const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
    for (const query of queue) {
      for (let attempt = 0; attempt < 40; attempt++) {
        while (this.lastSnapshot?.loading) await sleep(150);
        const { ok, error } = await this.organCommandResult(query);
        if (ok) break;
        if (!/loading/i.test(error ?? "")) {
          this.showError(error);
          return;
        }
        await sleep(250);
      }
      // Give the poll a beat to notice the rebuild this command started,
      // or the next iteration's wait would sail right past it.
      await sleep(300);
    }
  }

  showError(text) {
    this.el.error.textContent = text;
    this.el.error.classList.remove("hidden");
  }

  hideError() {
    this.el.error.classList.add("hidden");
    this.el.error.textContent = "";
  }

  // ---- the library drawer: what each source offers, and what's pulled -----

  wireDrawer() {
    this.el.drawerTab.addEventListener("click", () => this.toggleDrawer());
    this.el.drawerClose.addEventListener("click", () => this.closeDrawer());
  }

  toggleDrawer() {
    if (this.drawerOpen) this.closeDrawer();
    else this.openDrawer();
  }

  openDrawer() {
    this.drawerOpen = true;
    this.el.drawer.classList.remove("hidden");
    this.fetchOfferings();
  }

  closeDrawer() {
    this.drawerOpen = false;
    this.el.drawer.classList.add("hidden");
  }

  async fetchOfferings(render = true) {
    try {
      const response = await fetch(this.base + commands.organOfferings());
      this.offerings = response.ok ? ((await response.json()).sources ?? []) : null;
    } catch {
      this.offerings = null;
    }
    if (render) this.buildOfferings(this.offerings);
  }

  buildOfferings(sources) {
    const container = this.el.offerings;
    container.replaceChildren();
    if (sources == null) {
      container.append(this.emptyNote("Couldn't read this organ's sources."));
      return;
    }
    if (!sources.length) {
      container.append(
        this.emptyNote("No sources yet — double-click the console to add a sample set.")
      );
      return;
    }
    for (const source of sources) container.append(this.offeringSourceRow(source));
  }

  emptyNote(text) {
    const p = document.createElement("p");
    p.className = "pane-empty";
    p.textContent = text;
    return p;
  }

  offeringSourceRow(source) {
    const details = document.createElement("details");
    details.className = "organ-offerings-source";
    details.open = true;

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
      this.wireDragSource(head, () => ({
        kind: "offering-division",
        payload: { alias, manualName: manual.name },
        label: `${manual.name} (whole division)`,
      }));
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
    if (manual.kind === "microtonal") {
      const tag = document.createElement("span");
      tag.className = "organ-manual-pedal-tag";
      tag.textContent = "microtonal";
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
      this.wireDragSource(row, () => ({
        kind: "offering-stop",
        payload: { alias, manualName, stopName: stop.name },
        label: stop.name,
      }));
    }
    const check = document.createElement("span");
    check.className = "organ-offerings-stop-check";
    check.textContent = stop.pulled ? "✓" : "";
    const name = document.createElement("span");
    name.textContent = stop.name;
    row.append(check, name);
    return row;
  }

  // ---- adding to the organ: double-click the canvas -----------------------
  //
  // The Max gesture: double-click empty canvas (unlocked — or
  // ctrl-double-click through the lock) and the add menu opens where
  // you clicked. A manual or pedalboard added this way lands its
  // panels at that spot, via `pendingPlace` once the rebuild settles.

  wireCanvas() {
    // Double-click and right-click on empty space are the same gesture:
    // the add menu, or — locked — the padlock's nudge. (The webview's
    // own context menu is suppressed page-wide in main.js, so right-
    // click is ours to answer.) The empty-organ card floats over the
    // canvas and its copy says "double-click anywhere" — the card
    // itself must count too, or the instruction swallows its own
    // gesture; only its button stays out.
    const addGesture = (event) => {
      event.preventDefault();
      if (!(this.unlocked || event.ctrlKey)) {
        this.nudgeUnlock();
        return;
      }
      this.openAddMenu(event.clientX, event.clientY);
    };
    for (const type of ["dblclick", "contextmenu"]) {
      this.el.canvas.addEventListener(type, (event) => {
        if (event.target !== this.el.canvas) return; // empty space only
        addGesture(event);
      });
      this.el.emptyCard.addEventListener(type, (event) => {
        if (event.target.closest("button")) return;
        addGesture(event);
      });
    }
    // Popovers close on a click anywhere outside themselves.
    for (const el of [this.el.add, this.el.divisionMenu, this.el.keyboardMenu, this.el.tuning]) {
      el.addEventListener("click", (event) => event.stopPropagation());
    }
    window.addEventListener("click", () => {
      this.closeAdd();
      this.closeDivisionMenu();
      this.closeKeyboardMenu();
      this.closeTuningForm();
    });
    window.addEventListener("keydown", (event) => {
      if (event.key !== "Escape") return;
      this.closeAdd();
      this.closeDivisionMenu();
      this.closeKeyboardMenu();
      this.closeTuningForm();
    });
  }

  openAddMenu(x, y) {
    this.closeDivisionMenu();
    this.addAnchor = { x, y };
    this.closeAddPanels();
    this.el.add.classList.remove("hidden");
    this.el.addMenu.classList.remove("hidden");
    this.positionPopover(this.el.add, x, y);
  }

  positionPopover(el, x, y) {
    el.style.left = "0px";
    el.style.top = "0px";
    const { width, height } = el.getBoundingClientRect();
    el.style.left = `${Math.max(8, Math.min(x, window.innerWidth - width - 8))}px`;
    el.style.top = `${Math.max(8, Math.min(y, window.innerHeight - height - 8))}px`;
  }

  closeAdd() {
    this.el.add.classList.add("hidden");
    this.closeAddPanels();
  }

  closeAddPanels() {
    this.el.addMenu.classList.add("hidden");
    this.el.addManualForm.classList.add("hidden");
    this.el.addEncForm.classList.add("hidden");
    this.el.addSourceForm.classList.add("hidden");
  }

  wireAdd() {
    this.el.addManual.addEventListener("click", () => this.openManualForm("manual"));
    this.el.addPedal.addEventListener("click", () => this.openManualForm("pedal"));
    this.el.addMicrotonal.addEventListener("click", () => this.openManualForm("microtonal"));
    this.el.addEnc.addEventListener("click", () => this.openEncForm());
    this.el.addSource.addEventListener("click", () => this.openSourceForm());
    this.el.addManualCancel.addEventListener("click", () => this.closeAdd());
    this.el.addEncCancel.addEventListener("click", () => this.closeAdd());
    this.el.addSourceCancel.addEventListener("click", () => this.closeAdd());

    this.el.addManualForm.addEventListener("submit", (event) => {
      event.preventDefault();
      const name = this.el.addManualName.value.trim();
      if (!name) return;
      // The bounds are note names ("C2", "F♯4"), the same reading as the
      // compass fields in Preferences; a field naming no note keeps the
      // form open rather than guessing a compass.
      const low = parseKeyName(this.el.addManualLow.value);
      const high = parseKeyName(this.el.addManualHigh.value);
      this.el.addManualLow.classList.toggle("invalid", low == null);
      this.el.addManualHigh.classList.toggle("invalid", high == null);
      if (low == null || high == null) return;
      this.organCommand(commands.organManualAdd(name, low, high, this.addKind)).then((ok) => {
        if (!ok) return;
        this.rememberPlacement(name);
        this.closeAdd();
      });
    });

    this.el.addEncForm.addEventListener("submit", (event) => {
      event.preventDefault();
      const name = this.el.addEncName.value.trim();
      if (!name) return;
      this.organCommand(commands.organEnclosureAdd(name)).then((ok) => ok && this.closeAdd());
    });

    this.el.addSourceAdd.addEventListener("click", () => {
      const path = this.el.addSourcePath.value.trim();
      if (!path) return;
      this.organCommand(commands.organSourceAdd(path)).then((ok) => {
        if (ok) this.el.addSourcePath.value = "";
      });
    });
    this.el.addBrowseUp.addEventListener("click", () => {
      if (this.addBrowseParent) this.addBrowse(this.addBrowseParent);
    });
  }

  /// The new manual's panels should land where the add menu was opened,
  /// not wherever the default layout would seat them.
  rememberPlacement(name) {
    if (!this.addAnchor) return;
    const rect = this.el.canvas.getBoundingClientRect();
    this.pendingPlace = {
      name,
      x: this.addAnchor.x - rect.left,
      y: this.addAnchor.y - rect.top,
    };
  }

  /// Runs on every structural rebuild: once the awaited manual exists,
  /// seat its keyboard at the remembered spot and its jamb just left of
  /// it, then persist both. Sizes are real by now — the panels are in
  /// the DOM this decorate pass is decorating.
  placePending(snapshot) {
    const pending = this.pendingPlace;
    if (!pending) return;
    if (!snapshot.manuals.some((m) => m.name === pending.name)) return;
    this.pendingPlace = null;
    const canvas = this.el.canvas;
    const W = canvas.clientWidth;
    const H = canvas.clientHeight;
    const keyboard = canvas.querySelector(`.panel[data-panel="keyboard:${CSS.escape(pending.name)}"]`);
    const jamb = canvas.querySelector(`.panel[data-panel="jamb:${CSS.escape(pending.name)}"]`);
    if (!W || !H || !keyboard) return;
    const kx = Math.max(0, Math.min(pending.x, W - keyboard.offsetWidth));
    const ky = Math.max(0, Math.min(pending.y, H - keyboard.offsetHeight));
    const places = [commands.organPanelPlace(`keyboard:${pending.name}`, kx / W, ky / H)];
    if (jamb) {
      const jx = Math.max(0, kx - jamb.offsetWidth - 16);
      places.push(commands.organPanelPlace(`jamb:${pending.name}`, jx / W, ky / H));
    }
    this.runQueue(places);
  }

  openManualForm(kind) {
    this.addKind = kind;
    this.closeAddPanels();
    this.el.addManualForm.classList.remove("hidden");
    this.el.addManualName.value = "";
    this.el.addManualLow.value = "C2";
    this.el.addManualHigh.value = kind === "pedal" ? "G4" : "C7";
    this.el.addManualLow.classList.remove("invalid");
    this.el.addManualHigh.classList.remove("invalid");
    if (this.addAnchor) this.positionPopover(this.el.add, this.addAnchor.x, this.addAnchor.y);
    requestAnimationFrame(() => this.el.addManualName.focus());
  }

  openEncForm() {
    this.closeAddPanels();
    this.el.addEncForm.classList.remove("hidden");
    if (this.addAnchor) this.positionPopover(this.el.add, this.addAnchor.x, this.addAnchor.y);
    requestAnimationFrame(() => this.el.addEncName.focus());
  }

  openSourceForm() {
    // The desktop shell has a real file dialog — use it, as the picker
    // does. The in-form browser below stays the web fallback, and the
    // right tool again should this console ever front a remote server
    // (a native dialog would then pick paths on the wrong machine).
    if (window.__TAURI__) {
      this.pickSourceNative();
      return;
    }
    this.closeAddPanels();
    this.el.addSourceForm.classList.remove("hidden");
    this.el.addSourcePath.value = "";
    this.addBrowseDir = null;
    this.addBrowseParent = null;
    if (this.addAnchor) this.positionPopover(this.el.add, this.addAnchor.x, this.addAnchor.y);
    this.addBrowse();
  }

  /// The native open dialog, filtered to sample sets. A cancelled
  /// dialog is not an error — nothing happens; a pick goes straight to
  /// the server, whose refusals surface like any other edit's.
  async pickSourceNative() {
    this.closeAdd();
    const picked = await window.__TAURI__.core
      .invoke("plugin:dialog|open", {
        options: {
          title: "Choose a sample set",
          filters: [SET_FILTER],
          multiple: false,
          directory: false,
        },
      })
      .catch(() => null);
    const path = Array.isArray(picked) ? picked[0] : picked;
    if (typeof path === "string" && path) this.organCommand(commands.organSourceAdd(path));
  }

  /// This organ's own directory listing, the same idiom as the picker's
  /// Browse pane but scoped to this form: fetched directly, not
  /// snapshot-driven, and picking a file adds it as a source outright
  /// rather than loading it.
  async addBrowse(dir) {
    try {
      const query = dir ? `/api/browse?dir=${encodeURIComponent(dir)}` : "/api/browse";
      const response = await fetch(this.base + query);
      if (!response.ok) {
        this.addBrowseError = (await response.text()) || `${response.status} ${response.statusText}`;
        this.renderAddBrowse();
        return;
      }
      const data = await response.json();
      this.addBrowseDir = data.dir;
      this.addBrowseParent = data.parent;
      this.addBrowseEntries = data.entries;
      this.addBrowseError = null;
      this.renderAddBrowse();
    } catch (err) {
      this.addBrowseError = String(err);
      this.renderAddBrowse();
    }
  }

  renderAddBrowse() {
    this.el.addBrowseDir.textContent = this.addBrowseDir ?? "";
    this.el.addBrowseDir.title = this.addBrowseDir ?? "";
    this.el.addBrowseUp.disabled = !this.addBrowseParent;
    this.el.addBrowseError.classList.toggle("hidden", !this.addBrowseError);
    this.el.addBrowseError.textContent = this.addBrowseError ?? "";
    this.el.addBrowseList.replaceChildren();
    if (this.addBrowseError) return;
    // The server also lists Scala tuning files now; this browser means
    // loadable sets and organs.
    const loadable = /\.(organ|toml|organ_hauptwerk_xml)$/i;
    const entries = (this.addBrowseEntries ?? []).filter(
      (entry) => entry.dir || loadable.test(entry.name)
    );
    if (!entries.length) {
      this.el.addBrowseList.append(this.emptyNote("Nothing here."));
      return;
    }
    for (const entry of entries) {
      const row = document.createElement("button");
      row.type = "button";
      row.className = entry.dir ? "picker-row picker-browse-dir" : "picker-row";
      row.title = entry.path;
      row.addEventListener("click", () => {
        if (entry.dir) {
          this.addBrowse(entry.path);
        } else {
          this.el.addSourcePath.value = entry.path;
          this.organCommand(commands.organSourceAdd(entry.path));
        }
      });
      const name = document.createElement("span");
      name.className = "picker-row-name";
      name.textContent = entry.name;
      row.append(name);
      this.el.addBrowseList.append(row);
    }
  }
}
