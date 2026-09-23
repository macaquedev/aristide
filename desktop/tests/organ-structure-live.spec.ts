import { test, expect, type Page } from '@playwright/test';

// Runs against a real aristide-server with an organ loaded, e.g.
// ARISTIDE_LIVE=1 bunx playwright test organ-structure-live
test.skip(!process.env.ARISTIDE_LIVE, 'needs a running engine');

type State = { loading?: string; stops: { id: number; name: string; manual: string; midx: number }[]; manuals: { idx: number; name: string }[]; couplers: { idx: number; name: string; hidden?: boolean }[] };
const snapshot = async (page: Page): Promise<State> => (await page.request.get('/api/state')).json();
const settled = async (page: Page) => expect.poll(async () => (await snapshot(page)).loading ?? '', { timeout: 20_000 }).toBe('');

test('Organ adds, renames, moves and removes divisions, stops and couplers', async ({ page }) => {
  test.setTimeout(90_000);
  await page.goto('/');
  await page.getByRole('button', { name: 'Perform', exact: true }).click();
  await page.getByRole('button', { name: 'Organ', exact: true }).click();
  const tree = page.getByRole('navigation', { name: 'Organ' });
  const start = await snapshot(page);
  const first = start.stops[0];

  // Renaming lands live, without a rebuild.
  const name = page.getByRole('textbox', { name: 'Stop name' });
  await name.fill(`${first.name} renamed`);
  await name.press('Enter');
  await expect.poll(async () => (await snapshot(page)).stops.some(s => s.name === `${first.name} renamed`)).toBe(true);
  await settled(page);
  await expect(tree.getByRole('button', { name: `${first.name} renamed` })).toBeVisible();
  await name.fill(first.name);
  await name.press('Enter');
  await expect.poll(async () => (await snapshot(page)).stops.some(s => s.name === first.name)).toBe(true);

  // A new division, then a stop pulled into it from the sample set.
  await tree.getByRole('button', { name: 'Add division', exact: true }).click();
  await page.getByRole('dialog', { name: 'Add division' }).getByRole('textbox', { name: 'Name', exact: true }).fill('Echo');
  await page.getByRole('dialog', { name: 'Add division' }).getByRole('button', { name: 'Add division', exact: true }).click();
  await expect.poll(async () => (await snapshot(page)).manuals.some(m => m.name === 'Echo'), { timeout: 20_000 }).toBe(true);
  await settled(page);
  await expect(page.getByRole('textbox', { name: 'Division name' })).toHaveValue('Echo');

  await tree.getByRole('region', { name: 'Echo' }).getByRole('button', { name: 'Add stop', exact: true }).click();
  const sheet = page.getByRole('dialog', { name: 'Add stop to Echo' });
  // A stop already in the organ can be added again: a copy sharing its samples.
  const offered = sheet.getByRole('button', { name: /^Bourdon 8'/ }).first();
  await expect(offered).toBeVisible({ timeout: 15_000 });
  await page.screenshot({ path: 'test-results/organ-add-stop.png', fullPage: true });
  await offered.click();
  await expect.poll(async () => (await snapshot(page)).stops.filter(s => s.manual === 'Echo').length, { timeout: 20_000 }).toBe(1);
  await settled(page);
  await expect(page.locator('.roll-stop-name').first()).toContainText('Bourdon 8');
  await page.screenshot({ path: 'test-results/organ-structure.png', fullPage: true });

  // Move it to a division without a stop of that name, then remove it. A division
  // that already has one is not offered.
  await page.getByRole('combobox', { name: 'Division', exact: true }).click();
  await expect(page.getByRole('option', { name: start.manuals[0].name, exact: true })).toHaveAttribute('data-combobox-disabled', 'true');
  await page.getByRole('option', { name: start.manuals[1].name, exact: true }).click();
  await expect.poll(async () => (await snapshot(page)).stops.filter(s => s.manual === 'Echo').length, { timeout: 20_000 }).toBe(0);
  await settled(page);
  await expect(page.locator('.roll-stop-name').first()).toContainText('Bourdon 8');
  await page.getByRole('button', { name: 'Remove', exact: true }).click();
  await page.getByRole('dialog').getByRole('button', { name: 'Remove', exact: true }).click();
  await expect.poll(async () => (await snapshot(page)).stops.length, { timeout: 20_000 }).toBe(start.stops.length);
  await settled(page);
  expect((await snapshot(page)).stops.map(s => `${s.manual}/${s.name}`).sort()).toEqual(start.stops.map(s => `${s.manual}/${s.name}`).sort());

  // A coupler from the new division.
  await tree.getByRole('button', { name: 'Add coupler', exact: true }).click();
  const couplerSheet = page.getByRole('dialog', { name: 'Add coupler' });
  await couplerSheet.getByRole('combobox', { name: 'Play from' }).click();
  await page.getByRole('option', { name: start.manuals[1].name, exact: true }).click();
  await couplerSheet.getByRole('combobox', { name: 'Also sounds' }).click();
  await page.getByRole('option', { name: 'Echo', exact: true }).click();
  await couplerSheet.getByRole('textbox', { name: 'Name' }).fill('Echo to test');
  await couplerSheet.getByRole('button', { name: 'Add coupler', exact: true }).click();
  await expect.poll(async () => (await snapshot(page)).couplers.some(c => c.name === 'Echo to test'), { timeout: 20_000 }).toBe(true);
  await settled(page);
  await expect(page.getByRole('textbox', { name: 'Coupler name' })).toHaveValue('Echo to test');
  await page.screenshot({ path: 'test-results/organ-coupler.png', fullPage: true });
  await page.getByRole('button', { name: 'Remove coupler', exact: true }).click();
  await page.getByRole('dialog').getByRole('button', { name: 'Remove', exact: true }).click();
  await expect.poll(async () => (await snapshot(page)).couplers.some(c => c.name === 'Echo to test'), { timeout: 20_000 }).toBe(false);
  await settled(page);

  // Remove the division; the tab stays on Organ through every rebuild.
  await tree.getByRole('button', { name: 'Echo', exact: true }).click();
  await page.getByRole('button', { name: 'Remove division', exact: true }).click();
  await page.getByRole('dialog').getByRole('button', { name: 'Remove', exact: true }).click();
  await expect.poll(async () => (await snapshot(page)).manuals.some(m => m.name === 'Echo'), { timeout: 20_000 }).toBe(false);
  await settled(page);
  await expect(page.getByRole('button', { name: 'Organ', exact: true })).toHaveAttribute('aria-current', 'page');

  await page.setViewportSize({ width: 390, height: 844 });
  await page.screenshot({ path: 'test-results/organ-phone.png', fullPage: true });
  expect(await page.evaluate(() => document.documentElement.scrollWidth - document.documentElement.clientWidth)).toBeLessThanOrEqual(0);
});
