import { test, expect, type Page } from '@playwright/test';

async function rig(page: Page) {
  const requests: string[] = [];
  const state = {
    organ: 'Test organ',
    stops: [
      { id: 1, name: 'Bourdon', midx: 0, manual: 'Great', on: false, pitch: { native: 8, footage: null, cents: 0, gain: 0, own: false }, ranks: [{ id: 1, name: 'Bourdon' }] },
      { id: 2, name: 'Principal', midx: 1, manual: 'Pedal', on: false, pitch: { native: 16, footage: null, cents: 0, gain: 0, own: false }, ranks: [{ id: 2, name: 'Principal' }] },
    ],
    manuals: [{ idx: 0, name: 'Great', pedal: false, held: [] }, { idx: 1, name: 'Pedal', pedal: true, held: [] }],
    couplers: [], trems: [], generals: [] as number[], setter: false, gain: 0.178,
    combinations: { matching_generals: [] as number[], divisionals: {}, matching_divisionals: {}, frame: 0, frames: 0 },
    library: [{ name: 'Test organ', path: '/fixtures/demo.organ' }],
    midi: { ports: [], manuals: [] },
  };
  await page.route('**/api/**', async route => {
    const url = new URL(route.request().url());
    if (route.request().method() === 'POST') {
      requests.push(url.pathname + url.search);
      if (url.pathname === '/api/stop') state.stops.find(s => s.id === Number(url.searchParams.get('id')))!.on = url.searchParams.get('on') === '1';
      if (url.pathname === '/api/setter') state.setter = url.searchParams.get('on') === '1';
      if (url.pathname === '/api/general' && state.setter) { state.generals.push(Number(url.searchParams.get('n'))); state.setter = false; }
    }
    if (url.pathname === '/api/rule') {
      const stop = state.stops.find(s => s.id === Number(url.searchParams.get('stop')))!;
      return route.fulfill({ json: {
        stop: { id: stop.id, name: stop.name, manual: stop.manual, midx: stop.midx }, custom: false, voices: 1,
        stamps: [{ id: 'down', anchor: 'down', ms: 0 }, { id: 'up', anchor: 'up', ms: 0 }],
        events: [{ source: { stop: stop.id, rank: null }, cents: 0, level: 0, start: 'down', end: null }],
        sources: state.stops.map(s => ({ stop: s.id, name: s.name, manual: s.manual, midx: s.midx, ranks: s.ranks })),
      } });
    }
    await route.fulfill({ json: state });
  });
  await page.goto('/');
  await expect(page.getByRole('button', { name: 'Bourdon' })).toBeVisible();
  return { requests, state };
}

test('Play opens locked; stop drawing and Set work in perform mode', async ({ page }) => {
  const { requests } = await rig(page);
  await expect(page.getByRole('button', { name: 'Build', exact: true })).toBeDisabled();
  const stop = page.getByRole('button', { name: 'Bourdon' });
  const size = await stop.boundingBox();
  expect(size!.height).toBeGreaterThanOrEqual(60);
  await stop.click();
  await expect(stop).toHaveAttribute('aria-pressed', 'true');
  await stop.click({ button: 'right' });
  await expect(stop).toBeVisible();
  await page.getByRole('button', { name: 'Set', exact: true }).click();
  await page.getByRole('button', { name: 'General 1', exact: true }).click();
  await expect.poll(() => requests.includes('/api/general?n=1')).toBe(true);
  expect(requests.some(url => url.includes('recall=1'))).toBe(false);
});

test('editing a stop requires Edit; relocking returns from Build to Play', async ({ page }) => {
  await rig(page);
  await page.getByRole('button', { name: 'Perform', exact: true }).click();
  await page.getByRole('button', { name: 'Bourdon' }).click({ button: 'right' });
  await expect(page.locator('.roll-stop-name')).toContainText('Bourdon');
  await page.getByRole('button', { name: 'Edit', exact: true }).click();
  await expect(page.getByRole('button', { name: 'Bourdon' })).toBeVisible();
  await expect(page.getByRole('button', { name: 'Build', exact: true })).toBeDisabled();
});

test('held computer keys survive navigation and release in the other panel', async ({ page }) => {
  const { requests } = await rig(page);
  await page.keyboard.down('a');
  await expect.poll(() => requests.includes('/api/key?code=KeyA&on=1')).toBe(true);
  await page.getByRole('button', { name: 'Library', exact: true }).click();
  await page.keyboard.up('a');
  await expect.poll(() => requests.includes('/api/key?code=KeyA&on=0')).toBe(true);
  expect(requests).toEqual(['/api/key?code=KeyA&on=1', '/api/key?code=KeyA&on=0']);
});

test('touch hold is harmless while locked and opens Build when unlocked', async ({ browser }) => {
  const page = await browser.newPage({ hasTouch: true, viewport: { width: 800, height: 1000 } });
  const { requests } = await rig(page);
  const stop = page.getByRole('button', { name: 'Bourdon' });
  await stop.dispatchEvent('pointerdown', { pointerType: 'touch', pointerId: 1, button: 0 });
  await page.waitForTimeout(700);
  await expect(stop).toBeVisible();
  await stop.dispatchEvent('pointerup', { pointerType: 'touch', pointerId: 1, button: 0 });
  await stop.dispatchEvent('click');
  await expect(stop).toHaveAttribute('aria-pressed', 'false');
  await page.getByRole('button', { name: 'Perform', exact: true }).tap();
  await stop.dispatchEvent('pointerdown', { pointerType: 'touch', pointerId: 2, button: 0 });
  await expect(page.locator('.roll-stop-name')).toContainText('Bourdon');
  expect(requests.filter(url => url.startsWith('/api/stop'))).toHaveLength(0);
  await page.close();
});

test('small screen keeps navigation and pistons inside the viewport', async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await rig(page);
  expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(390);
  await expect(page.getByRole('button', { name: 'Panic', exact: true })).toBeVisible();
  await page.screenshot({ path: 'test-results/play-small.png', fullPage: true });
});

test('medium touchscreen visual review', async ({ page }) => {
  await rig(page);
  await page.getByRole('button', { name: 'Bourdon' }).click();
  await expect(page.getByRole('button', { name: 'Bourdon' })).toHaveAttribute('aria-pressed', 'true');
  await page.screenshot({ path: 'test-results/play-medium.png', fullPage: true });
});


test('offline browser offers previews without blaming the audio device', async ({ page }) => {
  await page.route('**/api/**', route => route.abort());
  await page.goto('/');
  const dialog = page.getByRole('dialog');
  await expect(dialog).toContainText('Aristide is not connected');
  await expect(dialog).not.toContainText('audio device');
  await expect(dialog).toHaveCSS('opacity', '1');
  await page.screenshot({ path: 'test-results/connection-unavailable.png', fullPage: true });
  await dialog.getByRole('link', { name: 'Preview Build' }).click();
  await expect(page.getByText('Not saved', { exact: false })).toBeVisible();
  await expect(page.getByRole('dialog')).toHaveCount(0);
});

test('connection recovery clears its modal without restarting', async ({ page }) => {
  await rig(page);
  let offline = true;
  await page.route('**/api/**', route => offline ? route.abort() : route.fallback());
  await expect(page.getByRole('dialog')).toContainText('Aristide is not connected');
  offline = false;
  await expect(page.getByRole('dialog')).toHaveCount(0);
  await expect(page.getByRole('button', { name: 'Panic', exact: true })).toBeEnabled();
});

test('dismissed connection error stays closed until a new outage', async ({ page }) => {
  await rig(page);
  let offline = true;
  await page.route('**/api/**', route => offline ? route.abort() : route.fallback());
  await expect(page.getByRole('dialog')).toBeVisible();
  await page.getByRole('dialog').getByRole('button', { name: 'Close', exact: true }).click();
  await page.waitForTimeout(800);
  await expect(page.getByRole('dialog')).toHaveCount(0);
  offline = false;
  await expect(page.getByRole('button', { name: 'Panic', exact: true })).toBeEnabled();
  offline = true;
  await expect(page.getByRole('dialog')).toContainText('Aristide is not connected');
});

test('with no organ loaded the app opens on the Library', async ({ page }) => {
  await page.route('**/api/**', route => route.fulfill({ json: {
    stops: [], manuals: [], couplers: [], trems: [], generals: [], setter: false, gain: 0.178,
    library: [{ name: 'Test organ', path: '/fixtures/demo.organ' }], midi: { ports: [], manuals: [] },
  } }));
  await page.goto('/');
  await expect(page.getByRole('button', { name: 'Library', exact: true })).toHaveAttribute('aria-current', 'page');
  await expect(page.getByRole('button', { name: /Test organ/ })).toBeVisible();
});
