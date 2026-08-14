// Client-side appearance settings — accent colour and layout density.
// Purely cosmetic and local to this console: stored in localStorage,
// never sent to the server, applied before the first snapshot arrives.

// Each accent carries its own dark ink (text over the accent) and a
// deeper shade (held sharps), picked by eye rather than derived, so
// every choice keeps contrast. The 1f/33 alpha suffixes match the
// soft/glow tokens in style.css.
const ACCENTS = {
  cyan: { accent: "#45c4ff", deep: "#2b9fd8", ink: "#08202e" },
  mint: { accent: "#3ddc97", deep: "#25b77a", ink: "#06231a" },
  amber: { accent: "#ffb454", deep: "#df9430", ink: "#2b1c05" },
  violet: { accent: "#a08eff", deep: "#7f6ae6", ink: "#191140" },
  rose: { accent: "#ff6f9c", deep: "#e0517e", ink: "#330a1c" },
};

const DENSITIES = ["compact", "regular", "spacious"];

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
  const { accent, deep, ink } = ACCENTS[name] ?? ACCENTS.cyan;
  const root = document.documentElement.style;
  root.setProperty("--accent", accent);
  root.setProperty("--accent-soft", `${accent}1f`);
  root.setProperty("--accent-glow", `${accent}33`);
  root.setProperty("--accent-deep", deep);
  root.setProperty("--accent-ink", ink);
}

function applyDensity(name) {
  document.body.dataset.density = DENSITIES.includes(name) ? name : "regular";
}

/// Builds the two picker rows in the settings drawer and restores the
/// saved choices. Call once at startup.
export function wireTheme(root) {
  const swatches = root.getElementById("accent-swatches");
  const segment = root.getElementById("density-segment");

  let accent = store.get("accent", "cyan");
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

  let density = store.get("density", "regular");
  applyDensity(density);
  for (const name of DENSITIES) {
    const chip = document.createElement("button");
    chip.textContent = name.toUpperCase();
    chip.classList.toggle("on", name === density);
    chip.addEventListener("click", () => {
      density = name;
      store.set("density", name);
      applyDensity(name);
      for (const other of segment.children) {
        other.classList.toggle("on", other === chip);
      }
    });
    segment.append(chip);
  }
}
