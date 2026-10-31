# Performance direction

The target is efficient streaming in native apps and browsers: startup latency, channel
switching, CPU time, main-thread stalls, committed wasm memory, browser buffer memory
and download size. Rust alone does not
establish superiority over FFmpeg. Compare equivalent operations and output quality;
remuxing H.264 is not a fair comparison with decoding or encoding it.

## Player baseline (2026-10-10)

The new `tools/player` entry point exports playback controls, without benchmark
exports. Rust 1.99.0, wasm-bindgen 0.2.128, wasm-opt 130 (`-Oz`), Brotli 1.2.0
(`-q 11`), release `opt-level=s`, LTO, one codegen unit and aborting panics.
Two builds produced identical artifact hashes and sizes for all four feature sets.

| Build | Features | Raw Wasm + JS bytes | Brotli Wasm + JS bytes | Initial download budget |
| --- | --- | ---: | ---: | ---: |
| Live | No optional decoders or movie readers | 295,490 | 108,769 | 115,000 |
| Live + sound | AC-3, E-AC-3 and MP2 fallback | 546,110 | 206,708 | 220,000 |
| Live + movies | MP4/MKV, without optional sound decoders | 359,382 | 131,778 | 140,000 |
| Full | Sound and movies | 610,022 | 228,963 | 245,000 |

These are the first player baselines, not a measured reduction against the old
benchmark harness. Feature removal changes coverage. Host HTML, media and HTTP
headers are excluded; generated JS is included without an extra minifier.
Run `python3 tools/size.py --check-budgets` after fetching dependencies. JSON keeps
source/artifact hashes, tool versions and the resolved lockfile; see tools/README.md.

Browser check: Linux, Ryzen 7 5700, in-app Chromium 155, visible tab. The 24-second
320×180 AAC control completed with 724 reported frames and no drops. The full
player's six-second 1920×1080/30 AC-3 fixture completed with 180 frames, no drops,
186.3 ms to the first presented frame, no observed long tasks and 10,354,688
committed Wasm bytes. The AC-3 fixture SHA-256 is
`a37d796f069bb82a4ca4dffe62e95180822abe85c5eaf4f38956049f517a0338`.
Movie-only playback also advanced with no drops in a short 320×180 MP4 check.
Wasm memory excludes decoder, GPU and JS allocations. These are short checks,
not sustained performance results or a Chrome/Edge/Safari acceptance matrix.

This run exposed a browser-only decoder regression: cached trace switches used
`std::env::vars_os()`, which panics on browser Wasm. Trace switches now remain off
on that target; native behavior is unchanged. A required CI smoke check decodes
the checked-in AC-3, E-AC-3 and MP2 fixtures in Wasm, encodes FLAC and packages them.
All 558 native decoder tests pass (two other tests remain ignored).

Fresh Node harness measurements: seven iterations, six-second AC-3 streamed
packaging median 45.020 ms, maximum 56.151 ms; programme FLAC median 8.591 ms,
maximum 10.127 ms, 237,119 output bytes versus 798,720 PCM bytes. The fixture setup
and initial programme decoding are excluded from FLAC timing. These shared-machine
samples do not establish a new speedup or an equivalent FFmpeg comparison.

MediaCapabilities predicts H.264 4K60 support at 15, 25 and 40 Mbps here, but reports
`powerEfficient=false`. A prediction is not proof of hardware decoding or smooth
playback. Actual 4K fixtures, longer runs and Edge/Safari checks remain open.

## Buffer and 4K follow-up (2026-10-10)

Same Linux/Ryzen 7 5700/in-app Chromium 155 setup, full player, visible tab and
localhost server. `python3 tools/4k.py --mbps 40` generates 18 seconds of synthetic
3840×2160/60, 8-bit 4:2:0 H.264 High level 5.2 + stereo AAC, in three six-second
segments. Encoding and CPU-heavy checks finished before browser measurements.
The playlist SHA-256 is
`d98985881c25d187a25707ccebb4e7e0d6630a7d1ae96579cfde26f806fdcb78`;
its JSON sidecar records all segment hashes and the encoder/probe settings.

| Measure | Original player, one run | Final player, two fresh-page runs |
| --- | ---: | ---: |
| Frames accounted for | 1,080 | 1,080 each |
| Dropped frames | 0 | 0 each |
| Waiting events after first frame | Not recorded | 0 each |
| Committed Wasm bytes | 70,909,952 | 35,848,192–36,700,160 (about 49% less) |
| First presented frame | 195.7 ms | 160.5–180.8 ms |
| Full player, Brotli Wasm + JS | 228,963 bytes | 229,337 bytes (+374, about 0.16%) |

The changed policy reserves space for the next append, removes the two-segment
ahead floor, gates speculative downloads by estimated bytes and caps their actual
response bodies. Merged external audio is included in the estimate; bitrate spikes
take effect immediately. Appended Rust output is freed before buffer waits. A
clock-based timer wakes near the threshold, with half a second of minimum startup
headroom. Finite playlists now continue after a throttle instead of ending with
unconsumed segments.

Keep one segment behind for video dependencies. An experiment trimming within the
current keyframe group removed future dependent frames, so that approach was
rejected. Long keyframe groups and oversized segments can exceed the approximate
32 MiB compressed-media target. Browser coded buffers, decoded surfaces, JS and GPU
memory are separate from the Wasm figures above. This result measures our own
before/after player; it does not compare FFmpeg, prove hardware decoding or establish
30-minute/four-hour acceptance. Edge and Safari remain untested here.

Current compressed build totals are live 109,318, live+sound 207,114, live+movies
132,204 and full 229,337 bytes. All four review budgets pass. CI now checks them
using the checksum-pinned official Binaryen 130 build and uploads the report;
the CI run itself still needs verification on GitHub.

The browser reports worker MSE available, ManagedMediaSource unavailable and
WebCodecs available on this installation. These are feature checks, not enabled
backends. Technology choices and their costs are recorded in docs/roadmap.md.

## Playlist lookup follow-up (2026-10-10)

The shared scheduler searched from the start of a playlist after every segment.
It now checks the segment's sequence offset and full identity first. Normal
unchanged playlists take constant time per lookup, so a full walk changes from
O(n²) to O(n). Broken numbering, changed URLs and byte ranges retain the existing
recovery search. This adds no dependency or playlist index allocation.

| Measure | Before | After |
| --- | ---: | ---: |
| Walk 20,000 segment anchors, median of 7 runs | 120.452 ms | 0.345 ms |
| Automated real-Chrome playback regressions | 0 | 5 |
| Full player, Brotli Wasm + JS | 229,337 bytes | 229,378 bytes (+41) |

Native Rust 1.99.0 release build, no optional features, Ryzen 7 5700/Linux.
Playlist parsing is outside the timing; URI cloning and consumption are included.
The synthetic playlist has unique URLs and consecutive sequence numbers.
This measures scheduler lookup only, not total playback or video processing.
Other work in the browser loop still scans playlist durations; this change does
not make that whole loop linear. Broken-provider fallback remains O(n) per lookup.

Repeat: `cargo test --release --no-default-features playlist_walk_measurement -- --ignored --nocapture`.
The benchmark is ignored in normal tests to avoid timing work in CI.
The five Playwright checks use real Chrome and small local inputs, including
40-second finite HLS at 4× playback. They verify completion, Dolby fallback,
finite-input errors, stop/restart and live recovery, not frame-rate performance.
See tools/README.md for repeat commands and the remaining Safari/Edge limits.
All four compressed-size budgets pass: live 109,385, live+sound 207,071,
live+movies 132,302 and full 229,378 bytes, using the same size recipe above.

## Work in order

1. Preserve the reproducible Node and browser harness (#38). Run each memory scenario
   in a fresh process; report median and p95 wall time, committed wasm bytes, output
   bytes, long tasks and dropped video frames. Compare against native FFmpeg and a
   specified FFmpeg wasm build separately, with build flags and hardware recorded.
2. Remove movie frame allocations (#35): borrow sample ranges from the downloaded
   MP4 piece, then copy once into the output fragment. Keep Matroska frames owned
   when their lifetime crosses piece boundaries. This avoids unnecessary work before
   attempting instruction-level optimization.
3. Pass AC-3/E-AC-3 through when the platform supports it (#32), with explicit opt-in,
   verified configuration boxes and the Rust decoder fallback. The issue reports
   about 55 ms of decoding per six-second AC-3 segment; eliminating that work is
   a larger opportunity than speeding up the wrapper. Confirm on a supported device.
4. Move required audio decoding into a dedicated worker (#34/#18). Transfer output
   buffers rather than serializing samples. Measure message/copy costs and startup
   separately. This improves responsiveness; it does not itself reduce decoder CPU.
5. Profile decoder kernels, then evaluate Rust wasm SIMD128 for the dominant DSP
   loops (transform, windowing, downmix). Keep a scalar build and compare both wasm
   bytes and latency. Do not change dependency code without measuring it first.
6. Test fixed-predictor/Rice FLAC (#39) on real programme audio, retaining verbatim
   frames when compression loses. Measure encoder CPU against browser memory saved.
7. Explore WebCodecs for platform video decoding (#12), after a device support matrix.
   Hardware acceleration is a browser preference, not a guarantee. A canvas backend
   also needs explicit decisions about sync, controls, accessibility and PiP.

Rust SIMD: https://doc.rust-lang.org/core/arch/wasm32/
WebCodecs specification: https://www.w3.org/TR/webcodecs/

## Local measurements (2026-10-01)

Rust 1.98.1, Node 26.10, release wasm with size optimization; seven runs in fresh
processes, median unless stated. Generated input is excluded from version control.

| Scenario | Before | After | Interpretation |
| --- | ---: | ---: | --- |
| Package 30-minute AAC MP4 | 33.604 ms | 27.253 ms | 18.9% less CPU; both commit 6,356,992 wasm bytes |
| Package 30-minute AC-3 MP4 | 5,817.975 ms | 5,862.843 ms | Within noise; audio decoding dominates |
| Six-second AC-3 stream, before/after FLAC compression | 59.691 ms | 72.910 ms | Compression adds about 13 ms; wasm committed bytes fall from 11,206,656 to 10,354,688 |
| Real programme audio FLAC encoder | verbatim PCM | 12.564 ms | 237,119 bytes versus 798,720 PCM bytes (29.7%) |
| Synthetic six-second AC-3 audio FLAC encoder | verbatim PCM | 18.641 ms | 277,281 bytes versus 1,155,072 PCM bytes (24.0%) |

The programme input is the existing Big Buck Bunny fixture's soundtrack transcoded
to AC-3 by `tools/media.py`, approximately 4.16 seconds of decoded sound. This is a
small realistic sample, not a broad corpus. Fixed predictor orders 0–2 with a bounded
Rice parameter search retain verbatim for incompressible channels. The trade is taken:
about 70% less appended audio in this sample for about 3 ms CPU per second of sound.
An independent FFmpeg decoder roundtrips silence, noise, extreme values and ramps
bit for bit. AC-3, E-AC-3 and MP2 pipeline decoding tests also pass.

The early AAC movie comparison uses a cached 152,736,156-byte, low-resolution movie.
It is historical evidence for borrowing samples, not the current generator's 1080p
movie. A clean source copy generated a 2,030,153,925-byte AAC movie: 419.247 ms median,
469.479 ms maximum and 19,136,512 committed wasm bytes. Its AC-3 MP4/MKV medians were
4,626.984/4,547.360 ms; programme FLAC was 13.735 ms with the same 29.7% ratio. These
fresh runs overlapped browser/testing work, so timings include machine contention.
The generator now records command arguments and source hashes before reusing media,
preventing stale cached movies from silently changing the workload.

Alternating audio timestamp offsets of ±100 ms over 20 discontinuities yielded a
maximum A/V mismatch of 316.811 ms before correction, 105.467 ms after; initial skew
was 5.478 ms. A gradual correction capped at 2% of video duration avoids new holes.
This bounds the measured fixture, not arbitrary broadcast timestamp corruption.

Browser: Linux x86_64, Chromium 154 in the desktop in-app browser. Six-second AC-3
synchronous processing took 84.2 ms; worker processing 74.9 ms, startup/transfer/timer
included roundtrip 100.3 ms, worker timer 11 ms for a requested 10 ms. This is a single
cold run. The worker example transfers input buffers, and keeps the library independent
of a host application's worker policy.

MSE reports H.264 and FLAC supported, HEVC/AC-3/E-AC-3 unsupported. WebCodecs reports
H.264 software supported, hardware preference unsupported, and both HEVC preferences
unsupported. Hardware preferences are hints per the [WebCodecs specification](https://www.w3.org/TR/webcodecs/).
Decision: defer a universal canvas backend; report unsupported media or choose a
supported stream in the Rust host. Playback must not launch or link FFmpeg. A target
hardware/browser matrix and actual decode/deinterlace CPU measurements are still needed.

Verified browser inputs: CMAF and AES-128 HLS reach the 24-second end; audio-only HLS
reaches the end; external audio/video merge buffers 0–24 seconds and advances; endless
raw TS advances beyond 25 seconds with one contiguous bounded range. Repeated 404s
advance past 72 seconds with a single range and no Failed status.

These results do not establish superiority over FFmpeg. The comparison needs equivalent
operations, matching codecs, a specified FFmpeg wasm build and native baselines.

## Cancellation regression checks (#7, #37)

The native cancellation tests verify that a suspended operation is dropped when the
stop flag is set and that normal completion retains its result. All four feature
combinations must also compile for wasm.

Browser acceptance checks still required:

- Switch live channels every 200 ms while requests are delayed. The old player must
  not seek/play or report status after it is dropped; Direct requests must abort.
- Repeat for movie startup, range downloads and seeks.
- Change video.src during an append and during removal. The pending wait must settle
  and report failure if the player remains active; a dropped player stays silent.
- Remove the updating SourceBuffer. Its wait must settle with an error. Removing an
  unrelated buffer must not finish the current buffer's update early.
- Repeat switching to check that listener closures and readers do not accumulate.

Custom Fetch implementations must honor the documented future/body drop contract.

Later full-harness run after FLAC compression: 30-minute AC-3 MP4 packaging
4,139.361 ms / 6,029,312 committed wasm bytes; Matroska 4,190.743 ms /
4,915,200 bytes. Compression reduces later CRC/output work and buffer size.
AAC MP4 repeat: 27.961 ms, within noise of the earlier 27.253 ms.
Six-second AC-3 stream repeat: 72.062 ms; programme FLAC: 12.353 ms and the
same 237,119 output bytes. JSON is generated in tools/results by the profile script.

The byte-range CMAF test initially exposed URI-only segment identity. Identity now
includes the range. The corrected run plays all 720 frames across 24.021 seconds
with zero dropped/corrupted frames and one buffered range. Thirty channel switches
at 200 ms with 2-second delayed playlist responses produce exactly 30 Buffering
states, no stale Playing state, zero current time, and no buffered media.
The six-second yielded AC-3 run reports no long tasks and starts in about 195 ms.

## Native comparison with FFmpeg

2026-10-01: AMD Ryzen 7 5700, Linux x86_64, Rust 1.98.1 default release profile,
FFmpeg n9.0.2. Same 6,866,512-byte six-second 1080p H.264/AAC TS input, native
command-line processes, file I/O and startup included. One warmup, eight measured
runs in alternating order. Rust median 14.508 ms, maximum 15.155 ms; FFmpeg median
60.497 ms, maximum 61.806 ms. Ratio 4.17× in this specific CLI workload.

Both write fragmented MP4. Fragment boundaries and metadata sizes differ:
Rust 6,675,424 bytes, FFmpeg 6,673,179 bytes. Independent decoding produces identical
hashes for all 180 video frames and byte-identical decoded audio (1,159,168 bytes).
Input probing/process startup contribute to FFmpeg's measured time. This does not
measure in-process libavformat, codec conversion, general decoder speed or FFmpeg wasm.

That measurement used the since-removed `transmux` example. The next section, the
`rstreamkit` command against FFmpeg on five workloads, supersedes it, and shows most of
that first 4.2× was FFmpeg's start-up and probing.

## The rstreamkit command against FFmpeg (2026-10-10)

AMD Ryzen 7 5700 (16 threads), Linux 7.2, Rust 1.99 release build, FFmpeg n9.0.2.
Both tools run as fresh processes on the same input and write to a RAM-backed
directory (`/dev/shm`). One warm-up, then timed runs in alternating order; medians,
maxima in brackets. Wall time includes process start-up and reading and writing the
files. FFmpeg's `-benchmark` rtime is its processing alone, without start-up and input
probing; our whole wall time is also compared with it, so a win there is a win on
processing. Peak memory is maximum resident set size, measured through `tools/peak.rs`:
spawned straight from Python, every process reports Python's memory as its own peak.

| Workload | Input | Runs | rstreamkit | FFmpeg | FFmpeg processing only | Peak memory, ours / FFmpeg | Faster by (whole / against processing only) |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 6 s 1080p TS, H.264/AAC, copied | 6,866,512 B | 8 | 7.94 ms (9.16) | 71.87 ms (74.32) | 9.0 ms | 12.7 / 68.1 MB | 9.05× / 1.13× |
| 10 min 1080p TS, H.264/AAC, copied | 686,602,884 B | 5 | 334 ms (365) | 831 ms (836) | 715 ms | 17.3 / 67.7 MB | 2.49× / 2.14× |
| 30 min 1080p MP4, H.264/AAC, copied | 1,989,780,176 B | 5 | 781 ms (858) | 1,442 ms (1,687) | 1,241 ms | 16.3 / 66.4 MB | 1.85× / 1.59× |
| 6 s 1080p TS, AC-3 5.1 decoded to stereo FLAC | 7,119,372 B | 8 | 56.4 ms (64.5) | 77.3 ms (85.1) | 13.0 ms | 13.2 / 72.0 MB | 1.37× / 0.23× |
| 30 min 640×360 MKV, AC-3 5.1 decoded to stereo FLAC | 514,185,862 B | 3 | 14,256 ms (14,543) | 2,256 ms (2,281) | 2,173 ms | 9.6 / 63.6 MB | 0.16× / 0.15× |

Every workload's outputs were checked against each other. All decoded pictures are
identical (`framecrc`: 180, 18,000, 53,666, 180 and 45,000 pictures). Copied sound
decodes to identical samples. Decoded sound comes from different AC-3 decoders and
downmixes, so it is checked to decode cleanly to the same length within one AC-3 frame.

FFmpeg's options: `-map 0:v:0 -map 0:a:0`, then `-c copy` (with `-bsf:a aac_adtstoasc`
for TS) or `-c:v copy -c:a flac -compression_level 0 -ac 2 -sample_fmt s16` (fixed
predictors and stereo 16-bit, as ours), then `-movflags frag_keyframe+empty_moov -f mp4`.
Input SHA-256 prefixes: `aac.ts` 26e59a26f948, `long.ts` 94510259bf7e,
`movie-aac.mp4` 539587a591c6, `ac3.ts` a37d796f069b, `movie-51.mkv` d2385bdd4070.

What it shows:

- **Repackaging is faster than FFmpeg on every input**, including against FFmpeg's
  processing time alone, with a quarter of its memory or less. Memory does not grow
  with the input: a 2 GB movie peaks at 16 MB.
- **Decoding AC-3 is slower.** In-process, 6 s of 5.1 takes 40.6 ms in the AC-3
  decoder dependency (`oxideav-ac3`) and 6.6 ms in our FLAC encoder, so the decoder is
  86% of the work. It decodes all six channels to PCM and mixes down afterwards; a
  stereo source costs 1.5 ms per second of sound, 5.1 costs 6.7. FFmpeg's whole job is
  13 ms. Its FLAC is also smaller here (456 against 497 MB for 30 minutes).
- **The first 4.2× was mostly FFmpeg starting up.** On that file FFmpeg's processing
  took 9.5 ms, less than our 14.5 ms whole command at the time. The changes below made
  ours faster than FFmpeg's processing.

What changed (same machine; in-process stage timings unless "command"):

| Change | Before | After |
| --- | ---: | ---: |
| Demuxing a 6 s TS segment | 4.9 ms | 2.4 ms |
| Demuxing 10 minutes of TS | 533 ms | 216 ms |
| A continuous TS stream cut at keyframes, 10 minutes | 535 ms | 190 ms |
| A 30-minute MP4: 4 MiB fresh buffers, then one reused 1 MiB buffer | 930 ms | 171 ms |
| Command, 6 s TS (all of the changes) | 14.5 ms | 7.9 ms |
| Command, 10 min TS (first measured after the demuxer change: threads only) | 657 ms | 334 ms |
| Command, 30 min MP4 (buffer reuse and threads) | 1,768 ms | 781 ms |

- H.264 start codes are found eight bytes at a time: a start code begins with a zero
  byte, compressed pictures have few, and the lowest flagged byte of the word test is
  exact. A test compares it with a byte-by-byte scan on 2,000 dense random buffers.
- Each TS packet found its stream through a SipHash map; a list of at most a few
  streams replaces it.
- A continuous stream's next buffer starts at the last one's capacity instead of
  growing (and copying, and faulting in pages) again for every group of pictures.
- 4 MiB movie reads made glibc return each freed buffer to the system, and every new
  one paid for its pages again: raising glibc's trim and mmap thresholds took the same
  run from 957 to 226 ms. The command now reuses one 1 MiB buffer.
- The command reads, converts and writes on three threads joined by two-piece queues:
  as fast as eight-piece queues, at 17 instead of 25 MB of peak memory.

The earlier looped AC-3 movie (`movie.mkv`) is not used here. Each of its 300 seams
overlaps its sound by about 10 ms: FFmpeg trims the overlap, we keep the samples and
re-anchor within one frame, so decoded lengths differed by 3.2 s. The benchmark uses
`movie-51.mkv`, encoded straight through.

Not measured: in-process libavformat, FFmpeg built for wasm, other machines, and
FFmpeg with non-default thread settings.

## A twice-as-fast Dolby decoder (2026-10-10)

The AC-3 decoder (oxideav-ac3 0.0.11 by Mark Karpeles, MIT) now lives in `ac3/` as
`rstreamkit-ac3`, with five changes found by sampling it with pprof. Before them, 23% of
decoding was `2f32.powi(-e)` per coefficient, 10% rebuilding the IMDCT's twiddle table on
every call, 21% the FFT (which also recomputed its twiddles and bit reversal in every group),
7.5% a downmix re-deciding its taps per sample, and 1.4% `std::env::var` per block. Each
change keeps every arithmetic operation and its order, so decoded PCM is byte-identical
to the original on stereo, 5.1, E-AC-3 and real programme audio; the crate's own 558
tests pass.

Decoding alone, best of ten in-process (the same Ryzen 7 5700):

| Input | oxideav-ac3 0.0.11 | rstreamkit-ac3 |
| --- | ---: | ---: |
| 6 s AC-3 5.1, 448 kbit/s (`tools/media/ac3.ts`) | 40.3 ms | 20.0 ms |
| 4.2 s programme audio (`tools/media/programme.ts`) | 10.4 ms | 5.9 ms |
| 4 s AC-3 mono (`h264_ac3.ts`; historical silent output, invalid audio-speed comparison) | 5.75 ms | 2.74 ms |
| 2 s AC-3 5.1 (`h264_ac3_51.ts`) | 9.08 ms | 3.59 ms |
| 2 s E-AC-3 5.1 (`h264_eac3_51.ts`) | 9.70 ms | 4.36 ms |

The command against FFmpeg afterwards (`python3 tools/compare.py "AC-3"`, checks passed):

| Workload | rstreamkit | FFmpeg (processing only) | Peak memory, ours / FFmpeg |
| --- | ---: | ---: | ---: |
| 6 s 1080p TS, AC-3 5.1 to stereo FLAC | 37.6 ms (was 56.4) | 74.3 ms (12.0 ms) | 13.2 / 71.8 MB |
| 30 min MKV, AC-3 5.1 to stereo FLAC | 8,437 ms (was 14,256) | 2,151 ms (2,077 ms) | 9.7 / 63.8 MB |

Size cost, measured on the browser test bed's wasm (`tools/harness`, release,
`wasm-opt -Oz`, `brotli -q 11`): 316,435 to 323,026 bytes (+6.6 KB, +2.1%). The native
`rstreamkit` binary grew from 1,571,424 to 1,588,312 bytes (+16.9 KB, +1.1%).

FFmpeg remains four times faster at decoding long AC-3. What remains is real work:
parsing the bitstream (exponents, bit allocation, mantissas) is about 64% of decoding,
the transforms about 20%, the downmix about 6%. The next levers change results within rounding rather than bit for bit (mixing
down before the inverse transforms, which would run 2 of them instead of 6) or need
care with decoder state (decoding parts of a file in parallel).

Reproduce:

```sh
python3 tools/media.py
cargo build --release --bin rstreamkit
python3 tools/compare.py        # all five; python3 tools/compare.py "10 min" runs one
```

Accelerated wasm soak: five batches of 2,000 AAC segments (10,000 total) commit
1,900,544 wasm bytes in every batch. Batch times are 345–356 ms. No growth after
warmup; native tests separately check monotonic decode clocks and bounded output.
This is a pipeline soak, not a real-time network/browser endurance claim.

Rust 1.88.0 checks pass for native and wasm targets with all features and minimal
features, using official checksum-verified toolchains installed under /tmp.

High-bitrate browser run: after 1,160.8 seconds of playback, the buffered range was
1,150.331–1,176.021 seconds (25.691 seconds). Measured appended bitrate was 8.899 Mbps,
so the range represents approximately 28.58 MB of compressed media, below the 32 MiB
budget. There were no skipped segments, failures or long tasks. The browser reported
1,038 total frames, 560 dropped and zero corrupted. Those drops require investigation;
this run establishes bounded buffering, not smooth 1080p rendering. It does not measure
the browser's actual allocator or decoder surface memory.

Further browser checks: two ten-second outages recover automatically; the corrupt
segment run skips eight segments and advances past 133.9 seconds. Frozen playlists
recover twice, then show one contiguous range beyond 74.7 seconds. A pause exceeding
60 seconds on a broken-sequence 12-second window resumes without failure and returns
to 2.69 seconds reported latency. Source closure and removal while updating=true
both settle with a Failed report, rather than parking the task indefinitely.
MP4/Matroska movie runs reach 40.8/68.2 seconds with zero dropped/corrupted frames.
Resolution and AAC-rate changes (320×180/48 kHz → 640×360/44.1 kHz → original) reach
6.097 seconds; draining each old source prevents the first segment being discarded.
The final source's 60 frames report no drops; frame counters reset with source rebuilds.

Dolby configuration comparison: AC-3 `dac3` bytes match exactly. The E-AC-3 fixture's
five core `dec3` bytes match exactly. Older FFmpeg muxers append a zero extension byte;
the test accepts that disabled flag while rejecting a nonzero extension. The optional
extension is documented by [Dolby](https://ott.dolby.com/OnDelKits/DDP/Dolby_Digital_Plus_Online_Delivery_Kit_v1.5/Documentation/Playback/SDM/help_files/topics/c_id_ddp_atmos_isobmff_2.html).
Both reference layouts must still decode our complete output without errors.

## FLAC CPU improvement (2026-10-01)

The encoder now keeps unfinished output bits in a register, emits the Rice
terminator and remainder together, and reuses one residual search buffer per
channel. Predictor selection and compressed bytes are unchanged. This follows
the [FLAC bitstream rules](https://www.rfc-editor.org/rfc/rfc9639.html#section-9.2.7).

Same Ryzen 7 5700/Linux, Rust 1.98.1 and Node 26.10, same wasm release build and
input files. Twelve fresh Node processes per version and scenario, alternating
before/after order; each process uses the existing seven-run profiler. The table
reports the median of those process medians. Build and audio preparation are
outside the FLAC encoder timing. Baseline: `bb66f506`.

| Work | Before | After | CPU reduction |
| --- | ---: | ---: | ---: |
| Programme audio FLAC encoding (4.16 seconds) | 12.583 ms | 8.899 ms | 29.3% |
| Synthetic six-second audio FLAC encoding | 18.957 ms | 11.750 ms | 38.0% |
| Full six-second AC-3 TS push | 76.921 ms | 69.813 ms | 9.2% |

Programme FLAC output remains 237,119 bytes (29.7% of PCM). Synthetic output
remains 277,281 bytes. Committed wasm memory is unchanged in these scenarios.
A native before/after encoder check produces byte-identical programme FLAC;
FFmpeg decodes it to the original PCM exactly. The independent decoder tests
also cover silence, noise, extreme values, ramps and mixed constant/predicted
channels. These are improvements over our previous implementation, not a
comparison with FFmpeg's audio encoder or decoder.

Reproduce with `bash tools/build.sh`, then
`node tools/node/profile.cjs flac tools/media/programme.ts` and the equivalent
`flac`/`push` runs for `tools/media/ac3.ts`. Keep a copy of `tools/node/pkg` and
`profile.cjs` from the baseline build, use identical input files, and alternate
baseline/current processes. This small programme sample does not represent all
soundtracks; browser dropped-frame improvement is still unmeasured.

## Shared retry loop (2026-10-10)

Browser playback now uses the same Rust retry loop as native hosts. Fake time
makes outage and stop tests immediate; it does not make real network retries
faster. No dependencies were added. The loop keeps its seven-attempt limit and
31-second total backoff budget. Dropping a browser wait now clears its timer.

| Measure | Before | After |
| --- | ---: | ---: |
| Native tests of the actual retry loop | 0 | 9 |
| Live player, Brotli bytes (Wasm + JS) | 109,318 | 108,751 |
| Live + sound player, Brotli bytes | 207,114 | 206,631 |
| Movie player, Brotli bytes | 132,204 | 131,598 |
| Full player, Brotli bytes | 229,337 | 228,866 |

Same Linux, Rust 1.99.0, wasm-bindgen 0.2.128, wasm-opt 130 and Brotli 1.2.0;
`python3 tools/size.py --check-budgets --output tools/results/size-retry.json`.
The baseline is the main tree at `39f9c383`, with the existing
`size-buffer-final.json` report. These small build-size changes are not runtime
speed measurements or a comparison with FFmpeg.

Run `cargo test --test retry` (also with `--no-default-features`). The nine cases
cover a ten-second outage through real TS parsing, a dropped response body,
404, retry exhaustion, three stop points, and dropping pending timer/request
work. The fake provider recovers at the scheduled 15-second attempt.

For browser checks, install `@playwright/test@1.64.0` in a development directory,
build the player and run `python3 tools/server.py`. Set `NODE_PATH` to that
installation's `node_modules`, then run `node tools/node/retry-smoke.cjs`.
`RSTREAMKIT_URL` can select another local player URL. Chrome 155.0.8059.39 passed
two checks: Stop reduced one pending retry timer to zero with no later request;
a 503 followed by a 404 made exactly two requests and stopped. These are
functional checks; real Edge/Safari and long 4K60 acceptance remain pending.

## Shared buffer control (2026-10-10)

Browser playback now uses the native-testable Rust trim and backpressure loop.
It stops before new media work when the buffer is full, honours stop/go-live,
and reports removal failures. Gap checks read ranges as an iterator instead of
building a temporary vector. The loop adds no dependencies or media-byte copies.

| Measure | Retry baseline | Shared buffer control |
| --- | ---: | ---: |
| Native buffer-control integration tests | 0 | 12 |
| Temporary range vectors per gap check | 1 | 0 |
| Live player, Brotli bytes | 108,751 | 109,690 |
| Live + sound player, Brotli bytes | 206,631 | 207,300 |
| Movie player, Brotli bytes | 131,598 | 132,475 |
| Full player, Brotli bytes | 228,866 | 229,637 |

Baseline: `70c709b` (PR #52), with the same Linux/Rust 1.99.0,
wasm-bindgen 0.2.128, wasm-opt 130 and Brotli 1.2.0. Repeat with
`python3 tools/size.py --check-budgets --output tools/results/size-buffer-control.json`.
All four size budgets pass. The full player grows 771 bytes (0.34%) to expose
and validate shared host control. This is a correctness/testability tradeoff;
no runtime CPU, memory peak or FFmpeg speed advantage was measured here.

Run `cargo test --test buffer_control`, also with `--no-default-features`.
Twelve cases cover normal backpressure, immediate room, gap recovery, go-live
before/during a wait, stop before/during removal, removal errors, small/empty
buffers, invalid limits/durations and dropping a paused wait. The nine shared
retry integration tests still pass too.

Playwright Chrome 155.0.8059.39 passes the five playback regressions from PR #50
against this exact build. It also passes four source-update fault checks from
`tools/node/buffer-smoke.cjs`: close during append, close during removal, remove
the active buffer, and remove an unrelated buffer. Each records `updating=true`.
The first three settle with Failed; the unrelated removal reaches Ended.

To repeat, install Playwright as described above, build and serve the player,
and run `NODE_PATH=tools/browser/node_modules node tools/node/buffer-smoke.cjs`.
The fault test serves 20 copies of the committed `bbb_480p.ts` fixture
through Playwright routes, so no media generator or PR #50 tooling is needed.
The roughly 90-second playlist is long enough to trigger actual rewind
removal. It runs at 4× after the video is ready. `RSTREAMKIT_URL` selects
another local player URL. These accelerated correctness checks do not establish
4K60 performance, a long soak, or real Edge/Safari acceptance. Full playlist and
session orchestration still needs extraction for #23.

## Shared playlist session (2026-10-10)

The browser now calls `session::Session` for playlist selection, restart tracking,
backpressure preparation and periodic refresh. Hosts supply clock, buffer and
playlist I/O. Selection borrows the segment; no media bytes are cloned. Go-live
during the periodic wait uses that refresh instead of fetching again next loop.

| Measure | Buffer-control baseline | Shared playlist session |
| --- | ---: | ---: |
| Native playlist-session integration tests | 0 | 13 |
| Shared retry/buffer/session integration tests | 21 | 34 |
| Live player, Brotli bytes | 109,690 | 110,873 |
| Live + sound player, Brotli bytes | 207,300 | 208,327 |
| Movie player, Brotli bytes | 132,475 | 133,808 |
| Full player, Brotli bytes | 229,637 | 231,013 |

Baseline `58ff9b6` (PR #53); same Linux/Rust 1.99.0, wasm-bindgen 0.2.128,
wasm-opt 130 and Brotli 1.2.0. Repeat with
`python3 tools/size.py --check-budgets --output tools/results/size-session.json`.
All four budgets pass. The full player grows 1,376 bytes (0.60%). No dependencies
are added. This is shared control and test coverage, not a measured CPU,
peak-memory or FFmpeg performance advantage.

Run `cargo test --test session`, also with `--no-default-features`. Thirteen
scenarios cover sliding windows with/without sequence updates, numbering/PTS
resets, long-pause refresh, go-live, a ten-second outage, a dropped response,
404, bounded endless bodies, finite backpressure, stop and frozen playlists.
Media cases use the real TS parser, packager and ordered output sink. The slow
request case advances fake time by three seconds per fetch; it is not a bandwidth
or real-network throughput benchmark. Body cap tests use 1 KiB limits for quick
termination; production browser limits are unchanged.

Playwright Chrome 155.0.8059.39 passes twelve checks on the exact review build:
five playback, four source-update faults, two retry checks, and one new session
check. The last issues go-live during a 2.5-second poll wait: it observes two
playlist requests total (initial + one refresh), with zero pending poll timers
after Stop. The former browser code did a periodic fetch followed by a separate
go-live fetch; that before count comes from code inspection, not a timing run.

Build/serve the full player, install Playwright as above, then run
`NODE_PATH=tools/browser/node_modules node tools/node/session-smoke.cjs`.
It serves committed TS fixture bytes through routes and needs no generated media.
`RSTREAMKIT_URL` selects another local full-player URL. Playback regression
fixtures/tooling remain in PR #50. These checks establish neither 4K60 performance
nor real Edge/Safari acceptance. #23 remains open for the complete media runner,
variant/rendition transitions and streamed/pending-I/O faults through that runner.

## Prepared playlist walks (2026-10-13)

Historical benchmark below is from `c80c0ee` (PR #55). The later constant-space
section compares against that cache-based implementation.


The old browser loop found the current/next segment and summed the remaining
playlist for every segment. Live stats also summed the whole window repeatedly.
The new `Session::batch` prepares suffix durations once, borrows segment references
and supplies the next segment directly. The browser keeps a finite batch across
buffer waits, so pausing does not rebuild the cache. Refresh/control/variant changes
start a new batch. Fixed-snapshot work is O(n), rather than O(n²).

Native bookkeeping benchmark, Ryzen 7 5700/Linux x86_64, Rust 1.99.0 release,
baseline `341edf2` (PR #54). The test retains the old finite-loop bookkeeping
beside the new walk; live-only duration sums are excluded for finite input.
Generated metadata has six-second segments. Each size runs in a fresh process:
one warm-up per path, then nine alternating pairs. Cache construction and the
same consumed-cursor updates are timed; playlist parsing, media processing,
network, playback waits and file I/O are excluded. Values are medians; p95 below
is the nearest-rank ninth sample. These are not end-to-end browser/4K or FFmpeg
benchmarks, and should not be read as playback speedups.

| Playlist entries | Before median | After median | Before p95 | After p95 | Added cache payload |
| --- | ---: | ---: | ---: | ---: | ---: |
| 128 | 0.022663 ms | 0.002565 ms | 0.024827 ms | 0.002645 ms | 1,032 bytes |
| 2,048 | 4.367354 ms | 0.032271 ms | 4.382802 ms | 0.034325 ms | 16,392 bytes |
| 20,000 | 573.062639 ms | 0.338950 ms | 582.513679 ms | 0.375549 ms | 160,008 bytes |

Cache payload costs eight bytes per pending segment plus a sentinel, not per
advertised segment already consumed. An initial live selection of three segments
uses 32 bytes. Allocator/header overhead and total process/Wasm peak memory are
not measured here. Media bytes are not copied and no dependencies are added.
Reverse suffix sums preserve tiny tails even after totals overflow; subtracting
from a single accumulated total would lose them. Minor floating-point roundoff
versus the former forward sum is expected.

| Player download (Wasm + JS, Brotli) | Before | After |
| --- | ---: | ---: |
| Live | 110,873 bytes | 111,440 bytes |
| Live + sound | 208,327 bytes | 208,904 bytes |
| Movie | 133,808 bytes | 134,520 bytes |
| Full | 231,013 bytes | 231,450 bytes |

Same wasm-bindgen 0.2.128, wasm-opt 130 and Brotli 1.2.0 size recipe. Full download
grows 437 bytes (0.19%); all budgets pass. Reproduce with
`python3 tools/size.py --check-budgets --output tools/results/size-playlist-batch.json`.

```sh
for count in 128 2048 20000; do
  RSTREAMKIT_BENCH_SEGMENTS=$count cargo test --release --test playlist_batch measure_playlist_bookkeeping -- --ignored --nocapture
done
```

Four native tests check selection/next pointers, variable durations, empty/live/
resumed snapshots, broken sequence/byte-range anchors, and extreme sums. Run
`cargo test --test playlist_batch`, also without default features. Required Rust
checks and decoder tests pass. Playwright Chrome 155.0.8059.39 passes twelve
existing checks against this build: five playback, four source-update faults,
two retry and one go-live check. In particular, finite playback still reaches
its last segment across buffer waits. Browser instructions are above and in
`tools/README.md`. Long 4K60 and real Edge/Safari validation remain pending.

## Constant-space runtime walks (2026-10-13)

The playlist walk now keeps bounded duration totals instead of an array per
segment. Equal durations use a count and one value. Ordinary varying durations
use scaled 128-bit integers. Extreme values use a fixed 2176-bit integer sum;
this fallback preserves small tails after overflowing totals. Conversion rounds
the exact remaining total once. The cursor also reuses URI storage and releases
an unusually large buffer after a short replacement.

For one unchanged snapshot, time remains O(n). Extra walk storage changes from
O(n) to O(1). The parsed playlist and media buffers still occupy their own memory;
this does not make the whole player constant-space.

Allocator measurements: native x86_64, Ryzen 7 5700/Linux, Rust 1.99.0 release,
predecessor `c80c0ee`. Thread-local counters cover batch construction/walking or
cursor updates only; fixture creation is excluded. These measure requested heap
bytes and object sizes, not whole-process peak memory or committed Wasm pages.

| Native runtime measure | Cache-based | Constant-space |
| --- | ---: | ---: |
| Normal 20,000-entry batch heap bytes requested | 160,008 | 0 |
| Normal batch object + requested array bytes | 160,064 | 64 |
| Normal three-entry batch object + array bytes | 88 | 64 |
| URI allocations across 20,000 equal-length names | 20,000 | 1 |
| URI bytes requested across those updates (cumulative) | 360,000 | 18 |

The new normal batch is 64 bytes on this build, independent of playlist length.
Rare extreme inputs use one 296-byte fallback allocation, also independent of
length; normal durations allocate none. Allocator headers/padding are excluded.
Cursor growth may reallocate for longer URIs; it does not promise one allocation
for arbitrary names. Storage above four times the next URI length (minimum
256 bytes) is replaced rather than retained indefinitely.

Timing driver retains the predecessor's suffix array and URI cloning beside the
new path. Each scenario is a fresh process, one warm-up per path then nine
alternating pairs. Construction and cursor updates are included; parsing, media,
network, file I/O and playback waits are excluded. CPU/OS/compiler are as above.
Variable durations cycle through 4.589889, 5.123457 and 5.5 seconds.

| Entries / durations | Before median | After median | Before p95 | After p95 |
| --- | ---: | ---: | ---: | ---: |
| 128 / equal | 0.002094 ms | 0.002225 ms | 0.002184 ms | 0.002294 ms |
| 2,048 / equal | 0.041348 ms | 0.041980 ms | 0.041658 ms | 0.042961 ms |
| 20,000 / equal | 0.302902 ms | 0.287533 ms | 0.457495 ms | 0.409774 ms |
| 128 / varying | 0.001954 ms | 0.003066 ms | 0.002044 ms | 0.003477 ms |
| 2,048 / varying | 0.035567 ms | 0.048312 ms | 0.037771 ms | 0.051086 ms |
| 20,000 / varying | 0.324584 ms | 0.485528 ms | 0.398252 ms | 0.489355 ms |

Equal-duration timing is broadly unchanged; varying durations cost another
0.161 ms for 20,000 entries. This is a deliberate memory/allocator tradeoff,
not a universal CPU win. Linear time is retained rather than improved beyond
O(n). No end-to-end speed, whole-player peak-memory or FFmpeg claim is made.

Repeat allocation measurements with
`cargo test --release --test playlist_allocations measure_playlist_allocations -- --ignored --nocapture`.
To capture the predecessor, copy that test file into an isolated `c80c0ee`
checkout and run only the ignored measurement; its new zero-allocation assertions
are intended for the new implementation. Timing:

```sh
for count in 128 2048 20000; do
  RSTREAMKIT_BENCH_SEGMENTS=$count cargo test --release --test playlist_batch measure_playlist_bookkeeping -- --ignored --nocapture
  RSTREAMKIT_BENCH_VARIABLE=1 RSTREAMKIT_BENCH_SEGMENTS=$count cargo test --release --test playlist_batch measure_playlist_bookkeeping -- --ignored --nocapture
done
```

Six new regular checks cover exact removable arithmetic, nonfinite restoration,
bounded URI retention, normal zero allocations, URI reuse and constant fallback
allocation. The arithmetic oracle checks every suffix in 112 generated cases,
using Python Decimal at 1600-digit precision. Reproduce with
`python3 tools/duration_vectors.py --check`; Python is a development tool only.
Runtime library code adds no dependencies and no unsafe code; the allocation-test
wrapper alone forwards unsafe System allocator calls for measurement.

| Download, Wasm + JS Brotli bytes | Before | After |
| --- | ---: | ---: |
| Live | 111,440 | 112,767 |
| Live + sound | 208,904 | 210,542 |
| Movie | 134,520 | 135,739 |
| Full | 231,450 | 232,785 |

Same wasm-bindgen 0.2.128, wasm-opt 130 and Brotli 1.2.0 recipe. Full download
grows 1,335 bytes (0.58%) to remove growing runtime storage. All four budgets
pass: `python3 tools/size.py --check-budgets --output tools/results/size-constant-space.json`.
Required Rust/native/wasm checks, media/decoder tests and twelve Playwright Chrome
155.0.8059.39 checks pass. 4K60 soak and real Edge/Safari acceptance remain open.


## Allocation-free H.264 scanning (2026-10-13)

The TS parser now borrows one Annex-B NAL unit at a time. Previously it built
one growing list of offsets and another of borrowed slices for every video PES
payload. The scan still uses the existing eight-byte zero search and preserves
three/four-byte start codes, empty units and trailing-zero handling. Input and
output media storage are unchanged. Only scan bookkeeping changes from O(k)
heap space to O(1), where k is the number of NAL units; time remains O(bytes).
No dependencies, runtime unsafe code or public API changes are added.

Baseline: `cb6e2a3` (PR #56). The test-only collected scanner preserves that
implementation for a side-by-side comparison. Linux x86_64, Ryzen 7 5700,
Rust 1.99.0, release profile. Synthetic escaped payloads contain 129 bytes per
NAL and alternate three/four-byte start codes. Inputs are built before measuring;
both paths consume borrowed slices without collecting the result. Nine alternating
pairs follow warmup. p95 is the largest of nine batch averages, not per-call latency.

| NAL units | Heap calls before → after¹ | Requested bytes before → after² | Median µs before → after | p95 µs before → after |
| ---: | ---: | ---: | ---: | ---: |
| 1 | 2 → 0 | 96 → 0 | 0.118 → 0.100 | 0.119 → 0.104 |
| 128 | 12 → 0 | 6,048 → 0 | 13.445 → 13.039 | 13.516 → 13.141 |
| 4,096 | 22 → 0 | 196,512 → 0 | 424.528 → 419.362 | 426.116 → 420.598 |

¹ Allocations plus reallocations. ² Cumulative requested sizes, including each
reallocation; not peak live memory, whole-app RSS or committed Wasm pages.
The modest timing difference does not establish faster end-to-end playback or
an advantage over FFmpeg. The reliable win is removing temporary heap work.

| Player download, Wasm + JS Brotli bytes | Before | After | Change |
| --- | ---: | ---: | ---: |
| Live | 112,767 | 112,830 | +63 |
| Live + sound | 210,542 | 210,259 | −283 |
| Movie | 135,739 | 135,702 | −37 |
| Full | 232,785 | 232,817 | +32 |

All four download budgets pass. Compression and code layout can make the same
source change shrink one feature build and grow another; full grows 0.014%.
Same recipe as the preceding report: wasm-bindgen 0.2.128, wasm-opt 130,
Brotli 1.2.0. Reproduce:

```sh
cargo test --release --lib measure_nal_scan -- --ignored --nocapture --test-threads=1
python3 tools/size.py --check-budgets --output tools/results/size-nal-scan.json
```

The zero-allocation regression covers 0, 1, 128 and 4,096 units; existing
byte-at-a-time oracle tests cover 2,000 generated inputs. Native chunking,
continuous input and independent FFmpeg output/decoder checks remain required.

Validation: formatting, all eight native/Wasm clippy configurations, default,
minimal and vod tests, and the vendored decoder suite pass. Five Playwright
Chrome 155.0.8059.39 checks pass against this exact full-player build: finite
completion, Dolby fallback, finite 404, stop/new session, and live 404 recovery.
This is not 4K60 or desktop Edge/Safari acceptance. #48 remains open for the
broader retained-size/startup audit and build-profile comparisons.


## Adaptive bitrate provider acceptance (2026-10-13)

`tools/node/abr-smoke.cjs` uses Playwright Chrome 155.0.8059.39 and the exact
full player from PR #57. Two fresh-browser runs pass. The fake provider advertises
100,000 and 1,000,000 bit/s levels. Both return the same committed 206,800-byte
`bbb_480p.ts` clip, preserving source timing to isolate switching policy.
Forty finite entries allow time for real MSE appends and buffer backpressure.

| Provider phase | Response delay | Completed variant requests, each run |
| --- | ---: | --- |
| Slow | 2,000 ms | high, low, low, low, low |
| Restored | 100 ms | low, low, low, low, high, high, high |
| Runtime code/dependencies added | — | 0 / 0 |

The test requires at least three slow low-level responses, three recovered
high-level responses, continued playback and recovered variant stats, with no
Failed/Unsupported/browser error. Cancelled requests are recorded separately.
All shipped bytes are unchanged. This models delayed complete-body throughput,
not packet loss, a shared bandwidth cap, perceptual quality or resolution changes.
It does not prove 4K60, Edge/Safari or dropped-frame acceptance. #9's provider
acceptance is covered; close after this test lands.

```sh
NODE_PATH=tools/browser/node_modules node tools/node/abr-smoke.cjs
```

Serve a built full player first; set `RSTREAMKIT_URL` for an alternate local URL.


## Mono AC-3 sound correction (2026-10-13)

Issue #59: the committed four-second `h264_ac3.ts` clip is mono and has audible
source sound (FFmpeg volumedetect mean −21.1 dB). Its original decoded stereo
checksum, `b928f2d0e7a08325`, was exactly the checksum of 768,000 zero bytes.
The old decoder-equality check therefore pinned silence. Earlier documentation
called this fixture stereo; its historical silent-output timing is not valid
evidence of correct audio processing speed.

LoRo and LtRt placed their mono gains in slot 0, while the mixer mapped the
source to center slot 1. Both now use slot 1. The existing gain stays unchanged.
Mono PCM intentionally changes; corrected checksum is `2f95f6c00b945b35`.
Other existing checksum gates remain unchanged. No dependency, unsafe code or
public API change is added; optional `sound` builds retain the same capabilities.

| Validation | Result |
| --- | ---: |
| Mono source duration | 4 seconds |
| Corrected mono vs FFmpeg score | 66.6 dB |
| Corrected mono correlation / level difference | 1.000 / about 0.0 dB |
| Chrome peak observed RMS, left / right | 0.064183 / 0.064183 |
| Chrome largest left/right sample difference | 0 |
| Chrome audio analyzer polls | 197 |

Native tests exercise positive/negative mono inputs in LoRo and LtRt and add
mono to the existing independent FFmpeg pipeline comparison. Stereo, 5.1 AC-3,
E-AC-3 and MP2 checks continue to pass. `tools/node/mono-smoke.cjs` uses Playwright
Chrome 155.0.8059.39 and Web Audio to require non-silent, equal channels from
actual MSE playback; completion alone would pass for silent sound too. The
analyzer feeds a zero-gain output to avoid playing test tones through speakers.
This observes decoded PCM, not physical speaker output or Edge/Safari support.

| Player download, Wasm + JS Brotli bytes | Before (0450007) | After | Change |
| --- | ---: | ---: | ---: |
| Live | 112,830 | 112,830 | 0 |
| Live + sound | 210,259 | 210,303 | +44 |
| Movie | 135,702 | 135,702 | 0 |
| Full | 232,817 | 232,654 | −163 |

All budgets pass with the same size recipe. Compression/layout differences do
not imply less CPU. Correct mono sound necessarily does more useful encoding
work than the prior silent output; no speedup claim is made for this fix.

```sh
cargo test --release --test pipeline ac3_eac3_and_mp2_sound_is_decoded_to_flac_that_matches_ffmpeg -- --nocapture
cargo test --release --manifest-path ac3/Cargo.toml stereo_from_mono
NODE_PATH=tools/browser/node_modules node tools/node/mono-smoke.cjs
python3 tools/size.py --check-budgets --output tools/results/size-mono-audio.json
```

FLAC experiment decision: do not retain the attempted constant-space residual
search. On the same nonconstant two-second 5.1 AC-3 corpus, an early native
variant reduced allocations from 378 to 126 but slowed median encoder time from
2.168 to 4.483 ms. Borrowing channel samples while retaining a residual vector
still measured 2.239 → 2.389 ms. These are targeted native experiments, not final
shipped-code or Wasm benchmarks. The original FLAC writer remains unchanged;
its speed/space tradeoff needs more profiling under #45/#48.

Mono validation also passes formatting, all eight native/Wasm clippy builds,
default/minimal/vod tests, the complete vendored decoder suite and five existing
Playwright playback/recovery checks.


## Live provider recovery acceptance (2026-10-13)

Playwright Chrome 155.0.8059.39, exact full build from PR #60. The provider uses
the committed `bbb_480p.ts` clip so repeated media isolates transport and playlist
policy. Tests add zero runtime code, dependencies or download bytes.

| Case | Real browser result |
| --- | --- |
| One segment returns 404 | One request for the bad segment, following good segment fetched; playback passes 5.8 s, beyond the first 4.59 s segment |
| One segment returns HTML with HTTP 200 | Skipped as invalid TS; following good segment fetched; playback passes 5.8 s |
| Ten-second HTTP 503 outage | Five attempts at about 0, 1, 3, 7 and 15 s; four Reconnecting states, then playback |
| Pause with eviction | 50.024 s paused across a 9.179778 s window (5.449 windows); old resource probe returns 404; fresh segments 10 and 11 fetched |
| Resume after eviction | Continued playback after the seek and latency below 17 s observed within 2.237 s; reported latency 9.551 s |

All four cases have no Failed/Unsupported/browser error. The outage and pause
were each run twice; final tests strengthen acceptance by requiring playback
past the bad-segment boundary and real advancement after the live-edge seek.
The first pause run took 50.009 s and reached the target after a seek in 0.360 s;
that earlier timing is not evidence of continued post-seek playback. The final
2.237 s result includes that stronger check. Latency is a player snapshot at
append time, not a continuously sampled wall-clock live-edge measurement.

Old resources are actually evicted in the pause provider, closing the retained-
file gap in the earlier test. HTTP 503 responses simulate an outage; this does
not reproduce physical network disconnection, codec changes or real provider
content. #2 and #3 acceptance are covered; close after the test PR merges.

```sh
NODE_PATH=tools/browser/node_modules node tools/node/resilience-smoke.cjs
NODE_PATH=tools/browser/node_modules node tools/node/pause-smoke.cjs
```

Serve the built full player first; `RSTREAMKIT_URL` can select another local URL.
These are standalone Playwright checks, not yet CI jobs or a real-time 4K soak.


## Stereo conversion buffer reduction (2026-10-13)

`decode::stereo` always allocated an output vector, then replaced it with the
already converted PCM when input was stereo. Return that PCM before allocating
the other-layout buffer. This changes no samples or public API; it reduces a
constant factor, not the O(n) time/space class. No runtime dependency is added.

Baseline `9da40bf` (PR #61). Release/native, Linux Ryzen 7 5700 / Rust 1.99.0.
The preserved old helper and new helper receive the same 1,536-sample, two-channel
byte buffer. Fixture construction is excluded. Nine alternating pairs of 10,000
calls follow 100-call warmups; p95 is the largest batch average. The test allocator
forwards System calls and checks a disabled thread-local counter during timing;
these sub-microsecond helper timings include that instrumentation and do not
measure full decoder CPU or a Wasm speedup.

| Stereo conversion only | Before | After |
| --- | ---: | ---: |
| Heap allocations | 2 | 1 |
| Requested buffer bytes | 12,288 | 6,144 |
| Median microseconds, final repeat | 0.125686 | 0.110222 |
| p95 microseconds, final repeat | 0.171303 | 0.137672 |
| Chrome mono-run committed Wasm bytes | 2,490,368 | 2,490,368 |

Requested bytes exclude input, other decoder state and allocator overhead.
The first native run measured medians 0.125723 / 0.108664 µs. The reliable win
is one fewer 6,144-byte buffer per normal AC-3 stereo frame; committed Wasm
memory did not decrease in this short browser run. Output-equivalence tests
cover mono/stereo/three/six/eight-channel inputs, short inputs and trailing bytes;
allocation tests cover 1, 32, 1,536 and 4,096 samples. Existing decoder checksum
and independent media gates all pass. The production FLAC search is unchanged.

| Player download, Wasm + JS Brotli bytes | Before | After | Change |
| --- | ---: | ---: | ---: |
| Live | 112,830 | 112,778 | −52 |
| Live + sound | 210,303 | 210,311 | +8 |
| Movie | 135,702 | 135,683 | −19 |
| Full | 232,654 | 232,807 | +153 |

All budgets pass; code-generation/compression layout can change artifact bytes
even when a feature's runtime path is unchanged. Full grows 0.066% for the buffer
reduction. The mono correction and this change together leave full download
10 bytes smaller than PR #57, with the same features and size recipe.

Formatting, all eight native/Wasm clippy builds, default/minimal/vod tests, the
vendored decoder suite, four Node Wasm audio checks and five existing Playwright
Chrome playback checks pass. The mono check exposed a test timing race: separate
AnalyserNode snapshots can straddle audio render blocks. It now compares both
channels within one AudioWorklet block. Two fresh runs each observe 1,824 blocks,
peak RMS 0.066115 in both channels and zero sample difference. The worklet sends
silent output; no sound is played through physical speakers. Test-only code adds
no shipped audio worker or UI framework.

```sh
cargo test --release --lib measure_stereo_conversion -- --ignored --nocapture --test-threads=1
python3 tools/size.py --check-budgets --output tools/results/size-stereo-allocation.json
NODE_PATH=tools/browser/node_modules node tools/node/mono-smoke.cjs
```


## Per-package browser profile tuning (2026-10-13)

The supplied small player now uses Rust `z` for the core, `s` for the Dolby
and codec-support crates, and `3` for MP2. Native library/CLI release settings
are unchanged. No dependency, feature or runtime API is added. This is a compiler
profile change, not a new decoder or renderer.

All candidates use identical source, Rust 1.99.0, wasm-bindgen 0.2.128,
wasm-opt 130 `-Oz` and Brotli 1.2.0 quality 11. Same advertised features.
The full-player payload tradeoff determined which candidates to reject:

| Core / codec settings | Full Wasm + JS Brotli bytes | Decision |
| --- | ---: | --- |
| All `s` (before) | 232,807 | Reference |
| All `z` | 224,704 | Reject as default: 5.1 AC-3 processing slows about 22% |
| All `3` | 282,946 | Reject: download budgets exceeded |
| Core `z`, all codecs `s` | 227,367 | Lean option; modest processing gains |
| Core `z`, all codecs `3` | 249,364 | Reject as default: full and sound budgets exceeded |
| Core `z`, Dolby/core `s`, MP2 `3` | 232,290 | Selected: fits budgets and improves MP2 processing about 18% |

The selected MP2 setting costs 4,923 full-player bytes over the lean option.
Compared with all-s, full is still 517 bytes smaller; sound-only grows 1,108.
Hosts that value those 4.9 KB over MP2 processing can reproduce the lean setting
with `--opt-level z --codec-opt-level s`. This trades CPU for download size without
removing codec support.

| Player download, Wasm + JS Brotli bytes | All-s before | Selected after |
| --- | ---: | ---: |
| Live | 112,778 | 107,969 |
| Live + sound | 210,311 | 211,419 |
| Movie | 135,683 | 128,972 |
| Full | 232,807 | 232,290 |

All four existing budgets pass. `size.py` reads defaults from the player manifest
and records effective per-package settings; environment variables no longer
silently change the four documented release settings. Python 3.11+ is needed for
the standard-library TOML reader. Reports still count Wasm + generated JS, not
the diagnostic HTML page or media downloads; whole-app startup remains #48 work.

CPU: Linux x86_64 Ryzen 7 5700, Node 26.11.1, identical development harnesses
optimized with `wasm-opt -Oz`. Each comparison runs in a fresh process with both
modules present, five warmups, then nine alternating pairs of five-call batches.
p95 below is the largest batch average, not individual-call tail latency. Two
fresh comparisons per candidate pass PCM-checksum and output-length gates.
Transmux timings include input copying and fresh decoder construction per call;
FLAC uses predecoded PCM. The harness is not the minimal player's exact call path.

| Selected-profile workload | Median ms before → after | p95 batch ms before → after |
| --- | ---: | ---: |
| AAC TS, 4.59 s | 0.1268 → 0.1306 | 0.2045 → 0.2248 |
| Stereo AC-3 TS, 2 s | 7.2707 → 6.8162 | 7.8906 → 7.0313 |
| 5.1 AC-3 TS, 2 s | 10.8378 → 10.2926 | 11.1078 → 10.6257 |
| 5.1 E-AC-3 TS, 2 s | 12.4971 → 11.9587 | 13.0465 → 13.0805 |
| MP2 TS, 2 s | 16.3837 → 13.3651 | 16.6666 → 13.5365 |
| FLAC from AC-3 5.1 PCM | 4.1267 → 3.6480 | 4.1427 → 3.8399 |

AAC is slightly slower in this run; E-AC-3's measured tail is slightly higher.
This is not a universal speedup. The first selected-profile comparison measured
MP2 medians 16.4191 / 13.4112 ms (see JSON for exact results if regenerated);
CPU ratios are meaningful only within each alternating pair. Committed harness
memory matches within each pair (1,900,544 bytes after AAC; 2,621,440 after the
audio workloads). No overall memory or FFmpeg advantage is claimed.

A headless Chrome 155.0.8059.39 control uses the same existing 18-second,
3840×2160/60 SDR, 40 Mbps clip, viewport 1440×900 and visible document state.
Both all-s and selected-profile runs finish with zero waiting events and zero
reported dropped frames. The source has 1,080 frames; Chrome reports 1,081 in
both runs, so that counter is not proof of 1,081 unique source frames. Short
checks do not settle the long-run target-device matrix. Peak sampled committed
Wasm is 39,059,456 bytes for all-s and 38,010,880 for selected. Each is only one
run; chunk delivery/allocator variation prevents treating the difference as a
proven profile memory saving. Long-task observations are two at 52/53 ms versus
one at 58 ms; they do not establish a general responsiveness improvement.

All required root Rust checks, decoder/media tests, harness clippy and four
size budgets pass. Playback/audio/ABR checks run against the exact selected
player; Edge/Safari and real-time 4K endurance remain open.

```sh
python3 tools/size.py --check-budgets --output tools/results/size-balanced-profile.json
python3 tools/profile_harness.py --profile s --profile balanced
node tools/node/opt-compare.cjs tools/results/profiles/s/rstreamkit_harness.js tools/results/profiles/balanced/rstreamkit_harness.js
```

The builder saves tool versions, source/artifact hashes, dirty state and the
resolved harness lockfile. The comparison records hardware, input hashes, PCM
checksums, memory and timings. Reproducing the selected harness yielded the same
Wasm artifact hash as the measured candidate. Module load/init numbers in the
Node report are diagnostic; this is not a browser cold-start benchmark.


## Adaptive bitrate failure fallback (2026-10-13)

Issue #9 also required fallback when a selected level repeatedly fails. The old
player skipped every bad segment but never called bitrate selection on that path.
Two consecutive unusable live segments now select the nearest lower compatible
level. A healthy segment resets the streak even if no usable timing sample exists.
Eight healthy segments delay reconsidering an upward retry; the existing three
headroom checks then apply. Further downward steps remain available during recovery.
Manual locks suppress automatic fallback, and a new manual quality request can
apply even when the current level has no usable segment. Finite errors still fail.

Variant changes use one existing apply/fetch/rendition path after the batch, rather
than duplicating it for errors. Counters fit in the previous state padding by using
bounded u8 fields. Policy state remains 16 bytes on this x86_64 target; the regression
compares against the previous field layout so it is portable to different ABIs.
No extra heap storage or dependency is added. Selection remains O(variant count)
when needed; normal per-segment counter work is constant.

| Failed-encoder provider case | Before | After |
| --- | ---: | ---: |
| Healthy lower-level segment requests | 0 | 4 |
| Failed higher-level requests before fallback | No fallback | 2 |
| New native failure/control cases | 0 | 4 |
| Policy state on x86_64, bytes | 16 | 16 |

The exact final Playwright fixture fails on preserved PR #63 artifacts (zero
lower requests) and passes on the fixed full build. High sequences 37 and 38
return 404; the healthy encoder has published through 42, and low sequences
39–42 play. This preserves sequence progress rather than replaying skipped
segments. Playback passes 6 seconds, stats report low quality and two skipped
segments, and no Failed/Unsupported/browser error occurs. Four native cases cover
nearest compatible fallback, lowest/empty sets, successful streak reset without
timing, recovery hysteresis and manual choices (some cases combine related checks).
Manual controls are tested in native policy; the small JS demo does not expose them.

| Wasm + JS Brotli download bytes | Before (PR #63) | After |
| --- | ---: | ---: |
| Live | 107,969 | 108,210 |
| Live + sound | 211,419 | 211,787 |
| Movie | 128,972 | 129,224 |
| Full | 232,290 | 232,741 |

All four budgets pass. Full adds 451 bytes (0.19%) for the reliability fix. Native
and Wasm lint/feature gates, media/decoder tests, normal provider downshift/upshift
and five existing Chrome playback cases pass. This is live-segment recovery;
it does not promise recovery from every manifest, source-close or unlimited outage.
No CPU speedup or broad 4K/Edge/Safari claim is made.

```sh
NODE_PATH=tools/browser/node_modules node tools/node/abr-failure-smoke.cjs
NODE_PATH=tools/browser/node_modules node tools/node/abr-smoke.cjs
```

The failure check uses repeated committed media with advancing lower-level
availability to isolate policy. For the before case, set `RSTREAMKIT_PLAYER_DIR`
to a directory containing PR #63's full-player JS and Wasm artifacts.


## Parser integration check (2026-10-13)

The bounded parser fixes were integrated with the merged retry/session, audio,
profile and ABR work. Exact integration commit `de1964e`: all required native/
Wasm feature checks, native feature test sets, decoder suite, coverage-target
compilation/lints and a fresh seeded 5,000-case campaign pass. Seed 20261013;
1,000 cases per target, no failure or limit breach in the tested corpus.
Earlier 55,000 cases remain historical evidence for the earlier parser branch.

| Target | Fresh cases passed | Child wall seconds |
| --- | ---: | ---: |
| HLS | 1,000 | 0.033 |
| TS | 1,000 | 0.165 |
| MP4 | 1,000 | 0.065 |
| Matroska | 1,000 | 0.266 |
| CMAF | 1,000 | 0.066 |

Limits remain 1 MiB input, 512 MiB address space, 30 CPU seconds and 45 wall
seconds per target campaign, with at most 32 MKV probe requests per input.
No audio decoding or coverage-guided/ASan campaign is claimed here. Four normal
regressions preserve the original findings; parser tooling adds zero library
runtime dependencies and stays outside the default test path.

| Wasm + JS Brotli bytes | Main before parser integration | Integrated |
| --- | ---: | ---: |
| Live | 108,210 | 108,210 |
| Live + sound | 211,787 | 211,787 |
| Movie | 129,224 | 129,283 |
| Full | 232,741 | 232,644 |

All budgets pass. Small code-generation/compression differences are not evidence
that bounds checking universally shrinks programs. Five Chrome playback cases
pass on the exact integrated full build. Refresh PR #51 CI before merging; #43
closes only after its seeded CI run and integration are confirmed on main.

### Browser-test integration — 2026-10-13

The guarded sequence lookup now lives in `Session::next_index`, alongside the shared playlist controller. Normal consecutive sequences check one offset and full segment identity; broken numbering, changed URLs and byte ranges keep the linear recovery path. The batch iterator already walks playlists once; this shortcut helps repeated individual lookups without adding an index or runtime dependency.

| Measure | Main after parser PR #51 | Integrated browser PR #50 |
| --- | ---: | ---: |
| Automated Chrome playback regressions in CI | 0 | 5 |
| Live compressed Wasm + JS | 108,210 B | 108,342 B |
| Live with sound | 211,787 B | 211,823 B |
| VOD | 129,283 B | 129,294 B |
| Full | 232,644 B | 232,581 B |

All four budgets pass. The final native lookup-only benchmark walks 20,000 segments in a median 0.311 ms over seven release runs (0.289–0.324 ms, Ryzen 7 5700/Linux, Rust 1.99.0). The old 120.452 ms result above predates the session refactor; it is historical, not a fresh main comparison. This is not a whole-player or FFmpeg speed claim. Fresh integrated validation also passes the root feature checks, decoder suite and 5,000 bounded seeded parser cases.

### Preserve selected audio across variants — 2026-10-13

Changing quality now keeps the selected external audio language. Changing track URLs also keeps the consumed segment and transmuxer clock, so a late selection continues from the current timeline. The selector scans existing rendition metadata once, uses constant extra space, and adds no runtime dependency. Harness-only controls let the regression exercise the public Rust Player API without adding minimal-player exports.

| Measure | Main after PR #50 | Language fix |
| --- | ---: | ---: |
| Chrome CI playback regressions | 5 | 6 |
| Live compressed Wasm + JS | 108,342 B | 108,356 B |
| Live with sound | 211,823 B | 211,684 B |
| VOD | 129,294 B | 129,327 B |
| Full | 232,581 B | 232,742 B |

All budgets pass. A mixed master advertises a higher-bandwidth HEVC variant; Chrome fetches none of its playlists and plays H.264. After six seconds, the test changes English (440 Hz) to Spanish (880 Hz), then selects the lower variant. Sequence 10 is the first lower-level audio; its playback begins around 14 seconds, and the test observes Spanish tone samples after 14.5 seconds. The standalone final run reached 15.36 seconds with 17 matching lower-level tone samples, no English tone after that boundary, and no failed status. A second run inside the six-test Playwright suite also passes. The same final test fails against main's runtime at the language-preservation assertion.

Fixtures are generated continuous TS segments; both quality levels share video bytes to isolate routing. An earlier repeated-clip experiment had artificial clock resets and is not acceptance evidence. Web Audio verifies decoded PCM, not speaker hardware. This covers the software routing acceptance for #8; Edge/Safari and long high-resolution acceptance remain separate.

### Real-time live catch-up acceptance — 2026-10-13

`tools/node/catchup-smoke.cjs` with installed Chrome 155 tests issue #14 using real wall time. The provider publishes a repeated generated two-second TS clip every two seconds and retains 120 seconds of input, so recovery can download every missed segment. Segment 4 is held for 12.501 seconds; playback genuinely waits after segment 3 has drained.

| Measure | Observed |
| --- | ---: |
| Actual playback stall | 10.474 s |
| Largest sampled latency | 16.196 s |
| Final reported latency | 7.961 s |
| Configured target / upper control band | 6 / 8 s |
| Recovery after playback resumes | 220.878 s |
| Catch-up / final playback rate | 1.05 / 1.00 |
| Seeks during catch-up | 0 |
| Largest currentTime step above elapsed wall time | 0.008688 s |
| Final committed Wasm memory | 2,293,760 B |
| Runtime code or download bytes added | 0 |

The bounded acceptance is 240 seconds after resumption to reach the existing target-plus-two-second band and restore 1× playback. This is not a claim that latency reaches exactly six seconds: the existing controller has a two-second deadband. With only 5% extra playback speed, ten seconds of delay takes roughly 200 seconds to remove. The test checks no seek or abrupt timeline jump, not physical display quality. Its headless report has 374 dropped frames out of 6,940 (5.39%); do not use it as smooth-rendering or 4K60 evidence. Repeated media isolates the policy, and this four-minute run is not a soak test. Reports stay in ignored `tools/results/catchup-smoke.json`.

### Smaller stats output — 2026-10-13

A Twiggy 0.8 retained-byte audit identified floating-point diagnostic formatting in the minimal player. The wrapper now prints seconds to three decimal places and bitrate to whole bits per second, keeping the same field names. Rust `Player::stats()` values and playback calculations keep full precision. No runtime dependency or codec feature changes.

| Compressed Wasm + JS | Before | After | Saved |
| --- | ---: | ---: | ---: |
| Live | 108,356 B | 107,362 B | 994 B |
| Live with sound | 211,684 B | 210,912 B | 772 B |
| VOD | 129,327 B | 128,537 B | 790 B |
| Full | 232,742 B | 231,976 B | 766 B |

The same-checkout full optimized Wasm shrinks from 580,195 to 577,582 bytes; all four budgets pass. A separate temporary-checkout experiment saved 617 compressed full bytes, but the table uses the controlled review-checkout build. Source hashes, exact lock resolution, tool versions and dirty status are recorded by `tools/size.py` in ignored `tools/results/size-stats-final.json` and the prior `size-language-final.json`.

The named raw compiler Wasm retains the AC-3/E-AC-3 and MP2 decoder paths, including JOC and object metadata; deleting unused source files alone is not a demonstrated size win. Twiggy lists the float Debug formatting root at 16,658 retained bytes before and the Display root at 15,053 after. These are overlapping, conservative call-graph totals, not additive shipped-byte savings. The actual shipped file is stripped and uses numbered functions; it cannot be assigned named subsystem totals from that report. The raw build also includes 362,361 bytes of wasm-bindgen metadata and 246,517 bytes of function names before stripping; neither is a new optimization opportunity in the shipped file. Audit outputs are `tools/results/retained-raw.txt`, `retained-shipped.txt` and `stats-retained-final.txt`.

Validation: all four minimal-player Wasm clippy feature combinations, formatting, six real-Chrome regressions. This is a download and diagnostic-format change; no processing-time or memory improvement is claimed. Total HTML startup assets, cold-start distributions and further codec feature tradeoffs remain under #48.

### Budgeted audio yielding and bounded raw output — 2026-10-13

The browser raw-TS adapter now feeds 16 KiB pieces, appends returned fragments before continuing, and cooperatively decodes its final fragment. It checks cancellation during processing and buffer waits. Additional outputs waiting for append are bounded by each piece and the existing GOP buffer, rather than by the whole transport chunk; the transport's input allocation itself is unchanged. Native synchronous methods remain available.

The shared browser audio decoder checks elapsed work every eight coded frames and yields after roughly eight milliseconds. One frame or final assembly can exceed that slice. A budget is shared across the raw feed's fragments, so many short GOPs cannot bypass it. It uses global performance.now(), with a Date.now() fallback. This replaces fixed-count timer waits in HLS too. The time/space complexity of decoding remains linear in samples; the change controls scheduling, timer overhead and pending raw output.

| Browser harness measurement | Fixed eight-frame yield | Final eight-ms slice |
| --- | ---: | ---: |
| Three repeated two-second AC-3 clips, median elapsed | 108.8 ms | 35.2 ms |
| Ten repeated two-second AC-3 clips, median elapsed | 418.1 ms | 148.6 ms |
| Full compressed player (before raw work / final) | 231,976 B | 232,961 B |
| Chrome CI regressions | 6 | 7 |

Each timing uses five alternating synchronous/yielded pairs after two warm-ups, with 320×180 HLS playback in the background. The final repeat's synchronous controls are 35.1 ms and 113.8 ms. Six-second-sized probes have zero long tasks in either mode; the stress control has five 114–116 ms long tasks, while budgeted yielding has zero. Both modes drop zero frames in each short paired sample. The first final-policy campaign similarly measures 36.2/35.9 ms and 114.7/152.3 ms. Thus there is no measured dropped-frame win here, and yielding still adds elapsed time in the large stress case. These are development-harness results (opt-level s), not a universal CPU speedup or 4K60 result.

Fifteen byte checks cover AAC, mono AC-3, 5.1 AC-3, 5.1 E-AC-3 and MP2 at 187-byte, 16-KiB and whole-input chunk sizes. Synchronous and yielded output hashes, byte counts and fragment counts agree. A generated continuous six-second H.264/5.1 AC-3 raw stream reaches 6.016 seconds and EOF, with 180 frames, zero dropped frames, peak observed audio RMS 0.020601 and no failed state. Chrome 155.0.8059.39, Ryzen 7 5700/Linux. The final measured harness Wasm SHA-256 is 1bf1c9287673a7017b3647d8a16711d83e2c0c1e0bf8a93a78382e4c1a735c51.

| Compressed Wasm + JS | Before | Final |
| --- | ---: | ---: |
| Live | 107,362 B | 108,121 B |
| Live with sound | 210,912 B | 211,720 B |
| VOD | 128,537 B | 129,213 B |
| Full | 231,976 B | 232,961 B |

All budgets pass. Root native/Wasm clippy passes in four feature sets, native tests pass in three sets, harness lint/format checks pass, and seven Chrome regressions pass. The decoder suite passed before the timer-policy refinement; decoder code was unchanged. Reports: tools/results/raw-yield-smoke.json (fixed-count experiment), raw-budget-smoke.json and raw-budget-repeat.json (final policy), raw-playback-final.json, size-raw-budget.json.

A separate repeated-clock playback diagnostic still ends early at 4.074666 seconds instead of roughly six. The prior synchronous runtime reproduces it, so it is not hidden as a successful playback case. Run the raw playback helper with RSTREAMKIT_REPEAT_RAW=1; this remains timeline/discontinuity work under #16/#25. Movies still use synchronous range processing, so #34 remains open.

### Short raw-clock restarts and real HLS boundaries — 2026-10-14

A three-clip raw stream could end early because short backward timestamp resets fit inside the old two-second forward-gap allowance. Track base times were reused, overwriting earlier output. Completed audio PES packets could also wait for another packet start, crossing a clock reset; numeric timestamp partitioning then reordered queued audio. The engine now detects overlap per track, emits completed length-delimited audio PES packets promptly, and partitions queued samples within their current clock epoch. Zero-length PES keeps its existing fallback. No dependency or per-sample index is added. The metadata split stays linear and uses the same two output vectors; it adds a linear scan to identify the epoch boundary.

| Three repeated two-second raw clips | Before | Fixed |
| --- | ---: | ---: |
| Playback endpoint | 4.074666 s | 6.122666 s |
| Browser frame counter | 50 | 150 |
| Fixed-run dropped frames | — | 0 |
| Full compressed Wasm + JS | 232,961 B | 233,328 B |
| Chrome CI regressions | 7 | 8 |

Native tests preserve every source picture and byte-for-byte coded audio order across AAC, mono/stereo/5.1 AC-3, 5.1 E-AC-3 and MP2, with 187-byte, 16-KiB and whole-input feeds: 36 property scenarios across two tests. Track times join exactly. Two additional real-fixture unit tests cover early complete-PES delivery and the zero-length fallback. The old runtime fails audio order and clock continuity; its Chrome counter stops at 50. The fixed raw check has a single buffered range [0.08, 6.122666], peak observed PCM RMS 0.036585 and no failed status. Browser frame counters are not a guarantee of unique physical presentations.

Issue #16's real-stream acceptance uses the [Mux-hosted public test catalog](https://test-streams.mux.dev/) and its 720p Big Buck Bunny media playlist: https://test-streams.mux.dev/x36xhzz/url_0/193039199_mp4_h264_aac_hd_7.m3u8 . The playlist SHA-256 is ea751cf5a236a8e2742cb940e9eff0acfb2e84e1cead9aee10258a43b32910c3. Its durations include 9.95- and 10.05-second segments.

| Real-network Chrome check | Result |
| --- | ---: |
| Wall time / media position | 120.886 / 120.717 s |
| Resolution | 1280 × 720 |
| Checked boundaries | 11 |
| Buffered ranges at every checked boundary | 1 |
| Dropped-frame increase at every boundary | 0 |
| Whole-run browser frame counter / dropped | 7,247 / 0 |
| Post-start stalls / seeks / hidden time | 0 / 0 / 0 |
| Callback media-time step at boundaries | 16.655–16.667 ms |
| Downloaded TS responses | 17 |
| Final Wasm committed memory | 12,255,232 B |

The standalone optional network script is tools/node/boundary-smoke.cjs; it is excluded from deterministic CI because the external source/network can change. Report: tools/results/real-boundary.json. Callback timing can be delayed by JavaScript, so the dropped-frame assertions use getVideoPlaybackQuality. This is one real 720p60 VOD source, not a 4K60, real-live provider matrix, VFR guarantee or long soak.

| Compressed Wasm + JS | Before | Fixed |
| --- | ---: | ---: |
| Live | 108,121 B | 108,548 B |
| Live with sound | 211,720 B | 212,290 B |
| VOD | 129,213 B | 129,660 B |
| Full | 232,961 B | 233,328 B |

All budgets pass. Root native/Wasm feature lints, three native test configurations, the decoder suite, fresh 5,000 bounded parser cases (seed 20261014), Node output checks and eight Chrome playback regressions pass. Added PES tests and expanded fixture arrays also pass their targeted checks. Reports remain in ignored tools/results. Issue #16 acceptance is covered on Chrome; other platforms, provider quirks and endurance remain in #24/#25/#47.

### Cooperative movie ranges and seek safety — 2026-10-14

Movie playback now uses vod::Session::push_yielded on Wasm, with the same eight-millisecond work-budget clock as HLS and raw TS. Synchronous and yielded paths share range extraction, frame selection and fragment packaging. AAC remains borrowed; decoded sound uses the existing FLAC output. Native synchronous playback is retained. The browser rechecks cancellation and seek position after decoding, because a user can now seek while the decoder yields. A cancelled library processing future may have advanced its cursor/decoder; discard that session.

| Six-second development-harness movie, repeat campaign | Synchronous | Yielded |
| --- | ---: | ---: |
| MP4 median elapsed | 32.2 ms | 32.6 ms |
| MKV median elapsed | 33.0 ms | 33.6 ms |
| Timer callbacks during each processing call | 0 | 1 |
| Long tasks in five calls, each format | 0 | 0 |
| Dropped background frames in each format | 0 | 0 |

Five alternating pairs after two warm-ups per method/format, Chrome 155/Ryzen 7 5700/Linux, harness opt-level s. Both methods include range indexing, input copying and development output hashing. The first campaign measures 31.4/31.5 ms for MP4 and 31.5/31.4 ms for MKV. There is no measured CPU speedup or dropped-frame win on these short inputs. The choice is to keep the event loop responsive with a small elapsed-time cost, while retaining the larger stress-case long-task improvement recorded above. This does not promise every frame/assembly/cold compilation stays below eight milliseconds.

Nine output comparisons cover AAC MP4, AC-3 MP4 and AC-3 MKV at 4-KiB, 64-KiB and whole-file range limits. Complete output hashes, byte counts and requested ranges match. Node also checks synchronous/yielded movie output equality, using the same global timer clock outside the browser. Measured harness Wasm SHA-256: f86eb8d90e30dce785d45ff9c1030787b6e054510d84de65e9d9809adc36a0db.

| Real browser playback check | Endpoint / result |
| --- | ---: |
| AC-3 MP4 | 6.010667 s, RMS 0.078334 |
| AC-3 MKV | 6.016 s, RMS 0.078333 |
| AES-128 HLS with AC-3 | 6.016 s, RMS 0.025189 |
| Seek injected during an audio yield | Reaches 15.909333 s after seeking to 14 s |
| First appended video base after that seek | 14 s |
| Failed statuses across all four cases | 0 |

The seek case uses a generated 40-second file; a zero-delay decoder timer triggers the seek. Appended video starts after the seek are 14, 18.9, 24, 28.9667, 34 and 39.0333 seconds: the old decoded range is discarded. All four cases have zero reported drops. MP4/MKV report 144 frames each; the generated encrypted stream reports 184 (browser counters are not a unique-frame guarantee). PCM is observed through Web Audio with silent output, not physical speakers.

| Compressed Wasm + JS | Before | Movie change |
| --- | ---: | ---: |
| Live | 108,548 B | 108,548 B |
| Live with sound | 212,290 B | 212,290 B |
| VOD | 129,660 B | 130,352 B |
| Full | 233,328 B | 234,032 B |

All budgets pass. Root feature lints/tests, harness checks, Node decoder/continuous/movie checks and all nine Chrome CI cases (the ninth contains four movie/encryption/seek checks) pass locally. Rust 1.88 is not installed locally; GitHub's MSRV job is required before merge. The new integer predicate is [supported since Rust 1.87](https://doc.rust-lang.org/std/primitive.usize.html#method.is_multiple_of), and the full API must still pass that CI job. Reports: tools/results/movie-yield-smoke.json, movie-yield-repeat.json, movie-playback-smoke.json and size-movie-yield.json. External hardware/browser and high-resolution endurance acceptance remain separate.

### Complete player startup accounting — 2026-10-14

The size report now includes the shared HTML page, with its inline styles and
scripts. No external font, image, UI framework or renderer is required. Each
build makes exactly three startup requests: HTML, generated JavaScript and Wasm.
A data-URI icon prevents an implicit favicon request. The HTML is 8,569 raw bytes
and 2,750 Brotli bytes; it has a separate 4,096-byte budget in CI. The existing
engine budgets and `startup_brotli_bytes` fields retain their Wasm + JS meaning.
Schema 2 adds `shared_startup_files`, `complete_startup_bytes` and
`complete_startup_brotli_bytes`, and hashes the shell source too.

| Build | Engine Brotli | Complete Brotli | Ready median (range) | Initial committed Wasm |
| --- | ---: | ---: | ---: | ---: |
| live | 108,548 B | 111,298 B | 33.4 ms (25.1–98.1) | 1,179,648 B |
| live-sound | 212,290 B | 215,040 B | 39.1 ms (25.9–59.4) | 1,245,184 B |
| vod | 130,352 B | 133,102 B | 39.5 ms (35.6–44.3) | 1,179,648 B |
| full | 234,032 B | 236,782 B | 40.8 ms (35.8–70.7) | 1,245,184 B |

This is a more complete measurement, not a new engine speed or size win. The
full engine remains 234,032 compressed bytes, unchanged from PR #70. The shell
adds 2,750 bytes to its accounted download. The earlier 20% full-player reduction
goal is not met. Feature removal is not counted as a same-feature saving.

Measured using Chrome 155.0.8059.39 on AMD Ryzen 7 5700, Linux, Node
v26.11.1. Five fresh Chrome processes per flavor, alternating flavor order,
use uncompressed loopback HTTP and a warm OS file cache. The page records readiness
at its ready setter, measured from navigation; process launch is excluded. Asset
inventory waits for network idle. Initial memory is taken before playback and
excludes JS, browser decoder, GPU and process RSS. These small samples are not
production cold-cache percentiles or 4K playback measurements. CI checks asset
identity and successful startup, without a timing threshold that would vary
with runner load.

Reproduce with `python3 tools/size.py --check-budgets`, the development server,
and `NODE_PATH=tools/browser/node_modules node tools/node/startup-smoke.cjs`.
Set `RSTREAMKIT_URL` to the server origin. Reports: `size-complete-startup.json`
and `startup-complete.json` under ignored `tools/results/`; they record source
and exact emitted asset hashes. The retained-symbol and build-profile results
above remain the CPU/size tradeoff evidence. Playback memory and processing
measurements are separate; this startup test does not replace them.

### Reuse FLAC channel scratch storage — 2026-10-14

The encoder now reuses the sample and prediction scratch vectors for its second
channel. It keeps the same predictor search, arithmetic and output bytes, without
retaining buffers between calls. Complexity remains O(n) time and O(n) temporary
space per frame. The improvement is less allocation churn, not a smaller
asymptotic bound or demonstrated peak-memory reduction.

| One 1,536-sample stereo frame, number 128 | Before | After |
| --- | ---: | ---: |
| Nonconstant frame allocations | 6 | 4 |
| Nonconstant total allocation requests | 30,749 B | 18,461 B |
| Constant frame allocations | 4 | 3 |
| Constant total allocation requests | 18,461 B | 12,317 B |
| Reallocations, either case | 1 | 1 |

Counts include the returned frame and its number encoding. Reusing the two
scratch buffers halves their total requested bytes; the total nonconstant
frame requests fall 40%. This is native test-allocator evidence, not browser
RSS or committed-memory savings. Each frame drops both scratch buffers.

Three fresh Node processes each compare nine alternating pairs of five calls,
after five warmup calls per module. Both modules use the shipped balanced
profile, Rust 1.99.0 and wasm-opt -Oz. Medians below are the medians of the three
process medians, on Ryzen 7 5700 / Linux; hashes and tool versions are recorded
in each build report. FLAC runs use predecoded PCM; transmux includes input
copying, decoding and packaging. The largest observed paired whole-transmux
regression was 1.8%, with mixed signs across repeats. No CPU speedup is claimed;
this does not meet issue #45's 10% CPU gate and leaves that issue open. The
allocation improvement is retained for #48.

| Input / stage | Before | After |
| --- | ---: | ---: |
| bbb_480p.ts / transmux | 0.145 ms | 0.147 ms |
| h264_ac3_st.ts / transmux | 6.922 ms | 6.921 ms |
| h264_ac3_st.ts / flac | 4.065 ms | 4.036 ms |
| h264_ac3_51.ts / transmux | 10.453 ms | 10.442 ms |
| h264_ac3_51.ts / flac | 3.696 ms | 3.695 ms |
| h264_eac3_51.ts / transmux | 12.212 ms | 12.160 ms |
| h264_eac3_51.ts / flac | 3.699 ms | 3.677 ms |
| h264_mp2.ts / transmux | 13.542 ms | 13.521 ms |
| h264_mp2.ts / flac | 4.007 ms | 3.983 ms |

| Engine compressed Wasm + JS | Before | After |
| --- | ---: | ---: |
| live | 108,548 B | 108,638 B |
| live-sound | 212,290 B | 212,104 B |
| vod | 130,352 B | 130,422 B |
| full | 234,032 B | 234,117 B |

All engine and shell budgets pass. Full engine grows 85 bytes (0.04%); including
the unchanged HTML, full startup is 236,867 Brotli bytes. Live and VOD also grow
slightly because the FLAC writer is available without codec decoders. The
sound-only build shrinks 186 bytes; compression/code generation can differ by
feature set.

Complete-output fingerprints match the unchanged encoder for five integer
signal patterns × six frame sizes × five frame numbers (150 frames). Patterns
include constant, ramp, seeded noise, extremes and mixed constant/predictable
channels. The existing independent FFmpeg lossless roundtrip and programme
conversion checks pass, as do all native feature checks and Node decoder/output
checks. Chrome playback validation is recorded with the final PR checks.

Reproduce the allocation report with
`cargo test --release measure_flac_scratch -- --ignored --nocapture`.
Paired builds use `tools/profile_harness.py --profile balanced`, then
`tools/node/opt-compare.cjs BEFORE.js AFTER.js`. Ignored reports are
`flac-scratch-native-before.log`, `flac-scratch-native-after.log`,
`flac-scratch-paired-{1,2,3}.json` and `size-flac-scratch.json`. Baseline and
candidate build.json files include exact source and Wasm hashes. This is a
small fixture comparison, not a varied programme corpus or 4K60 result.

### Named Wasm audio CPU samples — 2026-10-14

Two fresh Node v26.11.1 processes each sample four committed audio inputs,
with separate three-second decode and FLAC runs after twenty warmup calls.
The requested sampling interval is 1,000 microseconds. The sixteen stage runs
record 45,298 samples. The balanced release recipe matches the shipped
package settings; the development build retains function names with wasm-opt
-Oz -g. Removing all custom sections leaves identical executable/data sections
to the matching stripped build (SHA-256
`a1ef36da7df01167af987d1e339f486231d76bda8e4999e7388e486bd41fcd15`).
The named artifact is not shipped or used to claim a download saving.

| Decode input | Target | Self samples, run 1 | Run 2 |
| --- | --- | ---: | ---: |
| AC-3 stereo | FFT + IMDCT | 31.3% | 31.7% |
| AC-3 5.1 | FFT + IMDCT | 37.9% | 37.5% |
| E-AC-3 5.1 | FFT + IMDCT | 29.0% | 30.7% |
| E-AC-3 5.1 | Float-to-PCM packing | 18.4% | 18.2% |
| MP2 stereo | Synthesis filter | 78.2% | 77.6% |

These are self samples, including work the compiler inlined into each function.
The FFT + IMDCT rows sum their distinct self samples; they do not add inclusive
and self counts. Decode includes TS demux, packet copy, a fresh decoder and PCM
conversion. In the separate FLAC stage, 99.5–99.7% of samples land in the encoder;
inlining hides the split between predictor search, bit writing and checksums.
It would be wrong to claim that these samples isolate FLAC's internal kernels.
The profile times include sampling overhead and must not become speed claims.

Decision for #45: prototype a small, optional SIMD FFT path with scalar fallback,
and investigate MP2 synthesis independently. Preserve operation order and gate
retention on exact output, repeated unprofiled CPU measurements, feature budgets
and target support. E-AC-3 packing is a secondary target. Do not enable SIMD across
the whole module without measuring its size, compatibility and throughput.
No SIMD optimization is shipped by this report.

For realistic expectations, if FFT/IMDCT is 38% of a decode path, making only
that work twice as fast gives a theoretical total speedup of
`1 / (0.62 + 0.38 / 2) = 1.23`. This is an estimate from the sampled split, not
a measured result. Complete decoding and packaging would have a different split.
Small allocation changes alone cannot close the recorded fourfold long-Dolby
conversion gap with FFmpeg.

Reproduce with:

```sh
python3 tools/profile_harness.py --profile balanced --retain-names --output tools/results/audio-kernels
node tools/node/audio-cpu.cjs tools/results/audio-kernels/balanced/rstreamkit_harness.js tools/results/audio-cpu > tools/results/audio-cpu.json
```

`RSTREAMKIT_PROFILE_SECONDS` accepts 1–30 seconds per stage, default three.
The JSON report records input/PCM/module/Wasm/script hashes and sample shares;
the output directory keeps raw Chrome-compatible `.cpuprofile` files. Inspect
those in a CPU-profile viewer to see stacks instead of adding overlapping costs.
Local reports are `audio-cpu-first.json`, `audio-cpu-repeat.json`, their eight
raw profiles each, and `audio-kernel-equivalence.json`. PCM hashes and FLAC lengths
match across both campaigns. The named Wasm hash is
`a7abcdda8e5bb13b9f4eddf515d4509022a91cda90b72bb3c5220e212e3b24a1`.

Limits: Ryzen 7 5700/Linux, warm Node Wasm, small generated fixtures. This is not
a native CPU profile, a varied programme corpus, browser frame-time measurement
or 4K60 result. #45 remains open for native/corpus profiling and a measured
optimization decision meeting its CPU gate. All default shipping build recipes
and runtime source remain unchanged by the profiling tools.

### Opt-in SIMD FFT after measured no-go experiments — 2026-10-14

The Wasm SIMD build now processes two independent complex butterflies together.
It preserves the scalar multiply/subtract/add order; separate subtract/add
vectors also preserve signed zero. A private repr(C) pair gives a checked
8-byte layout. Two pairs form each 16-byte vector load/store; split_at_mut
keeps the mutable halves disjoint. No audio buffers, runtime crates or public
engine signatures are added. FFT complexity stays O(N log N), with the same
bounded scratch storage. The gain is in work per instruction.

The kernel is selected only when compiling Wasm with simd128. Native and default
Wasm use the existing scalar algorithm. A SIMD module requires SIMD support at
validation time; it cannot load on an unsupported engine and fall back internally.
Keep a separate default artifact set and let the host choose a compatible build.
No default browser requirement is changed by this PR.

The implementation uses [Rust's Wasm SIMD feature](https://doc.rust-lang.org/stable/rustc/platform-support/wasm32-unknown-unknown.html)
and bounded [unaligned loads](https://doc.rust-lang.org/core/arch/wasm32/fn.v128_load.html)
and [stores](https://doc.rust-lang.org/core/arch/wasm32/fn.v128_store.html).
The size assertion guards the pointer-length invariant. There is no relaxed SIMD
or changed floating-point association in the kernel.

Before retaining it, reject three cheaper attempts: iterator PCM packing and
two-byte chunk packing preserve output but improve whole E-AC-3 processing only
about 3%; native packing changes by about 0–2%. The chunk writer adds 110 full
compressed bytes. Both are discarded. Compiler-only SIMD also fails to deliver
a repeatable CPU gain. A first explicit kernel that assembled vector lanes from
scalar fields is rejected too. The retained kernel uses contiguous vector loads.
Ignored PCM patches and timing reports preserve those no-go experiments.

CPU: three fresh Node 26.11.1 processes on Ryzen 7 5700/Linux. Scalar and SIMD
use the same final source and balanced release recipe, with only the SIMD target
flag different. Each transform batch performs 10,000 calls inside Wasm after
warmup; black_box prevents loop elimination. Nine alternating pairs avoid the
repeated JS return-copy overhead that obscured the first microbenchmark.
The table gives medians of three process medians. No builds or other browser
tests ran during these final timing campaigns.

| Targeted transform | Default scalar | Opt-in SIMD | Change |
| --- | ---: | ---: | ---: |
| Long transform | 1.105 µs | 0.787 µs | 28.8% less |
| Short transform | 1.077 µs | 0.808 µs | 25.0% less |

Each output gate compares all bytes of 2,564 finite-coefficient transforms:
zeros/signed zeros, all 256 basis impulses and 1,024 seeded bit-pattern inputs,
for both long and short transforms. The complete output SHA-256 is
`bf324192da3b5334fa8f1ba826e535e9df44023c1c9d81b09cf731ff1a76ced8`.
The original scalar layout and final scalar layout match this fingerprint too.
NaN payloads are outside the exact-output gate. Public APIs still accept their
existing inputs; no new mathematical approximation is introduced.

Complete TS processing includes copying, a fresh decoder, PCM conversion,
FLAC and packaging. Each process uses five warmups and nine alternating pairs
of five-call batches. PCM hashes and output lengths match for mono/stereo/5.1
AC-3, 5.1 E-AC-3 and MP2. AAC is the no-sound-conversion control.

| Complete input processing | Default scalar | Opt-in SIMD |
| --- | ---: | ---: |
| bbb_480p.ts | 0.139 ms | 0.153 ms |
| h264_ac3.ts | 12.382 ms | 12.184 ms |
| h264_ac3_st.ts | 7.190 ms | 6.932 ms |
| h264_ac3_51.ts | 10.630 ms | 9.858 ms |
| h264_eac3_51.ts | 12.559 ms | 11.725 ms |
| h264_mp2.ts | 13.709 ms | 13.634 ms |

The 5.1 AC-3 row improves about 7%; E-AC-3 about 6.6%. Mono and stereo gains are
smaller. MP2 is near unchanged. AAC is slower in these tiny calls (about 0.01 ms
in the median-of-medians table), so do not claim a universal speedup. The targeted
FFT improvement clears #45's 10% gate; complete processing does not improve by
25–29%. Retain this as an optional measured path, while keeping #45 open for
native/corpus profiling and remaining audio hotspots.

| Engine Brotli download | Previous main | New default | Opt-in SIMD |
| --- | ---: | ---: | ---: |
| live | 108,638 B | 108,638 B | 108,254 B |
| live-sound | 212,104 B | 212,098 B | 211,947 B |
| vod | 130,422 B | 130,422 B | 129,936 B |
| full | 234,117 B | 234,048 B | 233,411 B |

All five engine/shell budgets pass in both modes. Full default saves 69 bytes;
SIMD saves 706 bytes versus previous main. The shared HTML remains 2,750 Brotli
bytes, giving complete full startup of 236,798 B and 236,161 B respectively.
Code-generation and compression layout affect these small differences; use the
exact hashes/tool versions in the reports, not a general claim that SIMD shrinks
programs. The SIMD size tool requires a separate artifact directory and refuses
to overwrite the default player.

Five fresh Chrome 155 processes per flavor and build record startup and initial
committed memory. Loopback HTTP is uncompressed and the OS cache is warm. The
campaigns are sequential, not randomized paired cold-cache trials. These timing
ranges do not establish a startup win. Memory values are identical across all
five trials in both modes; they exclude JS, browser decoder, GPU and process RSS.

| Build | Default ready median (range) | SIMD ready median (range) | Initial committed Wasm, both |
| --- | ---: | ---: | ---: |
| live | 39.7 ms (27.2–41.8) | 42.0 ms (29.0–44.0) | 1,179,648 B |
| live-sound | 43.2 ms (28.4–46.8) | 41.4 ms (28.6–45.0) | 1,245,184 B |
| vod | 38.9 ms (27.7–40.4) | 40.1 ms (27.4–40.9) | 1,179,648 B |
| full | 41.5 ms (29.1–43.9) | 41.6 ms (27.9–42.4) | 1,245,184 B |

Validation includes native/Wasm feature checks, explicit SIMD clippy, the decoder
suite, independent media/FLAC checks, Node synchronous/yielded output equality,
all ten Chrome SIMD playback regressions and all startup inventories. CI now
checks the SIMD branch with Rust 1.88, exact transform and audio gates, and all
SIMD size budgets. Default Chrome regression checks remain in CI too. No native
speedup, Edge/Safari result, long-run 4K60 acceptance or FFmpeg superiority is
claimed by these short audio tests.

Reproduce the matched builds and output gates:

```sh
python3 tools/profile_harness.py --profile balanced --output tools/results/scalar
python3 tools/profile_harness.py --profile balanced --simd --output tools/results/simd
node tools/node/fft-compare.cjs tools/results/scalar/balanced/rstreamkit_harness.js tools/results/simd/balanced/rstreamkit_harness.js
node tools/node/opt-compare.cjs tools/results/scalar/balanced/rstreamkit_harness.js tools/results/simd/balanced/rstreamkit_harness.js
python3 tools/size.py --check-budgets
python3 tools/size.py --simd --artifacts-dir tools/results/simd-player --check-budgets --output tools/results/size-simd.json
```

Final ignored reports: `fft-final-direct-{1,2,3}.json`,
`fft-final-audio-{1,2,3}.json`, `size-fft-default.json`, `size-fft-vector.json`,
`fft-startup-{default,vector}.json`, and `fft-simd-browser-final.log`.
The benchmark drivers record their hashes, input/PCM hashes and Wasm hashes;
matching build.json files record effective compiler flags and exact source hashes.
Audio and transform outputs match across the repeated campaigns. Keep the
full report limitations with any extracted numbers.

## Dolby parsing: reused bit allocation and a tighter mantissa loop (2026-10-29)

Two exact changes to the AC-3 decoder's per-block parsing. Bit allocation now remembers the
inputs of its last run per channel (exponents, bit-allocation codes, SNR offsets, delta
segments) and skips the work when a block repeats them, which encoders do whenever they reuse
exponents. The mantissa loop borrows the channel's arrays once instead of indexing the whole
decoder state for every coefficient. Arithmetic and its order are unchanged.

Inputs were generated with FFmpeg 9.0.2: 60 seconds of six distinct channels (tones and
noise) as 448 kb/s AC-3 5.1, its 192 kb/s stereo downmix and 384 kb/s E-AC-3 5.1. The
end-to-end movie is the same AC-3 looped to 10 minutes with 320×180 H.264 in Matroska.
Ryzen 5 7500X3D, Linux, Rust 1.99.1, release build without LTO.

| Measure (best of 15, in-process) | Before | After |
| --- | ---: | ---: |
| 60 s AC-3 5.1 to stereo | 169.8 ms | 131.4 ms (23% less) |
| 60 s AC-3 5.1, six channels out | 175.8 ms | 146.0 ms (17% less) |
| 60 s AC-3 stereo | 68.3 ms | 63.7 ms (7% less) |
| 60 s E-AC-3 5.1 to stereo | 175.1 ms | 156.9 ms (10% less) |
| `rstreamkit movie.mkv out.mp4`, 10 min 5.1, median of 7 | 2.428 s | 2.058 s (15% less) |

Every decoded PCM byte hashes the same before and after (FNV-1a over all four outputs) and
the command's MP4 output is byte-identical. The decoder's 559 tests and the workspace's 153
pass. Not measured: Wasm timing and download size (wasm-bindgen was not installed), real
broadcast audio, and FFmpeg on this machine. Fat LTO on the native build gave a further
6–9% in a trial; it is not part of this change.

Repeat (inputs from the FFmpeg commands in the commit, any AC-3 or E-AC-3 elementary
stream works):

```sh
cd ac3
cargo run --release --example decode_bench -- ac3_51.ac3 2 15
cargo run --release --example decode_bench -- eac3_51.eac3 2 15
```

Follow-up the same day: forcing `fetch_mantissa` inline into its callers. Interleaved A/B of the
two builds (eight rounds of best-of-3, because the machine was loaded), identical PCM hashes:

| 60 s input, best | Before | After |
| --- | ---: | ---: |
| AC-3 5.1 to stereo | 155.7 ms | 142.3 ms (8.6% less) |
| AC-3 stereo | 76.5 ms | 69.4 ms (9.3% less) |
| E-AC-3 5.1 to stereo | 186.9 ms | 171.1 ms (8.4% less) |
| Full player Wasm, Brotli, before wasm-opt | 250,151 B | 250,750 B (+599) |

Native times only; the player builds the decoder at opt-level `s`, where the gain was not measured.

The same decoder changes in Wasm, which the browser player uses. `tools/node/opt-compare.cjs` with
the Node harness built at 973a5c2 (before) and 1dc392f (after), opt-level `s`, Node 24.22.1, a
loaded machine. PCM checksums match on every fixture.

| Fixture, median | Before | After |
| --- | ---: | ---: |
| h264_ac3_51.ts, transmux with sound | 11.57 ms | 10.80 ms (6.6% less) |
| h264_eac3_51.ts, transmux with sound | 13.38 ms | 12.54 ms (6.3% less) |
| h264_ac3.ts (mono), transmux with sound | 14.23 ms | 13.44 ms (5.6% less) |
| h264_ac3_st.ts, transmux with sound | 7.71 ms | 7.60 ms (1.5% less) |
| FLAC encoding and MP2 rows | | within ±5%, unchanged code |

So in the browser the gain is about 6%, far below the native 30%: at opt-level `s` the decoder
was already less dominated by these loops. Harness Wasm grew 790,908 → 798,828 bytes before
compression, which also includes the Matroska lacing and header-stripping support.

## Matroska starts one round trip sooner (2026-10-29)

Matroska pictures go out a whole group at a time (their decode times come from the group), so
nothing could play until the next keyframe had been read. The first range was a fixed 768 KB;
a 2 s group at 6 Mb/s is about 1.5 MB, so playback waited for a second range. Ranges now reach
the next cue's cluster plus 32 KB (capped at 16 MB), and the header of the next keyframe block
completes the group without waiting for its body.

`tools/node/movie-latency-smoke.cjs`: 60 s 1280×720 6 Mb/s H.264 + AC-3 5.1, every range answered
after a fixed delay, Chrome 155, unoptimized full player, two runs each (ranges shown).

| Measure | Before | After |
| --- | ---: | ---: |
| MKV, 800 ms per range: time to Playing | 3.35 s | 2.50-2.51 s |
| MKV, 300 ms per range: time to Playing | 1.34-1.35 s | 0.99-1.00 s |
| MKV, 300 ms: time to 20 s buffered ahead | 2.88 s | 2.54-2.60 s |
| MP4 (unchanged code), 300 ms: time to Playing | 0.96 s | 0.97 s |

A local route with injected latency, not a real provider; real transfer time for the larger
first range is not modelled. Repeat: serve `tools/web`, then
`RSTREAMKIT_LATENCY_MS=300 NODE_PATH=tools/browser/node_modules node tools/node/movie-latency-smoke.cjs`.

## Dolby transforms: exact speedups measured and dropped (2026-10-29)

How much of native AC-3 decoding the IMDCT is, and whether it can be made faster while every
PCM byte stays identical. Same 60 s FFmpeg 5.1 input as above, `decode_bench`, release build.

- Skipping the full-bandwidth IMDCT, window and overlap-add (wrong output, timing only): best
  117.9 → 86.7 ms, so that stage is about 26%. A standalone loop of 62,028 long IMDCTs (the
  same count) takes 28.9 ms. The 5.1 to stereo downmix is a few milliseconds.
- SSE2 butterflies (a port of the Wasm SIMD ones, same operations and order) plus writing the
  pre-twiddle straight into bit-reversed order: the standalone IMDCT drops 28.9 → 26.9 ms, but
  six interleaved best-of-3 rounds give 114.2 → 113.4 ms (AC-3 5.1) and 129.5 → 126.8 ms
  (E-AC-3 5.1), near the noise of a loaded machine. The bit-reversal change alone: 116.4 →
  115.7 and 128.7 → 129.0 ms. PCM hashes matched in every run.

Not kept: the compiler already vectorizes the scalar butterflies, and the gain is too small to
justify new `unsafe` code. A real gain would need a different factorization (split-radix or
radix-4), which changes rounding, so it would need a tolerance gate instead of a hash.

## Live HLS startup is one whole segment download (2026-10-30)

Where a live channel's first picture waits, before changing anything. 36 s of 1920×1080/25 H.264
at 6 Mb/s + AAC as 6 s TS segments (about 4.3 MB each) behind a live playlist without ENDLIST,
served by `tools/server.py`, Chrome 155 with CDP network emulation at 40 Mb/s and 100 ms latency,
full player build, three runs:

| Step | Time from start() |
| --- | ---: |
| Media playlist answered | 107-109 ms |
| First segment (third from the live edge) fully downloaded | 1,156-1,172 ms |
| `Playing` reported | 1,159-1,173 ms |
| First decoded frame | 1,213-1,222 ms |

Everything after the playlist is the first segment's download: transmuxing and appending take
about 5 ms, and the next segment is only requested after that. The first 2 s keyframe group is
about a third of the segment, so appending while the segment downloads could bring the first
frame to roughly 0.5 s here. It needs the transmuxer to emit fragments before a segment ends
(today `Transmuxer::finish` turns a whole segment into one fragment), and a retry or a skipped
corrupt segment after part of it was appended must not duplicate or tear media. Done below.

## Dolby decoder at opt-level 3 in Wasm: not worth its size (2026-10-30)

Whether the Wasm player's `s` setting for `rstreamkit-ac3` is why native decoder speedups showed
only about 6% in Wasm. Node harness (`tools/harness`, its own `s` profile) built twice with only
`profile.release.package.rstreamkit-ac3.opt-level` changed, wasm-bindgen `--target nodejs`, no
wasm-opt (not installed here). `tools/node/opt-compare.cjs` run three times in each order;
medians of the per-run medians. PCM checksums and FLAC byte counts matched in every run.

| Fixture, transmux with sound | `s` | `3` |
| --- | ---: | ---: |
| h264_ac3.ts (mono) | 11.65 ms | 11.21 ms (3.8% less) |
| h264_ac3_st.ts | 6.58 ms | 6.36 ms (3.4% less) |
| h264_ac3_51.ts | 9.53 ms | 9.14 ms (4.0% less) |
| h264_eac3_51.ts | 10.81 ms | 10.36 ms (4.1% less) |
| Harness Wasm before wasm-opt | 812,965 B | 914,287 B (+101,322) |

FLAC-only and MP2 rows moved under 1%. Kept `s`: 4% of a few milliseconds per segment does not
buy about 100 KB before compression in a 245,000-byte Brotli budget. The Wasm/native gap is not
the opt-level; Brotli sizes after wasm-opt were not measured.

## Live HLS plays its first keyframe group while the first segment downloads (2026-10-30)

The first segment of a live TS playlist (no separate audio rendition, key, map or byte range) is
fed to the transmuxer as it arrives; once the keyframe after the first group is in,
`Transmuxer::take` turns that group into an init and fragment, which are appended and played
while the rest downloads. The rest comes out of `finish` as before, and every later segment takes
the unchanged path, so its output is identical. A download that fails before anything was
appended is fetched again with the usual retries; one that fails after is not retried (that would
append the group twice) but skipped like an unavailable segment, the next glued on where the group
ended.

Same setup as above (1920×1080/25 H.264 at 6 Mb/s + AAC, 6 s TS segments of about 4.4 MB with 2 s
keyframe groups, live playlist, CDP 40 Mb/s and 100 ms latency, cache off, a fresh context per
run), Chrome 156, full player builds of 19dbdba (before) and this change (after), eight
interleaved pairs:

| Build | First decoded frame from start() | `Playing` |
| --- | ---: | ---: |
| Before | 1,194-1,229 ms, median 1,199 | 1,175-1,190 ms |
| After | 574-591 ms, median 581 | 557-562 ms |

About 0.62 s sooner: the first frame now waits for the playlist and a third of the segment. No run
waited for data in the 3 s after the first frame. The unoptimized full Wasm grows 4,154 bytes
(1,197 Brotli bytes before wasm-opt, which is not installed here).

## Live HLS shows its first picture as soon as it arrives (2026-10-30)

The first segment's first keyframe group is now appended picture by picture as it downloads
(`Transmuxer::take`), each part holding every picture that is in but the latest, whose duration
the next one gives. Every part decodes on its own, since a picture only refers to earlier ones in
decode order. The first part waits until the sound that plays with it is in, so the init has the
sound track and the timeline starts where it would for the whole segment. The first picture is
shown paused; playback starts once 1 s is buffered (`START_AHEAD`), so a link that only just
keeps up does not stall straight away. Cuts made mid-stream (these and `Continuous` keyframe
groups) now give the last picture its real duration from the next picture's decode time instead
of repeating the previous gap, which re-based the clock on streams with uneven frame spacing.

Same setup as the two sections above, full builds of 3048b0a (before) and this change (after),
interleaved:

| Link | Build | First picture | Motion starts (`Playing`) | Waits in the first 10 s |
| --- | --- | ---: | ---: | --- |
| 40 Mb/s, 4 pairs | Before | 569-593 ms, median 585 | 556-561 ms | none (3 s watched) |
| 40 Mb/s, 4 pairs | After | 293-295 ms, median 293 | 419-422 ms | none (3 s watched) |
| 8 Mb/s, 3 pairs | Before | 1,822-1,845 ms | 1,804-1,807 ms | 2, 1.51-1.52 s in all |
| 8 Mb/s, 3 pairs | After | 456-465 ms | 1,171-1,177 ms | 2, 2.21-2.22 s in all |

At 8 Mb/s the 6 Mb/s stream barely keeps up either way: both builds have played the same media
10 s in; the new one shows a picture 1.4 s sooner and moves 0.6 s sooner. Playing from the first
picture instead (tried first) brought motion to 266 ms at 40 Mb/s but made the 8 Mb/s waits
2.8 s, hence the 1 s threshold.

## A raw TS channel shows its first picture without waiting for the next keyframe (2026-10-30)

A raw MPEG-TS stream (no playlist) arrives in real time, and `Continuous` cut it at keyframes, so
the first picture waited for the second keyframe: a whole keyframe interval of wall-clock time,
1-5 s on typical IPTV. Its first keyframe group now comes out picture by picture, as the first
live HLS segment's does. Motion still starts when that group is out whole
(`Continuous::first_group_out`): later groups each come out when the next keyframe arrives, so
starting earlier would only run dry, as a trial that played from the first picture did (one 2 s
stall in every run).

`tools/server.py`'s `/raw.ts` (320×180, one keyframe per 2 s, a 2 s piece every 2 s), Chrome 156
unthrottled, builds of a531a05 (before) and this change (after), four interleaved pairs:

| Build | First picture | Motion starts (`Playing`) | Waits in 6 s |
| --- | ---: | ---: | --- |
| Before | 2,013-2,041 ms, median 2,026 | 2,009-2,013 ms | none |
| After | 29-34 ms, median 31 | 2,007-2,008 ms | none |

Tried and dropped (2026-10-30): appending the rest of the first live segment a keyframe group at a
time as it downloads. At 8 Mb/s for the 6 Mb/s stream it moved the stall rather than shortening
it: waits of 0.28 s at 3.0 s and 1.9 s at 7.4 s against 1.6 s at 3.1 s and 0.6 s at 8.7 s, 2.2 s in
all either way. On a link that barely keeps up, total waiting is set by bandwidth.

