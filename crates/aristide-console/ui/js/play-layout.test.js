import { test, expect } from 'bun:test';
import { pagePlan } from './play-layout.js';

test('pagination visits every stop exactly once at both instrument extremes', () => {
  for (const [width, height] of [[1452, 600], [1032, 1500], [304, 340]]) {
    for (const counts of [[5], [30, 30, 30, 30, 30], [2, 80, 1, 3, 31]]) {
      const plan = pagePlan(width, height, counts);
      expect(plan.stopHeight).toBeGreaterThanOrEqual(64);
      expect(plan.columnWidth / plan.stopColumns).toBeGreaterThanOrEqual(96);
      for (let division = 0; division < counts.length; division++) {
        const seen = [];
        for (const page of plan.pages) if (division >= page.start && division < page.end) {
          for (let i = page.offset; i < Math.min(page.offset + plan.capacity, counts[division]); i++) seen.push(i);
        }
        expect(seen).toEqual(Array.from({ length: counts[division] }, (_, i) => i));
      }
    }
  }
});

test('empty instruments have no stop pages', () => {
  expect(pagePlan(1000, 600, []).pages).toEqual([]);
});
