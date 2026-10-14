// NODE_PATH=tools/browser/node_modules node tools/node/pause-smoke.cjs
// Real elapsed pause against a provider that removes expired resources.
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const { chromium, expect } = require('@playwright/test');
const fixture = fs.readFileSync(path.join(__dirname, '../../tests/fixtures/bbb_480p.ts'));
const duration = 4.589889;
const windowSeconds = 2 * duration;
(async () => {
  const browser = await chromium.launch({ channel: 'chrome' });
  try {
    const page = await browser.newPage();
    const errors = [];
    const requests = [];
    let epoch;
    let playlistRequests = 0;
    page.on('pageerror', error => errors.push(error.message));
    const first = () => Math.floor((Date.now() - epoch) / (duration * 1000));
    await page.route('**/pause-smoke.m3u8', route => {
      epoch ??= Date.now();
      playlistRequests++;
      const sequence = first();
      const body = `#EXTM3U\n#EXT-X-TARGETDURATION:5\n#EXT-X-MEDIA-SEQUENCE:${sequence}\n` +
        [sequence, sequence + 1].map(i => `#EXTINF:${duration},\npause-smoke-${i}.ts\n`).join('');
      return route.fulfill({ status: 200, contentType: 'application/vnd.apple.mpegurl', body });
    });
    await page.route(/\/pause-smoke-\d+\.ts(?:\?.*)?$/, route => {
      const sequence = Number(route.request().url().match(/pause-smoke-(\d+)\.ts/)[1]);
      const expired = sequence < first();
      requests.push({ sequence, first: first(), expired, probe: route.request().url().includes('?probe=1') });
      return route.fulfill({ status: expired ? 404 : 200,
        contentType: expired ? 'text/plain' : 'video/mp2t', body: expired ? 'evicted' : fixture });
    });
    await page.goto(process.env.RSTREAMKIT_URL || 'http://127.0.0.1:8765/player.html?build=full');
    await page.waitForFunction(() => window.ready === true);
    await page.locator('#url').fill('/pause-smoke.m3u8');
    await page.locator('#play').click();
    await expect.poll(() => page.locator('#video').evaluate(video => video.readyState)).toBeGreaterThanOrEqual(2);
    await page.evaluate(async () => (await import('/player/full/rstreamkit_player.js')).pause());
    const pausedAt = Date.now();
    const position = await page.locator('#video').evaluate(video => video.currentTime);
    await page.waitForTimeout(50000);
    const pausedMs = Date.now() - pausedAt;
    assert(pausedMs >= windowSeconds * 5 * 1000, 'pause exceeds five provider windows');
    assert(await page.locator('#video').evaluate(video => video.paused));
    const expiredStatus = await page.evaluate(async () => (await fetch('/pause-smoke-0.ts?probe=1')).status);
    assert.equal(expiredStatus, 404, 'old resources are actually evicted');
    const beforeResume = playlistRequests;
    const resumedAt = Date.now();
    await page.evaluate(async () => (await import('/player/full/rstreamkit_player.js')).resume());
    await expect.poll(() => playlistRequests, { timeout: 15000 }).toBeGreaterThan(beforeResume);
    await expect.poll(() => page.locator('#video').evaluate(video => video.currentTime), { timeout: 15000 }).toBeGreaterThan(position + 1);
    const recoveredPosition = await page.locator('#video').evaluate(video => video.currentTime);
    await expect.poll(() => page.locator('#video').evaluate(video => video.currentTime), { timeout: 10000 }).toBeGreaterThan(recoveredPosition + 1);
    let report;
    await expect.poll(async () => {
      report = await page.evaluate(() => window.sample());
      return Number(report.playback.stats.match(/latency_seconds: ([\d.]+)/)?.[1] ?? Infinity);
    }, { timeout: 15000 }).toBeLessThanOrEqual(17);
    const resumedMs = Date.now() - resumedAt;
    const statuses = report.session.statuses.map(item => item.status);
    assert(!statuses.some(status => /Failed|Unsupported|BrowserError/.test(status)));
    assert.deepEqual(errors, []);
    await page.evaluate(() => window.stop());
    console.log(JSON.stringify({ browser: await browser.version(), checks: 1, window_seconds: windowSeconds,
      paused_ms: pausedMs, pause_windows: pausedMs / 1000 / windowSeconds, expired_probe_status: expiredStatus,
      resume_to_advance_and_target_ms: resumedMs, playlist_requests: playlistRequests, requests,
      playback: report.playback, statuses, limits: ['Repeated committed media isolates refresh/eviction policy.',
        'Real elapsed time, not a simulated clock; not a provider codec/quality or Edge/Safari test.'] }));
  } finally { await browser.close(); }
})().catch(error => { console.error(error); process.exitCode = 1; });
