"""Bound each seeded parser campaign in a separate Linux process."""
import argparse
import hashlib
import json
from pathlib import Path
import resource
import subprocess
import time

ROOT = Path(__file__).resolve().parents[2]
HERE = Path(__file__).resolve().parent
RESULTS = ROOT / 'tools/results/fuzz'
TARGETS = ['hls', 'ts', 'mp4', 'mkv', 'cmaf']


def limits():
    resource.setrlimit(resource.RLIMIT_AS, (512 << 20, 512 << 20))
    resource.setrlimit(resource.RLIMIT_CPU, (30, 31))
    resource.setrlimit(resource.RLIMIT_CORE, (0, 0))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--cases', type=int, default=1000)
    parser.add_argument('--seed', type=int, default=20261010)
    parser.add_argument('--target', choices=TARGETS)
    args = parser.parse_args()
    if args.cases < 1 or not 0 < args.seed < 2**64:
        parser.error('cases must be positive and seed must be a nonzero u64')
    RESULTS.mkdir(parents=True, exist_ok=True)
    subprocess.run(['cargo', 'build', '--manifest-path', str(HERE / 'Cargo.toml'),
                    '--release', '--bin', 'smoke'], check=True, cwd=ROOT)
    binary = HERE / 'target/release/smoke'
    corpus = RESULTS / 'corpus'
    subprocess.run([str(binary), 'seed', str(corpus)], check=True, cwd=ROOT)
    report = {'seed': args.seed, 'cases_per_target': args.cases,
              'input_limit_bytes': 1 << 20, 'address_space_limit_bytes': 512 << 20,
              'cpu_limit_seconds': 30, 'wall_limit_seconds': 45,
              'rustc': subprocess.check_output(['rustc', '--version'], text=True).strip(),
              'binary_sha256': hashlib.sha256(binary.read_bytes()).hexdigest(),
              'revision': subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=ROOT, text=True).strip(),
              'cargo_lock': (HERE / 'Cargo.lock').read_text(),
              'source_hashes': {str(p.relative_to(ROOT)): hashlib.sha256(p.read_bytes()).hexdigest()
                                for p in sorted([*(ROOT / 'src').glob('*.rs'),
                                                 *(HERE / 'src').glob('*.rs'),
                                                 *(HERE / 'targets').glob('*.rs'),
                                                 HERE / 'Cargo.toml', HERE / 'run.py'])},
              'targets': []}
    failed = False
    for target in [args.target] if args.target else TARGETS:
        pending = RESULTS / f'{target}-pending.bin'
        command = [str(binary), target, str(args.cases), str(args.seed), str(corpus / target), str(pending)]
        log = RESULTS / f'{target}.log'
        start = time.monotonic()
        with log.open('w') as output:
            try:
                result = subprocess.run(command, cwd=ROOT, stdout=output, stderr=subprocess.STDOUT,
                                        preexec_fn=limits, timeout=45)
                code = result.returncode
            except subprocess.TimeoutExpired:
                code = 'wall-timeout'
        entry = {'target': target, 'exit': code, 'seconds': time.monotonic() - start,
                 'seeds': {p.name: hashlib.sha256(p.read_bytes()).hexdigest()
                           for p in sorted((corpus / target).iterdir())}}
        report['targets'].append(entry)
        failed |= code != 0
        print(f'{target}: exit={code}, {entry["seconds"]:.3f}s', flush=True)
    encoded = json.dumps(report, indent=2) + '\n'
    (RESULTS / 'report.json').write_text(encoded)
    (RESULTS / f'report-{args.seed}-{args.cases}.json').write_text(encoded)
    raise SystemExit(1 if failed else 0)


if __name__ == '__main__':
    main()
