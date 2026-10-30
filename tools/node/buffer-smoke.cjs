// Requires Playwright and Chrome. Uses a committed TS fixture; no media generator.
// Serve the built player, then NODE_PATH=tools/browser/node_modules node tools/node/buffer-smoke.cjs
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const { chromium, expect } = require('@playwright/test');
const fixture = fs.readFileSync(path.join(__dirname, '../../tests/fixtures/bbb_480p.ts'));
const playlist = '#EXTM3U\n#EXT-X-TARGETDURATION:5\n' +
  Array.from({ length: 20 }, (_, i) => `#EXTINF:4.589889,\nbuffer-smoke-${i}.ts\n`).join('') +
  '#EXT-X-ENDLIST\n';

(async () => {
  const browser = await chromium.launch({ channel: 'chrome' });
  try {
    const results = [];
    for (const fault of ['close-during-append', 'close-during-remove', 'remove-active', 'remove-unrelated']) {
      console.log(`Checking ${fault}`);
      const page = await browser.newPage();
      const errors = [];
      page.on('pageerror', error => errors.push(error.message));
      await page.addInitScript(fault => {
        const owners = new WeakMap();
        const add = MediaSource.prototype.addSourceBuffer;
        const append = SourceBuffer.prototype.appendBuffer;
        const remove = SourceBuffer.prototype.remove;
        window.bufferFault = null;
        MediaSource.prototype.addSourceBuffer = function (mime) {
          const sb = add.call(this, mime);
          owners.set(sb, this);
          if (fault === 'remove-unrelated') this.extra = add.call(this, mime);
          return sb;
        };
        const inject = sb => {
          if (window.bufferFault || !sb.updating) return;
          window.bufferFault = { fault, updating: sb.updating };
          queueMicrotask(() => {
            if (fault === 'remove-active') {
              owners.get(sb).removeSourceBuffer(sb);
            } else if (fault === 'remove-unrelated') {
              const ms = owners.get(sb);
              ms.removeSourceBuffer(ms.extra);
            } else {
              const video = document.querySelector('#video');
              video.removeAttribute('src');
              video.load();
            }
          });
        };
        SourceBuffer.prototype.appendBuffer = function (bytes) {
          const result = append.call(this, bytes);
          if (fault !== 'close-during-remove') inject(this);
          return result;
        };
        SourceBuffer.prototype.remove = function (start, end) {
          const result = remove.call(this, start, end);
          if (fault === 'close-during-remove') inject(this);
          return result;
        };
      }, fault);
      await page.goto(process.env.RSTREAMKIT_URL || 'http://127.0.0.1:8765/player.html?build=full');
      await page.waitForFunction(() => window.ready === true);
      // About 90 seconds keeps loading long enough to trigger actual rewind eviction.
      await page.route('**/buffer-smoke.m3u8', route => route.fulfill({
        status: 200, contentType: 'application/vnd.apple.mpegurl', body: playlist }));
      await page.route('**/buffer-smoke-*.ts', route => route.fulfill({
        status: 200, contentType: 'video/mp2t', body: fixture }));
      await page.locator('#url').fill('/buffer-smoke.m3u8');
      await page.locator('#play').click();
      if (fault === 'close-during-remove' || fault === 'remove-unrelated') {
        await expect.poll(() => page.locator('#video').evaluate(v => v.readyState), { timeout: 15000 }).toBeGreaterThanOrEqual(2);
        await page.locator('#video').evaluate(v => { v.playbackRate = 4; });
      }
      await expect.poll(() => page.evaluate(() => window.bufferFault), { timeout: 20000 }).not.toBeNull();
      await expect.poll(() => page.evaluate(() => window.sample().session.active), { timeout: 30000 }).toBe(false);
      const result = await page.evaluate(() => ({ ...window.bufferFault,
        statuses: window.sample().session.statuses.map(item => item.status) }));
      assert.equal(result.updating, true);
      if (fault === 'remove-unrelated') {
        assert(result.statuses.includes('Ended'), 'unrelated removal must still complete: ' + JSON.stringify(result.statuses));
        assert(!result.statuses.some(s => /Failed|BrowserError/.test(s)));
      } else {
        assert(result.statuses.some(s => s.startsWith('Failed')), 'source closure must settle the task');
      }
      assert.deepEqual(errors, []);
      results.push(result);
      await page.close();
    }
    console.log(JSON.stringify({ browser: await browser.version(), checks: results.length, results }));
  } finally { await browser.close(); }
})().catch(error => { console.error(error); process.exitCode = 1; });
