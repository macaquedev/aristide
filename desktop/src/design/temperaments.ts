const syntonicComma = 1200 * Math.log2(81 / 80);
const pureFifth = 1200 * Math.log2(3 / 2);

/** Regular temperament with fifths from E♭ to G♯, each narrowed by `tempering` cents. */
function chainOfFifths(tempering: number): number[] {
  const deviations = Array<number>(12).fill(0);
  for (let fifths = -3; fifths <= 8; fifths++) {
    const pitchClass = ((fifths * 7) % 12 + 12) % 12;
    const cents = ((fifths * (pureFifth - tempering)) % 1200 + 1200) % 1200 - pitchClass * 100;
    deviations[pitchClass] = Number((cents > 600 ? cents - 1200 : cents < -600 ? cents + 1200 : cents).toFixed(1));
  }
  return deviations;
}

/** Deviations from equal temperament in cents, C to B, for a temperament rooted on C. */
export const temperaments: Record<string, number[]> = {
  Equal: Array(12).fill(0),
  'Quarter-comma meantone': chainOfFifths(syntonicComma / 4),
  'Sixth-comma meantone': chainOfFifths(syntonicComma / 6),
  Pythagorean: chainOfFifths(0),
  'Werckmeister III': [0, -9.8, -7.8, -5.9, -9.8, -2, -11.7, -3.9, -7.8, -11.7, -3.9, -7.8],
  'Kirnberger III': [0, -9.8, -6.8, -5.9, -13.7, -2, -9.8, -3.4, -7.8, -10.3, -3.9, -11.7],
  Vallotti: [0, -5.9, -3.9, -2, -7.8, 2, -7.8, -2, -3.9, -5.9, 0, -9.8],
  'Young II': [0, -9.8, -3.9, -5.9, -7.8, -2, -11.7, -2, -7.8, -5.9, -3.9, -9.8],
};

export const temperamentNames = [...Object.keys(temperaments), 'Custom'];

export const rootedDeviations = (temperament: string, root: number) =>
  Array.from({ length: 12 }, (_, pitchClass) => temperaments[temperament][((pitchClass - root) % 12 + 12) % 12]);
