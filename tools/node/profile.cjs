const fs=require('node:fs');const {performance}=require('node:perf_hooks');
const kit=require('./pkg/rstreamkit_harness.js');
const [scenario,file]=process.argv.slice(2);const bytes=fs.readFileSync(file);const times=[];
const audio=scenario==='flac'?new kit.ProfileAudio(bytes):null;let encoded=0;
function index(){
if(file.endsWith('.mkv')){const probe=new kit.ProfileProbe(BigInt(bytes.length));let at=0,len=256<<10;
for(let i=0;i<64;i++){const next=probe.feed(BigInt(at),bytes.subarray(at,Math.min(at+len,bytes.length)));if(!next){const session=probe.session();probe.free();return session;}[at,len]=Array.from(next,Number)}throw Error('probe incomplete');}
let at=0;while(at+8<=bytes.length){let size=bytes.readUInt32BE(at),head=8;if(size===1){size=Number(bytes.readBigUInt64BE(at+8));head=16;}if(size===0)size=bytes.length-at;
if(bytes.toString('ascii',at+4,at+8)==='moov')return kit.ProfileSession.mp4(bytes.subarray(at+head,at+size),BigInt(bytes.length));if(size<head)throw Error('bad box');at+=size;}throw Error('no moov');}

for(let i=0;i<7;i++){const t=performance.now();
if(audio)encoded=audio.encode();
else if(scenario==='demux')kit.demux(bytes);
else if(scenario==='stream'){const stream=new kit.ProfileStream();stream.begin(bytes.length);for(let at=0;at<bytes.length;at+=16384)stream.feed(bytes.subarray(at,at+16384));stream.finish();stream.free();}
else if(scenario==='push')kit.transmux(bytes,false,1);
else {const session=index();if(scenario==='movie'){let range;while((range=session.range())){const [at,len]=Array.from(range,Number);session.push(bytes.subarray(at,Math.min(at+len,bytes.length)));}}session.free();}
times.push(performance.now()-t)}
times.sort((a,b)=>a-b);
console.log(JSON.stringify({scenario,file,input:bytes.length,median_ms:times[3],p95_ms:times[6],wasm_bytes:kit.memory_bytes(),...(audio?{pcm_bytes:audio.raw_size(),flac_bytes:encoded,ratio:encoded/audio.raw_size()}: {})}));
