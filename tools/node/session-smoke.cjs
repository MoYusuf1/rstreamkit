// NODE_PATH=tools/browser/node_modules node tools/node/session-smoke.cjs
// Requires Playwright, Chrome and a built/served full player. No generated media.
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const { chromium, expect } = require('@playwright/test');
const fixture = fs.readFileSync(path.join(__dirname, '../../tests/fixtures/bbb_480p.ts'));
const playlist = '#EXTM3U\n#EXT-X-TARGETDURATION:5\n' +
  Array.from({ length: 4 }, (_, i) => `#EXTINF:4.589889,\nsession-smoke-${i}.ts\n`).join('');

(async () => {
  const browser = await chromium.launch({ channel: 'chrome' });
  try {
    const page = await browser.newPage();
    const errors = [];
    page.on('pageerror', error => errors.push(error.message));
    await page.addInitScript(() => {
      const set = window.setTimeout.bind(window);
      const clear = window.clearTimeout.bind(window);
      window.pollWaits = 0;
      window.pollTimers = new Set();
      window.setTimeout = (callback, delay, ...args) => {
        const id = set(() => { window.pollTimers.delete(id); callback(...args); }, delay);
        if (delay === 2500) { window.pollWaits++; window.pollTimers.add(id); }
        return id;
      };
      window.clearTimeout = id => { window.pollTimers.delete(id); clear(id); };
    });
    let requests = 0;
    await page.route('**/session-smoke.m3u8', route => {
      requests++;
      return route.fulfill({ status: 200, contentType: 'application/vnd.apple.mpegurl', body: playlist });
    });
    await page.route('**/session-smoke-*.ts', route => route.fulfill({ status: 200, contentType: 'video/mp2t', body: fixture }));
    await page.goto(process.env.RSTREAMKIT_URL || 'http://127.0.0.1:8765/player.html?build=full');
    await page.waitForFunction(() => window.ready === true);
    await page.locator('#url').fill('/session-smoke.m3u8');
    await page.locator('#play').click();
    await expect.poll(() => page.evaluate(() => window.pollWaits), { timeout: 15000 }).toBe(1);
    assert.equal(requests, 1);
    await page.evaluate(async () => {
      const kit = await import('/player/full/rstreamkit_player.js');
      kit.go_live();
    });
    await expect.poll(() => page.evaluate(() => window.pollWaits), { timeout: 15000 }).toBe(2);
    assert.equal(requests, 2, 'initial playlist plus one refresh; no duplicate go-live fetch');
    await page.evaluate(() => window.stop());
    await expect.poll(() => page.evaluate(() => window.pollTimers.size)).toBe(0);
    assert.deepEqual(errors, []);
    const statuses = await page.evaluate(() => window.sample().session.statuses.map(item => item.status));
    assert(!statuses.some(s => /Failed|Unsupported|BrowserError/.test(s)));
    console.log(JSON.stringify({ browser: await browser.version(), checks: 1,
      playlist_requests: requests, go_live_refreshes: requests - 1, pending_poll_timers_after_stop: 0, statuses }));
  } finally { await browser.close(); }
})().catch(error => { console.error(error); process.exitCode = 1; });
