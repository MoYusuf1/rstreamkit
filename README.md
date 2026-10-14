# rstreamkit

Get live streams and video files ready to play in browsers, Rust apps, and the command line. rstreamkit keeps the picture unchanged and converts unsupported sound when needed. The engine is Rust and needs no FFmpeg at runtime.

## Compared with FFmpeg

Both tools did the same job: repackage video and sound without changing the picture.

| Job | rstreamkit | FFmpeg | Result |
| --- | ---: | ---: | --- |
| 6 seconds of HD video | 0.008 s | 0.072 s | **9× faster** |
| 10-minute HD recording (687 MB) | 0.33 s | 0.83 s | **2.5× faster** |
| 30-minute HD movie (2 GB) | 0.78 s | 1.44 s | **1.8× faster** |
| Memory for these jobs | 13–17 MB | 66–68 MB | **About 4–5× less** |
| Convert 30 minutes of Dolby sound | 8.4 s | 2.2 s | **FFmpeg is about 4× faster** |

Lower is better. Times include starting each program and reading and writing files.
Results are medians of 5–8 runs on a Ryzen 7 5700, Linux, FFmpeg 9.0.2. The repackaged picture and sound were checked against the inputs. These results cover these jobs; FFmpeg supports far more formats and can also convert video.

## Recent measurements

The 4K rows use the same 18-second clip at 40 Mbps. The sound and download rows measure separate changes:

| Measure | Before | After |
| --- | ---: | ---: |
| Rust/Wasm memory | 70.9 MB | **35.8–36.7 MB (about 49% less)** |
| Dropped frames | 0 of 1,080 | **0 of 1,080 in each run** |
| Full player download, compressed | 232,742 bytes | **233,328 bytes (+586)** |
| Process 2 seconds of MP2 sound in Wasm | 16.38 ms | **13.37 ms (18% less)** |

Memory excludes the browser, decoder and GPU. MP2 time is measured in Node. Download counts Wasm and JavaScript, with the same features. Stats formatting saved 766 bytes; audio scheduling and clock fixes added 1,352. Short checks do not prove long-run 4K performance. Chrome, Edge and Safari endurance tests are still planned.

## What you can use it for

- Play HLS live streams and MP4/MKV files, including seeking.
- Convert Dolby AC-3, E-AC-3 and MP2 sound when a browser needs it.
- Repackage files: `rstreamkit recording.ts recording.mp4`.
- Build a browser player or use the engine in a Rust app.

Your player must support the video codec. rstreamkit does not convert the picture. Browser builds use generated JavaScript glue and the browser’s media player.

[Measurements and repeat commands](docs/performance.md) · [Developer guide](docs/api.md) · [Demo and tests](tools/README.md) · [Roadmap](docs/roadmap.md) · [Contributor guide](docs/technical-breakdown.md)

Open source under [MIT](LICENSE-MIT) or [Apache 2.0](LICENSE-APACHE).

The Dolby (AC-3 and E-AC-3) decoder in [ac3/](ac3/README.md) is
[oxideav-ac3](https://github.com/OxideAV/oxideav-ac3) by Mark Karpeles, used under its MIT licence
and made faster here. Thank you.
