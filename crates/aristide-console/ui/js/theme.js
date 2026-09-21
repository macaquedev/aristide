// Screen zoom is local to the console and never alters the instrument.
// Play density is automatic; its fixed palette lives in style.css.
// Tauri uses webview zoom so hit testing stays in CSS pixels. Browsers use
// their own zoom controls. Existing stored accent/density choices are ignored.
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
/// hears the choice, and the chip for `current` starts lit. Returns a
/// setter that lights the chip for a value chosen elsewhere.
export function segmented(segment, values, current, render, onPick) {
  for (const value of values) {
    const chip = document.createElement("button");
    chip.textContent = render(value);
    chip.dataset.value = String(value);
    chip.classList.toggle("on", value === current);
    chip.setAttribute("aria-pressed", String(value === current));
    chip.addEventListener("click", () => {
      onPick(value);
      for (const other of segment.children) {
        other.classList.toggle("on", other === chip);
        other.setAttribute("aria-pressed", String(other === chip));
      }
    });
    segment.append(chip);
  }
  return (value) => {
    for (const chip of segment.children) {
      chip.classList.toggle("on", chip.dataset.value === String(value));
      chip.setAttribute("aria-pressed", String(chip.dataset.value === String(value)));
    }
  };
}

/// Builds the picker rows in the Preferences dialog and restores the
/// saved choices. Call once at startup.
export function wireTheme(root) {
  const scales = root.getElementById("scale-segment");
  const scaleNote = root.getElementById("scale-note");
  scale = readScale();
  applyScale(scale);
  onScaleChange = segmented(scales, SCALES, scale, (value) => `${Math.round(value * 100)}%`, chooseScale);
  if (hostZooms()) {
    scaleNote.textContent = "Ctrl + and Ctrl − step through these; Ctrl 0 returns to 100%.";
  } else {
    scales.setAttribute("aria-disabled", "true");
    for (const chip of scales.children) chip.disabled = true;
    scaleNote.textContent = "Use your browser’s zoom to change the console size (Ctrl +/−, or pinch to zoom).";
  }
}
