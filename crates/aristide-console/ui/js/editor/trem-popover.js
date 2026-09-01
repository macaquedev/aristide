// The tremulant-shape popover: right-click the Tremblant knob.
//
// A tremulant is a valve venting the wind, so its shape is spoken in
// wind terms: rate, pitch depth in cents (gain and timbre follow
// pressure physically), spin-up, unevenness. Every field posts on
// change — live on the engine, written to the organ file's
// [tremulant] section — and the next poll echoes what the server
// settled, the tuning popover's contract.

import { commands } from "../api.js";
import { setText } from "../dom.js";

/// The Tremblant knob doubles as the tremulant's editor: right-click
/// while unlocked (or ctrl through the lock) opens the shape popover.
/// Wave tremulants offer no shape, so the gesture stays silent then.
export function wireTremKnob(editor) {
  const knob = editor.root.querySelector('[data-key="trem"]');
  if (!knob) return;
  knob.addEventListener("contextmenu", (event) => {
    if (!(editor.unlocked || event.ctrlKey)) return;
    if (!shapeableTrem(editor)) return;
    event.preventDefault();
    event.stopPropagation();
    editor.openTremForm(event.clientX, event.clientY);
  });
}

export function wireTremForm(editor) {
  editor.el.tremClose.addEventListener("click", () => editor.closeTremForm());
  for (const [field, input] of [
    ["rate", editor.el.tremRate],
    ["depth", editor.el.tremDepth],
    ["ramp", editor.el.tremRamp],
    ["wobble", editor.el.tremWobble],
  ]) {
    input.addEventListener("change", () => {
      if (editor.tremOpen == null) return;
      const value = Number(input.value);
      if (!Number.isFinite(value)) return;
      tremCommand(editor, { idx: editor.tremOpen, [field]: value });
    });
  }
}

/// The first shapeable tremulant — wave trems are recorded in their
/// samples and offer nothing to edit.
function shapeableTrem(editor) {
  return (editor.lastSnapshot?.trems ?? []).find((t) => !t.wave) ?? null;
}

export function openTremForm(editor, x, y) {
  const trem = shapeableTrem(editor);
  if (!trem) return;
  editor.openingPopover("trem");
  editor.tremOpen = trem.idx;
  hideTremError(editor);
  editor.syncTremForm();
  editor.el.trem.classList.remove("hidden");
  editor.positionPopover(editor.el.trem, x, y);
}

export function closeTremForm(editor) {
  editor.tremOpen = null;
  editor.el.trem.classList.add("hidden");
  hideTremError(editor);
}

export function syncTremForm(editor) {
  const trem = (editor.lastSnapshot?.trems ?? []).find((t) => t.idx === editor.tremOpen);
  if (!trem || trem.wave) {
    editor.closeTremForm();
    return;
  }
  setText(editor.el.tremTitle, trem.name);
  for (const [input, value] of [
    [editor.el.tremRate, trem.rate],
    [editor.el.tremDepth, trem.depth],
    [editor.el.tremRamp, trem.ramp],
    [editor.el.tremWobble, trem.wobble],
  ]) {
    if (editor.root.activeElement !== input) input.value = value;
  }
}

async function tremCommand(editor, fields) {
  hideTremError(editor);
  const { ok, error } = await editor.organCommandResult(commands.tremParams(fields));
  if (error != null) showTremError(editor, error);
  return ok;
}

function showTremError(editor, text) {
  editor.el.tremError.textContent = text;
  editor.el.tremError.classList.remove("hidden");
}

function hideTremError(editor) {
  editor.el.tremError.classList.add("hidden");
  editor.el.tremError.textContent = "";
}
