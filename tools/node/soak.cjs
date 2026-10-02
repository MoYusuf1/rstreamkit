// Each batch advances the same stream for 2,000 segments. Repeating detects allocator growth.
const fs=require('node:fs');const {performance}=require('node:perf_hooks');
const kit=require('./pkg/rstreamkit_harness.js');const bytes=fs.readFileSync(process.argv[2]||'tests/fixtures/bbb_480p.ts');
const records=[];
for(let batch=0;batch<5;batch++){const started=performance.now();const output=kit.transmux(bytes,true,2000);records.push({batch,segments:2000,elapsed_ms:performance.now()-started,wasm_bytes:kit.memory_bytes(),last_output_bytes:output})}
if(records.slice(1).some(r=>r.wasm_bytes!==records[0].wasm_bytes))throw Error('committed wasm memory grew after warmup');
console.log(JSON.stringify({input_bytes:bytes.length,records}));
