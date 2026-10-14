# Reproducible tools

Requires Rust with wasm32-unknown-unknown, wasm-bindgen CLI matching Cargo.lock,
ffmpeg with libx264, Python 3 and Node. Generated files are ignored.

- `bash tools/demo.sh`: build, generate local media and serve http://127.0.0.1:8765.
  The page exposes `live(url)`, `movie(url)`, `zap()`, `detach()` and `sample()`.
- `cargo build --release --bin rstreamkit && python3 tools/compare.py`: the `rstreamkit`
  command against FFmpeg on five workloads, outputs checked. `tools/peak.rs` launches
  each run so its peak memory isn't inflated by Python's own (`python3 tools/compare.py "10 min"`
  runs one workload).
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

## Small player and download budgets

`tools/player` is a Rust browser entry point with playback controls only. The larger
`tools/harness` also exports benchmarks; its size is not the player download size.

With wasm-opt and Brotli installed, run:

```sh
cargo fetch --manifest-path tools/player/Cargo.toml --target wasm32-unknown-unknown
python3 tools/size.py --check-budgets
python3 tools/server.py
```

The size script builds offline after fetching dependencies. CI checks all four
download budgets with Binaryen 130 and saves the JSON report. It records compiler and
tool versions, source/artifact hashes, the resolved Cargo.lock, raw bytes and Brotli
bytes in `tools/results/size.json`. It leaves playable packages under `tools/web/player`.
The download count includes both Wasm and generated JS. It excludes the host page,
media, HTTP headers and optional type declarations. There is no JS minifier in this
recipe. Keep the same features and recipe when comparing changes.

Open http://127.0.0.1:8765/player.html?build=full. Choose `live`, `live-sound`,
`vod` or `full` in the URL. `vod` includes live and movie playback, without the optional
sound decoders. Removing sound means unsupported source audio needs another stream;
it does not provide equivalent coverage to the full build.

The page saves JSON with time to first presented frame, frame drops, visible/hidden
time, long tasks, bounded status/memory samples and codec capability predictions.
Wasm memory excludes JS, decoder and GPU memory. Long tasks are not total CPU time.
The development server serves uncompressed packages; the size report measures
compressed artifacts separately. Short checks and capability predictions do not
prove 4K60 support. Record fixture hashes and hardware alongside the saved report.

`python3 tools/4k.py --mbps 40` generates an 18-second 3840×2160/60 SDR H.264
High level 5.2 + AAC stress stream, split into three six-second segments. Use 15,
25 or 40 Mbps. The fixture is synthetic and tests load, not visual quality.
Its JSON sidecar records the encoder command, version, probed video format and
file hashes. Play `/media/4k60-40m.m3u8` in the small player. Finish encoding and
other CPU-heavy jobs before measuring, use a fresh page for each run, and compare
all 1,080 expected frames as well as reported drops: skipping media is not a win.

`bash tools/build.sh && node tools/node/audio-smoke.cjs` checks AC-3, E-AC-3 and MP2
decoding in browser-targeted Wasm using checked-in fixtures. CI treats this as a
required check, separately from the optional performance profiling job.

The page also offers pause/resume/go-live controls, codec probes and a cold
main-thread/worker audio benchmark. `worker.js` uses a transferable input buffer.
These are diagnostic controls; no generated media is sent outside localhost.

`node tools/node/soak.cjs` runs five batches of 2,000 short segments and checks
that committed wasm memory stops growing after the first batch. This accelerated
pipeline check is separate from a real-time browser/network soak.

## Shared control checks

Build with `python3 tools/size.py`, serve with `python3 tools/server.py`, and
install `@playwright/test@1.64.0` plus Chrome in a development directory. Set
`NODE_PATH` to its `node_modules`, then run:

```sh
node tools/node/retry-smoke.cjs
node tools/node/buffer-smoke.cjs
node tools/node/session-smoke.cjs
node tools/node/abr-smoke.cjs
node tools/node/mono-smoke.cjs
```

These use routes and committed fixtures; no generated media is needed.
`RSTREAMKIT_URL` can point at another local full-player URL. They check retries,
source failures and go-live request counts, not speed or 4K performance.

The ABR check starts at the higher advertised level, slows complete segment
responses for at least three lower-level downloads, then restores speed and
requires three higher-level downloads plus continued playback. Both levels use
the same committed clip to isolate policy; this is not a resolution/quality test.

The mono check observes decoded PCM with Web Audio and requires non-silent,
equal left/right channels; it sends no sound to physical speakers.
