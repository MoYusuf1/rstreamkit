// Fresh Chrome processes; loopback startup timing, asset inventory and Wasm memory.
const assert=require("node:assert/strict"),fs=require("node:fs"),path=require("node:path"),crypto=require("node:crypto"),os=require("node:os");
const {chromium}=require("@playwright/test");
const flavors=["live","live-sound","vod","full"],trials=Number(process.env.RSTREAMKIT_STARTUP_TRIALS||5);
assert(Number.isInteger(trials)&&trials>=1&&trials<=20,"trials must be 1–20");
const sha=file=>crypto.createHash("sha256").update(fs.readFileSync(file)).digest("hex");
(async()=>{
 const runs=[];let version;
 // Alternate flavor order to reduce ordering bias from the OS cache.
 for(let trial=0;trial<trials;trial++)for(const flavor of trial%2?[...flavors].reverse():flavors){
  const browser=await chromium.launch({channel:"chrome"});
  try{
   version=await browser.version();const page=await browser.newPage(),errors=[],requests=[],responses=[],failed=[];
   page.on("pageerror",e=>errors.push(e.message));page.on("request",r=>requests.push(new URL(r.url()).pathname));
   page.on("requestfailed",r=>failed.push({url:r.url(),error:r.failure()}));
   page.on("response",r=>responses.push({path:new URL(r.url()).pathname,status:r.status()}));
   await page.addInitScript(()=>{
    let ready=false;
    Object.defineProperty(window,"ready",{configurable:true,get:()=>ready,set:value=>{ready=value;if(value)window.startupReadyMs=performance.now();}});
   });
   const url=new URL("/player.html",process.env.RSTREAMKIT_URL||"http://127.0.0.1:8765");url.searchParams.set("build",flavor);
   await page.goto(url.href);await page.waitForFunction(()=>window.ready);await page.waitForLoadState("networkidle");
   const result=await page.evaluate(()=>({ready_ms:window.startupReadyMs,startup:sample().startup,
    wasm_bytes:sample().playback.wasm_bytes,resources:performance.getEntriesByType("resource").map(r=>({path:new URL(r.name).pathname,body_bytes:r.decodedBodySize,transfer_bytes:r.transferSize}))}));
   assert.deepEqual(errors,[]);assert.deepEqual(failed,[]);assert(responses.every(r=>r.status===200));
   const expected=["/player.html",`/player/${flavor}/rstreamkit_player.js`,`/player/${flavor}/rstreamkit_player_bg.wasm`].sort();
   assert.deepEqual([...requests].sort(),expected,"unexpected startup asset");
   assert(result.ready_ms>0&&result.wasm_bytes>0);
   for(const resource of result.resources)assert(resource.body_bytes>0);
   runs.push({trial,flavor,...result,requests,responses});
  }finally{await browser.close();}
 }
 const distribution=values=>{values.sort((a,b)=>a-b);const n=values.length;return {min:values[0],median:n%2?values[n>>1]:(values[n/2-1]+values[n/2])/2,max:values[n-1]};};
 const summary=flavors.map(flavor=>{const selected=runs.filter(r=>r.flavor===flavor);return {flavor,trials,
  ready_ms:distribution(selected.map(r=>r.ready_ms)),module_load_and_init_ms:distribution(selected.map(r=>r.startup.module_load_and_init_ms)),
  wasm_fetch_compile_init_ms:distribution(selected.map(r=>r.startup.wasm_fetch_compile_init_ms)),wasm_bytes:distribution(selected.map(r=>r.wasm_bytes))};});
 const root=path.join(__dirname,"../web");
 console.log(JSON.stringify({schema:1,browser:version,cpu:os.cpus()[0].model,node:process.version,
  source_sha256:{"tools/node/startup-smoke.cjs":sha(__filename),"tools/web/player.html":sha(path.join(root,"player.html"))},
  artifacts:flavors.map(flavor=>({flavor,wasm_sha256:sha(path.join(root,"player",flavor,"rstreamkit_player_bg.wasm")),js_sha256:sha(path.join(root,"player",flavor,"rstreamkit_player.js"))})),summary,runs,
  limits:["Fresh Chrome process per trial; loopback uncompressed HTTP and warm OS file cache.","Ready timing starts at navigation and ends at the page ready setter, before playback.","Committed Wasm memory excludes JS, browser decoder, GPU and process RSS.","Small repeated samples show a range, not a production cold-cache percentile or a 4K60 result."]},null,2));
})().catch(e=>{console.error(e);process.exitCode=1;});
