# rstreamkit

A streaming toolkit for the browser, in pure Rust: no C, no ffmpeg install, and small enough for
WebAssembly. It turns HLS, MPEG-TS and movie files (MP4, Matroska) into fragmented MP4 that a browser
plays through MediaSource, decoding the sound browsers can't (AC-3, E-AC-3, MP2) on the way.

It is a standalone, open-source library (MIT OR Apache-2.0), browser-first for now, and apps plug into
it, not the other way round: see [CONTRIBUTING.md](CONTRIBUTING.md). Its first client is
[RIPTV](https://github.com/MoYusuf1/riptv), a live-TV app.

| Module | Job |
|---|---|
| `hls` | Playlist parsing (media and master playlists, sliding windows) |
| `ts` | MPEG-TS demux: H.264 video, AAC (ADTS), AC-3, E-AC-3 and MPEG audio |
| `avc` | H.264 SPS: size, pixel aspect ratio (`pasp`), interlacing |
| `sound` (decoders: feature `sound`) | AC-3 / E-AC-3 / MP2 decoded to stereo PCM, written as FLAC frames |
| `body` | Reads a response body with a size cap, so an endless stream can't fill memory |
| `fmp4` | fMP4 init segment and fragments (`avc1` + `mp4a` or `fLaC`) |
| `Transmuxer` (crate root) | Feeds segments in, gets an init segment and fragments out |
| `vod`, `mp4`, `mkv` (feature `vod`) | Movie files (MP4, Matroska): index, tracks and byte-range reading, played as fMP4 |
| `mse` (wasm only) | Fetches the playlist and segments (through a `Fetch` you supply) and appends them to a `<video>` |

```rust
let mut t = rstreamkit::Transmuxer::default();
let out = t.push(&segment_bytes)?;      // out.init (once), out.fragment, out.skipped_audio
```

`Transmuxer::default().decode_sound(false)` turns the sound decoding off: AC-3, E-AC-3 and MP2 are then
dropped and named in `skipped_audio` (in the browser glue, `mse::start(.., decode_sound, ..)` reports
`Status::Unsupported(Unsupported::Sound(..))`), for a caller that has real ffmpeg do it instead.

In the browser, `mse::start(video, playlist_url, fetcher, partial, decode_sound, report)` plays a stream
(addresses are plain strings). `fetcher` is how bytes reach the page: `mse::Direct` uses the browser's own
`fetch` (the server must allow it: CORS, and `Access-Control-Expose-Headers: Content-Range` for byte
ranges), and an app that goes through a proxy implements the small `mse::Fetch` trait for it.
rstreamkit knows nothing about proxies or headers; it asks for real addresses (and, for movie files read
with `mse::probe` and `mse::play_movie`, byte ranges) and resolves playlist entries against each
response's final address. It depends on no HTTP or URL crate: the browser already has `fetch` and
`URL`, and does the resolving. Ask the browser first where it can do the job itself:
`mse::plays_hls_natively()` (Safari, and recent Chrome, play HLS on their own) and `mse::can_play(mime)`.

`cargo run --example transmux -- out.mp4 seg1.ts seg2.ts` writes a playable file from local segments.

## Features: pay for what you use

Both are on by default. A page that only plays live HLS can leave them out
(`rstreamkit = { version = "...", default-features = false }`) and be a fraction of the size.

| Features | Adds | Whole page, compressed |
|---|---|---|
| none | live HLS and MPEG-TS, H.264 and AAC | 57 KB |
| `vod` | MP4 and Matroska movie files (`vod`, `mp4`, `mkv`, `mse::probe`, `mse::play_movie`) | 86 KB |
| `sound` | AC-3, E-AC-3 and MP2 sound decoded in Rust | 163 KB |
| `sound` + `vod` (default) | both | 185 KB |

Measured on a page that uses everything its features offer: built for `wasm32` with `opt-level = "z"` and
LTO, then `wasm-bindgen`, `wasm-opt -Oz` and brotli. Without `sound`, AC-3, E-AC-3 and MP2 are reported as
`Unsupported::Sound` (and a movie with such sound as `Verdict::Unsupported`) instead of played.

### Building a small page

For the page's release profile, `opt-level = "s"` with `lto = true`, `codegen-units = 1` and
`panic = "abort"`, then `wasm-bindgen` and `wasm-opt -Oz`. Measured on the page that uses everything
(AC-3 decoding at the same time): `"s"` is 3.6% smaller than `"z"` with the same speed, and `3` is 11%
faster but 23% bigger, so it isn't worth it. The sound decoders' speed is the decoders': about 55 ms
for six seconds of 5.1 AC-3, whatever the level.

## What it doesn't do

HEVC and other video codecs (only H.264 passes through), AES-128, fMP4 segments, adaptive bitrate.
Streams it can't handle are reported as typed values, not strings: `Status::Unsupported(Unsupported)` in
the browser glue (video that isn't H.264, sound it can't decode, interlaced pictures, a raw MPEG-TS
stream, a media type the browser refuses) and `Error::unsupported()` from the `Transmuxer`. What to do
about it is the app's policy (say so, play without the sound, hand the stream to real ffmpeg), and a
browser that can't decode a codec can't play it, whatever the library does.

## Test

```sh
cargo test
```

The integration tests transmux real segments and have `ffmpeg` decode the result, and compare the
decoded AC-3, E-AC-3 and MP2 sound with ffmpeg's own (skipped if `ffmpeg` isn't installed).

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or [MIT license](LICENSE-MIT) at
your option. Unless you say otherwise, any contribution you submit for inclusion is dual licensed as
above, with no additional terms.
