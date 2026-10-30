// Movies that start late, never start, carry laced sound, have no index or are resumed part way:
// each plays or says why not.
const assert=require('node:assert/strict'),fs=require('node:fs'),path=require('node:path'),os=require('node:os');
const {execFileSync}=require('node:child_process'),{chromium,expect}=require('@playwright/test');
(async()=>{
 const temporary=fs.mkdtempSync(path.join(os.tmpdir(),'rstreamkit-late-'));let browser;
 try{
  const make=(name,args)=>{const out=path.join(temporary,name);execFileSync('ffmpeg',['-v','error',
   '-f','lavfi','-i','testsrc2=size=160x90:rate=30:duration=12','-f','lavfi','-i','sine=frequency=440:sample_rate=48000:duration=12',
   ...args,'-c:v','libx264','-preset','ultrafast','-g','30','-c:a','ac3','-ac','6','-b:a','384k',out]);return out;};
  const cases={
   // The whole clock starts late, as in files cut from a broadcast.
   clock:make('clock.mkv',['-output_ts_offset','0.3']),
   // Too far to jump: the engine must say so instead of waiting.
   far:make('far.mkv',['-output_ts_offset','8']),
   // Buffered at the start but never decodable (keyframe slices removed): the engine must say so.
   undecodable:make('undecodable.mkv',['-bsf:v','filter_units=remove_types=5']),
   // Sound laced as mkvmerge writes it.
   laced:(()=>{const plain=make('plain.mkv',[]),out=path.join(temporary,'laced.mkv');
    execFileSync('python3',[path.join(__dirname,'../lace_mkv.py'),plain,out]);return out;})(),   // Written to a pipe, so it has no cue index: plays from the start.
   nocues:(()=>{const out=path.join(temporary,'nocues.mkv');fs.writeFileSync(out,execFileSync('ffmpeg',['-v','error',
    '-f','lavfi','-i','testsrc2=size=160x90:rate=30:duration=12','-f','lavfi','-i','sine=frequency=440:sample_rate=48000:duration=12',
    '-c:v','libx264','-preset','ultrafast','-g','30','-c:a','ac3','-f','matroska','pipe:1'],{maxBuffer:64<<20}));return out;})(),
   // Resumed 6 s in, as an app does for a half-watched episode.
   resume:make('resume.mkv',[]),
  };
  const from={resume:6};
  browser=await chromium.launch({channel:'chrome'});const results=[];
  for(const [name,file] of Object.entries(cases)){
   const page=await browser.newPage(),errors=[];let requests=0;page.on('pageerror',e=>errors.push(e.message));
   const data=fs.readFileSync(file);
   await page.route('**/late-movie.mkv',route=>{
    const match=route.request().headers().range?.match(/^bytes=(\d+)-(\d*)$/);
    if(!match)return route.fulfill({body:data});
    requests++;
    const start=Number(match[1]),end=Math.min(match[2]?Number(match[2]):data.length-1,data.length-1);
    return route.fulfill({status:206,headers:{'Content-Range':'bytes '+start+'-'+end+'/'+data.length,'Accept-Ranges':'bytes'},body:data.subarray(start,end+1)});
   });
   await page.goto(new URL('/player.html?build=full',process.env.RSTREAMKIT_URL||'http://127.0.0.1:8765').href);
   await page.waitForFunction(()=>window.ready);
   await page.evaluate(()=>{document.querySelector('#video').muted=true;});
   await page.locator('#mode').selectOption('movie');
   await page.locator('#url').fill('/late-movie.mkv');
   const started=Date.now();
   if(from[name])page.evaluate(at=>window.start('/late-movie.mkv','movie',at),from[name]).catch(e=>errors.push(String(e)));
   else await page.locator('#play').click();
   let ok=true;
   try{await expect.poll(()=>page.locator('#video').evaluate(v=>v.currentTime),{timeout:15000}).toBeGreaterThan((from[name]??0)+3);}catch{ok=false;await page.waitForTimeout(name==='undecodable'?2000:0);}
   const report=await page.evaluate(()=>({sample:sample(),ready:document.querySelector('#video').readyState,
    buffered:[...Array(document.querySelector('#video').buffered.length).keys()].map(i=>[document.querySelector('#video').buffered.start(i),document.querySelector('#video').buffered.end(i)])}));
   results.push({name,plays:ok,ms:Date.now()-started,range_requests:requests,ready:report.ready,buffered:report.buffered,
    statuses:report.sample.session.statuses.map(s=>s.status)});
   await page.close();
   assert.deepEqual(errors,[]);
  }
  console.log(JSON.stringify({checks:results.length,results},null,1));
  const [clock,far,undecodable,laced,nocues,resume]=results;
  assert(resume.plays&&resume.statuses.includes('Playing')&&resume.buffered.every(([a])=>a>=5),'a resumed movie plays from where it was left');
  assert(nocues.plays&&nocues.statuses.includes('Playing'),'a movie without cues plays from the start');
  assert(clock.plays&&clock.statuses.includes('Playing')&&clock.range_requests<20,'late clock plays without refetching');
  assert(!far.plays&&far.statuses.some(s=>/Failed/.test(s))&&far.range_requests<20,'unreachable start fails promptly');
  assert(laced.plays&&laced.statuses.includes('Playing'),'laced sound plays');
  assert(!undecodable.plays&&undecodable.statuses.some(s=>/Failed/.test(s)),'undecodable start fails');
  assert(undecodable.statuses.some(s=>/no picture decoded.*\(MKV, H\.264 .*given as video\//.test(s)),'the failure names the tracks');
 }finally{await browser?.close();fs.rmSync(temporary,{recursive:true,force:true});}
})().catch(e=>{console.error(e);process.exit(1);});
