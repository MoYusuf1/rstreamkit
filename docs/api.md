# Public API contract

See [issue acceptance and browser validation](issues.md) for tested platforms and remaining checks.

The crate root (`Transmuxer`, streamed `Segment`, `Continuous`, `Output`, `Init`, `Fragment`,
`Error`, `Unsupported`) is the supported transmuxing interface. `hls` supports
playlist inspection and variant selection. With `vod`, `vod::Movie` and `Session`
support indexed playback and seeking. `cmaf::Rebaser` inspects initialization segments and
rebases supported fragmented MP4 clocks on native and wasm targets.
`net::{Fetch, Response, FetchError, Sink, Clock, retry, RetryError}` supports native and browser hosts. On wasm, `mse::{Fetch, Response, Direct,
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
and `net::Sink`, or use `Transmuxer`/`Continuous` directly. The `rstreamkit` command
(`src/main.rs`, feature `vod`) is a native tool built only on this public API: TS,
local HLS playlists of TS segments, MP4 and Matroska in, fragmented MP4 out, standard
input and output included. It does not launch or link FFmpeg. Reading URLs waits for
the shared live session, so the command reads local files and pipes for now.

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

`net::retry` runs the same bounded retry loop on native and browser hosts. Supply a
`Clock`, an operation, an error classifier, a stop check and a reconnect callback.
It makes at most seven attempts with 31 seconds of total backoff. Permanent errors
return immediately. Stop checks run between awaits; drop the future to cancel a
pending request or timer. Host `Fetch` and `Clock` implementations must release
pending work on drop. Browser timers clear on drop too. The browser uses the shared buffer and
playlist-session helpers below; complete media processing still needs extraction.

`net::PlaybackBuffer` supplies playback position, buffered duration, gap crossing
and async range removal. `net::trim_buffer` owns the rewind cutoff;
`net::wait_for_room` owns the actual backpressure loop through `Clock`.
`BufferWait` distinguishes room available (including whether backpressure was
needed, even if gap recovery freed space before a timer), go-live
interruption and cancellation. Limits are finite, nonnegative seconds. Live
hosts refresh after waiting/interruption; finite hosts keep their current place.
Dropping a pending future releases host work. The browser uses these helpers together with the shared playlist controller.

`session::{Session, PlaylistSource, Controls, Preparation}` provides shared HLS
selection and refresh. `prepare` applies backpressure and refreshes after a pause
or go-live; finite media keeps its place. `wait_and_refresh` waits half a target
duration (1–5 seconds), reports a stalled playlist through the source callback,
and refreshes live media. Go-live during that wait uses the same refresh.
`Session::next` borrows a segment; call `consumed` after successful output or an
explicit skip. `discontinuous` detects sequence gaps/restarts. `finished` is true
only when finite input is exhausted. `session::buffer_limits` exposes the same
budget calculation used by the browser. Hosts own URL resolution, playlist body
limits, transport retry/error classification and media output. Stop is checked
between awaits; drop pending work for immediate cancellation. Full per-segment
processing, variant/rendition changes and a single generic media runner are still
pending. The new controller does not claim to finish #23 by itself.

`Session::batch(&media)` returns an exact-size, fused `session::Batch` iterator.
Each `ScheduledSegment` borrows the current/next segment and supplies cached
remaining/window seconds. Build once per snapshot and keep it across finite
buffer waits; rebuild after refresh or a control/selection change. The immutable
borrow prevents changing the playlist underneath a batch. The walk now uses constant extra duration storage, without copying media bytes.
Ordinary finite durations need no heap allocation. Extreme inputs use one bounded
fallback allocation instead of an array that grows with the playlist. Remaining
durations are correctly rounded from exact totals; results can differ from the
old repeated sums by roundoff. The playlist itself still occupies O(n) memory.
`Session::consumed` reuses URI storage and replaces an oversized buffer when its
capacity exceeds four times the new URI length (with a 256-byte minimum).

### Cooperative browser audio

On Wasm, Transmuxer::finish_yielded, Continuous::feed_yielded and
Continuous::finish_yielded keep the synchronous output bytes while yielding
during sound decoding. The browser checks elapsed work every eight audio frames
and yields after roughly eight milliseconds; one frame can exceed the budget.
The clock uses global performance.now() (also usable in workers), falling back
to Date.now() when unavailable. Native synchronous methods stay available.

A host should feed raw input in bounded pieces and append each returned output
before asking for more. The browser adapter uses 16 KiB pieces; this limits
outputs awaiting append even if its transport returns a large network chunk.
The keyframe buffer still has its existing explicit cap. This does not bound
memory already allocated by a caller's transport.

Movie range playback can use vod::Session::push_yielded on Wasm. It shares
frame selection, seeking and fragment packaging with the synchronous push
method, and uses the same audio work budget. A cancelled processing future may
already have advanced the cursor or decoder: discard that session when
cancelling. The browser adapter owns the session and rechecks seek position
after sound processing before it appends the result.
