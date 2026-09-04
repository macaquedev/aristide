// The one place a keyboard panel's size becomes a --kb-scale factor,
// shared by the editor's grip drag and console.js's replay of a stored
// size. Keys and cheek scale together (see style.css, where both
// multiply --kb-scale — a label that stayed 10px beside three-times
// keys, or half-size ones, would read as a mistake); the padding and
// gap between them don't, so the factor is solved against the scaling
// parts alone.

const MIN = 0.35; // shrunk past legibility helps nobody
const MAX = 3; // nor blown past the canvas

/// What a keyboard panel is made of, in px at scale 1: `scaling` is
/// the part --kb-scale multiplies, `fixed` the chrome around it. Null
/// before the panel has anything to measure.
export function measureKeyboard(panel) {
  const keys = panel.querySelector(".keys");
  if (!keys) return null;
  const current = parseFloat(panel.style.getPropertyValue("--kb-scale")) || 1;
  const cheek = panel.querySelector(".cheek")?.offsetWidth ?? 0;
  const scaling = (keys.offsetWidth + cheek) / current;
  if (!(scaling > 0)) return null;
  return { scaling, fixed: panel.offsetWidth - keys.offsetWidth - cheek };
}

/// The clamped factor that makes a panel so measured `targetPx` wide.
export function keyboardScale({ scaling, fixed }, targetPx) {
  return Math.max(MIN, Math.min(MAX, (targetPx - fixed) / scaling)).toFixed(4);
}

// Keep this query in step with the responsive canvas rules in style.css.
export const usesFlowLayout = () =>
  window.matchMedia("(max-width: 1100px), (pointer: coarse) and (max-width: 1600px)").matches;
