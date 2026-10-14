// Real Chrome raw-TS playback and decoded-sound check.
const assert=require('node:assert/strict'),fs=require('node:fs'),path=require('node:path');
const {chromium,expect}=require('@playwright/test');
const os=require('node:os'),{execFileSync}=require('node:child_process');
(async()=>{
 const temporary=fs.mkdtempSync(path.join(os.tmpdir(),'rstreamkit-raw-audio-'));
 let browser;
 try{
  const file=path.join(temporary,'six-seconds.ts');
  if(process.env.RSTREAMKIT_REPEAT_RAW!=='1')execFileSync('ffmpeg',['-v','error','-f','lavfi','-i','testsrc2=size=160x90:rate=30:duration=6',
   '-f','lavfi','-i','aevalsrc=0.06*sin(2*PI*440*t)|0.06*sin(2*PI*550*t)|0.06*sin(2*PI*660*t)|0.06*sin(2*PI*110*t)|0.06*sin(2*PI*770*t)|0.06*sin(2*PI*880*t):s=48000:d=6:c=5.1',
   '-c:v','libx264','-preset','ultrafast','-g','60','-c:a','ac3','-b:a','384k','-f','mpegts',file]);
  const clip=fs.readFileSync(path.join(__dirname,'../../tests/fixtures/h264_ac3_51.ts'));
  const fixture=process.env.RSTREAMKIT_REPEAT_RAW==='1'?Buffer.concat([clip,clip,clip]):fs.readFileSync(file);
  browser=await chromium.launch({channel:'chrome'});
  const page=await browser.newPage(),errors=[];let requests=0;
  if(process.env.RSTREAMKIT_PLAYER_DIR)for(const file of ['rstreamkit_player.js','rstreamkit_player_bg.wasm'])
   await page.route('**/player/full/'+file,route=>route.fulfill({contentType:file.endsWith('.wasm')?'application/wasm':'application/javascript',body:fs.readFileSync(path.join(process.env.RSTREAMKIT_PLAYER_DIR,file))}));
  page.on('pageerror',e=>errors.push(e.message));
  await page.route('**/raw-dolby.ts',route=>{requests++;return route.fulfill({contentType:'video/mp2t',body:fixture});});
  await page.goto(new URL('/player.html?build=full',process.env.RSTREAMKIT_URL||'http://127.0.0.1:8765').href);
  await page.waitForFunction(()=>window.ready);
  await page.evaluate(()=>{
   const video=document.querySelector('#video');video.muted=false;
   const context=new AudioContext(),analyzer=context.createAnalyser();analyzer.fftSize=2048;
   context.createMediaElementSource(video).connect(analyzer);
   const silent=context.createGain();silent.gain.value=0;analyzer.connect(silent).connect(context.destination);
   window.rawAudio={max_rms:0,blocks:0};const samples=new Float32Array(analyzer.fftSize);
   const timer=setInterval(()=>{analyzer.getFloatTimeDomainData(samples);rawAudio.blocks++;
    rawAudio.max_rms=Math.max(rawAudio.max_rms,Math.sqrt(samples.reduce((sum,v)=>sum+v*v,0)/samples.length));
   },20);
   document.querySelector('#play').addEventListener('click',()=>context.resume());
   window.closeRawAudio=async()=>{clearInterval(timer);await context.close();};
  });
  await page.locator('#url').fill('/raw-dolby.ts');await page.locator('#play').click();
  await expect.poll(()=>page.locator('#video').evaluate(v=>v.currentTime),{timeout:20000}).toBeGreaterThan(5.6).catch(async error=>{
   console.error(JSON.stringify(await page.evaluate(()=>({sample:sample(),audio:rawAudio,
    duration:document.querySelector('#video').duration,ended:document.querySelector('#video').ended}))));
   throw error;
  });
  await expect.poll(()=>page.locator('#video').evaluate(v=>v.ended),{timeout:10000}).toBe(true);
  const report=await page.evaluate(()=>{
   const q=document.querySelector('#video').getVideoPlaybackQuality();
   return {sample:sample(),audio:rawAudio,raw_quality:{total:q.totalVideoFrames,dropped:q.droppedVideoFrames}};
  });
  if(report.sample.playback.frames.total < (process.env.RSTREAMKIT_REPEAT_RAW==='1'?149:179))
   console.error(JSON.stringify(report));
  assert(report.sample.playback.frames.total >= (process.env.RSTREAMKIT_REPEAT_RAW==='1'?149:179), 'all source pictures reach playback');
  assert(report.audio.max_rms>.005,'raw Dolby produces decoded sound');
  assert(report.sample.session.statuses.some(s=>s.status==='Ended'));
  assert(!report.sample.session.statuses.some(s=>/Failed|Unsupported|BrowserError/.test(s.status)));
  assert.deepEqual(errors,[]);
  await page.evaluate(async()=>{stop();await closeRawAudio();});
  console.log(JSON.stringify({browser:await browser.version(),checks:1,requests,...report,
   limits:['Generated continuous six-second Dolby TS checks raw playback and sound, not codec quality or 4K60.',
    'Web Audio measures decoded PCM; this is not a physical speaker test.']}));
 }finally{if(browser)await browser.close();fs.rmSync(temporary,{recursive:true,force:true});}
})().catch(e=>{console.error(e);process.exitCode=1;});
