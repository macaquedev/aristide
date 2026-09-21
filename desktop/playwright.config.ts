import { defineConfig } from '@playwright/test';

export default defineConfig({
  testDir: './tests',
  fullyParallel: true,
  use: {
    baseURL: 'http://127.0.0.1:1420',
    viewport: { width: 1280, height: 800 },
    launchOptions: process.env.ARISTIDE_TEST_BROWSER ? { executablePath: process.env.ARISTIDE_TEST_BROWSER } : {},
  },
  webServer: { command: 'bun run dev', url: 'http://127.0.0.1:1420', reuseExistingServer: !process.env.CI },
});
