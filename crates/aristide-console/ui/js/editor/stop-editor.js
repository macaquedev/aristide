// The stop-editor popover: right-click any drawknob.
//
// Name and voicing (footage, cents, gain) post live, field by field —
// no rebuild, exactly the tuning popover's contract. Retargeting a
// stop's source is structural, so picking one swaps a subview in over
// the form (openStopSrcView/closeStopSrcView), the same idiom as the
// tuning popover's own scale browser.

import { commands } from "../api.js";
import { renderIfChanged, resetRender, setText } from "../dom.js";
import { menuItem } from "../menu.js";
import { formatFootage, parseFootage, splitFootageName } from "../pitch.js";
import { emptyNote, pistonRow } from "../wiring.js";

export function wireStopForm(editor) {
  editor.el.stopClose.addEventListener("click", () => editor.closeStopForm());
  editor.el.stopSrcChange.addEventListener("click", () => openStopSrcView(editor));
  editor.el.stopSrcCancel.addEventListener("click", () => closeStopSrcView(editor));

  // Deleting is the drag-to-bin gesture as a button: the stop comes
  // off the console, its source still offers it — no confirm, same
  // as the bin.
  editor.el.stopDelete.addEventListener("click", () => {
    if (editor.stopOpen == null) return;
    const id = editor.stopOpen;
    editor.closeStopForm();
    editor.organCommand(commands.organUnpull(id));
  });

  // Every field commits on change already — Enter in the name field
  // must not also reload the page.
  editor.el.stopForm.addEventListener("submit", (event) => event.preventDefault());

  editor.el.stopName.addEventListener("change", () => {
    if (editor.stopOpen == null) return;
    const stop = editor.lastSnapshot?.stops.find((s) => s.id === editor.stopOpen);
    const name = editor.el.stopName.value.trim();
    if (!stop || !name || name === stop.name) return;
    // A hand-typed name supersedes any pending rename offer.
    hideStopLabelSync(editor);
    stopCommand(editor, commands.organStopRename(editor.stopOpen, name));
  });

  editor.el.stopFootage.addEventListener("change", async () => {
    if (editor.stopOpen == null) return;
    const stop = editor.lastSnapshot?.stops.find((s) => s.id === editor.stopOpen);
    const text = editor.el.stopFootage.value.trim();
    const ok = await stopCommand(editor, commands.organStopVoice(editor.stopOpen, { footage: text || "native" }));
    if (ok && stop) offerStopLabelSync(editor, stop, text);
  });

  editor.el.stopCents.addEventListener("change", () => {
    if (editor.stopOpen == null) return;
    const cents = Number(editor.el.stopCents.value);
    if (!Number.isFinite(cents)) return;
    stopCommand(editor, commands.organStopVoice(editor.stopOpen, { cents }));
  });

  editor.el.stopGain.addEventListener("change", () => {
    if (editor.stopOpen == null) return;
    const gain = Number(editor.el.stopGain.value);
    if (!Number.isFinite(gain)) return;
    stopCommand(editor, commands.organStopVoice(editor.stopOpen, { gain }));
  });

  editor.el.stopReset.addEventListener("click", async () => {
    if (editor.stopOpen == null) return;
    const stop = editor.lastSnapshot?.stops.find((s) => s.id === editor.stopOpen);
    const ok = await stopCommand(editor, commands.organStopVoice(editor.stopOpen, { reset: 1 }));
    if (ok && stop) offerStopLabelSync(editor, stop, "native");
  });

  // The rename offer's answers (see offerStopLabelSync). Yes renames
  // the stop to its name minus the footage tail — the server's
  // rename carries every file reference along — and, if a custom or
  // hidden engraving was set, returns it to auto so the knob face
  // reads the footage off the real pitch from now on. No remembers
  // the refusal for this stop so later edits don't nag.
  editor.el.stopLabelSyncYes.addEventListener("click", async () => {
    const pending = editor.stopLabelSync;
    hideStopLabelSync(editor);
    if (!pending || pending.id !== editor.stopOpen) return;
    const ok = await stopCommand(editor, commands.organStopRename(pending.id, pending.base));
    if (ok && pending.relabel) {
      stopCommand(editor, commands.organStopLabel(pending.id, { auto: 1 }));
    }
  });
  editor.el.stopLabelSyncNo.addEventListener("click", () => {
    if (editor.stopLabelSync) editor.stopLabelSyncDeclined.add(editor.stopLabelSync.id);
    hideStopLabelSync(editor);
  });

  editor.el.stopLabelMode.addEventListener("change", () => {
    if (editor.stopOpen == null) return;
    const mode = editor.el.stopLabelMode.value;
    editor.el.stopLabelText.classList.toggle("hidden", mode !== "custom");
    if (mode === "auto") {
      stopCommand(editor, commands.organStopLabel(editor.stopOpen, { auto: 1 }));
    } else if (mode === "none") {
      stopCommand(editor, commands.organStopLabel(editor.stopOpen, { label: "" }));
    } else {
      // "custom" posts nothing yet — reveal the text field and let
      // the player type the engraving; it commits on its own change.
      editor.el.stopLabelText.focus();
    }
  });

  editor.el.stopLabelText.addEventListener("change", () => {
    if (editor.stopOpen == null) return;
    stopCommand(editor, commands.organStopLabel(editor.stopOpen, { label: editor.el.stopLabelText.value }));
  });

  editor.el.stopOwnPipes.addEventListener("change", () => {
    if (editor.stopOpen == null) return;
    stopCommand(editor, commands.organStopOwnPipes(editor.stopOpen, editor.el.stopOwnPipes.checked));
  });

  editor.el.stopTuningEdit.addEventListener("click", () => {
    if (editor.stopOpen == null) return;
    const id = editor.stopOpen;
    const rect = editor.el.stop.getBoundingClientRect();
    editor.closeStopForm();
    editor.openTuningForm({ kind: "stop", id }, rect.left, rect.top);
  });
}

export function openStopForm(editor, id, x, y) {
  editor.openingPopover("stop");
  editor.stopOpen = id;
  resetRender(editor.el.stopPistons);
  hideStopError(editor);
  hideStopLabelSync(editor);
  closeStopSrcView(editor);
  syncStopForm(editor);
  editor.el.stop.classList.remove("hidden");
  editor.positionPopover(editor.el.stop, x, y);
}

export function closeStopForm(editor) {
  editor.stopOpen = null;
  editor.el.stop.classList.add("hidden");
  hideStopError(editor);
  hideStopLabelSync(editor);
  closeStopSrcView(editor);
}

/// Refills the form from the snapshot's stop entry — on open and on
/// every later poll, so a rebuild's or another session's edit lands
/// in the fields. Never touches the source-picker subview — a poll
/// landing mid-navigation must not yank it shut (the tuning popover's
/// browse idiom).
export function syncStopForm(editor) {
  const stop = editor.lastSnapshot?.stops.find((s) => s.id === editor.stopOpen);
  if (!stop) {
    editor.closeStopForm();
    return;
  }
  setText(editor.el.stopTitle, stop.name);
  const pitch = stop.pitch ?? {};
  editor.el.stopReset.classList.toggle("hidden", !pitch.own);

  if (editor.root.activeElement !== editor.el.stopName) editor.el.stopName.value = stop.name;

  if (editor.root.activeElement !== editor.el.stopFootage) {
    editor.el.stopFootage.value = formatFootage(pitch.footage ?? pitch.native);
  }
  // A mixture speaks several footages at once — there is no single
  // number the footage field could hold, so it's disabled and the
  // stop is voiced in cents alone.
  const mixture = pitch.native == null;
  editor.el.stopFootage.disabled = mixture;
  editor.el.stopFootage.title = mixture
    ? "A mixture speaks several footages — tune it in cents"
    : "";

  if (editor.root.activeElement !== editor.el.stopCents) editor.el.stopCents.value = pitch.cents ?? 0;
  if (editor.root.activeElement !== editor.el.stopGain) editor.el.stopGain.value = pitch.gain ?? 0;

  // label absent = auto, "" = hidden, anything else = that exact text.
  const labelMode = stop.label == null ? "auto" : stop.label === "" ? "none" : "custom";
  if (editor.root.activeElement !== editor.el.stopLabelMode) editor.el.stopLabelMode.value = labelMode;
  editor.el.stopLabelText.classList.toggle("hidden", labelMode !== "custom");
  if (labelMode === "custom" && editor.root.activeElement !== editor.el.stopLabelText) {
    editor.el.stopLabelText.value = stop.label;
  }

  editor.el.stopOwnPipes.checked = !!stop.own_pipes;

  const src = stop.src;
  setText(
    editor.el.stopSrc,
    src ? `${src.from} · ${src.manual}${src.stop ? ` · ${src.stop}` : ""}` : "—"
  );

  setText(editor.el.stopTuningSummary, editor.stopTuningLine(stop));

  // A mixture's individual ranks, only when there's more than one to
  // tell apart — a single-rank stop's tuning is the row above, in full.
  const ranks = stop.ranks ?? [];
  const rankStatus = (rank) =>
    rank.own
      ? `own · ${editor.tuningLabel((editor.lastSnapshot?.rank_tuning ?? []).find(
          (t) => t.stop === stop.id && t.rank === rank.id
        ))}`
      : "follows stop";
  const shown = ranks.length > 1 ? ranks : [];
  const rankRows = shown.map((r) => [r.id, r.name, rankStatus(r)]);
  renderIfChanged(editor.el.stopRanks, JSON.stringify([stop.id, rankRows]), () => {
    editor.el.stopRanks.replaceChildren();
    for (const rank of shown) {
      const row = document.createElement("div");
      row.className = "stop-rank-row";
      const name = document.createElement("span");
      name.className = "stop-rank-name";
      name.textContent = rank.name;
      const status = document.createElement("span");
      status.className = "stop-rank-status";
      status.textContent = rankStatus(rank);
      const edit = document.createElement("button");
      edit.type = "button";
      edit.className = "ghost";
      edit.textContent = "Edit…";
      edit.addEventListener("click", () => {
        const rect = editor.el.stop.getBoundingClientRect();
        editor.closeStopForm();
        editor.openTuningForm({ kind: "rank", stop: stop.id, rank: rank.id }, rect.left, rect.top);
      });
      row.append(name, status, edit);
      editor.el.stopRanks.append(row);
    }
  });

  editor.syncPistonRow(editor.el.stopPistons, `stop:${stop.name}`);
}

/// One popover's quick piston row, rebuilt only when the bindings it
/// shows (or the quick-bind in flight) change — a poll must never
/// recreate the Listen button under the pointer.
export function syncPistonRow(editor, container, action) {
  const listening = editor.quickBind?.action === action && editor.quickBind?.manual == null;
  const bound = (editor.lastSnapshot?.controls ?? []).filter((c) => c.action === action);
  const signature = JSON.stringify([action, bound, listening]);
  renderIfChanged(container, signature, () => {
    container.replaceChildren(
      pistonRow(
        { snapshot: editor.lastSnapshot, send: editor.send, listening },
        action,
        (act, cancelling) => editor.quickBindListen(act, null, cancelling)
      )
    );
  });
}

/// Sends a stop field update directly (not through the app-wide
/// `send()`), so a 400's reason lands in this popover rather than the
/// global status strip — the same local-fetch idiom `tremCommand` uses.
async function stopCommand(editor, query) {
  hideStopError(editor);
  const { ok, error } = await editor.organCommandResult(query);
  if (error != null) showStopError(editor, error);
  return ok;
}

/// After a footage edit lands: if the stop's *name* still carries a
/// footage tail that no longer reads as what the stop now speaks
/// ("Montre 8'" revoiced to 16'), offer to move the footage out of
/// the name. The knob face is already honest — auto engraving strips
/// the name's tail and writes the real pitch — but the name itself
/// would keep saying 8' in the popover title, piston bindings and
/// stop lists. Yes renames to the bare name and returns a custom or
/// hidden engraving to auto, so the footage is thereafter inferred
/// from the pitch alone; the answer machinery is in wireStopForm.
/// `text` is the footage the edit sent — "native" or the field's text.
function offerStopLabelSync(editor, stop, text) {
  hideStopLabelSync(editor);
  if (editor.stopLabelSyncDeclined.has(stop.id)) return;
  const split = splitFootageName(stop.name);
  if (!split) return;
  const feet =
    !text || /^native$/i.test(text) ? stop.pitch?.native : parseFootage(text);
  if (feet == null || formatFootage(feet) === formatFootage(split.feet)) return;
  editor.stopLabelSync = { id: stop.id, base: split.base, relabel: stop.label != null };
  const em = (words) => {
    const el = document.createElement("em");
    el.textContent = words;
    return el;
  };
  editor.el.stopLabelSyncText.replaceChildren(
    "The name still says ",
    em(`${split.tail}`),
    " — rename the stop ",
    em(split.base),
    ` and engrave the ${formatFootage(feet)}' it now speaks?`
  );
  editor.el.stopLabelSync.classList.remove("hidden");
}

function hideStopLabelSync(editor) {
  editor.stopLabelSync = null;
  editor.el.stopLabelSync.classList.add("hidden");
}

function showStopError(editor, text) {
  editor.el.stopError.textContent = text;
  editor.el.stopError.classList.remove("hidden");
}

function hideStopError(editor) {
  editor.el.stopError.classList.add("hidden");
  editor.el.stopError.textContent = "";
}

/// Swaps the source-picker subview in over the form: every source's
/// every division's every stop, including already-pulled ones —
/// retargeting a stop at one already on the console is legal
/// borrowing, not a claim that has to be free first.
async function openStopSrcView(editor) {
  if (editor.stopOpen == null) return;
  editor.stopSrcOpen = true;
  editor.el.stopForm.classList.add("hidden");
  editor.el.stopSrcView.classList.remove("hidden");
  editor.el.stopSrcList.replaceChildren(emptyNote("Reading the sources…"));
  if (!editor.offerings) await editor.fetchOfferings(false);
  renderStopSrcList(editor);
}

function closeStopSrcView(editor) {
  editor.stopSrcOpen = false;
  editor.el.stopSrcView.classList.add("hidden");
  editor.el.stopForm.classList.remove("hidden");
}

function renderStopSrcList(editor) {
  editor.el.stopSrcList.replaceChildren();
  const sources = editor.offerings;
  if (sources == null) {
    editor.el.stopSrcList.append(emptyNote("Couldn't read this organ's sources."));
    return;
  }
  const stop = editor.lastSnapshot?.stops.find((s) => s.id === editor.stopOpen);
  const current = stop?.src;
  let any = false;
  for (const source of sources) {
    for (const manual of source.manuals ?? []) {
      const stops = manual.stops ?? [];
      if (!stops.length) continue;
      any = true;
      const title = document.createElement("span");
      title.className = "organ-stop-group-title";
      title.textContent = `${source.alias} · ${manual.name}`;
      editor.el.stopSrcList.append(title);
      for (const srcStop of stops) {
        const isCurrent =
          !!current &&
          current.from === source.alias &&
          current.manual === manual.name &&
          current.stop === srcStop.name;
        const row = menuItem(srcStop.name, {
          checked: isCurrent,
          disabled: isCurrent,
          onClick: async () => {
            if (editor.stopOpen == null) return;
            const { ok, error } = await editor.organCommandResult(
              commands.organStopSource(editor.stopOpen, source.alias, manual.name, srcStop.name)
            );
            // The response is a snapshot mid-rebuild, not the settled
            // result — the popover stays open and the next poll's
            // syncStopForm() will re-sync its source line once the
            // rebuild lands.
            if (error != null) showStopError(editor, error);
            else closeStopSrcView(editor);
          },
        });
        editor.el.stopSrcList.append(row);
      }
    }
  }
  if (!any) {
    editor.el.stopSrcList.append(emptyNote("The sources have nothing to offer."));
  }
}
