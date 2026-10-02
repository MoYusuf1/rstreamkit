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

Reproduce after media generation:

```sh
cargo build --release --example transmux
python3 tools/compare.py
```

The script records timing, checks both outputs decode without errors and compares
video frame hashes and decoded PCM bytes. Its FFmpeg command uses stream copying,
AAC header conversion, and `frag_keyframe+empty_moov`; no video/audio re-encoding.

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
