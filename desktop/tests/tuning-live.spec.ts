import { test, expect } from '@playwright/test';

// Runs against a real aristide-server with an organ loaded, e.g.
// ARISTIDE_LIVE=1 ARISTIDE_SCALES=/path/with/bohlen-pierce.scl bunx playwright test tuning-live
test.skip(!process.env.ARISTIDE_LIVE, 'needs a running engine');

test('the connected Tuning panel edits the engine', async ({ page }) => {
  const choose = async (label: string, value: string) => {
    await page.getByRole('combobox', { name: label, exact: true }).click();
    await page.getByRole('option', { name: value, exact: true }).click();
  };
  await page.goto('/');
  await page.getByRole('button', { name: 'Tuning', exact: true }).click();
  await expect(page.getByRole('combobox', { name: 'Temperament', exact: true })).toBeDisabled();
  await page.getByRole('button', { name: 'Perform', exact: true }).click();

  await choose('Temperament', 'Quarter-comma meantone');
  await expect(page.getByText('Wolf G♯–E♭ · 737.6 ¢', { exact: true })).toBeVisible();
  await page.getByRole('button', { name: '415', exact: true }).click();
  await expect(page.getByRole('button', { name: 'Reference pitch: 415 Hz', exact: true })).toBeVisible();
  await page.screenshot({ path: 'test-results/live-instrument.png', fullPage: true });

  const division = page.getByRole('navigation', { name: 'Tuning scopes' }).getByRole('button').filter({ hasText: /Manual|Pedal|Great|Positif|Récit/ }).nth(1);
  const name = (await division.getAttribute('aria-label'))!;
  await division.click();
  await expect(page.getByRole('switch', { name: 'Scale follows Whole instrument' })).toBeChecked();
  await page.getByRole('switch', { name: 'Scale follows Whole instrument' }).click();
  await expect(page.getByRole('switch', { name: 'Scale follows Whole instrument' })).not.toBeChecked();
  await choose('Temperament', 'Vallotti');
  await expect(page.getByRole('button', { name, exact: true })).toContainText('415 Hz · Vallotti');
  await page.getByRole('button', { name: 'Whole instrument', exact: true }).click();
  await page.getByRole('button', { name: '440', exact: true }).click();
  await expect(page.getByRole('button', { name, exact: true })).toContainText('440 Hz · Vallotti');

  await page.getByRole('button', { name, exact: true }).click();
  await page.getByRole('button', { name: 'Import Scala', exact: true }).click();
  const sheet = page.getByRole('dialog', { name: 'Import Scala' });
  await sheet.getByRole('textbox', { name: 'Folder' }).fill(process.env.ARISTIDE_SCALES ?? '');
  await sheet.getByRole('button', { name: 'Go', exact: true }).click();
  await sheet.getByRole('button', { name: 'bohlen-pierce.scl', exact: true }).click();
  await sheet.getByRole('button', { name: 'Use scale', exact: true }).click();
  await expect(page.getByText(/^13 steps · Repeat 1901\.9[56] ¢$/).first()).toBeVisible();
  await expect(page.getByRole('button', { name, exact: true })).toContainText('Bohlen-Pierce');
  await page.screenshot({ path: 'test-results/live-scala.png', fullPage: true });

  await page.getByRole('button', { name: 'Undo', exact: true }).click();
  await expect(page.getByRole('button', { name, exact: true })).toContainText('Vallotti');
  await page.getByRole('switch', { name: 'Scale follows Whole instrument' }).click();
  await expect(page.getByRole('switch', { name: 'Scale follows Whole instrument' })).toBeChecked();
  await expect(page.getByRole('button', { name, exact: true })).toContainText('Quarter-comma meantone');
  await page.getByRole('button', { name: `Expand ${name}`, exact: true }).click();
  await expect(page.getByRole('button', { name: "Expand Montre 8'", exact: true })).toHaveCount(0);
  await page.getByRole('button', { name: 'Expand Plein jeu III', exact: true }).click();
  const rank = page.getByRole('button', { name: 'Plein jeu 2nd rank', exact: true });
  await rank.click();
  await expect(page.getByRole('switch', { name: 'Scale follows Plein jeu III' })).toBeChecked();
  await page.getByRole('switch', { name: 'Scale follows Plein jeu III' }).click();
  await expect(page.getByRole('switch', { name: 'Scale follows Plein jeu III' })).not.toBeChecked();
  await choose('Temperament', 'Pythagorean');
  await expect(rank).toContainText('Pythagorean');
  await expect(page.getByRole('button', { name: 'Plein jeu 1st rank', exact: true })).toContainText('Quarter-comma meantone');
  await expect(page.getByRole('button', { name: 'Plein jeu III', exact: true })).toContainText('Quarter-comma meantone');
  await page.screenshot({ path: 'test-results/live-rank.png', fullPage: true });

  await page.getByRole('button', { name: 'Whole instrument', exact: true }).click();
  await choose('Temperament', 'As recorded');
  await page.getByRole('button', { name: 'Edit', exact: true }).click();
  await expect(page.getByRole('combobox', { name: 'Temperament', exact: true })).toBeDisabled();
});
