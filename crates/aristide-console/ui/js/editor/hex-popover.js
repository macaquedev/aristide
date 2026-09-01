// The hex-layout popover: a microtonal manual's isomorphic grid.
//
// Two step-vectors (right, up-right, in key-number steps), the grid
// size, and the bottom-left key. Every field posts on change —
// structural, so the keyboard redraws and the next snapshot echoes
// back what the server settled on (a preset refits the width, wild
// values clamp), keeping the form honest the same way the tuning
// popover is.

import { commands } from "../api.js";
import { setText } from "../dom.js";
import { keyName, parseKeyName } from "../pitch.js";

export function wireHexForm(editor) {
  editor.el.hexClose.addEventListener("click", () => editor.closeHexForm());
  editor.el.hexReset.addEventListener("click", () => hexCommand(editor, { reset: 1 }));
  for (const button of editor.el.hex.querySelectorAll("[data-preset]")) {
    button.addEventListener("click", () => hexCommand(editor, { preset: button.dataset.preset }));
  }
  for (const [field, input] of [
    ["right", editor.el.hexRight],
    ["upright", editor.el.hexUpright],
    ["rows", editor.el.hexRows],
    ["cols", editor.el.hexCols],
  ]) {
    input.addEventListener("change", () => {
      if (editor.hexManual == null) return;
      const value = Number(input.value);
      if (Number.isInteger(value)) hexCommand(editor, { [field]: value });
    });
  }
  editor.el.hexAnchor.addEventListener("change", () => {
    if (editor.hexManual == null) return;
    // A note name ("C2") or a raw key number — numbers past MIDI's
    // 127 are legal on a widened manual, so they pass through.
    const text = editor.el.hexAnchor.value.trim();
    const key = parseKeyName(text) ?? (/^\d+$/.test(text) ? Number(text) : null);
    if (key == null || key > 65535) {
      showHexError(editor, `${text || "(empty)"} does not name a key`);
      return;
    }
    hexCommand(editor, { anchor: key });
  });
}

export function openHexForm(editor, idx, x, y) {
  editor.openingPopover("hex");
  editor.hexManual = idx;
  hideHexError(editor);
  editor.syncHexForm();
  editor.el.hex.classList.remove("hidden");
  editor.positionPopover(editor.el.hex, x, y);
}

export function closeHexForm(editor) {
  editor.hexManual = null;
  editor.el.hex.classList.add("hidden");
  hideHexError(editor);
}

/// Refills the form from the snapshot's effective layout — on open
/// and on every poll, so the server's settling (clamps, refits,
/// another session's edit) lands in the fields. A manual that
/// stopped being microtonal takes its popover with it.
export function syncHexForm(editor) {
  const idx = editor.hexManual;
  const manual = editor.lastSnapshot?.manuals.find((m) => m.idx === idx);
  if (!manual?.hex) {
    editor.closeHexForm();
    return;
  }
  setText(editor.el.hexTitle, `${manual.name} · hex layout`);
  const fields = [
    [editor.el.hexRight, manual.hex.right],
    [editor.el.hexUpright, manual.hex.upright],
    [editor.el.hexRows, manual.hex.rows],
    [editor.el.hexCols, manual.hex.cols],
    [
      editor.el.hexAnchor,
      manual.hex.anchor <= 127 ? keyName(manual.hex.anchor) : String(manual.hex.anchor),
    ],
  ];
  for (const [input, value] of fields) {
    if (editor.root.activeElement !== input) input.value = value;
  }
}

async function hexCommand(editor, fields) {
  if (editor.hexManual == null) return false;
  hideHexError(editor);
  const { ok, error } = await editor.organCommandResult(
    commands.organManualHex(editor.hexManual, fields)
  );
  if (error != null) showHexError(editor, error);
  return ok;
}

function showHexError(editor, text) {
  editor.el.hexError.textContent = text;
  editor.el.hexError.classList.remove("hidden");
}

function hideHexError(editor) {
  editor.el.hexError.classList.add("hidden");
  editor.el.hexError.textContent = "";
}
