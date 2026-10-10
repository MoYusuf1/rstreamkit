# Performance direction

The target is efficient streaming in native apps and browsers: startup latency, channel
switching, CPU time, main-thread stalls, committed wasm memory, browser buffer memory
and download size. Rust alone does not
establish superiority over FFmpeg. Compare equivalent operations and output quality;
remuxing H.264 is not a fair comparison with decoding or encoding it.

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
