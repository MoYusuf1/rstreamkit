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
# Three keyframe groups per segment, so the first segment of a live playlist starts early.
subprocess.run([
    'ffmpeg', '-hide_banner', '-loglevel', 'error', '-y',
    '-f', 'lavfi', '-i', 'testsrc2=size=320x180:rate=30',
    '-f', 'lavfi', '-i', 'sine=sample_rate=48000',
    '-t', '18', '-c:v', 'libx264', '-preset', 'ultrafast',
    '-g', '60', '-sc_threshold', '0', '-pix_fmt', 'yuv420p',
    '-c:a', 'aac', '-hls_time', '6', '-hls_list_size', '0',
    '-hls_segment_filename', str(media / 'groups%d.ts'),
    str(media / 'groups.m3u8'),
], check=True)
# The same, AES-128 encrypted, with its own key (the IV comes from the sequence number).
(media / 'groups.key').write_bytes(bytes(range(16)))
(media / 'groups-key-info.txt').write_text(f"groups.key\n{media / 'groups.key'}\n")
subprocess.run([
    'ffmpeg', '-hide_banner', '-loglevel', 'error', '-y',
    '-f', 'lavfi', '-i', 'testsrc2=size=320x180:rate=30',
    '-f', 'lavfi', '-i', 'sine=sample_rate=48000',
    '-t', '18', '-c:v', 'libx264', '-preset', 'ultrafast',
    '-g', '60', '-sc_threshold', '0', '-pix_fmt', 'yuv420p',
    '-c:a', 'aac', '-hls_time', '6', '-hls_list_size', '0',
    '-hls_key_info_file', str(media / 'groups-key-info.txt'),
    '-hls_segment_filename', str(media / 'locked%d.ts'),
    str(media / 'locked.m3u8'),
], check=True)
shutil.copyfile(root / 'tests/fixtures/h264_ac3_51.ts', media / 'browser-dolby.ts')
(media / 'browser-dolby.m3u8').write_text(
    '#EXTM3U\n#EXT-X-TARGETDURATION:3\n#EXTINF:2.005333,\n'
    'browser-dolby.ts\n#EXT-X-ENDLIST\n')
