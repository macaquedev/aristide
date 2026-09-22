import { test, expect } from '@playwright/test';
import { initialRoll, validNote, snapPitch } from '../src/design/rollModel';

test('release lifetimes and cents remain independent of visual guides', () => {
  const note = initialRoll.notes[0];
  expect(validNote({ ...note, start: 'u0', end: null }, initialRoll.stamps)).toBe(false);
  expect(validNote({ ...note, start: 'u50', end: 'u0' }, initialRoll.stamps)).toBe(false);
  expect(validNote({ ...note, start: 'u0', end: 'd50' }, initialRoll.stamps)).toBe(false);
  expect(validNote({ ...note, end: 'u50' }, initialRoll.stamps)).toBe(true);
  expect(validNote({ ...note, start: 'u0', end: 'u50' }, initialRoll.stamps)).toBe(true);
  expect(snapPitch(-2, 31, false)).toBe(-2);
  expect(snapPitch(1250, 12, false)).toBe(1250);
  expect(snapPitch(1250, 12, true)).toBe(1300);
});

for (const layout of ['split', 'stacked', 'focus', 'lanes']) {
  test(`${layout}: comparison retains edits and makes no engine requests`, async ({ page }) => {
    const requests: string[] = [];
    page.on('request', request => { if (request.url().includes('/api/')) requests.push(request.url()); });
    await page.goto(`/?study=1&layout=${layout}`);
    await expect(page.getByText('Titanique', { exact: false }).first()).toBeVisible();
    await expect(page.getByRole('checkbox', { name: 'Snap pitch' })).not.toBeChecked();
    await expect(page.getByRole('checkbox', { name: 'Time grid' })).not.toBeChecked();
    await page.screenshot({ path: `test-results/roll-${layout}.png`, fullPage: true });
    await page.getByRole('button', { name: 'Add event', exact: true }).click();
    if (layout === 'focus' || layout === 'lanes') await page.getByRole('dialog').getByRole('button', { name: 'Close' }).click();
    await expect(page.getByText('Grand-orgue · 4 events · Follow division')).toBeVisible();
    await page.getByRole('button', { name: 'Stacked', exact: true }).click();
    await expect(page.getByText('Grand-orgue · 4 events · Follow division')).toBeVisible();
    await page.getByRole('button', { name: 'Undo', exact: true }).click();
    await expect(page.getByText('Grand-orgue · 3 events · Follow division')).toBeVisible();
    expect(requests).toEqual([]);
  });
}

test('hold after release uses paired arrows and a finite release endpoint', async ({ page }) => {
  await page.goto('/?study=1&layout=split');
  await page.getByRole('button', { name: 'Théorbe 0 ¢', exact: true }).click();
  await page.getByRole('combobox', { name: 'Duration', exact: true }).click();
  await page.getByRole('option', { name: 'Hold after release → / ←', exact: true }).click();
  await expect(page.getByTestId('continuation-arrow')).toBeVisible();
  await expect(page.getByRole('combobox', { name: 'Ends at', exact: true })).toHaveValue('Key up + 50 ms');
  await expect(page.getByRole('button', { name: 'Théorbe, 0 ¢, 0 ms, hold after release', exact: true })).toBeVisible();
  await expect(page.getByRole('button', { name: 'Théorbe, 0 ¢, held after release, ends 50 ms', exact: true })).toBeVisible();
  await page.screenshot({ path: 'test-results/roll-continuation.png', fullPage: true });
  await page.getByRole('button', { name: 'Undo', exact: true }).click();
  await expect(page.getByTestId('continuation-arrow')).toHaveCount(0);
});

test('shared timestamps move endpoints together', async ({ page }) => {
  await page.goto('/?study=1&layout=split');
  await page.getByRole('button', { name: 'Edit Key down timestamp 50 ms', exact: true }).click();
  await page.getByRole('button', { name: 'Timestamp: 50 ms', exact: true }).click();
  await page.getByRole('textbox', { name: 'Timestamp', exact: true }).fill('75');
  await page.getByRole('dialog', { name: 'Timestamp: 75 ms', exact: true }).getByRole('button', { name: 'Done', exact: true }).click();
  await expect(page.getByRole('dialog', { name: 'Timestamp: 75 ms', exact: true })).toBeHidden();
  await page.getByRole('dialog', { name: 'Edit timestamp', exact: true }).getByRole('button', { name: 'Done', exact: true }).click();
  await expect(page.getByRole('button', { name: 'Flute 4′, +1250 ¢, 0 ms, ends 75 ms', exact: true })).toBeVisible();
  await expect(page.getByRole('button', { name: 'Ophicleide 32′, -2 ¢, 75 ms, until release', exact: true })).toBeVisible();
  await page.getByRole('button', { name: 'Undo', exact: true }).click();
  await expect(page.getByRole('button', { name: 'Flute 4′, +1250 ¢, 0 ms, ends 50 ms', exact: true })).toBeVisible();
});

test('draw, drag and zoom preserve timestamp constraints', async ({ page }) => {
  await page.goto('/?study=1&layout=split');
  const canvas = page.getByRole('group', { name: 'Key down piano roll', exact: true });
  await canvas.scrollIntoViewIfNeeded();
  const box = (await canvas.boundingBox())!;
  await canvas.click({ position: { x: 80, y: 330 } });
  await expect(page.getByText('Grand-orgue · 4 events · Follow division')).toBeVisible();
  await expect(page.getByRole('combobox', { name: 'Starts at', exact: true })).toHaveValue('Key down + 0 ms');
  await expect(page.getByRole('combobox', { name: 'Duration', exact: true })).toHaveValue('Until release →');
  const event = canvas.locator('.event-note.is-selected');
  const body = (await event.locator('.note-body').boundingBox())!;
  await page.mouse.move(body.x + 25, body.y + 12);
  await page.mouse.down();
  await page.mouse.move(body.x + 25 + (box.width - 84) * 50 / 120, body.y - 25, { steps: 6 });
  await page.mouse.up();
  await expect(page.getByRole('combobox', { name: 'Starts at', exact: true })).toHaveValue('Key down + 50 ms');
  await page.getByRole('button', { name: 'Zoom pitch in', exact: true }).click();
  await page.getByRole('button', { name: 'Zoom time in', exact: true }).click();
  await page.getByRole('button', { name: 'Centre 0 ¢', exact: true }).click();
  await expect(page.getByRole('combobox', { name: 'Starts at', exact: true })).toHaveValue('Key down + 50 ms');
  await page.getByRole('button', { name: 'Undo', exact: true }).click();
  await expect(page.getByRole('combobox', { name: 'Starts at', exact: true })).toHaveValue('Key down + 0 ms');
});

test('release drawing creates a finite ending', async ({ page }) => {
  await page.goto('/?study=1&layout=split');
  const release = page.getByRole('group', { name: 'Key up piano roll', exact: true });
  await release.click({ position: { x: 85, y: 320 } });
  await expect(page.getByRole('combobox', { name: 'Starts at', exact: true })).toHaveValue('Key up + 0 ms');
  await expect(page.getByRole('combobox', { name: 'Ends at', exact: true })).toHaveValue('Key up + 50 ms');
  await page.getByRole('combobox', { name: 'Duration', exact: true }).click();
  await expect(page.getByRole('option', { name: 'Until release →', exact: true })).toHaveCount(0);
});

test('touch drawing and narrow layouts stay usable', async ({ browser }) => {
  const page = await browser.newPage({ hasTouch: true, viewport: { width: 390, height: 844 } });
  await page.goto('/?study=1&layout=focus');
  const canvas = page.getByRole('group', { name: 'Key down piano roll', exact: true });
  await canvas.scrollIntoViewIfNeeded();
  await canvas.scrollIntoViewIfNeeded();
  const box = (await canvas.boundingBox())!;
  await page.touchscreen.tap(box.x + 95, box.y + 280);
  await expect(page.getByRole('dialog', { name: 'Event', exact: true })).toBeVisible();
  await expect(page.getByRole('combobox', { name: 'Duration', exact: true })).toHaveValue('Until release →');
  await page.getByRole('button', { name: 'Close' }).click();
  await expect(page.getByRole('dialog')).toHaveCount(0);
  await page.getByText('Pan', { exact: true }).click();
  await canvas.scrollIntoViewIfNeeded();
  const panBox = (await canvas.boundingBox())!;
  const cdp = await page.context().newCDPSession(page);
  const point = { id: 1, x: panBox.x + 180, y: panBox.y + 230 };
  await cdp.send('Input.dispatchTouchEvent', { type: 'touchStart', touchPoints: [point] });
  await cdp.send('Input.dispatchTouchEvent', { type: 'touchMove', touchPoints: [{ ...point, x: point.x - 70, y: point.y - 40 }] });
  await cdp.send('Input.dispatchTouchEvent', { type: 'touchEnd', touchPoints: [] });
  expect(Number(await page.getByRole('slider', { name: 'Key down time scroll', exact: true }).inputValue())).toBeGreaterThan(0);
  expect(Number(await page.getByRole('slider', { name: 'Key down pitch scroll', exact: true }).inputValue())).toBeLessThan(0);
  await page.getByRole('button', { name: 'Centre 0 ¢', exact: true }).click();
  expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(390);
  await page.screenshot({ path: 'test-results/roll-touch.png', fullPage: true });
  await page.close();
});

test('add a timestamp and resize a finite event to it', async ({ page }) => {
  await page.goto('/?study=1&layout=split');
  await page.getByRole('region', { name: 'Key down timeline', exact: true }).getByRole('button', { name: '+ Timestamp' }).click();
  await page.getByRole('textbox', { name: 'Time (ms)', exact: true }).fill('100');
  await page.getByRole('button', { name: 'Add timestamp', exact: true }).click();
  await expect(page.getByRole('dialog')).toHaveCount(0);
  const canvas = page.getByRole('group', { name: 'Key down piano roll', exact: true });
  await canvas.scrollIntoViewIfNeeded();
  const box = (await canvas.boundingBox())!;
  const handle = (await canvas.locator('[data-note="flute"] [data-resize]').boundingBox())!;
  await page.mouse.move(handle.x + 5, handle.y + 12);
  await page.mouse.down();
  await page.mouse.move(box.x + 70 + (box.width - 84) * 100 / 120, handle.y + 12, { steps: 6 });
  await page.mouse.up();
  await expect(page.getByRole('combobox', { name: 'Ends at', exact: true })).toHaveValue('Key down + 100 ms');
  await page.getByRole('button', { name: 'Undo', exact: true }).click();
  await expect(page.getByRole('combobox', { name: 'Ends at', exact: true })).toHaveValue('Key down + 50 ms');
});

test('pitch guides only snap when enabled; panning does not edit notes', async ({ page }) => {
  await page.goto('/?study=1&layout=split');
  const event = page.locator('[data-note="ophicleide"]');
  await event.focus();
  await page.keyboard.press('ArrowUp');
  await expect(page.getByRole('button', { name: 'Pitch: -1 ¢', exact: true })).toBeVisible();
  await page.getByRole('combobox', { name: 'Global tuning guide', exact: true }).click();
  await page.getByRole('option', { name: 'Global · 31 equal', exact: true }).click();
  await expect(page.getByRole('button', { name: 'Pitch: -1 ¢', exact: true })).toBeVisible();
  await page.getByRole('checkbox', { name: 'Snap pitch' }).check();
  await event.focus();
  await page.keyboard.press('ArrowUp');
  await expect(page.getByRole('button', { name: 'Pitch: 38.71 ¢', exact: true })).toBeVisible();
  const canvas = page.getByRole('group', { name: 'Key down piano roll', exact: true });
  await canvas.scrollIntoViewIfNeeded();
  const box = (await canvas.boundingBox())!;
  await page.mouse.move(box.x + 220, box.y + 300);
  await page.mouse.down({ button: 'middle' });
  await page.mouse.move(box.x + 120, box.y + 240, { steps: 6 });
  await page.mouse.up({ button: 'middle' });
  expect(Number(await page.getByRole('slider', { name: 'Key down time scroll', exact: true }).inputValue())).toBeGreaterThan(0);
  expect(Number(await page.getByRole('slider', { name: 'Key down pitch scroll', exact: true }).inputValue())).toBeLessThan(0);
  await expect(page.getByRole('button', { name: 'Pitch: 38.71 ¢', exact: true })).toBeVisible();
});

test('source references share source edits and ranks remain selectable', async ({ page }) => {
  await page.goto('/?study=1&layout=split');
  await page.getByRole('button', { name: 'Duplicate', exact: true }).click();
  await page.getByRole('button', { name: 'Flute 4′ ↗', exact: true }).click();
  await page.getByRole('button', { name: 'Source pitch: 0 ¢', exact: true }).click();
  await page.getByRole('textbox', { name: 'Source pitch', exact: true }).fill('25');
  await page.getByRole('dialog', { name: 'Source pitch: 25 ¢', exact: true }).getByRole('button', { name: 'Done' }).click();
  await page.getByRole('dialog', { name: 'Source · prototype', exact: true }).getByRole('button', { name: 'Close' }).click();
  await expect(page.getByText('Source offset +25 ¢ · combined +1275 ¢', { exact: true })).toBeVisible();
  await page.getByRole('button', { name: 'Flute 4′ +1250 ¢', exact: true }).first().click();
  await expect(page.getByText('Source offset +25 ¢ · combined +1275 ¢', { exact: true })).toBeVisible();
  await page.getByRole('button', { name: 'Flute 4′ ↗', exact: true }).click();
  await page.getByRole('dialog', { name: 'Source · prototype', exact: true }).getByText('Rank', { exact: true }).click();
  await page.getByRole('button', { name: 'Salicional 8′ Second organ', exact: true }).click();
  await expect(page.getByRole('button', { name: 'Salicional 8′ ↗', exact: true })).toBeVisible();
});

for (const layout of ['split', 'stacked', 'focus', 'lanes']) {
  test(`${layout}: right-click deletes the pointed event and Undo restores it`, async ({ page }) => {
    await page.goto(`/?study=1&layout=${layout}`);
    const event = page.locator('[data-note="theorbe"]');
    await event.locator('.note-body').click({ button: 'right' });
    await expect(event).toHaveCount(0);
    await expect(page.getByText('Grand-orgue · 2 events · Follow division')).toBeVisible();
    await expect(page.locator('[data-note="flute"]')).toHaveCount(1);
    await page.getByRole('button', { name: 'Undo', exact: true }).click();
    await expect(event).toHaveCount(1);
  });
}

test('Delete prefers hovered notes, then selection, and ignores typing and hidden Build', async ({ page }) => {
  await page.goto('/?study=1&layout=split');
  await page.locator('[data-note="theorbe"] .note-body').hover();
  await page.keyboard.press('Delete');
  await expect(page.locator('[data-note="theorbe"]')).toHaveCount(0);
  await expect(page.locator('[data-note="flute"]')).toHaveCount(1);
  await page.getByRole('button', { name: 'Undo', exact: true }).click();
  await page.getByText('Titanique', { exact: false }).first().hover();
  await page.keyboard.press('Delete');
  await expect(page.locator('[data-note="flute"]')).toHaveCount(0);
  await page.getByRole('button', { name: 'Undo', exact: true }).click();
  await page.getByRole('button', { name: 'Pitch: 1250 ¢', exact: true }).click();
  await page.getByRole('textbox', { name: 'Pitch', exact: true }).press('Delete');
  await expect(page.getByText('Grand-orgue · 3 events · Follow division')).toBeVisible();
  await page.getByRole('button', { name: 'Done', exact: true }).click();
  await page.getByRole('radiogroup', { name: 'Study panel' }).getByText('Tuning', { exact: true }).click();
  await page.keyboard.press('Delete');
  await page.getByRole('radiogroup', { name: 'Study panel' }).getByText('Build', { exact: true }).click();
  await expect(page.getByText('Grand-orgue · 3 events · Follow division')).toBeVisible();
});

test('deleting a continuation removes both halves; empty roll right-click does nothing', async ({ page }) => {
  await page.goto('/?study=1&layout=split');
  await page.getByRole('button', { name: 'Try release example' }).click();
  const release = page.getByRole('group', { name: 'Key up piano roll', exact: true });
  await release.locator('[data-note="theorbe"] .note-body').click({ button: 'right' });
  await expect(page.locator('[data-note="theorbe"]')).toHaveCount(0);
  await expect(page.getByText('Grand-orgue · 3 events · Follow division')).toBeVisible();
  await release.click({ button: 'right', position: { x: 90, y: 330 } });
  await expect(page.getByText('Grand-orgue · 3 events · Follow division')).toBeVisible();
  await page.getByRole('button', { name: 'Undo', exact: true }).click();
  await expect(page.locator('[data-note="theorbe"]')).toHaveCount(2);
  await page.screenshot({ path: 'test-results/roll-delete-undo.png', fullPage: true });
});
