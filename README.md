# rstreamkit

**Play live streams and video files in a web page, quickly, without FFmpeg.**

Videos and live streams come in many formats, and web browsers only play some of them directly.
rstreamkit quietly converts the rest into something the browser can play, while you watch. It can
also turn sound that browsers can't play (such as Dolby AC-3) into sound they can.

It is written in Rust, runs inside the web page itself, and is free to use and change.

## Is it faster than FFmpeg?

[FFmpeg](https://ffmpeg.org) is the best-known tool for working with video. We gave both programs
the same job: repackage six seconds of HD video and sound so a browser can play it.

| Test | rstreamkit | FFmpeg | Result |
| --- | ---: | ---: | --- |
| First measurement | 14.5 ms | 60.5 ms | **4.2× faster** |
| Repeated later, on a fresh copy of the code | 17.8 ms | 73.4 ms | **4.1× faster** |

*ms means thousandths of a second. Lower is better.*

**Why you can trust the numbers**

- Same file, same job. Each program was started fresh, so the times include its start-up and the
  reading and writing of the file.
- Every time is the middle result of eight timed runs. The repeat was itself run three times, and
  stayed between 4.1× and 4.2× faster.
- We checked the results. All 180 video frames and all of the sound came out identical from both
  programs.
- Measured on an AMD Ryzen 7 5700 running Linux, with FFmpeg 9.0.2.

**What this does not mean.** It does not say rstreamkit is faster at everything. FFmpeg can convert
almost any format into any other; rstreamkit does one narrow job, repackaging for playback, which is
what it is built for. Part of FFmpeg's time here is start-up and inspecting the file, which this
test counts.

## What it can do

- Play live streams (HLS, the format most live TV and radio streams use).
- Play movie files (MP4 and MKV), including jumping to any point.
- Make sound playable that the browser can't play by itself.
- Run inside a browser page, or inside a regular Rust program.

It does not convert video the browser can't show (such as HEVC on browsers without support). It
tells your app clearly, and your app decides what to do.

## Learn more

[Benchmark details and commands](docs/performance.md) ·
[Developer guide](docs/api.md) ·
[Technical breakdown for contributors and AI agents](docs/technical-breakdown.md) ·
[Demo and tests](tools/README.md) · [Roadmap](docs/roadmap.md)

Open source under [MIT](LICENSE-MIT) or [Apache 2.0](LICENSE-APACHE).
