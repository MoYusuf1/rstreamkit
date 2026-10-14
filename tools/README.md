# Reproducible tools

Requires Rust with wasm32-unknown-unknown, wasm-bindgen CLI matching Cargo.lock,
ffmpeg with libx264, Python 3 and Node. Generated files are ignored.

## Browser regressions

Build the four player flavors with `python3 tools/size.py`, then:

```sh
cd tools/browser
npm ci --ignore-scripts
npx playwright install chrome
python3 fixtures.py
npm test -- --project=chrome
```

Five checks cover finite playback after throttling, Dolby sound, missing file
segments, stopping during a fetch, and recovery from a missing live segment.
They save JSON reports and failure traces under `tools/results/`. CI runs real
Chrome with one worker. These are correctness checks, not speed or 4K benchmarks.
The finite-playlist test uses 4× playback after the video is ready to exercise
the buffer threshold without a 40-second wall-clock wait.

Use `--project=edge` with installed Edge, or `--project=webkit` after
`npx playwright install webkit`. Playwright WebKit is not Safari acceptance;
use macOS and real Safari for the final platform checks. Codec support depends
on the browser build and OS. See [Playwright's browser guide](https://playwright.dev/docs/browsers).

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
node tools/node/abr-failure-smoke.cjs
node tools/node/mono-smoke.cjs
node tools/node/resilience-smoke.cjs
node tools/node/pause-smoke.cjs
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

Resilience checks exercise a real ten-second HTTP 503 outage, one missing body
and one HTML body. The pause check takes about a minute: pause for 50 seconds,
prove old resources return 404, then require fresh downloads and continued playback.


## Compare player build settings

`tools/size.py` uses the small player's release settings: `z` for the core,
`s` for Dolby and codec support, and `3` for MP2. It records effective package
settings and enforces the existing budgets. Python 3.11 or newer is required.
Override settings for a measured comparison, without changing defaults:

```sh
python3 tools/size.py --opt-level s --codec-opt-level s --output tools/results/size-s.json
python3 tools/size.py --opt-level z --codec-opt-level z --output tools/results/size-z.json
python3 tools/size.py --opt-level 3 --codec-opt-level 3 --output tools/results/size-3.json
python3 tools/size.py --opt-level z --codec-opt-level s --output tools/results/size-lean.json
python3 tools/size.py --check-budgets
```

For CPU/output checks, build identical development harnesses, then compare them
in one fresh Node process after other builds finish:

```sh
python3 tools/profile_harness.py --profile s --profile balanced
node tools/node/opt-compare.cjs tools/results/profiles/s/rstreamkit_harness.js tools/results/profiles/balanced/rstreamkit_harness.js
```

`z`, `3`, `lean` and `fast-codecs` are other harness profile choices. The helper
uses `wasm-opt -Oz` and saves compiler settings, dependency locks and source/artifact
hashes. Comparison checks decoded PCM checksums and output lengths before timing.
CPU measurements are Node harness processing, not browser startup or frame rates.

The ABR failure check stops the higher encoder and keeps the lower encoder
advancing. It requires fallback after two failures and playback past the first
healthy segment. `RSTREAMKIT_PLAYER_DIR` can load preserved full-player artifacts
for a before/after regression check.

The Chrome suite also runs `tools/node/rendition-smoke.cjs`: a mixed HEVC/H.264 master, decoded English/Spanish tones, a late language change and a quality switch. It uses development-only harness controls and generated TS fixtures. Build the harness with `bash tools/build.sh`; run with the same `NODE_PATH` and `RSTREAMKIT_URL` settings as the other browser smoke tools.

`tools/node/catchup-smoke.cjs` is a manual, real-time Chrome check for a ten-second playback stall. Run it with the same `NODE_PATH` and `RSTREAMKIT_URL`; generate browser fixtures first. Allow five minutes. It checks return to the six-second target plus its two-second control band, a 1.05× catch-up rate, and no seek. It is kept out of the short CI suite.

`tools/node/raw-yield-smoke.cjs` compares complete continuous-stream output and measures synchronous/budgeted decoding while Chrome plays video. The larger repeated input is a scheduling stress case. `tools/node/raw-playback-smoke.cjs` generates a continuous six-second Dolby stream and checks real EOF playback plus decoded PCM. RSTREAMKIT_REPEAT_RAW=1 instead exercises the known repeated-timestamp early-end diagnostic; it is expected to fail until #16/#25 is resolved.

`tools/node/boundary-smoke.cjs` is an optional two-minute real-network check of a public 720p HLS source. It checks buffered continuity and frame counters across actual segment boundaries. The source can change, so this is outside deterministic CI. Set RSTREAMKIT_BOUNDARY_SOURCE to another compatible media playlist if needed.

`tools/node/movie-yield-smoke.cjs` compares complete MP4/MKV outputs at three range sizes and profiles synchronous/yielded decoding. `tools/node/movie-playback-smoke.cjs` checks MP4, MKV, encrypted Dolby HLS and a seek injected during a decoder yield; the Chrome suite runs these four cases together.
