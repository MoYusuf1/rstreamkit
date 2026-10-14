// NODE_PATH=tools/browser/node_modules node tools/node/abr-smoke.cjs
// Browser policy acceptance with a delayed provider; no new runtime code.
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const { chromium, expect } = require('@playwright/test');
const fixture = fs.readFileSync(path.join(__dirname, '../../tests/fixtures/bbb_480p.ts'));
const master = '#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=100000\nabr-low.m3u8\n' +
  '#EXT-X-STREAM-INF:BANDWIDTH=1000000\nabr-high.m3u8\n';
const playlist = variant => '#EXTM3U\n#EXT-X-TARGETDURATION:5\n#EXT-X-MEDIA-SEQUENCE:0\n' +
  Array.from({ length: 40 }, (_, i) => `#EXTINF:4.589889,\nabr-${variant}-${i}.ts\n`).join('') +
  '#EXT-X-ENDLIST\n';

(async () => {
  const browser = await chromium.launch({ channel: 'chrome' });
  try {
    const page = await browser.newPage();
    const errors = [];
    const requests = [];
    let restored = false;
    page.on('pageerror', error => errors.push(error.message));
    await page.route('**/abr-master.m3u8', route => route.fulfill({ status: 200,
      contentType: 'application/vnd.apple.mpegurl', body: master }));
    await page.route(/\/abr-(low|high)\.m3u8$/, route => route.fulfill({ status: 200,
      contentType: 'application/vnd.apple.mpegurl', body: playlist(route.request().url().includes('low') ? 'low' : 'high') }));
    await page.route(/\/abr-(low|high)-\d+\.ts$/, async route => {
      const match = route.request().url().match(/abr-(low|high)-(\d+)\.ts$/);
      const request = { variant: match[1], sequence: Number(match[2]), restored,
        requested_at: Date.now(), delay_ms: restored ? 100 : 2000 };
      requests.push(request);
      await new Promise(resolve => setTimeout(resolve, request.delay_ms));
      try {
        await route.fulfill({ status: 200, contentType: 'video/mp2t', body: fixture });
        request.delivered_ms = Date.now() - request.requested_at;
      } catch { request.cancelled = true; }
    });
    await page.goto(process.env.RSTREAMKIT_URL || 'http://127.0.0.1:8765/player.html?build=full');
    await page.waitForFunction(() => window.ready === true);
    await page.locator('#url').fill('/abr-master.m3u8');
    await page.locator('#play').click();
    await expect.poll(() => requests.filter(r => r.variant === 'low' && !r.restored && r.delivered_ms != null).length,
      { timeout: 20000 }).toBeGreaterThanOrEqual(3);
    assert.equal(requests[0].variant, 'high', 'starts at the selected higher variant');
    assert.equal(requests.filter(r => !r.restored && r.variant === 'high' && r.delivered_ms != null).length, 1,
      'steps down after the first slow segment and stays down');
    restored = true;
    await expect.poll(() => requests.filter(r => r.variant === 'high' && r.restored && r.delivered_ms != null).length,
      { timeout: 25000 }).toBeGreaterThanOrEqual(3);
    const last = requests.filter(r => r.delivered_ms != null).slice(-3);
    assert(last.every(r => r.variant === 'high' && r.restored), 'stays high after recovery');
    const position = await page.locator('#video').evaluate(video => video.currentTime);
    await expect.poll(() => page.locator('#video').evaluate(video => video.currentTime)).toBeGreaterThan(position + 1);
    const report = await page.evaluate(() => window.sample());
    assert(report.playback.stats.includes('abr-high.m3u8'), 'stats report the recovered variant');
    await page.evaluate(() => window.stop());
    const statuses = report.session.statuses.map(item => item.status);
    assert(!statuses.some(status => /Failed|Unsupported|BrowserError/.test(status)));
    assert.deepEqual(errors, []);
    assert(statuses.some(status => status.includes('switching variant to 100000')));
    assert(statuses.some(status => status.includes('switching variant to 1000000')));
    console.log(JSON.stringify({ browser: await browser.version(), checks: 1, fixture_bytes: fixture.length,
      slow_delay_ms: 2000, restored_delay_ms: 100, requests, playback: report.playback, statuses,
      limits: ['Same compressed media at both levels isolates switching policy; not a quality or resolution test.',
        'Delayed complete bodies model throughput, not packet loss or a shared network bandwidth cap.',
        'Does not establish 4K60, frame-drop or Edge/Safari acceptance.'] }));
  } finally { await browser.close(); }
})().catch(error => { console.error(error); process.exitCode = 1; });
