import { test, expect, type Page } from '@playwright/test';

// Runs against a real aristide-server with an organ loaded, e.g.
// ARISTIDE_LIVE=1 bunx playwright test organ-live
test.skip(!process.env.ARISTIDE_LIVE, 'needs a running engine');

type Rule = { custom: boolean; stamps: { id: string; anchor: string; ms: number }[]; events: { cents: number; start: string; end: string | null }[] };
const rule = async (page: Page, stop: number): Promise<Rule> => (await page.request.get(`/api/rule?stop=${stop}`)).json();
const held = async (page: Page) => (await (await page.request.get('/api/state')).json()).manuals[1].held as number[];

test('Organ edits a stop rule live without interrupting a held note', async ({ page }) => {
  await page.goto('/');
  await expect(page.getByRole('button', { name: 'Play', exact: true })).toBeVisible();
  const state = await (await page.request.get('/api/state')).json();
  const stop = state.stops.find((s: { midx: number }) => s.midx === 1);
  await page.request.post(`/api/organ/rule?stop=${stop.id}&reset=1`);
  await page.request.post(`/api/stop?id=${stop.id}&on=1`);
  await page.request.post('/api/note?manual=1&key=60&on=1');

  // The stop editor opens through the stop.
  const knob = page.getByRole('button', { name: new RegExp(`^${stop.name}`) }).first();
  await knob.click({ button: 'right' });
  await expect(page.locator('.roll-stop-name')).toContainText(stop.name);
  await expect(page.getByText(/1 voice per key|voices per key/)).toBeVisible();

  await page.getByRole('button', { name: 'Add event', exact: true }).click();
  await expect.poll(async () => (await rule(page, stop.id)).events.length).toBe(2);
  expect((await rule(page, stop.id)).custom).toBe(true);
  await expect(page.locator('.roll-stop-name .modified')).toBeVisible();

  // Pitch through the shared number stepper.
  await page.getByRole('button', { name: /^Pitch: 0/ }).click();
  await page.getByRole('textbox', { name: 'Pitch' }).fill('1200');
  await page.getByRole('button', { name: 'Done', exact: true }).click();
  await expect.poll(async () => (await rule(page, stop.id)).events[1].cents).toBe(1200);

  // A finite ending creates and attaches a timestamp.
  await page.getByRole('combobox', { name: 'Duration', exact: true }).click();
  await page.getByRole('option', { name: 'End at timestamp' }).click();
  await expect.poll(async () => (await rule(page, stop.id)).events[1].end).not.toBeNull();
  const finite = await rule(page, stop.id);
  expect(finite.stamps.find(s => s.id === finite.events[1].end)).toMatchObject({ anchor: 'down', ms: 50 });
  await page.screenshot({ path: 'test-results/organ-live.png', fullPage: true });

  // Global undo steps back through the stop editor's edits.
  await page.getByRole('button', { name: 'Undo', exact: true }).click();
  await expect.poll(async () => (await rule(page, stop.id)).events[1].end).toBeNull();

  // The stop's own tuning lives in its editor; the Tuning tab keeps only what stops share.
  const tuning = async () => (await (await page.request.get('/api/tuning')).json()).stops.find((s: { id: number }) => s.id === stop.id);
  await page.locator('.mantine-SegmentedControl-label', { hasText: /^Tuning/ }).click();
  const follow = page.getByRole('switch', { name: `Scale follows ${stop.manual}` });
  await expect(follow).toBeChecked();
  await expect(page.getByRole('navigation', { name: 'Tuning scopes' })).toHaveCount(0);
  await follow.dispatchEvent('click');
  await expect.poll(async () => (await tuning()).own.scale).toBe(true);
  await expect(page.locator('.mantine-SegmentedControl-label', { hasText: /^Tuning/ }).locator('.modified')).toBeVisible();
  await page.screenshot({ path: 'test-results/organ-stop-tuning.png', fullPage: true });
  await page.getByRole('switch', { name: `Scale follows ${stop.manual}` }).dispatchEvent('click');
  await expect.poll(async () => (await tuning()).own.scale).toBe(false);
  await page.locator('.mantine-SegmentedControl-label', { hasText: 'Events' }).click();
  await page.getByRole('button', { name: 'Tuning', exact: true }).click();
  await expect(page.getByRole('navigation', { name: 'Tuning scopes' })).toBeVisible();
  await expect(page.getByRole('navigation', { name: 'Tuning scopes' }).getByRole('button', { name: stop.name, exact: true })).toHaveCount(0);
  await page.getByRole('button', { name: 'Organ', exact: true }).click();

  // The held note survived every edit and a round trip through Play.
  expect(await held(page)).toContain(60);
  await page.getByRole('button', { name: 'Play', exact: true }).click();
  await page.getByRole('button', { name: 'Organ', exact: true }).click();
  await expect(page.locator('.roll-stop-name')).toContainText(stop.name);
  expect(await held(page)).toContain(60);

  // The play surface marks the custom stop.
  await page.getByRole('button', { name: 'Play', exact: true }).click();
  await expect(knob.locator('.stop-mark')).toBeVisible();

  await page.setViewportSize({ width: 390, height: 844 });
  await page.getByRole('button', { name: 'Organ', exact: true }).click();
  await expect(page.locator('.roll-stop-name')).toContainText(stop.name);
  await page.screenshot({ path: 'test-results/organ-live-phone.png', fullPage: true });
  const overflow = await page.evaluate(() => document.documentElement.scrollWidth - document.documentElement.clientWidth);
  expect(overflow).toBeLessThanOrEqual(0);
  await page.setViewportSize({ width: 1280, height: 800 });

  await page.getByRole('button', { name: 'Reset', exact: true }).click();
  await expect.poll(async () => (await rule(page, stop.id)).custom).toBe(false);

  await page.request.post('/api/note?manual=1&key=60&on=0');
});
