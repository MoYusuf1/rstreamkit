// Run the browser-targeted decoder in Node: native tests cannot catch unsupported wasm APIs.
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const kit = require('./pkg/rstreamkit_harness.js');

(async () => {
for (const fixture of ['h264_ac3.ts', 'h264_ac3_51.ts', 'h264_eac3_51.ts', 'h264_mp2.ts']) {
    const bytes = fs.readFileSync(path.join(__dirname, '../../tests/fixtures', fixture));
    const audio = new kit.ProfileAudio(bytes);
    try {
        assert(audio.raw_size() > 0, `${fixture}: no decoded PCM`);
        assert(audio.encode() > 0, `${fixture}: no FLAC output`);
        assert(kit.transmux(bytes, true, 1) > 0, `${fixture}: no packaged output`);
        assert.equal(await kit.continuous_yielded(bytes, 16384), kit.continuous_sync(bytes, 16384),
            fixture + ': yielded continuous output differs');
        console.log(`${fixture}: wasm decode, FLAC and packaging passed`);
    } finally {
        audio.free();
    }
}

const movie = fs.readFileSync(path.join(__dirname, '../../tests/fixtures/movie_ac3.mp4'));
let moov;
for (let at = 0; at + 8 <= movie.length;) {
    let size = movie.readUInt32BE(at), header = 8;
    if (size === 1) { size = Number(movie.readBigUInt64BE(at + 8)); header = 16; }
    if (size === 0) size = movie.length - at;
    if (movie.toString('ascii', at + 4, at + 8) === 'moov') {
        moov = movie.subarray(at + header, at + size); break;
    }
    assert(size >= header); at += size;
}
assert(moov, 'movie index exists');
const outputs = [];
for (const method of ['push', 'push_yielded']) {
    const session = kit.ProfileSession.mp4(moov, BigInt(movie.length));
    try {
        let range;
        while ((range = session.range())) {
            const [at, len] = Array.from(range, Number);
            await session[method](movie.subarray(at, Math.min(at + len, movie.length)));
        }
        outputs.push([session.output_checksum(), session.output_bytes()]);
    } finally { session.free(); }
}
assert.deepEqual(outputs[0], outputs[1], 'movie yielded output differs');
console.log('movie_ac3.mp4: Node synchronous/yielded output bytes match');

})().catch(error => { console.error(error); process.exitCode = 1; });
