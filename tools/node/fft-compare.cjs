// node tools/node/fft-compare.cjs SCALAR.js CANDIDATE.js
const assert=require('node:assert/strict'),fs=require('node:fs'),path=require('node:path'),crypto=require('node:crypto'),os=require('node:os');
const {performance}=require('node:perf_hooks');
assert.equal(process.argv.length,4,'supply scalar and candidate modules');
const modules=process.argv.slice(2).map(p=>path.resolve(p)),builds=modules.map(p=>require(p));
const inputs=[new Float32Array(256),Float32Array.from({length:256},(_,i)=>i%2?-0:0)];
for(let at=0;at<256;at++){const x=new Float32Array(256);x[at]=1;inputs.push(x);}
let state=0x12345678;
const random=()=>{state^=state<<13;state^=state>>>17;state^=state<<5;return state>>>0;};
for(let n=0;n<1024;n++){
 const x=new Float32Array(256),view=new Uint32Array(x.buffer);
 for(let i=0;i<256;i++)view[i]=(random()&0x807fffff)|((random()%190)<<23);
 inputs.push(x);
}
const digest=crypto.createHash('sha256');let cases=0;
for(const short of [false,true])for(const input of inputs){
 const outputs=builds.map(kit=>kit.transform(input,short));
 const bytes=outputs.map(x=>Buffer.from(x.buffer,x.byteOffset,x.byteLength));
 assert.deepEqual(bytes[0],bytes[1],`transform bytes differ: short=${short}, case=${cases}`);
 digest.update(bytes[0]);cases++;
}
const measurements=[];
for(const short of [false,true]){
 const input=Float32Array.from({length:256},(_,i)=>Math.sin(i*.17)*.01);
 for(const kit of builds)for(let i=0;i<5;i++)kit.transform_repeat(input,short,1000);
 const samples=[[],[]];
 for(let pair=0;pair<9;pair++)for(const index of pair%2?[1,0]:[0,1]){
  const start=performance.now();builds[index].transform_repeat(input,short,10000);
  samples[index].push((performance.now()-start)/10000);
 }
 samples.forEach(x=>x.sort((a,b)=>a-b));
 measurements.push({short,calls_per_batch:10000,pairs:9,median_ms:samples.map(x=>x[4]),max_batch_ms:samples.map(x=>x[8])});
}
console.log(JSON.stringify({script_sha256:crypto.createHash("sha256").update(fs.readFileSync(__filename)).digest("hex"),input_seed:0x12345678,node:process.version,cpu:os.cpus()[0].model,cases,complete_output_sha256:digest.digest('hex'),
 builds:modules.map(file=>({file,wasm_sha256:crypto.createHash('sha256').update(fs.readFileSync(file.replace(/\.js$/,'_bg.wasm'))).digest('hex')})),measurements,
 limits:['Warm Node batches run 10,000 transforms inside Wasm; one input copy per batch.',
  'Exact finite coefficient gates include signed zero, all basis impulses and seeded bit-pattern vectors.',
  'Not a native, complete player or 4K benchmark; exceptional NaN payloads are outside the exact gate.']}));
