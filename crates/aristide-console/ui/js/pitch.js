// Pitch wording shared across panes: notes and transpositions are
// spoken the way an organist reads a stoplist — "C2–C7", "an octave
// lower" — never as MIDI note numbers or signed semitone counts.

const NOTE_NAMES = ["C", "C♯", "D", "E♭", "E", "F", "F♯", "G", "A♭", "A", "B♭", "B"];
const LETTER_SEMITONES = { c: 0, d: 2, e: 4, f: 5, g: 7, a: 9, b: 11 };

/// Footages the organ world writes as fractions, feet paired with the
/// label — in the order the stop popover's own datalist offers them.
/// `formatFootage` uses the same table (extended a fourth-foot lower)
/// to snap a computed value back to its familiar name.
export const STANDARD_FOOTAGES = [
  [32, "32"],
  [16, "16"],
  [32 / 3, "10 2/3"],
  [8, "8"],
  [32 / 5, "6 2/5"],
  [16 / 3, "5 1/3"],
  [4, "4"],
  [16 / 5, "3 1/5"],
  [8 / 3, "2 2/3"],
  [2, "2"],
  [8 / 5, "1 3/5"],
  [4 / 3, "1 1/3"],
  [1, "1"],
  [4 / 5, "4/5"],
  [2 / 3, "2/3"],
  [1 / 2, "1/2"],
];

/// A footage in feet, written the way the organ world writes it. A rank
/// is never built to land exactly on 5.333' — it's "5 1/3" tuned a hair
/// sharp or flat — so a value within 60 cents (a quarter-semitone) of a
/// standard footage snaps to that name; anything further off isn't
/// really that footage any more; it's shown as plain feet instead.
export function formatFootage(feet) {
  if (feet == null) return "";
  for (const [candidate, label] of STANDARD_FOOTAGES) {
    if (Math.abs(1200 * Math.log2(feet / candidate)) < 60) return label;
  }
  return feet.toFixed(2);
}

/// "16" → 16, "2 2/3" → 8/3, "5-1/3" → 16/3, "2.667" → 2.667 — the
/// server's sidecar footage grammar, mirrored so the console can
/// reason about footage text it is about to send without a round
/// trip. A trailing foot mark (' or ′) is tolerated, as on a knob.
/// Returns feet as a number, or null when the text names no footage.
export function parseFootage(text) {
  let s = String(text).trim().replace(/['′]+$/, "").trim();
  let whole = 0;
  const mixed = /^(\d+)[ -](\S*\/\S*)$/.exec(s);
  if (mixed) {
    whole = Number(mixed[1]);
    s = mixed[2];
  }
  let value;
  const frac = s.split("/");
  if (frac.length === 2) {
    const num = Number(frac[0].trim());
    const den = Number(frac[1].trim());
    if (frac[0].trim() === "" || !Number.isFinite(num) || !(den > 0)) return null;
    value = num / den;
  } else {
    value = Number(s);
    if (s === "" || !Number.isFinite(value)) return null;
  }
  const feet = whole + value;
  return Number.isFinite(feet) && feet > 0 ? feet : null;
}

/// "Montre 8'" → { base: "Montre", tail: "8'", feet: 8 }. Sample sets
/// routinely engrave the footage into the stop's name itself; this is
/// how the console notices, so a footage edit can offer to move that
/// tail out of the name and let the knob engrave the real pitch. The
/// tail is the last word — or last two, for a mixed fraction like
/// "2 2/3" — when it reads as a footage. Roman-numeral tails (mixture
/// rank counts) don't parse and stay put; so does a name that is
/// nothing but a footage, since stripping it would leave no name.
export function splitFootageName(name) {
  if (parseFootage(name) != null) return null; // the whole name is a footage
  const tokens = String(name).trim().split(/\s+/);
  for (const take of [2, 1]) {
    if (tokens.length <= take) continue;
    const tail = tokens.slice(-take).join(" ");
    if (!/^\d/.test(tail)) continue;
    const feet = parseFootage(tail);
    if (feet != null) return { base: tokens.slice(0, -take).join(" "), tail, feet };
  }
  return null;
}

/// MIDI note number in scientific pitch notation, the naming every
/// sample set's documentation uses: middle C (60) is C4.
export function keyName(key) {
  return `${NOTE_NAMES[key % 12]}${Math.floor(key / 12) - 1}`;
}

/// The reverse: "C4", "f♯3", "Bb2" (either accidental glyph, any case)
/// back to a MIDI note, or null when the text doesn't name one. A bare
/// number is still accepted — pasted MIDI values shouldn't be an error.
export function parseKeyName(text) {
  const s = String(text).replace(/\s+/g, "");
  if (/^\d+$/.test(s)) {
    const n = Number(s);
    return n <= 127 ? n : null;
  }
  const m = /^([a-g])([#♯b♭]?)(-?\d+)$/i.exec(s);
  if (!m) return null;
  let semitone = LETTER_SEMITONES[m[1].toLowerCase()];
  if (m[2] === "#" || m[2] === "♯") semitone += 1;
  else if (m[2]) semitone -= 1;
  const key = (Number(m[3]) + 1) * 12 + semitone;
  return key >= 0 && key <= 127 ? key : null;
}

/// A trigger's note spelled as its pitch: "note:36" reads as "note C2";
/// every other trigger (cc:, program:, key:) is already legible as-is.
export function noteTriggerText(trigger) {
  const m = /^note:(\d+)$/.exec(trigger);
  return m ? `note ${keyName(Number(m[1]))}` : trigger;
}

/// "" for no shift; otherwise "an octave lower", "2 octaves higher",
/// "5 semitones lower" — octaves when it is octaves, semitones when not.
export function shiftWords(shift) {
  if (!shift) return "";
  const direction = shift > 0 ? "higher" : "lower";
  const size = Math.abs(shift);
  if (size % 12 === 0) {
    const octaves = size / 12;
    return octaves === 1 ? `an octave ${direction}` : `${octaves} octaves ${direction}`;
  }
  return `${size} semitone${size === 1 ? "" : "s"} ${direction}`;
}
