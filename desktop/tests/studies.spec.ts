import { test, expect } from '@playwright/test';

for (const layout of ['rows', 'steps', 'roll', 'keys']) {
  test(`Build ${layout}: edits survive layout changes; add and undo work`, async ({ page }) => {
    const requests: string[] = [];
    page.on('request', request => { if (request.url().includes('/api/')) requests.push(request.url()); });
    await page.goto(`/?study=1&panel=build&layout=${layout}&legacy=1`);
    await expect(page.getByText('Not saved', { exact: false })).toBeVisible();
    await page.getByRole('button', { name: 'Add voice', exact: true }).click();
    await expect(page.getByText('Grand-orgue · 4 voices per key')).toBeVisible();
    await page.getByRole('button', { name: 'Undo', exact: true }).first().click();
    await expect(page.getByText('Grand-orgue · 3 voices per key')).toBeVisible();
    await page.getByRole('button', { name: 'Step grid', exact: true }).click();
    await expect(page.getByRole('button', { name: 'Step grid', exact: true })).toHaveAttribute('aria-pressed', 'true');
    await expect(page.getByText('Grand-orgue · 3 voices per key')).toBeVisible();
    await page.getByRole('button', { name: ({ rows: 'Voice rows', steps: 'Step grid', roll: 'Piano roll', keys: 'Per-key view' })[layout], exact: true }).click();
    await page.screenshot({ path: `test-results/build-${layout}.png`, fullPage: true });
    expect(requests).toEqual([]);
  });
}

test('number tap, typing, drag and hold use one consistent control', async ({ page }) => {
  await page.goto('/?study=1&layout=rows');
  await page.getByRole('button', { name: 'Voice 1 pitch: 0 ¢', exact: true }).click();
  const input = page.getByRole('textbox', { name: 'Voice 1 pitch', exact: true });
  await input.fill('350');
  await page.getByRole('button', { name: 'Done', exact: true }).click();
  const control = page.getByRole('button', { name: 'Voice 1 pitch: 350 ¢', exact: true });
  const box = (await control.boundingBox())!;
  await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2);
  await page.mouse.down();
  await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2 - 10, { steps: 4 });
  await page.mouse.up();
  await expect(page.getByRole('button', { name: 'Voice 1 pitch: 550 ¢', exact: true })).toBeVisible();
  await page.getByRole('button', { name: 'Voice 1 pitch: 550 ¢', exact: true }).click({ button: 'right' });
  await expect(page.getByText('Assign control · prototype', { exact: true })).toBeVisible();
  await expect(page.getByText('MIDI unavailable in prototype', { exact: false })).toBeVisible();
});

test('source selection and step cells change the study without engine requests', async ({ page }) => {
  await page.goto('/?study=1&layout=steps');
  await page.getByRole('button', { name: 'Voice 1 at 480 ms', exact: true }).click();
  await expect(page.getByRole('button', { name: 'Voice 1 delay: 480 ms', exact: true })).toBeVisible();
  await page.getByRole('button', { name: 'Study organ · Bourdon', exact: true }).click();
  await page.getByRole('button', { name: 'Second organ · String', exact: true }).click();
  await expect(page.getByRole('dialog')).toBeHidden();
  await expect(page.getByRole('button', { name: 'Second organ · String', exact: true })).toBeVisible();
});

for (const layout of ['tree', 'table', 'cascade', 'keyboard']) {
  test(`Tuning ${layout}: inheritance is explicit and removable`, async ({ page }) => {
    await page.goto(`/?study=1&panel=tuning&legacy=1&layout=${layout}`);
    await page.getByRole('button', { name: 'Grand-orgue', exact: true }).click();
    const follow = page.getByRole('switch', { name: 'Follow Whole instrument', exact: true });
    await expect(follow).toBeChecked();
    await expect(page.getByRole('button', { name: 'Reference pitch: 440 Hz', exact: true })).toBeDisabled();
    await follow.uncheck();
    await page.getByRole('button', { name: 'Reference pitch: 440 Hz', exact: true }).click();
    await page.getByRole('textbox', { name: 'Reference pitch', exact: true }).fill('415');
    await page.getByRole('button', { name: 'Done', exact: true }).click();
    await page.getByRole('button', { name: 'Bourdon', exact: true }).click();
    await expect(page.getByRole('button', { name: 'Reference pitch: 415 Hz', exact: true })).toBeDisabled();
    await page.screenshot({ path: `test-results/tuning-${layout}.png`, fullPage: true });
  });
}

test('study sheets and controls fit a narrow touchscreen', async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await page.goto('/?study=1&layout=rows');
  await expect(page.getByRole('button', { name: 'Voice rows', exact: true })).toBeVisible();
  expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(390);
  await page.screenshot({ path: 'test-results/study-small.png', fullPage: true });
  await page.getByRole('button', { name: 'Study organ · Bourdon', exact: true }).click();
  await expect(page.getByRole('heading', { name: 'Source', exact: true })).toBeVisible();
  expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(390);
});

test('two-finger editing changes only the held pipe and marks its override', async ({ browser }) => {
  const page = await browser.newPage({ hasTouch: true, viewport: { width: 1280, height: 1000 } });
  await page.goto('/?study=1&layout=rows');
  const keyboard = page.getByRole('button', { name: 'Hold C4', exact: true });
  const key = (await keyboard.boundingBox())!;
  const cdp = await page.context().newCDPSession(page);
  const first = { id: 1, x: key.x + key.width / 2, y: key.y + key.height / 2 };
  await cdp.send('Input.dispatchTouchEvent', { type: 'touchStart', touchPoints: [first] });
  await expect(page.getByText('C4 only', { exact: false })).toBeVisible();
  const number = (await page.getByRole('button', { name: 'Voice 1 pitch: 0 ¢', exact: true }).boundingBox())!;
  const second = { id: 2, x: number.x + number.width / 2, y: number.y + number.height / 2 };
  await cdp.send('Input.dispatchTouchEvent', { type: 'touchStart', touchPoints: [first, second] });
  await cdp.send('Input.dispatchTouchEvent', { type: 'touchMove', touchPoints: [first, { ...second, y: second.y - 10 }] });
  await expect(page.getByRole('button', { name: 'Voice 1 pitch: 200 ¢', exact: true })).toBeVisible();
  await cdp.send('Input.dispatchTouchEvent', { type: 'touchEnd', touchPoints: [] });
  await expect(page.getByRole('button', { name: 'Voice 1 pitch: 0 ¢', exact: true })).toBeVisible();
  await expect(keyboard.getByLabel('Pipe override')).toBeVisible();
  await page.close();
});
