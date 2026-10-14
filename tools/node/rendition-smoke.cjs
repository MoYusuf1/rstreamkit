// Development test: NODE_PATH=tools/browser/node_modules node tools/node/rendition-smoke.cjs
const assert=require('node:assert/strict');
const fs=require('node:fs'),os=require('node:os'),path=require('node:path');
const {execFileSync}=require('node:child_process');
const {chromium,expect}=require('@playwright/test');
(async()=>{
 const temporary=fs.mkdtempSync(path.join(os.tmpdir(),'rstreamkit-renditions-'));let browser;
 try {
  execFileSync('ffmpeg',['-v','error','-f','lavfi','-i','testsrc2=size=160x90:rate=30:duration=80','-c:v','libx264','-preset','ultrafast','-g','60','-an','-f','hls','-hls_time','2','-hls_list_size','0','-hls_segment_filename',path.join(temporary,'video-%d.ts'),path.join(temporary,'video.m3u8')]);
  for(const [language,frequency] of [['eng',440],['spa',880]])
   execFileSync('ffmpeg',['-v','error','-f','lavfi','-i','sine=frequency='+frequency+':sample_rate=48000:duration=80','-c:a','aac','-b:a','96k','-f','hls','-hls_time','2','-hls_list_size','0','-hls_segment_filename',path.join(temporary,language+'-%d.ts'),path.join(temporary,language+'.m3u8')]);
  browser=await chromium.launch({channel:'chrome'});
  const page=await browser.newPage();const requests=[],errors=[];let hevcRequests=0;
  if(process.env.RSTREAMKIT_HARNESS_DIR)for(const file of ['rstreamkit_harness.js','rstreamkit_harness_bg.wasm'])
   await page.route('**/pkg/'+file,route=>route.fulfill({contentType:file.endsWith('.wasm')?'application/wasm':'application/javascript',body:fs.readFileSync(path.join(process.env.RSTREAMKIT_HARNESS_DIR,file))}));
  page.on('pageerror',error=>errors.push(error.message));
  const epoch=Date.now(),duration=2;
  const list=(prefix,extension)=>{
   const end=5+Math.floor((Date.now()-epoch)/1000/duration),first=Math.max(0,end-5);
   return '#EXTM3U\n#EXT-X-TARGETDURATION:5\n#EXT-X-MEDIA-SEQUENCE:'+first+'\n'+
    Array.from({length:end-first+1},(_,i)=>'#EXTINF:'+duration+',\n'+prefix+'-'+(first+i)+'.'+extension+'\n').join('');
  };
  await page.route('**/rendition-master.m3u8',route=>route.fulfill({contentType:'application/vnd.apple.mpegurl',body:
   '#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=2000000,CODECS="hvc1.1.6.L93.B0,mp4a.40.2"\nrendition-hevc.m3u8\n'+['high','low'].map(level=>
    ['eng','spa'].map(language=>'#EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID="'+level+'",NAME="'+language+'",LANGUAGE="'+language+'",DEFAULT='+(language==='eng'?'YES':'NO')+',URI="rendition-'+level+'-'+language+'.m3u8"\n').join('')+
    '#EXT-X-STREAM-INF:BANDWIDTH='+(level==='high'?1000000:100000)+',CODECS="avc1.64001f,mp4a.40.2",AUDIO="'+level+'"\nrendition-'+level+'.m3u8\n').join('')}));
  await page.route('**/rendition-hevc.m3u8',route=>{hevcRequests++;return route.fulfill({status:404,body:'unsupported variant should not be fetched'});});
  await page.route(/\/rendition-(high|low)(-(eng|spa))?\.m3u8$/,route=>{
   const m=route.request().url().match(/rendition-(high|low)(-(eng|spa))?\.m3u8$/);
   return route.fulfill({contentType:'application/vnd.apple.mpegurl',body:list('rendition-'+m[1]+(m[2]||''),'ts')});
  });
  await page.route(/\/rendition-(high|low)(-(eng|spa))?-\d+\.(ts|aac)$/,route=>{
   const m=route.request().url().match(/rendition-(high|low)(-(eng|spa))?-(\d+)\.(ts|aac)$/);
   requests.push({level:m[1],language:m[3]||null,sequence:Number(m[4]),ms:Date.now()-epoch});
   return route.fulfill({contentType:'video/mp2t',body:fs.readFileSync(path.join(temporary,(m[3]||'video')+'-'+m[4]+'.ts'))});
  });
  await page.route('**/rendition-test.html',route=>route.fulfill({contentType:'text/html',body:
   '<video id="video" muted playsinline></video><button id="start">Start</button><script type="module">import * as kit from "/pkg/rstreamkit_harness.js";await kit.default();window.kit=kit;window.statuses=[];window.onStatus=s=>statuses.push(s);document.querySelector("#start").onclick=()=>kit.live(new URL("/rendition-master.m3u8",location).href);window.ready=true;</script>'}));
  await page.goto(new URL(process.env.RSTREAMKIT_URL||'http://127.0.0.1:8765').origin+'/rendition-test.html');
  await page.waitForFunction(()=>window.ready);
  await page.evaluate(()=>{
   const video=document.querySelector('#video');video.muted=false;
   const context=new AudioContext(),analyzer=context.createAnalyser();analyzer.fftSize=4096;analyzer.smoothingTimeConstant=0;
   context.createMediaElementSource(video).connect(analyzer);
   const silent=context.createGain();silent.gain.value=0;analyzer.connect(silent).connect(context.destination);
   const bins=new Float32Array(analyzer.frequencyBinCount);window.tones=[];
   const timer=setInterval(()=>{analyzer.getFloatFrequencyData(bins);let peak=0;
    for(let i=1;i<bins.length;i++)if(bins[i]>bins[peak])peak=i;
    tones.push({frequency:peak*context.sampleRate/analyzer.fftSize,db:bins[peak],time:video.currentTime});
    if(tones.length>1000)tones.shift();
   },50);
   document.querySelector('#start').addEventListener('click',()=>context.resume());
   window.closeAudio=async()=>{clearInterval(timer);await context.close();};
  });
  await page.locator('#start').click();
  try { await expect.poll(()=>page.locator('#video').evaluate(v=>v.currentTime),{timeout:20000}).toBeGreaterThan(6); } catch(error) { console.error(JSON.stringify({requests,errors,report:await page.evaluate(()=>({statuses,stats:kit.stats(),error:document.querySelector('#video').error?.message}))}));throw error; }
  const before=await page.evaluate(()=>({tones,stats:kit.stats()}));
  assert(before.tones.some(t=>t.db>-40&&Math.abs(t.frequency-440)<40),'default English tone is decoded');
  await page.evaluate(()=>kit.set_audio_track('spa'));
  await expect.poll(()=>requests.filter(r=>r.level==='high'&&r.language==='spa').length,{timeout:15000}).toBeGreaterThanOrEqual(1);
  await page.evaluate(()=>kit.set_variant('rendition-low.m3u8'));
  await expect.poll(()=>requests.filter(r=>r.level==='low'&&r.language).length,{timeout:15000}).toBeGreaterThanOrEqual(2).catch(async error=>{console.error(JSON.stringify({requests,errors,report:await page.evaluate(()=>({statuses,stats:kit.stats(),time:document.querySelector('#video').currentTime,buffer:Array.from({length:document.querySelector('#video').buffered.length},(_,i)=>[document.querySelector('#video').buffered.start(i),document.querySelector('#video').buffered.end(i)])}))}));throw error;});
  assert(requests.filter(r=>r.level==='low'&&r.language).every(r=>r.language==='spa'),'quality change keeps the selected language');
  const lowAudioStart=(requests.find(r=>r.level==='low'&&r.language).sequence-requests.find(r=>!r.language).sequence)*duration;
  await expect.poll(()=>page.evaluate(start=>tones.filter(t=>t.time>start+.5&&t.db>-40&&Math.abs(t.frequency-880)<40).length,lowAudioStart),{timeout:45000}).toBeGreaterThanOrEqual(5);
  const report=await page.evaluate(()=>({tones,stats:kit.stats(),statuses,current_time:document.querySelector('#video').currentTime}));
  assert.equal(hevcRequests,0,'mixed master chooses H.264 before fetching the HEVC variant');
  assert(!report.tones.some(t=>t.time>lowAudioStart+.5&&t.db>-40&&Math.abs(t.frequency-440)<40),'lower quality audio stays Spanish');
  assert(!report.statuses.some(s=>/Failed|Unsupported|BrowserError/.test(s)));assert.deepEqual(errors,[]);
  await page.evaluate(async()=>{kit.stop();await closeAudio();});
  console.log(JSON.stringify({browser:await browser.version(),checks:1,hevc_requests:hevcRequests,low_audio_start:lowAudioStart,requests,...report,limits:['Observed decoded PCM, not physical speakers.','Generated tones isolate language routing; this is not a 4K or Safari test.']}));
 }finally{if(browser)await browser.close();fs.rmSync(temporary,{recursive:true,force:true});}
})().catch(error=>{console.error(error);process.exitCode=1;});
