# rstreamkit-ac3

The AC-3 (Dolby Digital) and E-AC-3 (Dolby Digital Plus) decoder behind rstreamkit's `sound`
feature.

## Credit

This decoder is **[oxideav-ac3](https://github.com/OxideAV/oxideav-ac3) 0.0.11 by Mark Karpeles**
(copyright Karpelès Lab Inc.), a pure-Rust decoder written from the ATSC A/52 and ETSI TS 102 366
specifications. It is used here under its MIT licence, kept unchanged in [LICENSE](LICENSE), and
those terms still apply to this directory. Thank you to its author: rstreamkit plays Dolby sound in
browsers because of this work.

rstreamkit keeps its own copy so it can tune the decoder for its use. The changes are below. Every
speed change is measured and preserves oxideav-ac3 0.0.11 output. The mono
routing correction below intentionally restores sound that the original made silent.

## What changed from oxideav-ac3 0.0.11

The library's name is `rstreamkit_ac3` (it was `oxideav_ac3`). The examples, changelog, design
notes and CI configuration were left out; the source and the test suite are complete.

Speed. Six seconds of 5.1 AC-3 used to take 40.3 ms to decode to stereo and now take 20.0 ms
(AMD Ryzen 7 5700; the best of ten runs, in-process):

| Change | Where | Why it was slow |
| --- | --- | --- |
| The IMDCT's pre/post twiddles are built once, in a `OnceLock` | `imdct.rs` | They were rebuilt on every call: 128 cosines and 128 sines, six channels times six blocks a frame. |
| `2^-e` is built from its IEEE-754 exponent bits | `audblk.rs` (`pow2_neg`) | `2f32.powi(-e)` ran a loop of multiplications for every coefficient. The values are exactly the same. |
| The inverse FFT takes its twiddles and bit reversal from a table built once per size | `imdct.rs` (`IfftPlan`) | It recomputed both in every group of every stage. The table replays the same recurrence, so every product is unchanged. |
| The downmix works out each output's sources and weights once per call | `downmix.rs` | It re-decided, for every sample and every output, which sources exist and which weights are zero. Same products, same order of additions. |
| Debug-trace environment variables are read once | `lib.rs` (`trace_var`) | About ten `std::env::var` lookups ran per block. |

Results on the files it was measured with:

| Input | Before | After |
| --- | ---: | ---: |
| 6 s, AC-3 5.1, 448 kbit/s | 40.3 ms | 20.0 ms |
| 4.2 s of programme audio, AC-3 | 10.4 ms | 5.9 ms |
| 4 s, AC-3 mono (historical silent output; invalid audio-speed comparison) | 5.75 ms | 2.74 ms |
| 2 s, AC-3 5.1 | 9.08 ms | 3.59 ms |
| 2 s, E-AC-3 5.1 | 9.70 ms | 4.36 ms |

The cost is a little size: a browser page built with rstreamkit grows by 6.6 KB brotli-compressed
(+2.1%), and the native command by 16.9 KB (+1.1%).

The speed changes preserve original decoded PCM. The 4 s `h264_ac3.ts` fixture
is mono (the earlier table called it stereo); its original pinned output was
768,000 zero PCM bytes. The mono routing fix intentionally changes that output.
Stereo, 5.1 AC-3 and E-AC-3 retain their existing output checks. The checksum test
now pins corrected mono output as well, and an independent FFmpeg test covers it.

Mono routing: LoRo and LtRt now place their existing mono gains in the center
slot, matching the source mapping used by `Downmix::apply`. Positive and negative
center samples reach both outputs. This fixes silent mono without adding a
runtime dependency or changing other channel layouts.

Run the decoder's own tests with:

```sh
cargo test --release --manifest-path ac3/Cargo.toml
```


Optional Wasm SIMD FFT: with `simd128` enabled at compile time, two independent
butterflies run together with the same multiply/add/subtract order. A private
checked layout enables bounded vector loads/stores; default and native builds
keep the scalar algorithm. The measured long/short transforms take about 29%/25%
less time in warm Node tests, with 2,564 complete finite-output comparisons.
Complete audio processing gains are smaller. See `../docs/performance.md` for
CPU, bytes, platform limits and repeat commands. Original MIT credit remains above.
