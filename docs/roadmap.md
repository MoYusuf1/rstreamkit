# Roadmap: reliable 4K streaming with a small Rust engine

Updated 2026-10-10. [GitHub tracker #26](https://github.com/MoYusuf1/rstreamkit/issues/26).
Initial review: `fdcf179`. The targets below remain goals; completed baseline and
buffer work is recorded in [performance.md](performance.md). All 24 issues open at review are mapped below, plus new
[#47](https://github.com/MoYusuf1/rstreamkit/issues/47) (4K60) and
[#48](https://github.com/MoYusuf1/rstreamkit/issues/48) (size).

## What we are building

A Rust media toolkit that prepares live streams and files for playback in apps,
servers and browsers. Keep the engine independent of any one app. Do not launch
or link FFmpeg at runtime. Browsers still need generated JavaScript glue and
use their own media codecs. Native hosts supply networking and rendering.

The agreed web target is **3840×2160 at 60 fps, SDR**, on desktop Chrome, Edge
and Safari, for supported stream/device combinations. HDR and mobile are later
work. Unsupported devices should get a suitable lower variant or a clear error.

Cloud gaming is useful evidence that demanding browser video is possible. In
cloud streaming, the game runs remotely and the client receives video; the
browser is not doing all the game simulation. See [NVIDIA's cloud-gaming
explanation](https://www.nvidia.com/en-us/geforce-now/faq/). We should use the same
principle: keep compressed video compressed and use the platform's decode path.
It does not prove that every codec, browser or machine can play 4K60.

## What changed since the old plan

| Area | Current evidence in the repo | What remains |
| --- | --- | --- |
| TS processing | Word-at-a-time start-code scanning and small stream lists; recorded 2–2.8× demux improvement | Browser/wasm and 4K traces need current measurements |
| Native CLI | Local TS/HLS, MP4/MKV and pipes; reused 1 MiB read buffer and two-piece thread queues | URL transport and the shared live-session policy (#44, #23) |
| Copying video | CLI workloads recorded at 7.94 ms for six seconds, 334 ms for ten minutes, 781 ms for a 30-minute MP4; 13–17 MB peak RSS | No in-process libavformat or 4K playback claim (#42) |
| Dolby audio | In-repo MIT decoder is about twice as fast on recorded inputs; 30-minute AC-3 job fell from 14.3 to 8.4 seconds | Still about four times FFmpeg's wall time on that long audio-conversion workload (#45) |
| Download size | Recorded full harness: 323,026 Brotli bytes after wasm-opt; decoder changes added about 6.6 KB | Measure a real minimal player and all startup assets (#48) |
| Playback recovery | Implemented controls, retry/skip, prefetch, ABR and timeline repair | Several acceptance checks remain; open titles describe older code |
| Web playback | Generated stream checks and a stable accelerated pipeline soak exist | Earlier high-bitrate run dropped many frames; no verified 4K60 matrix (#42, #47) |

These are recorded measurements, not reruns made during this planning update.
Read [performance.md](performance.md) for input hashes, methods and quality limits.
The old 4.2× six-second comparison is historical and has been superseded by the
multi-workload CLI measurements.

## Work order

Each row should produce a small, reviewable change with its own evidence. Evidence
collection starts first; it must not wait for the large session extraction.

| Order | Work | Issues | Exit check |
| --- | --- | --- | --- |
| 0 | Refresh browser/wasm/size baselines and classify open work as implementation, acceptance or external dependency | #24, #42, #47, #48 | Named artifacts, inputs and machines; visible 1080p control and 4K capability report; no stale performance claims |
| 1 | Ship less unnecessary code and avoid needless sound work | #48, #32, #34 | Per-flavor startup bytes; supported sound passthrough tested; responsive fallback; no output-quality regression |
| 2 | Extract the actual shared session and finish its fault tests | #23, #2, #3, #37, #43 | Fake clock/provider drive retries, throttle, trim, cancellation and backpressure; malformed input stays within stated limits |
| 3 | Finish adaptation, prefetch, live catch-up and stream correctness | #8, #9, #11, #13, #14, #16 | Same bandwidth traces and real stream fixtures; measured startup/stalls/latency; audible track changes and no boundary jitter |
| 4 | Deliver the supported 4K60 paths and run desktop acceptance | #47, #12, #24, #25, #32, #34 | 30-minute foreground 4K60 runs on named machines, controlled lower-variant fallback and separate four-hour soak |
| 5 | Optimize only the measured hot paths; complete native URL integration | #45, #44 | CPU/bytes/memory/quality table; actual shared session used by native adapter; decline experiments without a useful win |
| 6 | Finish host adoption and release readiness | #22, #29, #30 | Client controls use the API; package dependency order solved; reviewed API, docs, dry runs and approved release |
| Later | Offer compatible decoder changes upstream when the maintainer chooses | #46 | Keep provenance; contact the upstream project only when explicitly authorized |

Small early patches: make baseline/size reports reproducible, check exact playback
capabilities, fix acceptance tests, and feature-gate code proven to be retained.
Worker integration and the shared session are larger changes. HEVC carriage is
new format work, even when the browser already has a decoder.

## Every open issue, mapped to a finish line

Issue bodies contain historical reports. This table describes the remaining work
against the reviewed code; being implemented is not the same as passing acceptance.

| Issue | State now | Next acceptance or decision | Order |
| --- | --- | --- | --- |
| #2 | Recovery implemented; acceptance incomplete | Native scripted ten-second outage, missing and garbage segments recover within the documented retry budget | 2 |
| #3 | Resume refresh implemented | Pause five window lengths while the server really deletes old resources; resume without failure at an explained live position | 2 |
| #8 | Codec metadata and external audio implemented | Mixed H.264/HEVC master, audible external audio and language changes late in playback | 3 |
| #9 | ABR implemented | Bandwidth drops and recovers; sustainable variant selected without oscillation; add decode-limit input for 4K | 3 |
| #11 | Main format cases implemented | Complete clock/language/encryption fixtures and explicit malformed-input errors | 3 |
| #12 | Capability research, not HEVC playback support | Separate supported HEVC carriage from interlace research; measured per-device go/no-go; no silent FFmpeg fallback | 4 |
| #13 | One speculative download implemented | Matched traces prove first-frame/stall benefit; report wasted bytes and queue memory | 3 |
| #14 | Stats and catch-up implemented | After a ten-second stall, measure return to target latency; state when a seek is necessary | 3 |
| #16 | Boundary snapping implemented | Real multi-segment stream shows continuous buffering and no repeated boundary jitter | 3 |
| #22 | Library controls/stats implemented | Verify the consuming client's overlay and go-live adoption; external app work is not in this repo | 6 |
| #23 | Traits and policy helpers exist | Extract the full loop and test every listed provider scenario natively | 2 |
| #24 | CI exists; browser matrix incomplete | Current main checks plus actual Chrome/Edge/Safari playback on recorded hardware | 0, 4 |
| #25 | Accelerated soak exists | Mixed parameter/discontinuity fixtures and four hours of real-time playback with stable resources | 4 |
| #26 | Tracking issue | Keep this queue and acceptance ledger synchronized; do not close it after only one milestone | All |
| #29 | API work exists; release incomplete | Review the shared-session/feature contracts, MSRV and migration; validate the exact tagged commit | 6 |
| #30 | Demo/docs exist; publication incomplete | Fresh-clone flow, docs and dry run; solve the unpublished path decoder dependency before any release | 6 |
| #32 | Opt-in Dolby passthrough implemented | Audible AC-3/E-AC-3 playback on a supported target and tested decoder fallback | 1, 4 |
| #34 | Yielding exists on live path | Measure actual current decoder on live, movies, AES and raw TS; compare yielding/worker CPU, startup and frame drops | 1, 4 |
| #37 | Wait cancellation implemented | Closure during trim and removal of an unrelated buffer; no hang or false completion | 2 |
| #42 | Five CLI workloads recorded; remaining comparison work open | Visible baseline, in-process comparison and new wasm figures; explain the old frame-drop result | 0 |
| #43 | Systematic fuzzing missing | Seeded bounded campaigns, CI smoke runs and regression cases for parsers | 2 |
| #44 | Local-file CLI now implemented | Rust URL transport uses the shared live session, cancellation and bounded outputs; no duplicate player loop | 5 |
| #45 | Profiling and bit-exact decoder speedups already landed | Re-profile native AND wasm; benchmark remaining kernels and keep or reject SIMD/algorithm changes with quality gates | 5 |
| #46 | Deferred upstream contribution | Prepare an attributable patch when requested; do not message upstream now | Later |
| #47 | New 4K60 goal | Supported 4K60 SDR paths pass the foreground matrix and fall back predictably | 0, 4 |
| #48 | New size budget | Real-player feature builds, retained-size audit, startup/decode costs and CI growth report | 0, 1 |

## Web playback and library choices

These are recommended experiments and defaults, not a claim that a stack is
universally fastest. Keep a library only when its retained bytes and measured
benefit justify it.

| Choice | Recommendation | Main cost or limit |
| --- | --- | --- |
| Native HLS / HTML video | First choice for proven compatible sources; retain the documented failure fallback | Browser and OS decide codecs and recovery behavior |
| Rust → MSE → `<video>` | Main path for streams needing repackaging, sound repair or timeline repair | Browser buffer copies/decoding still cost memory; app must manage its queues |
| MediaCapabilities | Add exact codec/profile, dimensions, bitrate and fps checks to variant planning | `supported`, `smooth`, `powerEfficient` are capability estimates, not a playback pass |
| Audio/processing worker | Compare a dedicated Rust wasm worker with yielding; bound messages by bytes and cancel old generations | Transfer/startup cost; separate wasm instances add memory; transfers are not universally zero-copy from wasm |
| MSE in a worker | Probe support and test as an optional adapter | Specification support does not establish support on every target browser |
| WebCodecs + GPU presentation | A scoped experiment only if MSE cannot meet a measured need | We then own audio/video scheduling, frame lifetime, controls, PiP and accessibility |
| WebGPU/wgpu, WebGL or a UI framework | Do not add one just to display ordinary video; evaluate GPU work for a concrete filter or interlace need | More code and possible frame copies; a GPU renderer does not supply missing codecs |
| `wasm-bindgen`, `web-sys`, `js-sys` | Keep the direct browser bindings; audit selected features and match the CLI to each actual build's lockfile | Root and harness resolve separately; measure actual shipped assets |
| `rstreamkit-ac3`, `oxideav-core`, `oxideav-mp2` | Keep current decoding until a candidate beats it on corpus quality, CPU and retained size; audit decoder-only reachability | Runtime branches, JOC state, tracing and generic framework code may retain bytes; confirm with a size profiler |

Sources: [Media Capabilities](https://w3c.github.io/media-capabilities/),
[MSE including worker interfaces](https://www.w3.org/TR/media-source-2/),
[WebCodecs hardware hints and tradeoffs](https://www.w3.org/TR/webcodecs/#hardware-acceleration),
[Chrome's WebCodecs integration guide](https://developer.chrome.com/docs/web-platform/best-practices/webcodecs).
Hardware preferences are hints, not guarantees. Format support must exist in both
our parser/muxer and the platform decoder.

## Size, CPU and memory work

### Technology choices checked on 2026-10-10

| Technology | Where it can help | Next step and tradeoff |
| --- | --- | --- |
| MediaSource in a worker | Move packaging and browser buffer work away from the UI thread | Probe `canConstructInDedicatedWorker`; build a worker adapter after the shared session (#23, #34). Messages, cancellation and video controls still need coordination. |
| ManagedMediaSource | Let supported browsers signal when to stream and clear media under pressure | Safari experiment in #12. Handle streaming and buffer-change events, plus the documented AirPlay/remote-playback conditions. Keep the existing adapter available. |
| Rust Wasm SIMD128 | Process several audio values per instruction in measured hot loops | Benchmark #45 on the actual parser/DSP/FLAC bottlenecks; check PCM exactly and measure download growth. An instruction-set change does not guarantee a speedup. |
| Browser video decoding and configuration probes | Keep compressed video out of a custom raw-frame pipeline | Continue MSE/native HLS and probe full codec, size, frame rate and bitrate. Actual foreground playback remains the acceptance test. |

The [MSE editor's draft](https://w3c.github.io/media-source/) documents worker
construction, transferable media handles and ManagedMediaSource; these additions
need feature checks and are not uniform browser guarantees. [WebKit's release
notes](https://webkit.org/blog/14735/webkit-features-in-safari-17-1/) describe
ManagedMediaSource's remote-playback conditions. [Rust's target documentation](https://doc.rust-lang.org/stable/rustc/platform-support/wasm32-unknown-unknown.html)
explains Wasm target features. The test page now records worker MSE,
ManagedMediaSource and WebCodecs availability. Presence alone does not enable a
backend or prove that it is faster.

Measure four real-player builds: live only, live plus sound, live plus movies,
and full. The benchmark harness exposes extra entry points and is not the
minimal shipping player. Track optimized wasm, Brotli wasm, generated JavaScript,
total startup payload, initialization time, CPU slices and committed memory.
Keep development tools out of product download totals.

Use [retained-size profiling](https://rustwasm.github.io/book/reference/code-size.html)
before replacing dependencies or removing modules. Native-compatible HLS can
start without downloading the Rust engine; a host can load it on fallback. Cache
capability decisions by full configuration rather than probing every segment.
Source files and Cargo tree
entries are not shipped-byte counts. Unused encoder code may already disappear;
runtime-disabled object-audio paths may not. Preserve decoder licences, credits,
public contracts and corpus tests. Consider independent codec features or a
lazy audio module only if the startup saving exceeds its complexity and cold-start
cost. Compare `s`, `z` and `3`, LTO and wasm-opt rather than assuming the smallest
compiler setting is the fastest.

| Work | Complexity or memory target | Technique and limit |
| --- | --- | --- |
| Read, parse and copy compressed media | O(B) time for B bytes; bounded live working set | Streaming, reusable buffers and final-position writes; reading B bytes cannot be O(1) |
| Movie indexing and seeking | Existing sample index remains O(N); indexed seek is O(log K) for K keyframes | Avoid rebuilding/sorting an extra whole-film order table; consider lazy indexes only if their I/O tradeoff wins |
| Short playlist windows and track lists | Keep simple O(W) scans where W is small | Do not add a hash map merely to claim constant lookup; profile larger windows separately |
| Queues and prefetch | Enforce byte and count limits, including the largest in-flight item | Downstream backpressure; two giant segments are not a small queue; evaluate smaller safe fragments rather than retaining a complete long segment |
| Sound fallback | Reduce measured parse/DSP/allocation work; preserve decoder state | SIMD only after profiling; early downmix changes rounding and may require a separate quality-reviewed path |
| Buffer retention | Separate compressed queue, wasm, JS and decoded surfaces | Browser allocator/decoder memory is not captured by compressed-byte estimates |

A concrete 4K risk: at 40 Mbps, one six-second segment is about 30 MB; two are
60 MB, before old media or decoded surfaces. The new scheduling policy removes
the two-segment ahead floor and gates speculative downloads on estimated free
bytes, with an actual response-body cap. It includes separate audio output and
reacts immediately to a bitrate spike. Appended Rust output is released before
waiting for more buffer space.

Keep a conservative one-segment lookback for video dependencies: removing part
of the current GOP can also remove future dependent frames. This means long GOPs
can exceed the 32 MiB compressed-media target even with correct backpressure.
That target is not a hard process/decoder memory cap. Smaller independently
decodable fragments or a lower rendition remain the way to reduce that floor;
measure startup and stall costs as well as memory. Encrypted whole-segment downloads
and Web Crypto copies need a separate peak-memory measurement; do not assume the
clear-stream memory shape applies to AES sources.

An 8-bit 4:2:0 3840×2160 image contains about 12.44 MB of raw samples; RGBA is
33.18 MB. These are layout calculations, not browser allocator measurements.
Decoder reference surfaces and queued frames multiply that cost. Avoid moving
raw video through wasm just to draw it; hardware buffers can have additional
padding and different layouts.

The decoder profile puts about 64% in bitstream parsing and 20% in transforms.
If transforms alone became three times faster, the ideal overall gain would be
about 15%, using that profile. Re-profile first: native and wasm bottlenecks can
differ. Do not promise a threefold decoder win from SIMD or early downmix.

## Proposed acceptance targets

Started on 2026-10-10: added a playback-only Rust browser entry point, four repeatable
size builds with download budgets, downloadable browser reports and exact 4K60
capability probes. The baseline uncovered and fixed a Dolby decoder panic on browser
Wasm; CI now checks audio decoding on that target. Full compressed Wasm + JS is
229,337 bytes after the buffering follow-up. The new 18-second 40 Mbps 4K60
fixture completed twice with all 1,080 frames, no drops or recorded waits and
about 49% less committed Wasm memory than the original short run. CI now enforces
the four size budgets. See `docs/performance.md` for the measurement limits.
Phase 0 still needs longer 4K runs, more bitrate/segment cases, the remaining
browser matrix and broader CPU/platform-memory data.

Targets must be tied to named hardware, browser, codec/profile and bitrate.
They are goals to validate, not results we have achieved.

| Measure | Initial goal |
| --- | --- |
| 4K60 SDR | 30 foreground minutes on supported configurations; <1% dropped frames and no unexplained stalls on a provisioned link; report startup separately |
| UI processing | 95th-percentile processing slices under 8 ms, with no audio-decode long tasks; report total CPU and startup too |
| Download size | Candidate 20% reduction in the full real-player compressed startup payload with the same capabilities; set absolute per-flavor budgets after baseline |
| Sustained memory | Four-hour real-time run without increasing post-warmup resource trends; publish separate queue/wasm/JS/platform measurements or mark unavailable |
| Fault recovery | Script outages, expired resources, garbage, frozen windows, stops and seeks; record time to playing and time to target live latency separately |
| Adaptation | Step down on link or decode pressure, step up without repeated oscillation; report stalls, latency and wasted bytes on matched traces |
| CPU changes | Prefer at least 10% targeted repeated gain; disclose size/memory cost and smaller useful wins; no silent quality loss |
| Release | All native/wasm feature combinations and decoder checks pass; dependency packaging and fresh-clone demo verified before an approved tag/publication |

Start 4K fixtures at stated lab bitrates such as 15, 25 and 40 Mbps, keeping exact
input hashes. These are stress points, not universal quality requirements. Cover
short and long segments, H.264 initially, supported HEVC after carriage work,
AAC and Dolby fallback, and network jitter. Test platform playback on the same
machine to tell a toolkit problem from a decoder/device limit.

## What is realistic

Smooth 4K60 on supported desktop hardware is a reasonable engineering target.
We cannot guarantee it for all codecs, profiles or machines. Supporting HEVC
carriage is within scope; implementing a universal software HEVC decoder,
HDR pipeline, DRM, full surround/Atmos rendering and every FFmpeg feature would
be separate substantial projects.

We already have useful packaging wins. The next goal is trustworthy browser
playback with bounded work and small downloads, plus a shared native session.
Close existing acceptance gaps before growing the format list. Do not cut
working capabilities or audio quality just to make a size chart look better.
