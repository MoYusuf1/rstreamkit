# Changelog

## Unreleased (0.2.0)

- Start encrypted (AES-128) live channels as quickly as clear ones: the first segment is decrypted as it arrives instead of after the whole download, and its key is fetched alongside it. With 6 s 1080p segments at 40 Mb/s the first picture falls from 1.30 to 0.30 s.

- Show a raw TS channel's first picture as soon as it arrives instead of at the next keyframe (on the test stream, 2.03 s to 0.03 s); playback still moves when the first keyframe group is complete, so it does not run dry. `Continuous` gives its first keyframe group picture by picture; add `Continuous::first_group_out`.

- Show a live channel's first picture as soon as it has arrived: the first keyframe group of the first segment is appended picture by picture as it downloads, and playback moves once 1 s is buffered. With 6 s 1080p segments, the first picture falls from 0.58 to 0.29 s at 40 Mb/s and from 1.82 to 0.46 s at 8 Mb/s. `Transmuxer::take` now gives those pictures rather than whole keyframe groups. Fragments cut mid-stream, including `Continuous` keyframe groups, give their last picture its real duration instead of repeating the previous gap, which could re-base the clock on streams with uneven frame spacing. Add `ts::Demuxed::next_dts`.

- Live playback fails after 5 segments in a row are skipped with nothing played between them, with the last reason, instead of buffering forever (a broken key server or a dead channel fails every segment). A key server's wrong answer is quoted: `the AES-128 key is 35 bytes, not 16: "Cannot read stream: key unavailable"`.

- Keep sound in a stream that arrives in pieces (raw TS, a live channel's first segment) when the clock wraps part way through a group of pictures; it was held back and that fragment went out with no sound.
- Keep pictures and sound in order in a segment that straddles the 33-bit clock wrap (every 26.5 hours of a broadcast clock). They were sorted by raw timestamp, so the part after the wrap came first and the next segment was re-based about 124 ms early.

- Show a live channel's first picture about twice as soon: the browser player appends the first segment's first keyframe group while the rest of it downloads, instead of waiting for the whole segment. With 6 s 1080p segments at 40 Mb/s, the first frame falls from 1.20 to 0.58 s. Add `Transmuxer::take`, which gives a still-arriving `Segment`'s complete keyframe groups. A segment that answers 404 or 410 is requested once, not again, when it was downloaded ahead or streamed first, and a first segment's failed streamed request counts as the first of its retries. A download that breaks part way no longer leaves an uncaught `network error` on the page: cancelling the already-failed body is awaited. `Continuous` no longer reports AC-3 or MP2 as skipped when AAC plays beside it, or refuses H.264 with another video track beside it, matching whole segments.

- The command-line tool reads HLS playlists of fMP4 (CMAF) segments as well as TS: the init map is written once and each fragment's clock is made continuous. Encrypted playlists are still refused, with a reason.

- A download whose server sends headers and then no data for 20 s (`net::BODY_TIMEOUT`) fails and is retried, instead of freezing playback; `net::Timed` now times bodies too and needs a `Clone + Unpin + 'static` clock.
- The browser player gives up on a request that gets no response within 20 s (`net::HEADERS_TIMEOUT`) and retries it like any other temporary failure, reporting `Reconnecting`, instead of waiting forever on a server that accepted the connection and never answered. Slow providers that answer in 8-13 s still work. Add `net::Timed`, which wraps any `Fetch` with this limit for native hosts too.

- A movie that never starts or never decodes a picture now fails with its tracks and what the browser was given, for example `(MKV, H.264 1920x1080, AC-3 6 ch 48000 Hz, 5400.0 s; given as video/mp4; codecs="avc1.640028,flac")`, so a stall report doesn't need a probe of the file. Add `Movie::summary`.

- Describe HEVC movies by their real profile, tier and level (from `hvcC`) instead of always Main at level 4, so `canPlayType` answers for the actual stream (for example `hvc1.2.4.L93.90` for 720p Main 10), AV1 by its real profile, level, tier and bit depth from `av1C`, and VP9 from `vpcC` (MP4) or CodecPrivate features (Matroska).

- Play Matroska files without a cue index (written to a pipe or recorded) from the start instead of refusing them; starting later or seeking reports the new `Unsupported::NoIndex`. Add `Movie::seekable`.

- Start Matroska movies one round trip sooner: each range reaches the next keyframe cluster, and a keyframe's header completes the group before it. Time to Playing from a server answering in 300 ms falls from 1.34 to 0.99 s.

- Play Matroska tracks with header-stripping compression (older mkvmerge's default for AC-3 and H.264) instead of refusing them; other content encodings are still refused.

- Read laced Matroska sound (Xiph, fixed-size and EBML lacing, as mkvmerge writes by default): laced blocks were dropped, so such movies had no sound track data and nothing playable in the browser's combined buffer.

- Finish a Matroska movie when a range ends inside a trailing index or tags: the session now ends and hands over its last pictures instead of leaving the browser loop asking for nothing, without yielding.

- Report `Failed` when a movie's media sits buffered at the playhead for 8 s without a decoded picture, instead of waiting silently; handle `play()` promise rejections so pages log no uncaught errors.

- Start movies whose first media begins up to 3 s after the start (a late Matroska clock) instead of refetching the opening forever; report `Failed` when nothing playable reaches the start.

- Read Matroska `FlagInterlaced` as the spec defines it (1 interlaced, 2 progressive): progressive HEVC, VP9 and AV1 files made by FFmpeg were reported as interlaced.

- Decode AC-3 5.1 about 30% faster with byte-identical output by reusing unchanged bit allocation, tightening mantissa unpacking and inlining mantissa reads.

- Add an opt-in Wasm SIMD FFT: measured long/short transform time falls about 29%/25%, with matching finite outputs; default builds retain the scalar path.

- Reuse FLAC channel scratch buffers: measured frame allocation requests fall 40%, with unchanged output and 85 extra compressed bytes in the full engine.

- Count the whole player download, budget its HTML, and check startup requests and initial memory for all four builds.

- Decode movie sound with the shared browser work budget and discard old decoded ranges when a user seeks during a yield.

- Preserve pictures and coded-audio order across short raw clock restarts; emit complete audio PES packets without waiting for another start.

- Yield audio decoding on an eight-millisecond work budget; process raw TS in bounded pieces and flush its final fragment cooperatively.

- Shrink player diagnostics: full compressed Wasm and JavaScript falls by 766 bytes; reported seconds keep millisecond precision.

- Keep the selected external audio language and timeline when changing stream quality. Add a decoded-tone Chrome regression.

Breaking changes:
- HLS segment/variant metadata adds public fields.
- Movie frames carry borrowed or owned bytes (`Cow`) and a lifetime; MP4 sample
  extraction borrows input bytes. Matroska retains owned frames across pieces.
- Additional public enums are non-exhaustive; Demuxed gains passthrough metadata.

Changes:
- Fall back after two unusable live segments; respect manual quality choices and delay automatic upward retry.
- Tune the small player per package: measured MP2 Wasm packaging falls 16.38 → 13.37 ms, with a slightly smaller full download.
- Return stereo PCM directly instead of allocating an unused second output buffer.
- Restore mono AC-3 sound in stereo output by routing its gain to the center slot; mono PCM intentionally changes from silence.
- Scan H.264 units without temporary heap lists; preserve media output and keep all size budgets.
- Replace the growing duration cache with bounded storage while retaining linear playlist walks.
- Reuse cursor URI storage and release oversized buffers after shorter replacements.
- Prepare playlist walks once; reuse next-segment and duration metadata across finite buffer waits.
- Reduce measured native 20,000-entry bookkeeping from 573 ms to 0.339 ms, with a 160 KB cache.
- Share HLS playlist selection, refresh and restart control; add thirteen native session scenarios.
- Reuse the periodic refresh when go-live is requested during its wait.
- Share buffer trimming and backpressure with native hosts; add twelve control tests.
- Report failed browser removals and inspect live gaps without a temporary range vector.
- Share the retry loop between native and browser hosts; test nine outage/cancellation cases with fake time.
- Clear browser timers when their waiting future is dropped.
- Reject oversized MP4 timing runs, bound malformed edit counts and check file-offset arithmetic.
- Clamp extreme negative Matroska seeks; add five bounded parser fuzz targets and four regression tests.
- Cancel player tasks and Direct fetches on stop/drop; release buffer listeners.
- Handle source closure/removal while waiting for appends and removals.
- Retain HLS durations, source dates and discontinuities; prefer supported codecs.
- Recover skipped live segments, tolerate transient outages with bounded backoff,
  and refresh playlist snapshots after long pauses.
- Bound live buffers using appended bitrate and expose playback states/controls.
- Emit new initialization on codec changes and when sound first appears.
- Audio-only AAC/decoded-sound streams and explicit scrambled-TS errors.
- Borrow MP4 samples through fragment assembly.
- Stream continuous TS into bounded keyframe fragments.
- Parse CMAF maps, AES-128 keys and byte ranges; decrypt with browser Web Crypto.
- Merge external audio renditions into the video SourceBuffer.
- Add native Fetch/Sink transport types and CMAF clock rebasing, preserving mse re-exports.
- Explicit AC-3/basic E-AC-3 passthrough with spec-derived configuration boxes.
- Compress decoded stereo audio with fixed-predictor FLAC; yield between audio batches.
- Reduce measured programme FLAC encoder CPU by 29.3% while preserving output bytes.
- Adaptive throughput-based variant selection and one-segment bounded prefetch.
- Reproducible browser/demo and Node profiling tools; multi-segment soak tests.
- Isolate temporary FFmpeg test files to avoid parallel test races.
- Plain-language README; `docs/technical-breakdown.md`, a handoff for contributors and AI agents.
- `rstreamkit` command (feature `vod`): MPEG-TS from a file or standard input, local HLS
  playlists of TS segments, MP4 and Matroska in; fragmented MP4 out, to a file or standard
  output. It replaces the `transmux` and `dump` examples. Reading, converting and writing run on
  their own threads with two-piece queues; it refuses to write over its own input, and checks
  the input exists before creating the output.
- TS demuxing 2 to 2.8 times faster with identical output: start codes are found a word at a
  time, packets reach their stream without a hash lookup, and a continuous stream's buffer
  keeps its size from one keyframe to the next instead of growing again each time.
- `tools/compare.py` races the command against FFmpeg on five workloads, checks both outputs,
  and reports FFmpeg's own processing time and both tools' true peak memory (`tools/peak.rs`).
- The AC-3/E-AC-3 decoder is now `rstreamkit-ac3` in `ac3/`: oxideav-ac3 0.0.11 by Mark Karpeles
  (MIT, licence and credit kept), decoding twice as fast with byte-identical output. Its
  transform constants are built once, powers of two come from their exponent bits, the inverse
  FFT is table-driven, the downmix plans its taps once, and trace switches are read once. A test
  pins decoded checksums to the original's output; its own 558 tests run in CI.
- CI's app-name guard no longer names the removed `examples` directory: grep exits with status
  2 for a missing path even when it finds a match, which would have silently disabled the guard.

## 0.1.0

Initial H.264 HLS/MPEG-TS transmuxing, optional Rust AC-3/E-AC-3/MP2 decoding,
MP4/Matroska indexing and browser MediaSource playback.
