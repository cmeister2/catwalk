import { defineConfig } from '@playwright/test';
import { fileURLToPath } from 'node:url';
import { JOB_TIMEOUT_MS } from '../../web/config.js';

const root = fileURLToPath(new URL('../..', import.meta.url));
const executablePath = process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE;

export default defineConfig({
  testDir: '.',
  testMatch: 'uploader.spec.js',
  timeout: JOB_TIMEOUT_MS,
  expect: { timeout: JOB_TIMEOUT_MS },
  fullyParallel: false,
  workers: 1,
  forbidOnly: Boolean(process.env.CI),
  retries: process.env.CI ? 1 : 0,
  outputDir: '../../test-results',
  reporter: process.env.CI ? [['line'], ['html', { open: 'never' }]] : 'list',
  use: {
    baseURL: 'http://127.0.0.1:4173/web/',
    browserName: 'chromium',
    viewport: { width: 1280, height: 960 },
    actionTimeout: 15_000,
    navigationTimeout: 30_000,
    launchOptions: executablePath ? { executablePath } : {},
    screenshot: 'only-on-failure',
    trace: 'retain-on-failure',
  },
  webServer: {
    command: 'python3 -m http.server 4173 --bind 127.0.0.1 --directory target',
    cwd: root,
    url: 'http://127.0.0.1:4173/web/',
    reuseExistingServer: !process.env.CI,
    timeout: 30_000,
    stdout: 'ignore',
    stderr: 'pipe',
  },
});
