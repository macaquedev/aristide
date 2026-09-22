import { test, expect } from '@playwright/test';

for (const layout of ['channel', 'rack', 'inspector', 'tabbed']) {
  test(`tuning ${layout}: scope controls and layout`, async ({ page }) => {
    const requests: string[] = [];
    page.on('request', r => { if (r.url().includes('/api/')) requests.push(r.url()); });
    await page.goto(`/?study=1&panel=tuning&layout=${layout}`);
    await page.getByRole('button', { name: '415', exact: true }).click();
    await expect(page.getByRole('button', { name: 'Reference pitch: 415 Hz', exact: true })).toBeVisible();
    await page.getByRole('button', { name: 'Grand-orgue', exact: true }).click();
    await expect(page.getByRole('button', { name: 'Reference pitch: 415 Hz', exact: true })).toBeDisabled();
    await page.getByRole('switch', { name: 'Follow Whole instrument' }).uncheck();
    await expect(page.getByRole('button', { name: 'Reference pitch: 415 Hz', exact: true })).toBeEnabled();
    await page.getByRole('button', { name: '440', exact: true }).click();
    await page.getByRole('button', { name: 'Undo', exact: true }).click();
    await expect(page.getByRole('button', { name: 'Reference pitch: 415 Hz', exact: true })).toBeVisible();
    await page.getByRole('button', { name: 'Récit', exact: true }).click();
    await page.screenshot({ path: `test-results/tuning-desk-${layout}.png`, fullPage: true });
    expect(requests).toEqual([]);
    await page.setViewportSize({ width: 390, height: 844 });
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
  });
}

test('custom drag is one undo and layouts preserve edits', async ({ page }) => {
  await page.goto('/?study=1&panel=tuning');
  await page.getByRole('button', { name: 'Récit', exact: true }).click();
  const note = page.getByRole('button', { name: 'Select C', exact: true });
  const bounds = (await note.boundingBox())!;
  await page.mouse.move(bounds.x + bounds.width / 2, bounds.y + bounds.height / 2);
  await page.mouse.down();
  await page.mouse.move(bounds.x + bounds.width / 2, bounds.y + bounds.height * .3, { steps: 8 });
  await page.mouse.up();
  await expect(page.getByRole('button', { name: 'C deviation: 20 ¢', exact: true })).toBeVisible();
  await page.getByRole('button', { name: 'Device rack', exact: true }).click();
  await expect(page.getByRole('button', { name: 'C deviation: 20 ¢', exact: true })).toBeVisible();
  await page.getByRole('button', { name: 'Undo', exact: true }).click();
  await expect(page.getByRole('button', { name: 'C deviation: 0 ¢', exact: true })).toBeVisible();
  await note.focus();
  await note.press('ArrowUp');
  await expect(page.getByRole('button', { name: 'C deviation: 1 ¢', exact: true })).toBeVisible();
  await page.getByRole('button', { name: 'C deviation: 1 ¢', exact: true }).click({ button: 'right' });
  await expect(page.getByRole('dialog')).toContainText('Note deviation');
});

test('tabbed scale supports non-twelve steps and scope search', async ({ page }) => {
  await page.goto('/?study=1&panel=tuning&layout=tabbed');
  await page.getByRole('tab', { name: 'Scale', exact: true }).click();
  await page.getByRole('combobox', { name: 'Tuning system', exact: true }).click();
  await page.getByRole('option', { name: 'Equal division', exact: true }).click();
  await page.getByRole('button', { name: /^Steps per repeat:/ }).click();
  await page.getByRole('textbox', { name: 'Steps per repeat', exact: true }).fill('19');
  await page.getByRole('button', { name: 'Done', exact: true }).click();
  await expect(page.locator('.tuning-note-track')).toHaveCount(19);
  await page.getByRole('tab', { name: 'Note', exact: true }).click();
  await page.getByRole('button', { name: 'Select 19', exact: true }).click();
  await expect(page.getByText('Step 19', { exact: true })).toBeVisible();
  await page.getByRole('textbox', { name: 'Find scope' }).fill('C4');
  await page.getByRole('button', { name: 'Bourdon · C4 pipe', exact: true }).click();
  await expect(page.getByRole('switch', { name: 'Follow Bourdon', exact: true })).toBeChecked();
});

test('touch selects and drags custom tuning', async ({ browser }) => {
  const context = await browser.newContext({ hasTouch: true, viewport: { width: 1024, height: 768 } });
  const page = await context.newPage();
  await page.goto('/?study=1&panel=tuning');
  await page.getByRole('button', { name: 'Récit', exact: true }).tap();
  const note = page.getByRole('button', { name: 'Select C', exact: true });
  const bounds = (await note.boundingBox())!;
  const client = await context.newCDPSession(page);
  const point = { x: bounds.x + bounds.width / 2, y: bounds.y + bounds.height / 2 };
  await client.send('Input.dispatchTouchEvent', { type: 'touchStart', touchPoints: [point] });
  await client.send('Input.dispatchTouchEvent', { type: 'touchMove', touchPoints: [{ ...point, y: point.y - bounds.height * .1 }] });
  await client.send('Input.dispatchTouchEvent', { type: 'touchEnd', touchPoints: [] });
  await expect(page.getByRole('button', { name: 'C deviation: 10 ¢', exact: true })).toBeVisible();
  await context.close();
});
