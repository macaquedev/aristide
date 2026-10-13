import { test, expect } from '@playwright/test';

test('break chart: dragging a key limit rewrites the sentence and the readout', async ({ page }) => {
  const requests: string[] = [];
  page.on('request', request => { if (request.url().includes('/api/')) requests.push(request.url()); });
  await page.goto('/?study=1&panel=limits&view=breaks&overlap=stack');
  const sentence = page.locator('.limits-inspector');
  await expect(sentence.getByRole('button', { name: 'Key to: −950 ¢', exact: true })).toBeVisible();
  const handle = page.getByRole('slider', { name: 'Key to', exact: true });
  const box = (await handle.locator('line').boundingBox())!;
  await page.mouse.move(box.x, box.y + 40);
  await page.mouse.down();
  await page.mouse.move(box.x + 300, box.y + 40, { steps: 6 });
  await page.mouse.up();
  await expect(sentence.getByRole('button', { name: /^Key to: \+/ })).toBeVisible();
  await expect(page.locator('.limit-probe')).toContainText('Fourniture I +1900 ¢');
  await page.getByRole('button', { name: 'Undo', exact: true }).click();
  await expect(sentence.getByRole('button', { name: 'Key to: −950 ¢', exact: true })).toBeVisible();
  expect(requests).toEqual([]);
});

test('a sentence chip sets and clears a limit, and 19 equal renames keys', async ({ page }) => {
  await page.goto('/?study=1&panel=limits&view=sentence&overlap=stack');
  const inspector = page.locator('.limits-inspector');
  await inspector.getByRole('button', { name: 'Sounding to: the highest pitch', exact: true }).click();
  await page.getByRole('textbox', { name: 'Cents' }).fill('2400');
  await page.getByRole('button', { name: 'Done', exact: true }).click();
  await expect(inspector.getByRole('button', { name: 'Sounding to: +2400 ¢', exact: true })).toBeVisible();
  await inspector.getByRole('button', { name: 'Sounding to: +2400 ¢', exact: true }).click();
  await page.getByRole('button', { name: 'No limit', exact: true }).click();
  await expect(inspector.getByRole('button', { name: 'Sounding to: the highest pitch', exact: true })).toBeVisible();
  await page.locator('label', { hasText: '19 equal' }).click();
  await expect(inspector).toContainText('relative to key 34 = 415 Hz');
});

test('overlapping unisons stack or merge on the roll', async ({ page }) => {
  await page.goto('/?study=1&panel=limits&view=strips&overlap=stack');
  for (const name of [/^Bourdon 8′, 0 ¢/, /^Flûte 8′ · Positif, 0 ¢/, /^Bourdon 8′ · Pédale, 0 ¢/]) await expect(page.getByRole('button', { name })).toHaveCount(1);
  await page.locator('label', { hasText: 'Merge' }).click();
  const merged = page.getByRole('button', { name: /^Bourdon 8′, 0 ¢/ });
  await expect(merged).toContainText('×3');
  await merged.click();
  await expect(page.locator('.limits-inspector')).toContainText('Bourdon 8′');
  await page.getByRole('button', { name: /, 0 ¢/ }).click();
  await expect(page.locator('.limits-inspector')).toContainText('Flûte 8′');
});

for (const [width, height] of [[1280, 800], [390, 844]]) test(`limits study fits ${width} px`, async ({ page }) => {
  await page.setViewportSize({ width, height });
  for (const view of ['chart', 'breaks', 'strips', 'sentence']) {
    await page.goto(`/?study=1&panel=limits&view=${view}&overlap=stack`);
    await expect(page.locator('.limit-probe')).toBeVisible();
    expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(width);
    await page.screenshot({ path: `test-results/limits-${view}-${width}.png`, fullPage: true });
  }
});
