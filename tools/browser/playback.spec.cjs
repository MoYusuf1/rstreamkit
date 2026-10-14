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

// The harness exposes track controls without adding exports to the minimal player.
test('mixed master keeps decoded language audio after a late quality switch', async ({ baseURL }, testInfo) => {
  test.skip(testInfo.project.name !== 'chrome', 'This helper launches installed Chrome.');
  test.setTimeout(90000);
  const path = require('node:path');
  const { promisify } = require('node:util');
  const { execFile } = require('node:child_process');
  const { stdout } = await promisify(execFile)(process.execPath, [path.join(__dirname, '../node/rendition-smoke.cjs')], {
    env: { ...process.env, RSTREAMKIT_URL: baseURL, NODE_PATH: process.env.NODE_PATH || path.join(__dirname, 'node_modules') },
    timeout: 85000, maxBuffer: 1024 * 1024,
  });
  await testInfo.attach('rendition-report', { body: stdout, contentType: 'application/json' });
  expect(JSON.parse(stdout).hevc_requests).toBe(0);
});

test('raw Dolby TS plays through EOF with decoded sound', async ({ baseURL }, testInfo) => {
  test.skip(testInfo.project.name !== 'chrome', 'This helper launches installed Chrome.');
  test.setTimeout(45000);
  const path = require('node:path');
  const { promisify } = require('node:util');
  const { execFile } = require('node:child_process');
  const { stdout } = await promisify(execFile)(process.execPath, [path.join(__dirname, '../node/raw-playback-smoke.cjs')], {
    env: { ...process.env, RSTREAMKIT_URL: baseURL, NODE_PATH: process.env.NODE_PATH || path.join(__dirname, 'node_modules') },
    timeout: 40000, maxBuffer: 1024 * 1024,
  });
  await testInfo.attach('raw-playback-report', { body: stdout, contentType: 'application/json' });
  expect(JSON.parse(stdout).audio.max_rms).toBeGreaterThan(0.005);
});

test('raw timestamp restarts preserve all three clips', async ({ baseURL }, testInfo) => {
  test.skip(testInfo.project.name !== 'chrome', 'This helper launches installed Chrome.');
  test.setTimeout(45000);
  const path = require('node:path');
  const { promisify } = require('node:util');
  const { execFile } = require('node:child_process');
  const { stdout } = await promisify(execFile)(process.execPath, [path.join(__dirname, '../node/raw-playback-smoke.cjs')], {
    env: { ...process.env, RSTREAMKIT_URL: baseURL, RSTREAMKIT_REPEAT_RAW: '1',
      NODE_PATH: process.env.NODE_PATH || path.join(__dirname, 'node_modules') },
    timeout: 40000, maxBuffer: 1024 * 1024,
  });
  await testInfo.attach('raw-restart-report', { body: stdout, contentType: 'application/json' });
  const report = JSON.parse(stdout);
  expect(report.raw_quality.total - report.initial_quality.total).toBeGreaterThanOrEqual(149);
});
