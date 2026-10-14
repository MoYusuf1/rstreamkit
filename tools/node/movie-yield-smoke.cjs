// Movie range output equivalence and repeated Chrome scheduling measurements.
const assert=require('node:assert/strict'),fs=require('node:fs'),path=require('node:path'),crypto=require('node:crypto'),os=require('node:os');
const {chromium,expect}=require('@playwright/test');
const files=['movie_aac.mp4','movie_ac3.mp4','movie_ac3.mkv'];
(async()=>{
 const browser=await chromium.launch({channel:'chrome'});
 try{
  const page=await browser.newPage(),errors=[];page.on('pageerror',e=>errors.push(e.message));
  await page.route('**/movie-yield-test.html',r=>r.fulfill({contentType:'text/html',body:
   '<video id="video" muted playsinline></video><script type="module">import * as kit from "/pkg/rstreamkit_harness.js";await kit.default();window.kit=kit;window.statuses=[];window.onStatus=s=>statuses.push(s);window.ready=true;</script>'}));
  await page.route('**/movie-fixture/*',r=>r.fulfill({body:fs.readFileSync(path.join(__dirname,'../../tests/fixtures',new URL(r.request().url()).pathname.split('/').pop()))}));
  await page.goto(new URL('/movie-yield-test.html',process.env.RSTREAMKIT_URL||'http://127.0.0.1:8765').href);
  await page.waitForFunction(()=>window.ready);
  await page.evaluate(async files=>{
   window.movieInputs={};
   for(const file of files)movieInputs[file]=new Uint8Array(await(await fetch('/movie-fixture/'+file)).arrayBuffer());
   window.openMovie=(bytes,matroska)=>{
    if(matroska){
     const probe=new kit.ProfileProbe(BigInt(bytes.length));let at=0,len=256<<10;
     try{for(let i=0;i<64;i++){
      const next=probe.feed(BigInt(at),bytes.subarray(at,Math.min(at+len,bytes.length)));
      if(!next)return probe.session();[at,len]=Array.from(next,Number);
     }}finally{probe.free();}throw Error('probe incomplete');
    }
    const view=new DataView(bytes.buffer,bytes.byteOffset,bytes.byteLength);
    for(let at=0;at+8<=bytes.length;){
     let size=view.getUint32(at),head=8;if(size===1){size=Number(view.getBigUint64(at+8));head=16;}
     if(size===0)size=bytes.length-at;
     if(String.fromCharCode(...bytes.subarray(at+4,at+8))==='moov')
      return kit.ProfileSession.mp4(bytes.subarray(at+head,at+size),BigInt(bytes.length));
     if(size<head)throw Error('bad MP4 box');at+=size;
    }throw Error('missing moov');
   };
   window.runMovie=async(file,method,limit)=>{
    const bytes=movieInputs[file],session=openMovie(bytes,file.endsWith('.mkv')),ranges=[];
    try{
     for(let range;(range=session.range_limit(BigInt(limit)));){
      const [at,len]=Array.from(range,Number);ranges.push([at,len]);
      const chunk=bytes.subarray(at,Math.min(at+len,bytes.length));
      if(method==='sync')session.push(chunk);else await session.push_yielded(chunk);
     }
     return {hash:session.output_checksum().toString(16),bytes:session.output_bytes(),ranges};
    }finally{session.free();}
   };
  },files);
  const equivalence=[];
  for(const file of files)for(const limit of [4096,65536,4<<20]){
   const result=await page.evaluate(async({file,limit})=>({sync:await runMovie(file,'sync',limit),yielded:await runMovie(file,'yielded',limit)}),{file,limit});
   assert.deepEqual(result.sync,result.yielded,file+' output differs at '+limit);
   equivalence.push({file,limit,...result.sync});
  }
  await page.evaluate(()=>{
   window.tasks=[];new PerformanceObserver(list=>tasks.push(...list.getEntries().map(e=>({start:e.startTime,duration:e.duration})))).observe({type:'longtask'});
   kit.live(new URL('/media/browser.m3u8',location).href);
  });
  await expect.poll(()=>page.locator('#video').evaluate(v=>v.currentTime),{timeout:15000}).toBeGreaterThan(1);
  const measurements=[];
  for(const file of ['movie_ac3.mp4','movie_ac3.mkv']){
   await page.evaluate(async file=>{for(let i=0;i<2;i++){await runMovie(file,'sync',4<<20);await runMovie(file,'yielded',4<<20);}},file);
   for(let pair=0;pair<5;pair++)for(const method of pair%2===0?['sync','yielded']:['yielded','sync']){
    const result=await page.evaluate(async({file,method})=>{
     await new Promise(resolve=>setTimeout(resolve,50));const v=document.querySelector('#video'),before=v.getVideoPlaybackQuality();
     let beats=0;const timer=setInterval(()=>beats++,1),start=performance.now();
     const output=await runMovie(file,method,4<<20);const end=performance.now();clearInterval(timer);
     await new Promise(resolve=>setTimeout(resolve,50));const after=v.getVideoPlaybackQuality();
     return {elapsed_ms:end-start,beats,output,longtasks:tasks.filter(t=>t.start>=start-1&&t.start<end),
      frames:after.totalVideoFrames-before.totalVideoFrames,dropped:after.droppedVideoFrames-before.droppedVideoFrames};
    },{file,method});
    measurements.push({file,method,pair,...result});
   }
  }
  for(const file of ['movie_ac3.mp4','movie_ac3.mkv']){
   assert.equal(new Set(measurements.filter(r=>r.file===file).map(r=>r.output.hash)).size,1);
  }
  const playback=await page.evaluate(()=>({statuses,time:document.querySelector('#video').currentTime}));
  assert(!playback.statuses.some(s=>/Failed|Unsupported|BrowserError/.test(s)));assert.deepEqual(errors,[]);
  await page.evaluate(()=>kit.stop());
  const sha=file=>crypto.createHash('sha256').update(fs.readFileSync(file)).digest('hex');
  console.log(JSON.stringify({browser:await browser.version(),cpu:os.cpus()[0].model,node:process.version,
   harness_wasm_sha256:sha(path.join(__dirname,'../web/pkg/rstreamkit_harness_bg.wasm')),
   inputs:files.map(file=>({file,sha256:sha(path.join(__dirname,'../../tests/fixtures',file))})),
   equivalence,measurements,playback,
   limits:['Six-second committed movies; includes range indexing/copying and development output hashing.',
    'Warm alternating pairs on 320x180 background playback; not a 4K60 or cold-start benchmark.',
    'Yielding controls main-thread work; it need not reduce total elapsed time.']}));
 }finally{await browser.close();}
})().catch(e=>{console.error(e);process.exitCode=1;});
