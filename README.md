# rstreamkit

**Get live streams and video files ready to play, quickly, without FFmpeg: in a web page, in your
own program, or from the command line.**

Videos and live streams come in many formats, and players (web browsers especially) only take some
of them. rstreamkit repackages the rest into a form they can play, without touching the picture. It
can also turn sound that browsers can't play (such as Dolby AC-3) into sound they can.

It is written in Rust, is free to use and change, and doesn't need FFmpeg installed.

## Is it faster than FFmpeg?

[FFmpeg](https://ffmpeg.org) is the best-known tool for working with video. We gave both programs
the same jobs: repackage HD video and its sound for streaming, without changing a single picture.

| Job | rstreamkit | FFmpeg | Result |
| --- | ---: | ---: | --- |
| 6 seconds of HD video | 0.008 s | 0.072 s | **9× faster** |
| A 10-minute HD recording (687 MB) | 0.33 s | 0.83 s | **2.5× faster** |
| A 30-minute HD movie (2 GB) | 0.78 s | 1.44 s | **1.8× faster** |
| Memory used, on any of these | 13 to 17 MB | 66 to 68 MB | **4 to 5 times less** |

*Lower is better. s means seconds.*

**Why you can trust the numbers**

- Same files, same jobs. Each program was started fresh every time, so the times include starting
  up and reading and writing the files.
- Every time is the middle result of five to eight timed runs.
- FFmpeg can also report how long its work took without its start-up. rstreamkit's *whole* run is
  still faster than that, on every one of these jobs.
- We checked the results. Every picture came out identical from both programs (53,666 of them in
  the movie), and so did all of the sound.
- Measured on an AMD Ryzen 7 5700 running Linux, with FFmpeg 9.0.2. A script repeats all of it
  (see the benchmark details).

**What this does not mean.** It doesn't mean rstreamkit is faster at everything. FFmpeg can convert
almost any format into any other; rstreamkit does one narrow job, repackaging for playback. When
Dolby sound has to be converted, FFmpeg is faster today: 30 minutes of 5.1 sound take 2.2 seconds in
FFmpeg and 8.4 seconds here (it was 14 before our decoder got twice as fast). That's still far faster
than real time.

## What it can do

- Play live streams (HLS, the format most live TV and radio streams use).
- Play movie files (MP4 and MKV), including jumping to any point.
- Make sound playable that a browser can't play by itself.
- Convert files from the command line: `rstreamkit recording.ts recording.mp4`.
- Run inside a browser page, or inside your own Rust program.

It does not convert video that a player can't show (such as HEVC on browsers without support). It
says so clearly, and your app decides what to do.

## Learn more

[Benchmark details and commands](docs/performance.md) ·
[Developer guide](docs/api.md) ·
[Technical breakdown for contributors and AI agents](docs/technical-breakdown.md) ·
[Demo and tests](tools/README.md) · [Roadmap](docs/roadmap.md)

Open source under [MIT](LICENSE-MIT) or [Apache 2.0](LICENSE-APACHE).

The Dolby (AC-3 and E-AC-3) decoder in [ac3/](ac3/README.md) is
[oxideav-ac3](https://github.com/OxideAV/oxideav-ac3) by Mark Karpeles, used under its MIT licence
and made faster here. Thank you.
