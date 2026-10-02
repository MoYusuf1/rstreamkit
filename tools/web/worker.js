import init,* as kit from './pkg/rstreamkit_harness.js';
onmessage=async e=>{await init();const t=performance.now();await kit.timer();const timer=performance.now()-t;const start=performance.now();const output=kit.transmux(new Uint8Array(e.data),true,1);postMessage({worker_timer_ms:timer,worker_cpu_ms:performance.now()-start,worker_output_bytes:output,worker_wasm_bytes:kit.memory_bytes()})};
