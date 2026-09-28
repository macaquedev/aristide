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
  await page.getByRole('button', { name: 'Delete stop', exact: true }).click();
  await page.getByRole('dialog').getByRole('button', { name: 'Delete', exact: true }).click();
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
  await page.getByRole('button', { name: 'Delete coupler', exact: true }).click();
  await page.getByRole('dialog').getByRole('button', { name: 'Delete', exact: true }).click();
  await expect.poll(async () => (await snapshot(page)).couplers.some(c => c.name === 'Echo to test'), { timeout: 20_000 }).toBe(false);
  await settled(page);

  // Remove the division; the tab stays on Organ through every rebuild.
  await tree.getByRole('button', { name: 'Echo', exact: true }).click();
  await page.getByRole('button', { name: 'Delete division', exact: true }).click();
  await page.getByRole('dialog').getByRole('button', { name: 'Delete', exact: true }).click();
  await expect.poll(async () => (await snapshot(page)).manuals.some(m => m.name === 'Echo'), { timeout: 20_000 }).toBe(false);
  await settled(page);
  await expect(page.getByRole('button', { name: 'Organ', exact: true })).toHaveAttribute('aria-current', 'page');

  await page.setViewportSize({ width: 390, height: 844 });
  await page.screenshot({ path: 'test-results/organ-phone.png', fullPage: true });
  expect(await page.evaluate(() => document.documentElement.scrollWidth - document.documentElement.clientWidth)).toBeLessThanOrEqual(0);
});

test('A stop renames and deletes from its menu and the Delete key', async ({ page }) => {
  test.setTimeout(90_000);
  await page.goto('/');
  await page.getByRole('button', { name: 'Organ', exact: true }).click();
  const tree = page.getByRole('navigation', { name: 'Organ' });
  const start = await snapshot(page);
  const first = start.stops[0];

  await tree.getByRole('button', { name: first.name, exact: true }).click({ button: 'right' });
  await expect(page.getByRole('menuitem', { name: 'Delete' })).toBeVisible();
  await page.waitForTimeout(300);
  await page.screenshot({ path: 'test-results/organ-stop-menu.png' });
  await page.getByRole('menuitem', { name: 'Rename' }).click();
  const dialog = page.getByRole('dialog', { name: `Rename ${first.name}` });
  await dialog.getByRole('textbox', { name: 'Name' }).fill('Menu renamed');
  await dialog.getByRole('button', { name: 'Rename' }).click();
  await expect.poll(async () => (await snapshot(page)).stops.some(s => s.name === 'Menu renamed')).toBe(true);
  await settled(page);

  // The Delete key asks first; Cancel keeps the stop.
  await tree.getByRole('button', { name: 'Menu renamed', exact: true }).click();
  await page.keyboard.press('Delete');
  await expect(page.getByRole('dialog', { name: 'Delete Menu renamed?' })).toBeVisible();
  await page.waitForTimeout(300);
  await page.screenshot({ path: 'test-results/organ-stop-delete.png' });
  await page.getByRole('dialog').getByRole('button', { name: 'Cancel' }).click();
  expect((await snapshot(page)).stops.length).toBe(start.stops.length);

  await tree.getByRole('button', { name: 'Menu renamed', exact: true }).click({ button: 'right' });
  await page.getByRole('menuitem', { name: 'Rename' }).click();
  await page.getByRole('dialog').getByRole('textbox', { name: 'Name' }).fill(first.name);
  await page.getByRole('dialog').getByRole('textbox', { name: 'Name' }).press('Enter');
  await expect.poll(async () => (await snapshot(page)).stops.some(s => s.name === first.name)).toBe(true);
});

test('Stops drag to reorder and to another division', async ({ page }) => {
  test.setTimeout(90_000);
  await page.goto('/');
  await page.getByRole('button', { name: 'Organ', exact: true }).click();
  const tree = page.getByRole('navigation', { name: 'Organ' });
  const start = await snapshot(page);
  const [pedal, manual] = start.manuals;
  const order = async (midx: number) => (await snapshot(page)).stops.filter(s => s.midx === midx).map(s => s.name);
  const drag = async (from: string, to: string, where: 'top' | 'bottom') => {
    const row = tree.getByRole('button', { name: to, exact: true });
    await row.scrollIntoViewIfNeeded();
    const source = await tree.getByRole('button', { name: from, exact: true }).boundingBox();
    if (!source) throw new Error('row not laid out');
    await page.mouse.move(source.x + 40, source.y + source.height / 2);
    await page.mouse.down();
    await page.mouse.move(source.x + 40, source.y + source.height / 2 + 10, { steps: 3 });
    // The tree scrolls under a drag near its edges; aim at where the target is now.
    for (let i = 0; i < 2; i++) {
      const target = await row.boundingBox();
      if (!target) throw new Error('row not laid out');
      await page.mouse.move(target.x + 40, target.y + (where === 'top' ? 4 : target.height - 4), { steps: 4 });
    }
    return async () => page.mouse.up();
  };

  // Last stop of the first manual to its top.
  const before = await order(manual.idx);
  const last = before[before.length - 1];
  const release = await drag(last, before[0], 'top');
  await page.screenshot({ path: 'test-results/organ-drag.png' });
  await release();
  await expect.poll(() => order(manual.idx)).toEqual([last, ...before.slice(0, -1)]);
  expect((await snapshot(page)).loading ?? '').toBe('');

  // A stop whose name the Pedal lacks moves there, after the Pedal's first stop.
  const pedalNames = await order(pedal.idx);
  const travelling = before.find(name => !pedalNames.includes(name))!;
  await (await drag(travelling, pedalNames[0], 'bottom'))();
  await expect.poll(() => order(pedal.idx)).toEqual([pedalNames[0], travelling, ...pedalNames.slice(1)]);
  await expect(tree.getByRole('region', { name: pedal.name }).getByRole('button', { name: travelling, exact: true })).toBeVisible();

  // And back, restoring the first manual's order.
  await (await drag(travelling, before[before.indexOf(travelling) + 1] ?? before[0], 'top'))();
  await expect.poll(async () => (await order(manual.idx)).includes(travelling)).toBe(true);
  const restore = before.map(name => (start.stops.find(s => s.midx === manual.idx && s.name === name))!.id);
  await page.request.post(`/api/organ/stop/order?manual=${manual.idx}&stops=${restore.join(',')}`);
  await expect.poll(() => order(manual.idx)).toEqual(before);
});

test('A blank stop is added silent, takes a source, and keeps it through edits', async ({ page }) => {
  test.setTimeout(120_000);
  await page.goto('/');
  await page.getByRole('button', { name: 'Organ', exact: true }).click();
  const tree = page.getByRole('navigation', { name: 'Organ' });
  const start = await snapshot(page);
  const manual = start.manuals[1];
  const source = start.stops.find(s => s.midx === manual.idx)!;
  const rule = async (id: number) => (await page.request.get(`/api/rule?stop=${id}`)).json() as Promise<{ custom: boolean; voices: number }>;
  const blankId = async (name: string) => (await snapshot(page)).stops.find(s => s.name === name)?.id;

  await tree.getByRole('region', { name: manual.name }).getByRole('button', { name: 'Add stop', exact: true }).click();
  const sheet = page.getByRole('dialog', { name: `Add stop to ${manual.name}` });
  await expect(sheet.getByRole('textbox', { name: 'Blank stop name' })).toHaveValue('New stop');
  await sheet.getByRole('textbox', { name: 'Blank stop name' }).fill(source.name);
  await expect(sheet.getByRole('button', { name: 'Add blank stop' })).toBeDisabled();
  await sheet.getByRole('textbox', { name: 'Blank stop name' }).fill('Idea');
  await page.waitForTimeout(300);
  await page.screenshot({ path: 'test-results/organ-blank-sheet.png' });
  await sheet.getByRole('button', { name: 'Add blank stop' }).click();
  await expect.poll(() => blankId('Idea'), { timeout: 20_000 }).toBeDefined();
  await settled(page);
  await expect(page.locator('.roll-stop-name').first()).toContainText('Idea');
  await expect(page.getByText('0 voices per key')).toBeVisible();
  await expect(page.getByRole('button', { name: /^Source: / })).toHaveCount(0);

  // Give it pipes from another stop. It has none of its own, so it is never its own source.
  await page.getByRole('button', { name: 'Add event', exact: true }).click();
  await page.getByRole('button', { name: /^Source: / }).click();
  await expect(page.getByRole('dialog', { name: 'Source' }).getByRole('button', { name: 'Idea', exact: true })).toHaveCount(0);
  await page.getByRole('dialog', { name: 'Source' }).getByRole('button', { name: source.name, exact: true }).click();
  await expect.poll(async () => (await rule((await blankId('Idea'))!)).voices).toBeGreaterThan(0);
  await page.screenshot({ path: 'test-results/organ-blank-sourced.png' });

  // Renamed and moved to another division, then rebuilt: the rule stays.
  // A stop with a rule of its own carries the custom mark after its name.
  const row = (name: string) => tree.getByRole('button', { name: new RegExp(`^${name}\\b`) });
  await row('Idea').click({ button: 'right' });
  await page.getByRole('menuitem', { name: 'Rename' }).click();
  await page.getByRole('dialog').getByRole('textbox', { name: 'Name' }).fill('Thought');
  await page.getByRole('dialog').getByRole('textbox', { name: 'Name' }).press('Enter');
  await expect.poll(() => blankId('Thought')).toBeDefined();
  const other = start.manuals.find(m => m.idx !== manual.idx)!;
  await page.getByRole('combobox', { name: 'Division', exact: true }).click();
  await page.getByRole('option', { name: other.name, exact: true }).click();
  await expect.poll(async () => (await snapshot(page)).stops.find(s => s.name === 'Thought')?.midx).toBe(other.idx);
  await expect(page.locator('.roll-stop-name').first()).toContainText('Thought');
  await expect(page.getByText(`${other.name} · 1 voice per key`)).toBeVisible();
  await page.request.post(`/api/organ/stop/blank?on=${encodeURIComponent(other.name)}&name=Rebuild`);
  await settled(page);
  const kept = await rule((await blankId('Thought'))!);
  expect(kept.custom).toBe(true);
  expect(kept.voices).toBeGreaterThan(0);

  for (const name of ['Thought', 'Rebuild']) {
    await expect(page.getByText('Rebuilding the organ')).toBeHidden();
    await row(name).click();
    await page.keyboard.press('Delete');
    await page.getByRole('dialog').getByRole('button', { name: 'Delete', exact: true }).click();
    await expect.poll(() => blankId(name), { timeout: 20_000 }).toBeUndefined();
    await settled(page);
  }
  expect((await snapshot(page)).stops.map(s => `${s.manual}/${s.name}`).sort()).toEqual(start.stops.map(s => `${s.manual}/${s.name}`).sort());
});
