# Contributing

rstreamkit is a standalone, open-source Rust toolkit for processing streams and files in native apps
and browsers. Browser playback is a platform adapter around the shared media core.
Apps are its clients; RIPTV is the first. Apps conform to rstreamkit, not the reverse.

## Rules

1. **No app-specific code.** Nothing in the library may know an app: no app names, proxy headers or
   URL schemes. Apps plug in through traits (`mse::Fetch` is the model) and own their policy, such as
   what to do with a stream the browser can't play. CI fails if an app name appears in the source.
2. **What an app needs arrives as an issue or pull request with a general use case.** "RIPTV needs X" is
   a reason to look, not a design.
3. **Public API changes start with an issue.** Breaking changes are called out in the pull request.
4. **Stay modular.** Heavy or optional capabilities are Cargo features (`sound`, `vod`), off-able
   without touching the rest; a new dependency has to earn its bytes (the wasm size of a page is a
   feature of the library). **Keep logic testable.** `mse` only compiles for the browser, so put everything that can run natively
   in the modules around it, with a test.
5. **Don't copy or translate ffmpeg's code.** It is LGPL/GPL; work from the specifications.
6. **Changes land by pull request.** The maintainer may push directly.

## Before you push

```sh
cargo fmt --check
for f in "" "--no-default-features" "--no-default-features --features sound" "--no-default-features --features vod"; do
  cargo clippy --all-targets $f -- -D warnings
  cargo clippy --target wasm32-unknown-unknown $f -- -D warnings
done
cargo test
cargo test --no-default-features
cargo test --no-default-features --features vod
```

CI runs the same, with ffmpeg installed so the tests that decode our output run instead of skipping.
Contributions are dual licensed under MIT OR Apache-2.0.
