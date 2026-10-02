# rstreamkit

**rstreamkit helps developers play live streams and video files in apps and browsers.**
Its Rust engine reads HLS, MPEG-TS, MP4, and Matroska and prepares video and audio
for playback, with no FFmpeg needed. Browser playback uses a small JavaScript bridge.

| Comparison | rstreamkit | FFmpeg |
| --- | --- | --- |
| Put a six-second H.264/AAC stream into MP4 | **14.5 ms — 4.2× faster** | 60.5 ms |
| Output from that test | Same 180 video frames and audio | Same 180 video frames and audio |
| Engine language | Rust | [Mainly C](https://ffmpeg.org/developer.html) |
| Where it works | Apps, servers, and browsers | [Many platforms](https://ffmpeg.org/about.html), including apps and servers |
| Media support | Focused on streaming and playback | More formats, plus general video conversion |

Speed is the median of eight command-line runs on a Ryzen 7 5700 running Linux.
It includes startup and file reading/writing, with video and audio copied unchanged.
The speed advantage applies to this test; it does not prove every task is faster.

[Benchmark details and commands](docs/performance.md) ·
[Developer guide](docs/api.md) · [Demo and tests](tools/README.md)

Open source under [MIT](LICENSE-MIT) or [Apache 2.0](LICENSE-APACHE).
