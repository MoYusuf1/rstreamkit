#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
cargo build --manifest-path tools/harness/Cargo.toml --target wasm32-unknown-unknown --release
wasm-bindgen tools/harness/target/wasm32-unknown-unknown/release/rstreamkit_harness.wasm --target web --out-dir tools/web/pkg
wasm-bindgen tools/harness/target/wasm32-unknown-unknown/release/rstreamkit_harness.wasm --target nodejs --out-dir tools/node/pkg
