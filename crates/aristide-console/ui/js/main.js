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
// what is true at that moment — which manual the computer keyboard
// plays, whether the legend is up, whether the window is full screen.
new MenuBar(document, document.getElementById("menus"), [
  {
    title: "Organ",
    items: () => [
      { label: "Open organ…", run: () => picker.open() },
      "-",
      { label: "Cancel registration", run: () => view.cancel() },
      { label: "Silence everything", accel: "Panic", run: () => view.panic() },
      "-",
      { label: "Preferences…", accel: "Ctrl ,", run: () => prefs.open() },
    ],
  },
  {
    title: "Play",
    items: () => [
      { label: "Computer keyboard map", check: keys.isOpen, run: () => keys.toggle() },
      // Accelerators are bindings now, not built-ins: what shifts the
      // keyboard is whatever the player has bound to octave-up and
      // octave-down, which may equally be a piston on their console.
      { label: "Octave down", run: () => keys.shift(-1) },
      { label: "Octave up", run: () => keys.shift(+1) },
      "-",
      `SHIFTED ${keys.transpose > 0 ? "+" : ""}${keys.transpose} SEMITONES`,
      "-",
      "COMPUTER KEYBOARD PLAYS",
      ...keys.manuals.map((manual) => ({
        label: manual.name,
        radio: keys.target?.idx === manual.idx,
        run: () => keys.setTarget(manual.idx),
      })),
    ],
  },
  {
    title: "View",
    items: () => [
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
