# Roadmap: a leading Rust streaming toolkit

Updated 2026-10-01. [GitHub tracker: #26](https://github.com/MoYusuf1/rstreamkit/issues/26).

## Vision

Make rstreamkit a first choice for developers building live TV, video players,
and stream-processing services in Rust. One media core should work in desktop
apps, servers, and browsers. It should start quickly, recover from bad streams,
use predictable memory, and explain failures clearly.

All project media code stays Rust. Playback must not launch or link FFmpeg.
Browsers still need a small JavaScript bridge, and platform playback uses the
browser or operating system's codecs. A native host still supplies networking
and rendering. Those boundaries are part of the design.

Our opportunity is focused streaming and playback. FFmpeg also provides a huge
range of encoders, filters, devices, and formats. Matching all of that would be
a much larger project. Its [official overview](https://ffmpeg.org/about.html)
describes that scope; its [libraries](https://ffmpeg.org/about.html)
can run inside apps, so a command-line startup win is not an in-process win.

## What we can claim today

| Evidence | Result | Limit |
| --- | --- | --- |
| Copy a six-second H.264/AAC TS clip into MP4 | 14.5 ms versus FFmpeg's 60.5 ms; 4.2× faster | Native command-line test, including startup and file access; not encoder or library speed |
| Programme FLAC encoding, previous versus new Rust code | 12.583 → 8.899 ms; 29.3% less CPU | One 4.16-second soundtrack; compressed bytes unchanged |
| Full six-second AC-3 processing, previous versus new Rust code | 76.921 → 69.813 ms; 9.2% less CPU | Wasm processing benchmark, not measured playback improvement |
| Accelerated pipeline soak | 10,000 segments; stable committed wasm memory | Does not test four hours of real-time network playback |
| Fault recovery | Generated outages, bad segments and frozen playlists recover | Real providers and some combined failures remain untested |

Methods and output checks are in [performance.md](performance.md). Detailed
issue acceptance and the desktop browser matrix are in [issues.md](issues.md).
The work from PR #41 landed on main on 2026-10-09.

## Weak layers and tradeoffs

This is an audit of known gaps, not a claim that we have found every possible bug.

| Layer | Weakness today | Work to do | Tradeoff |
| --- | --- | --- | --- |
| Shared playback engine | Much of the scheduling loop still lives in browser code | Move retries, timing, trimming and backpressure into a testable Rust session (#23) | More interfaces now; less duplicated behavior across platforms later |
| Recovery and cancellation | Individual fault checks pass; combined faults, expired server resources and movie switching need more coverage | Script failures with a fake clock and provider; test every output side effect after cancellation (#2, #3, #7, #37) | Bounded retries eventually give up; we must report that limit |
| Audio CPU and page responsiveness | Audio decoding dominates; some movie, encrypted and raw-stream paths do synchronous work | Profile decoding; compare yielding and worker execution across all paths (#34) | Workers add startup, transfer and queue costs; yielding adds elapsed time |
| Video playback | The high-bitrate test reported many dropped frames | Repeat visibly, record hardware and compare the same stream through platform playback (#24, #36) | A hidden test window or weak device can distort results; buffer limits alone do not prove smooth playback |
| Network adaptation | Variant switching, prefetch and live catch-up exist but lack controlled comparisons | Test low/high bandwidth, jitter and recovery; measure stalls and wasted downloads (#9, #13, #14) | More buffering helps stalls but adds latency and memory; prefetch can waste data |
| Sound quality and track choice | Rust-decoded output is 16-bit stereo; passthrough and language switching need target-device checks | Preserve original sound where supported; document downmixing; test actual playback (#8, #32) | Passthrough depends on platform support; multichannel output adds codec and device complexity |
| Picture/sound timing | Generated splice correction improves skew; real broadcast timing is not measured | Expand the corpus and quantify drift across ordinary boundaries and splices (#16, #40) | Aggressive correction can create gaps or visible timing changes |
| Format coverage | Focused HLS/TS/MP4/Matroska support; HEVC, interlacing and DRM are incomplete or unsupported | Establish the desktop capability matrix and choose formats from demand (#11, #12, #24) | Each new format adds parser, timing, fixtures and maintenance work |
| Native integration | The processing core runs natively, but there is no complete native player or general converter | Supply a small Rust CLI and a host adapter example after the shared session | Rendering and hardware support require platform adapters; Rust alone does not provide them |
| Memory and copies | Some copies are required; Matroska retains frames; the live budget estimates compressed bytes | Measure ownership, queues, decoder output and platform buffers separately (#35, #36) | A small queue saves memory but may stall; compressed bytes exclude decoder surfaces |
| Parser robustness | Caps and regression tests exist; systematic fuzzing is missing | Fuzz malformed TS, HLS, MP4, Matroska and CMAF with resource limits | Rust reduces some memory hazards, but does not prevent panics, hangs or oversized allocations |
| Evidence and delivery | A single FFmpeg CLI benchmark and a Linux browser cannot establish broad superiority | Add equivalent library/CLI baselines, target-machine runs, release review and reproducible reports (#24, #29, #30, #38) | More test hardware and careful reports cost time; narrow claims remain trustworthy |

## Order of work

These are proposed milestones, not release dates. Finish the acceptance checks
for each milestone before expanding the next. Effort is relative: small means a
focused patch; medium means several changes; large means substantial design and
integration work.

| Order | Milestone | Exit check | Effort |
| --- | --- | --- | --- |
| 1 | Finish the shared Rust session (#23), with reproducible fault tests | Native tests drive the actual retry/throttle/trim loop using a fake clock; browser adapter uses that same policy | Large |
| 2 | Establish a trustworthy performance and playback baseline (#42) | Same inputs, output quality, hardware and build flags; separate CLI, in-process and browser results; investigate visible 1080p drops | Medium |
| 3 | Finish playback acceptance (#2, #3, #7, #9, #13, #14, #25, #34, #37) | Expired resources, bandwidth changes, stop/seek and combined faults recover as designed; four-hour real-time soak shows no growing resource trend | Medium–large |
| 4 | Verify desktop Chrome, Edge and Safari (#24, #32) | Each supported codec has real playback, OS/hardware details and a clear fallback; unsupported combinations remain marked | Medium; needs target machines |
| 5 | Add native adoption (#44) and harden parsers (#43) | Rust CLI/adapter demonstrates the shared session; seeded fuzz smoke tests run in CI and longer fuzz campaigns have documented limits | Medium–large |
| 6 | Optimize the measured decoder hotspots (#45) | Scalar/SIMD comparison includes CPU, binary size and exact output checks; keep only a demonstrated improvement | Medium–large; benefit uncertain |
| 7 | Release a reviewed API (#29, #30) | Migration notes, reproducible demo, green CI, reviewed tag and package checks | Small–medium after earlier work |
| 8 | Expand formats from a real use case (#12 and future issues) | An implementation plan names the target platform, sample files, fallback, maintenance cost and acceptance tests | Large; one capability at a time |

The first FLAC CPU patch is already measured. The next implementation priority
is #23: it is the biggest structural weakness and unlocks reliable testing and
native use. Benchmark work can proceed alongside that extraction.

## How we decide whether an optimization wins

Proposed gates below are goals, not current results or universal promises.

| Measure | Proposed gate |
| --- | --- |
| Audio work on the UI thread | Keep processing slices under 8 ms at the 95th percentile on named reference hardware; compare end-to-end startup too |
| Visible video playback | Under 1% dropped frames during a ten-minute 1080p/8 Mbps reference run; compare against platform playback before assigning blame |
| Sustained operation | Four hours with stable post-warmup memory and no accumulating listeners, readers or queued fragments |
| Fault recovery | Recover automatically in scripted outage, expired-window and corrupt-segment cases; record time to playing and time to target latency separately |
| Adaptation and prefetch | Fewer stalls or faster startup on the same bandwidth trace, with downloaded/wasted bytes and latency reported |
| CPU optimization | At least 10% improvement in the targeted workload across repeated runs, no output regression; report binary-size and memory changes |
| Rust portability | Native and wasm builds on the stated MSRV; shared scheduling behavior tested without a browser |
| Comparison with FFmpeg | Equivalent operation and decoded output; disclose probing, startup, copying, thread count, codec and hardware differences |

Run more than one soundtrack and bitrate. Report median and tail latency;
retain regressions as fixtures. A win in FLAC encoding must not become a claim
about video encoding. FFmpeg's [stream-copy and transcoding documentation](https://ffmpeg.org/ffmpeg.html#Streamcopy)
explains why those jobs must be compared separately.

## What is realistic

| Ambition | Assessment |
| --- | --- |
| An efficient Rust library for our supported streams and files | Realistic; much of the media core exists, with acceptance work still needed |
| Easier Rust integration, clear errors and predictable recovery | Strong opportunity; shared policy and good host interfaces are within our control |
| Faster packaging and lower memory for selected workloads | Realistic to pursue and measure; the current narrow packaging win is already evidence |
| Smooth desktop browser playback and useful native adapters | Realistic with target machines, the shared session and enough integration testing |
| Faster AC-3 decoding than mature alternatives | Unproven; profile dependency kernels before committing to a decoder rewrite |
| HEVC or multichannel playback on supported devices | Plausible through platform support and targeted integration; not universal |
| Low-latency HLS, DASH, WebM/AV1, subtitles or recording | Possible future projects; each needs an explicit use case and separate scope |
| Full FFmpeg replacement, every codec/filter/device, top-tier video encoders | Not a realistic near-term roadmap; it requires far broader specialist engineering and testing |

My judgment is that our strongest route is to lead in focused Rust streaming:
good recovery, smooth playback, small deployment size and honest measurements.
We should earn each performance claim rather than promise an overall winner.

## Research rules

Rust [wasm SIMD instructions](https://doc.rust-lang.org/core/arch/wasm32/index.html)
are an option for measured DSP hotspots, not an automatic speedup. Keep the
scalar path until deployment and output checks pass. Platform codecs through
[WebCodecs](https://www.w3.org/TR/webcodecs/) need capability checks and actual
playback validation; hardware preference is not a guarantee. Start with an
experiment and a stop/go decision before adding either to the main pipeline.

DRM, broad software HEVC support, deinterlacing, GPU filters and a video encoder
remain research or later projects. No fallback should quietly invoke FFmpeg.

## GitHub work queue

Use [#26](https://github.com/MoYusuf1/rstreamkit/issues/26) as the index.
The implementation landed through [PR #41](https://github.com/MoYusuf1/rstreamkit/pull/41).
New work has separate issues with acceptance checks:

- [#42: fair FFmpeg baselines and visible playback checks](https://github.com/MoYusuf1/rstreamkit/issues/42).
- [#43: parser fuzzing and resource limits](https://github.com/MoYusuf1/rstreamkit/issues/43).
- [#44: a native Rust CLI and host adapter](https://github.com/MoYusuf1/rstreamkit/issues/44).
- [#45: audio decoder profiling and a SIMD go/no-go decision](https://github.com/MoYusuf1/rstreamkit/issues/45).

The next code task is [#23](https://github.com/MoYusuf1/rstreamkit/issues/23),
followed by the baseline and acceptance work above. This queue does not commit
us to shipping speculative codecs or a decoder rewrite.
