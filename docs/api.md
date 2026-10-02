# Public API contract

See [issue acceptance and browser validation](issues.md) for tested platforms and remaining checks.

The crate root (`Transmuxer`, streamed `Segment`, `Continuous`, `Output`, `Init`, `Fragment`,
`Error`, `Unsupported`) is the supported transmuxing interface. `hls` supports
playlist inspection and variant selection. With `vod`, `vod::Movie` and `Session`
support indexed playback and seeking. `cmaf::Rebaser` inspects initialization segments and
rebases supported fragmented MP4 clocks on native and wasm targets.
`net::{Fetch, Response, FetchError, Sink}` supports native and browser hosts. On wasm, `mse::{Fetch, Response, Direct,
Player, Status}` supports browser playback with application-owned transport.

The public low-level modules (`avc`, `ts`, `fmp4`, `sound`, `body`, `mp4`, `mkv`)
remain available for embedders and tools. Their changes are also API changes;
we do not hide them merely to avoid documenting a breaking change.

- `sound` and `vod` remain optional and enabled by default. Browser glue is
  target-gated, so a separate `web` feature would not reduce native binary size.
- Transport URLs are strings resolved by the browser. No HTTP-client or URL crate
  types leak into signatures. `Direct` uses browser fetch; custom `Fetch` owns its
  headers, credentials and proxy policy.
- `Response` remains constructible with a struct literal. Its field contract is
  stable within a minor release; adding a required field is a breaking change.
  Public enums use `non_exhaustive` where additional variants are foreseeable.
- The library reports capabilities/errors. The application decides whether to
  choose another supported stream or show an unsupported message. Playback does
  not launch or link FFmpeg.
- MSRV is Rust 1.88 (edition 2024 and stable slice `as_chunks`). Dependency
  minimums must be checked as part of release validation. Rust 1.88 native and wasm checks pass with both default and minimal features using the
  checksum-verified official toolchain.
- Before 1.0, incompatible API changes require a new minor version and a
  changelog migration note. Patch releases preserve the API. Release tags are
  created only after validation of the tagged commit.

Pending 0.2 migration: HLS metadata adds fields to Segment/Variant; movie Frame
now carries a Cow slice and a lifetime, and MP4 Index::frames borrows its input.
Code building those structures must provide the new fields or use defaults where
available; owned frame data can be supplied with `.into()`.

`mse::Player` exposes pause/resume/seek/go_live and validated variant/language
selection. `stats()` returns a snapshot of appended bitrate, buffer/latency and
skipped segments, including `live_window_seconds` (zero for finite media).
`Player::resume` requests fresh live media after a pause longer than that window.
Hosts controlling the video element directly must own the equivalent resume policy.
The existing mse transport paths re-export the net types.
Explicit `Transmuxer::passthrough_ac3(true)` enables AC-3 or simple E-AC-3 compressed
samples. Probe MSE codec support before opting in. Six-block E-AC-3 with one
independent substream and no mixing metadata is supported; other layouts decode
through the optional sound feature. Dolby object/Atmos metadata is not supported.

## Platform independence

The media core and transport/output traits compile on native Rust targets and in
WebAssembly. `mse` is an optional platform adapter selected by the wasm target;
it connects the same pipeline to browser media APIs. Native hosts supply `net::Fetch`
and `net::Sink`, or use `Transmuxer`/`Continuous` directly. The example `transmux`
is a native Rust command-line tool and does not launch or link FFmpeg.

The library is not a complete FFmpeg replacement: it does not provide general
video encoding, every codec, or a native device renderer. Browser AES-128 HLS uses
WebCrypto through Rust; native hosts must supply decryption before the media core.

## Browser native-first recipe

Keep this policy in the host app rather than adding an automatic player helper:

1. Inspect the master playlist's CODECS and the platform's capabilities. Only try
   native HLS for a known compatible stream. A `canPlayType` result alone does not
   prove that a particular soundtrack works.
2. Attach an `error` listener before setting the video source to the HLS URL.
3. On native failure, clear that source and start `mse::start` with the same URL
   and the app's transport. Track which mode is active to avoid a retry loop.
4. On a channel change, remove the native listener and drop the old `Player` before
   starting the new source. A stale native error must not restart the old channel.

This lets native HLS handle compatible streams and uses the Rust path for audio
decoding and repair of broken timelines. Unsupported HEVC, DRM, Atmos and fully
interlaced video still need a supported alternative or a clear error. Desktop
Edge and Safari playback validation remains open.
