#!/usr/bin/env python3
"""Build comparable development-only Node harnesses; never changes player defaults."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess

ROOT = Path(__file__).resolve().parents[1]
PACKAGES = ["rstreamkit-ac3", "oxideav-mp2", "oxideav-core"]
PROFILES = {
    "s": ("s", {}), "z": ("z", {}), "3": ("3", {}),
    "lean": ("z", dict.fromkeys(PACKAGES, "s")),
    "fast-codecs": ("z", dict.fromkeys(PACKAGES, "3")),
    "balanced": ("z", {**dict.fromkeys(PACKAGES, "s"), "oxideav-mp2": "3"}),
}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--profile", choices=PROFILES, action="append",
                        help="repeat to build several; default builds s and balanced")
    parser.add_argument("--output", type=Path, default=ROOT / "tools/results/profiles")
    parser.add_argument("--retain-names", action="store_true",
                        help="keep development-only Wasm function names for CPU sampling")
    parser.add_argument("--simd", action="store_true", help="experiment with Wasm SIMD128; not a shipping default")
    args = parser.parse_args()
    for name in args.profile or ["s", "balanced"]:
        level, packages = PROFILES[name]
        overrides = {"CARGO_PROFILE_RELEASE_OPT_LEVEL": level,
                     "CARGO_PROFILE_RELEASE_LTO": "true", "CARGO_PROFILE_RELEASE_CODEGEN_UNITS": "1",
                     "CARGO_PROFILE_RELEASE_PANIC": "abort"}
        if args.simd:
            flags = os.environ.get("CARGO_ENCODED_RUSTFLAGS", "\x1f".join(os.environ.get("RUSTFLAGS", "").split()))
            overrides["CARGO_ENCODED_RUSTFLAGS"] = flags + ("\x1f" if flags else "") + "-C\x1ftarget-feature=+simd128"
        command = ["cargo"]
        for package, value in packages.items():
            setting = value if value.isdigit() else f'"{value}"'
            command += ["--config", f'profile.release.package.{package}.opt-level={setting}']
        command += ["build", "--offline", "--manifest-path", "tools/harness/Cargo.toml",
                    "--target", "wasm32-unknown-unknown", "--release"]
        subprocess.run(command, cwd=ROOT, env={**os.environ, **overrides}, check=True)
        output = args.output.resolve() / name
        output.mkdir(parents=True, exist_ok=True)
        subprocess.run(["wasm-bindgen", str(ROOT / "tools/harness/target/wasm32-unknown-unknown/release/rstreamkit_harness.wasm"),
                        "--target", "nodejs", "--out-dir", str(output)], check=True)
        wasm = output / "rstreamkit_harness_bg.wasm"
        wasm_opt = ["-Oz"] + (["--enable-simd"] if args.simd else []) + (["-g"] if args.retain_names else [])
        subprocess.run(["wasm-opt", str(wasm), *wasm_opt, "-o", str(wasm)], check=True)
        tools = {tool: subprocess.check_output([tool, "--version"], text=True).strip()
                 for tool in ["rustc", "cargo", "wasm-bindgen", "wasm-opt"]}
        report = {"profile": name, "overrides": overrides, "packages": packages, "tools": tools,
                  "wasm_opt": wasm_opt, "retain_names": args.retain_names, "simd128": args.simd,
                  "effective_rustflags": overrides.get("CARGO_ENCODED_RUSTFLAGS", os.environ.get("CARGO_ENCODED_RUSTFLAGS", os.environ.get("RUSTFLAGS", ""))),
                  "revision": subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip(),
                  "wasm_sha256": hashlib.sha256(wasm.read_bytes()).hexdigest(),
                  "working_tree": subprocess.check_output(["git", "status", "--porcelain"], cwd=ROOT, text=True),
                  "source_sha256": {str(p.relative_to(ROOT)): hashlib.sha256(p.read_bytes()).hexdigest()
                                    for p in sorted([*ROOT.glob("src/**/*.rs"), *ROOT.glob("ac3/src/**/*.rs"),
                                                     *ROOT.glob("tools/harness/src/**/*.rs"),
                                                     ROOT / "Cargo.toml", ROOT / "ac3/Cargo.toml",
                                                     ROOT / "tools/harness/Cargo.toml", Path(__file__).resolve()])},
                  "cargo_lock": (ROOT / "tools/harness/Cargo.lock").read_text()}
        (output / "build.json").write_text(json.dumps(report, indent=2) + "\n")
        print(f"{name}: {wasm}", flush=True)


if __name__ == "__main__":
    main()
