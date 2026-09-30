# rffmpeg

The part of ffmpeg a live-TV player needs, in pure Rust: no C, no ffmpeg install, and small enough for
WebAssembly. It turns HLS + MPEG-TS into fragmented MP4 that a browser plays through MediaSource,
decoding the sound browsers can't (AC-3, E-AC-3, MP2) on the way.

| Module | Job |
|---|---|
| `hls` | Playlist parsing (media and master playlists, sliding windows) |
| `ts` | MPEG-TS demux: H.264 video, AAC (ADTS), AC-3, E-AC-3 and MPEG audio |
| `avc` | H.264 SPS: size, pixel aspect ratio (`pasp`), interlacing |
| `sound` | AC-3 / E-AC-3 / MP2 decoded to stereo PCM, written as FLAC frames |
| `fmp4` | fMP4 init segment and fragments (`avc1` + `mp4a` or `fLaC`) |
| `Transmuxer` (crate root) | Feeds segments in, gets an init segment and fragments out |
| `mse` (wasm only) | Fetches the playlist and segments and appends them to a `<video>` |

```rust
let mut t = rffmpeg::Transmuxer::default();
let out = t.push(&segment_bytes)?;      // out.init (once), out.fragment, out.skipped_audio
```

`cargo run --example transmux -- out.mp4 seg1.ts seg2.ts` writes a playable file from local segments.

## What it doesn't do

HEVC and other video codecs (only H.264 passes through), AES-128, fMP4 segments, adaptive bitrate.
Streams it can't handle are reported with a `convert:` marker (`needs_conversion`), so a caller can
hand them to real ffmpeg. [RIPTV](https://github.com/MoYusuf1/riptv) does exactly that.

## Test

```sh
cargo test
```

The integration tests transmux real segments and have `ffmpeg` decode the result, and compare the
decoded AC-3, E-AC-3 and MP2 sound with ffmpeg's own (skipped if `ffmpeg` isn't installed).
