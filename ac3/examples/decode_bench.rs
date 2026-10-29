//! Times AC-3/E-AC-3 decoding in-process and fingerprints the output, so a speed change can be
//! checked for byte-identical PCM.
//!
//! `cargo run --release --example decode_bench -- <file.ac3|file.eac3> [channels] [runs]`
//! prints the best and median wall time over `runs` (default 10) fresh decodes and an FNV-1a hash
//! of every decoded byte from one more, untimed decode. `channels` (default 2) is the requested
//! output layout.

use std::time::Instant;

use oxideav_core::{CodecId, CodecParameters, Frame, Packet, TimeBase};
use rstreamkit_ac3::{decoder::make_decoder, syncinfo};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let path = args
        .get(1)
        .expect("usage: decode_bench <file> [channels] [runs]");
    let channels: u16 = args.get(2).map_or(2, |s| s.parse().expect("channels"));
    let runs: usize = args.get(3).map_or(10, |s| s.parse().expect("runs"));
    let bytes = std::fs::read(path).expect("read input");
    let eac3 = path.ends_with(".eac3") || path.ends_with(".ec3");

    // Split the elementary stream into syncframes once, outside the timed loop.
    let mut frames = Vec::new();
    let mut at = 0;
    while at + 8 <= bytes.len() {
        let len = if eac3 {
            // E-AC-3 frmsiz: 11 bits after the syncword and strmtyp/substreamid, in 16-bit words.
            ((usize::from(bytes[at + 2] & 0x07) << 8 | usize::from(bytes[at + 3])) + 1) * 2
        } else {
            syncinfo::parse(&bytes[at..])
                .expect("syncframe")
                .frame_length as usize
        };
        frames.push(&bytes[at..(at + len).min(bytes.len())]);
        at += len;
    }

    let mut times = Vec::with_capacity(runs);
    let mut hash = 0;
    // The last pass is untimed and only hashes, so hashing does not dilute the timings.
    for run in 0..=runs {
        let mut params = CodecParameters::audio(CodecId::new(if eac3 { "eac3" } else { "ac3" }));
        params.channels = Some(channels);
        let mut dec = make_decoder(&params).expect("decoder");
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        let start = Instant::now();
        for f in &frames {
            dec.send_packet(&Packet::new(0, TimeBase::new(1, 48_000), f.to_vec()))
                .expect("send");
            if let Ok(Frame::Audio(a)) = dec.receive_frame() {
                if run == runs {
                    for b in a.data.iter().flatten() {
                        h = (h ^ u64::from(*b)).wrapping_mul(0x0100_0000_01b3);
                    }
                }
            }
        }
        if run < runs {
            times.push(start.elapsed().as_secs_f64() * 1e3);
        }
        hash = h;
    }
    times.sort_by(f64::total_cmp);
    println!(
        "{path}: {} frames, best {:.2} ms, median {:.2} ms, pcm fnv {hash:016x}",
        frames.len(),
        times[0],
        times[times.len() / 2]
    );
}
