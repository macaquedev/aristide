import { OrganPreferences } from "./organ-prefs.js";
import { wireDialogFocus } from "./dialog-focus.js";
import { resolveBase, connect, commands } from "./api.js";
import { ConflictDialog } from "./conflict.js";
import { Console } from "./console.js";
import { Editor } from "./editor.js";
import { applyHarnessHooks } from "./harness-hooks.js";
import { PianoKeys } from "./keys.js";
import { Workspaces } from "./workspaces.js";
import { Picker } from "./picker.js";
import { Preferences } from "./prefs.js";
import { stepScale, wireTheme } from "./theme.js";

wireTheme(document);
wireDialogFocus(document);

// The webview's own context menu (Stop, Reload…) never belongs on an
// organ console. Right-clicks that mean something are answered where
// they land (see editor.js's canvas wiring); the rest do nothing —
// except on fields that take text, where the menu is how paste happens.
window.addEventListener("contextmenu", (event) => {
  if (event.target.closest("textarea, input[type=text], input[type=number], input:not([type])")) return;
  event.preventDefault();
});

const base = await resolveBase();
let send;
let snapshot = {}; // the latest state, for menus that ask what is true now
const prefs = new Preferences(document, (query) => send(query));
const picker = new Picker(document, base, (query, options) => send(query, options));
const editor = new Editor(document, base, (query) => send(query));
const organPrefs = new OrganPreferences(document, editor);
// The bar's tuning readout opens the whole-instrument tuning popover —
// an organ fact, edited where organ facts are edited, on the console.
const view = new Console(
  document,
  (query) => send(query),
  (x, y) => editor.openTuningForm("organ", x, y),
  (x, y) => editor.beginBuild(x, y)
);
view.onKeyboardSettings = (idx) => editor.openMidiForm(idx, 20, 80);
view.decorate = (snapshot) => editor.decorateConsole(snapshot);
const keys = new PianoKeys(document, (query) => send(query));
const conflict = new ConflictDialog(document, (query) => send(query));

// See harness-hooks.js: a handful of `?param` switches the screenshot
// script uses to reach states a static screenshot can't drive to itself.
// Inert without those params.
applyHarnessHooks({ prefs, editor });

const workspaces = new Workspaces({ root: document, editor, organPrefs, prefs, picker, view, keys });

// Ctrl+plus / minus / 0 size the console as they would a browser page.
// In a browser they still do — stepScale declines and the keystroke
// falls through to the browser's own zoom.
const ZOOM_KEYS = { "=": 1, "+": 1, "-": -1, "0": 0 };

window.addEventListener("keydown", (event) => {
  if (!(event.ctrlKey || event.metaKey) || event.altKey) return;
  if (event.key === ",") {
    event.preventDefault();
    if (workspaces.mode !== "play") workspaces.navigate("setup");
  } else if (event.key in ZOOM_KEYS && stepScale(ZOOM_KEYS[event.key])) {
    event.preventDefault();
  }
});

send = connect(
  base,
  (state) => {
    snapshot = state;
    view.render(snapshot);
    keys.update(snapshot);
    picker.update(snapshot);
    conflict.update(snapshot);
    editor.update(snapshot);
    prefs.update(snapshot);
    organPrefs.update(snapshot);
    workspaces.update(snapshot);
  },
  (message) => view.offline(message),
  // A refused command (a 4xx and its reason) lands in the editor's
  // status strip — the same place rebuild errors show — instead of
  // masquerading as a lost connection. Except a 409: that is the sample
  // set's own organ refusing to change its instrument (the player's
  // wiring, room, pitch and layout it takes), answered with the save-as
  // dialog, which holds the command and sends it again once the organ
  // has a name of its own.
  (reason, status, query) => {
    if (status === 409) editor.openSaveAsForm(query);
    else editor.showError(reason);
  },
);
