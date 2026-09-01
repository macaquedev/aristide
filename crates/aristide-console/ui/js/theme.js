// Client-side appearance settings — accent colour, layout density and
// the size of the whole console. Purely cosmetic and local to this
// console: stored in localStorage, never sent to the server, applied
// before the first snapshot arrives.

// Each accent carries its own dark ink (text over the accent) and a
// deeper shade (held sharps), picked by eye rather than derived, so
// every choice keeps contrast. The 1f/33 alpha suffixes match the
// soft/glow tokens in style.css. The pickers live in the Preferences
// dialog, which is exactly and only this: the player's own settings.
// LED hues: an engaged control fills with the accent and its legend
// goes ink, like any lit button on a control surface.
const ACCENTS = {
  amber: { accent: "#ffb02e", deep: "#e08900", ink: "#241500" },
  cyan: { accent: "#41c7e8", deep: "#279db8", ink: "#04222b" },
  green: { accent: "#7bd45b", deep: "#54a83a", ink: "#0e2405" },
  violet: { accent: "#a08eff", deep: "#7f6ae6", ink: "#191140" },
  coral: { accent: "#ff7a54", deep: "#e05426", ink: "#2b0d02" },
};

const DENSITIES = ["compact", "regular", "spacious"];

// Whole-console size, as a browser's zoom: 1 is the native design size
// (a 13px label is 13 device-independent pixels). Density changes what
// fits; scale changes how big it all is, text included — the two are
// independent and compose. This is a *webview* zoom, done by the
// desktop shell (see `set_zoom` in main.rs), never a CSS zoom: the
// panel canvas, every drag and every popover are laid out in CSS
// pixels, and a page-level zoom keeps those pixels the ones the code
// measures in. In a plain browser the browser's own zoom (Ctrl +/−)
// already does exactly this and remembers it per site, so the row
// only points there.
const SCALES = [0.5, 0.6, 0.7, 0.8, 0.9, 1, 1.1, 1.25, 1.5, 1.75, 2];
const NATIVE_SCALE = 1;

const store = {
  get(key, fallback) {
    try {
      return localStorage.getItem(`aristide.${key}`) ?? fallback;
    } catch {
      return fallback;
    }
  },
  set(key, value) {
    try {
      localStorage.setItem(`aristide.${key}`, value);
    } catch {
      /* private mode etc. — the choice just won't survive a restart */
    }
  },
};

function applyAccent(name) {
  const { accent, deep, ink } = ACCENTS[name] ?? ACCENTS.amber;
  const root = document.documentElement.style;
  root.setProperty("--accent", accent);
  root.setProperty("--accent-soft", `${accent}24`);
  root.setProperty("--accent-deep", deep);
  root.setProperty("--accent-ink", ink);
}

function applyDensity(name) {
  document.body.dataset.density = DENSITIES.includes(name) ? name : "regular";
}

const hostZooms = () => Boolean(window.__TAURI__);

function applyScale(scale) {
  if (!hostZooms()) return;
  window.__TAURI__.core.invoke("set_zoom", { scale }).catch((err) => {
    console.warn("console scale not applied:", err);
  });
}

function readScale() {
  const scale = Number(store.get("scale", NATIVE_SCALE));
  return SCALES.includes(scale) ? scale : NATIVE_SCALE;
}

let scale = NATIVE_SCALE;
let onScaleChange = () => {};

function chooseScale(next) {
  scale = next;
  store.set("scale", String(next));
  applyScale(next);
  onScaleChange(next);
}

/// Ctrl+plus / Ctrl+minus / Ctrl+0 as any browser has them, stepping
/// through the same choices the Preferences row offers so the two never
/// disagree. Returns false when the host does its own zooming (a plain
/// browser), so the caller leaves the keystroke to it.
export function stepScale(direction) {
  if (!hostZooms()) return false;
  const at = SCALES.indexOf(scale);
  const next = direction === 0
    ? NATIVE_SCALE
    : SCALES[Math.min(SCALES.length - 1, Math.max(0, at + direction))];
  if (next !== scale) chooseScale(next);
  return true;
}

/// A row of exclusive chips: `render(value)` names each, `onPick`
/// hears the choice, and the chip for `current` starts lit.
function segmented(segment, values, current, render, onPick) {
  for (const value of values) {
    const chip = document.createElement("button");
    chip.textContent = render(value);
    chip.dataset.value = String(value);
    chip.classList.toggle("on", value === current);
    chip.addEventListener("click", () => {
      onPick(value);
      for (const other of segment.children) {
        other.classList.toggle("on", other === chip);
      }
    });
    segment.append(chip);
  }
  return (value) => {
    for (const chip of segment.children) {
      chip.classList.toggle("on", chip.dataset.value === String(value));
    }
  };
}

/// Builds the picker rows in the Preferences dialog and restores the
/// saved choices. Call once at startup.
export function wireTheme(root) {
  const swatches = root.getElementById("accent-swatches");
  const densities = root.getElementById("density-segment");
  const scales = root.getElementById("scale-segment");
  const scaleNote = root.getElementById("scale-note");

  let accent = store.get("accent", "amber");
  applyAccent(accent);
  for (const [name, { accent: colour }] of Object.entries(ACCENTS)) {
    const swatch = document.createElement("button");
    swatch.className = "swatch";
    swatch.style.setProperty("--c", colour);
    swatch.setAttribute("aria-label", `${name} accent`);
    swatch.classList.toggle("on", name === accent);
    swatch.addEventListener("click", () => {
      accent = name;
      store.set("accent", name);
      applyAccent(name);
      for (const other of swatches.children) {
        other.classList.toggle("on", other === swatch);
      }
    });
    swatches.append(swatch);
  }

  const density = store.get("density", "regular");
  applyDensity(density);
  segmented(densities, DENSITIES, density, (name) => name.toUpperCase(), (name) => {
    store.set("density", name);
    applyDensity(name);
  });

  scale = readScale();
  applyScale(scale);
  onScaleChange = segmented(scales, SCALES, scale, (value) => `${Math.round(value * 100)}%`, chooseScale);
  if (hostZooms()) {
    scaleNote.textContent = "Ctrl + and Ctrl − step through these; Ctrl 0 returns to 100%.";
  } else {
    scales.setAttribute("aria-disabled", "true");
    scaleNote.textContent = "Not here: in a browser its own zoom (Ctrl +/−) sizes the console and is remembered per site.";
  }
}
