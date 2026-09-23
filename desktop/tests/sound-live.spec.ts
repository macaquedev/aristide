import { test, expect, type Page } from '@playwright/test';

// Runs against a real aristide-server with an organ loaded, e.g.
// ARISTIDE_LIVE=1 bunx playwright test sound-live
test.skip(!process.env.ARISTIDE_LIVE, 'needs a running engine');

const routing = async (page: Page) => (await page.request.get('/api/routing')).json();

test('the connected Sound panel routes sources to speaker groups', async ({ page }) => {
  await page.goto('/');
  await expect(page.getByRole('button', { name: 'Play', exact: true })).toBeVisible();
  // A held note must survive every panel change and edit below.
  const state = await (await page.request.get('/api/state')).json();
  const stop = state.stops.find((s: { midx: number }) => s.midx === 1);
  await page.request.post(`/api/stop?id=${stop.id}&on=1`);
  await page.request.post('/api/note?manual=1&key=60&on=1');
  // Start from the default routing; a set's own organ refuses (409) and is already there.
  const start = await routing(page);
  await page.request.post('/api/routing?manual=1&follow=1');
  await page.request.post(`/api/routing?stop=${start.stops.find((s: { midx: number }) => s.midx === 1).id}&follow=1`);

  await page.getByRole('button', { name: 'Perform', exact: true }).click();
  await page.getByRole('button', { name: 'Settings', exact: true }).click();
  await page.getByRole('textbox', { name: 'New group' }).fill('Rear');
  await page.getByRole('button', { name: 'Add', exact: true }).click();
  await expect(page.getByRole('button', { name: 'Remove Rear', exact: true })).toBeVisible();
  await page.screenshot({ path: 'test-results/sound-speakers.png', fullPage: true });

  await page.getByRole('button', { name: 'Sound', exact: true }).click();
  const division = (await routing(page)).divisions[1].name as string;
  await expect(page.getByRole('columnheader', { name: /Rear/ })).toBeVisible();
  await page.getByRole('button', { name: `Connect ${division} to Rear`, exact: true }).click();
  const cell = page.getByRole('button', { name: `${division} to Rear: 0 dB`, exact: true });
  await expect(cell).toBeVisible();
  await expect.poll(async () => (await routing(page)).divisions[1].sends.Rear).toBe(0);

  // Drag down to lower the level; it lands without a release click.
  const box = (await cell.boundingBox())!;
  await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2);
  await page.mouse.down();
  await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2 + 24, { steps: 6 });
  await page.mouse.up();
  await expect.poll(async () => (await routing(page)).divisions[1].sends.Rear).toBe(-6);

  // Tap opens the stepper.
  await page.getByRole('button', { name: `${division} to Rear: -6 dB`, exact: true }).click();
  await page.getByRole('button', { name: `Raise ${division} to Rear`, exact: true }).click();
  await expect.poll(async () => (await routing(page)).divisions[1].sends.Rear).toBe(-5);
  await page.getByRole('button', { name: 'Done', exact: true }).click();

  // A stop follows its division until it gets sends of its own.
  const first = (await routing(page)).stops.find((s: { midx: number }) => s.midx === 1);
  await page.locator('.route-expand', { hasText: division }).click();
  await expect(page.getByRole('button', { name: `${first.name} to Rear: -5 dB`, exact: true })).toHaveAttribute('data-inherited', 'true');
  await page.getByRole('button', { name: `${first.name} to Main: 0 dB`, exact: true }).click();
  await page.getByRole('button', { name: 'Disconnect', exact: true }).click();
  await expect(page.getByRole('button', { name: `${first.name} follow`, exact: true })).toBeVisible();
  await expect.poll(async () => (await routing(page)).stops.find((s: { id: number }) => s.id === first.id).own).toBe(true);
  await page.mouse.click(5, 5);
  await page.waitForTimeout(300);
  await page.screenshot({ path: 'test-results/sound-live.png', fullPage: true });

  await page.getByRole('button', { name: 'Undo', exact: true }).click();
  await expect.poll(async () => (await routing(page)).stops.find((s: { id: number }) => s.id === first.id).own).toBe(false);
  await page.getByRole('button', { name: `${first.name} to Main: 0 dB`, exact: true }).click();
  await page.getByRole('button', { name: 'Disconnect', exact: true }).click();
  await page.getByRole('button', { name: `${first.name} follow`, exact: true }).click();
  await expect.poll(async () => (await routing(page)).stops.find((s: { id: number }) => s.id === first.id).own).toBe(false);

  // Perform mode locks the matrix.
  await page.getByRole('button', { name: 'Edit', exact: true }).click();
  await expect(page.getByRole('button', { name: `${division} to Rear: -5 dB`, exact: true })).toBeDisabled();

  const after = await (await page.request.get('/api/state')).json();
  expect(after.manuals[1].held).toContain(60);
  expect(after.loading ?? null).toBeNull();
  await page.request.post('/api/note?manual=1&key=60&on=0');
  await page.request.post(`/api/stop?id=${stop.id}&on=0`);
});

test('Sound fits a phone', async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await page.goto('/');
  await page.getByRole('button', { name: 'Sound', exact: true }).click();
  await expect(page.getByRole('columnheader', { name: /Main/ })).toBeVisible();
  const width = await page.evaluate(() => document.documentElement.scrollWidth);
  expect(width).toBeLessThanOrEqual(390);
  await page.screenshot({ path: 'test-results/sound-phone.png', fullPage: true });
});
