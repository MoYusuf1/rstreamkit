#!/usr/bin/env python3
"""The rstreamkit command against FFmpeg on the same jobs, with both outputs checked.

Each tool runs as a fresh process: one warm-up, then timed runs in alternating order. Per tool:
median and maximum wall time, peak memory (max RSS) and output size. FFmpeg also reports its own
`-benchmark` rtime, which leaves out its start-up and input probing: ours is never given that
discount, so a win against it is a win on processing alone.

Checks, on the warm-up outputs: every decoded picture identical (framecrc); copied sound
identical sample for sample; decoded sound (different decoders) decodes cleanly to the same length.

Build and generate first:
  cargo build --release --bin rstreamkit && python3 tools/media.py
Run one workload with: python3 tools/compare.py "10 min"
"""
import hashlib, json, os, platform, re, statistics, subprocess, sys, tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
MEDIA = ROOT / 'tools/media'
OURS = ROOT / 'target/release/rstreamkit'
PEAK = ROOT / 'target/peak'
FRAGMENTED = ['-movflags', 'frag_keyframe+empty_moov', '-f', 'mp4']
COPY = ['-c', 'copy']
ADTS = ['-bsf:a', 'aac_adtstoasc']
# FLAC compression level 0 uses fixed predictors, as ours does; stereo, 16-bit, like ours.
TO_FLAC = ['-c:v', 'copy', '-c:a', 'flac', '-compression_level', '0', '-ac', '2', '-sample_fmt', 's16']
# name, input, FFmpeg options, whether the sound is copied (else decoded), timed runs
WORKLOADS = [
    ('6 s 1080p TS, H.264/AAC, copied', 'aac.ts', COPY + ADTS, True, 8),
    ('10 min 1080p TS, H.264/AAC, copied', 'long.ts', COPY + ADTS, True, 5),
    ('30 min 1080p MP4, H.264/AAC, copied', 'movie-aac.mp4', COPY, True, 5),
    ('6 s 1080p TS, AC-3 5.1 decoded to stereo FLAC', 'ac3.ts', TO_FLAC, False, 8),
    ('30 min MKV, AC-3 5.1 decoded to stereo FLAC', 'movie-51.mkv', TO_FLAC, False, 3),
]


def run(command):
    """Wall time (ms), peak resident memory (bytes) and stderr of one fresh process, measured by
    tools/peak.rs: spawned from Python directly, a program's peak would include Python's memory."""
    result = subprocess.run([str(PEAK), *command], capture_output=True, text=True)
    if result.returncode:
        raise RuntimeError(f'{command[0]} failed on {command}: {result.stderr}')
    measured = json.loads(result.stdout)
    return measured['ms'], measured['peak_bytes'], result.stderr


def decode(path, *options):
    result = subprocess.run(['ffmpeg', '-v', 'error', '-i', str(path), *options], capture_output=True, check=True)
    if result.stderr:
        raise RuntimeError(f'{path}: {result.stderr.decode()}')
    return result.stdout


def pictures(path):
    lines = decode(path, '-map', '0:v:0', '-f', 'framecrc', '-').decode().splitlines()
    return [line.rsplit(',', 1)[1].strip() for line in lines if line and not line.startswith('#')]


def check(ours, theirs, copied):
    a, b = pictures(ours), pictures(theirs)
    if not a or a != b:
        raise RuntimeError(f'pictures differ: {len(a)} against {len(b)}')
    sound = [decode(path, '-map', '0:a:0', '-f', 's16le', '-') for path in (ours, theirs)]
    if copied and sound[0] != sound[1]:
        raise RuntimeError('copied sound differs')
    # Decoders and downmixes differ; both must decode in full, to within one AC-3 frame.
    if not copied and abs(len(sound[0]) - len(sound[1])) > 1536 * 2 * 2:
        raise RuntimeError(f'decoded sound lengths differ: {len(sound[0])} against {len(sound[1])}')
    return {'decoded_pictures': len(a), 'decoded_sound_bytes': len(sound[0]), 'sound_identical': sound[0] == sound[1]}


def sha256(path):
    digest = hashlib.sha256()
    with open(path, 'rb') as f:
        while chunk := f.read(1 << 24):
            digest.update(chunk)
    return digest.hexdigest()


def compare(name, file, options, copied, runs, directory):
    source = MEDIA / file
    outputs = {'rstreamkit': directory / 'ours.mp4', 'ffmpeg': directory / 'theirs.mp4'}
    commands = {
        'rstreamkit': [str(OURS), str(source), str(outputs['rstreamkit'])],
        'ffmpeg': ['ffmpeg', '-hide_banner', '-nostats', '-benchmark', '-y', '-i', str(source),
                   '-map', '0:v:0', '-map', '0:a:0', *options, *FRAGMENTED, str(outputs['ffmpeg'])],
    }
    times = {tool: [] for tool in commands}
    memory = {tool: [] for tool in commands}
    processing = []
    for trial in range(runs + 1):
        for tool in (list(commands) if trial % 2 == 0 else list(reversed(commands))):
            elapsed, peak, errors = run(commands[tool])
            if trial == 0:
                continue
            times[tool].append(elapsed)
            memory[tool].append(peak)
            if tool == 'ffmpeg':
                processing.append(float(re.search(r'rtime=([\d.]+)s', errors).group(1)) * 1000)
        if trial == 0:
            checked = check(outputs['rstreamkit'], outputs['ffmpeg'], copied)
    result = {tool: {'median_ms': round(statistics.median(t), 2), 'max_ms': round(max(t), 2),
                     'peak_memory_bytes': max(memory[tool]), 'output_bytes': outputs[tool].stat().st_size}
              for tool, t in times.items()}
    result['ffmpeg']['benchmark_rtime_median_ms'] = round(statistics.median(processing), 2)
    for path in outputs.values():
        path.unlink()
    return {'workload': name, 'input': file, 'input_bytes': source.stat().st_size,
            'input_sha256': sha256(source), 'timed_runs': runs, 'check': checked,
            'speedup_wall': round(result['ffmpeg']['median_ms'] / result['rstreamkit']['median_ms'], 2),
            'speedup_vs_ffmpeg_processing_only': round(
                result['ffmpeg']['benchmark_rtime_median_ms'] / result['rstreamkit']['median_ms'], 2),
            'results': result}


def main():
    wanted = sys.argv[1] if len(sys.argv) > 1 else ''
    subprocess.run(['rustc', '-O', '--edition', '2024', str(ROOT / 'tools/peak.rs'), '-o', str(PEAK)], check=True)
    cpu = next((l.split(':', 1)[1].strip() for l in open('/proc/cpuinfo') if l.startswith('model name')), '?')
    ffmpeg = subprocess.run(['ffmpeg', '-version'], capture_output=True, text=True).stdout.splitlines()[0]
    print(json.dumps({'platform': platform.platform(), 'cpu': cpu, 'threads': os.cpu_count(), 'ffmpeg': ffmpeg,
                      'rstreamkit': str(OURS), 'note': 'wall times include process start-up and file I/O'}))
    # Outputs go to a RAM-backed directory when there is one, so the disk is not what is measured.
    base = '/dev/shm' if Path('/dev/shm').is_dir() else None
    with tempfile.TemporaryDirectory(prefix='rtk-compare-', dir=base) as directory:
        for workload in WORKLOADS:
            if wanted in workload[0]:
                print(json.dumps(compare(*workload, Path(directory))), flush=True)


if __name__ == '__main__':
    main()
