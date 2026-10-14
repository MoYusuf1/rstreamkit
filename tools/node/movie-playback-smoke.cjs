// Real movie playback, encrypted Dolby and a seek during an audio yield.
const assert=require('node:assert/strict'),fs=require('node:fs'),path=require('node:path'),os=require('node:os'),crypto=require('node:crypto');
const {execFileSync}=require('node:child_process'),{chromium,expect}=require('@playwright/test');
(async()=>{
 const temporary=fs.mkdtempSync(path.join(os.tmpdir(),'rstreamkit-movie-'));let browser;
 try{
  const long=path.join(temporary,'long.mp4'),ts=path.join(temporary,'six.ts');
  execFileSync('ffmpeg',['-v','error','-f','lavfi','-i','testsrc2=size=160x90:rate=30:duration=40',
   '-f','lavfi','-i','sine=frequency=440:sample_rate=48000:duration=40','-c:v','libx264','-preset','ultrafast',
   '-g','60','-c:a','ac3','-ac','6','-b:a','384k','-movflags','+faststart',long]);
  execFileSync('ffmpeg',['-v','error','-i',long,'-t','6','-c','copy','-f','mpegts',ts]);
  const key=Buffer.from(Array.from({length:16},(_,i)=>i)),iv=Buffer.alloc(16);iv[15]=1;
  const cipher=crypto.createCipheriv('aes-128-cbc',key,iv),encrypted=Buffer.concat([cipher.update(fs.readFileSync(ts)),cipher.final()]);
  browser=await chromium.launch({channel:'chrome'});const results=[];
  for(const scenario of ['mp4','mkv','encrypted','seek']){
   const page=await browser.newPage(),errors=[],requests=[];page.on('pageerror',e=>errors.push(e.message));
   try{
    const data=fs.readFileSync(scenario==='seek'?long:path.join(__dirname,'../../tests/fixtures','movie_ac3.'+(scenario==='mkv'?'mkv':'mp4')));
    await page.route('**/test-movie.*',route=>{
     const match=route.request().headers().range?.match(/^bytes=(\d+)-(\d*)$/);
     if(!match)return route.fulfill({body:data});
     const start=Number(match[1]),end=Math.min(match[2]?Number(match[2]):data.length-1,data.length-1);
     requests.push({start,end});
     return route.fulfill({status:206,headers:{'Content-Range':'bytes '+start+'-'+end+'/'+data.length,'Accept-Ranges':'bytes'},body:data.subarray(start,end+1)});
    });
    await page.route('**/encrypted-dolby.m3u8',r=>r.fulfill({contentType:'application/vnd.apple.mpegurl',body:
     '#EXTM3U\n#EXT-X-TARGETDURATION:7\n#EXT-X-KEY:METHOD=AES-128,URI="test-key",IV=0x00000000000000000000000000000001\n#EXTINF:6,\ntest-encrypted.ts\n#EXT-X-ENDLIST\n'}));
    await page.route('**/test-key',r=>r.fulfill({body:key}));
    await page.route('**/test-encrypted.ts',r=>r.fulfill({body:encrypted}));
    await page.goto(new URL('/player.html?build=full',process.env.RSTREAMKIT_URL||'http://127.0.0.1:8765').href);
    await page.waitForFunction(()=>window.ready);
    await page.evaluate(scenario=>{
     const video=document.querySelector('#video');video.muted=false;
     const context=new AudioContext(),analyzer=context.createAnalyser();analyzer.fftSize=2048;
     context.createMediaElementSource(video).connect(analyzer);
     const silent=context.createGain();silent.gain.value=0;analyzer.connect(silent).connect(context.destination);
     window.movieAudio={max_rms:0,blocks:0};const values=new Float32Array(2048);
     const timer=setInterval(()=>{analyzer.getFloatTimeDomainData(values);movieAudio.blocks++;
      movieAudio.max_rms=Math.max(movieAudio.max_rms,Math.sqrt(values.reduce((a,b)=>a+b*b,0)/values.length));
     },20);
     document.querySelector('#play').addEventListener('click',()=>context.resume());
     window.closeMovieAudio=async()=>{clearInterval(timer);await context.close();};
     window.seekReport={injected:false,video_starts:[]};
     if(scenario==='seek'){
      const timeout=window.setTimeout.bind(window);
      window.setTimeout=function(fn,delay,...args){
       if(delay===0&&!seekReport.injected&&video.duration>20){seekReport.injected=true;video.currentTime=14;}
       return timeout(fn,delay,...args);
      };
      const append=SourceBuffer.prototype.appendBuffer;
      SourceBuffer.prototype.appendBuffer=function(data){
       const bytes=data instanceof ArrayBuffer?new Uint8Array(data):new Uint8Array(data.buffer,data.byteOffset,data.byteLength);
       if(seekReport.injected&&String.fromCharCode(...bytes.subarray(4,8))==='moof'){
        const view=new DataView(bytes.buffer,bytes.byteOffset,bytes.byteLength);let track,base;
        for(let i=0;i+16<=Math.min(bytes.length,1024);i++){
         const tag=String.fromCharCode(...bytes.subarray(i,i+4));
         if(tag==='tfhd')track=view.getUint32(i+8);
         if(tag==='tfdt'&&track===1){base=Number(view.getBigUint64(i+8))/90000;break;}
        }
        if(base!=null)seekReport.video_starts.push(base);
       }
       return append.call(this,data);
      };
     }
    },scenario);
    await page.locator('#mode').selectOption(scenario==='encrypted'?'live':'movie');
    await page.locator('#url').fill(scenario==='encrypted'?'/encrypted-dolby.m3u8':'/test-movie.'+(scenario==='mkv'?'mkv':'mp4'));
    await page.locator('#play').click();
    if(scenario==='seek'){
     await expect.poll(()=>page.evaluate(()=>seekReport.injected),{timeout:15000}).toBe(true);
     await expect.poll(()=>page.locator('#video').evaluate(v=>v.currentTime),{timeout:15000}).toBeGreaterThan(15);
    }else{
     await expect.poll(()=>page.locator('#video').evaluate(v=>v.ended),{timeout:20000}).toBe(true);
     assert(await page.locator('#video').evaluate(v=>v.currentTime)>5.6);
    }
    const report=await page.evaluate(()=>({sample:sample(),audio:movieAudio,seek:seekReport}));
    assert(report.audio.max_rms>.005,'decoded movie sound is non-silent');
    assert(!report.sample.session.statuses.some(s=>/Failed|Unsupported|BrowserError/.test(s.status)));assert.deepEqual(errors,[]);
    if(scenario==='seek'){
     assert(report.seek.video_starts.length>0);
     assert(report.seek.video_starts.every(t=>t>=11.9),'old decoded range was discarded after the seek');
    }
    await page.evaluate(async()=>{stop();await closeMovieAudio();});
    results.push({scenario,requests,...report});
   }catch(e){console.error(JSON.stringify({scenario,requests,report:await page.evaluate(()=>({sample:sample(),audio:movieAudio,seek:seekReport}))}));throw e;}
   finally{await page.close();}
  }
  console.log(JSON.stringify({browser:await browser.version(),checks:results.length,results,
   limits:['Generated low-resolution input and committed six-second fixtures; not 4K60 or hardware audio acceptance.',
    'The seek is injected at an actual zero-delay audio yield to exercise the new asynchronous race.']}));
 }finally{if(browser)await browser.close();fs.rmSync(temporary,{recursive:true,force:true});}
})().catch(e=>{console.error(e);process.exitCode=1;});
