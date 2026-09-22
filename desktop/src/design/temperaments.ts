const syntonicComma = 1200 * Math.log2(81 / 80);
const pureFifth = 1200 * Math.log2(3 / 2);

/** Walks eleven fifths of the given sizes up from `start`, returning deviations from equal temperament with C at 0. */
function circleOfFifths(start: number, fifths: number[]): number[] {
  const deviations = Array<number>(12).fill(0);
  let cents = 0;
  fifths.forEach((size, i) => { cents += size - 700; deviations[(start + (i + 1) * 7) % 12] = cents; });
  const c = deviations[0];
  return deviations.map(d => Number((d - c).toFixed(1)));
}

const meantoneFifth = pureFifth - syntonicComma / 4;
const regular = (fifth: number) => circleOfFifths(3, Array(11).fill(fifth));
/** Rameau 1726 as reconstructed by Claudi Meneghin: seven meantone fifths B♭–B, pure B–G♯, the wolf split over G♯–E♭–B♭. */
const rameauFifths = [...Array(7).fill(meantoneFifth), ...Array(3).fill(pureFifth)];
const rameau = circleOfFifths(10, [...rameauFifths, (8400 - rameauFifths.reduce((sum, f) => sum + f, 0)) / 2]);

/** Deviations from equal temperament in cents, C to B, for a temperament rooted on C. */
export const temperaments: Record<string, number[]> = {
  Equal: Array(12).fill(0),
  'Quarter-comma meantone': regular(meantoneFifth),
  'Sixth-comma meantone': regular(pureFifth - syntonicComma / 6),
  Pythagorean: regular(pureFifth),
  'Werckmeister III': [0, -9.8, -7.8, -5.9, -9.8, -2, -11.7, -3.9, -7.8, -11.7, -3.9, -7.8],
  'Kirnberger III': [0, -9.8, -6.8, -5.9, -13.7, -2, -9.8, -3.4, -7.8, -10.3, -3.9, -11.7],
  'Rameau 1726': rameau,
  Vallotti: [0, -5.9, -3.9, -2, -7.8, 2, -7.8, -2, -3.9, -5.9, 0, -9.8],
  'Young II': [0, -9.8, -3.9, -5.9, -7.8, -2, -11.7, -2, -7.8, -5.9, -3.9, -9.8],
};

export const temperamentNames = [...Object.keys(temperaments), 'Custom'];

export const rootedDeviations = (temperament: string, root: number) =>
  Array.from({ length: 12 }, (_, pitchClass) => temperaments[temperament][((pitchClass - root) % 12 + 12) % 12]);
