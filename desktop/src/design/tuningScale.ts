import type { Tuning } from './model';

export const ratioToCents = (ratio: number) => 1200 * Math.log2(ratio);
export const scalePeriod = (t: Tuning) => t.system === 'Twelve-note' ? 1200 : t.system === 'Equal division' ? t.period ?? 1200 : t.period;
export const scaleIntervals = (t: Tuning): number[] => t.system === 'Equal division'
  ? Array.from({ length: t.steps }, (_, i) => i * scalePeriod(t)! / t.steps)
  : t.intervals;

/** Study mapping: consecutive key IDs, with the reference key anchored to step 1.
 * A finite collection deliberately has no extrapolation or modulo wrapping. */
export function mappedPitch(t: Tuning, key: number): number | undefined {
  const offset = key - t.referenceKey;
  let cents: number;
  if (t.system === 'Twelve-note') {
    const pitchClass = ((key % 12) + 12) % 12;
    const anchorClass = ((t.referenceKey % 12) + 12) % 12;
    cents = offset * 100 + t.deviations[pitchClass] - t.deviations[anchorClass];
  } else {
    const intervals = scaleIntervals(t);
    const period = scalePeriod(t);
    if (period === null && (offset < 0 || offset >= intervals.length)) return undefined;
    const index = ((offset % intervals.length) + intervals.length) % intervals.length;
    cents = intervals[index] + (period === null ? 0 : Math.floor(offset / intervals.length) * period);
  }
  return t.hz * 2 ** ((cents + t.fine) / 1200);
}
