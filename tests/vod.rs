//! Movie files: a real MP4 and a real MKV (H.264 with B-frames, 5.1 AC-3 sound, 6 s, made by
//! ffmpeg) are read the way the browser reads them, a range at a time, and the result is handed
//! back to ffmpeg to decode.

use std::{process::Command, rc::Rc};

use rstreamkit::{
    mkv, mp4,
    vod::{Audio, Container, Movie, Verdict},
};

const MP4: &[u8] = include_bytes!("fixtures/movie_ac3.mp4");
const MKV: &[u8] = include_bytes!("fixtures/movie_ac3.mkv");

fn load_mp4(file: &[u8]) -> Movie {
    // A small window, so the search for `moov` takes more than one step.
    let mut at = 0;
    loop {
        let bytes = &file[at..(at + 64).min(file.len())];
        match mp4::find_moov(bytes, at as u64) {
            mp4::Moov::At(off, len) => {
                let (off, len) = (off as usize, len as usize);
                let (_, _, head) = mp4::box_header(&file[off..]).unwrap();
                return Movie::from_mp4(&file[off + head..off + len], file.len() as u64).unwrap();
            }
            mp4::Moov::Next(to) => at = to as usize,
            mp4::Moov::Missing => panic!("no moov"),
        }
    }
}

fn load_mkv(file: &[u8]) -> Movie {
    let size = file.len() as u64;
    let mut probe = mkv::Probe::new(size);
    let (mut at, mut len) = (0u64, 4096u64);
    loop {
        let bytes = &file[at as usize..(at + len).min(size) as usize];
        match probe.feed(at, bytes).unwrap() {
            mkv::Step::Read(a, l) => (at, len) = (a, l),
            mkv::Step::Done(parsed) => return Movie::from_mkv(*parsed, size),
        }
    }
}

/// The init segment and every fragment of a session that starts at `at`, reading `chunk` bytes at a time.
fn play(movie: &Rc<Movie>, file: &[u8], at: f64, chunk: u64) -> Vec<u8> {
    let mut out = movie.init().unwrap().bytes;
    let mut session = movie.session(at);
    let mut asks = 0;
    while let Some((start, len)) = session.range(chunk) {
        let bytes = &file[start as usize..((start + len) as usize).min(file.len())];
        out.extend(session.push(bytes).unwrap());
        asks += 1;
        assert!(asks < 1000, "the session never ends");
    }
    assert!(session.done());
    out
}

fn ffmpeg_ok() -> bool {
    Command::new("ffmpeg")
        .arg("-version")
        .output()
        .is_ok_and(|o| o.status.success())
}

/// What ffprobe says about the decoded file: complaints from ffmpeg decoding it, then for each
/// stream (video, audio) its codec, frames read, first timestamp and end timestamp.
fn decoded(bytes: &[u8], name: &str) -> (String, Vec<(String, u32, f64, f64)>) {
    let path = std::env::temp_dir().join(format!("rstreamkit-{name}-{}.mp4", std::process::id()));
    std::fs::write(&path, bytes).unwrap();
    let decode = Command::new("ffmpeg")
        .args(["-v", "error", "-i"])
        .arg(&path)
        .args(["-f", "null", "-"])
        .output()
        .unwrap();
    let probe = Command::new("ffprobe")
        .args(["-v", "error", "-count_frames", "-show_entries"])
        .arg("stream=codec_name,start_time,duration,nb_read_frames")
        .args(["-of", "csv=p=0"])
        .arg(&path)
        .output()
        .unwrap();
    std::fs::remove_file(path).ok();
    let streams = String::from_utf8_lossy(&probe.stdout)
        .lines()
        .map(|l| {
            let f: Vec<&str> = l.trim().split(',').collect();
            (
                f[0].to_owned(),
                f[3].parse().unwrap_or(0),
                f[1].parse().unwrap_or(f64::NAN),
                f[2].parse().unwrap_or(f64::NAN),
            )
        })
        .collect();
    (
        String::from_utf8_lossy(&decode.stderr).trim().to_owned(),
        streams,
    )
}

#[test]
fn an_mp4_is_understood_and_ac3_is_ours_to_play() {
    let movie = load_mp4(MP4);
    assert_eq!(movie.container, Container::Mp4);
    assert_eq!(
        (
            movie.video.name.as_str(),
            movie.video.width,
            movie.video.height
        ),
        ("H.264", 320, 180)
    );
    let audio = movie.audio.as_ref().unwrap();
    assert_eq!((audio.name.as_str(), audio.rate), ("AC-3", 48_000));
    assert!(matches!(audio.audio, Audio::Sound(_)));
    assert!((movie.duration - 6.0).abs() < 0.1, "{}", movie.duration);

    // A browser that can't play AC-3 gets it from us; one that can, plays the file itself.
    let no_ac3 = |t: &str| !t.contains("ac-3");
    assert_eq!(movie.verdict(&no_ac3), Verdict::Rust);
    assert_eq!(movie.verdict(&|_| true), Verdict::Native);
    // Whatever the browser says, what we'd be playing is H.264, decoded sound in FLAC.
    let mime = movie.init().unwrap().mime;
    assert!(
        mime.starts_with("video/mp4; codecs=\"avc1.") && mime.ends_with(",flac\""),
        "{mime}"
    );
    // The picture's first frame is shown two frames late in this file (an edit list), which the
    // player takes off again.
    assert!(
        movie.shift() < 0.0 && movie.shift() > -0.2,
        "{}",
        movie.shift()
    );
}

#[test]
fn an_mkv_is_understood_the_same_way() {
    let movie = load_mkv(MKV);
    assert_eq!(movie.container, Container::Matroska);
    assert_eq!(
        (movie.video.name.as_str(), movie.video.width),
        ("H.264", 320)
    );
    assert_eq!(movie.audio.as_ref().unwrap().name, "AC-3");
    assert!((movie.duration - 6.0).abs() < 0.1, "{}", movie.duration);
    assert_eq!(movie.verdict(&|t| !t.contains("ac-3")), Verdict::Rust);
    assert_eq!(movie.shift(), -1.0, "room for the picture to be reordered");
}

#[test]
fn a_whole_mp4_plays_through_ffmpeg() {
    if !ffmpeg_ok() {
        return eprintln!("ffmpeg not installed; skipping");
    }
    let movie = Rc::new(load_mp4(MP4));
    // Small chunks, so pieces end in the middle of the interleaving.
    for chunk in [20_000, 300_000] {
        let (complaints, streams) = decoded(&play(&movie, MP4, 0.0, chunk), "mp4");
        assert_eq!(complaints, "", "chunk {chunk}");
        let (video, audio) = (&streams[0], &streams[1]);
        assert_eq!((video.0.as_str(), video.1), ("h264", 144), "chunk {chunk}");
        assert_eq!(audio.0, "flac");
        // Every AC-3 frame became one FLAC frame: 6 s of sound, and it lines up with the picture.
        assert!(
            (audio.3 - audio.2 - 6.0).abs() < 0.1,
            "sound lasts {}",
            audio.3 - audio.2
        );
        assert!(
            (audio.2 - video.2).abs() < 0.05,
            "sound starts at {}, picture at {}",
            audio.2,
            video.2
        );
    }
}

#[test]
fn a_whole_mkv_plays_through_ffmpeg() {
    if !ffmpeg_ok() {
        return eprintln!("ffmpeg not installed; skipping");
    }
    let movie = Rc::new(load_mkv(MKV));
    for chunk in [20_000, 300_000] {
        let (complaints, streams) = decoded(&play(&movie, MKV, 0.0, chunk), "mkv");
        assert_eq!(complaints, "", "chunk {chunk}");
        let (video, audio) = (&streams[0], &streams[1]);
        assert_eq!((video.0.as_str(), video.1), ("h264", 144), "chunk {chunk}");
        assert_eq!(audio.0, "flac");
        assert!(
            (audio.3 - audio.2 - 6.0).abs() < 0.1,
            "sound lasts {}",
            audio.3 - audio.2
        );
        assert!(
            (audio.2 - video.2).abs() < 0.05,
            "sound starts at {}, picture at {}",
            audio.2,
            video.2
        );
    }
}

/// Seeking is starting a session later: at the keyframe before the moment (here every 2 s), with
/// the sound from there, and nothing of what came before.
#[test]
fn a_seek_starts_at_the_keyframe_before_it() {
    if !ffmpeg_ok() {
        return eprintln!("ffmpeg not installed; skipping");
    }
    for (name, movie, file) in [("mp4", load_mp4(MP4), MP4), ("mkv", load_mkv(MKV), MKV)] {
        let movie = Rc::new(movie);
        let (complaints, streams) = decoded(&play(&movie, file, 3.0, 50_000), name);
        assert_eq!(complaints, "", "{name}");
        let (video, audio) = (&streams[0], &streams[1]);
        // From 2 s on: 4 s of the 6 s clip is 96 frames.
        assert_eq!(video.1, 96, "{name}");
        let shift = -movie.shift();
        assert!(
            (video.2 - (2.0 + shift)).abs() < 0.12,
            "{name}: the picture starts at {} (2 s plus {shift})",
            video.2
        );
        assert!(
            (audio.2 - video.2).abs() < 0.06 && (audio.3 - audio.2 - 4.0).abs() < 0.1,
            "{name}: sound from {} to {}",
            audio.2,
            audio.3
        );
    }
}

#[test]
fn a_file_the_index_can_not_be_read_from_is_refused_not_misplayed() {
    // Not a movie at all.
    assert!(matches!(
        mp4::find_moov(&[0; 64], 0),
        mp4::Moov::Next(_) | mp4::Moov::Missing
    ));
    assert!(Movie::from_mp4(&[0, 0, 0, 8, b'f', b'r', b'e', b'e'], 8).is_err());
    let mut probe = mkv::Probe::new(MKV.len() as u64);
    assert!(probe.feed(0, &[0xFF; 64]).is_err());
}
