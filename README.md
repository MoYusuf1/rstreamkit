# rstreamkit

**A Rust toolkit for streaming video and audio in apps, servers, and browsers.**
It handles HLS, MPEG-TS, MP4, and Matroska. The media engine is all Rust;
browser playback is optional and uses a small JavaScript bridge.

| Comparison | rstreamkit | FFmpeg | What this means |
| --- | --- | --- | --- |
| Put a six-second H.264/AAC stream into an MP4 file | **14.5 ms** | **60.5 ms** | **4.2× faster in this measured test** |
| Picture and sound from that test | 180 video frames; identical audio | 180 video frames; identical audio | The speed gain preserves the picture and sound |
| Media engine | Rust | [Mainly C](https://ffmpeg.org/developer.html) | Fits a Rust-only media stack |
| FFmpeg runtime dependencies | **0** | Uses the FFmpeg engine | No FFmpeg library or installation needed by rstreamkit |
| Where it runs | Native apps and browsers | [Many platforms](https://ffmpeg.org/about.html) | Both work outside browsers |
| Format coverage | Focused streaming formats | Broader coverage | FFmpeg still handles more kinds of media |

Speed: median of eight native command-line runs on a Ryzen 7 5700, Linux,
Rust 1.98.1, FFmpeg 9.0.2. Includes startup and file I/O; no re-encoding.
This test does not prove rstreamkit is faster at every job.

[Benchmark details and commands](docs/performance.md) ·
[Developer guide](docs/api.md) · [Demo and tests](tools/README.md)

Open source under [MIT](LICENSE-MIT) or [Apache 2.0](LICENSE-APACHE).
