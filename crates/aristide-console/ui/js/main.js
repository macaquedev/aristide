import { resolveBase, connect, commands } from "./api.js";
import { ConflictDialog } from "./conflict.js";
import { Console } from "./console.js";
import { Editor } from "./editor.js";
import { applyHarnessHooks } from "./harness-hooks.js";
import { PianoKeys } from "./keys.js";
import { MenuBar } from "./menu.js";
import { Picker } from "./picker.js";
import { Preferences } from "./prefs.js";
import { wireTheme } from "./theme.js";

wireTheme(document);

// The webview's own context menu (Stop, Reload…) never belongs on an
// organ console. Right-clicks that mean something are answered where
// they land (see editor.js's canvas wiring); the rest do nothing.
window.addEventListener("contextmenu", (event) => event.preventDefault());

const base = await resolveBase();
let send;
let snapshot = {}; // the latest state, for menus that ask what is true now
const prefs = new Preferences(document, base, (query) => send(query));
const picker = new Picker(document, base, (query) => send(query));
const editor = new Editor(document, base, (query) => send(query));
const view = new Console(document, (query) => send(query), (tab) => prefs.open(tab), () => editor.unlock());
view.decorate = (snapshot) => editor.decorateConsole(snapshot);
const keys = new PianoKeys(document, (query) => send(query));
const conflict = new ConflictDialog(document, (query) => send(query));

// See harness-hooks.js: a handful of `?param` switches the screenshot
// script uses to reach states a static screenshot can't drive to itself.
// Inert without those params.
applyHarnessHooks({ prefs, editor });

function fullscreen() {
  if (document.fullscreenElement) document.exitFullscreen();
  else document.documentElement.requestFullscreen?.();
}

// Renaming happens right where the name is shown: the organ's name in
// the bar becomes a text field, Enter or clicking away commits, Escape
// abandons. The server owns what a rename really means (the file, the
// wiring key, the library), so this only sends the new name.
const organButton = document.getElementById("organ-name");
const renameForm = document.getElementById("organ-rename-form");
const renameInput = document.getElementById("organ-rename");
let renaming = false;

function startRename() {
  renaming = true;
  renameInput.value = snapshot.organ ?? "";
  organButton.classList.add("hidden");
  renameForm.classList.remove("hidden");
  renameInput.focus();
  renameInput.select();
}

function endRename(commit) {
  if (!renaming) return; // the submit's blur must not commit twice
  renaming = false;
  const name = renameInput.value.trim();
  if (commit && name && name !== snapshot.organ) {
    send(commands.organRename(name));
  }
  renameForm.classList.add("hidden");
  organButton.classList.remove("hidden");
}

renameForm.addEventListener("submit", (event) => {
  event.preventDefault();
  endRename(true);
});
renameInput.addEventListener("blur", () => endRename(true));
renameInput.addEventListener("keydown", (event) => {
  if (event.key === "Escape") {
    event.stopPropagation();
    endRename(false);
  }
});

// Menus are rebuilt each time one is pulled down, so every item states
// what is true at that moment — whether the legend is up, whether the
// window is full screen. Playing itself needs no menu: the computer
// keyboard is assigned in Preferences → MIDI like any other device,
// and shifting it is a binding like any other.
//
// The first menu is the organ's own name: the instrument is picked and
// renamed where it is named. An ad-hoc combination has no file to keep
// a name in, so renaming waits until it is saved as one.
new MenuBar(document, document.getElementById("menus"), [
  {
    button: organButton,
    list: document.getElementById("organ-menu-list"),
    items: () => [
      {
        // An ad-hoc combination has no file to keep a name in; the
        // label says why the item waits rather than just greying out.
        label: snapshot.setup?.implicit
          ? "Rename organ… (save the combination first)"
          : "Rename organ…",
        disabled: !snapshot.organ || !!snapshot.setup?.implicit,
        run: startRename,
      },
      "-",
      { label: "Load an organ…", run: () => picker.open() },
      { label: "New blank organ…", run: () => picker.newBlank() },
      { label: "New organ from a sample set…", run: () => picker.newFromSet() },
      ...(picker.library.length
        ? [
            "-",
            "Recent",
            ...picker.library.slice(0, 8).map((entry) => ({
              label: entry.name,
              run: () => picker.load(entry.path),
            })),
          ]
        : []),
    ],
  },
  {
    title: "Organ",
    items: () => [
      { label: "Cancel registration", run: () => view.cancel() },
      { label: "Silence everything", accel: "Panic", run: () => view.panic() },
      "-",
      { label: "Preferences…", accel: "Ctrl ,", run: () => prefs.open() },
    ],
  },
  {
    title: "View",
    items: () => [
      { label: "Computer keyboard map", check: keys.isOpen, run: () => keys.toggle() },
      {
        label: "Full screen",
        check: Boolean(document.fullscreenElement),
        run: fullscreen,
      },
      "-",
      { label: "Appearance…", run: () => prefs.open("appearance") },
    ],
  },
  {
    title: "Help",
    items: () => [{ label: "About Aristide", run: () => prefs.openAbout() }],
  },
]);

window.addEventListener("keydown", (event) => {
  if (event.key === "," && (event.ctrlKey || event.metaKey)) {
    event.preventDefault();
    prefs.isOpen ? prefs.close() : prefs.open();
  }
});

send = connect(
  base,
  (state) => {
    snapshot = state;
    view.render(snapshot);
    keys.update(snapshot);
    prefs.update(snapshot);
    picker.update(snapshot);
    conflict.update(snapshot);
    editor.update(snapshot);
  },
  (message) => view.offline(message),
  // A refused command (a 4xx and its reason) lands in the editor's
  // status strip — the same place rebuild errors show — instead of
  // masquerading as a lost connection.
  (reason) => editor.showError(reason),
);
