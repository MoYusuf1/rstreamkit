# Changelog

## Unreleased (0.2.0)

Breaking changes:
- HLS segment/variant metadata adds public fields.
- Movie frames carry borrowed or owned bytes (`Cow`) and a lifetime; MP4 sample
  extraction borrows input bytes. Matroska retains owned frames across pieces.
- Additional public enums are non-exhaustive; Demuxed gains passthrough metadata.

Changes:
- Prepare playlist walks once; reuse next-segment and duration metadata across finite buffer waits.
- Reduce measured native 20,000-entry bookkeeping from 573 ms to 0.339 ms, with a 160 KB cache.
- Share HLS playlist selection, refresh and restart control; add thirteen native session scenarios.
- Reuse the periodic refresh when go-live is requested during its wait.
- Share buffer trimming and backpressure with native hosts; add twelve control tests.
- Report failed browser removals and inspect live gaps without a temporary range vector.
- Share the retry loop between native and browser hosts; test nine outage/cancellation cases with fake time.
- Clear browser timers when their waiting future is dropped.
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
