// Browser output-equivalence and cooperative scheduling probe for raw TS.
const assert=require('node:assert/strict'),fs=require('node:fs'),path=require('node:path');
const {chromium,expect}=require('@playwright/test');
const crypto=require('node:crypto'),os=require('node:os');
const sha=file=>crypto.createHash('sha256').update(fs.readFileSync(file)).digest('hex');
(async()=>{
 const browser=await chromium.launch({channel:'chrome'});
 try{
  const page=await browser.newPage(),errors=[];page.on('pageerror',e=>errors.push(e.message));
  await page.route('**/raw-yield-test.html',route=>route.fulfill({contentType:'text/html',body:
   '<video id="video" muted playsinline></video><script type="module">import * as kit from "/pkg/rstreamkit_harness.js";await kit.default();window.kit=kit;window.statuses=[];window.onStatus=s=>statuses.push(s);window.ready=true;</script>'}));
  await page.route('**/raw-fixture/*',route=>route.fulfill({contentType:'video/mp2t',body:
   fs.readFileSync(path.join(__dirname,'../../tests/fixtures',new URL(route.request().url()).pathname.split('/').pop()))}));
  const origin=new URL(process.env.RSTREAMKIT_URL||'http://127.0.0.1:8765').origin;
  await page.goto(origin+'/raw-yield-test.html');await page.waitForFunction(()=>window.ready);
  const equivalence=[];
  for(const name of ['bbb_480p.ts','h264_ac3.ts','h264_ac3_51.ts','h264_eac3_51.ts','h264_mp2.ts']){
   const checks=await page.evaluate(async name=>{
    const bytes=new Uint8Array(await(await fetch('/raw-fixture/'+name)).arrayBuffer()),checks=[];
    for(const chunk of [187,16384,bytes.length]){
     const baseline=kit.continuous_sync(bytes,chunk),candidate=await kit.continuous_yielded(bytes,chunk);
     if(baseline!==candidate)throw Error(name+': output differs at chunk '+chunk);
     checks.push({chunk,output:baseline});
    }
    return checks;
   },name);
   equivalence.push({name,checks});
  }
  await page.evaluate(async()=>{
   const source=new Uint8Array(await(await fetch('/raw-fixture/h264_ac3_51.ts')).arrayBuffer());
   window.inputs={};
   for(const repetitions of [3,10]){
    const bytes=new Uint8Array(source.length*repetitions);
    for(let i=0;i<repetitions;i++)bytes.set(source,i*source.length);
    inputs[repetitions]=bytes;
   }
   window.tasks=[];new PerformanceObserver(list=>tasks.push(...list.getEntries().map(e=>({start:e.startTime,duration:e.duration})))).observe({type:'longtask'});
   kit.live(new URL('/media/browser.m3u8',location).href);
  });
  await expect.poll(()=>page.locator('#video').evaluate(v=>v.currentTime),{timeout:15000}).toBeGreaterThan(1);
  const measurements=[];
  for(const repetitions of [3,10]){
   // Warm both methods before alternating the measured calls.
   await page.evaluate(async repetitions=>{
    for(let i=0;i<2;i++){kit.continuous_sync(inputs[repetitions],inputs[repetitions].length);await kit.continuous_yielded(inputs[repetitions],inputs[repetitions].length);}
   },repetitions);
   for(let pair=0;pair<5;pair++)for(const method of pair%2===0?['sync','yielded']:['yielded','sync']){
    const result=await page.evaluate(async({repetitions,method})=>{
     await new Promise(resolve=>setTimeout(resolve,50));
     const v=document.querySelector('#video'),quality=v.getVideoPlaybackQuality();
     let beats=0;const timer=setInterval(()=>beats++,1);
     const start=performance.now();
     const output=await kit['continuous_'+method](inputs[repetitions],inputs[repetitions].length);
     const end=performance.now();clearInterval(timer);
     await new Promise(resolve=>setTimeout(resolve,50));
     const after=v.getVideoPlaybackQuality();
     return {elapsed_ms:end-start,beats,output,longtasks:tasks.filter(t=>t.start>=start-1&&t.start<end),
      frames:after.totalVideoFrames-quality.totalVideoFrames,dropped:after.droppedVideoFrames-quality.droppedVideoFrames};
    },{repetitions,method});
    measurements.push({repetitions,method,pair,...result});
   }
  }
  for(const repetitions of [3,10]){
   const outputs=measurements.filter(r=>r.repetitions===repetitions).map(r=>r.output);
   assert.equal(new Set(outputs).size,1,'stress output bytes match');
   assert(measurements.filter(r=>r.repetitions===repetitions&&r.method==='yielded').every(r=>r.beats>0),'audio decoding yields to the event loop');
  }
  const playback=await page.evaluate(()=>({statuses,time:document.querySelector('#video').currentTime,stats:kit.stats()}));
  assert(!playback.statuses.some(s=>/Failed|Unsupported|BrowserError/.test(s)));assert.deepEqual(errors,[]);
  await page.evaluate(()=>kit.stop());
  console.log(JSON.stringify({browser:await browser.version(),cpu:os.cpus()[0].model,node:process.version,
   harness_wasm_sha256:sha(path.join(__dirname,'../web/pkg/rstreamkit_harness_bg.wasm')),
   inputs:equivalence.map(r=>({name:r.name,sha256:sha(path.join(__dirname,'../../tests/fixtures',r.name))})),
   equivalence,measurements,playback,
   limits:['The 3x/10x repeated two-second Dolby input is a 6/20-second decode stress case, not an ordinary network chunk.',
    'Background 320x180 playback supplies frame counters; small per-call samples are not a 4K60 benchmark.',
    'Yielding improves scheduling but can increase elapsed time; no CPU speedup is implied.']}));
 }finally{await browser.close();}
})().catch(e=>{console.error(e);process.exitCode=1;});
