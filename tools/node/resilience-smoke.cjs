// NODE_PATH=tools/browser/node_modules node tools/node/resilience-smoke.cjs
// The three provider failures required by issue #2, plus a server that never answers, using actual
// browser time.
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const { chromium, expect } = require('@playwright/test');
const fixture = fs.readFileSync(path.join(__dirname, '../../tests/fixtures/bbb_480p.ts'));
const playlist = '#EXTM3U\n#EXT-X-TARGETDURATION:5\n' +
  Array.from({ length: 6 }, (_, i) => `#EXTINF:4.589889,\nresilience-${i}.ts\n`).join('');
(async () => {
  const browser = await chromium.launch({ channel: 'chrome' });
  const results = [];
  try {
    for (const fault of ['missing', 'garbage', 'outage', 'silent']) {
      const page = await browser.newPage();
      try {
        const errors = [];
        const requests = [];
        let outageAt;
        page.on('pageerror', error => errors.push(error.message));
        await page.route('**/resilience.m3u8', route => route.fulfill({ status: 200,
          contentType: 'application/vnd.apple.mpegurl', body: playlist }));
        await page.route(/\/resilience-\d+\.ts$/, route => {
          const sequence = Number(route.request().url().match(/resilience-(\d+)\.ts/)[1]);
          let status = 200, body = fixture;
          if (sequence === 4 && fault === 'missing') { status = 404; body = 'missing'; }
          if (sequence === 4 && fault === 'garbage') body = '<html>provider error</html>';
          if (sequence === 3 && fault === 'outage') {
            outageAt ??= Date.now();
            if (Date.now() - outageAt < 10000) { status = 503; body = 'outage'; }
          }
          // Accept the first request for segment 3 and never answer it.
          if (sequence === 3 && fault === 'silent' && !outageAt) {
            outageAt = Date.now();
            requests.push({ sequence, status: null, elapsed_outage_ms: 0 });
            return;
          }
          requests.push({ sequence, status, elapsed_outage_ms: outageAt == null ? null : Date.now() - outageAt });
          return route.fulfill({ status, contentType: status === 200 ? 'video/mp2t' : 'text/plain', body });
        });
        await page.goto(process.env.RSTREAMKIT_URL || 'http://127.0.0.1:8765/player.html?build=full');
        await page.waitForFunction(() => window.ready === true);
        await page.locator('#url').fill('/resilience.m3u8');
        await page.locator('#play').click();
        await expect.poll(() => page.locator('#video').evaluate(video => video.readyState), { timeout: fault === 'silent' ? 60000 : 30000 }).toBeGreaterThanOrEqual(2);
        if (fault === 'missing' || fault === 'garbage') {
          await expect.poll(() => page.evaluate(() => window.sample().session.statuses.some(
            item => item.status.includes('skipping unavailable segment')))).toBe(true);
        }
        if (fault === 'silent') {
          await expect.poll(() => requests.filter(request => request.sequence === 3).length, { timeout: 30000 }).toBe(2);
        }
        const position = await page.locator('#video').evaluate(video => video.currentTime);
        await expect.poll(() => page.locator('#video').evaluate(video => video.currentTime)).toBeGreaterThan(position + 1);
        if (fault === 'missing' || fault === 'garbage') {
          await expect.poll(() => page.locator('#video').evaluate(video => video.currentTime), { timeout: 12000 }).toBeGreaterThan(5.6);
        }
        const report = await page.evaluate(() => window.sample());
        const statuses = report.session.statuses.map(item => item.status);
        assert(!statuses.some(status => /Failed|Unsupported|BrowserError/.test(status)));
        assert.deepEqual(errors, []);
        if (fault === 'outage') {
          const attempts = requests.filter(request => request.sequence === 3);
          assert.equal(attempts.length, 5, 'bounded retry schedule succeeds at the fifth attempt');
          assert(attempts.slice(0, 4).every(request => request.status === 503));
          assert(attempts[4].status === 200 && attempts[4].elapsed_outage_ms >= 10000);
          assert(statuses.some(status => status.includes('Reconnecting')));
        } else if (fault === 'silent') {
          const attempts = requests.filter(request => request.sequence === 3);
          assert(attempts[1].status === 200 && attempts[1].elapsed_outage_ms >= 20000, 'unanswered request times out at 20 s, then retries');
          assert(statuses.some(status => status.includes('Reconnecting')));
        } else {
          assert.equal(requests.filter(request => request.sequence === 4).length, 1, 'bad segment is skipped without retrying');
          assert(requests.some(request => request.sequence === 5 && request.status === 200));
        }
        await page.evaluate(() => window.stop());
        results.push({ fault, requests, playback: report.playback, statuses });
      } finally { await page.close(); }
    }
    console.log(JSON.stringify({ browser: await browser.version(), checks: results.length, results,
      limits: ['Repeated committed media isolates provider-failure policy.',
        'The outage uses HTTP 503 responses for ten real seconds; not a physical network disconnect.',
        'The silent server is a request Playwright never fulfils; the player aborts it after 20 s.',
        'Not a 4K60, Edge/Safari or long-run soak test.'] }));
  } finally { await browser.close(); }
})().catch(error => { console.error(error); process.exitCode = 1; });
