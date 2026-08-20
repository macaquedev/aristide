// The organ-structure editor: a Max-MSP-style unlockable patch, not a
// dialog. Locked, the console behaves exactly as it always has, except
// that a ctrl-drag still edits — that's the "reach through the lock"
// gesture the rest of this module exists to serve. Unlocked, plain drags
// do the same thing, plus a FAB and a library drawer appear.
//
// This owns the editing chrome (padlock, drawer, bin, FAB, forms, the
// rebuild status strip) and decorates the DOM Console already built —
// it never builds jambs or keyboards itself. `decorateConsole(snapshot)`
// is called by Console right after every structural rebuild (see
// console.js's `decorate` hook); `update(snapshot)` is called on every
// poll, the same as Preferences and the other panels.
//
// The drag controller (startDrag/dragMove/findDropTarget/
// applyDropHighlight/endDrag) and the FAB/offerings/remove-confirm code
// are ported from the old Preferences organ pane (prefs.js, before this
// module existed) — same shapes, same server calls, now reading off the
// console's own elements instead of a modal's.

import { commands } from "./api.js";

function clampNote(value) {
  return Math.min(127, Math.max(0, Math.trunc(Number(value) || 0)));
}

/// Swallows the click a suppressed drag would otherwise leave behind —
/// a drag that crossed the threshold must not also toggle the drawknob
/// (or fire whatever else the element's own click listener does).
function suppressClick(event) {
  event.preventDefault();
  event.stopImmediatePropagation();
}

export class Editor {
  constructor(root, base, send) {
    this.root = root;
    this.base = base;
    this.send = send;
    this.unlocked = false;
    this.drawerOpen = false;
    this.drag = null; // the live drag, if any — see startDrag()
    this.lastSnapshot = null;
    this.autoUnlockedFor = null; // organ name already auto-unlocked once
    this.offerings = null;
    this.offeringsFile = null; // setup.file the cached offerings were fetched for
    this.renamingManual = null; // manual idx whose cheek is a rename input
    this.pendingRemove = null; // {kind: "manual"|"enclosure", ...} awaiting confirm
    this.fabPedal = false;
    this.fabBrowseDir = null;
    this.fabBrowseParent = null;
    this.fabBrowseEntries = null;
    this.fabBrowseError = null;
    this._lockNoteTimer = null;

    this.el = {
      lock: root.getElementById("editor-lock"),
      lockNote: root.getElementById("editor-lock-note"),
      status: root.getElementById("editor-status"),
      statusText: root.getElementById("editor-status-text"),
      error: root.getElementById("editor-error"),
      drawerTab: root.getElementById("editor-drawer-tab"),
      drawer: root.getElementById("editor-drawer"),
      drawerClose: root.getElementById("editor-drawer-close"),
      offerings: root.getElementById("editor-offerings"),
      bin: root.getElementById("editor-bin"),
      removeConfirm: root.getElementById("editor-remove-confirm"),
      removeConfirmText: root.getElementById("editor-remove-confirm-text"),
      removeConfirmYes: root.getElementById("editor-remove-confirm-yes"),
      removeConfirmNo: root.getElementById("editor-remove-confirm-no"),
      fabDock: root.getElementById("editor-fab-dock"),
      fab: root.getElementById("editor-fab"),
      fabMenu: root.getElementById("editor-fab-menu"),
      fabAddManual: root.getElementById("editor-fab-add-manual"),
      fabAddPedal: root.getElementById("editor-fab-add-pedal"),
      fabAddEnc: root.getElementById("editor-fab-add-enc"),
      fabAddSource: root.getElementById("editor-fab-add-source"),
      fabManualForm: root.getElementById("editor-fab-manual-form"),
      fabManualName: root.getElementById("editor-fab-manual-name"),
      fabManualLow: root.getElementById("editor-fab-manual-low"),
      fabManualHigh: root.getElementById("editor-fab-manual-high"),
      fabManualCancel: root.getElementById("editor-fab-manual-cancel"),
      fabEncForm: root.getElementById("editor-fab-enc-form"),
      fabEncName: root.getElementById("editor-fab-enc-name"),
      fabEncCancel: root.getElementById("editor-fab-enc-cancel"),
      fabSourceForm: root.getElementById("editor-fab-source-form"),
      fabSourcePath: root.getElementById("editor-fab-source-path"),
      fabSourceAdd: root.getElementById("editor-fab-source-add"),
      fabSourceCancel: root.getElementById("editor-fab-source-cancel"),
      fabBrowseUp: root.getElementById("editor-fab-browse-up"),
      fabBrowseDir: root.getElementById("editor-fab-browse-dir"),
      fabBrowseError: root.getElementById("editor-fab-browse-error"),
      fabBrowseList: root.getElementById("editor-fab-browse-list"),
    };

    this.wireLock();
    this.wireDrawer();
    this.wireRemoveConfirm();
    this.wireFab();
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
    this.el.fabDock.classList.remove("hidden");
    this.el.drawerTab.classList.remove("hidden");
  }

  lock() {
    this.unlocked = false;
    this.root.body.classList.remove("editing");
    this.el.lock.classList.remove("on");
    this.el.lock.setAttribute("aria-pressed", "false");
    this.el.lock.setAttribute("aria-label", "Unlock editing");
    this.el.lock.dataset.tip = "Unlock editing (Ctrl+E)";
    this.el.fabDock.classList.add("hidden");
    this.el.drawerTab.classList.add("hidden");
    this.closeDrawer();
    this.closeFabPanels();
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
  /// status, the empty-organ auto-unlock and offerings staleness all
  /// need to track fields (`loading`, `setup.file`) that don't
  /// necessarily change Console's own structural signature.
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

    this.el.fab.classList.toggle("pulse", !(snapshot.manuals?.length));

    const file = snapshot.setup?.file ?? null;
    if (file !== this.offeringsFile) {
      this.offeringsFile = file;
      this.offerings = null;
      if (this.drawerOpen) this.fetchOfferings();
    }
  }

  /// Called by Console right after every structural rebuild (see its
  /// `decorate` hook) — wires drag sources and manual drop targets onto
  /// the DOM it just built. Nothing here duplicates Console's own
  /// rendering; it only adds listeners and dataset markers.
  decorateConsole(snapshot) {
    this.lastSnapshot = snapshot;
    const empty = !!snapshot.organ && !snapshot.stops.length && !snapshot.manuals.length;
    if (empty) return; // the empty card has its own single button, wired by Console
    this.tagManualTargets(snapshot);
    this.wireStopDrags(snapshot);
    this.wireCheekDrags(snapshot);
    this.wireShoeDrags(snapshot);
    this.wireCheekRename();
  }

  /// Every manual/pedal keyboard always renders regardless of stop
  /// count, so it's the reliable drop target; a jamb `.division` only
  /// exists once a manual has stops, found here from its first knob.
  tagManualTargets(snapshot) {
    for (const board of this.root.querySelectorAll(".keyboard[data-manual]")) {
      board.dataset.dropManual = board.dataset.manual;
    }
    for (const division of this.root.querySelectorAll(".division")) {
      const knob = division.querySelector('.knob[data-key^="stop-"]');
      if (!knob) continue;
      const id = Number(knob.dataset.key.slice("stop-".length));
      const stop = snapshot.stops.find((s) => s.id === id);
      if (stop) division.dataset.dropManual = String(stop.midx);
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
    this.el.fabDock.classList.add("dragging");
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
    this.el.fabDock.classList.remove("dragging");
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
    this.hideError();
    try {
      const response = await fetch(this.base + query, { method: "POST" });
      if (!response.ok) {
        this.showError((await response.text()) || `${response.status} ${response.statusText}`);
        return false;
      }
      if (this.drawerOpen) this.fetchOfferings();
      return true;
    } catch (err) {
      this.showError(String(err));
      return false;
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
    const container = this.el.offerings;
    container.replaceChildren();
    if (sources == null) {
      container.append(this.emptyNote("Couldn't read this organ's sources."));
      return;
    }
    if (!sources.length) {
      container.append(this.emptyNote("No sources yet — add one with the + button."));
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

  // ---- the "+" FAB: add a manual, a pedalboard, a box, or a sample set ----

  wireFab() {
    this.el.fab.addEventListener("click", (event) => {
      event.stopPropagation();
      const opening =
        this.el.fabMenu.classList.contains("hidden") &&
        this.el.fabManualForm.classList.contains("hidden") &&
        this.el.fabEncForm.classList.contains("hidden") &&
        this.el.fabSourceForm.classList.contains("hidden");
      this.closeFabPanels();
      if (opening) this.el.fabMenu.classList.remove("hidden");
    });
    for (const el of [this.el.fabMenu, this.el.fabManualForm, this.el.fabEncForm, this.el.fabSourceForm]) {
      el.addEventListener("click", (event) => event.stopPropagation());
    }
    window.addEventListener("click", () => this.closeFabPanels());

    this.el.fabAddManual.addEventListener("click", () => this.openManualForm(false));
    this.el.fabAddPedal.addEventListener("click", () => this.openManualForm(true));
    this.el.fabAddEnc.addEventListener("click", () => this.openEncForm());
    this.el.fabAddSource.addEventListener("click", () => this.openSourceForm());
    this.el.fabManualCancel.addEventListener("click", () => this.closeFabPanels());
    this.el.fabEncCancel.addEventListener("click", () => this.closeFabPanels());
    this.el.fabSourceCancel.addEventListener("click", () => this.closeFabPanels());

    this.el.fabManualForm.addEventListener("submit", (event) => {
      event.preventDefault();
      const name = this.el.fabManualName.value.trim();
      if (!name) return;
      const low = clampNote(this.el.fabManualLow.value);
      const high = clampNote(this.el.fabManualHigh.value);
      this.organCommand(commands.organManualAdd(name, low, high, this.fabPedal ? 1 : 0)).then(
        (ok) => ok && this.closeFabPanels()
      );
    });

    this.el.fabEncForm.addEventListener("submit", (event) => {
      event.preventDefault();
      const name = this.el.fabEncName.value.trim();
      if (!name) return;
      this.organCommand(commands.organEnclosureAdd(name)).then((ok) => ok && this.closeFabPanels());
    });

    this.el.fabSourceAdd.addEventListener("click", () => {
      const path = this.el.fabSourcePath.value.trim();
      if (!path) return;
      this.organCommand(commands.organSourceAdd(path)).then((ok) => {
        if (ok) this.el.fabSourcePath.value = "";
      });
    });
    this.el.fabBrowseUp.addEventListener("click", () => {
      if (this.fabBrowseParent) this.fabBrowse(this.fabBrowseParent);
    });
  }

  closeFabPanels() {
    this.el.fabMenu.classList.add("hidden");
    this.el.fabManualForm.classList.add("hidden");
    this.el.fabEncForm.classList.add("hidden");
    this.el.fabSourceForm.classList.add("hidden");
  }

  openManualForm(pedal) {
    this.fabPedal = pedal;
    this.closeFabPanels();
    this.el.fabManualForm.classList.remove("hidden");
    this.el.fabManualName.value = "";
    this.el.fabManualLow.value = 36;
    this.el.fabManualHigh.value = pedal ? 67 : 96;
    requestAnimationFrame(() => this.el.fabManualName.focus());
  }

  openEncForm() {
    this.closeFabPanels();
    this.el.fabEncForm.classList.remove("hidden");
    this.el.fabEncName.value = "";
    requestAnimationFrame(() => this.el.fabEncName.focus());
  }

  openSourceForm() {
    this.closeFabPanels();
    this.el.fabSourceForm.classList.remove("hidden");
    this.el.fabSourcePath.value = "";
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
    this.el.fabBrowseDir.textContent = this.fabBrowseDir ?? "";
    this.el.fabBrowseDir.title = this.fabBrowseDir ?? "";
    this.el.fabBrowseUp.disabled = !this.fabBrowseParent;
    this.el.fabBrowseError.classList.toggle("hidden", !this.fabBrowseError);
    this.el.fabBrowseError.textContent = this.fabBrowseError ?? "";
    this.el.fabBrowseList.replaceChildren();
    if (this.fabBrowseError) return;
    const entries = this.fabBrowseEntries ?? [];
    if (!entries.length) {
      this.el.fabBrowseList.append(this.emptyNote("Nothing here."));
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
          this.el.fabSourcePath.value = entry.path;
          this.organCommand(commands.organSourceAdd(entry.path));
        }
      });
      const name = document.createElement("span");
      name.className = "picker-row-name";
      name.textContent = entry.name;
      row.append(name);
      this.el.fabBrowseList.append(row);
    }
  }
}
