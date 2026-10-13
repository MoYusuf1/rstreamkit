#!/usr/bin/env python3
"""Independent exact suffix-sum oracle; Python is a development tool only."""
import argparse
import decimal
from pathlib import Path
import random
import struct

def generate():
    rng = random.Random(20261013)
    decimal.getcontext().prec = 1600
    def value(bits):
        return struct.unpack('>d', struct.pack('>Q', bits))[0]
    def bits(number):
        return struct.unpack('>Q', struct.pack('>d', number))[0]
    cases = [
        [1e308, 1e308, .25], [1e308, -1e308, .25], [-1e308, 1e308, -.25],
        [1.0, 2**-53, 2**-54], [1.0, 2**-52, 2**-53],
        [value(1), value(2), -value(1)], [value(0x0010000000000000), -value(1)],
        [value(0x7fefffffffffffff), 2**970], [-0.0, 0.0],
        [2**100, 2**-26], [2**101, 2**-26], [value(1), value(2), value(3)],
    ]
    cases += [[1.1] * 17, [value(0x7fefffffffffffff)] * 3, [value(1)] * 7, [4.589889] * 100]
    for index in range(64):
        mask = (1 << 64) - 1 if index % 2 else (1 << 63) - 1
        cases.append([value((rng.getrandbits(64) & mask & ~(0x7ff << 52)) |
                            (rng.randrange(2047) << 52)) for _ in range(16)])
    for _ in range(32):
        cases.append([rng.uniform(.125, 128.0) for _ in range(32)])
    lines = ['# Exact binary64 suffix sums, Python Decimal precision=1600; seed=20261013.']
    for values in cases:
        exact = [decimal.Decimal.from_float(float(v)) for v in values]
        suffixes = [bits(float(sum(exact[i:], decimal.Decimal(0))))
                    for i in range(len(values) + 1)]
        lines.append(' '.join(f'{bits(v):016x}' for v in values) + '|' +
                     ' '.join(f'{v:016x}' for v in suffixes))
    return '\n'.join(lines) + '\n'

if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--check', action='store_true')
    args = parser.parse_args()
    target = Path(__file__).resolve().parents[1] / 'tests/fixtures/duration_sums.txt'
    result = generate()
    if args.check:
        if target.read_text() != result:
            raise SystemExit('duration vectors differ; regenerate with tools/duration_vectors.py')
    else:
        target.write_text(result)
    print(f'{len(result.splitlines()) - 1} duration vectors verified')
