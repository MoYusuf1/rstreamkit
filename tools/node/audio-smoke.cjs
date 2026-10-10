// Run the browser-targeted decoder in Node: native tests cannot catch unsupported wasm APIs.
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const kit = require('./pkg/rstreamkit_harness.js');

for (const fixture of ['h264_ac3.ts', 'h264_ac3_51.ts', 'h264_eac3_51.ts', 'h264_mp2.ts']) {
    const bytes = fs.readFileSync(path.join(__dirname, '../../tests/fixtures', fixture));
    const audio = new kit.ProfileAudio(bytes);
    try {
        assert(audio.raw_size() > 0, `${fixture}: no decoded PCM`);
        assert(audio.encode() > 0, `${fixture}: no FLAC output`);
        assert(kit.transmux(bytes, true, 1) > 0, `${fixture}: no packaged output`);
        console.log(`${fixture}: wasm decode, FLAC and packaging passed`);
    } finally {
        audio.free();
    }
}
