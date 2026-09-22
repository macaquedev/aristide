import { defineConfig } from '@playwright/test';

const port = process.env.ARISTIDE_TEST_PORT ?? '1420';
const baseURL = `http://127.0.0.1:${port}`;

export default defineConfig({
  testDir: './tests',
  fullyParallel: true,
  use: {
    baseURL,
    viewport: { width: 1280, height: 800 },
    launchOptions: process.env.ARISTIDE_TEST_BROWSER ? { executablePath: process.env.ARISTIDE_TEST_BROWSER } : {},
  },
  webServer: { command: `bun run dev --port ${port}`, url: baseURL, reuseExistingServer: !process.env.CI },
});
