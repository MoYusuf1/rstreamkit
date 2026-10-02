# Reproducible tools

Requires Rust with wasm32-unknown-unknown, wasm-bindgen CLI matching Cargo.lock,
ffmpeg with libx264, Python 3 and Node. Generated files are ignored.

- `bash tools/demo.sh`: build, generate local media and serve http://127.0.0.1:8765.
  The page exposes `live(url)`, `movie(url)`, `zap()`, `detach()` and `sample()`.
- `bash tools/profile.sh`: build, generate media and profile wasm in fresh Node
  processes, producing JSON records for demux, streamed and whole-segment pushes,
  movie index and complete sessions. Only the MP4 moov and requested movie ranges
  enter wasm memory; JS reads the generated file to simulate network ranges. The
  FLAC scenario decodes the programme fixture before timing just the encoder.

`/fault/{404,garbage,outage,frozen,no-sequence}.m3u8` uses a six-segment sliding
window with contiguous generated timestamps. Outage/freeze periods repeat every
minute. Add `?delay=1` to delay a request. Status and long-task logs are available
through `sample()`. Browser-specific acceptance results belong in docs/issues.md.

Timing results depend on CPU, browser, compiler and media. Record these with each
run. Sine-wave benchmark audio is deliberately synthetic and must not be used to
claim a representative FLAC compression ratio.

The page also offers pause/resume/go-live controls, codec probes and a cold
main-thread/worker audio benchmark. `worker.js` uses a transferable input buffer.
These are diagnostic controls; no generated media is sent outside localhost.

`node tools/node/soak.cjs` runs five batches of 2,000 short segments and checks
that committed wasm memory stops growing after the first batch. This accelerated
pipeline check is separate from a real-time browser/network soak.
