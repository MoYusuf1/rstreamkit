# Bounded parser fuzzing

Run from the repo root on Linux:

```sh
python3 tools/fuzz/run.py
python3 tools/fuzz/run.py --cases 10000 --seed 24681357
```

The first command runs 1,000 cases for each of 5 targets. The second runs 50,000
cases total. The Rust harness mutates real fixtures with bit flips, truncation,
insertions and changed 64-bit fields. Seeds and input order are fixed. This is
deterministic mutation testing, not coverage-guided fuzzing.

| Limit | Seeded run, per target | Coverage-guided run below |
| --- | ---: | ---: |
| Input bytes | 1 MiB | 1 MiB |
| Memory | 512 MiB address space | 512 MiB RSS limit |
| CPU time | 30 seconds per campaign | Not set |
| Wall time | 45 seconds per campaign | 5 seconds per input; 20 minutes per campaign |
| MKV range requests | At most 32 per input | At most 32 per input |

The OS limits apply to each seeded child, not Cargo compilation. The parent kills
a campaign that exceeds the wall limit. An allocation failure or killed child
fails the run. These are harness budgets, not library guarantees for every host.
Audio decoding is off: this campaign targets parsing and indexing, not the sound
decoder. Release builds keep overflow checks and debug assertions enabled.

Reports, source/input/binary hashes, resolved dependencies, logs and the current
input are under `tools/results/fuzz/`. A failure leaves `TARGET-pending.bin`.
Replay it before running another campaign of that target:

```sh
tools/fuzz/target/release/smoke replay mp4 tools/results/fuzz/mp4-pending.bin
```

Preserve useful findings as small normal regression tests. The corpus includes
valid HLS, TS, MP4 sample tables, an MKV with cues, and generated CMAF init/media.
MP4 checks also use extreme absolute offsets and seek times. CMAF checks failed
rebasing leaves input bytes unchanged, plus invalid fallback times and repeated
initialization tracks. Matroska follows bounded probe reads and exercises cursors.

For coverage-guided campaigns, install nightly Rust and cargo-fuzz, then:

```sh
cargo install cargo-fuzz --version 0.13.2 --locked
python3 tools/fuzz/run.py --cases 1
cargo +nightly fuzz run --fuzz-dir tools/fuzz --features coverage mp4 tools/results/fuzz/corpus/mp4 -- -seed=20261010 -max_len=1048576 -rss_limit_mb=512 -timeout=5 -max_total_time=1200
```

Repeat with `hls`, `ts`, `mkv` or `cmaf` and its corpus directory. Use
`-runs=10000 -max_total_time=30` for a bounded CI smoke. The five targets share
the same parser calls as the deterministic runner. libFuzzer's RSS monitoring is
not a hard OS address-space limit; ASan needs a much larger virtual address space.
See the [Rust Fuzz Book](https://rust-fuzz.github.io/book/cargo-fuzz/guide.html).

The harness is a separate unpublished crate. It adds **0 runtime dependencies**
and is outside the default `cargo test` path. Passing these campaigns is evidence
about the tested corpus, not proof that all malformed inputs are safe.

## Results — 2026-10-10

Linux/Ryzen 7 5700, Rust 1.99.0. Both final seeded campaigns passed:

| Check | Before | After |
| --- | ---: | ---: |
| Seeded parser targets | 0 | 5 |
| Cases in the smoke campaign, seed 20261010 | Not available | 5,000 passed |
| Cases in the larger campaign, seed 24681357 | Not available | 50,000 passed |
| Overflow panics in the initial run | 2 | 0 in the final campaigns |
| Focused regression tests for these findings | 0 | 4 |
| Added library runtime dependencies | 0 | 0 |

Findings: MP4 absolute offsets could overflow; the smallest signed MKV seek time
could underflow; oversized MP4 timing runs and edit counts could do excessive
work. Offset arithmetic is now checked, negative seeks clamp safely, timing runs
must fit the actual samples, and impossible edit counts use the existing zero
shift fallback. Normal fixture playback and independent decoder tests pass.

The coverage-guided targets compile and lint locally, but their nightly/ASan
campaigns have not run here. CI uses the stable seeded runner; its remote result
must be checked after publishing. The sound decoder and browser endurance are
separate work. No FFmpeg speed comparison is made by these results.
