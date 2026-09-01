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
/// The save-as dialog is a modal, not here — it only ever closes the
/// plain save form (openSaveAsForm does that itself) and nothing
/// closes it back from this table.
export const POPOVER_CLOSES = {
  tuning: ["hex", "coupler", "midi", "compass", "room", "bindings", "save"],
  midi: ["tuning", "hex", "coupler", "compass", "room", "bindings", "save"],
  compass: ["tuning", "hex", "coupler", "midi", "room", "bindings", "save"],
  room: ["tuning", "hex", "coupler", "midi", "compass", "bindings", "save"],
  bindings: ["tuning", "hex", "coupler", "midi", "compass", "room", "save"],
  save: ["tuning", "hex", "coupler", "midi", "compass", "room", "bindings"],
  trem: ["tuning", "hex", "coupler"],
  stop: ["tuning", "hex", "coupler", "trem", "midi", "compass", "room", "bindings", "save"],
  coupler: ["tuning", "hex", "trem", "stop", "midi", "compass", "room", "bindings", "save"],
  hex: ["tuning", "coupler"],
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
    stop: { el: editor.el.stop, close: () => editor.closeStopForm() },
    coupler: { el: editor.el.coupler, close: () => editor.closeCouplerForm() },
    hex: { el: editor.el.hex, close: () => editor.closeHexForm() },
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
