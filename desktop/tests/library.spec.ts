import { test, expect, type Page } from '@playwright/test';

async function rig(page: Page, loaded = false) {
  const requests: string[] = [];
  await page.addInitScript(() => { (globalThis as { aristidePickFile?: () => Promise<string> }).aristidePickFile = async () => '/sets/abbey.organ'; });
  const state = {
    ...(loaded ? { organ: 'Village' } : {}),
    stops: [], manuals: [], couplers: [], trems: [], generals: [], setter: false, gain: 0.178, midi: { ports: [], manuals: [] },
    library: [
      { name: 'Village', path: '/organs/village.toml', played: Date.now() / 1000, loaded, owned: true },
      { name: 'Demo', path: '/sets/demo.organ', played: Date.now() / 1000 - 86_400 * 3, loaded: false, owned: false },
    ],
  };
  await page.route('**/api/**', async route => {
    const url = new URL(route.request().url());
    if (url.pathname === '/api/library') return route.fulfill({ json: [
      { path: '/organs/village.toml', format: 'GrandOrgue', location: '~/organs/Village' },
      { path: '/sets/demo.organ', format: 'GrandOrgue', location: '/sets' },
    ] });
    if (route.request().method() === 'POST') {
      requests.push(decodeURIComponent(url.pathname + url.search));
      const path = url.searchParams.get('path');
      if (url.pathname === '/api/library/rename') state.library.find(e => e.path === path)!.name = url.searchParams.get('name')!;
      if (url.pathname === '/api/library/delete') state.library = state.library.filter(e => e.path !== path);
    }
    await route.fulfill({ json: state });
  });
  await page.goto('/');
  await page.getByRole('button', { name: 'Library', exact: true }).click();
  await expect(page.getByRole('button', { name: 'Load Demo' })).toBeVisible();
  return requests;
}

test('the Library lists organs with their format, folder and last play', async ({ page }) => {
  await rig(page);
  const meta = page.getByRole('listitem').filter({ hasText: 'Demo' }).locator('.library-meta > span');
  await expect(meta).toHaveText(['GrandOrgue', '/sets', 'Played 3 days ago']);
});

test('an organ that is not loaded can be renamed', async ({ page }) => {
  const requests = await rig(page);
  await page.getByRole('button', { name: 'Demo actions' }).click();
  await page.getByRole('menuitem', { name: 'Rename' }).click();
  await page.getByRole('textbox', { name: 'Organ name' }).fill('Abbey');
  await page.getByRole('button', { name: 'Save', exact: true }).click();
  await expect(page.getByRole('button', { name: 'Load Abbey' })).toBeVisible();
  expect(requests).toEqual(['/api/library/rename?path=/sets/demo.organ&name=Abbey']);
});

test('delete asks first, and cancelling keeps the organ', async ({ page }) => {
  const requests = await rig(page);
  await page.getByRole('button', { name: 'Demo actions' }).click();
  await page.getByRole('menuitem', { name: 'Delete' }).click();
  const dialog = page.getByRole('dialog');
  await expect(dialog).toContainText('Are you sure you want to delete Demo?');
  await dialog.getByRole('button', { name: 'Cancel' }).click();
  expect(requests).toEqual([]);
  await page.getByRole('button', { name: 'Demo actions' }).click();
  await page.getByRole('menuitem', { name: 'Delete' }).click();
  await page.getByRole('dialog').getByRole('button', { name: 'Delete', exact: true }).click();
  await expect(page.getByRole('button', { name: 'Load Demo' })).toHaveCount(0);
  expect(requests).toEqual(['/api/library/delete?path=/sets/demo.organ']);
});

test('the playing organ cannot be deleted and tapping it returns to Play', async ({ page }) => {
  const requests = await rig(page, true);
  await page.getByRole('button', { name: 'Village actions' }).click();
  await expect(page.getByRole('menuitem', { name: /Delete/ })).toBeDisabled();
  await page.keyboard.press('Escape');
  await page.getByRole('button', { name: 'Village, playing' }).click();
  await expect(page.getByRole('button', { name: 'Play', exact: true })).toHaveAttribute('aria-current', 'page');
  expect(requests.filter(url => url.startsWith('/api/organ/load'))).toEqual([]);
});

test('New organ asks for a name and creates a blank organ', async ({ page }) => {
  const requests = await rig(page);
  await page.getByRole('button', { name: 'New organ' }).click();
  await expect(page.getByRole('button', { name: 'Create' })).toBeDisabled();
  await page.getByRole('textbox', { name: 'Organ name' }).fill('Chapel');
  await page.getByRole('button', { name: 'Create' }).click();
  await expect(page.getByRole('button', { name: 'Create' })).toHaveCount(0);
  expect(requests).toEqual(['/api/organ/new?name=Chapel']);
});

test('Load from GrandOrgue/Hauptwerk loads the file chosen in the system picker', async ({ page }) => {
  const requests = await rig(page);
  await page.getByRole('button', { name: 'Load from GrandOrgue/Hauptwerk' }).click();
  await expect.poll(() => requests).toEqual(['/api/organ/load?path=/sets/abbey.organ']);
});
