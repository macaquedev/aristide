// The popover registry: one source of truth for what's open in the
// editor, and what opening one popover closes.
//
// Every popover's kind, its element, and how to close it — so a closer
// needn't be named again every place one popover has to make way for
// another. POPOVER_CLOSES is the only place that says what opening one
// closes; every openXForm calls openingPopover(editor, kind) instead of
// hand-listing closeYForm() calls.

/// What opening one of `popovers(editor)`'s kinds has always closed
/// among the others — asymmetric on purpose in spots, not tidied here
/// into a blanket "close everything": the settings cluster (tuning,
/// midi, compass, room, bindings, save) never touches trem or the stop
/// editor; trem closes only tuning/hex/coupler; hex closes only
/// tuning/coupler; the stop and coupler editors close everything else.
/// keyVoicing is the one exception to the symmetry: it is a SUBVIEW of
/// the stop editor (right-click a key while a stop's editor is open),
/// so it closes everything the stop editor does except the stop editor
/// itself, and opening a stop editor closes it.
/// The save-as dialog is a modal, not here — it only ever closes the
/// plain save form (openSaveAsForm does that itself) and nothing
/// closes it back from this table.
export const POPOVER_CLOSES = {
  tuning: ["hex", "coupler", "midi", "compass", "room", "bindings", "save", "piston", "keyVoicing"],
  midi: ["tuning", "hex", "coupler", "compass", "room", "bindings", "save", "piston", "keyVoicing"],
  compass: ["tuning", "hex", "coupler", "midi", "room", "bindings", "save", "piston", "keyVoicing"],
  room: ["tuning", "hex", "coupler", "midi", "compass", "bindings", "save", "piston", "keyVoicing"],
  bindings: ["tuning", "hex", "coupler", "midi", "compass", "room", "save", "piston", "keyVoicing"],
  save: ["tuning", "hex", "coupler", "midi", "compass", "room", "bindings", "piston", "keyVoicing"],
  trem: ["tuning", "hex", "coupler"],
  stop: ["tuning", "hex", "coupler", "trem", "midi", "compass", "room", "bindings", "save", "piston", "keyVoicing"],
  // The key-voicing popover is a subview of the stop editor: it closes
  // the others but NOT the editor it belongs to, and nothing but a
  // close-everything closes it back.
  keyVoicing: ["tuning", "hex", "coupler", "trem", "midi", "compass", "room", "bindings", "save", "piston"],
  coupler: ["tuning", "hex", "trem", "stop", "midi", "compass", "room", "bindings", "save", "piston", "keyVoicing"],
  hex: ["tuning", "coupler"],
  // The piston popover is a single quick-bind row; it makes way for
  // everything and everything makes way for it.
  piston: ["tuning", "hex", "coupler", "trem", "stop", "midi", "compass", "room", "bindings", "save", "keyVoicing"],
};

export function popovers(editor) {
  return {
    tuning: { el: editor.el.tuning, close: () => editor.closeTuningForm() },
    midi: { el: editor.el.midi, close: () => editor.closeMidiForm() },
    compass: { el: editor.el.compass, close: () => editor.closeCompassForm() },
    room: { el: editor.el.room, close: () => editor.closeRoomForm() },
    bindings: { el: editor.el.bindings, close: () => editor.closeBindingsForm() },
    save: { el: editor.el.save, close: () => editor.closeSaveForm() },
    trem: { el: editor.el.trem, close: () => editor.closeTremForm() },
    keyVoicing: { el: editor.el.keyVoicing, close: () => editor.closeKeyVoicing() },
    stop: { el: editor.el.stop, close: () => editor.closeStopForm() },
    coupler: { el: editor.el.coupler, close: () => editor.closeCouplerForm() },
    hex: { el: editor.el.hex, close: () => editor.closeHexForm() },
    piston: { el: editor.el.piston, close: () => editor.closePistonForm() },
    saveAs: { el: editor.el.saveAs, close: () => editor.closeSaveAsForm() },
  };
}

/// The three quick menus every popover makes way for — never each
/// other, since a division/keyboard-menu item is how most of these
/// popovers open in the first place.
export function closeQuickMenus(editor) {
  editor.closeAdd();
  editor.closeDivisionMenu();
  editor.closeKeyboardMenu();
}

/// Closes the quick menus and whatever `POPOVER_CLOSES[kind]` says
/// this kind has always closed, in place of the openXForm functions
/// each hand-listing their own subset of closeYForm() calls.
export function openingPopover(editor, kind) {
  closeQuickMenus(editor);
  const registry = popovers(editor);
  for (const other of POPOVER_CLOSES[kind] ?? []) registry[other].close();
}

/// Every popover and quick menu, closed at once — a click outside all
/// of them, or Escape. Never the save-as dialog: it's a modal, closed
/// only by its own Escape/Cancel/backdrop handling.
export function closeAllPopovers(editor) {
  closeQuickMenus(editor);
  editor.closeCouplersMenu();
  const registry = popovers(editor);
  for (const kind of Object.keys(registry)) {
    if (kind !== "saveAs") registry[kind].close();
  }
}
