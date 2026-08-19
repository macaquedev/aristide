// Client-side appearance settings — accent colour and layout density.
// Purely cosmetic and local to this console: stored in localStorage,
// never sent to the server, applied before the first snapshot arrives.

// Each accent carries its own dark ink (text over the accent) and a
// deeper shade (held sharps), picked by eye rather than derived, so
// every choice keeps contrast. The 1f/33 alpha suffixes match the
// soft/glow tokens in style.css. The pickers live in Preferences →
// Appearance.
const ACCENTS = {
  blue: {
    accent: "#a8c7fa", deep: "#7cacf8", ink: "#062e6f",
    container: "#004a77", onContainer: "#c2e7ff",
  },
  green: {
    accent: "#6dd58c", deep: "#5bb974", ink: "#072711",
    container: "#0f5223", onContainer: "#c4eed0",
  },
  yellow: {
    accent: "#fdd663", deep: "#f9ab00", ink: "#402d00",
    container: "#574500", onContainer: "#ffdf99",
  },
  purple: {
    accent: "#d0bcff", deep: "#ab94f0", ink: "#381e72",
    container: "#4f378b", onContainer: "#eaddff",
  },
  pink: {
    accent: "#ffb1c8", deep: "#ee7da0", ink: "#5e1133",
    container: "#703348", onContainer: "#ffd9e2",
  },
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
  const { accent, deep, ink, container, onContainer } =
    ACCENTS[name] ?? ACCENTS.blue;
  const root = document.documentElement.style;
  root.setProperty("--accent", accent);
  root.setProperty("--accent-soft", `${accent}1f`);
  root.setProperty("--accent-deep", deep);
  root.setProperty("--accent-ink", ink);
  root.setProperty("--accent-container", container);
  root.setProperty("--on-accent-container", onContainer);
}

function applyDensity(name) {
  document.body.dataset.density = DENSITIES.includes(name) ? name : "regular";
}

/// Builds the two picker rows in Preferences → Appearance and restores
/// the saved choices. Call once at startup.
export function wireTheme(root) {
  const swatches = root.getElementById("accent-swatches");
  const segment = root.getElementById("density-segment");

  let accent = store.get("accent", "blue");
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
