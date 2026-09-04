// Drag controller: plain when unlocked, ctrl-drag always.
//
// Plain pointer events, not HTML5 drag-and-drop: a floating label
// follows the pointer and the drop target is read straight off
// `elementFromPoint`. Every drag source waits for ~4px of movement
// before committing to a drag — below that it's a click (a drawknob
// still toggles its stop; a cheek's dblclick still renames).

import { commands } from "../api.js";

/// How far the pointer travels before a press on a drag source becomes
/// a drag rather than a click — GTK's threshold. A mouse button's own
/// travel wobbles the pointer a few pixels (more on a high-DPI mouse),
/// and below this nothing is dragged, so a wobbly click stays a click.
const DRAG_THRESHOLD_PX = 8;

function withinRect(rect, { clientX, clientY }) {
  return (
    clientX >= rect.left && clientX <= rect.right && clientY >= rect.top && clientY <= rect.bottom
  );
}

/// Swallows the click a real drag leaves behind if it lands on the
/// dragged control — which it does only when the browser dispatches the
/// click to the pressed element rather than to the nearest common
/// ancestor of press and release, where it may bubble on as any click
/// elsewhere would. Either way the listener stands down at the next
/// press, so it can never eat a later, honest click on the control.
function suppressNextClick(source) {
  const swallow = (event) => {
    if (source.contains(event.target)) {
      event.preventDefault();
      event.stopImmediatePropagation();
    }
    disarm();
  };
  const disarm = () => {
    window.removeEventListener("click", swallow, true);
    window.removeEventListener("pointerdown", disarm, true);
  };
  window.addEventListener("click", swallow, true);
  window.addEventListener("pointerdown", disarm, true);
}

export function binAllowed(kind) {
  return kind === "stop" || kind === "manual" || kind === "enclosure" || kind === "coupler";
}

export function manualAllowed(kind) {
  return kind !== "enclosure";
}

export function encAllowed(kind) {
  return kind === "stop";
}

/// The kinds that live in a division's knob rank — the ones a drop
/// on a jamb carries a position for.
export function rankKind(kind) {
  return kind === "stop" || kind === "coupler";
}

/// The dragged control's rank token — the vocabulary the order
/// endpoint and the snapshot's `rank` share.
export function dragToken(drag) {
  return drag.kind === "coupler" ? `c${drag.payload.idx}` : `s${drag.payload.id}`;
}

/// A division's current display rank as tokens, from the snapshot —
/// the list a reorder splices into, so seated couplers keep their
/// places when a stop moves and vice versa.
export function rankTokens(editor, midx) {
  const manual = editor.lastSnapshot?.manuals.find((m) => m.idx === midx);
  if (manual?.rank) return [...manual.rank];
  return (editor.lastSnapshot?.stops ?? [])
    .filter((stop) => stop.midx === midx)
    .map((stop) => `s${stop.id}`);
}

/// The destination rank with the dragged control where the drop's
/// seam showed — in front of `beforeToken`, or at the bottom when
/// the drop carried no position (null, or a keyboard drop).
export function spliceRank(editor, midx, drag) {
  const token = dragToken(drag);
  const tokens = rankTokens(editor, midx).filter((t) => t !== token);
  const before = drag.insert?.beforeToken ?? null;
  const at = before == null ? tokens.length : tokens.indexOf(before);
  tokens.splice(at < 0 ? tokens.length : at, 0, token);
  return tokens;
}

/// `getInfo()` returns `{kind, payload, label}` for the drag about to
/// start, or null to refuse it. Called only once the pointer has
/// actually moved past the threshold, so it can read live state.
/// Whether the drag then also swallows the control's click is
/// endDrag's call: a release still over the source was a click.
export function wireDragSource(editor, el, getInfo) {
  el.addEventListener("pointerdown", (event) => {
    if (event.button !== 0 || editor.drag) return;
    if (!(event.ctrlKey || editor.unlocked)) return;
    event.stopPropagation(); // a control drag is never a panel move
    const startX = event.clientX;
    const startY = event.clientY;
    let moved = false;
    const onMove = (e) => {
      if (moved || e.pointerId !== event.pointerId) return;
      if (editor.drag) { cleanup(); return; }
      if (Math.hypot(e.clientX - startX, e.clientY - startY) < DRAG_THRESHOLD_PX) return;
      moved = true;
      cleanup();
      const info = getInfo();
      if (!info) return;
      startDrag(editor, e, info.kind, info.payload, info.label, el);
    };
    const cleanup = () => {
      window.removeEventListener("pointermove", onMove);
      window.removeEventListener("pointerup", onUp);
      window.removeEventListener("pointercancel", onUp);
      window.removeEventListener("blur", cleanup);
    };
    const onUp = (e) => { if (e.pointerId === event.pointerId) cleanup(); };
    window.addEventListener("pointermove", onMove);
    window.addEventListener("pointerup", onUp);
    window.addEventListener("pointercancel", onUp);
    window.addEventListener("blur", cleanup);
  });
}

export function startDrag(editor, event, kind, payload, label, source) {
  event.preventDefault();
  const ghost = document.createElement("div");
  ghost.className = "organ-drag-ghost";
  ghost.textContent = label;
  document.body.append(ghost);
  editor.drag = {
    kind,
    pointerId: event.pointerId,
    payload,
    ghost,
    label,
    source,
    targetType: null,
    targetIdx: null,
    insert: null,
  };
  positionGhost(editor, event.clientX, event.clientY);
  if (binAllowed(kind)) editor.el.bin.classList.add("visible");
  editor._dragMove = (e) => { if (e.pointerId === editor.drag?.pointerId) dragMove(editor, e); };
  editor._dragEnd = (e) => { if (e.pointerId === editor.drag?.pointerId) endDrag(editor, e); };
  editor._dragBlur = () => endDrag(editor, { type: "pointercancel" });
  window.addEventListener("pointermove", editor._dragMove);
  window.addEventListener("pointerup", editor._dragEnd);
  window.addEventListener("pointercancel", editor._dragEnd);
  window.addEventListener("blur", editor._dragBlur);
}

export function positionGhost(editor, x, y) {
  if (!editor.drag) return;
  editor.drag.ghost.style.left = `${x}px`;
  editor.drag.ghost.style.top = `${y}px`;
}

export function dragMove(editor, event) {
  if (!editor.drag) return;
  positionGhost(editor, event.clientX, event.clientY);
  applyDropHighlight(editor, findDropTarget(editor, event.clientX, event.clientY));
}

export function findDropTarget(editor, x, y) {
  const el = document.elementFromPoint(x, y);
  if (!el || !editor.drag) return null;
  if (el.closest("[data-drop-bin]") && binAllowed(editor.drag.kind)) return { type: "bin" };
  const shoe = el.closest(".shoe[data-enclosure]");
  if (shoe && encAllowed(editor.drag.kind)) {
    return { type: "shoe", idx: Number(shoe.dataset.enclosure) };
  }
  // The coupler rail takes a dragged coupler home: unseated from
  // whatever jamb held it, a tablet again.
  if (editor.drag.kind === "coupler" && el.closest(".panel-couplers")) {
    return { type: "rail" };
  }
  const manual = el.closest("[data-drop-manual]");
  if (manual && manualAllowed(editor.drag.kind)) {
    const hit = { type: "manual", idx: Number(manual.dataset.dropManual) };
    // Over a jamb division a dragged stop or coupler carries a
    // *position* too: where in the knob rank it would land. A
    // keyboard is a plain "onto this manual" target, as before.
    if (rankKind(editor.drag.kind) && manual.classList.contains("division")) {
      hit.insert = insertionPoint(editor, manual, x, y);
    }
    return hit;
  }
  return null;
}

/// Where in a division's knob rank the dragged control would land:
/// the nearest rank knob — stop or seated coupler, the dragged one
/// doesn't count — and which side of it the pointer sits, normalized
/// to "before this token", with null meaning the bottom of the rank.
/// Works unchanged when a resized jamb has wrapped the rank into
/// columns: nearest-knob is a distance, not an index.
export function insertionPoint(editor, division, x, y) {
  const dragged =
    editor.drag.kind === "coupler"
      ? `coupler-${editor.drag.payload.idx}`
      : `stop-${editor.drag.payload.id}`;
  const knobs = [
    ...division.querySelectorAll('.knob[data-key^="stop-"], .knob[data-key^="coupler-"]'),
  ].filter((knob) => knob.dataset.key !== dragged);
  if (!knobs.length) return { beforeToken: null, marker: null, side: "after" };
  let nearest = null;
  let best = Infinity;
  for (const knob of knobs) {
    const rect = knob.getBoundingClientRect();
    const dx = x - (rect.left + rect.width / 2);
    const dy = y - (rect.top + rect.height / 2);
    const d = dx * dx + dy * dy;
    if (d < best) {
      best = d;
      nearest = knob;
    }
  }
  const rect = nearest.getBoundingClientRect();
  // "stop-12" → "s12", "coupler-3" → "c3": the rank vocabulary.
  const token = (knob) =>
    knob.dataset.key.startsWith("coupler-")
      ? `c${knob.dataset.key.slice("coupler-".length)}`
      : `s${knob.dataset.key.slice("stop-".length)}`;
  // Which side of the nearest knob the pointer means: judged along
  // whichever axis it's further out on, so a wrapped grid reads
  // left/right within a row and above/below across rows — and the
  // seam is drawn on the matching edge.
  const dx = (x - (rect.left + rect.width / 2)) / rect.width;
  const dy = (y - (rect.top + rect.height / 2)) / rect.height;
  const before = Math.abs(dx) > Math.abs(dy) ? dx < 0 : dy < 0;
  const horizontal = Math.abs(dx) > Math.abs(dy);
  const side = horizontal ? (before ? "left" : "right") : before ? "before" : "after";
  if (before) {
    return { beforeToken: token(nearest), marker: nearest, side };
  }
  const next = knobs[knobs.indexOf(nearest) + 1] ?? null;
  return { beforeToken: next ? token(next) : null, marker: nearest, side };
}

export function applyDropHighlight(editor, hit) {
  for (const el of editor.root.querySelectorAll(".drop-target")) el.classList.remove("drop-target");
  for (const el of editor.root.querySelectorAll(
    ".insert-before, .insert-after, .insert-left, .insert-right"
  )) {
    el.classList.remove("insert-before", "insert-after", "insert-left", "insert-right");
  }
  editor.el.bin.classList.remove("drop-target");
  for (const el of editor.root.querySelectorAll(".panel-couplers.drop-target")) {
    el.classList.remove("drop-target");
  }
  editor.drag.targetType = hit?.type ?? null;
  editor.drag.targetIdx = hit?.idx ?? null;
  editor.drag.insert = hit?.insert
    ? { manual: hit.idx, beforeToken: hit.insert.beforeToken }
    : null;
  editor.drag.ghost.textContent = editor.drag.label;
  if (!hit) return;

  if (hit.type === "bin") {
    editor.el.bin.classList.add("drop-target");
    editor.drag.ghost.textContent =
      editor.drag.kind === "enclosure"
        ? `Remove the ${editor.drag.label} box`
        : editor.drag.kind === "manual"
          ? `Remove ${editor.drag.label}`
          : editor.drag.kind === "coupler"
            ? `Delete the ${editor.drag.label} coupler`
            : `Drop to remove ${editor.drag.label}`;
    return;
  }

  // Home again: a coupler over the rail reads as its tablet's return
  // — unless it never left.
  if (hit.type === "rail") {
    if (editor.drag.payload.midx == null) return;
    editor.root.querySelector('.panel[data-panel="couplers"]')?.classList.add("drop-target");
    editor.drag.ghost.textContent = `${editor.drag.label} → the coupler rail`;
    return;
  }

  if (hit.type === "shoe") {
    const shoeEl = editor.root.querySelector(`.shoe[data-enclosure="${hit.idx}"]`);
    shoeEl?.classList.add("drop-target");
    const enclosure = editor.lastSnapshot?.enclosures.find((e) => e.idx === hit.idx);
    const stop = editor.lastSnapshot?.stops.find((s) => s.id === editor.drag.payload.id);
    if (enclosure) {
      const already = stop?.enc?.includes(hit.idx);
      editor.drag.ghost.textContent = already
        ? `In ${enclosure.name} — drop to take out`
        : `Drop to add to ${enclosure.name}`;
    }
    return;
  }

  // Over a jamb division a dragged stop or coupler shows where it
  // would land — a seam beside the nearest knob — whether it's
  // coming home to its own rank (a pure reorder) or arriving from
  // the rail or another manual.
  if (rankKind(editor.drag.kind) && hit.insert) {
    hit.insert.marker?.classList.add(`insert-${hit.insert.side}`);
    const manual = editor.lastSnapshot?.manuals.find((m) => m.idx === hit.idx);
    editor.drag.ghost.textContent =
      hit.idx === editor.drag.payload.midx
        ? `Place ${editor.drag.label} here`
        : `${editor.drag.label} → ${manual?.name ?? "here"}`;
    return;
  }

  // Dropping a stop back on its own manual, or a manual's cheek on its
  // own board, isn't a move — no need to light it up as one.
  if (rankKind(editor.drag.kind) && hit.idx === editor.drag.payload.midx) return;
  if (editor.drag.kind === "manual" && hit.idx === editor.drag.payload.idx) return;
  for (const el of editor.root.querySelectorAll(`[data-drop-manual="${hit.idx}"]`)) {
    el.classList.add("drop-target");
  }
  const manual = editor.lastSnapshot?.manuals.find((m) => m.idx === hit.idx);
  if (manual) editor.drag.ghost.textContent = `${editor.drag.label} → ${manual.name}`;
}

export function endDrag(editor, event) {
  window.removeEventListener("pointermove", editor._dragMove);
  window.removeEventListener("pointerup", editor._dragEnd);
  window.removeEventListener("pointercancel", editor._dragEnd);
  window.removeEventListener("blur", editor._dragBlur);
  const drag = editor.drag;
  editor.drag = null;
  if (!drag) return;
  drag.ghost.remove();
  editor.el.bin.classList.remove("visible", "drop-target");
  for (const el of editor.root.querySelectorAll(".drop-target")) el.classList.remove("drop-target");
  for (const el of editor.root.querySelectorAll(
    ".insert-before, .insert-after, .insert-left, .insert-right"
  )) {
    el.classList.remove("insert-before", "insert-after", "insert-left", "insert-right");
  }

  if (event.type === "pointercancel") return;

  // Let go where it was picked up: the pointer never left the
  // control, so nobody meant to drop it anywhere — that was a click
  // with a wobble, and the control's own click goes through.
  // Otherwise the click the browser fires after this release belongs
  // to the drag and must not also toggle or open anything.
  if (drag.source) {
    if (withinRect(drag.source.getBoundingClientRect(), event)) return;
    suppressNextClick(drag.source);
  }

  const { targetType, targetIdx } = drag;
  if (!targetType) return;

  if (drag.kind === "stop") {
    if (targetType === "bin") {
      editor.organCommand(commands.organUnpull(drag.payload.id));
    } else if (targetType === "shoe") {
      const enclosure = editor.lastSnapshot?.enclosures.find((e) => e.idx === targetIdx);
      const stop = editor.lastSnapshot?.stops.find((s) => s.id === drag.payload.id);
      if (enclosure) {
        const already = stop?.enc?.includes(targetIdx);
        editor.organCommand(commands.organEnclosureAssign(enclosure.name, drag.payload.id, !already));
      }
    } else if (targetType === "manual") {
      const sameManual = targetIdx === drag.payload.midx;
      if (drag.insert && drag.insert.manual === targetIdx) {
        // The drop carried a position: deal the destination rank out
        // anew with the dragged stop where the seam showed — tokens,
        // so any couplers seated in the rank keep their places.
        const tokens = spliceRank(editor, targetIdx, drag);
        if (sameManual) {
          editor.organCommand(commands.organRankOrder(targetIdx, tokens));
        } else {
          // Arriving from another manual: move first (live), then
          // place — the queue waits each response out, and refusals
          // surface like any other edit's.
          editor.runQueue([
            commands.organMove(drag.payload.id, targetIdx),
            commands.organRankOrder(targetIdx, tokens),
          ]);
        }
      } else if (!sameManual) {
        // A keyboard drop names no position — the stop joins the
        // manual at the bottom of its rank, as it always has. Live,
        // but the server refuses it mid-rebuild (stale names would
        // poison the file), so it goes through the queue.
        editor.runQueue([commands.organMove(drag.payload.id, targetIdx)]);
      }
    }
  } else if (drag.kind === "coupler") {
    if (targetType === "bin") {
      editor.showRemoveConfirm("coupler", drag.payload);
    } else if (targetType === "rail") {
      // Home to the rail: the seat's division deals its rank out
      // without the coupler, which unseats it.
      if (drag.payload.midx != null) {
        const token = dragToken(drag);
        const tokens = rankTokens(editor, drag.payload.midx).filter((t) => t !== token);
        editor.organCommand(commands.organRankOrder(drag.payload.midx, tokens));
      }
    } else if (targetType === "manual") {
      // Seat it in the jamb where the seam showed — or, from a
      // keyboard drop, at the bottom of that division's rank. The
      // server unseats it everywhere else.
      editor.organCommand(
        commands.organRankOrder(targetIdx, spliceRank(editor, targetIdx, drag))
      );
    }
  } else if (drag.kind === "manual") {
    if (targetType === "bin") {
      editor.showRemoveConfirm("manual", drag.payload);
    } else if (targetType === "manual" && targetIdx !== drag.payload.idx) {
      editor.organCommand(commands.organManualOrder(drag.payload.idx, targetIdx));
    }
  } else if (drag.kind === "enclosure" && targetType === "bin") {
    editor.showRemoveConfirm("enclosure", drag.payload);
  } else if (drag.kind === "offering-stop" && targetType === "manual") {
    const manual = editor.lastSnapshot?.manuals.find((m) => m.idx === targetIdx);
    if (manual) {
      editor.organCommand(
        commands.organPull(drag.payload.alias, drag.payload.manualName, manual.name, drag.payload.stopName)
      );
    }
  } else if (drag.kind === "offering-division" && targetType === "manual") {
    const manual = editor.lastSnapshot?.manuals.find((m) => m.idx === targetIdx);
    if (manual) editor.organCommand(commands.organPull(drag.payload.alias, drag.payload.manualName, manual.name));
  }
}
