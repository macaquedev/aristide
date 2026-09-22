import catalogue from './temperaments.json' with { type: 'json' };

export const pitchNames = ['C', 'C♯', 'D', 'D♯', 'E', 'F', 'F♯', 'G', 'G♯', 'A', 'A♯', 'B'];
export const noteName = (key: number) => `${pitchNames[((key % 12) + 12) % 12]}${Math.floor(key / 12) - 1}`;
export const ratioToCents = (ratio: number) => 1200 * Math.log2(ratio);
export const formatCents = (value: number) => `${value > 0 ? '+' : ''}${Number(value.toFixed(2))}`;

/** Which key sounds what: the pitch every other key is measured from. */
export type Anchor = { key: number; hz: number; offset: number };

/** The intervals and where they sit on the keys. `offsets` are the resolved
 * deviations from equal, C to B, for every twelve-class temperament. */
export type Shape =
  | { system: 'temperament'; temperament: string; root: number; offsets: number[] }
  | { system: 'equal'; steps: number; period: number }
  | { system: 'steps'; intervals: number[]; period: number | null; startKey: number }
  | { system: 'scale'; name: string; scl: string; kbm: string | null; intervals: number[]; period: number; startKey: number };

export const temperaments: { id: string; name: string; offsets: number[] }[] = catalogue;
export const RECORDED = 'original';
export const CUSTOM = 'custom';

export const temperamentName = (id: string) =>
  id === RECORDED ? 'As recorded' : id === CUSTOM ? 'Custom' : temperaments.find(t => t.id === id)?.name ?? id;

export const presetOffsets = (id: string, root: number) => {
  const table = temperaments.find(t => t.id === id)!.offsets;
  return Array.from({ length: 12 }, (_, pitchClass) => table[((pitchClass - root) % 12 + 12) % 12]);
};

export const defaultAnchor: Anchor = { key: 69, hz: 440, offset: 0 };
export const equalTemperament: Shape = { system: 'temperament', temperament: 'equal', root: 0, offsets: Array(12).fill(0) };

export const stepCount = (shape: Shape) =>
  shape.system === 'temperament' ? 12 : shape.system === 'equal' ? shape.steps : shape.intervals.length;

export const repeatInterval = (shape: Shape): number | null =>
  shape.system === 'temperament' ? 1200 : shape.period;

/** Step intervals in cents from the first step. */
export const stepIntervals = (shape: Shape): number[] =>
  shape.system === 'temperament' ? Array.from({ length: 12 }, (_, i) => i * 100 + shape.offsets[(shape.root + i) % 12] - shape.offsets[shape.root])
    : shape.system === 'equal' ? Array.from({ length: shape.steps }, (_, i) => i * shape.period / shape.steps)
    : shape.intervals;

/** Cents of `key` on the shape's own ladder, or undefined where nothing sounds. */
function ladderCents(shape: Shape, key: number): number | undefined {
  if (shape.system === 'temperament') return key * 100 + shape.offsets[((key % 12) + 12) % 12];
  if (shape.system === 'equal') return key * shape.period / shape.steps;
  const offset = key - shape.startKey;
  const count = shape.intervals.length;
  if (shape.period === null && (offset < 0 || offset >= count)) return undefined;
  const index = ((offset % count) + count) % count;
  return shape.intervals[index] + Math.floor(offset / count) * (shape.period ?? 0);
}

export function keyHz(shape: Shape, anchor: Anchor, key: number): number | undefined {
  const cents = ladderCents(shape, key);
  const reference = ladderCents(shape, anchor.key);
  if (cents === undefined || reference === undefined) return undefined;
  return anchor.hz * 2 ** ((cents - reference + anchor.offset) / 1200);
}

/** Whether a finite collection leaves the reference key without a pitch. */
export const referenceUnmapped = (shape: Shape, anchor: Anchor) => ladderCents(shape, anchor.key) === undefined;

/** Deviations shown relative to the reference note, which stays at zero. */
export const relativeOffsets = (offsets: number[], anchor: Anchor) =>
  offsets.map(cents => Number((cents - offsets[((anchor.key % 12) + 12) % 12]).toFixed(3)));

const lineOfFifths = ['F♭', 'C♭', 'G♭', 'D♭', 'A♭', 'E♭', 'B♭', 'F', 'C', 'G', 'D', 'A', 'E', 'B', 'F♯', 'C♯', 'G♯', 'D♯', 'A♯', 'E♯', 'B♯', 'F𝄪', 'C𝄪', 'G𝄪'];
const rootPosition = [0, 7, 2, -3, 4, -1, 6, 1, -4, 3, -2, 5];

/** The fifth furthest from pure (the wolf when it is far enough), and the range of fifth sizes, spelled as the temperament's chain spells it. */
export function widestFifth(shape: Extract<Shape, { system: 'temperament' }>) {
  const root = shape.temperament === CUSTOM || shape.temperament === RECORDED ? 0 : shape.root;
  const pure = ratioToCents(3 / 2);
  const fifths = Array.from({ length: 12 }, (_, chain) => {
    const from = (root + (chain - 3) * 7 + 144) % 12;
    const to = (from + 7) % 12;
    return { chain, cents: 700 + shape.offsets[to] - shape.offsets[from] };
  });
  const widest = fifths.reduce((a, b) => Math.abs(b.cents - pure) > Math.abs(a.cents - pure) ? b : a);
  const spell = (chain: number) => lineOfFifths[rootPosition[root] + ((chain % 12) + 12) % 12 - 3 + 8];
  const sizes = fifths.map(f => f.cents);
  return { from: spell(widest.chain), to: spell(widest.chain + 1), cents: widest.cents, wolf: Math.abs(widest.cents - pure) > 15, smallest: Math.min(...sizes), largest: Math.max(...sizes) };
}
