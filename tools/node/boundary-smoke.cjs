// Optional real-network acceptance; not part of deterministic CI.
const assert=require('node:assert/strict'),crypto=require('node:crypto');
const {chromium,expect}=require('@playwright/test');
const source=process.env.RSTREAMKIT_BOUNDARY_SOURCE||'https://test-streams.mux.dev/x36xhzz/url_0/193039199_mp4_h264_aac_hd_7.m3u8';
(async()=>{
 const response=await fetch(source);assert(response.ok);
 const playlist=await response.text(),durations=Array.from(playlist.matchAll(/#EXTINF:([\d.]+)/g),m=>Number(m[1]));
 let end=0;const boundaries=durations.map(d=>end+=d).filter(t=>t<120);
 const browser=await chromium.launch({channel:'chrome'});
 try{
  const page=await browser.newPage({viewport:{width:1280,height:900}}),errors=[],segments=[];
  page.on('pageerror',e=>errors.push(e.message));
  page.on('response',response=>{if(new URL(response.url()).pathname.endsWith('.ts'))segments.push({url:response.url(),status:response.status()});});
  await page.goto(new URL('/player.html?build=full',process.env.RSTREAMKIT_URL||'http://127.0.0.1:8765').href);
  await page.waitForFunction(()=>window.ready);
  await page.evaluate(()=>{
   const video=document.querySelector('#video');window.boundaryTrace={frames:[],samples:[],seeks:[]};
   let id;const frame=(now,metadata)=>{boundaryTrace.frames.push({time:metadata.mediaTime,now});id=video.requestVideoFrameCallback(frame);};
   id=video.requestVideoFrameCallback(frame);
   video.addEventListener('seeking',()=>{if(video.currentTime>1)boundaryTrace.seeks.push(video.currentTime);});
   const timer=setInterval(()=>{const q=video.getVideoPlaybackQuality();boundaryTrace.samples.push({
    time:video.currentTime,total:q.totalVideoFrames,dropped:q.droppedVideoFrames,ranges:video.buffered.length,hidden:document.hidden});
   },100);
   window.finishTrace=()=>{clearInterval(timer);video.cancelVideoFrameCallback(id);};
  });
  await page.locator('#url').fill(source);await page.locator('#play').click();
  const started=Date.now();
  await expect.poll(()=>page.locator('#video').evaluate(v=>v.currentTime),{timeout:150000,intervals:[1000]}).toBeGreaterThan(120);
  const elapsed=Date.now()-started;
  const report=await page.evaluate(()=>({sample:sample(),trace:boundaryTrace}));
  const windows=boundaries.map(time=>{
   const samples=report.trace.samples.filter(s=>Math.abs(s.time-time)<=.5);
   assert(samples.length>=2,'boundary '+time+' was sampled');
   const after=report.trace.frames.findIndex(f=>f.time>=time);
   return {time,samples:samples.length,range_counts:[...new Set(samples.map(s=>s.ranges))],
    dropped_delta:samples.at(-1).dropped-samples[0].dropped,
    callback_step:after>0?report.trace.frames[after].time-report.trace.frames[after-1].time:null};
  });
  assert(windows.every(w=>w.range_counts.length===1&&w.range_counts[0]===1),'one contiguous buffered range across boundaries');
  assert(windows.every(w=>w.dropped_delta===0),'no dropped-frame increase around boundaries');
  assert.equal(report.trace.seeks.length,0,'no seek hides a boundary gap');
  assert(!report.trace.samples.some(s=>s.hidden),'measurement tab stayed visible');
  assert(!report.sample.session.statuses.some(s=>/Failed|Unsupported|BrowserError/.test(s.status)));
  assert.equal(report.sample.session.waiting_count,0,'no post-start playback stall');
  assert.deepEqual(errors,[]);
  await page.evaluate(()=>{finishTrace();stop();});
  console.log(JSON.stringify({browser:await browser.version(),source,playlist_sha256:crypto.createHash('sha256').update(playlist).digest('hex'),
   elapsed_ms:elapsed,boundaries:windows,segments,...report,
   limits:['One 720p public CDN VOD source, not a live provider matrix or 4K60 acceptance.',
    'requestVideoFrameCallback cadence can be delayed by JavaScript; dropped-frame checks use getVideoPlaybackQuality.',
    'Network content is external and can change; source URL and playlist hash are recorded.']}));
 }finally{await browser.close();}
})().catch(error=>{console.error(error);process.exitCode=1;});
