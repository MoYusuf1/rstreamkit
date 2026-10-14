// node tools/node/audio-cpu.cjs NAMED_HARNESS.js [OUTPUT_DIRECTORY]
// Sampling diagnostics only; never compare these times as a speed benchmark.
const assert=require('node:assert/strict'),fs=require('node:fs'),path=require('node:path'),crypto=require('node:crypto'),os=require('node:os');
const inspector=require('node:inspector'),{performance}=require('node:perf_hooks');
const modulePath=path.resolve(process.argv[2]||'');
assert(process.argv[2],'supply a harness built with --retain-names');
const output=path.resolve(process.argv[3]||'tools/results/audio-cpu');fs.mkdirSync(output,{recursive:true});
const seconds=Number(process.env.RSTREAMKIT_PROFILE_SECONDS||3);
assert(Number.isFinite(seconds)&&seconds>=1&&seconds<=30,'profile duration must be 1–30 seconds');
const kit=require(modulePath),sha=file=>crypto.createHash('sha256').update(fs.readFileSync(file)).digest('hex');
const files=['h264_ac3_st.ts','h264_ac3_51.ts','h264_eac3_51.ts','h264_mp2.ts'];
(async()=>{
 const session=new inspector.Session();session.connect();
 const post=(method,params={})=>new Promise((resolve,reject)=>session.post(method,params,(e,r)=>e?reject(e):resolve(r)));
 const results=[];
 try{
  await post('Profiler.enable');await post('Profiler.setSamplingInterval',{interval:1000});
  for(const file of files){
   const input=path.join(__dirname,'../../tests/fixtures',file),bytes=fs.readFileSync(input),audio=new kit.ProfileAudio(bytes);
   try{
    const pcm_checksum=audio.pcm_checksum().toString(16),pcm_bytes=audio.raw_size(),flac_bytes=audio.encode();
    assert(pcm_bytes>0&&flac_bytes>0);
    for(const stage of ['decode','flac']){
     const run=stage==='decode'?()=>{const a=new kit.ProfileAudio(bytes);assert.equal(a.raw_size(),pcm_bytes);a.free();}:()=>audio.encode();
     for(let i=0;i<20;i++)run();
     await post('Profiler.start');let calls=0;const start=performance.now();
     while(performance.now()-start<seconds*1000){run();calls++;}
     const elapsed_ms=performance.now()-start,{profile}=await post('Profiler.stop');
     const artifact=file+'.'+stage+'.cpuprofile';fs.writeFileSync(path.join(output,artifact),JSON.stringify(profile));
     const nodes=new Map(profile.nodes.map(n=>[n.id,n])),counts=new Map();
     // Merge cloned/inlined name occurrences for the display, retaining raw profiles.
     for(const id of profile.samples||[]){const name=nodes.get(id).callFrame.functionName.replace(/\[[0-9a-f]{16}\]/g,'');counts.set(name,(counts.get(name)||0)+1);}
     const samples=(profile.samples||[]).length;assert(samples>100,'not enough CPU samples');
     const top=[...counts].map(([name,self_samples])=>({name,self_samples,self_fraction:self_samples/samples})).sort((a,b)=>b.self_samples-a.self_samples);
     assert(top.some(n=>n.name.includes('rstreamkit')),'missing Wasm symbols: use --retain-names');
     results.push({file,stage,input_sha256:sha(input),pcm_checksum,pcm_bytes,flac_bytes,calls,elapsed_ms,samples,profile:artifact,top:top.slice(0,20)});
    }
   }finally{audio.free();}
  }
 }finally{session.disconnect();}
 console.log(JSON.stringify({schema:1,node:process.version,cpu:os.cpus()[0].model,os:os.release(),module:modulePath,
  module_sha256:sha(modulePath),wasm_sha256:sha(modulePath.replace(/\.js$/,'_bg.wasm')),script_sha256:sha(__filename),seconds,sampling_interval_us:1000,results,
  limits:['Warm named Wasm in Node; not native CPU, browser frame time or a shipped-byte measurement.',
   'Decode includes TS demux, packet copy, a fresh decoder and PCM conversion; FLAC uses predecoded PCM.',
   'Self samples include inlined work; sampled shares are approximate and cannot be added to inclusive shares.',
   'Profiler overhead makes elapsed time unsuitable for before/after speed claims.',
   'Small synthetic fixtures are not a varied programme corpus or 4K60 acceptance.']},null,2));
})().catch(e=>{console.error(e);process.exitCode=1;});
