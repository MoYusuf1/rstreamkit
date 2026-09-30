//! Transmux local HLS segments into one fMP4 file, to inspect with ffprobe/mpv:
//!   cargo run --example transmux -- out.mp4 seg1.ts seg2.ts ...

use rstreamkit::Transmuxer;

fn main() {
    let mut args = std::env::args().skip(1);
    let out = args
        .next()
        .expect("usage: transmux <out.mp4> <segment.ts>...");
    let mut t = Transmuxer::default();
    let mut file = vec![];
    for path in args {
        let seg = std::fs::read(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
        let o = t.push(&seg).unwrap_or_else(|e| panic!("{path}: {e}"));
        if let Some(init) = o.init {
            println!("codecs: {}", init.mime);
            file.extend(init.bytes);
        }
        if let Some(a) = o.skipped_audio {
            println!("note: dropped {a} audio");
        }
        println!("{path}: {} bytes of fragment", o.fragment.len());
        file.extend(o.fragment);
    }
    std::fs::write(&out, file).expect("write output");
    println!("wrote {out}");
}
