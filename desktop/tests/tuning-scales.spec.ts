import { test, expect } from '@playwright/test';
import { defaultAnchor, keyHz, presetOffsets, ratioToCents, relativeOffsets, temperaments, widestFifth, type Shape } from '../src/tuning/model';

test('repeat count and interval are independent, in both directions', () => {
  const nineteen: Shape = { system: 'equal', steps: 19, period: 1200 };
  expect(keyHz(nineteen, defaultAnchor, 88)).toBeCloseTo(880, 8);
  const triple: Shape = { system: 'equal', steps: 13, period: ratioToCents(3) };
  expect(keyHz(triple, defaultAnchor, 82)).toBeCloseTo(1320, 8);
  expect(keyHz(triple, defaultAnchor, 56)).toBeCloseTo(440 / 3, 8);
  expect(keyHz(triple, defaultAnchor, 70)).toBeCloseTo(440 * 3 ** (1 / 13), 8);
});

test('finite collections never wrap or extrapolate', () => {
  const finite: Shape = { system: 'steps', intervals: [0, -137, 386, 2507], period: null, startKey: 40 };
  const anchor = { ...defaultAnchor, key: 40 };
  expect(keyHz(finite, anchor, 39)).toBeUndefined();
  expect(keyHz(finite, anchor, 44)).toBeUndefined();
  expect(keyHz(finite, anchor, 41)).toBeCloseTo(440 * 2 ** (-137 / 1200), 8);
  expect(keyHz(finite, anchor, 43)).toBeCloseTo(440 * 2 ** (2507 / 1200), 8);
  expect(keyHz({ ...finite, period: 2300 }, anchor, 44)).toBeCloseTo(440 * 2 ** (2300 / 1200), 8);
});

test('the reference key keeps its pitch whatever the root', () => {
  for (const root of [0, 2, 8]) {
    const shape: Shape = { system: 'temperament', temperament: 'meantone4', root, offsets: presetOffsets('meantone4', root) };
    expect(keyHz(shape, defaultAnchor, 69)).toBeCloseTo(440, 8);
    expect(relativeOffsets(shape.offsets, defaultAnchor)[9]).toBe(0);
  }
  const onC: Shape = { system: 'temperament', temperament: 'meantone4', root: 0, offsets: presetOffsets('meantone4', 0) };
  expect(keyHz(onC, { ...defaultAnchor, offset: 10 }, 69)).toBeCloseTo(440 * 2 ** (10 / 1200), 8);
});

test('the wolf is spelled along the temperament chain', () => {
  const at = (id: string, root: number) => widestFifth({ system: 'temperament', temperament: id, root, offsets: presetOffsets(id, root) });
  expect(at('meantone4', 0)).toMatchObject({ from: 'G♯', to: 'E♭', wolf: true });
  expect(at('meantone4', 2)).toMatchObject({ from: 'A♯', to: 'F', wolf: true });
  expect(at('equal', 0)).toMatchObject({ wolf: false, smallest: 700, largest: 700 });
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
  await page.getByRole('switch', { name: 'Scale follows Whole instrument' }).uncheck();
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

test('historical temperaments follow the root note', async ({ page }) => {
  await page.goto('/?study=1&panel=tuning&layout=channel');
  const choose = async (label: string, value: string) => {
    await page.getByRole('combobox', { name: label, exact: true }).click();
    await page.getByRole('option', { name: value, exact: true }).click();
  };
  await choose('Temperament', 'Quarter-comma meantone');
  await expect(page.getByText('Wolf G♯–E♭ · 737.6 ¢', { exact: true })).toBeVisible();
  for (let i = 0; i < 3; i++) await page.getByRole('button', { name: 'Next note' }).click();
  await expect(page.getByRole('button', { name: 'D♯ deviation: 20.5 ¢', exact: true })).toBeDisabled();
  await choose('Root note', 'D');
  await expect(page.getByText('Wolf A♯–F · 737.6 ¢', { exact: true })).toBeVisible();
  await page.getByRole('button', { name: 'Next note' }).click();
  await expect(page.getByRole('button', { name: 'D♯ deviation: -20.5 ¢', exact: true })).toBeVisible();
  await expect(page.getByRole('button', { name: 'Reference pitch: 440 Hz', exact: true })).toBeVisible();
  await choose('Temperament', 'Custom');
  await expect(page.getByRole('button', { name: 'D♯ deviation: -20.5 ¢', exact: true })).toBeEnabled();
  await page.screenshot({ path: 'test-results/tuning-meantone.png', fullPage: true });
});

test('Rameau keeps seven meantone fifths and splits the wolf', () => {
  const pitch = temperaments.find(t => t.id === 'rameau1726')!.offsets.map((cents, i) => i * 100 + cents);
  const fifth = (from: number) => ((pitch[(from + 7) % 12] - pitch[from]) % 1200 + 1200) % 1200;
  for (const from of [10, 5, 0, 7, 2, 9, 4]) expect(fifth(from)).toBeCloseTo(696.6, 0);
  for (const from of [11, 6, 1]) expect(fifth(from)).toBeCloseTo(702, 0);
  expect(fifth(8)).toBeCloseTo(fifth(3), 0);
  expect(fifth(8)).toBeCloseTo(709, 0);
});
