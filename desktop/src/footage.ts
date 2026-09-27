const glyphs: Record<string, string> = {
  '1/2': '½', '1/3': '⅓', '2/3': '⅔', '1/4': '¼', '3/4': '¾', '1/5': '⅕', '2/5': '⅖', '3/5': '⅗', '4/5': '⅘',
  '1/6': '⅙', '5/6': '⅚', '1/7': '⅐', '1/8': '⅛', '3/8': '⅜', '5/8': '⅝', '7/8': '⅞', '1/9': '⅑', '1/10': '⅒',
};

const nearestFraction = (value: number) => {
  let best = { numerator: 0, denominator: 1, error: Infinity };
  for (const tolerance of [.001, .01]) for (let denominator = 1; denominator <= 16; denominator++) {
    const numerator = Math.round(value * denominator);
    const error = Math.abs(value - numerator / denominator);
    if (error < tolerance) return { numerator, denominator };
    if (error < best.error) best = { numerator, denominator, error };
  }
  return best;
};

export const formatFootage = (feet: number) => {
  const { numerator, denominator } = nearestFraction(feet);
  const whole = Math.floor(numerator / denominator);
  const remainder = numerator % denominator;
  if (!remainder) return `${whole}′`;
  const glyph = glyphs[`${remainder}/${denominator}`];
  const fraction = glyph ?? `${whole ? '\u2009' : ''}${remainder}⁄${denominator}`;
  return `${whole || ''}${fraction}′`;
};
