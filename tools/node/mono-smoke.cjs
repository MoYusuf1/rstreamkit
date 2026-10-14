// NODE_PATH=tools/browser/node_modules node tools/node/mono-smoke.cjs
// Observe actual decoded sound; completion alone would also pass for silence.
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const { chromium, expect } = require('@playwright/test');
const fixture = fs.readFileSync(path.join(__dirname, '../../tests/fixtures/h264_ac3.ts'));
(async () => {
  const browser = await chromium.launch({ channel: 'chrome' });
  try {
    const page = await browser.newPage();
    const errors = [];
    page.on('pageerror', error => errors.push(error.message));
    await page.route('**/mono-smoke.m3u8', route => route.fulfill({ status: 200,
      contentType: 'application/vnd.apple.mpegurl',
      body: '#EXTM3U\n#EXT-X-TARGETDURATION:4\n#EXTINF:4,\nmono-smoke.ts\n#EXT-X-ENDLIST\n' }));
    await page.route('**/mono-smoke.ts', route => route.fulfill({ status: 200, contentType: 'video/mp2t', body: fixture }));
    await page.goto(process.env.RSTREAMKIT_URL || 'http://127.0.0.1:8765/player.html?build=full');
    await page.waitForFunction(() => window.ready === true);
    await page.evaluate(() => {
      const video = document.querySelector('#video');
      video.muted = false;
      const context = new AudioContext();
      const source = context.createMediaElementSource(video);
      const splitter = context.createChannelSplitter(2);
      const mute = context.createGain(); mute.gain.value = 0; mute.connect(context.destination);
      source.connect(splitter);
      const analyzers = [0, 1].map(channel => {
        const analyzer = context.createAnalyser(); analyzer.fftSize = 256;
        splitter.connect(analyzer, channel); analyzer.connect(mute); return analyzer;
      });
      const values = analyzers.map(() => new Float32Array(256));
      window.audioResult = { samples: 0, max_rms: [0, 0], max_channel_difference: 0 };
      const timer = setInterval(() => {
        analyzers.forEach((analyzer, channel) => analyzer.getFloatTimeDomainData(values[channel]));
        for (let channel = 0; channel < 2; channel++) {
          const rms = Math.sqrt(values[channel].reduce((sum, n) => sum + n*n, 0) / values[channel].length);
          window.audioResult.max_rms[channel] = Math.max(window.audioResult.max_rms[channel], rms);
        }
        for (let i = 0; i < values[0].length; i++) {
          window.audioResult.max_channel_difference = Math.max(window.audioResult.max_channel_difference,
            Math.abs(values[0][i] - values[1][i]));
        }
        window.audioResult.samples++;
      }, 25);
      document.querySelector('#play').addEventListener('click', () => context.resume());
      window.closeAudio = async () => { clearInterval(timer); await context.close(); };
    });
    await page.locator('#url').fill('/mono-smoke.m3u8');
    await page.locator('#play').click();
    await expect.poll(() => page.evaluate(() => window.sample().session?.active), { timeout: 15000 }).toBe(false);
    const audio = await page.evaluate(() => window.audioResult);
    const report = await page.evaluate(() => window.sample());
    await page.evaluate(() => window.closeAudio());
    assert(audio.max_rms.every(value => value > 0.01), 'both channels contain audible-level samples');
    assert(audio.max_channel_difference < 0.00001, 'mono is copied equally into left and right');
    assert(report.session.statuses.some(item => item.status === 'Ended'));
    assert(!report.session.statuses.some(item => /Failed|Unsupported|BrowserError/.test(item.status)));
    assert.deepEqual(errors, []);
    console.log(JSON.stringify({ browser: await browser.version(), checks: 1, audio, playback: report.playback,
      statuses: report.session.statuses, limits: ['Web Audio observes decoded PCM, not physical speakers.',
        'This checks mono AC-3 on Chrome; Edge/Safari and other mono codecs remain separate checks.'] }));
  } finally { await browser.close(); }
})().catch(error => { console.error(error); process.exitCode = 1; });
