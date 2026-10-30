# Issue acceptance and playback validation

Results as of 2026-10-01. The work landed on `main` on 2026-10-09 through
PR #41. The table records evidence and further checks. Timing details are in
[performance.md](performance.md).

Local acceptance is complete for #4, #6, #7, #10, #15, #18, #33, #35, #36,
#38, #39 and #40, and those issues were closed when the work landed. Extra
checks listed below remain useful follow-up work. Since then, #2, #3, #37 and #59
closed after their provider/audio fixes landed.

| Issue | Local work and evidence | Remaining acceptance |
| --- | --- | --- |
| #2 Network and bad segments | Bounded backoff; skip live 404/410 and corrupt segments. Repeated browser 404s play beyond 72 seconds. Two ten-second outages recover automatically. Garbage run reaches 133.9 seconds after skipping eight corrupt segments, without Failed. | Three scripted Chrome cases pass: ten-second HTTP outage recovers on attempt five, single 404/garbage are skipped and playback passes the damaged boundary. Landed in PR #61; issue closed. |
| #3 Long pause | Refresh after throttling; native tests cover normal/broken sequence windows. Pause over 60 seconds against a 12-second window, then Player::resume: playback advances, latency returns to 2.69 seconds, no Failed. | Two real-time eviction runs pass after a 50-second pause across a 9.18-second window. The stricter run verifies continued playback after the live-edge seek. Landed in PR #61; issue closed. |
| #4 Freeze and broken sequence | URI/range identity, freeze status and gap recovery; native sliding-window tests. Browser freeze recovers twice and reaches 74.7 seconds with one range. Broken-sequence playback advances past 56 seconds. | More provider quirks; long-pause/live-edge check recorded separately. |
| #6 Parameter changes | New initialization on changed SPS/audio and track additions; native regression. Browser audio-appearing fixture reaches the end in six seconds. A 320×180/48 kHz → 640×360/44.1 kHz → original sequence reaches 6.097 seconds. Sources drain before replacement. | Additional profiles and live changes; rebuilding can introduce a brief transition. |
| #7 Channel switching | Drop cancels the run and Direct requests. Thirty switches at 200 ms with delayed responses produce no stale Playing state. | 2026-10-30, Chrome 156, every range answered after 800 ms (and 300 ms): switching from an MKV to an MP4 before the first played, then seeking the MP4 to 30, 70 and 50 s 300 ms apart, plays the MP4 with its own duration and lands at 50 s with no page errors. Listener/reader accumulation is still unchecked. |
| #8 Variants and renditions | CODECS filtering, exposed metadata and separate audio merged into one MSE buffer. Browser demuxed playlist plays. | Chrome decoded-tone test passes a late language change and quality switch; mixed master fetches zero HEVC playlists. Landed in PR #65; issue closed. Edge/Safari remain in #24/#47. |
| #9 ABR | Native throughput/headroom selection tests and browser switching implementation. | Delayed-provider Chrome check passes twice: starts high, stays low through slow responses, then returns high after recovery. Throttle acceptance landed in PR #58; four native failure/control cases and a Chrome failed-encoder fallback now pass. Landed in PR #64; issue closed. |
| #10 Raw TS | Incremental bounded pipeline; arbitrary chunk tests. Endless browser TS plays beyond 25 seconds. | Longer endurance; audio-only raw stream behavior. |
| #11 Format gaps | CMAF, AES-128, radio, ranges, language parsing and clear scrambling errors. CMAF/radio/AES reach 24 seconds; range CMAF plays 720 frames with no drops. | More external clock/language combinations and malformed encrypted inputs. |
| #12 HEVC/interlacing | Capability probes and documented decision to defer a universal canvas backend. Clear unsupported reports; no FFmpeg runtime fallback. 2026-10-30: with the refusal lifted in a scratch build, Chrome 155 on Linux played 1080i29.97 H.264 (top field first, 8 Mb/s) through MSE with first frame at 0.76 s and 0 of 228 frames dropped, but a full-resolution frame shows comb lines on moving edges: Chrome weaves the fields and does not deinterlace. Refusing interlaced video (`Unsupported::Interlaced`, or play it anyway with `partial`) stays correct; a host's deinterlacing re-encode is what fixes the picture. | Desktop target hardware support and decode/deinterlace CPU measurements. |
| #13 Prefetch | One bounded speculative download overlaps append work. | Equivalent throttled before/after startup and stall measurements. |
| #14 Live edge | Latency stats, modest rate catch-up and explicit go-live. | Real-time Chrome check: 10.474 s playback stall, 220.878 s recovery to the 6–8 s control band, zero seeks. Landed in PR #66; issue closed. Not smooth-rendering or 4K evidence. |
| #15 Timing tags | EXTINF, date/discontinuity/range metadata and total duration; parser tests. | Implemented; parser tests cover per-segment duration, timing tags and total span. |
| #16 Gapless video | Small boundary snapping and monotonic repeated-segment tests. | Chrome real-source acceptance: 120.7 s at 720p60, eleven boundaries with one range and zero dropped-frame increase. Short raw reset and audio-order regressions pass; landed in PR #69, issue closed. |
| #18 Worker timer and CPU | Global timer works in a worker: 11 ms for a requested 10 ms. Measured wasm AC-3 processing and transfer overhead. | Implemented: six-second wasm processing is about 82× realtime in the measured run. Broader corpus and worker placement remain host policy. |
| #22 Controls and stats | Pause/resume/seek/go-live, variant/language controls, typed states and stats. | Adoption in the client app is outside this repository. |
| #23 Native fake provider | Shared retry, buffer and playlist-session control; 34 native integration tests and a Chrome go-live regression. | Extract per-segment processing/error policy and variant/rendition changes into one generic runner; add streamed throttling/pending-I/O cases through it. |
| #24 CI and matrix | All four feature combinations checked natively and for wasm; CI and MSRV jobs added. | PR CI is green, including checks, MSRV and profiling. Main CI after landing and Edge/Safari playback still need verification. |
| #25 Multi-segment/soak | Thousands of restarts; 10,000 wasm pushes keep committed memory at 1,900,544 bytes. Existing independent multi-push decode tests pass. | Generated resolution/discontinuity sequences and a true real-time browser soak. |
| #26 Roadmap | Shared native/wasm core and measured performance direction documented. | Update the tracking issue after individual acceptance. |
| #29 API/release | API contracts, non-exhaustive enums, migration notes and Rust 1.88 native/wasm checks. Version prepared as 0.2.0. | Reviewed tagged commit; no tag or release has been made. |
| #30 Adoption | Local demo, wasm documentation, changelog and successful publish dry run. Clean source copy builds/generates/profiles with one command. | Release publication after review; no package uploaded. |
| #32 Dolby passthrough | Opt-in AC-3/simple E-AC-3; configuration bytes match FFmpeg and independent decode succeeds. | Actual playback on a desktop platform with Dolby support. Linux test browser reports unsupported. |
| #33 Native-first | Host-owned native-HLS/fallback recipe in [api.md](api.md). | Decision implemented: use the documented host-owned recipe rather than automatic library policy. |
| #34 Main-thread audio | Synchronous cold run 84.2 ms; yielded six-second AC-3 run has no long tasks and starts in about 195 ms. | Repeated Chrome comparisons and raw/HLS/movie work-budget decoding pass; nine range-byte checks and four movie/encryption/seek scenarios pass. Merged in PR #70; issue closed. Platform endurance remains separate. |
| #35 Movie copies | MP4 sample data borrowed; Matroska cross-piece samples remain owned. Decode tests and CPU/memory profiles pass. Browser MP4 reaches 40.8 seconds; Matroska reaches 68.2 seconds. Both runs report zero dropped frames. | Review measurement scope; CPU remains dominated by compressed audio decoding. |
| #36 Live buffer | Documented 32 MiB policy based on appended bitrate. About 28.58 MB buffered after 19 minutes at 8.899 Mbps. | Investigate dropped 1080p frames separately; this is a compressed-byte estimate. |
| #37 Closed source wait | Four repeatable Chrome faults: close during append/remove and remove active buffer settle with Failed; unrelated removal still reaches Ended. Every injected fault records updating=true. | Landed in PR #53; issue closed. |
| #38 Harness | Build/demo/profile commands, generated media, Node/browser probes and clean-source reproduction. Generator cache includes arguments/source hashes; historical low-resolution and fresh 1080p movie results are distinguished. | Clean-source profiling and CI report pass. Broader hardware repetition is follow-up work. |
| #39 FLAC | Real programme sample: 29.7% of PCM size; encoder CPU reduced from 12.583 to 8.899 ms (29.3%) across twelve alternating fresh processes; trade taken. Lossless noise/extreme/ramp checks pass. | Decision implemented and recorded: take the compression trade. A broader corpus would improve confidence. |
| #40 Discontinuity skew | Twenty alternating ±100 ms splices: maximum reduced from 316.811 to 105.467 ms with correction capped at 2%; regression test. | Real broadcast splice measurements. |
| #45 Audio CPU | Measured opt-in SIMD FFT: long/short transform time falls about 29%/25%, with 2,564 exact finite-output checks. Full 5.1 AC-3 processing gains about 7%; full SIMD engine is 233,411 Brotli bytes. | Native and broader programme profiling; MP2 synthesis. Default path remains available; Edge/Safari acceptance stays separate. |
| #48 Footprint | Per-flavor budgets; zero-list video scan and one-buffer stereo conversion. Conversion requested bytes fall 12,288 → 6,144; full player remains under 245,000 Brotli bytes. | Retained-symbol and build-profile audits recorded. FLAC allocation requests fall 30,749 → 18,461 B per measured frame. Full default startup now counts 236,798 Brotli bytes (236,161 for opt-in SIMD); five fresh Chrome processes per build record readiness and initial Wasm memory. Same-feature 20% reduction remains unmet; broader playback memory/CPU comparisons remain. |
| #59 Mono Dolby silence | Correct center-slot routing in LoRo/LtRt; positive/negative unit regression, 66.6 dB independent FFmpeg comparison and non-silent equal Chrome audio channels. Full download shrinks 163 Brotli bytes. | Landed in PR #60; issue closed. Edge/Safari remain separate acceptance. |

## Host-app movie reports (2026-10-29)

A host app's experimental player recorded two silent movie stalls on real provider files: a 1080p
MKV with 1.8 s buffered at `readyState` 1 for 293 s, and a resumed MKV episode with nothing
buffered. Neither provider file was available here, so each cause below was reproduced with a
generated file in Chrome, fixed, and kept as a Chrome regression in
`tools/node/movie-late-start-smoke.cjs`.

| Cause | Fix | Evidence |
| --- | --- | --- |
| First media a little after zero (late clock) taken for a seek; the opening refetched forever | dd34500 | Before: 526 range requests in 15 s, never plays; after: plays, 5 requests |
| Laced sound blocks (mkvmerge's default) dropped; empty combined buffer | f206821 | Before: `readyState` 1; after: 0-12 s buffered and playing |
| Buffered media never decoded, or nothing playable at the start, reported nothing | 4b6c1ea, dd34500 | `Failed` with a reason after 8 s / once 25 s are buffered elsewhere |
| A range ending inside the trailing Cues left the session unfinished and the loop spinning | 205f303 | Native regression test |
| Matroska `FlagInterlaced` read backwards | b24539e | Unit test for 0, 1, 2 |

Not confirmed against the original files; the host app pins 72f8cdd until it updates.

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
