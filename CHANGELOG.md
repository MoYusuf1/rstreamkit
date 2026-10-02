# Changelog

## Unreleased (0.2.0)

Breaking changes:
- HLS segment/variant metadata adds public fields.
- Movie frames carry borrowed or owned bytes (`Cow`) and a lifetime; MP4 sample
  extraction borrows input bytes. Matroska retains owned frames across pieces.
- Additional public enums are non-exhaustive; Demuxed gains passthrough metadata.

Changes:
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
- Adaptive throughput-based variant selection and one-segment bounded prefetch.
- Reproducible browser/demo and Node profiling tools; multi-segment soak tests.
- Isolate temporary FFmpeg test files to avoid parallel test races.

## 0.1.0

Initial H.264 HLS/MPEG-TS transmuxing, optional Rust AC-3/E-AC-3/MP2 decoding,
MP4/Matroska indexing and browser MediaSource playback.
