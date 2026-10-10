#!/usr/bin/env python3
"""Generate a short, repeatable 4K60 SDR HLS stress fixture, not a quality sample."""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess

ROOT = Path(__file__).resolve().parent


def sha256(path):
    with path.open("rb") as source:
        return hashlib.file_digest(source, "sha256").hexdigest()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--mbps", type=int, choices=[15, 25, 40], default=40)
    args = parser.parse_args()
    media = ROOT / "media"
    media.mkdir(exist_ok=True)
    name = f"4k60-{args.mbps}m"
    playlist = media / f"{name}.m3u8"
    command = ["ffmpeg", "-hide_banner", "-loglevel", "error", "-y",
               "-f", "lavfi", "-i", "testsrc2=size=3840x2160:rate=60",
               "-f", "lavfi", "-i", "sine=sample_rate=48000", "-t", "18",
               "-c:v", "libx264", "-preset", "ultrafast", "-threads", "8",
               "-pix_fmt", "yuv420p", "-profile:v", "high", "-level:v", "5.2",
               "-coder", "1", "-8x8dct", "1", "-g", "360", "-keyint_min", "360",
               "-sc_threshold", "0", "-b:v", f"{args.mbps}M", "-minrate", f"{args.mbps}M",
               "-maxrate", f"{args.mbps}M", "-bufsize", f"{args.mbps * 2}M",
               "-x264-params", "nal-hrd=cbr:force-cfr=1", "-c:a", "aac", "-ac", "2",
               "-b:a", "128k", "-f", "hls", "-hls_time", "6", "-hls_list_size", "0",
               "-hls_flags", "independent_segments", "-hls_segment_filename",
               str(media / f"{name}-%d.ts"), str(playlist)]
    subprocess.run(command, check=True)
    probe = subprocess.check_output([
        "ffprobe", "-v", "error", "-show_streams", "-of", "json", str(playlist)
    ], text=True)
    video = next(s for s in json.loads(probe)["streams"] if s["codec_type"] == "video")
    assert (video["width"], video["height"], video["r_frame_rate"], video["pix_fmt"], video["profile"]) == (
        3840, 2160, "60/1", "yuv420p", "High")
    paths = [playlist, *sorted(media.glob(f"{name}-*.ts"))]
    report = {"generator_sha256": sha256(Path(__file__)), "command": command,
              "ffmpeg": subprocess.check_output(["ffmpeg", "-version"], text=True).splitlines()[0],
              "video": video,
              "files": [{"file": p.name, "bytes": p.stat().st_size, "sha256": sha256(p)} for p in paths]}
    playlist.with_suffix(".json").write_text(json.dumps(report, indent=2) + "\n")
    print(f"Fixture: /media/{playlist.name} (18 seconds; 3 six-second segments)")


if __name__ == "__main__":
    main()
