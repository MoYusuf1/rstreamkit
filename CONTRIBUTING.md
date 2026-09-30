# Contributing

rstreamkit is a standalone, open-source toolkit for playing streams and files in the browser.
Apps are its clients; RIPTV is the first. Apps conform to rstreamkit, not the reverse.

## Rules

1. **No app-specific code.** Nothing in the library may know an app: no app names, proxy headers or
   URL schemes. Apps plug in through traits (`mse::Fetch` is the model) and own their policy, such as
   what to do with a stream the browser can't play. CI fails if an app name appears in the source.
2. **What an app needs arrives as an issue or pull request with a general use case.** "RIPTV needs X" is
   a reason to look, not a design.
3. **Public API changes start with an issue.** Breaking changes are called out in the pull request.
4. **Keep logic testable.** `mse` only compiles for the browser, so put everything that can run natively
   in the modules around it, with a test.
5. **Don't copy or translate ffmpeg's code.** It is LGPL/GPL; work from the specifications.
6. **Changes land by pull request.** The maintainer may push directly.

## Before you push

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo clippy --target wasm32-unknown-unknown -- -D warnings
cargo test
```

CI runs the same, with ffmpeg installed so the tests that decode our output run instead of skipping.
Contributions are dual licensed under MIT OR Apache-2.0.
