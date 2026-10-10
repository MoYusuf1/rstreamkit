const { test, expect } = require('@playwright/test');

async function open(page) {
  const errors = [];
  page.on('pageerror', error => errors.push(error.message));
  await page.goto('/player.html?build=full');
  await page.waitForFunction(() => window.ready === true);
  return errors;
}

async function play(page, url, rate = 1) {
  await page.locator('#url').fill(url);
  await page.locator('#play').click();
  await expect.poll(() => page.locator('#video').evaluate(video => video.readyState)).toBeGreaterThanOrEqual(2);
  await page.locator('#video').evaluate((video, speed) => { video.playbackRate = speed; }, rate);
}

async function completed(page, testInfo) {
  let report;
  try {
    await expect.poll(() => page.evaluate(() => window.sample().session?.active)).toBe(false);
  } finally {
    report = await page.evaluate(() => window.sample());
    await testInfo.attach('playback-report', {
      body: JSON.stringify(report, null, 2), contentType: 'application/json',
    });
  }
  expect(report.session.statuses.some(s => /Failed|Unsupported|BrowserError/.test(s.status))).toBe(false);
  expect(report.session.statuses.some(s => s.status === 'Ended')).toBe(true);
  expect(report.playback.width).toBeGreaterThan(0);
  return report;
}

test('finite HLS reaches the last segment after buffer throttling', async ({ page }, testInfo) => {
  const errors = await open(page);
  await play(page, '/media/browser.m3u8', 4);
  const report = await completed(page, testInfo);
  expect(report.playback.current_time).toBeGreaterThan(39);
  expect(errors).toEqual([]);
});

test('Dolby fallback decodes and completes in the browser', async ({ page }, testInfo) => {
  const errors = await open(page);
  await play(page, '/media/browser-dolby.m3u8');
  const report = await completed(page, testInfo);
  expect(report.playback.current_time).toBeGreaterThan(1.9);
  expect(errors).toEqual([]);
});

test('a missing finite segment reports failure instead of silent completion', async ({ page }, testInfo) => {
  const errors = await open(page);
  let failures = 0;
  await page.route('**/media/browser5.ts', route => {
    failures++;
    return route.fulfill({ status: 404, body: 'missing test segment' });
  });
  await play(page, '/media/browser.m3u8', 4);
  await expect.poll(() => page.evaluate(() => window.sample().session?.active)).toBe(false);
  const report = await page.evaluate(() => window.sample());
  await testInfo.attach('playback-report', { body: JSON.stringify(report), contentType: 'application/json' });
  expect(failures).toBeGreaterThan(0);
  expect(report.session.statuses.some(s => s.status.includes('Failed("HTTP 404")'))).toBe(true);
  expect(errors).toEqual([]);
});

test('stopping cancels an unfinished fetch before a new session', async ({ page }, testInfo) => {
  const errors = await open(page);
  let release;
  let requested;
  const seen = new Promise(resolve => { requested = resolve; });
  const blocked = new Promise(resolve => { release = resolve; });
  await page.route('**/media/browser.m3u8', async route => {
    requested();
    await blocked;
    await route.abort().catch(() => {});
  });
  try {
    await page.locator('#url').fill('/media/browser.m3u8');
    await page.locator('#play').click();
    await seen;
    await page.locator('#stop').click();
    release();
    await page.unroute('**/media/browser.m3u8');
    await play(page, '/media/browser-dolby.m3u8');
    await completed(page, testInfo);
    expect(errors).toEqual([]);
  } finally { release(); }
});

test('live playback skips an unavailable segment and keeps moving', async ({ page }, testInfo) => {
  const errors = await open(page);
  await page.route('**/media/browser.m3u8', async route => {
    const response = await route.fetch();
    await route.fulfill({ response, body: (await response.text()).replace('#EXT-X-ENDLIST', '') });
  });
  await page.route('**/media/browser18.ts', route => route.fulfill({ status: 404, body: 'missing' }));
  await play(page, '/media/browser.m3u8');
  await expect.poll(() => page.evaluate(() => window.sample().session.statuses.some(
    s => s.status.includes('skipping unavailable segment')))).toBe(true);
  const start = await page.locator('#video').evaluate(video => video.currentTime);
  await expect.poll(() => page.locator('#video').evaluate(video => video.currentTime)).toBeGreaterThan(start + 1);
  await page.locator('#stop').click();
  const report = await page.evaluate(() => window.sample());
  await testInfo.attach('playback-report', { body: JSON.stringify(report), contentType: 'application/json' });
  expect(report.session.statuses.some(s => /Failed|BrowserError/.test(s.status))).toBe(false);
  expect(errors).toEqual([]);
});
