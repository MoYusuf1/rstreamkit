// node tools/node/opt-compare.cjs /absolute/baseline.js /absolute/candidate.js
// Development harness comparison; run after compilation, one process per comparison.
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const crypto = require('node:crypto');
const os = require('node:os');
const { performance } = require('node:perf_hooks');
const files = process.argv.slice(2);
assert.equal(files.length, 2, 'supply baseline and candidate Node harness modules');
const builds = files.map(file => {
  const absolute = path.resolve(file);
  const start = performance.now();
  const kit = require(absolute);
  const moduleLoad = performance.now()-start;
  const wasm = fs.readFileSync(absolute.replace(/\.js$/, '_bg.wasm'));
  return { kit, module: absolute, wasm_bytes: wasm.length,
    wasm_sha256: crypto.createHash('sha256').update(wasm).digest('hex'),
    module_load_init_ms: moduleLoad };
});
const results=[];
for (const name of ['bbb_480p.ts', 'h264_ac3_st.ts', 'h264_ac3_51.ts', 'h264_eac3_51.ts', 'h264_mp2.ts']) {
  const bytes=fs.readFileSync(path.join(__dirname,'../../tests/fixtures',name));
  const audio=name==='bbb_480p.ts' ? null : builds.map(build => new build.kit.ProfileAudio(bytes));
  try {
    if (audio) {
      assert.equal(audio[0].pcm_checksum(),audio[1].pcm_checksum(), `${name}: decoded PCM differs`);
      assert.equal(audio[0].raw_size(),audio[1].raw_size());
      assert.equal(audio[0].encode(),audio[1].encode(), `${name}: encoded byte count differs`);
    }
    for (const scenario of audio ? ['transmux','flac'] : ['transmux']) {
      const run=index => scenario==='flac' ? audio[index].encode() : builds[index].kit.transmux(bytes,true,1);
      assert.equal(run(0),run(1), `${name}: output byte counts differ`);
      for (const index of [0,1]) for(let i=0;i<5;i++)run(index);
      const samples=[[],[]];
      for(let pair=0;pair<9;pair++)for(const index of pair%2===0?[0,1]:[1,0]){
        const start=performance.now();for(let i=0;i<5;i++)run(index);
        samples[index].push((performance.now()-start)/5);
      }
      samples.forEach(times=>times.sort((a,b)=>a-b));
      results.push({file:name,input_bytes:bytes.length,input_sha256:crypto.createHash('sha256').update(bytes).digest('hex'),
        scenario,pcm_checksum:audio?audio[0].pcm_checksum().toString(16):null,
        median_ms:samples.map(times=>times[4]),p95_batch_ms:samples.map(times=>times[8]),
        output_bytes:run(0),wasm_bytes:builds.map(build=>build.kit.memory_bytes())});
    }
  } finally { audio?.forEach(item=>item.free()); }
}
console.log(JSON.stringify({node:process.version,platform:process.platform,arch:process.arch,
  cpu:os.cpus()[0].model,os_release:os.release(),builds:builds.map(({kit,...build})=>build),results,
  limits:['Both modules coexist in one Node process; module load/init is diagnostic, not a browser cold-start benchmark.',
    'Transmux includes input copying and a new decoder per call; FLAC uses predecoded PCM.',
    'Wasm committed bytes exclude JS, browser decoder and GPU memory.',
    'Timing changes do not establish 4K playback or general FFmpeg superiority.']}));
