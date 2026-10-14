// NODE_PATH=tools/browser/node_modules node tools/node/abr-failure-smoke.cjs
// Repeated unusable high-level segments must fall back without ending live playback.
const assert=require('node:assert/strict');
const fs=require('node:fs'),path=require('node:path');
const {chromium,expect}=require('@playwright/test');
const fixture=fs.readFileSync(path.join(__dirname,'../../tests/fixtures/bbb_480p.ts'));
(async()=>{
 const browser=await chromium.launch({channel:'chrome'});
 try{
  const page=await browser.newPage();const errors=[],requests=[];
  page.on('pageerror',error=>errors.push(error.message));
  if(process.env.RSTREAMKIT_PLAYER_DIR){
   for(const [file,contentType] of [['rstreamkit_player.js','application/javascript'],['rstreamkit_player_bg.wasm','application/wasm']]){
    await page.route(`**/player/full/${file}`,route=>route.fulfill({status:200,contentType,
     body:fs.readFileSync(path.join(process.env.RSTREAMKIT_PLAYER_DIR,file))}));
   }
  }

  await page.route('**/failure-master.m3u8',route=>route.fulfill({status:200,contentType:'application/vnd.apple.mpegurl',
   body:'#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=100000\nfailure-low.m3u8\n#EXT-X-STREAM-INF:BANDWIDTH=1000000\nfailure-high.m3u8\n'}));
  await page.route(/\/failure-(low|high)\.m3u8$/,route=>{
   const variant=route.request().url().includes('low')?'low':'high';
   // The failed encoder has stopped at 39; the healthy encoder has published through 42.
   const body='#EXTM3U\n#EXT-X-TARGETDURATION:5\n'+Array.from({length:variant==='low'?43:40},(_,i)=>
    `#EXTINF:4.589889,\nfailure-${variant}-${i}.ts\n`).join('');
   return route.fulfill({status:200,contentType:'application/vnd.apple.mpegurl',body});
  });
  await page.route(/\/failure-(low|high)-\d+\.ts$/,route=>{
   const match=route.request().url().match(/failure-(low|high)-(\d+)\.ts$/);
   const variant=match[1],sequence=Number(match[2]);requests.push({variant,sequence});
   return route.fulfill({status:variant==='high'?404:200,contentType:'video/mp2t',body:variant==='high'?'missing':fixture});
  });
  await page.goto(process.env.RSTREAMKIT_URL||'http://127.0.0.1:8765/player.html?build=full');
  await page.waitForFunction(()=>window.ready===true);
  await page.locator('#url').fill('/failure-master.m3u8');await page.locator('#play').click();
  try {
    await expect.poll(()=>requests.filter(r=>r.variant==='low').length,{timeout:15000}).toBeGreaterThanOrEqual(3);
  } catch (error) {
    console.error(JSON.stringify({requests,report:await page.evaluate(()=>window.sample())}));
    throw error;
  }
  await expect.poll(()=>page.locator('#video').evaluate(video=>video.currentTime),{timeout:15000}).toBeGreaterThan(5.6);
  const report=await page.evaluate(()=>window.sample());
  const statuses=report.session.statuses.map(item=>item.status);
  assert.equal(requests.filter(r=>r.variant==='high').length,2,'two failures trigger fallback, not every high segment');
  assert(requests.slice(-3).every(r=>r.variant==='low'),'healthy lower downloads do not immediately retry the failed level');
  assert(report.playback.stats.includes('failure-low.m3u8'));
  assert(report.playback.stats.includes('dropped_segments: 2'));
  assert(!statuses.some(status=>/Failed|Unsupported|BrowserError/.test(status)));
  assert.deepEqual(errors,[]);await page.evaluate(()=>window.stop());
  console.log(JSON.stringify({browser:await browser.version(),checks:1,requests,playback:report.playback,statuses,
   limits:['Same committed media isolates failure policy; not a resolution/quality test.',
    'This case covers live segment failures; finite input keeps terminal-error behavior.']}));
 }finally{await browser.close();}
})().catch(error=>{console.error(error);process.exitCode=1;});
