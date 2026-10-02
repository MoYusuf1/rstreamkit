#!/usr/bin/env python3
"""Compare native CLI remuxing of identical H.264/AAC input, including startup and file I/O."""
import json, subprocess, tempfile, time, statistics, platform
from pathlib import Path
root=Path(__file__).resolve().parent.parent
source=root/'tools/media/aac.ts'
with tempfile.TemporaryDirectory(prefix='rtk-compare-') as directory:
    out=Path(directory)/'output.mp4'
    commands={
        'rstreamkit':[str(root/'target/release/examples/transmux'),str(out),str(source)],
        'ffmpeg':['ffmpeg','-v','error','-y','-i',str(source),'-map','0:v:0','-map','0:a:0','-c','copy','-bsf:a','aac_adtstoasc','-movflags','frag_keyframe+empty_moov','-f','mp4',str(out)],
    }
    times={name:[] for name in commands}; sizes={}; fingerprints={}
    for trial in range(9):
        # Alternate execution order to reduce systematic warm-cache bias.
        for name in (list(commands) if trial%2==0 else list(reversed(commands))):
            started=time.perf_counter();subprocess.run(commands[name],check=True,stdout=subprocess.DEVNULL,stderr=subprocess.PIPE);elapsed=(time.perf_counter()-started)*1000
            if trial:times[name].append(elapsed)
            sizes[name]=out.stat().st_size
            if trial==0:
                check=subprocess.run(['ffmpeg','-v','error','-i',str(out),'-f','null','-'],capture_output=True,check=True)
                if check.stderr:raise RuntimeError(check.stderr.decode())
                video=subprocess.run(['ffmpeg','-v','error','-i',str(out),'-map','0:v:0','-f','framemd5','-'],capture_output=True,check=True).stdout.decode()
                hashes=[line.split(',')[-1].strip() for line in video.splitlines() if line and not line.startswith('#')]
                audio=subprocess.run(['ffmpeg','-v','error','-i',str(out),'-map','0:a:0','-f','s16le','-'],capture_output=True,check=True).stdout
                fingerprints[name]=(hashes,audio)
    if fingerprints['rstreamkit']!=fingerprints['ffmpeg']:raise RuntimeError('decoded output differs between the tools')
    print(json.dumps({'platform':platform.platform(),'operation':'H.264/AAC TS to fragmented MP4; native CLI startup and file I/O included; one warmup, eight measured trials','input_bytes':source.stat().st_size,'decoded_video_frames':len(fingerprints['rstreamkit'][0]),'decoded_audio_bytes':len(fingerprints['rstreamkit'][1]),'decoded_output_equal':True,'results':{name:{'median_ms':statistics.median(t),'max_ms':max(t),'output_bytes':sizes[name]} for name,t in times.items()}}))
