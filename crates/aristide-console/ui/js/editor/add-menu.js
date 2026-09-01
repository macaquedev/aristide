// Adding to the organ: double-click the canvas.
//
// The Max gesture: double-click empty canvas (unlocked — or
// ctrl-double-click through the lock) and the add menu opens where
// you clicked. A manual or pedalboard added this way lands its
// panels at that spot, via `pendingPlace` once the rebuild settles.

import { commands, localFetch } from "../api.js";
import { parseKeyName } from "../pitch.js";
import { emptyNote, option } from "../wiring.js";

// What the native add-source dialog offers. Narrower than the picker's
// filter: a source must be a sample set — the server refuses another
// organ file — so Aristide's own .toml composites are left out.
const SET_FILTER = {
  name: "Sample sets (GrandOrgue, Hauptwerk)",
  extensions: ["organ", "Organ_Hauptwerk_xml"],
};

export function wireCanvas(editor) {
  // Double-click and right-click on empty space are the same gesture:
  // the add menu, or — locked — the padlock's nudge. (The webview's
  // own context menu is suppressed page-wide in main.js, so right-
  // click is ours to answer.) The empty-organ card floats over the
  // canvas and its copy says "double-click anywhere" — the card
  // itself must count too, or the instruction swallows its own
  // gesture; only its button stays out.
  const addGesture = (event) => {
    event.preventDefault();
    if (!(editor.unlocked || event.ctrlKey)) {
      editor.nudgeUnlock();
      return;
    }
    editor.openAddMenu(event.clientX, event.clientY);
  };
  for (const type of ["dblclick", "contextmenu"]) {
    editor.el.canvas.addEventListener(type, (event) => {
      if (event.target !== editor.el.canvas) return; // empty space only
      addGesture(event);
    });
    editor.el.emptyCard.addEventListener(type, (event) => {
      if (event.target.closest("button")) return;
      addGesture(event);
    });
  }
  // Popovers close on a click anywhere outside themselves.
  for (const el of [
    editor.el.add,
    editor.el.divisionMenu,
    editor.el.keyboardMenu,
    editor.el.couplersMenu,
    ...Object.entries(editor.popovers)
      .filter(([kind]) => kind !== "saveAs")
      .map(([, entry]) => entry.el),
  ]) {
    el.addEventListener("click", (event) => event.stopPropagation());
  }
  window.addEventListener("click", () => editor.closeAllPopovers());
  window.addEventListener("keydown", (event) => {
    if (event.key !== "Escape") return;
    // The dialog is modal: Escape means it, and nothing under it.
    if (editor.saveAsOpen) {
      event.preventDefault();
      editor.closeSaveAsForm();
      return;
    }
    editor.closeAllPopovers();
  });
}

export function openAddMenu(editor, x, y) {
  editor.closeDivisionMenu();
  editor.closeCouplerForm();
  editor.closeSettingsPopovers();
  editor.addAnchor = { x, y };
  closeAddPanels(editor);
  editor.el.add.classList.remove("hidden");
  editor.el.addMenu.classList.remove("hidden");
  editor.positionPopover(editor.el.add, x, y);
}

function closeAddPanels(editor) {
  editor.el.addMenu.classList.add("hidden");
  editor.el.addManualForm.classList.add("hidden");
  editor.el.addEncForm.classList.add("hidden");
  editor.el.addCouplerForm.classList.add("hidden");
  editor.el.addSourceForm.classList.add("hidden");
}

export function closeAdd(editor) {
  editor.el.add.classList.add("hidden");
  closeAddPanels(editor);
}

export function wireAdd(editor) {
  editor.el.addManual.addEventListener("click", () => openManualForm(editor, "manual"));
  editor.el.addPedal.addEventListener("click", () => openManualForm(editor, "pedal"));
  editor.el.addMicrotonal.addEventListener("click", () => openManualForm(editor, "microtonal"));
  editor.el.addEnc.addEventListener("click", () => openEncForm(editor));
  editor.el.addCoupler.addEventListener("click", () => editor.openCouplerAddForm());
  editor.el.addSource.addEventListener("click", () => openSourceForm(editor));
  editor.el.addManualCancel.addEventListener("click", () => editor.closeAdd());
  editor.el.addEncCancel.addEventListener("click", () => editor.closeAdd());
  editor.el.addCouplerCancel.addEventListener("click", () => editor.closeAdd());
  editor.el.addSourceCancel.addEventListener("click", () => editor.closeAdd());

  editor.el.addManualForm.addEventListener("submit", (event) => {
    event.preventDefault();
    const name = editor.el.addManualName.value.trim();
    if (!name) return;
    // The bounds are note names ("C2", "F♯4"), the same reading as the
    // compass fields in Preferences; a field naming no note keeps the
    // form open rather than guessing a compass.
    const low = parseKeyName(editor.el.addManualLow.value);
    const high = parseKeyName(editor.el.addManualHigh.value);
    editor.el.addManualLow.classList.toggle("invalid", low == null);
    editor.el.addManualHigh.classList.toggle("invalid", high == null);
    if (low == null || high == null) return;
    editor.organCommand(commands.organManualAdd(name, low, high, editor.addKind)).then((ok) => {
      if (!ok) return;
      rememberPlacement(editor, name);
      editor.closeAdd();
    });
  });

  editor.el.addEncForm.addEventListener("submit", (event) => {
    event.preventDefault();
    const name = editor.el.addEncName.value.trim();
    if (!name) return;
    editor.organCommand(commands.organEnclosureAdd(name)).then((ok) => ok && editor.closeAdd());
  });

  // The name follows the selection ("Swell to Great", "16′ Swell to
  // Great") until the player types one of their own — see
  // suggestCouplerName.
  for (const select of [editor.el.addCouplerSounds, editor.el.addCouplerOn, editor.el.addCouplerAt]) {
    select.addEventListener("change", () => suggestCouplerName(editor));
  }
  editor.el.addCouplerName.addEventListener("input", () => {
    editor.addCouplerNamed = editor.el.addCouplerName.value.trim() !== "";
  });
  editor.el.addCouplerForm.addEventListener("submit", (event) => {
    event.preventDefault();
    const name = editor.el.addCouplerName.value.trim();
    if (!name) return;
    // Spoken order to wire order: what SOUNDS is the route's target,
    // what it's played ON is where the route listens.
    const to = Number(editor.el.addCouplerSounds.value);
    const from = Number(editor.el.addCouplerOn.value);
    const shift = Number(editor.el.addCouplerAt.value) || 0;
    if (!Number.isFinite(from) || !Number.isFinite(to)) return;
    const routes = [{ from, to, shift }];
    // A coupler that duplicates an existing one gets the warning —
    // and, accepted, a permanent link: either control moves both.
    const twin = editor.duplicateCouplerOf(null, routes);
    if (twin) {
      editor.closeAdd();
      editor.showLinkConfirm(
        `${twin.name} already does exactly this. Add ${name} anyway, ` +
          "linked, so either control moves both?",
        () => editor.addCouplerLinked(name, routes, twin.name),
        null
      );
      return;
    }
    editor.organCommand(commands.organCouplerAdd(name, routes)).then(
      (ok) => ok && editor.closeAdd()
    );
  });

  editor.el.addSourceAdd.addEventListener("click", () => {
    const path = editor.el.addSourcePath.value.trim();
    if (!path) return;
    editor.organCommand(commands.organSourceAdd(path)).then((ok) => {
      if (ok) editor.el.addSourcePath.value = "";
    });
  });
  editor.el.addBrowseUp.addEventListener("click", () => {
    if (editor.addBrowseParent) addBrowse(editor, editor.addBrowseParent);
  });
}

/// The new manual's panels should land where the add menu was opened,
/// not wherever the default layout would seat them.
function rememberPlacement(editor, name) {
  if (!editor.addAnchor) return;
  const rect = editor.el.canvas.getBoundingClientRect();
  editor.pendingPlace = {
    name,
    x: editor.addAnchor.x - rect.left,
    y: editor.addAnchor.y - rect.top,
  };
}

/// Runs on every structural rebuild: once the awaited manual exists,
/// seat its keyboard at the remembered spot and its jamb just left of
/// it, then persist both. Sizes are real by now — the panels are in
/// the DOM this decorate pass is decorating.
export function placePending(editor, snapshot) {
  const pending = editor.pendingPlace;
  if (!pending) return;
  if (!snapshot.manuals.some((m) => m.name === pending.name)) return;
  editor.pendingPlace = null;
  const canvas = editor.el.canvas;
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
  editor.runQueue(places);
}

function openManualForm(editor, kind) {
  editor.addKind = kind;
  closeAddPanels(editor);
  editor.el.addManualForm.classList.remove("hidden");
  editor.el.addManualName.value = "";
  editor.el.addManualLow.value = "C2";
  editor.el.addManualHigh.value = kind === "pedal" ? "G4" : "C7";
  editor.el.addManualLow.classList.remove("invalid");
  editor.el.addManualHigh.classList.remove("invalid");
  if (editor.addAnchor) editor.positionPopover(editor.el.add, editor.addAnchor.x, editor.addAnchor.y);
  requestAnimationFrame(() => editor.el.addManualName.focus());
}

function openEncForm(editor) {
  closeAddPanels(editor);
  editor.el.addEncForm.classList.remove("hidden");
  if (editor.addAnchor) editor.positionPopover(editor.el.add, editor.addAnchor.x, editor.addAnchor.y);
  requestAnimationFrame(() => editor.el.addEncName.focus());
}

export function openCouplerAddForm(editor) {
  closeAddPanels(editor);
  editor.el.addCouplerForm.classList.remove("hidden");
  editor.el.addCouplerName.value = "";
  editor.addCouplerNamed = false;
  editor.el.addCouplerAt.value = "0";
  // Couplers taken off the console come back from here — a set's own
  // can't be deleted, only hidden, and hiding must be reversible
  // where couplers are added, not in some other surface.
  editor.el.addCouplerRestore.replaceChildren();
  const hidden = (editor.lastSnapshot?.couplers ?? []).filter((c) => c.hidden);
  if (hidden.length) {
    const heading = document.createElement("span");
    heading.className = "menu-heading";
    heading.textContent = "Off the console";
    editor.el.addCouplerRestore.append(heading);
    for (const coupler of hidden) {
      const row = document.createElement("div");
      row.className = "coupler-restore-row";
      const name = document.createElement("span");
      name.textContent = coupler.name;
      name.title = coupler.name;
      const restore = document.createElement("button");
      restore.type = "button";
      restore.className = "ghost";
      restore.textContent = "Restore";
      restore.addEventListener("click", () => {
        editor.closeAdd();
        editor.organCommand(commands.organCoupler(coupler.idx, true));
      });
      row.append(name, restore);
      editor.el.addCouplerRestore.append(row);
    }
  }
  const manuals = editor.lastSnapshot?.manuals ?? [];
  for (const select of [editor.el.addCouplerSounds, editor.el.addCouplerOn]) {
    select.replaceChildren();
    for (const manual of manuals) select.append(option(manual.idx, manual.name));
  }
  // The classic default: the second manual sounding on the first —
  // and a name to match, ready to be overtyped.
  if (manuals.length > 1) {
    editor.el.addCouplerSounds.value = String(manuals[1].idx);
    editor.el.addCouplerOn.value = String(manuals[0].idx);
  }
  suggestCouplerName(editor);
  if (editor.addAnchor) editor.positionPopover(editor.el.add, editor.addAnchor.x, editor.addAnchor.y);
}

/// The conventional name for what the add form's selects say:
/// "Swell to Great", "16′ Swell to Great" for a sub-octave, "Great
/// 4′" when a manual couples to itself at a pitch. Only fills the
/// name until the player types their own.
function suggestCouplerName(editor) {
  if (editor.addCouplerNamed) return;
  const manuals = editor.lastSnapshot?.manuals ?? [];
  const name = (value) => manuals.find((m) => String(m.idx) === value)?.name;
  const sounds = name(editor.el.addCouplerSounds.value);
  const on = name(editor.el.addCouplerOn.value);
  if (!sounds || !on) return;
  const shift = Number(editor.el.addCouplerAt.value) || 0;
  const pitch = shift === -12 ? "16′" : shift === 12 ? "4′" : "";
  editor.el.addCouplerName.value =
    sounds === on
      ? `${sounds} ${pitch || "Unison"}`.trim()
      : `${pitch} ${sounds} to ${on}`.trim();
}

function openSourceForm(editor) {
  // The desktop shell has a real file dialog — use it, as the picker
  // does. The in-form browser below stays the web fallback, and the
  // right tool again should this console ever front a remote server
  // (a native dialog would then pick paths on the wrong machine).
  if (window.__TAURI__) {
    pickSourceNative(editor);
    return;
  }
  closeAddPanels(editor);
  editor.el.addSourceForm.classList.remove("hidden");
  editor.el.addSourcePath.value = "";
  editor.addBrowseDir = null;
  editor.addBrowseParent = null;
  if (editor.addAnchor) editor.positionPopover(editor.el.add, editor.addAnchor.x, editor.addAnchor.y);
  addBrowse(editor);
}

/// The native open dialog, filtered to sample sets. A cancelled
/// dialog is not an error — nothing happens; a pick goes straight to
/// the server, whose refusals surface like any other edit's.
async function pickSourceNative(editor) {
  editor.closeAdd();
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
  if (typeof path === "string" && path) editor.organCommand(commands.organSourceAdd(path));
}

/// This organ's own directory listing, the same idiom as the picker's
/// Browse pane but scoped to this form: fetched directly, not
/// snapshot-driven, and picking a file adds it as a source outright
/// rather than loading it.
async function addBrowse(editor, dir) {
  const query = dir ? `/api/browse?dir=${encodeURIComponent(dir)}` : "/api/browse";
  const { ok, data, error } = await localFetch(editor.base, query, { json: true });
  if (!ok) {
    editor.addBrowseError = error;
  } else {
    editor.addBrowseDir = data.dir;
    editor.addBrowseParent = data.parent;
    editor.addBrowseEntries = data.entries;
    editor.addBrowseError = null;
  }
  renderAddBrowse(editor);
}

function renderAddBrowse(editor) {
  editor.el.addBrowseDir.textContent = editor.addBrowseDir ?? "";
  editor.el.addBrowseDir.title = editor.addBrowseDir ?? "";
  editor.el.addBrowseUp.disabled = !editor.addBrowseParent;
  editor.el.addBrowseError.classList.toggle("hidden", !editor.addBrowseError);
  editor.el.addBrowseError.textContent = editor.addBrowseError ?? "";
  editor.el.addBrowseList.replaceChildren();
  if (editor.addBrowseError) return;
  // The server also lists Scala tuning files now; this browser means
  // loadable sets and organs.
  const loadable = /\.(organ|toml|organ_hauptwerk_xml)$/i;
  const entries = (editor.addBrowseEntries ?? []).filter(
    (entry) => entry.dir || loadable.test(entry.name)
  );
  if (!entries.length) {
    editor.el.addBrowseList.append(emptyNote("Nothing here."));
    return;
  }
  for (const entry of entries) {
    const row = document.createElement("button");
    row.type = "button";
    row.className = entry.dir ? "picker-row picker-browse-dir" : "picker-row";
    row.title = entry.path;
    row.addEventListener("click", () => {
      if (entry.dir) {
        addBrowse(editor, entry.path);
      } else {
        editor.el.addSourcePath.value = entry.path;
        editor.organCommand(commands.organSourceAdd(entry.path));
      }
    });
    const name = document.createElement("span");
    name.className = "picker-row-name";
    name.textContent = entry.name;
    row.append(name);
    editor.el.addBrowseList.append(row);
  }
}
