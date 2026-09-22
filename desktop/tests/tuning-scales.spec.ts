import { test, expect } from '@playwright/test';
import { defaultTuning } from '../src/design/model';
import { mappedPitch, ratioToCents, scaleIntervals } from '../src/design/tuningScale';

test('repeat count and interval are independent, in both directions', () => {
  const nineteen = { ...defaultTuning, system: 'Equal division', steps: 19 };
  expect(scaleIntervals(nineteen)).toHaveLength(19);
  expect(mappedPitch(nineteen, 88)).toBeCloseTo(880, 8);
  const triple = { ...nineteen, steps: 13, period: ratioToCents(3) };
  expect(mappedPitch(triple, 82)).toBeCloseTo(1320, 8);
  expect(mappedPitch(triple, 56)).toBeCloseTo(440 / 3, 8);
  expect(mappedPitch(triple, 70)).toBeCloseTo(440 * 3 ** (1 / 13), 8);
});

test('finite collections never wrap or extrapolate', () => {
  const finite = { ...defaultTuning, system: 'Pitch collection', period: null, referenceKey: 40, intervals: [0, -137, 386, 2507] };
  expect(mappedPitch(finite, 39)).toBeUndefined();
  expect(mappedPitch(finite, 44)).toBeUndefined();
  expect(mappedPitch(finite, 41)).toBeCloseTo(440 * 2 ** (-137 / 1200), 8);
  expect(mappedPitch(finite, 43)).toBeCloseTo(440 * 2 ** (2507 / 1200), 8);
  expect(mappedPitch({ ...finite, period: 2300 }, 44)).toBeCloseTo(440 * 2 ** (2300 / 1200), 8);
});

test('channel strip handles non-octave periods and finite collections', async ({ page }) => {
  await page.goto('/?study=1&panel=tuning&layout=channel');
  const choose = async (label: string, value: string) => {
    await page.getByRole('combobox', { name: label, exact: true }).click();
    await page.getByRole('option', { name: value, exact: true }).click();
  };
  await choose('Tuning system', 'Equal division');
  await choose('Repeat interval', '3:1 · 1901.96 ¢');
  await page.getByRole('button', { name: /^Steps per repeat:/ }).click();
  await page.getByRole('textbox', { name: 'Steps per repeat', exact: true }).fill('13');
  await page.getByRole('button', { name: 'Done', exact: true }).click();
  await expect(page.locator('.tuning-note-track')).toHaveCount(13);
  await expect(page.getByText('13 equal steps · Repeat 1901.96 ¢', { exact: true })).toBeVisible();
  await expect(page.getByRole('combobox', { name: 'Root note' })).toHaveCount(0);
  await expect(page.getByRole('button', { name: 'Reference key: 69 (A4)', exact: true })).toBeVisible();
  await page.screenshot({ path: 'test-results/tuning-triple-period.png', fullPage: true });
  await choose('Tuning system', 'Pitch collection');
  await expect(page.getByRole('combobox', { name: 'Repeat interval' })).toHaveValue('None · no repetition');
  await expect(page.locator('.tuning-note-track')).toHaveCount(7);
  await page.getByRole('button', { name: 'Select 7', exact: true }).click();
  await page.getByRole('button', { name: 'Step 7 interval: 2107 ¢', exact: true }).click();
  await page.getByRole('textbox', { name: 'Step 7 interval', exact: true }).fill('2507');
  await page.getByRole('button', { name: 'Done', exact: true }).click();
  await expect(page.getByText('Keys 69–75 only · outside unmapped', { exact: true })).toBeVisible();
  await page.getByRole('button', { name: 'Add step', exact: true }).click();
  await expect(page.locator('.tuning-note-track')).toHaveCount(8);
  await page.getByRole('button', { name: 'Remove last', exact: true }).click();
  await expect(page.locator('.tuning-note-track')).toHaveCount(7);
  await page.screenshot({ path: 'test-results/tuning-no-repeat.png', fullPage: true });
  await page.getByRole('button', { name: 'Grand-orgue', exact: true }).click();
  await expect(page.getByRole('combobox', { name: 'Repeat interval' })).toBeDisabled();
  await page.getByRole('switch', { name: 'Follow Whole instrument' }).uncheck();
  await choose('Repeat interval', 'Custom interval');
  await page.getByRole('button', { name: /^Repeat size:/ }).click();
  await page.getByRole('textbox', { name: 'Repeat size', exact: true }).fill('2300');
  await page.getByRole('button', { name: 'Done', exact: true }).click();
  await expect(page.getByText('7 steps · Repeat 2300 ¢', { exact: true })).toBeVisible();
  await page.getByRole('button', { name: 'Whole instrument', exact: true }).click();
  await expect(page.getByRole('combobox', { name: 'Repeat interval' })).toHaveValue('None · no repetition');
  await page.setViewportSize({ width: 390, height: 844 });
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
});
