const { test, expect } = require('@playwright/test');

// A live channel's first segment plays its first keyframe group while the rest downloads. When the
// download then breaks, the segment is not asked for again (which would append those pictures
// twice): it is skipped, and the next is glued on where the group ended, leaving one buffered range.
test('a first segment that breaks after it started playing is skipped, not appended twice', async ({ page }) => {
  const errors = [];
  page.on('pageerror', error => errors.push(error.message));
  const first = [];
  page.on('request', request => { if (request.url().includes('groups0.ts')) first.push(request.url()); });
  await page.route('**/media/groups.m3u8', async route => {
    const response = await route.fetch();
    const body = (await response.text()).replace('#EXT-X-ENDLIST', '').replace('groups0.ts', 'groups0.ts?cut=0.6');
    await route.fulfill({ response, body });
  });
  await page.goto('/player.html?build=full');
  await page.waitForFunction(() => window.ready === true);
  await page.evaluate(() => { window.start('/media/groups.m3u8', 'live'); });
  await expect.poll(() => page.evaluate(() => window.sample().session.statuses.some(
    s => s.status.includes('skipping unavailable segment') && s.status.includes('part way')))).toBe(true);
  // Past the first group, into the segment glued on after it.
  await expect.poll(() => page.locator('#video').evaluate(video => video.currentTime)).toBeGreaterThan(3);
  const ranges = await page.locator('#video').evaluate(video =>
    Array.from({ length: video.buffered.length }, (_, i) => [video.buffered.start(i), video.buffered.end(i)]));
  await page.evaluate(() => window.stop());
  const statuses = (await page.evaluate(() => window.sample())).session.statuses.map(s => s.status);
  expect(first).toHaveLength(1);
  expect(ranges).toHaveLength(1);
  // One 2 s group, then two whole 6 s segments: nothing appended twice, no hole.
  expect(ranges[0][1] - ranges[0][0]).toBeGreaterThan(13);
  expect(ranges[0][1] - ranges[0][0]).toBeLessThan(15);
  expect(statuses.some(s => /Failed|BrowserError/.test(s))).toBe(false);
  expect(errors).toEqual([]);
});
