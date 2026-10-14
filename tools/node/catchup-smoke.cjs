// Real-time acceptance for #14. This can take four minutes.
const assert=require('node:assert/strict'),fs=require('node:fs'),path=require('node:path');
const {chromium,expect}=require('@playwright/test');
const fixture=fs.readFileSync(path.join(__dirname,'../media/browser0.ts'));
(async()=>{
 const browser=await chromium.launch({channel:'chrome'});
 try {
  const page=await browser.newPage(),errors=[],requests=[];let epoch,blockedAt,releasedAt;
  page.on('pageerror',error=>errors.push(error.message));
  await page.route('**/catchup.m3u8',route=>{
   epoch??=Date.now();const end=5+Math.floor((Date.now()-epoch)/2000),first=Math.max(0,end-59);
   return route.fulfill({contentType:'application/vnd.apple.mpegurl',body:
    '#EXTM3U\n#EXT-X-TARGETDURATION:2\n#EXT-X-MEDIA-SEQUENCE:'+first+'\n'+
    Array.from({length:end-first+1},(_,i)=>'#EXTINF:2,\ncatchup-'+(first+i)+'.ts\n').join('')});
  });
  await page.route(/\/catchup-\d+\.ts$/,async route=>{
   const sequence=Number(route.request().url().match(/catchup-(\d+)\.ts$/)[1]);
   requests.push({sequence,ms:Date.now()-epoch});
   if(sequence===4&&blockedAt==null){blockedAt=Date.now();await new Promise(resolve=>setTimeout(resolve,12500));releasedAt=Date.now();}
   return route.fulfill({contentType:'video/mp2t',body:fixture});
  });
  const address=new URL(process.env.RSTREAMKIT_URL||'http://127.0.0.1:8765/player.html?build=full');
  if(address.pathname==='/'){address.pathname='/player.html';address.search='?build=full';}
  await page.goto(address.href);
  await page.waitForFunction(()=>window.ready);
  await page.evaluate(()=>{
   const video=document.querySelector('#video');window.catchup={samples:[],seeks:[],max_jump:0};
   video.addEventListener('seeking',()=>{if(video.currentTime>1)catchup.seeks.push(video.currentTime);});
   let last;
   const timer=setInterval(()=>{
    const now=performance.now(),time=video.currentTime,rate=video.playbackRate;
    if(last)catchup.max_jump=Math.max(catchup.max_jump,time-last.time-(now-last.now)/1000*Math.max(rate,last.rate));
    last={now,time,rate};const report=sample();
    catchup.samples.push({ms:report.session?.elapsed_ms,time,rate,stats:report.playback.stats});
    if(catchup.samples.length>500)catchup.samples.shift();
   },500);
   window.finishCatchup=()=>clearInterval(timer);
  });
  await page.locator('#url').fill('/catchup.m3u8');await page.locator('#play').click();
  await expect.poll(()=>page.evaluate(()=>sample().session?.waits.some(w=>w.duration_ms>=10000)),{timeout:30000}).toBe(true);
  const resumedAt=Date.now();
  await expect.poll(()=>page.evaluate(()=>catchup.samples.some(s=>s.rate===1.05)),{timeout:10000}).toBe(true);
  try {
   await expect.poll(()=>page.evaluate(()=>{
    const v=document.querySelector('#video'),r=sample();
    const latency=Number(r.playback.stats.match(/latency_seconds: ([\d.]+)/)?.[1]||Infinity);
    return v.currentTime>25&&v.playbackRate===1&&latency<=8.1;
   }),{timeout:240000,intervals:[1000]}).toBe(true);
  }catch(error){console.error(JSON.stringify({requests,report:await page.evaluate(()=>({sample:sample(),catchup}))}));throw error;}
  const recoveryMs=Date.now()-resumedAt;
  const report=await page.evaluate(()=>({sample:sample(),catchup}));
  assert(releasedAt-blockedAt>=12500);
  assert.equal(report.catchup.seeks.length,0,'catch-up uses a small rate change without seeking');
  assert(report.catchup.max_jump<.25,'currentTime advances continuously');
  assert(!report.sample.session.statuses.some(s=>/Failed|Unsupported|BrowserError/.test(s.status)));
  assert.deepEqual(errors,[]);
  await page.evaluate(()=>{finishCatchup();stop();});
  console.log(JSON.stringify({browser:await browser.version(),checks:1,blocked_ms:releasedAt-blockedAt,recovery_ms:recoveryMs,
   target_seconds:6,control_band_seconds:8,requests,...report,
   limits:['Real wall time; the provider keeps two minutes of segments so recovery does not discard content.',
    'Repeated generated 2-second media isolates latency policy; not 4K or long-run endurance.',
    'No seek/currentTime jump is observed; this does not substitute for a visual quality review.']}));
 }finally{await browser.close();}
})().catch(error=>{console.error(error);process.exitCode=1;});
