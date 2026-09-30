//! Writes a movie's init segment and fragments as one fMP4 file (for looking at with ffprobe).
//!   cargo run --example dump -- movie.mp4|movie.mkv OUT.mp4 [start seconds] [chunk bytes]
use std::rc::Rc;

use rffmpeg::{mkv, mp4, vod::Movie};

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let file = std::fs::read(&a[1]).unwrap();
    let size = file.len() as u64;
    let movie = if a[1].ends_with(".mkv") {
        let mut probe = mkv::Probe::new(size);
        let (mut at, mut len) = (0u64, 4096u64);
        loop {
            let bytes = &file[at as usize..(at + len).min(size) as usize];
            match probe.feed(at, bytes).unwrap() {
                mkv::Step::Read(x, l) => (at, len) = (x, l),
                mkv::Step::Done(p) => break Movie::from_mkv(*p, size),
            }
        }
    } else {
        let mut at = 0;
        loop {
            let bytes = &file[at..(at + 64).min(file.len())];
            match mp4::find_moov(bytes, at as u64) {
                mp4::Moov::At(off, len) => {
                    let (off, len) = (off as usize, len as usize);
                    let (_, _, head) = mp4::box_header(&file[off..]).unwrap();
                    break Movie::from_mp4(&file[off + head..off + len], size).unwrap();
                }
                mp4::Moov::Next(to) => at = to as usize,
                mp4::Moov::Missing => panic!("no moov"),
            }
        }
    };
    let movie = Rc::new(movie);
    let from: f64 = a.get(3).and_then(|s| s.parse().ok()).unwrap_or(0.0);
    let chunk: u64 = a.get(4).and_then(|s| s.parse().ok()).unwrap_or(300_000);
    let mut out = movie.init().unwrap().bytes;
    let mut s = movie.session(from);
    while let Some((start, len)) = s.range(chunk) {
        let bytes = &file[start as usize..((start + len) as usize).min(file.len())];
        out.extend(s.push(bytes).unwrap());
    }
    std::fs::write(&a[2], out).unwrap();
    println!(
        "{} ({}) duration {:.2}s shift {}",
        movie.video.name,
        movie.container == rffmpeg::vod::Container::Mp4,
        movie.duration,
        movie.shift()
    );
}
