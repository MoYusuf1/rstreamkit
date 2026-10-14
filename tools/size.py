#!/usr/bin/env python3
"""Build the same playback entry point with four feature sets and record bytes."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import tomllib

ROOT = Path(__file__).resolve().parents[1]
PLAYER = ROOT / "tools/player"
FLAVORS = {"live": [], "live-sound": ["sound"], "vod": ["vod"], "full": ["sound", "vod"]}


def run(*args, env=None):
    result = subprocess.run(args, cwd=ROOT, check=True, text=True, capture_output=True, env=env)
    if result.stderr:
        print(result.stderr, file=sys.stderr, end="")
    return result.stdout.strip()


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def main():
    profile = tomllib.loads((PLAYER / "Cargo.toml").read_text())["profile"]["release"]
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, default=ROOT / "tools/results/size.json")
    parser.add_argument("--check-budgets", action="store_true",
                        help="check engine and shared HTML Brotli budgets in tools/player/budgets.json")
    parser.add_argument("--opt-level", choices=["s", "z", "3"], default=str(profile["opt-level"]),
                        help="compare code-size and CPU tradeoffs; default matches the shipped release profile")
    parser.add_argument("--codec-opt-level", choices=["s", "z", "3"],
                        help="override AC-3, MP2 and codec-core packages separately")
    parser.add_argument("--mp2-opt-level", choices=["s", "z", "3"],
                        help="override MP2 independently after the shared codec setting")
    args = parser.parse_args()
    package_levels = {}
    if args.codec_opt_level:
        package_levels = {name: args.codec_opt_level for name in
                          ["rstreamkit-ac3", "oxideav-mp2", "oxideav-core"]}
    if args.mp2_opt_level:
        package_levels["oxideav-mp2"] = args.mp2_opt_level
    effective_levels = {name: str(settings["opt-level"]) for name, settings in
                        profile.get("package", {}).items() if "opt-level" in settings}
    effective_levels.update(package_levels)
    overrides = {"CARGO_PROFILE_RELEASE_OPT_LEVEL": args.opt_level,
                 "CARGO_PROFILE_RELEASE_LTO": "true",
                 "CARGO_PROFILE_RELEASE_CODEGEN_UNITS": "1",
                 "CARGO_PROFILE_RELEASE_PANIC": "abort"}
    cargo_env = {**os.environ, **overrides}
    report = {
        "schema": 2,
        "revision": run("git", "rev-parse", "HEAD"),
        "working_tree": run("git", "status", "--porcelain"),
        "tools": {name: run(name, "--version") for name in
                  ["rustc", "cargo", "wasm-bindgen", "wasm-opt", "brotli"]},
        "recipe": {"profile": f"release: opt-level={args.opt_level}, lto=true, codegen-units=1, panic=abort",
                   "cargo_profile_overrides": overrides,
                   "codec_opt_level": args.codec_opt_level,
                   "package_opt_levels": effective_levels,
                   "wasm_opt": "-Oz", "brotli_quality": 11,
                   "target": "wasm32-unknown-unknown", "entry": "tools/player/src/lib.rs"},
        "flavors": [],
    }
    shell = ROOT / "tools/web/player.html"
    shell_br = ROOT / "tools/results/player.html.br"
    shell_br.parent.mkdir(parents=True, exist_ok=True)
    run("brotli", "-f", "-q", "11", str(shell), "-o", str(shell_br))
    report["shared_startup_files"] = [{"file": "player.html", "bytes": shell.stat().st_size,
                                        "brotli_bytes": shell_br.stat().st_size,
                                        "sha256": digest(shell)}]
    for name, features in FLAVORS.items():
        command = ["cargo"]
        for package, level in package_levels.items():
            setting = level if level.isdigit() else f'"{level}"'
            command += ["--config", f'profile.release.package.{package}.opt-level={setting}']
        command += ["build", "--offline", "--manifest-path", str(PLAYER / "Cargo.toml"),
                   "--target", "wasm32-unknown-unknown", "--release", "--no-default-features"]
        if features:
            command += ["--features", ",".join(features)]
        run(*command, env=cargo_env)
        output = ROOT / "tools/web/player" / name
        output.mkdir(parents=True, exist_ok=True)
        run("wasm-bindgen", str(PLAYER / "target/wasm32-unknown-unknown/release/rstreamkit_player.wasm"),
            "--target", "web", "--out-dir", str(output))
        wasm = output / "rstreamkit_player_bg.wasm"
        wasm_before_opt_bytes = wasm.stat().st_size
        run("wasm-opt", str(wasm), "-Oz", "-o", str(wasm))
        files = []
        for path in [wasm, output / "rstreamkit_player.js"]:
            compressed = path.with_suffix(path.suffix + ".br")
            run("brotli", "-f", "-q", "11", str(path), "-o", str(compressed))
            files.append({"file": path.name, "bytes": path.stat().st_size,
                          "brotli_bytes": compressed.stat().st_size, "sha256": digest(path)})
        entry = {"name": name, "features": features, "files": files,
                 "wasm_before_opt_bytes": wasm_before_opt_bytes,
                 "startup_bytes": sum(f["bytes"] for f in files),
                 "startup_brotli_bytes": sum(f["brotli_bytes"] for f in files)}
        entry["complete_startup_bytes"] = entry["startup_bytes"] + shell.stat().st_size
        entry["complete_startup_brotli_bytes"] = entry["startup_brotli_bytes"] + shell_br.stat().st_size
        report["flavors"].append(entry)
        print(f'{name}: {entry["startup_brotli_bytes"]:,} Brotli bytes (wasm + JS)', flush=True)
    report["source_sha256"] = {
        str(p.relative_to(ROOT)): digest(p)
        for p in sorted([*ROOT.glob("src/**/*.rs"), *ROOT.glob("ac3/src/**/*.rs"),
                         *PLAYER.glob("src/**/*.rs"), ROOT / "Cargo.toml", ROOT / "ac3/Cargo.toml",
                         PLAYER / "Cargo.toml", PLAYER / "Cargo.lock", shell, Path(__file__).resolve()])
    }
    # Preserve the exact dependency resolution, even though generated Cargo.lock files are ignored.
    report["cargo_lock"] = (PLAYER / "Cargo.lock").read_text()
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(report, indent=2) + "\n")
    print(f"Report: {args.output}")
    if args.check_budgets:
        budgets = json.loads((PLAYER / "budgets.json").read_text())
        failures = [f'{f["name"]}: {f["startup_brotli_bytes"]:,} > {budgets[f["name"]]:,} bytes'
                    for f in report["flavors"] if f["startup_brotli_bytes"] > budgets[f["name"]]]
        shell_budget = budgets["player.html"]
        if shell_br.stat().st_size > shell_budget:
            failures.append(f'player.html: {shell_br.stat().st_size:,} > {shell_budget:,} bytes')
        if failures:
            raise SystemExit("Download budget exceeded:\n" + "\n".join(failures))
        print("All download budgets passed.")


if __name__ == "__main__":
    main()
