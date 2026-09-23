import { test, expect, type Page } from '@playwright/test';

async function rig(page: Page) {
  const requests: string[] = [];
  const state = {
    organ: 'Test organ', stops: [], manuals: [{ idx: 0, name: 'Great', pedal: false, held: [] }], couplers: [], trems: [],
    generals: [], setter: false, gain: 0.178, library: [], midi: { ports: [], manuals: [] },
  };
  const routing = {
    channels: 4,
    speakers: [
      { name: 'Main', output: [1, 2], defined: true, available: true },
      { name: 'Rear', output: [3, 4], defined: true, available: true },
      { name: 'Gallery', output: null, defined: false, available: false },
    ],
    divisions: [{ idx: 0, name: 'Great', own: true, sends: { Main: 0, Rear: -6 } as Record<string, number> }],
    stops: [
      { id: 1, name: 'Bourdon', midx: 0, own: false, file: null, sends: { Main: 0, Rear: -6 } },
      { id: 2, name: 'Chamade', midx: 0, own: false, file: 'chamade', sends: null },
    ],
  };
  await page.route('**/api/**', async route => {
    const url = new URL(route.request().url());
    if (route.request().method() === 'POST') requests.push(url.pathname + url.search);
    if (url.pathname === '/api/routing') {
      if (route.request().method() === 'POST' && url.searchParams.get('manual') === '0' && !url.searchParams.has('stop')) {
        const group = url.searchParams.get('speakers')!;
        if (url.searchParams.get('off') === '1') delete routing.divisions[0].sends[group];
        else routing.divisions[0].sends[group] = Number(url.searchParams.get('level_db'));
      }
      return route.fulfill({ json: routing });
    }
    await route.fulfill({ json: state });
  });
  await page.goto('/');
  await page.getByRole('button', { name: 'Sound', exact: true }).click();
  return { requests };
}

test('Sound shows inheritance and marks, and locks in perform mode', async ({ page }) => {
  await rig(page);
  await expect(page.getByRole('columnheader', { name: /Gallery/ })).toContainText('Plays through Main');
  // A division opens by itself when one of its stops is routed apart.
  await expect(page.getByText('Organ file · chamade')).toBeVisible();
  await expect(page.getByRole('button', { name: 'Bourdon to Rear: -6 dB', exact: true })).toHaveAttribute('data-inherited', 'true');
  await expect(page.getByRole('button', { name: 'Great to Rear: -6 dB', exact: true })).toBeDisabled();
  await expect(page.getByRole('button', { name: 'Great reset', exact: true })).toHaveCount(0);
  const cell = await page.getByRole('button', { name: 'Great to Main: 0 dB', exact: true }).boundingBox();
  expect(cell!.height).toBeGreaterThanOrEqual(48);
});

test('Sound connects, re-levels and disconnects in edit mode', async ({ page }) => {
  const { requests } = await rig(page);
  await page.getByRole('button', { name: 'Perform', exact: true }).click();
  await page.getByRole('button', { name: 'Connect Great to Gallery', exact: true }).click();
  await expect.poll(() => requests).toContain('/api/routing?manual=0&speakers=Gallery&level_db=0');
  await page.getByRole('button', { name: 'Great to Rear: -6 dB', exact: true }).click();
  await page.getByRole('button', { name: 'Lower Great to Rear', exact: true }).click();
  await expect.poll(() => requests).toContain('/api/routing?manual=0&speakers=Rear&level_db=-7');
  await page.getByRole('button', { name: 'Disconnect', exact: true }).click();
  await expect.poll(() => requests).toContain('/api/routing?manual=0&speakers=Rear&off=1');
  await expect(page.getByRole('button', { name: 'Connect Great to Rear', exact: true })).toBeVisible();
  await page.getByRole('button', { name: 'Undo', exact: true }).click();
  await expect.poll(() => requests.at(-1)).toMatch(/^\/api\/routing\?manual=0&sends=Main%3A0%2CRear%3A-/);
});
