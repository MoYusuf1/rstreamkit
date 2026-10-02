# Issue acceptance and playback validation

Local results as of 2026-10-01. An implementation is not the same as a closed issue.
The rows below record evidence and the acceptance work still needed. No GitHub
issue has been closed by this change. Timing details are in [performance.md](performance.md).

| Issue | Local work and evidence | Remaining acceptance |
| --- | --- | --- |
| #2 Network and bad segments | Bounded backoff; skip live 404/410 and corrupt segments. Repeated browser 404s play beyond 72 seconds. Two ten-second outages recover automatically. Garbage run reaches 133.9 seconds after skipping eight corrupt segments, without Failed. | Acceptance review; robustness is bounded by the retry budget. |
| #3 Long pause | Refresh after throttling; native tests cover normal/broken sequence windows. Pause over 60 seconds against a 12-second window, then Player::resume: playback advances, latency returns to 2.69 seconds, no Failed. | Server eviction of old resources is separate from this retained-resource harness; repeated 404 recovery is tested under #2. |
| #4 Freeze and broken sequence | URI/range identity, freeze status and gap recovery; native sliding-window tests. Browser freeze recovers twice and reaches 74.7 seconds with one range. Broken-sequence playback advances past 56 seconds. | More provider quirks; long-pause/live-edge check recorded separately. |
| #6 Parameter changes | New initialization on changed SPS/audio and track additions; native regression. Browser audio-appearing fixture reaches the end in six seconds. A 320×180/48 kHz → 640×360/44.1 kHz → original sequence reaches 6.097 seconds. Sources drain before replacement. | Additional profiles and live changes; rebuilding can introduce a brief transition. |
| #7 Channel switching | Drop cancels the run and Direct requests. Thirty switches at 200 ms with delayed responses produce no stale Playing state. | Delayed movie/range/seek switching and listener/reader accumulation checks. |
| #8 Variants and renditions | CODECS filtering, exposed metadata and separate audio merged into one MSE buffer. Browser demuxed playlist plays. | Audible verification, language switching at later playback positions and mixed HEVC/H.264 master. |
| #9 ABR | Native throughput/headroom selection tests and browser switching implementation. | Throttled provider must settle, then step up when bandwidth returns. |
| #10 Raw TS | Incremental bounded pipeline; arbitrary chunk tests. Endless browser TS plays beyond 25 seconds. | Longer endurance; audio-only raw stream behavior. |
| #11 Format gaps | CMAF, AES-128, radio, ranges, language parsing and clear scrambling errors. CMAF/radio/AES reach 24 seconds; range CMAF plays 720 frames with no drops. | More external clock/language combinations and malformed encrypted inputs. |
| #12 HEVC/interlacing | Capability probes and documented decision to defer a universal canvas backend. Clear unsupported reports; no FFmpeg runtime fallback. | Desktop target hardware support and decode/deinterlace CPU measurements. |
| #13 Prefetch | One bounded speculative download overlaps append work. | Equivalent throttled before/after startup and stall measurements. |
| #14 Live edge | Latency stats, modest rate catch-up and explicit go-live. | Induced ten-second stall must recover to target latency within a measured bound. |
| #15 Timing tags | EXTINF, date/discontinuity/range metadata and total duration; parser tests. | Review API documentation against the issue. |
| #16 Gapless video | Small boundary snapping and monotonic repeated-segment tests. | Real multi-segment stream boundary jitter, beyond generated fixtures. |
| #18 Worker timer and CPU | Global timer works in a worker: 11 ms for a requested 10 ms. Measured wasm AC-3 processing and transfer overhead. | Broader realistic programme corpus; worker placement remains host policy. |
| #22 Controls and stats | Pause/resume/seek/go-live, variant/language controls, typed states and stats. | Adoption in the client app is outside this repository. |
| #23 Native fake provider | Portable Fetch/Sink, actual incremental download/append helpers, native missing-segment and clock tests; shared scheduling policy. | Extract the complete control loop, including throttle/trim/retry orchestration, and script every required scenario natively. |
| #24 CI and matrix | All four feature combinations checked natively and for wasm; CI and MSRV jobs added. | Green main CI after landing; Edge/Safari matrix still needs target machines. |
| #25 Multi-segment/soak | Thousands of restarts; 10,000 wasm pushes keep committed memory at 1,900,544 bytes. Existing independent multi-push decode tests pass. | Generated resolution/discontinuity sequences and a true real-time browser soak. |
| #26 Roadmap | Shared native/wasm core and measured performance direction documented. | Update the tracking issue after individual acceptance. |
| #29 API/release | API contracts, non-exhaustive enums, migration notes and Rust 1.88 native/wasm checks. Version prepared as 0.2.0. | Reviewed tagged commit; no tag or release has been made. |
| #30 Adoption | Local demo, wasm documentation, changelog and successful publish dry run. Clean source copy builds/generates/profiles with one command. | Release publication after review; no package uploaded. |
| #32 Dolby passthrough | Opt-in AC-3/simple E-AC-3; configuration bytes match FFmpeg and independent decode succeeds. | Actual playback on a desktop platform with Dolby support. Linux test browser reports unsupported. |
| #33 Native-first | Host-owned native-HLS/fallback recipe in [api.md](api.md). | Recipe review; no automatic policy helper is added. |
| #34 Main-thread audio | Synchronous cold run 84.2 ms; yielded six-second AC-3 run has no long tasks and starts in about 195 ms. | Comparable repeated dropped-frame measurements; movies/encrypted/raw paths still need yielding evaluation. |
| #35 Movie copies | MP4 sample data borrowed; Matroska cross-piece samples remain owned. Decode tests and CPU/memory profiles pass. Browser MP4 reaches 40.8 seconds; Matroska reaches 68.2 seconds. Both runs report zero dropped frames. | Review measurement scope; CPU remains dominated by compressed audio decoding. |
| #36 Live buffer | Documented 32 MiB policy based on appended bitrate. About 28.58 MB buffered after 19 minutes at 8.899 Mbps. | Investigate dropped 1080p frames separately; this is a compressed-byte estimate. |
| #37 Closed source wait | Wait races update completion, source closure/end and buffer removal; cancellation drops the waiter. Browser hooks confirm updating=true at closure/removal and both settle with Failed rather than hanging. | Mid-remove closure and removal of an unrelated buffer. |
| #38 Harness | Build/demo/profile commands, generated media, Node/browser probes and clean-source reproduction. Generator cache includes arguments/source hashes; historical low-resolution and fresh 1080p movie results are distinguished. | CI profiling report and broader hardware repetition. |
| #39 FLAC | Real programme sample: 29.7% of PCM size at about 12.4 ms encoder CPU; trade taken. Lossless noise/extreme/ramp checks pass. | Broader corpus improves confidence; issue needs the measured decision recorded. |
| #40 Discontinuity skew | Twenty alternating ±100 ms splices: maximum reduced from 316.811 to 105.467 ms with correction capped at 2%; regression test. | Real broadcast splice measurements. |

## Desktop browser matrix

All results below refer to Linux Chromium 154 in the desktop test browser unless
another platform is named. A positive capability probe is not a playback test.

| Platform | H.264/AAC MSE | H.264/FLAC MSE | AC-3/E-AC-3 MSE | HEVC | Other evidence |
| --- | --- | --- | --- | --- | --- |
| Linux Chromium 154 | Played TS/CMAF/ranges | Played Rust-decoded AC-3 | Probe says unsupported | MSE and WebCodecs probes unsupported | Radio/AES/external audio/raw TS tested locally |
| Desktop Chrome on target OS | Unverified | Unverified | Unverified | Unverified | Hardware/OS must be recorded |
| Desktop Edge | Unverified | Unverified | Unverified | Unverified | Target machine required |
| Desktop Safari | Unverified | Unverified | Unverified | Unverified | Target macOS machine required |

Firefox and mobile Safari are outside the user's current desktop validation target.
No claim is made about their codec support or ManagedMediaSource compatibility.
