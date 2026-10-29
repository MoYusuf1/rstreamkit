// Movie loading from a slow server: each range answers after RSTREAMKIT_LATENCY_MS (default 400).
// Reports time to Playing and to having 20 seconds buffered ahead of the playhead.
const assert=require('node:assert/strict'),fs=require('node:fs'),path=require('node:path'),os=require('node:os');
const {execFileSync}=require('node:child_process'),{chromium,expect}=require('@playwright/test');
const latency=Number(process.env.RSTREAMKIT_LATENCY_MS||400);
(async()=>{
 const temporary=fs.mkdtempSync(path.join(os.tmpdir(),'rstreamkit-latency-'));let browser;
 try{
  const make=name=>{const out=path.join(temporary,name);execFileSync('ffmpeg',['-v','error',
   '-f','lavfi','-i','testsrc2=size=1280x720:rate=30:duration=60','-f','lavfi','-i','sine=frequency=440:sample_rate=48000:duration=60',
   '-c:v','libx264','-preset','ultrafast','-b:v','6M','-g','60','-c:a','ac3','-ac','6','-b:a','448k',out]);return out;};
  const files={mkv:make('movie.mkv'),mp4:make('movie.mp4')};
  browser=await chromium.launch({channel:'chrome'});const results=[];
  for(const [name,file] of Object.entries(files)){
   const page=await browser.newPage(),errors=[];let requests=0;page.on('pageerror',e=>errors.push(e.message));
   const data=fs.readFileSync(file);
   await page.route('**/slow-movie.*',async route=>{
    const match=route.request().headers().range?.match(/^bytes=(\d+)-(\d*)$/);
    await new Promise(r=>setTimeout(r,latency));
    if(!match)return route.fulfill({body:data});
    requests++;
    const start=Number(match[1]),end=Math.min(match[2]?Number(match[2]):data.length-1,data.length-1);
    return route.fulfill({status:206,headers:{'Content-Range':'bytes '+start+'-'+end+'/'+data.length,'Accept-Ranges':'bytes'},body:data.subarray(start,end+1)});
   });
   await page.goto(new URL('/player.html?build=full',process.env.RSTREAMKIT_URL||'http://127.0.0.1:8765').href);
   await page.waitForFunction(()=>window.ready);
   await page.evaluate(()=>{document.querySelector('#video').muted=true;});
   await page.locator('#mode').selectOption('movie');
   await page.locator('#url').fill('/slow-movie.'+name);
   await page.locator('#play').click();
   await expect.poll(()=>page.evaluate(()=>sample().session.statuses.some(s=>s.status==='Playing')),{timeout:30000}).toBe(true);
   const playing=await page.evaluate(()=>sample().session.statuses.find(s=>s.status==='Playing').ms);
   await page.waitForFunction(()=>{const v=document.querySelector('#video');
    return v.buffered.length&&v.buffered.end(v.buffered.length-1)-v.currentTime>=20;},null,{timeout:60000,polling:50});
   const ahead_ms=await page.evaluate(()=>performance.now()-sample().session.started);
   results.push({name,bytes:data.length,playing_ms:Math.round(playing),ms_to_20s_ahead:Math.round(ahead_ms),range_requests:requests});
   assert.deepEqual(errors,[]);
   await page.close();
  }
  console.log(JSON.stringify({latency_ms:latency,checks:results.length,results},null,1));
 }finally{await browser?.close();fs.rmSync(temporary,{recursive:true,force:true});}
})().catch(e=>{console.error(e);process.exit(1);});
