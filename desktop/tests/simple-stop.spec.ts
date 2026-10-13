import { test, expect, type Page } from '@playwright/test';

const tapKey = async (page: Page, keyboard: ReturnType<Page['locator']>, index: number) => {
  const box = (await keyboard.boundingBox())!;
  await page.mouse.click(box.x + (index + .5) * box.width / 54, box.y + box.height * .8);
};

test('a stop reads as sounds, and Try a key says what plays', async ({ page }) => {
  const requests: string[] = [];
  page.on('request', request => { if (request.url().includes('/api/')) requests.push(request.url()); });
  await page.goto('/?study=1&panel=simple');
  await expect(page.locator('.simple-sound')).toHaveCount(7);
  await expect(page.getByRole('button', { name: 'Pitch: an octave down · 4′' }).first()).toBeVisible();
  await expect(page.getByRole('button', { name: 'Keys: C4 – F6' }).first()).toBeVisible();
  const tryKeys = page.locator('.simple-try .simple-keyboard');
  await tapKey(page, tryKeys, 34); // A♯4: above the Pédale's last key
  await expect(page.locator('.simple-try-list')).toContainText('Couple the Pédale · no pipe here');
  await expect(page.locator('.simple-try-list')).toContainText('Doublette 2′ · not on this key');
  await page.screenshot({ path: 'test-results/simple-stop.png', fullPage: true });
  expect(requests).toEqual([]);
});

test('pitch, split and add are one choice each', async ({ page }) => {
  await page.goto('/?study=1&panel=simple');
  await page.getByRole('button', { name: 'Pitch: as it is · 8′' }).first().click();
  await page.getByRole('menuitem', { name: /a twelfth up/ }).click();
  await expect(page.getByRole('button', { name: 'Pitch: a twelfth up · 2⅔′' })).toBeVisible();

  await page.getByRole('button', { name: 'Keys: all keys' }).first().click();
  const sheet = page.getByRole('dialog', { name: /Which keys play Bourdon 8′/ });
  await expect(sheet).toBeVisible();
  await page.waitForTimeout(400);
  await tapKey(page, sheet.locator('.simple-keyboard'), 24);
  await page.getByRole('button', { name: /^Split here: from C4 up/ }).click();
  await expect(page.locator('.simple-sound')).toHaveCount(8);
  await expect(page.getByRole('button', { name: 'Keys: C2 – B3' })).toHaveCount(3);

  await page.getByRole('button', { name: '+ Add a sound' }).click();
  await page.getByRole('button', { name: 'Couple the Positif' }).click();
  await expect(page.locator('.simple-sound')).toHaveCount(9);
  await page.getByRole('button', { name: 'Undo', exact: true }).click();
  await expect(page.locator('.simple-sound')).toHaveCount(8);
});

test('timing and fine-tuning wait one step away', async ({ page }) => {
  await page.goto('/?study=1&panel=simple');
  await page.getByRole('button', { name: 'More for Prestant 4′' }).click();
  await page.getByRole('menuitem', { name: 'Timing…' }).click();
  await page.getByRole('textbox', { name: 'Starts after the key, in ms' }).fill('120');
  await page.getByRole('button', { name: 'Close' }).click();
  await expect(page.getByRole('button', { name: '120 ms late' })).toBeVisible();
});

for (const [width, height] of [[1280, 800], [390, 844]]) test(`simple stop fits ${width} px`, async ({ page }) => {
  await page.setViewportSize({ width, height });
  await page.goto('/?study=1&panel=simple');
  await expect(page.locator('.simple-sound')).toHaveCount(7);
  expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(width);
  await page.screenshot({ path: `test-results/simple-stop-${width}.png`, fullPage: true });
});
