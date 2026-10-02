"""Generate disposable media; no downloaded or generated media is committed."""
from pathlib import Path
import subprocess
import hashlib, json
ROOT = Path(__file__).resolve().parent / "media"
ROOT.mkdir(exist_ok=True)
GENERATOR = hashlib.sha256(Path(__file__).read_bytes()).hexdigest()
def run(name, args):
    path = ROOT / name
    # A cached movie from older generator settings must not silently change a benchmark.
    inputs = {}
    for i, arg in enumerate(args[:-1]):
        if arg == '-i':
            source = Path(args[i+1])
            if source.is_file(): inputs[str(source)] = hashlib.sha256(source.read_bytes()).hexdigest()
    signature = json.dumps({'generator': GENERATOR, 'args': args, 'inputs': inputs}, sort_keys=True)
    marker = ROOT / (name + '.args.json')
    if not path.exists() or not marker.exists() or marker.read_text() != signature:
        try:
            subprocess.run(["ffmpeg", "-hide_banner", "-loglevel", "error", "-y", *args, str(path)], check=True)
            marker.write_text(signature)
        except subprocess.CalledProcessError:
            path.unlink(missing_ok=True)
            raise
video = ["-f", "lavfi", "-i", "testsrc2=size=1920x1080:rate=30"]
for codec in ["aac", "ac3"]:
    run(codec + ".ts", [*video, "-f", "lavfi", "-i", "sine=frequency=440:sample_rate=48000", "-t", "6", "-c:v", "libx264", "-preset", "ultrafast", "-b:v", "8M", "-minrate", "8M", "-maxrate", "8M", "-bufsize", "16M", "-g", "60", "-c:a", codec, "-ac", "6" if codec == "ac3" else "2", "-b:a", "448k" if codec == "ac3" else "128k", "-f", "mpegts"])
run("live.m3u8", ["-f", "lavfi", "-i", "testsrc2=size=320x180:rate=30", "-f", "lavfi", "-i", "sine=sample_rate=48000", "-t", "24", "-c:v", "libx264", "-preset", "ultrafast", "-g", "60", "-c:a", "aac", "-hls_time", "2", "-hls_list_size", "0", "-hls_segment_filename", str(ROOT / "live%d.ts")])
run("short.mp4", ["-f", "lavfi", "-i", "testsrc2=size=320x180:rate=24", "-f", "lavfi", "-i", "sine=sample_rate=48000", "-t", "6", "-c:v", "libx264", "-preset", "ultrafast", "-c:a", "ac3"])
for ext in ["mp4", "mkv"]:
    run("movie." + ext, ["-stream_loop", "299", "-i", str(ROOT / "short.mp4"), "-t", "1800", "-c", "copy"])
(ROOT / "ac3.m3u8").write_text('#EXTM3U\n#EXT-X-TARGETDURATION:6\n#EXTINF:6,\nac3.ts\n#EXT-X-ENDLIST\n')

run("cmaf.m3u8", ["-i", str(ROOT / "live.m3u8"), "-c", "copy", "-bsf:a", "aac_adtstoasc", "-hls_segment_type", "fmp4", "-hls_time", "2", "-hls_list_size", "0", "-hls_segment_filename", str(ROOT / "cmaf%d.m4s"), "-hls_fmp4_init_filename", "cmaf-init.mp4"])
run("radio.m3u8", ["-i", str(ROOT / "live.m3u8"), "-map", "0:a", "-c", "copy", "-hls_time", "2", "-hls_list_size", "0", "-hls_segment_filename", str(ROOT / "radio%d.ts")])
run("video.m3u8", ["-i", str(ROOT / "live.m3u8"), "-map", "0:v", "-c", "copy", "-hls_time", "2", "-hls_list_size", "0", "-hls_segment_filename", str(ROOT / "video%d.ts")])
(ROOT / "demuxed.m3u8").write_text('#EXTM3U\n#EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID="sound",NAME="English",LANGUAGE="eng",DEFAULT=YES,URI="radio.m3u8"\n#EXT-X-STREAM-INF:BANDWIDTH=500000,CODECS="avc1.42c00c,mp4a.40.2",AUDIO="sound"\nvideo.m3u8\n')
key = bytes(range(16)); (ROOT / "key.bin").write_bytes(key)
(ROOT / "key-info.txt").write_text("key.bin\n" + str(ROOT / "key.bin") + "\n00000000000000000000000000000001\n")
run("encrypted.m3u8", ["-i", str(ROOT / "live.m3u8"), "-c", "copy", "-hls_time", "2", "-hls_list_size", "0", "-hls_key_info_file", str(ROOT / "key-info.txt"), "-hls_segment_filename", str(ROOT / "encrypted%d.ts")])
run("programme.ts", ["-i", str(ROOT.parent.parent / "tests/fixtures/bbb_480p.ts"), "-c:v", "copy", "-c:a", "ac3", "-b:a", "192k", "-f", "mpegts"])

run("short-aac.mp4", ["-i", str(ROOT / "aac.ts"), "-c", "copy"])
run("movie-aac.mp4", ["-stream_loop", "299", "-i", str(ROOT / "short-aac.mp4"), "-t", "1800", "-c", "copy"])
# CMAF byte ranges into a single resource; subsequent ranges omit offsets per RFC 8216.
init=(ROOT/"cmaf-init.mp4").read_bytes();blob=bytearray(init)
playlist=['#EXTM3U','#EXT-X-TARGETDURATION:2',f'#EXT-X-MAP:URI="ranges.mp4",BYTERANGE="{len(init)}@0"']
for i in range(12):
    chunk=(ROOT/f"cmaf{i}.m4s").read_bytes();offset=f'@{len(blob)}' if i==0 else ''
    playlist.extend(['#EXTINF:2,',f'#EXT-X-BYTERANGE:{len(chunk)}{offset}','ranges.mp4']);blob.extend(chunk)
playlist.append('#EXT-X-ENDLIST');(ROOT/'ranges.mp4').write_bytes(blob);(ROOT/'ranges.m3u8').write_text('\n'.join(playlist)+'\n')
(ROOT/'appearing.m3u8').write_text('#EXTM3U\n#EXT-X-TARGETDURATION:2\n#EXTINF:2,\nvideo0.ts\n#EXTINF:2,\nlive1.ts\n#EXTINF:2,\nlive2.ts\n#EXT-X-ENDLIST\n')
run('changed.ts', ['-f', 'lavfi', '-i', 'testsrc2=size=640x360:rate=30', '-f', 'lavfi', '-i', 'sine=sample_rate=44100', '-t', '2', '-c:v', 'libx264', '-preset', 'ultrafast', '-g', '60', '-c:a', 'aac', '-ac', '2', '-f', 'mpegts'])
(ROOT/'changing.m3u8').write_text('#EXTM3U\n#EXT-X-TARGETDURATION:2\n#EXTINF:2,\nlive0.ts\n#EXT-X-DISCONTINUITY\n#EXTINF:2,\nchanged.ts\n#EXT-X-DISCONTINUITY\n#EXTINF:2,\nlive0.ts\n#EXT-X-ENDLIST\n')

for name in ['hevc_ac3.ts','interlaced_576i.ts']:
    (ROOT/name).write_bytes((ROOT.parent.parent/'tests'/'fixtures'/name).read_bytes())
    stem=name.removesuffix('.ts')
    (ROOT/(stem+'.m3u8')).write_text(f'#EXTM3U\n#EXT-X-TARGETDURATION:6\n#EXTINF:6,\n{name}\n#EXT-X-ENDLIST\n')
