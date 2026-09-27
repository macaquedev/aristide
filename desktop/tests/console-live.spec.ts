import { test, expect, type Page } from '@playwright/test';

// Runs against a real aristide-server with an organ loaded and its own
// config directory, e.g. ARISTIDE_LIVE=1 bunx playwright test console-live
test.skip(!process.env.ARISTIDE_LIVE, 'needs a running engine');

const state = async (page: Page) => (await page.request.get('/api/state')).json();
const device = async (page: Page, manual: number) => (await state(page)).midi.manuals[manual].inputs[0]?.device;

test('each manual of the organ is played from the keyboard pressed for it', async ({ page }) => {
  await page.goto('/');
  await expect(page.getByRole('button', { name: 'Play', exact: true })).toBeVisible();
  const organ = await state(page);
  for (const manual of organ.manuals) await page.request.post(`/api/midi/assign?manual=${manual.idx}&device=`);
  const [first, second] = organ.manuals.filter((m: { pedal: boolean }) => !m.pedal);
  const stop = organ.stops.find((s: { midx: number }) => s.midx === first.idx);
  await page.request.post(`/api/stop?id=${stop.id}&on=1`);
  await page.request.post(`/api/note?manual=${first.idx}&key=60&on=1`);

  await page.getByRole('button', { name: 'Settings', exact: true }).click();
  await page.getByRole('tab', { name: 'Console' }).click();
  for (const manual of organ.manuals) await expect(page.getByRole('cell', { name: manual.name, exact: true })).toBeVisible();

  // One manual: press a letter and the computer keyboard plays it.
  const row = page.getByRole('row', { name: new RegExp(second.name) });
  await row.getByRole('button', { name: 'Detect' }).click();
  await expect(row.getByText(`Press any key on the keyboard for ${second.name}.`)).toBeVisible();
  await page.keyboard.down('KeyZ'); await page.keyboard.up('KeyZ');
  await expect.poll(() => device(page, second.idx)).toBe('Computer keyboard');
  await page.screenshot({ path: 'test-results/console-organ.png', fullPage: true });

  // Choosing it for another manual moves it there; nothing waits on a question.
  await page.getByRole('combobox', { name: `${first.name} MIDI device` }).click();
  await page.getByRole('option', { name: 'Computer keyboard' }).click();
  await expect.poll(() => device(page, first.idx)).toBe('Computer keyboard');
  expect(await device(page, second.idx)).toBeUndefined();

  // Detect all walks the organ's manuals.
  await page.getByRole('button', { name: 'Detect all' }).click();
  const sheet = page.getByRole('dialog');
  await expect(sheet.getByText(`Press any key on the keyboard for ${organ.manuals[0].name}.`)).toBeVisible();
  await sheet.getByRole('button', { name: 'Skip' }).click();
  await expect(sheet.getByText(`Press any key on the keyboard for ${organ.manuals[1].name}.`)).toBeVisible();
  await page.keyboard.down('KeyX'); await page.keyboard.up('KeyX');
  await expect(sheet.getByText(`Press any key on the keyboard for ${organ.manuals[2].name}.`)).toBeVisible();
  await page.screenshot({ path: 'test-results/console-detect-all.png', fullPage: true });
  await sheet.getByRole('button', { name: 'Skip' }).click();
  await expect(sheet.getByText('Every manual has its keyboard.')).toBeVisible();
  await sheet.getByRole('button', { name: 'Done' }).click();
  expect(await device(page, organ.manuals[1].idx)).toBe('Computer keyboard');

  // Nothing above stopped the held note.
  expect((await state(page)).manuals[first.idx].held).toContain(60);
  await page.request.post(`/api/note?manual=${first.idx}&key=60&on=0`);
});
