import { test, expect, type Page } from '@playwright/test';

// Runs against a real aristide-server with an organ loaded and its own
// config directory, e.g. ARISTIDE_LIVE=1 bunx playwright test console-live
test.skip(!process.env.ARISTIDE_LIVE, 'needs a running engine');

const state = async (page: Page) => (await page.request.get('/api/state')).json();

test('the console is detected once and plays the organ in order', async ({ page }) => {
  for (const _ of (await state(page)).console.keyboards) await page.request.post('/api/console/remove?keyboard=0');
  await page.goto('/');
  await expect(page.getByRole('button', { name: 'Play', exact: true })).toBeVisible();
  const organ = await state(page);
  const hand = organ.manuals.find((m: { pedal: boolean }) => !m.pedal);
  const stop = organ.stops.find((s: { midx: number }) => s.midx === hand.idx);
  await page.request.post(`/api/stop?id=${stop.id}&on=1`);
  await page.request.post(`/api/note?manual=${hand.idx}&key=60&on=1`);

  await page.getByRole('button', { name: 'Settings', exact: true }).click();
  await page.getByRole('tab', { name: 'Console' }).click();
  await expect(page.getByText('No keyboards yet.')).toBeVisible();
  await page.screenshot({ path: 'test-results/console-empty.png', fullPage: true });

  // Detection: a letter answers for the computer keyboard.
  await page.getByRole('button', { name: 'Detect console' }).first().click();
  await expect(page.getByText('Press any key on your lowest manual.')).toBeVisible();
  await page.keyboard.down('KeyZ'); await page.keyboard.up('KeyZ');
  await expect(page.getByText(/Manual 1: Computer keyboard/)).toBeVisible();
  // The same keyboard again is a slip, not a second manual.
  await page.keyboard.down('KeyX'); await page.keyboard.up('KeyX');
  await expect(page.getByText(/That was Manual 1/)).toBeVisible();
  await page.screenshot({ path: 'test-results/console-detect.png', fullPage: true });
  await page.getByRole('button', { name: 'That was the top manual' }).click();
  await expect(page.getByText('Press any pedal.')).toBeVisible();
  await page.getByRole('button', { name: 'No pedalboard' }).click();
  await page.getByRole('button', { name: 'Done', exact: true }).click();
  await expect(page.getByRole('textbox', { name: 'Keyboard name' })).toHaveValue('Manual 1');
  await expect(page.getByRole('cell', { name: hand.name, exact: true })).toBeVisible();

  // Choosing the computer keyboard for another keyboard moves it there.
  await page.getByRole('button', { name: 'Manual', exact: true }).click();
  await page.getByRole('combobox', { name: 'Manual 2 device' }).click();
  await page.getByRole('option', { name: 'Computer keyboard' }).click();
  await expect.poll(async () => (await state(page)).console.keyboards.map((k: { device: string }) => k.device)).toEqual(['', 'Computer keyboard']);
  await page.screenshot({ path: 'test-results/console-moved.png', fullPage: true });

  // The organ chooses which keyboard plays a division.
  await page.getByRole('button', { name: 'Organ', exact: true }).click();
  await page.getByRole('button', { name: hand.name, exact: true }).click();
  await page.getByRole('combobox', { name: 'Played from' }).click();
  await page.getByRole('option', { name: 'Manual 2' }).click();
  await expect.poll(async () => (await state(page)).midi.manuals[hand.idx]).toMatchObject({ keyboard: 1, automatic: false });
  await page.screenshot({ path: 'test-results/console-organ.png', fullPage: true });
  await page.request.post(`/api/console/map?manual=${hand.idx}&keyboard=auto`);

  // Nothing above stopped the held note.
  expect((await state(page)).manuals[hand.idx].held).toContain(60);
  await page.request.post(`/api/note?manual=${hand.idx}&key=60&on=0`);
});
