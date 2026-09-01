// The coupler-route popover: right-click any coupler rocker.
//
// Everything posts live, field by field, the stop popover's own
// contract. A routes change always rebuilds the organ (the file's
// coupler line is rewritten outright), so route edits go through a
// coalescing queue: each change posts the whole array, an apply in
// flight makes later changes wait, and only the newest state is ever
// sent — clicking through three pitches costs one rebuild, not
// three. Later polls refresh the title and name; the routes fold
// back in from the server's echo only once an apply has settled and
// the pointer is elsewhere (syncCouplerForm).

import { commands } from "../api.js";
import { renderIfChanged, resetRender, setText } from "../dom.js";
import { option } from "../wiring.js";

export function wireCouplerForm(editor) {
  editor.el.couplerClose.addEventListener("click", () => editor.closeCouplerForm());
  // Every field commits on its own change — Enter in the name field
  // must not also reload the page.
  editor.el.couplerForm.addEventListener("submit", (event) => event.preventDefault());

  editor.el.couplerName.addEventListener("change", () => {
    if (editor.couplerOpen == null) return;
    const coupler = editor.lastSnapshot?.couplers.find((c) => c.idx === editor.couplerOpen);
    const name = editor.el.couplerName.value.trim();
    if (!coupler || !name || name === coupler.name) return;
    couplerCommand(editor, commands.organCouplerRename(editor.couplerOpen, name));
  });

  editor.el.couplerRouteAdd.addEventListener("click", () => {
    if (!editor.couplerRoutes) return;
    editor.couplerRoutes.push({ from: 0, to: 0, shift: 0 });
    renderCouplerRoutes(editor);
    scheduleCouplerApply(editor);
  });

  // The coupled-keys override — display only, live, the same
  // per-field contract as the name.
  editor.el.couplerKeys.addEventListener("change", () => {
    if (editor.couplerOpen == null) return;
    couplerCommand(editor, commands.organCouplerKeys(editor.couplerOpen, editor.el.couplerKeys.value));
  });

  editor.el.couplerDelete.addEventListener("click", () => {
    if (editor.couplerOpen == null) return;
    const coupler = editor.lastSnapshot?.couplers.find((c) => c.idx === editor.couplerOpen);
    if (!coupler) return;
    // Skip the close-time duplicate nag — the coupler is leaving.
    editor.couplerOpen = null;
    editor.couplerRoutes = null;
    editor.closeCouplerForm();
    editor.showRemoveConfirm("coupler", { idx: coupler.idx, name: coupler.name });
  });
}

/// Queue the working copy for an auto-apply. Coalescing: the newest
/// state replaces anything still waiting, so however many fields
/// change while a rebuild is in flight, exactly one apply follows.
function scheduleCouplerApply(editor) {
  if (editor.couplerOpen == null || !editor.couplerRoutes) return;
  editor.couplerPending = {
    idx: editor.couplerOpen,
    routes: structuredClone(editor.couplerRoutes),
  };
  pumpCouplerApply(editor);
}

/// Drains the pending apply, waiting out rebuilds the same way
/// runQueue does — the server refuses structural edits mid-rebuild,
/// and every apply here starts one. The pending edit is captured at
/// schedule time, so it still lands if the popover closes meanwhile.
async function pumpCouplerApply(editor) {
  if (editor.couplerApplying) return;
  editor.couplerApplying = true;
  const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
  while (editor.couplerPending) {
    const { idx, routes } = editor.couplerPending;
    editor.couplerPending = null;
    for (let attempt = 0; attempt < 40; attempt++) {
      while (editor.lastSnapshot?.loading) await sleep(150);
      const ok = await couplerCommand(editor, commands.organCouplerRoutes(idx, routes));
      if (ok) {
        editor.couplerResync = true;
        break;
      }
      // A real refusal stays shown; only "still loading" retries.
      if (!/loading/i.test(editor.el.couplerError.textContent)) break;
      hideCouplerError(editor);
      await sleep(250);
    }
    // Give the poll a beat to notice the rebuild this apply started,
    // or the next iteration's wait would sail right past it.
    await sleep(300);
  }
  editor.couplerApplying = false;
}

export function openCouplerForm(editor, idx, x, y) {
  const coupler = editor.lastSnapshot?.couplers.find((c) => c.idx === idx);
  if (!coupler) return;
  editor.openingPopover("coupler");
  editor.couplerOpen = idx;
  resetRender(editor.el.couplerPistons);
  hideCouplerError(editor);
  editor.couplerRoutes = structuredClone(coupler.routes ?? []);
  renderCouplerRoutes(editor);
  setText(editor.el.couplerTitle, coupler.name);
  if (editor.root.activeElement !== editor.el.couplerName) editor.el.couplerName.value = coupler.name;
  editor.el.coupler.classList.remove("hidden");
  editor.positionPopover(editor.el.coupler, x, y);
}

export function closeCouplerForm(editor) {
  const idx = editor.couplerOpen;
  const routes = editor.couplerRoutes;
  editor.couplerOpen = null;
  editor.couplerRoutes = null;
  editor.el.coupler.classList.add("hidden");
  hideCouplerError(editor);
  // Editing done: if the routes now duplicate another coupler's,
  // offer the permanent link — the same warning adding a duplicate
  // gets, at the same "finished" moment rather than on every
  // transient state mid-edit.
  if (idx != null && routes) warnDuplicateCoupler(editor, idx, routes);
}

/// The first other coupler whose routes do exactly what `routes` do
/// — field-for-field, order-blind — or null. Hidden couplers don't
/// count: they're off the console.
export function duplicateCouplerOf(editor, excludeIdx, routes) {
  const signature = (routes) =>
    JSON.stringify(
      (routes ?? [])
        .map((route) => [
          route.from ?? null,
          route.to ?? null,
          route.shift ?? 0,
          route.low ?? null,
          route.high ?? null,
          !!route.unison_off,
          route.scope ?? "",
          route.repitch ?? null,
          !!route.own_pipes,
        ])
        .map((fields) => JSON.stringify(fields))
        .sort()
    );
  const mine = signature(routes);
  return (
    (editor.lastSnapshot?.couplers ?? []).find(
      (coupler) =>
        coupler.idx !== excludeIdx && !coupler.hidden && signature(coupler.routes) === mine
    ) ?? null
  );
}

function warnDuplicateCoupler(editor, idx, routes) {
  const coupler = editor.lastSnapshot?.couplers.find((c) => c.idx === idx);
  if (!coupler) return;
  const twin = duplicateCouplerOf(editor, idx, routes);
  if (!twin || (coupler.linked ?? []).includes(twin.idx)) return;
  editor.showLinkConfirm(
    `${coupler.name} now does exactly what ${twin.name} does. Link them, ` +
      "so either control moves both?",
    () => editor.runQueue([commands.organCouplerLink(idx, twin.idx, true)]),
    null
  );
}

/// Refreshes the title and (unless focused) the name field from the
/// snapshot; the routes stay the local working copy until an
/// auto-apply has settled, when the server's echo folds back in —
/// but never while the pointer is in the route table, and never
/// mid-queue. A coupler that vanished (removed from elsewhere)
/// takes its popover with it.
export function syncCouplerForm(editor) {
  const coupler = editor.lastSnapshot?.couplers.find((c) => c.idx === editor.couplerOpen);
  if (!coupler) {
    editor.closeCouplerForm();
    return;
  }
  setText(editor.el.couplerTitle, coupler.name);
  if (editor.root.activeElement !== editor.el.couplerName) editor.el.couplerName.value = coupler.name;
  if (editor.root.activeElement !== editor.el.couplerKeys) {
    editor.el.couplerKeys.value = coupler.keys ?? "auto";
  }
  editor.syncPistonRow(editor.el.couplerPistons, `coupler:${coupler.name}`);
  renderCouplerLinks(editor, coupler);
  if (
    editor.couplerResync &&
    !editor.couplerApplying &&
    !editor.couplerPending &&
    !editor.lastSnapshot?.loading &&
    !editor.el.couplerRoutesBox.contains(editor.root.activeElement)
  ) {
    editor.couplerResync = false;
    editor.couplerRoutes = structuredClone(coupler.routes ?? []);
    renderCouplerRoutes(editor);
  }
}

/// The popover's linked-partners lines: one per linked coupler,
/// with its undo. Synced from the snapshot on every poll but rebuilt
/// only when the links change — the Unlink button is under the
/// pointer when it matters.
function renderCouplerLinks(editor, coupler) {
  const box = editor.el.couplerLinkedBox;
  const partners = (coupler.linked ?? [])
    .map((idx) => [idx, editor.lastSnapshot?.couplers.find((c) => c.idx === idx)])
    .filter(([, partner]) => partner);
  renderIfChanged(box, JSON.stringify([coupler.idx, partners.map(([i, p]) => [i, p.name])]), () => {
    box.replaceChildren();
    for (const [linkedIdx, partner] of partners) {
      const row = document.createElement("div");
      row.className = "coupler-linked";
      const label = document.createElement("span");
      label.textContent = `Linked with ${partner.name} — either control moves both.`;
      const unlink = document.createElement("button");
      unlink.type = "button";
      unlink.className = "ghost";
      unlink.textContent = "Unlink";
      unlink.addEventListener("click", () => {
        if (editor.couplerOpen == null) return;
        couplerCommand(editor, commands.organCouplerLink(editor.couplerOpen, linkedIdx, false));
      });
      row.append(label, unlink);
      box.append(row);
    }
  });
}

/// Rebuilds the route blocks from `editor.couplerRoutes` — the local
/// working copy, never the snapshot directly. Each route reads the
/// way a coupler is named: "Swell to Great" means SWELL'S STOPS
/// SOUND when the GREAT is played, so the row says "Sounds [Swell]
/// on [Great] at [Sub-octave (16′)]" — the wire's from/to (played/
/// sounding) stays under the hood. Fields this form doesn't expose
/// (low/high/repitch) are left on the route object untouched, so
/// every auto-apply round-trips them.
function renderCouplerRoutes(editor) {
  const container = editor.el.couplerRoutesBox;
  container.replaceChildren();
  const manuals = editor.lastSnapshot?.manuals ?? [];
  const word = (text) => {
    const span = document.createElement("span");
    span.className = "rail-label";
    span.textContent = text;
    return span;
  };
  const options = (select, entries) => {
    for (const [value, text] of entries) select.append(option(value, text));
  };
  const manualEntries = manuals.map((manual) => [String(manual.idx), manual.name]);

  editor.couplerRoutes.forEach((route, i) => {
    const block = document.createElement("div");
    block.className = "coupler-route";

    // "Sounds <division> on <keyboard>" — the coupler's own word
    // order. Sounding nothing turns the route into a pure silencer
    // (the classic Unison Off stop), which needs no pitch either.
    const what = document.createElement("div");
    what.className = "coupler-route-row";
    const soundsSelect = document.createElement("select");
    options(soundsSelect, [...manualEntries, ["", "(nothing — silence)"]]);
    soundsSelect.value = route.to == null ? "" : String(route.to);
    const onSelect = document.createElement("select");
    onSelect.title = "The keyboard you play — where the coupler listens.";
    options(onSelect, manualEntries);
    onSelect.value = route.from == null ? "" : String(route.from);
    onSelect.addEventListener("change", () => {
      route.from = Number(onSelect.value);
      scheduleCouplerApply(editor);
    });
    soundsSelect.title = "Whose stops speak — the division this coupler borrows.";
    soundsSelect.addEventListener("change", () => {
      if (soundsSelect.value === "") {
        route.to = null;
        // A route that sounds nothing must at least silence, or it
        // does nothing at all (the server refuses a dead line).
        route.unison_off = true;
      } else {
        route.to = Number(soundsSelect.value);
      }
      renderCouplerRoutes(editor);
      scheduleCouplerApply(editor);
    });
    what.append(word("Sounds"), soundsSelect, word("on"), onSelect);
    block.append(what);

    // "at <pitch>" — the organ's own words for the shift, with the
    // raw key count only for the odd coupler (a fourths coupler,
    // a quint) the presets don't name.
    if (route.to != null) {
      const at = document.createElement("div");
      at.className = "coupler-route-row";
      const pitchSelect = document.createElement("select");
      options(pitchSelect, [
        ["0", "Unison"],
        ["-12", "Sub-octave (16′)"],
        ["12", "Super-octave (4′)"],
        ["custom", "Other…"],
      ]);
      const keysInput = document.createElement("input");
      keysInput.type = "number";
      keysInput.min = "-24";
      keysInput.max = "24";
      keysInput.step = "1";
      keysInput.title = "Keys added to what you play: −12 an octave down, +7 a fifth up…";
      const keysWord = word("keys");
      const showKeys = (shown) => {
        keysInput.classList.toggle("hidden", !shown);
        keysWord.classList.toggle("hidden", !shown);
      };
      const shift = route.shift ?? 0;
      const preset = ["0", "-12", "12"].includes(String(shift));
      pitchSelect.value = preset ? String(shift) : "custom";
      keysInput.value = shift;
      showKeys(!preset);
      pitchSelect.addEventListener("change", () => {
        if (pitchSelect.value === "custom") {
          showKeys(true);
          keysInput.focus();
          return;
        }
        route.shift = Number(pitchSelect.value);
        keysInput.value = route.shift;
        showKeys(false);
        scheduleCouplerApply(editor);
      });
      keysInput.addEventListener("change", () => {
        const value = Number(keysInput.value);
        if (!Number.isFinite(value)) return;
        route.shift = Math.round(value);
        scheduleCouplerApply(editor);
      });
      at.append(word("at"), pitchSelect, keysInput, keysWord);
      block.append(at);
    }

    const how = document.createElement("div");
    how.className = "coupler-route-row";
    const scopeSelect = document.createElement("select");
    scopeSelect.title =
      "Which played keys couple: all of them, or only the lowest/highest " +
      "held — the intelligent Bass and Melody couplers.";
    options(scopeSelect, [
      ["", "every key"],
      ["bass", "lowest key held (Bass)"],
      ["melody", "highest key held (Melody)"],
    ]);
    scopeSelect.value = route.scope ?? "";
    scopeSelect.addEventListener("change", () => {
      if (scopeSelect.value) route.scope = scopeSelect.value;
      else delete route.scope;
      scheduleCouplerApply(editor);
    });

    const unisonLabel = document.createElement("label");
    unisonLabel.title =
      "Silence the played keyboard's own stops here, so the note moves " +
      "instead of doubling.";
    const unisonCheck = document.createElement("input");
    unisonCheck.type = "checkbox";
    unisonCheck.checked = !!route.unison_off;
    unisonCheck.disabled = route.to == null; // a pure silencer must silence
    unisonCheck.addEventListener("change", () => {
      if (unisonCheck.checked) route.unison_off = true;
      else delete route.unison_off;
      scheduleCouplerApply(editor);
    });
    unisonLabel.append(unisonCheck, document.createTextNode(" own stops off"));

    const ownPipesLabel = document.createElement("label");
    ownPipesLabel.title =
      "Speak an independent set of pipes — copies double pipes already " +
      "sounding instead of sharing them";
    const ownPipesCheck = document.createElement("input");
    ownPipesCheck.type = "checkbox";
    ownPipesCheck.checked = !!route.own_pipes;
    ownPipesCheck.addEventListener("change", () => {
      if (ownPipesCheck.checked) route.own_pipes = true;
      else delete route.own_pipes;
      scheduleCouplerApply(editor);
    });
    ownPipesLabel.append(ownPipesCheck, document.createTextNode(" own pipes"));

    how.append(scopeSelect, unisonLabel, ownPipesLabel);

    if (editor.couplerRoutes.length > 1) {
      const remove = document.createElement("button");
      remove.type = "button";
      remove.className = "ghost coupler-route-remove";
      remove.title = "Remove this route";
      remove.textContent = "×";
      remove.addEventListener("click", () => {
        editor.couplerRoutes.splice(i, 1);
        renderCouplerRoutes(editor);
        scheduleCouplerApply(editor);
      });
      how.append(remove);
    }

    block.append(how);
    container.append(block);
  });
}

/// Sends a coupler field update directly (not through the app-wide
/// `send()`), so a 400's reason lands in this popover rather than the
/// global status strip — the same local-fetch idiom `stopCommand` uses.
async function couplerCommand(editor, query) {
  hideCouplerError(editor);
  const { ok, error } = await editor.organCommandResult(query);
  if (error != null) showCouplerError(editor, error);
  return ok;
}

function showCouplerError(editor, text) {
  editor.el.couplerError.textContent = text;
  editor.el.couplerError.classList.remove("hidden");
}

function hideCouplerError(editor) {
  editor.el.couplerError.classList.add("hidden");
  editor.el.couplerError.textContent = "";
}
