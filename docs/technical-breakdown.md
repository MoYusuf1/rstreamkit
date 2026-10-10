# rstreamkit: technical breakdown and lessons learned

This is a handoff document for an AI assistant (or a developer) starting work on rstreamkit. Read it
completely before you change anything. It covers:

- what the library is, and what it deliberately is not;
- the tools and stack it uses, and how the pieces fit together;
- how to build, test, measure and release it;
- the rules the owner holds the code to;
- the hard-won lessons: bugs that took real debugging to find, and their causes.

It describes how things work, not what is open: work items live in the GitHub issue tracker. When
sources disagree, trust the tests, then the code, then prose.

Repository: <https://github.com/MoYusuf1/rstreamkit> (private for now). The library is about 9,000
lines of Rust in `src/`, plus integration tests and a test-bed crate under `tools/`.

---

## 1. What rstreamkit is

rstreamkit is a standalone, open-source (MIT OR Apache-2.0) Rust library that prepares streams and
files for playback in a web browser.

- **In:** HLS playlists with MPEG-TS or fragmented-MP4 (CMAF) segments, including AES-128 and byte
  ranges; endless raw MPEG-TS; MP4 files; Matroska (MKV) files.
- **Out:** fragmented MP4 (fMP4) that a `<video>` element plays through MediaSource Extensions (MSE).
- **Video is never decoded or re-encoded.** H.264 passes through byte for byte: it is repackaged,
  not transcoded. Only *sound the browser can't play* (AC-3, E-AC-3, MP2) is decoded, in Rust, and
  written back as FLAC, which browsers play.
- The media core is plain Rust and runs natively and in WebAssembly. The `rstreamkit` command
  (`src/main.rs`) converts files natively, like `ffmpeg -i in -c copy out.mp4`. The browser
  adapter (`mse`) compiles for wasm only. Native hosts use the core with their own I/O through two
  traits.
- Apps are clients. They plug in through traits and must not need changes inside the library.

### What it deliberately does not do

Each item is a `ponytail:` note in the module that owns it (a `ponytail:` comment marks a deliberate
simplification and names its ceiling and upgrade path; keep that convention).

- Only H.264 video. HEVC, MPEG-2 and the rest are reported as `Unsupported::Video`.
- One picture track and one sound track per movie. No subtitles.
- Decoded sound is stereo, 16-bit. 5.1 is folded down; LFE is dropped.
- MP4: no fragmented input files, and no edit lists beyond the first entry. Only the AVC, HEVC, AAC,
  AC-3 and E-AC-3 sample entries are understood.
- MKV: needs a cue index. No lacing and no content encodings.
- Dolby object/Atmos metadata is not interpreted. No DRM.
- In the browser, AES-128 uses Web Crypto. Native hosts must decrypt before the media core.
- No FFmpeg, ever, at runtime: not launched, not linked.

---

## 2. Stack and tools

### Languages and core crates

| Piece | Tool | Notes |
|---|---|---|
| Language | Rust, edition 2024. MSRV 1.88; developed on 1.99 | Everything is Rust. The only JavaScript is what wasm-bindgen generates, plus the test-bed page. |
| Browser target | `wasm32-unknown-unknown`; `wasm-bindgen` 0.2.128, `wasm-bindgen-futures`, `js-sys`, `web-sys` | `web-sys` features are opt-in per type (the list is in `Cargo.toml`), and each one costs bytes. |
| Sound decoders | `rstreamkit-ac3` (in `ac3/`), `oxideav-mp2` 0.0.10, `oxideav-core` 0.1 | Optional, behind the `sound` feature; about a quarter of a built page. `ac3/` is oxideav-ac3 0.0.11 by Mark Karpeles (MIT), kept in this repo and made twice as fast. |
| Everything else | `thiserror`, `futures-core` | No HTTP client and no URL crate: the browser provides both. |
| Reference and oracle | `ffmpeg` / `ffprobe` (n9.0.2 here) | Test fixtures, an independent decoder in tests, the benchmark baseline. Never a dependency. |

### Repository layout

```
Cargo.toml            features: default = ["sound", "vod"]; wasm-only deps under cfg(target_arch = "wasm32")
src/
  lib.rs              Transmuxer, Segment, Continuous, Output/Init/Fragment, Error/Unsupported; the timeline
  ts.rs               MPEG-TS demuxer, fed incrementally
  fmp4.rs             fMP4 writer: init (moov, incl. dac3/dec3), moof, mdat header
  avc.rs              just enough of an H.264 SPS reader: size, interlacing, pixel aspect
  sound.rs            AC-3/E-AC-3/MP2 decoding (feature sound), Dolby config parsing, FLAC writer
  hls.rs              playlists, variant choice, raw-TS detection
  cmaf.rs             fMP4 segment inspection, clock rebasing, merging a separate audio rendition
  net.rs, body.rs     host transport and output traits; reading a body without trusting its size
  live.rs, cancel.rs  live scheduling and memory policy; cancellation (compiled for wasm and tests)
  mp4.rs, mkv.rs, vod.rs   movie files (feature vod)
  mse.rs              the browser player (wasm only)
  main.rs             the `rstreamkit` command (feature vod): files and pipes in, fragmented MP4 out
ac3/                  rstreamkit-ac3: the AC-3/E-AC-3 decoder (from oxideav-ac3, MIT; LICENSE and credit kept)
tests/                pipeline, vod, vod_without_sound, continuous, flac, passthrough, cli; fixtures/
tools/                harness crate, test-bed page, Node profiler, media generator, fault server,
                      FFmpeg comparison (compare.py) and its memory-honest launcher (peak.rs)
docs/                 api.md (public contract), performance.md (measurements), issues.md, roadmap.md, this file
.github/workflows/    ci.yml: fmt, app-name guard, clippy (8 runs), tests (3 runs), MSRV, profiling
```

**Where each change belongs:**

- anything that can run natively goes in the core modules, with a test;
- policy (retries, buffer sizes, variant switching) goes in `live.rs`, which is pure and tested;
- only browser API calls go in `mse.rs` (it compiles for wasm only, so nothing there is unit-tested);
- anything specific to one app stays in that app, behind `net::Fetch` and `net::Sink`.

### Dev tools used while building

| Tool | What we used it for |
|---|---|
| `cargo fmt --check`, `cargo clippy -- -D warnings` | Hard gates. Run clippy for **both** native and `--target wasm32-unknown-unknown`, in every feature combination: native clippy never sees `mse`. |
| `cargo test` | Unit tests beside the code; integration tests that hand our output to ffmpeg. |
| `wasm-bindgen` CLI 0.2.128 | Turns the harness `.wasm` into `web` and `nodejs` packages. Its version must equal the crate's exactly. |
| `wasm-opt -Oz`, `brotli -q 11` | Measuring what a page really ships (recipe in section 3). |
| `llvm-nm -S --demangle` | Attributing wasm code bytes to crates, on an unstripped build. |
| Node (v26) | Headless wasm profiling, one fresh process per scenario. |
| A browser automation pane (Chromium 152 to 154, Linux) | The truth about MSE playback: `video.buffered`, `getVideoPlaybackQuality()`, console, long tasks. |
| Python 3 | `tools/media.py` (fixtures), `tools/server.py` (CORS, Range and fault server), `tools/compare.py` (FFmpeg comparison). |
| `ffmpeg -f framecrc` / `framemd5`, `-f s16le` | Proving two outputs decode to the same frames and the same samples. |
| `ffmpeg -benchmark` | FFmpeg's processing time (`rtime`) without its start-up and probing. |
| `tools/compare.py`, `tools/peak.rs` | The command against FFmpeg on five workloads; true peak memory of each run. |
| A scratch crate depending on the library by path | Timing each stage in-process (demux, mux, read, fragment) when there's no profiler. |
| `git archive HEAD \| tar -x -C <scratch>` | Testing a clean copy when the working tree holds someone else's uncommitted work. |
| `gh` CLI | Issues and pull requests. |

### The test bed (`tools/`)

- `tools/harness/`: a separate crate (`publish = false`, `opt-level = "s"`, LTO, `panic = "abort"`)
  that exports the library to JavaScript: `live`, `movie`, `stop`, `pause`, `resume`, `go_live`,
  `stats`, `transmux`, `demux`, `memory_bytes`, and the profiling classes `ProfileStream`,
  `ProfileSession`, `ProfileProbe` and `ProfileAudio`.
- `tools/web/index.html`: buttons for live, movie and stop; a 30-channel zap; detaching the source
  mid-append; removing a buffer while it updates; pause, resume and go-live; a codec probe; a cold
  main-thread versus worker audio benchmark. `sample()` returns buffered ranges, playback quality,
  statuses, stats and long tasks.
- `tools/server.py`: serves `tools/web` and `tools/media` with CORS (exposing `Content-Range`) and
  Range requests. It also serves `/raw.ts`, `/garbage.ts`, and scripted live providers at
  `/fault/{404,garbage,outage,frozen,no-sequence}.m3u8` (a six-segment sliding window; add `?delay=1`
  to slow a response). It contains a PES-timestamp shifter that fabricates discontinuities from a
  real segment.
- `tools/media.py`: generates every fixture with ffmpeg and records the arguments and input hashes,
  so a stale cached file can't silently change a benchmark. Nothing generated is committed.
- `tools/node/profile.cjs`: scenarios `demux`, `stream`, `push`, `index`, `movie`, `flac`. Seven
  runs in a fresh process, then median, p95 and committed wasm bytes as one JSON line.
  `tools/node/soak.cjs`: five batches of 2,000 segments; fails if committed memory grows after the
  first batch.
- `tests/fixtures/`: small real streams: H.264 with AAC; AC-3 stereo and 5.1; E-AC-3 5.1; MP2; an
  anamorphic PAL stream; interlaced 576i; HEVC; MP4 and MKV movies.

---

## 3. Build, test, measure, release

Prerequisites: Rust 1.88 or newer, the `wasm32-unknown-unknown` target, and `ffmpeg`/`ffprobe` with
libx264. Tests that decode our output skip themselves when ffmpeg is missing. The harness also needs
Node, Python 3 and the matching `wasm-bindgen` CLI.

```bash
cargo test                                       # default features (sound + vod)
cargo test --no-default-features                 # the lean core
cargo test --no-default-features --features vod  # movies, with undecodable sound reported
bash tools/build.sh                              # harness -> tools/web/pkg and tools/node/pkg
bash tools/demo.sh                               # build, generate media, serve http://127.0.0.1:8765
bash tools/profile.sh                            # fresh-process Node profiling, JSON per scenario
cargo run --release -- in.ts out.mp4             # the command (TS, .m3u8, MP4, MKV; - is a pipe)
cargo build --release --bin rstreamkit && python3 tools/compare.py   # against FFmpeg, 5 workloads
```

### Before every push (CI runs the same)

```bash
cargo fmt --check
for f in "" "--no-default-features" "--no-default-features --features sound" "--no-default-features --features vod"; do
  cargo clippy --all-targets $f -- -D warnings
  cargo clippy --target wasm32-unknown-unknown $f -- -D warnings
done
cargo test
cargo test --no-default-features
cargo test --no-default-features --features vod
cargo test --release --manifest-path ac3/Cargo.toml   # the Dolby decoder's own 558 tests
```

Put that in a `#!/usr/bin/env bash` file with `set -euo pipefail`. **fish does not word-split
`$f`**, so typed into fish the loop silently passes one argument. And a chain without a gate once
let a failing lint be followed by a commit.

CI (`.github/workflows/ci.yml`) also greps `src tests Cargo.toml` for app names. It runs an
MSRV job (`cargo check` with Rust 1.88, native and wasm, all and no features) and a non-blocking
profiling job. Run the checks locally rather than waiting on CI.

### Cargo features

| Feature | Default | Adds | Without it |
|---|---|---|---|
| `sound` | yes | AC-3, E-AC-3 and MP2 decoders | That sound is passed through where the platform plays it, otherwise reported as `Unsupported::Sound`. FLAC writing is unchanged. |
| `vod` | yes | `mp4`, `mkv`, `vod`; the `rstreamkit` command; in the browser `mse::probe` and `mse::play_movie` | No movie files and no command. Live playback is unchanged. |

`mse` has no feature of its own: `target_arch = "wasm32"` selects it.

### Measuring

- **Committed wasm memory, not live heap.** Wasm linear memory never shrinks, so
  `memory.buffer.byteLength` after the work is the cost the user pays.
- **One fresh process per scenario**, so allocator state from one doesn't leak into the next.
- **Medians and p95, never means.** Seven or more runs. When comparing A with B, alternate their
  order. Record CPU, OS and every tool version with each result.
- **Shipped size, not `.wasm` size.** Build with `opt-level = "z"`, LTO and `panic = "abort"`, run
  `wasm-bindgen`, then
  `wasm-opt -Oz --enable-bulk-memory --enable-nontrapping-float-to-int --enable-sign-ext --enable-mutable-globals`,
  then `brotli -q 11`. Remeasure after any dependency or `web-sys` feature change.
- **Long tasks and dropped frames in a real browser**, because a fast average can still freeze the
  page.

Reference measurements. Machine: AMD Ryzen 7 5700, Linux, Rust 1.99, Node 26.10, harness release
wasm. Input: one 6-second 1080p H.264 segment at 8 Mbit/s.

| Scenario | Input | Committed wasm memory | Median time |
|---|---:|---:|---:|
| AAC, streamed in 16 KiB chunks (what the browser player does) | 6.87 MB | 8.52 MB (1.24×) | 7.4 ms |
| AAC, whole segment pushed at once | 6.87 MB | 22.2 MB (3.2×) | 8.2 ms |
| AC-3 5.1 decoded to FLAC, streamed | 7.19 MB | 10.4 MB (1.45×) | 85 ms |
| 5 batches of 2,000 AAC segments | | 1,900,544 bytes in every batch | 368 to 399 ms a batch |

**Against FFmpeg.** `tools/compare.py` races the `rstreamkit` command against FFmpeg n9.0.2 on five
workloads. Both run as fresh processes, so start-up and file reading and writing are included. One
warm-up, then timed runs in alternating order; medians. FFmpeg's own `-benchmark` rtime gives its
processing time without start-up and probing, and our whole time is compared with that too. Every
workload's two outputs are checked: identical pictures (`framecrc`), identical copied sound, and
decoded sound of the same length. Full method and the before/after of each change are in
`docs/performance.md`.

| Workload | rstreamkit | FFmpeg (processing only) | Peak memory, ours / FFmpeg |
|---|---:|---:|---:|
| 6 s 1080p TS, copied | 7.9 ms | 71.9 ms (9.0 ms) | 12.7 / 68.1 MB |
| 10 min 1080p TS (687 MB), copied | 334 ms | 831 ms (715 ms) | 17.3 / 67.7 MB |
| 30 min 1080p MP4 (2 GB), copied | 781 ms | 1,442 ms (1,241 ms) | 16.3 / 66.4 MB |
| 6 s 1080p TS, AC-3 5.1 to stereo FLAC | 37.6 ms | 74.3 ms (12.0 ms) | 13.2 / 71.8 MB |
| 30 min MKV, AC-3 5.1 to stereo FLAC | 8,437 ms | 2,151 ms (2,077 ms) | 9.7 / 63.8 MB |

Repackaging beats even FFmpeg's processing-only time, in a quarter of the memory or less. Decoding
long AC-3 is still about four times slower, even after the decoder was made twice as fast (lesson
5.9): it decodes all six channels before mixing down. Read the results narrowly: they say nothing about in-process
libavformat, FFmpeg built for wasm, or other machines.

### Releasing

- Public API changes start with an issue, and breaking changes are called out in the pull request.
- Before 1.0, an incompatible change needs a new minor version and a migration note in
  `CHANGELOG.md`. Patch releases keep the API.
- Run `cargo publish --dry-run`. docs.rs builds for `wasm32-unknown-unknown`
  (`[package.metadata.docs.rs]`).
- Tag only a commit that has passed every check above.

---

## 4. How it works (the heart of the library)

### Data flow

```
LIVE (HLS)
  playlist ── hls::parse ──▶ Media { segments }, Variants, Renditions
                              live::Window  which segment is next (sliding windows, sequence resets)
                              live::Abr     which variant (throughput, smoothed)
  segment chunks ──▶ Segment::feed ──▶ ts::Demuxer ──▶ Demuxed ──▶ Transmuxer::finish ──▶ Output
  Output = { init (when something changed), fragments: [moof + mdat for each track], skipped_audio }
  AC-3 / E-AC-3 / MP2 ──▶ sound::Decoder ──▶ PCM ──▶ sound::flac_frame ──▶ FLAC samples in the fMP4
  CMAF / fMP4 segments ──▶ cmaf::Rebaser (rewrites decode times in place; no transmuxing)
  endless raw TS ──▶ Continuous (keyframe-bounded fragments, bounded memory)

MOVIES (feature vod)
  range reads ──▶ mp4::parse / mkv::Probe ──▶ Index ──▶ vod::Movie ──▶ Session
  Session: range() ──▶ fetch those bytes ──▶ push() ──▶ one fragment. A seek is a new Session.

BROWSER (wasm only)
  mse::start / mse::play_movie ──▶ MediaSource + SourceBuffer ──▶ <video>
  each append lends a view of wasm memory (no JS copy) and waits for the buffer's `updateend`
```

### Modules

| File | Role | Compiled |
|---|---|---|
| `lib.rs` | `Transmuxer`, `Segment`, `Continuous`, `Output`, `Init`, `Fragment`, `Error`, `Unsupported`; timeline logic | always |
| `ts.rs` | PAT/PMT, PES reassembly, Annex-B to AVCC written straight into the output buffer, ADTS, AC-3/E-AC-3/MP2 frames; `feed`, `take_fragment`, `finish` | always |
| `fmp4.rs` | init segment, `moof`, `mdat_header`; video timescale 90 kHz, audio timescale = sample rate; track 1 video, track 2 audio | always |
| `avc.rs` | SPS reader: picture size, interlacing, pixel aspect | always |
| `sound.rs` | decoders (module `decode`, stubbed without `sound`), `dolby_config`, stereo downmix, FLAC writer | always |
| `hls.rs` | playlist parsing, `pick_variant_for`, `looks_like_ts`, `describe_non_playlist` | always |
| `cmaf.rs` | init inspection, `Rebaser`, `merge_init`, `audio_fragment` | always |
| `net.rs` | `Fetch`, `Response`, `Body`, `FetchError`, `Sink`, `download_segment`, `append_fragments` | always |
| `body.rs` | `read_each`, `read_capped` | always |
| `live.rs` | `Window`, `Abr`, `buffer_limits`, `gap_target`, `retry_delay` | wasm or test |
| `cancel.rs` | `cancellable` | wasm or test |
| `mp4.rs`, `mkv.rs` | file indexes (`Index`, `Cursor`) | `vod` |
| `vod.rs` | `Movie`, `Session`, `Verdict` (native, ours, unsupported), the fragmenter | `vod` |
| `mse.rs` | `Player`, `start`, `Status`, `Stats`, `Direct`, `can_play`, `plays_hls_natively`; with `vod`, `probe` and `play_movie` | wasm |

### Decisions that shape the code

- **A segment becomes one fragment per track.** `Output.fragments` holds a `moof` + `mdat` for each
  track that has samples, pictures first. The picture bytes are written once, into the buffer the
  demuxer filled, and are never copied to interleave the sound. The browser accepts this in one
  SourceBuffer. `Output::fragment()` concatenates them (a copy) for tests and files.
- **Room for the header.** Segment buffers reserve 8 bytes at the front (`MDAT_HEADER`), so the
  `mdat` header is written in place once the length is known.
- **Timeline.** PES timestamps are 33 bits and wrap; each is unwrapped to the value nearest the last
  one seen. Video and audio keep their own base (`vbase`, `abase`) and end (`vend`, `aend`). A
  segment starting more than `JUMP` (2 s at 90 kHz) from where the last one ended is a new timeline
  (restart, ad splice, missing tag), and each track carries on exactly where it stopped. If the two
  clocks drift more than 20 ms apart over several splices, video is nudged toward audio, by at most
  2% of that fragment's video duration.
- **New init segments.** One goes out at start, and again when the SPS or PPS changes, when sound
  first appears or changes kind or rate, or when a Dolby configuration changes.
- **Sound policy.** AAC passes through. AC-3, E-AC-3 and MP2 are decoded to stereo 16-bit FLAC.
  `Transmuxer::passthrough_ac3(true)` is opt-in: it forwards AC-3 and simple E-AC-3 untouched, for
  platforms whose MSE supports them. Probe `MediaSource.isTypeSupported` first. In the browser,
  `finish_yielded` yields to the event loop every 8 audio frames.
- **FLAC writer.** It uses fixed predictors of order 0 to 2 with a Rice-parameter search, chosen by
  exact bit cost, and falls back to verbatim per channel when compression loses. The CRC16 is
  table-driven and the bit writer keeps the unfinished byte in a register. Real programme audio
  comes out near 30% of raw PCM.
- **Errors are typed.** `Error` and `Unsupported` are `#[non_exhaustive]`, and `Unsupported`'s
  `Display` is a sentence for the viewer. `mse::Status` is one of `Playing`, `Buffering`,
  `Reconnecting`, `Note`, `Unsupported`, `Ended` or `Failed`. Apps match on variants, never on
  strings.
- **Transport is a trait.** The call is `Fetch::get(url, Option<Range<u64>>) -> Response`, and
  dropping the future or its body cancels the request. `FetchError::{Temporary, Permanent}` drives
  retries: `retry_delay` gives 0, 1, 2, 4, 8, 8 and 8 s, then the player gives up. `mse::Direct` is
  the plain browser `fetch`, and it aborts on drop.
- **Dropping a player cancels everything.** `cancellable` checks a stop flag every time a suspended
  future is polled. Dropping the inner future releases body readers, event listeners and the
  MediaSource.
- **Live buffers are bounded by bytes.** `buffer_limits` turns a 32 MiB budget into seconds using
  the bitrate actually appended (decoded sound included). It keeps at most 30 s ahead and a bounded
  amount behind. An append that throws trims old ranges and retries once.
- **Adaptive bitrate.** `Abr` smooths throughput (0.7 old, 0.3 new) and picks the best variant
  below 75% of it. It steps down at once, and steps up only after three healthy checks in a row.

---

## 5. Lessons learned (the expensive ones)

### 5.1 A 6 MB segment cost 35 MB of memory that never came back

**Symptom:** after one 6.1 MB live segment, the page held 34.9 MB of wasm memory (5.5× the segment)
and never released it.

**Cause:** wasm linear memory only grows. The segment was downloaded into one buffer, demuxed into
more and muxed into another, with vectors doubling as they grew. Each step raised the high-water
mark, and freed holes were too small for the next, larger request.

**Fix:**

- size the buffer once, from `Content-Length`, with 8 bytes reserved for the `mdat` header;
- demux as bytes arrive (`Segment::feed`) and write the pictures straight into that buffer as AVCC;
- give each track its own `moof` + `mdat`, so nothing is interleaved into a bigger buffer;
- lend the bytes to `appendBuffer` as a view of wasm memory, and wait for `updateend`.

**Verification:** a fresh Node process now commits 8.52 MB for a 6.87 MB segment (1.24×). The soak
holds 1,900,544 bytes across 10,000 segments. The tests are
`a_segment_fed_in_pieces_is_the_same_as_one_pushed_whole` and
`a_segment_dropped_unfinished_leaves_no_trace`.

**Lesson:** in wasm, the high-water mark is the cost. Measure committed memory in a fresh process,
and design so each byte is written once, in its final place. An API that takes a whole segment as
`&[u8]` makes its caller hold that segment in wasm memory as well: 3.2× instead of 1.24×.

### 5.2 Playback froze at splices that were perfectly valid

**Symptom:** live channels froze in Chrome at ad breaks and encoder restarts. Every fragment was
valid, and ffmpeg played the output without complaint.

**Cause:** at a timestamp jump, both tracks were re-based to one point. Sound and picture never end
at exactly the same moment, so one track was left with a hole of up to about 90 ms. Chrome waits at
a hole; it does not skip it.

**Fix:** each track has its own base and end, and after a jump each carries on exactly where it
stopped. A bounded correction (2% of a fragment) pulls the clocks back together when they drift.

**Verification:** `each_track_carries_on_exactly_where_it_stopped_after_a_jump` and
`a_clock_restarting_below_the_sent_media_still_plays_on`, plus browser runs against the fault
server's shifted segments.

**Lesson:** valid bytes don't mean it plays. Test splices in a real browser and read
`video.buffered` for holes. Fabricate discontinuities by shifting the PES timestamps of a real
segment. A verbatim repeat whose length equals `JUMP` doesn't produce a jump at all.

### 5.3 Our AC-3 "disagreed" with FFmpeg, and the first fix made it wrong

**Symptom:** CI failed. Decoded 5.1 AC-3 matched FFmpeg's to only 17 dB, against about 40 dB for
stereo sources.

**Cause:** our output was right. Our decoder folds 5.1 to stereo the way the A/52 specification
says (section 7.8); the older FFmpeg on the CI runner mixes differently. The first "fix" tuned our
mix to match FFmpeg, which made it wrong against the specification.

**Fix:** that change was reverted. The test now compares 5.1 sound by correlation and level rather
than sample by sample.

**Lesson:** a reference implementation is evidence, not the specification. When you disagree with
it, read the specification before you change code that might be correct.

### 5.4 A byte-range playlist lost its place

**Symptom:** a CMAF playlist that addresses every segment as a byte range of one file would not
play through.

**Cause:** the player recognised a segment by its URI, and here every segment has the same URI.

**Fix:** a segment's identity is its sequence number, URI and byte range together (`live::Window`).

**Verification:** the run now plays all 720 frames across 24.021 seconds, with no dropped or
corrupted frames and one buffered range.

**Lesson:** decide what makes a thing unique from the specification (RFC 8216 allows many ranges of
one resource), not from the common case.

### 5.5 A valid 1080p stream was rejected

**Symptom:** a valid 1080p channel was refused as broken.

**Cause:** a general-purpose H.264 SPS parser failed on oddities in the trailing VUI data, which
this library never needs.

**Fix:** `avc.rs` reads only up to the cropping fields, plus what it needs for interlacing and pixel
aspect, and stops.

**Lesson:** read only what you need from an untrusted bitstream. Every field you parse is another
way to fail.

### 5.6 A raw stream was called a bad playlist

**Symptom:** a raw MPEG-TS address served with an unexpected content type produced "bad playlist:
not text".

**Cause:** the player trusted `Content-Type` to tell playlists from streams.

**Fix:** look at the bytes first. `hls::looks_like_ts` checks for TS sync bytes every 188 bytes,
whatever the header says.

**Lesson:** media servers mislabel things. Python's `http.server` labels `.ts` as a Qt Linguist
file, which makes it a handy test.

### 5.7 "Four times faster than FFmpeg" was mostly FFmpeg starting up

**Symptom:** a 6-second remux took 14.5 ms here and 60 to 74 ms in FFmpeg, and read as a 4×
advantage.

**Cause:** both tools were timed as whole processes. FFmpeg's own `-benchmark` showed its
processing took 9.0 to 9.5 ms; the rest was start-up and probing the input. On processing alone,
FFmpeg was faster than our whole command.

**Fix:** time our stages in-process (demuxing was 4.9 of the 14.5 ms) and speed up what dominated:

- find H.264 start codes eight bytes at a time and jump to the first zero byte (a start code
  begins with one, and compressed pictures have few);
- route each 188-byte packet through a short list, not a SipHash map lookup per packet;
- start a continuous stream's next buffer at the last one's capacity, so it doesn't grow, copy and
  fault in fresh pages again for every group of pictures;
- read, convert and write on three threads joined by two-piece queues.

**Verification:** the command now takes 7.9 ms against FFmpeg's 9.0 ms processing alone (71.9 ms
whole), and 334 ms against 715 ms on a 10-minute recording. Output bytes are unchanged, and
`the_word_at_a_time_scan_finds_what_a_byte_at_a_time_one_does` checks the scan against a plain one.

**Lesson:** "faster than X" needs the same scope on both sides. Separate start-up from processing,
report the comparison you lose as well as the one you win, then go and win it.

### 5.8 A 2 GB movie spent most of its time in page faults

**Symptom:** the command was slower than FFmpeg on a 30-minute MP4 (1,768 against 1,493 ms),
although its parsing and fragmenting were cheap.

**Cause:** it read 4 MiB at a time into a fresh buffer and built a fresh 4 MiB fragment. Freed
buffers that large make glibc return the memory to the system, so every new one paid for all its
pages again. In-process, 4 MiB pieces took 930 ms and 1 MiB pieces 171 ms; raising glibc's trim
and mmap thresholds (`MALLOC_TRIM_THRESHOLD_`, `MALLOC_MMAP_THRESHOLD_`) took the same 4 MiB run
from 957 to 226 ms without changing a line of code.

**Fix:** one read buffer, reused for every 1 MiB piece, and writing on its own thread.

**Verification:** 781 ms against FFmpeg's 1,442 ms, with a `cat` of the same file into RAM taking
615 to 738 ms. The command is now close to the cost of moving the bytes at all.

**Lesson:** a "big buffers are faster" intuition can be backwards. Allocation size interacts with
the allocator's trim policy, so measure several sizes, and reuse buffers in hot loops.

### 5.9 The Dolby decoder spent a third of its time recomputing constants

**Symptom:** converting 30 minutes of 5.1 AC-3 took 14.3 s here and 2.3 s in FFmpeg. In-process,
the decoder (then the oxideav-ac3 crate) was 86% of our time.

**Cause:** sampled with pprof (no `perf` on the machine):

- 23% was `2f32.powi(-e)`, a loop of multiplications, run for every coefficient;
- 10% was a twiddle table its comment called cached, rebuilt (128 cosines and 128 sines) on every
  inverse transform;
- the FFT recomputed its twiddles and bit reversal in every group;
- the downmix re-decided its taps for every sample, and about ten `std::env::var` lookups ran per
  block.

**Fix:** with the maintainer's agreement, the MIT-licensed decoder was taken into the repo as `ac3/`
(`rstreamkit-ac3`), its LICENSE kept and its author credited. Then:

- powers of two are built from their exponent bits;
- the twiddles are tables built once, replaying the original recurrence so every value is the same;
- the bit reversal is a table;
- the downmix plans its taps once a call;
- the trace switches are read once.

No arithmetic changed order.

**Verification:** decoding is twice as fast (40.3 to 20.0 ms for 6 s of 5.1), the command takes 8.4 s
instead of 14.3 s for the 30-minute movie, and decoded PCM is byte-identical to the original's on
stereo, 5.1, E-AC-3 and programme audio. `the_absorbed_dolby_decoder_still_decodes_exactly_what_the_original_did`
pins checksums of the original's output, and the crate's own 558 tests pass.

**Lesson:** profile code you depend on before deciding it is the floor. Read a sampling profiler's
inclusive view: with inlining, the self-time labels put loop bodies under whatever was inlined into
them (here, `cos` at 32% after every cosine had been cached).

### 5.10 Browser and MSE facts (verified on Chromium, Linux)

- AC-3 and E-AC-3 are **not** supported by MSE or WebCodecs there. FLAC, Opus, H.264, AAC and
  `audio/mpeg` are. HEVC is not. Decoding to FLAC is what makes AC-3 sources audible.
- `canPlayType` for HLS can say "maybe" and native HLS still fail, for example on a discontinuity
  without a tag. Treat it as permission to try, with a fallback, never as proof.
- Await `updateend`, which fires after success, error and abort alike, instead of polling. The
  source can close, or the buffer be removed, while `updating` is true. Settle the wait with an
  error in both cases.
- CORS must expose `Content-Range`, or the page can't see a ranged response's total size.
- Hardware decoding is a browser preference, not a guarantee. MSE accepting a type does not mean
  playback is smooth.
- A hidden browser pane pauses a muted video. To test there, unmute it (set a tiny volume) and call
  `play()`.
- This evidence comes from Chromium on Linux. Before assuming the same elsewhere, run the test bed's
  codec probe and playback checks on that browser.

### 5.11 Where the time goes

- Don't decode video. Passing it through costs almost nothing: 7.4 ms for six seconds of 1080p in
  wasm, 2.4 ms natively.
- The real CPU cost is sound. Natively, 6 s of AC-3 5.1 took 40.6 ms to decode, then 20.0 ms after
  lesson 5.9 (our FLAC encoding adds 6.6 ms). The decoder decodes all six channels and mixes down
  afterwards, so 5.1 costs several times what stereo does. Its optimisation level wasn't a lever.
  Mixing down before the inverse transforms, SIMD in the transforms, running it off the main thread,
  or not running it at all (passthrough) are.
- Yield between audio batches in the browser, so decoding is never one long task.
- Profiling found two FLAC hot spots. A bitwise CRC16 over every byte was cut from 11 to 4 ms by a
  table. Updating the output `Vec` every few bits was replaced by a register, giving 29% less
  encoder CPU with byte-identical output.
- Cheap structure beats clever structure. Merging two per-track lists already in file order beat
  sorting a third table, and halved the MP4 index (24 bytes a sample: size and keyframe flag share
  one field). A Matroska cue costs 16 bytes.
- Dropping the HTTP-client and URL crates in favour of the browser's `fetch` and `URL` cut the wasm
  dependency count from 87 to 31.

### 5.12 Measuring honestly

- Most guesses about where time and memory went were wrong until measured. Measure before and after,
  each in a fresh process, with tool versions written down.
- **x264 constant-bitrate output is not byte-deterministic.** Identical commands produced 6,866,512
  and 6,912,760 bytes. Keep the exact input you benchmark and record its size and hash;
  regenerating it doesn't give you the same input.
- When comparing with another tool, prove equal output (frame hashes, decoded samples) and state
  what the comparison doesn't cover. A narrow claim that is true outlasts a broad one.
- **A child process's peak memory includes its parent's**, until it starts the program: Linux
  counts the copy it began as. Spawned from a Python script holding hundreds of megabytes, both
  tools "peaked" at exactly the same 376 MB. Launch measured programs from something tiny (`tools/peak.rs`).
- Hashing every decoded 1080p frame with MD5 (`framemd5`) takes minutes on long files; `framecrc`
  checks the same thing much faster.
- Synthetic inputs carry their own artefacts. A movie made by looping a clip overlaps its sound by
  10 ms at every seam, which FFmpeg trims and this library keeps. Encode benchmark inputs straight
  through.

### 5.13 Rust and API design

- Replace string protocols with types. An early `convert:` string prefix became the `Unsupported`
  enum, which apps can match.
- Mark public enums and structs `#[non_exhaustive]` where growth is foreseeable. The public
  low-level modules (`ts`, `fmp4`, `sound`, `mp4`, `mkv`, `body`, `avc`) are API too.
- Never hold a `RefCell` borrow across `.await` (clippy's `await_holding_refcell_ref` is right).
  That is why `Segment` owns only its demuxer and is finished through `Transmuxer::finish(segment)`.
- An async fn in a public trait needs `#[allow(async_fn_in_trait)]`. It is deliberately not `Send`,
  because browser values aren't.
- Cancellation is dropping. Make every long operation release its resources when its future is
  dropped, and test that it does.
- Keep policy out of the library. It reports `Unsupported`; the app decides whether to show a
  message, play without sound or hand the stream elsewhere.

### 5.14 Working method

- Write the lint-and-test script once, gate it with `set -e`, and print the compiler's errors when it
  fails.
- When the checkout holds someone else's uncommitted work, test a clean copy made with
  `git archive HEAD`, stage only your own files by name, and never stash, reset or discard theirs.
- Edit repetitive text with scripts that fail loudly when a pattern doesn't match.
- `grep -r PATTERN a b` exits with status 2 when `b` is missing, even if it found a match in `a`, so
  `if grep ...` silently stops guarding. Keep guard paths in step with the tree.
- Never `pkill -f PATTERN` when the pattern also appears in your own command line: it kills the
  shell you are typing into. Use the process ID.
- Keep module headers honest. When a feature lands, update the header and its `ponytail:` note in the
  same change. A stale header misleads the next reader more than having no header.

---

## 6. Product and code rules (non-negotiable)

- **The library knows no app.** No app names, proxy headers, URL schemes or credentials. Apps plug
  in through `net::Fetch` and `net::Sink` and own all policy. CI fails if an app name appears in
  `src`, `tests` or `Cargo.toml`. Apps conform to the library, not the reverse.
- **Ask the browser first.** Don't rebuild what the platform provides: `URL`, `fetch`, Web Crypto,
  `MediaSource.isTypeSupported`, native HLS.
- **No FFmpeg at runtime, and never copy or translate FFmpeg source** (LGPL/GPL). Work from the
  specifications: ISO/IEC 14496 for MP4 and AVC, RFC 8216 for HLS, RFC 9639 for FLAC, ATSC A/52
  and ETSI TS 102 366 for AC-3 and E-AC-3.
- **Lean and fast.** Heavy capabilities are Cargo features. Every dependency must earn its wasm
  bytes. Don't poll; wait on events. Write each byte once.
- **Unsupported is a report, not a workaround.** There is no software video decoder in wasm.
- **Testable by default.** Logic that can run natively lives outside `mse`, with a test.
- **`ac3/` is borrowed work.** It is oxideav-ac3 by Mark Karpeles under the MIT licence: keep its
  `LICENSE` and the credits in `ac3/README.md` and the main README. Changes there must keep decoded
  output identical (a checksum test enforces it) unless a change is meant to alter it, and then the
  checksums, the README's change list and the changelog are updated together.
- **The project had an earlier name, and it is retired.** If you meet it in a comment, doc or
  commit message, replace it with rstreamkit.
- **The repository is private for now.** Don't publish it or change its visibility.
- Licensing is MIT OR Apache-2.0. Pull requests are the default path; the maintainer may push
  straight to `main`.

---

## 7. Starting work: a checklist for the next AI

1. Read this file, then `docs/api.md` (the public contract) and `docs/performance.md` (methods and
   numbers). Then skim `src/lib.rs`, `src/ts.rs`, `src/fmp4.rs`, `src/live.rs` and the top of
   `src/mse.rs`.
2. Run `cargo test`, then convert a fixture natively:
   `cargo run --release -- tests/fixtures/movie_ac3.mkv /tmp/out.mp4`. Then run `bash tools/demo.sh`,
   open `http://127.0.0.1:8765` and play `/media/live.m3u8`, `/media/ac3.m3u8` and `/media/movie.mp4`.
3. Before a change: decide where it belongs (section 2), and find the test that covers that area.
4. After a change:
   - run the pre-push checks in section 3;
   - verify behaviour only a browser can show on the test bed, including the `/fault/*` playlists;
   - for memory or speed changes, measure before and after in fresh processes.
5. Done means:
   - every check passes for every feature combination, native and wasm;
   - new logic has a native test;
   - public API changes are reflected in `docs/api.md` and `CHANGELOG.md`;
   - module headers and `ponytail:` notes still tell the truth;
   - no app name appears in the library source.

| Task | Where | Then |
|---|---|---|
| A new TS stream type | `ts.rs` (PMT handling, `Demuxed`), `lib.rs::assemble`, `fmp4.rs::AudioTrack` | add a fixture and a pipeline test judged by ffmpeg |
| A new HLS tag | `hls.rs::parse`; act on it in `live.rs` or `mse.rs` | parser unit test, plus a `live` test if timing matters |
| Buffering or retry policy | `live.rs` | pure; no browser needed to test it |
| A new decoder | `sound.rs` (`decode` module and `Kind`); optional dependency under `sound` | compare with ffmpeg by correlation; keep the feature-off stub |
| Another movie container | an `Index` with `cursor`, `range`, `frames`, `done`, wired into `vod::Movie` | `tests/vod.rs` plays the result through ffmpeg |
| A public type | `lib.rs`, `net.rs`, `mse.rs` | `#[non_exhaustive]` where it can grow; update `docs/api.md` and `CHANGELOG.md` |

### Glossary

- **fMP4:** fragmented MP4. One *init segment* (`moov`: codecs, sizes, timescales), then repeating
  fragments, each a `moof` (sample sizes, durations, decode time `tfdt`) followed by an `mdat` (the
  bytes).
- **MSE:** MediaSource Extensions. JavaScript hands fMP4 to a `SourceBuffer`, and the browser
  decodes it.
- **MPEG-TS:** 188-byte packets. The **PAT** and **PMT** list the streams. **PES** packets carry
  frames. **PTS** and **DTS** are display and decode times, in 90 kHz ticks, 33 bits wide.
- **Annex-B / AVCC:** H.264 with start codes, or with length prefixes. MP4 needs AVCC.
- **ADTS:** the framing AAC uses in TS. MP4 needs raw AAC plus a two-byte config.
- **HLS:** HTTP Live Streaming. A `.m3u8` playlist lists the segments. **CMAF** segments are already
  fMP4.
- **Discontinuity:** the timeline jumps (an ad splice, an encoder restart); the player must glue it.
- **Passthrough:** forwarding compressed sound unchanged instead of decoding it.

### The public API in one paragraph

`Transmuxer::default().push(&segment)` (or `Segment::feed` then `Transmuxer::finish`) returns
`Output { init, fragments, skipped_audio }`. Append `init` once, then each fragment's `moof` and
`mdat`. `Continuous` does the same for endless TS. `hls::parse` reads playlists. With `vod`,
`vod::Movie` and `Session` serve indexed files and seeking. `cmaf::Rebaser` rebases fMP4 clocks.
`net::{Fetch, Sink}` let any host supply its I/O. In the browser,
`mse::start(video, playlist_url, fetch, partial, decode_sound, report)` returns a `Player` (`pause`,
`resume`, `seek`, `go_live`, `set_variant`, `set_audio_track`, `stats`; drop it to stop) and reports
`Status`. Movies use `mse::probe`, then `mse::play_movie`. `docs/api.md` is the full contract.
