"""Local CORS/range server and scripted live provider. python3 tools/server.py"""
from http.server import ThreadingHTTPServer, SimpleHTTPRequestHandler
from pathlib import Path
from urllib.parse import urlsplit, parse_qs
import json, time
ROOT = Path(__file__).resolve().parent
START = time.monotonic()

def shift(data, ticks):
    data = bytearray(data)
    for at in range(0, len(data)-187, 188):
        p = memoryview(data)[at:at+188]
        afc = (p[3] >> 4) & 3
        off = 4 if afc == 1 else 5 + p[4] if afc == 3 else 188
        if off+19 >= 188 or not p[1] & 64 or bytes(p[off:off+3]) != b'\0\0\1': continue
        flags = p[off+7] >> 6
        for i, present in [(off+9, flags & 2), (off+14, flags == 3)]:
            if not present: continue
            t = ((p[i] & 14) << 29) | (p[i+1] << 22) | ((p[i+2] & 254) << 14) | (p[i+3] << 7) | (p[i+4] >> 1)
            t = (t + ticks) % (1 << 33)
            p[i] = (p[i] & 240) | ((t >> 30) & 7) << 1 | 1
            p[i+1] = (t >> 22) & 255; p[i+2] = ((t >> 15) & 127) << 1 | 1
            p[i+3] = (t >> 7) & 255; p[i+4] = (t & 127) << 1 | 1
    return bytes(data)

class Handler(SimpleHTTPRequestHandler):
    def __init__(self, *args, **kwargs): super().__init__(*args, directory=str(ROOT / 'web'), **kwargs)
    def end_headers(self):
        self.send_header('Access-Control-Allow-Origin', '*')
        self.send_header('Access-Control-Expose-Headers', 'Content-Range,Content-Length')
        super().end_headers()
    def send(self, data, kind='application/octet-stream', code=200):
        self.send_response(code); self.send_header('Content-Type', kind)
        self.send_header('Content-Length', str(len(data))); self.end_headers()
        try: self.wfile.write(data)
        except (BrokenPipeError, ConnectionResetError): pass
    def do_GET(self):
        url = urlsplit(self.path); q = parse_qs(url.query)
        if 'delay' in q: time.sleep(min(float(q['delay'][0]), 20))
        if url.path == '/raw.ts':
            self.send_response(200);self.send_header('Content-Type','video/mp2t');self.end_headers()
            data=(ROOT/'media'/'live0.ts').read_bytes()
            try:
                for i in range(7200):
                    self.wfile.write(shift(data,i*180000));self.wfile.flush();time.sleep(2)
            except (BrokenPipeError,ConnectionResetError):pass
            return
        if url.path.startswith('/fault/') or url.path.startswith('/high/'):
            mode = url.path.split('/')[-1].split('.')[0]
            elapsed = time.monotonic() - START
            if mode == 'outage' and 10 < elapsed % 60 < 20: return self.send(b'outage', code=503)
            duration = 6 if url.path.startswith('/high/') else 2
            n = int(elapsed / duration)
            if mode == 'frozen' and 10 < elapsed % 60 < 24: n = int(elapsed // 60 * 30 + 5)
            first = max(0, n-5)
            entries = []
            for i in range(first, n+1):
                path = '/segment/' + str(i) + '.ts' + ('?high=1' if duration==6 else '')
                if mode == '404' and i % 10 == 5: path = '/missing.ts'
                if mode == 'garbage' and i % 10 == 5: path = '/garbage.ts'
                entries.append(f'#EXTINF:{duration},\n' + path + '\n')
            seq = 0 if mode == 'no-sequence' else first
            return self.send((f'#EXTM3U\n#EXT-X-TARGETDURATION:{duration}\n#EXT-X-MEDIA-SEQUENCE:' + str(seq) + '\n' + ''.join(entries)).encode(), 'application/vnd.apple.mpegurl')
        if url.path == '/garbage.ts': return self.send(b'<html>offline</html>')
        if url.path.startswith('/segment/'):
            try: i = int(url.path.split('/')[-1].split('.')[0])
            except ValueError: return self.send(b'invalid', code=400)
            high='high' in q
            data = (ROOT/'media'/('aac.ts' if high else 'live0.ts')).read_bytes()
            return self.send(shift(data, i*(540000 if high else 180000)), 'video/mp2t')
        if url.path.startswith('/media/'):
            name = Path(url.path).name
            path = ROOT/'media'/name
            if not path.is_file(): return self.send(b'missing', code=404)
            size = path.stat().st_size
            range_header = self.headers.get('Range')
            if range_header:
                try:
                    a,b = range_header.removeprefix('bytes=').split('-'); start = int(a); end = min(int(b) if b else size-1, size-1)
                    if start > end or start < 0: raise ValueError()
                except ValueError: return self.send(b'invalid range', code=416)
                self.send_response(206); self.send_header('Content-Range', f'bytes {start}-{end}/{size}')
                self.send_header('Content-Length', str(end-start+1)); self.end_headers()
                with path.open('rb') as f: f.seek(start); data = f.read(end-start+1)
                try: self.wfile.write(data)
                except (BrokenPipeError, ConnectionResetError): pass
                return
            kind = 'application/vnd.apple.mpegurl' if name.endswith('.m3u8') else 'application/octet-stream'
            return self.send(path.read_bytes(), kind)
        super().do_GET()
if __name__ == '__main__':
    print('Test bed: http://127.0.0.1:8765', flush=True)
    ThreadingHTTPServer(('127.0.0.1', 8765), Handler).serve_forever()
