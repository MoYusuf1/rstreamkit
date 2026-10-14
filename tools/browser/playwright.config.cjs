const { defineConfig } = require('@playwright/test');
const path = require('node:path');
module.exports = defineConfig({
  testDir: '.',
  testMatch: '*.spec.cjs',
  workers: 1,
  retries: 0,
  timeout: 45_000,
  expect: { timeout: 35_000 },
  outputDir: '../results/playwright',
  reporter: [['list'], ['json', { outputFile: '../results/playwright.json' }]],
  use: { baseURL: 'http://127.0.0.1:8765', trace: 'retain-on-failure' },
  projects: [
    { name: 'chrome', use: { browserName: 'chromium', channel: 'chrome' } },
    { name: 'edge', use: { browserName: 'chromium', channel: 'msedge' } },
    { name: 'webkit', use: { browserName: 'webkit' } },
  ],
  webServer: {
    command: 'python3 tools/server.py',
    cwd: path.resolve(__dirname, '../..'),
    url: 'http://127.0.0.1:8765/player.html',
    reuseExistingServer: !process.env.CI,
  },
});
