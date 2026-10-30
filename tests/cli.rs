//! The command-line tool, run the way a user runs it and judged by ffmpeg: a remux must hand back
//! the very pictures it was given, and everything it writes must decode without complaint.
use std::{
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

const BIN: &str = env!("CARGO_BIN_EXE_rstreamkit");

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

/// The `framemd5` hash of every decoded picture, in order.
fn pictures(path: &Path) -> Vec<String> {
    let out = Command::new("ffmpeg")
        .args(["-v", "error", "-i"])
        .arg(path)
        .args(["-map", "0:v:0", "-f", "framemd5", "-"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", path.display());
    let hashes: Vec<String> = String::from_utf8(out.stdout)
        .unwrap()
        .lines()
        .filter(|l| !l.starts_with('#'))
        .map(|l| l.rsplit(',').next().unwrap().trim().to_owned())
        .collect();
    assert!(
        !hashes.is_empty(),
        "{}: no pictures decoded",
        path.display()
    );
    hashes
}

/// Decodes everything in `path`, sound included; ffmpeg must have nothing to say about it.
fn decodes_cleanly(path: &Path) {
    let out = Command::new("ffmpeg")
        .args(["-v", "error", "-i"])
        .arg(path)
        .args(["-f", "null", "-"])
        .output()
        .unwrap();
    let errors = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success() && errors.is_empty(),
        "{}: {errors}",
        path.display()
    );
}

fn convert(input: &Path, output: &Path) {
    let status = Command::new(BIN).arg(input).arg(output).status().unwrap();
    assert!(status.success(), "{}", input.display());
}

#[test]
fn every_kind_of_input_comes_out_with_the_same_pictures() {
    if Command::new("ffmpeg").arg("-version").output().is_err() {
        eprintln!("ffmpeg not installed; skipping");
        return;
    }
    let dir = std::env::temp_dir().join(format!("rstreamkit-cli-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();

    // A TS file, an MP4 movie (AAC, copied) and a Matroska one (AC-3, decoded to FLAC).
    for name in ["bbb_480p.ts", "movie_aac.mp4", "movie_ac3.mkv"] {
        let out = dir.join(format!("{name}.mp4"));
        convert(&fixture(name), &out);
        assert_eq!(pictures(&out), pictures(&fixture(name)), "{name}");
        decodes_cleanly(&out);
    }

    // A local playlist that plays the segment twice across a discontinuity.
    let ts = fixture("bbb_480p.ts");
    let playlist = dir.join("list.m3u8");
    let ts_uri = ts.display();
    std::fs::write(
        &playlist,
        format!(
            "#EXTM3U\n#EXT-X-TARGETDURATION:5\n#EXTINF:4,\n{ts_uri}\n\
             #EXT-X-DISCONTINUITY\n#EXTINF:4,\n{ts_uri}\n#EXT-X-ENDLIST\n"
        ),
    )
    .unwrap();
    let out = dir.join("list.mp4");
    convert(&playlist, &out);
    let once = pictures(&ts);
    assert_eq!(pictures(&out), [once.clone(), once].concat());
    decodes_cleanly(&out);

    // An fMP4 (CMAF) playlist, as FFmpeg writes one: an init map and .m4s fragments.
    let cmaf = dir.join("cmaf");
    std::fs::create_dir_all(&cmaf).unwrap();
    let made = Command::new("ffmpeg")
        .args(["-v", "error", "-y", "-i"])
        .arg(fixture("movie_aac.mp4"))
        .args([
            "-c",
            "copy",
            "-f",
            "hls",
            "-hls_time",
            "2",
            "-hls_playlist_type",
            "vod",
        ])
        .args([
            "-hls_segment_type",
            "fmp4",
            "-hls_fmp4_init_filename",
            "init.mp4",
        ])
        .arg(cmaf.join("index.m3u8"))
        .status()
        .unwrap();
    assert!(made.success());
    let out = dir.join("cmaf.mp4");
    convert(&cmaf.join("index.m3u8"), &out);
    assert_eq!(pictures(&out), pictures(&fixture("movie_aac.mp4")));
    decodes_cleanly(&out);

    // Piped through, standard input to standard output: the same bytes as file to file.
    let mut child = Command::new(BIN)
        .args(["-", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(&std::fs::read(&ts).unwrap())
        .unwrap();
    let piped = child.wait_with_output().unwrap();
    assert!(piped.status.success());
    assert_eq!(
        piped.stdout,
        std::fs::read(dir.join("bbb_480p.ts.mp4")).unwrap()
    );

    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn what_it_cannot_read_is_refused_with_a_reason() {
    let dir = std::env::temp_dir().join(format!("rstreamkit-cli-bad-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let junk = dir.join("junk.bin");
    std::fs::write(&junk, b"<html>offline</html>").unwrap();
    let out = Command::new(BIN)
        .arg(&junk)
        .arg(dir.join("out.mp4"))
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("isn't MPEG-TS"));
    let usage = Command::new(BIN).output().unwrap();
    assert_eq!(usage.status.code(), Some(2));

    // Writing over what is being read would destroy it.
    let copy = dir.join("same.ts");
    std::fs::copy(fixture("bbb_480p.ts"), &copy).unwrap();
    let same = Command::new(BIN).arg(&copy).arg(&copy).output().unwrap();
    assert!(!same.status.success());
    assert_eq!(
        std::fs::read(&copy).unwrap(),
        std::fs::read(fixture("bbb_480p.ts")).unwrap()
    );
    // A missing INPUT is reported as missing, not mistaken for the same file as a missing OUTPUT.
    let missing = Command::new(BIN)
        .arg(dir.join("absent.ts"))
        .arg(dir.join("absent.mp4"))
        .output()
        .unwrap();
    assert!(String::from_utf8_lossy(&missing.stderr).contains("No such file"));
    assert!(!dir.join("absent.mp4").exists());
    std::fs::remove_dir_all(&dir).unwrap();
}
