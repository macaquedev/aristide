// Pitch wording shared across panes: a transposition is spoken the way
// an organist thinks — "an octave lower" — never counted out in signed
// semitones.

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
