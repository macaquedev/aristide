import { resolveBase, connect } from "./api.js";
import { Console } from "./console.js";
import { PianoKeys } from "./keys.js";
import { MenuBar } from "./menu.js";
import { Picker } from "./picker.js";
import { Preferences } from "./prefs.js";
import { wireTheme } from "./theme.js";

wireTheme(document);

const base = await resolveBase();
let send;
const prefs = new Preferences(document, base, (query) => send(query));
const picker = new Picker(document, base, (query) => send(query));
const view = new Console(document, (query) => send(query), (tab) => prefs.open(tab));
const keys = new PianoKeys(document, (query) => send(query));

function fullscreen() {
  if (document.fullscreenElement) document.exitFullscreen();
  else document.documentElement.requestFullscreen?.();
}

// Menus are rebuilt each time one is pulled down, so every item states
// what is true at that moment — whether the legend is up, whether the
// window is full screen. Playing itself needs no menu: the computer
// keyboard is assigned in Preferences → MIDI like any other device,
// and shifting it is a binding like any other.
new MenuBar(document, document.getElementById("menus"), [
  {
    title: "Load",
    items: () => [
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
  (snapshot) => {
    view.render(snapshot);
    keys.update(snapshot);
    prefs.update(snapshot);
    picker.update(snapshot);
  },
  (message) => view.offline(message),
);
