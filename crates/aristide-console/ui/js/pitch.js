// Pitch wording shared across panes: notes and transpositions are
// spoken the way an organist reads a stoplist — "C2–C7", "an octave
// lower" — never as MIDI note numbers or signed semitone counts.

const NOTE_NAMES = ["C", "C♯", "D", "E♭", "E", "F", "F♯", "G", "A♭", "A", "B♭", "B"];
const LETTER_SEMITONES = { c: 0, d: 2, e: 4, f: 5, g: 7, a: 9, b: 11 };

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
