use rstreamkit_fuzz::{MAX_INPUT, exercise};
use std::{env, fs, path::PathBuf};

fn next(state: &mut u64) -> u64 {
    *state ^= *state << 13;
    *state ^= *state >> 7;
    *state ^= *state << 17;
    *state
}

fn main() {
    let args: Vec<String> = env::args().collect();
    if args[1] == "seed" {
        let root = PathBuf::from(&args[2]);
        let write = |target: &str, name: &str, bytes: &[u8]| {
            let dir = root.join(target);
            fs::create_dir_all(&dir).unwrap();
            fs::write(dir.join(name), bytes).unwrap();
        };
        write("hls", "live", b"#EXTM3U\n#EXT-X-TARGETDURATION:2\n#EXT-X-MEDIA-SEQUENCE:42\n#EXTINF:2,\nfirst.ts\n#EXTINF:2,\nsecond.ts\n");
        write(
            "hls",
            "invalid-time",
            b"#EXTM3U\n#EXTINF:NaN,\nsegment.ts\n",
        );
        write("hls", "overflow", b"#EXTM3U\n#EXT-X-MEDIA-SEQUENCE:18446744073709551615\n#EXTINF:2,\na.ts\n#EXTINF:2,\nb.ts\n");
        let ts = include_bytes!("../../../tests/fixtures/h264_ac3_st.ts");
        write("ts", "transport", ts);
        write("ts", "garbage", b"<html>offline</html>");
        let movie = include_bytes!("../../../tests/fixtures/movie_aac.mp4");
        if let rstreamkit::mp4::Moov::At(at, size) = rstreamkit::mp4::find_moov(movie, 0) {
            let moov = &movie[at as usize..(at + size) as usize];
            let (_, _, head) = rstreamkit::mp4::box_header(moov).unwrap();
            write("mp4", "index", &moov[head..]);
        } else {
            panic!("seed has no moov");
        }
        write("mp4", "offset-overflow", b"\0\0\0\x08free\0\0\0\x08moov");
        write(
            "mkv",
            "movie",
            include_bytes!("../../../tests/fixtures/movie_ac3.mkv"),
        );
        let out = rstreamkit::Transmuxer::default()
            .decode_sound(false)
            .push(ts)
            .unwrap();
        let fragment = out.fragment();
        let init = out.init.unwrap().bytes;
        let mut cmaf = (init.len() as u32).to_le_bytes().to_vec();
        cmaf.extend_from_slice(&init);
        cmaf.extend_from_slice(&fragment);
        write("cmaf", "fragment", &cmaf);
        write("cmaf", "init", &init);
        return;
    }
    if args[1] == "replay" {
        exercise(&args[2], &fs::read(&args[3]).unwrap());
        return;
    }
    let target = &args[1];
    let cases: usize = args[2].parse().unwrap();
    let seed: u64 = args[3].parse().unwrap();
    assert_ne!(seed, 0);
    let corpus = PathBuf::from(&args[4]);
    let pending = PathBuf::from(&args[5]);
    let mut paths: Vec<_> = fs::read_dir(corpus)
        .unwrap()
        .map(|p| p.unwrap().path())
        .collect();
    paths.sort();
    let inputs: Vec<_> = paths.iter().map(|p| fs::read(p).unwrap()).collect();
    assert!(!inputs.is_empty() && inputs.iter().all(|b| b.len() <= MAX_INPUT));
    let mut state = seed;
    for case in 0..cases {
        let mut data = inputs[case % inputs.len()].clone();
        if case >= inputs.len() {
            for _ in 0..1 + next(&mut state) % 8 {
                match next(&mut state) % 5 {
                    0 if !data.is_empty() => {
                        let at = next(&mut state) as usize % data.len();
                        data[at] ^= 1 << (next(&mut state) % 8);
                    }
                    1 => {
                        data.truncate(next(&mut state) as usize % (data.len() + 1));
                    }
                    2 if data.len() < MAX_INPUT => {
                        let at = next(&mut state) as usize % (data.len() + 1);
                        data.insert(at, next(&mut state) as u8);
                    }
                    3 if data.len() >= 8 => {
                        let at = next(&mut state) as usize % (data.len() - 7);
                        data[at..at + 8].copy_from_slice(&next(&mut state).to_be_bytes());
                    }
                    _ => {}
                }
            }
        }
        // Persist BEFORE calling the parser: panics, OOM and timeouts leave a replay input.
        fs::write(&pending, &data).unwrap();
        println!("case={case} seed={seed} bytes={}", data.len());
        exercise(target, &data);
    }
    fs::remove_file(pending).unwrap();
    println!("PASS target={target} cases={cases} seed={seed}");
}
