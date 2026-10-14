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
    await page.evaluate(async () => {
      const video = document.querySelector('#video');
      video.muted = false;
      const context = new AudioContext();
      // Separate analyzer snapshots can straddle audio render blocks. Compare both
      // channels inside the same AudioWorklet block instead.
      const module = URL.createObjectURL(new Blob([`
        registerProcessor('mono-observer', class extends AudioWorkletProcessor {
          constructor() { super(); this.blocks=0; this.rms=[0,0]; this.difference=0; }
          process(inputs) {
            const channels=inputs[0];
            if (channels.length < 2) return true;
            const [left,right]=channels;
            let a=0,b=0;
            for (let i=0;i<left.length;i++) {
              a+=left[i]*left[i]; b+=right[i]*right[i];
              this.difference=Math.max(this.difference,Math.abs(left[i]-right[i]));
            }
            this.rms[0]=Math.max(this.rms[0],Math.sqrt(a/left.length));
            this.rms[1]=Math.max(this.rms[1],Math.sqrt(b/right.length));
            if (++this.blocks % 32 === 0) this.port.postMessage({
              blocks:this.blocks,max_rms:this.rms,max_channel_difference:this.difference
            });
            // Outputs remain silent; only input PCM is observed.
            return true;
          }
        });
      `], {type:'application/javascript'}));
      try { await context.audioWorklet.addModule(module); } finally { URL.revokeObjectURL(module); }
      const observer = new AudioWorkletNode(context, 'mono-observer');
      window.audioResult = { blocks: 0, max_rms: [0, 0], max_channel_difference: 0 };
      observer.port.onmessage = event => { window.audioResult = event.data; };
      context.createMediaElementSource(video).connect(observer);
      observer.connect(context.destination);
      document.querySelector('#play').addEventListener('click', () => context.resume());
      window.closeAudio = async () => { await context.close(); };
    });
    await page.locator('#url').fill('/mono-smoke.m3u8');
    await page.locator('#play').click();
    await expect.poll(() => page.evaluate(() => window.sample().session?.active), { timeout: 15000 }).toBe(false);
    const audio = await page.evaluate(() => window.audioResult);
    const report = await page.evaluate(() => window.sample());
    await page.evaluate(() => window.closeAudio());
    assert(audio.blocks >= 32, 'audio render blocks were observed');
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
