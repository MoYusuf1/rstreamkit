// NODE_PATH=tools/browser/node_modules node tools/node/retry-smoke.cjs
// Requires Playwright and Chrome; serve the built player with tools/server.py.
const assert = require('node:assert/strict');
const { chromium, expect } = require('@playwright/test');

(async () => {
  const browser = await chromium.launch({ channel: 'chrome' });
  try {
    const page = await browser.newPage();
    const errors = [];
    page.on('pageerror', error => errors.push(error.message));
    await page.addInitScript(() => {
      const set = window.setTimeout.bind(window);
      const clear = window.clearTimeout.bind(window);
      const timers = new Set();
      window.retryTimers = timers;
      window.setTimeout = (callback, delay, ...args) => {
        const id = set(() => {
          timers.delete(id);
          callback(...args);
        }, delay);
        if (delay === 1000) timers.add(id);
        return id;
      };
      window.clearTimeout = id => { timers.delete(id); clear(id); };
    });
    let requests = 0;
    let permanentAfterFirst = false;
    await page.route('**/retry-test.m3u8', route => {
      requests++;
      return route.fulfill({ status: permanentAfterFirst && requests > 1 ? 404 : 503, body: 'offline' });
    });
    await page.goto(process.env.RSTREAMKIT_URL || 'http://127.0.0.1:8765/player.html?build=full');
    await page.waitForFunction(() => window.ready === true);
    await page.locator('#url').fill('/retry-test.m3u8');
    await page.locator('#play').click();
    await expect.poll(() => page.evaluate(() => window.retryTimers.size)).toBe(1);
    assert.equal(requests, 1);
    await page.evaluate(() => window.stop());
    await expect.poll(() => page.evaluate(() => window.retryTimers.size)).toBe(0);
    await page.waitForTimeout(1200);
    assert.equal(requests, 1, 'stopped session must not retry');

    requests = 0;
    permanentAfterFirst = true;
    await page.locator('#play').click();
    await expect.poll(() => page.evaluate(() => window.sample().session.statuses.some(
      item => item.status.startsWith('Failed')))).toBe(true);
    assert.equal(requests, 2, 'retry 503 once, stop at permanent 404');
    assert.equal(await page.evaluate(() => window.retryTimers.size), 0);
    assert.deepEqual(errors, []);
    console.log(JSON.stringify({ browser: await browser.version(), checks: 2,
      pending_retry_timers_before_stop: 1, pending_retry_timers_after_stop: 0,
      requests_after_stop: 0, transient_then_permanent_requests: requests }));
  } finally { await browser.close(); }
})().catch(error => { console.error(error); process.exitCode = 1; });
