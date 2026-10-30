# rstreamkit

rstreamkit gets TV streams and movie files ready to play: in a web page, in an app, or from the
command line. It does a slice of FFmpeg's job (repackaging, not re-encoding), but faster, lighter
and without installing FFmpeg.

## What you get

**Faster and lighter than FFmpeg**

- Repackages a short HD clip **9× faster** (0.008 s vs 0.072 s).
- Repackages a 10-minute HD recording **2.5× faster**, and a 2 GB movie **1.8× faster**.
- Uses **about 4–5× less memory** (13–17 MB vs 66–68 MB).
- Runs inside a web page as a small download (about 235 KB compressed). Nothing to install.
- The picture is never re-encoded, so quality stays exactly the same.

**Sound that browsers can't play**

- Turns Dolby sound (AC-3, E-AC-3) and MP2 into sound any browser plays, right in the page.
- Gets stereo and mono right, including mono Dolby sound, which used to come out silent.

**Live TV that keeps going**

- If the stream drops for a few seconds, it reconnects by itself and carries on.
- If a piece of the stream is missing or broken, it skips it instead of stopping.
- If the server takes the request and never answers, it gives up after 20 seconds and tries again,
  instead of freezing forever. Slow servers that do answer still work.
- Pause for a long time, press play, and it catches back up to live.
- Switch channels quickly and often without old channels leaking in.
- Picks a lower quality when the connection is slow and goes back up when it recovers.
- Keeps its memory use capped during long watches.

**Movies and episodes**

- Starts playing while it downloads. No waiting for the whole file.
- Seeking downloads only the part you jump to.
- Plays MKV files without an index, and ones with packed sound.
- Starts MKV movies a step sooner.
- Describes HEVC, AV1 and VP9 video accurately, so the browser gives the right "can I play this?"
  answer.

**When something goes wrong, it says so**

- It reports what it is doing (buffering, reconnecting, playing) instead of going quiet.
- A movie that can't start fails within seconds, with a reason and the file's tracks
  (for example "MKV, H.264 1920x1080, Dolby 5.1"), so nobody has to inspect the file by hand.

## Where FFmpeg is still better

- It converts Dolby sound about **4× faster** when saving a file.
- It reads far more formats and can change the video itself. rstreamkit only repackages video,
  so the player has to support the video type.

## How the speed was measured

| Job | rstreamkit | FFmpeg |
| --- | ---: | ---: |
| 6 seconds of HD video | 0.008 s | 0.072 s |
| 10-minute HD recording (687 MB) | 0.33 s | 0.83 s |
| 30-minute HD movie (2 GB) | 0.78 s | 1.44 s |
| Memory for these jobs | 13–17 MB | 66–68 MB |
| Convert 30 minutes of Dolby sound | 8.4 s | 2.2 s |

Lower is better. Medians of 5–8 runs on a Ryzen 7 5700, Linux, FFmpeg 9.0.2, including start-up
and file reading and writing. The output was checked against the input.

## Use it

- Repackage a file: `rstreamkit recording.ts recording.mp4`.
- Build a browser player, or use the engine in a Rust app.

[All measurements](docs/performance.md) · [Developer guide](docs/api.md) · [Demo and tests](tools/README.md) · [Roadmap](docs/roadmap.md) · [Contributor guide](docs/technical-breakdown.md)

Open source under [MIT](LICENSE-MIT) or [Apache 2.0](LICENSE-APACHE).

The Dolby (AC-3 and E-AC-3) decoder in [ac3/](ac3/README.md) is
[oxideav-ac3](https://github.com/OxideAV/oxideav-ac3) by Mark Karpeles, used under its MIT licence
and made faster here. Thank you.
