//! Runs the real pipeline on a real HLS segment (Big Buck Bunny, first ~200 KB of a Mux test
//! stream segment: H.264 High 848x480 + AAC-LC 44.1 kHz).

use std::process::Command;

use rffmpeg::{Transmuxer, ts};

const SEGMENT: &[u8] = include_bytes!("fixtures/bbb_480p.ts");

fn tfdt(fragment: &[u8]) -> u64 {
    // The first traf in our fragments is the video track.
    let p = fragment
        .windows(4)
        .position(|w| w == b"tfdt")
        .expect("tfdt box");
    u64::from_be_bytes(fragment[p + 8..p + 16].try_into().unwrap())
}

#[test]
fn demuxes_video_and_audio_from_a_real_segment() {
    let d = ts::demux(SEGMENT).unwrap();
    assert!(
        d.video.len() > 100 && d.audio.len() > 100,
        "{} video / {} audio",
        d.video.len(),
        d.audio.len()
    );
    assert!(d.video[0].key, "segment must start on a keyframe");
    assert!(d.sps.is_some() && d.pps.is_some());
    assert_eq!(
        d.aac,
        Some(ts::AacConfig {
            object_type: 2,
            freq_index: 4,
            channels: 2
        })
    );
    // The source timestamps start ~10 s in; the muxer has to normalise that.
    assert!(
        d.video[0].dts > 800_000,
        "expected an offset start, got {}",
        d.video[0].dts
    );
    // Decode order: DTS never goes backwards, and B-frame PTS may lead DTS but never trail it.
    assert!(d.video.windows(2).all(|w| w[0].dts <= w[1].dts));
    assert!(d.video.iter().all(|s| s.pts >= s.dts));
}

#[test]
fn first_fragment_starts_at_zero_with_the_right_codecs() {
    let out = Transmuxer::default().push(SEGMENT).unwrap();
    let init = out.init.expect("first segment carries the init segment");
    assert_eq!(init.mime, "video/mp4; codecs=\"avc1.64001f,mp4a.40.2\"");
    assert_eq!(tfdt(&out.fragment), 0);
    assert!(out.skipped_audio.is_none());
}

#[test]
fn a_timestamp_jump_is_glued_onto_the_end_of_the_previous_fragment() {
    let d = ts::demux(SEGMENT).unwrap();
    let span = d.video.last().unwrap().dts - d.video[0].dts;
    assert!(
        span > 180_000,
        "fixture must be longer than the 2 s jump threshold"
    );

    // Pushing the same segment twice looks like a stream that restarted its clock.
    let mut t = Transmuxer::default();
    let first = t.push(SEGMENT).unwrap();
    let second = t.push(SEGMENT).unwrap();
    assert!(second.init.is_none());
    let start2 = tfdt(&second.fragment);
    assert!(
        start2.abs_diff(span) < 3 * 3003,
        "second fragment starts at {start2}, first spans {span}"
    );
    assert!(first.fragment.len() > 10_000 && second.fragment.len() > 10_000);
}

/// Every track's base time in a fragment (video first, then audio).
fn tfdts(fragment: &[u8]) -> Vec<u64> {
    fragment
        .windows(4)
        .enumerate()
        .filter(|(_, w)| *w == b"tfdt")
        .map(|(p, _)| u64::from_be_bytes(fragment[p + 8..p + 16].try_into().unwrap()))
        .collect()
}

/// A stream whose clock restarts below the media already sent (encoder restart, ad splice with reset
/// timestamps) must play on after it, not be written on top of what is buffered. Pushing one
/// segment over and over restarts the clock every time; once the sent media is longer than the
/// segment's own start time, the restarted clock is *below* it.
#[test]
fn a_clock_restarting_below_the_sent_media_still_plays_on() {
    let d = ts::demux(SEGMENT).unwrap();
    let span = d.video.last().unwrap().dts - d.video[0].dts;
    assert!(
        d.video[0].dts < 4 * span,
        "fixture must start early enough for the restart to fall below the sent media"
    );

    let mut t = Transmuxer::default();
    let mut previous: Option<Vec<u64>> = None;
    for push in 0..12 {
        let at = tfdts(&t.push(SEGMENT).unwrap().fragment);
        assert_eq!(at.len(), 2, "video and audio");
        if let Some(before) = &previous {
            for (track, (now, was)) in at.iter().zip(before).enumerate() {
                assert!(now > was, "push {push}, track {track}: {now} after {was}");
            }
            // Each segment follows the last, about one segment further on.
            assert!(
                at[0].abs_diff(before[0] + span) < 3 * 3003,
                "push {push}: video at {} after {}, a segment is {span}",
                at[0],
                before[0]
            );
        }
        previous = Some(at);
    }
}

/// Independent check: hand the muxed bytes to ffmpeg, which must decode every frame without complaint.
#[test]
fn ffmpeg_decodes_the_output_without_errors() {
    if Command::new("ffmpeg").arg("-version").output().is_err() {
        eprintln!("ffmpeg not installed; skipping");
        return;
    }
    let d = ts::demux(SEGMENT).unwrap();
    let mut t = Transmuxer::default();
    let a = t.push(SEGMENT).unwrap();
    let b = t.push(SEGMENT).unwrap();
    let file = std::env::temp_dir().join(format!("rffmpeg-test-{}.mp4", std::process::id()));
    std::fs::write(
        &file,
        [a.init.unwrap().bytes, a.fragment, b.fragment].concat(),
    )
    .unwrap();

    let decode = Command::new("ffmpeg")
        .args(["-v", "error", "-i"])
        .arg(&file)
        .args(["-f", "null", "-"])
        .output()
        .unwrap();
    let complaints = String::from_utf8_lossy(&decode.stderr).into_owned();
    assert!(
        decode.status.success() && complaints.trim().is_empty(),
        "ffmpeg said: {complaints}"
    );

    let probe = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-count_frames",
            "-select_streams",
            "v:0",
            "-show_entries",
            "stream=codec_name,width,height,nb_read_frames",
            "-of",
            "csv=p=0",
        ])
        .arg(&file)
        .output()
        .unwrap();
    let line = String::from_utf8_lossy(&probe.stdout).trim().to_owned();
    assert_eq!(
        line,
        format!("h264,848,480,{}", d.video.len() * 2),
        "ffprobe: {line}"
    );
    std::fs::remove_file(file).ok();
}

/// Anamorphic PAL (720x576 with 64:45 pixels) has to reach the browser as such, or it plays
/// stretched to 5:4 instead of 16:9. The init segment states it in a `pasp` box.
#[test]
fn anamorphic_pixels_are_declared_in_the_init_segment() {
    let init = Transmuxer::default()
        .push(include_bytes!("fixtures/pal_anamorphic.ts"))
        .unwrap()
        .init
        .unwrap()
        .bytes;
    let p = init
        .windows(4)
        .position(|w| w == b"pasp")
        .expect("pasp box");
    assert_eq!(&init[p + 4..p + 12], &[0, 0, 0, 64, 0, 0, 0, 45]);
    // Square-pixel streams stay as they were.
    let square = Transmuxer::default()
        .push(SEGMENT)
        .unwrap()
        .init
        .unwrap()
        .bytes;
    assert!(!square.windows(4).any(|w| w == b"pasp"));
}

fn ffmpeg_ok() -> bool {
    Command::new("ffmpeg")
        .arg("-version")
        .output()
        .is_ok_and(|o| o.status.success())
}

/// ffmpeg's own decode of a file's sound: stereo, 48 kHz, 16-bit.
fn ffmpeg_pcm(path: &std::path::Path) -> Vec<i16> {
    let out = Command::new("ffmpeg")
        .args(["-v", "error", "-i"])
        .arg(path)
        .args(["-vn", "-ac", "2", "-ar", "48000", "-f", "s16le", "-"])
        .output()
        .unwrap();
    out.stdout
        .as_chunks::<2>()
        .0
        .iter()
        .map(|&b| i16::from_le_bytes(b))
        .collect()
}

/// Signal-to-error ratio in dB, at the best alignment within a few thousand samples.
fn snr(ours: &[i16], reference: &[i16]) -> (f64, i64) {
    let n = ours.len().min(reference.len()).saturating_sub(12_000);
    let mut best = (f64::MIN, 0);
    // Smallest shifts first, and a later one must be clearly better: tones repeat, so many shifts
    // look alike and the true one is the nearest.
    let mut shifts: Vec<i64> = (-4096i64..=4096).step_by(2).collect();
    shifts.sort_by_key(|s| s.abs());
    for shift in shifts {
        let (mut signal, mut error) = (0f64, 0f64);
        for i in (4096..n).step_by(5) {
            let j = (i as i64 + shift) as usize;
            let (a, b) = (f64::from(ours[i]), f64::from(reference[j]));
            signal += b * b;
            error += (a - b) * (a - b);
        }
        let db = 10.0 * (signal / error.max(1.0)).log10();
        if db > best.0 + 0.05 {
            best = (db, shift);
        }
    }
    best
}

/// How alike two signals are, ignoring exact levels: correlation, and how much louder `a` is (dB).
fn compare_shape(a: &[i16], b: &[i16]) -> (f64, f64) {
    let n = a.len().min(b.len());
    let (mut ab, mut aa, mut bb) = (0f64, 0f64, 0f64);
    for i in 0..n {
        let (x, y) = (f64::from(a[i]), f64::from(b[i]));
        ab += x * y;
        aa += x * x;
        bb += y * y;
    }
    (
        ab / (aa * bb).sqrt().max(1.0),
        10.0 * (aa / bb.max(1.0)).log10(),
    )
}

/// AC-3, E-AC-3 and MP2 sound, which browsers can't decode, comes out of the transmuxer as FLAC
/// audio in the same MP4 as the picture, and sounds like what ffmpeg makes of the original.
#[test]
fn ac3_eac3_and_mp2_sound_is_decoded_to_flac_that_matches_ffmpeg() {
    if !ffmpeg_ok() {
        eprintln!("ffmpeg isn't installed: skipping");
        return;
    }
    let clips: [(&str, &[u8]); 4] = [
        ("h264_ac3_st.ts", include_bytes!("fixtures/h264_ac3_st.ts")),
        ("h264_ac3_51.ts", include_bytes!("fixtures/h264_ac3_51.ts")),
        (
            "h264_eac3_51.ts",
            include_bytes!("fixtures/h264_eac3_51.ts"),
        ),
        ("h264_mp2.ts", include_bytes!("fixtures/h264_mp2.ts")),
    ];
    for (name, bytes) in clips {
        let out = Transmuxer::default().push(bytes).unwrap();
        assert!(out.skipped_audio.is_none(), "{name}: sound was dropped");
        let init = out.init.unwrap();
        assert!(init.mime.ends_with(",flac\""), "{name}: {}", init.mime);

        let dir = std::env::temp_dir();
        let (ours_file, source_file) = (
            dir.join(format!("riptv-sound-{}-{name}.mp4", std::process::id())),
            dir.join(format!("riptv-sound-{}-{name}", std::process::id())),
        );
        std::fs::write(&ours_file, [init.bytes.as_slice(), &out.fragment].concat()).unwrap();
        std::fs::write(&source_file, bytes).unwrap();

        let codecs = Command::new("ffprobe")
            .args([
                "-v",
                "error",
                "-show_entries",
                "stream=codec_name",
                "-of",
                "csv=p=0",
            ])
            .arg(&ours_file)
            .output()
            .unwrap();
        let codecs = String::from_utf8_lossy(&codecs.stdout).into_owned();
        assert!(
            codecs.contains("h264") && codecs.contains("flac"),
            "{name}: {codecs}"
        );

        let (ours, reference) = (ffmpeg_pcm(&ours_file), ffmpeg_pcm(&source_file));
        assert!(
            ours.len() > 48_000,
            "{name}: only {} samples came out",
            ours.len()
        );
        let (db, shift) = snr(&ours, &reference);
        assert!(
            shift.abs() <= 2,
            "{name}: sound is {shift} samples out of step"
        );
        let (corr, level_db) = compare_shape(&ours, &reference);
        eprintln!(
            "{name}: {db:.1} dB against ffmpeg, correlation {corr:.3}, level {level_db:+.1} dB, {} samples",
            ours.len()
        );
        if name.contains("eac3") {
            // E-AC-3 is folded to stereo with the mix levels in its own metadata, which ffmpeg
            // weighs differently: the same sound at a similar level, not the same samples.
            assert!(
                corr > 0.85 && level_db.abs() < 6.0,
                "{name}: {corr:.3} {level_db:+.1} dB"
            );
        } else if name.contains("51") {
            // Our decoder folds AC-3 5.1 to stereo with the spec's downmix (§7.8, the mix levels the
            // stream carries). Recent ffmpeg does the same and matches to 40+ dB; an older one uses
            // fixed levels, and on CI that matched to 17 dB: the same sound at the same level
            // (correlation 0.991, +0.2 dB). Stereo AC-3 and MP2 have no downmix and stay strict.
            assert!(
                corr > 0.98 && level_db.abs() < 1.5,
                "{name}: {corr:.3} {level_db:+.1} dB"
            );
        } else {
            assert!(db > 40.0, "{name}: {db:.1} dB against ffmpeg's decode");
        }
        std::fs::remove_file(ours_file).ok();
        std::fs::remove_file(source_file).ok();
    }
}
