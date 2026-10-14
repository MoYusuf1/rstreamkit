"""Small, local browser regression inputs. FFmpeg is a test tool only."""
from pathlib import Path
import shutil
import subprocess

root = Path(__file__).resolve().parents[2]
media = root / 'tools/media'
media.mkdir(exist_ok=True)
subprocess.run([
    'ffmpeg', '-hide_banner', '-loglevel', 'error', '-y',
    '-f', 'lavfi', '-i', 'testsrc2=size=320x180:rate=30',
    '-f', 'lavfi', '-i', 'sine=sample_rate=48000',
    '-t', '40', '-c:v', 'libx264', '-preset', 'ultrafast',
    '-g', '60', '-sc_threshold', '0', '-pix_fmt', 'yuv420p',
    '-c:a', 'aac', '-hls_time', '2', '-hls_list_size', '0',
    '-hls_segment_filename', str(media / 'browser%d.ts'),
    str(media / 'browser.m3u8'),
], check=True)
shutil.copyfile(root / 'tests/fixtures/h264_ac3_51.ts', media / 'browser-dolby.ts')
(media / 'browser-dolby.m3u8').write_text(
    '#EXTM3U\n#EXT-X-TARGETDURATION:3\n#EXTINF:2.005333,\n'
    'browser-dolby.ts\n#EXT-X-ENDLIST\n')
